//! Writing and removing music tags.
//!
//! The counterpart to [`super::audio`], and the only place in the engine that
//! changes what is *inside* a file. Everything here is irreversible (P2), so it
//! is also the most defensive code in the crate.
//!
//! Four things about lofty 0.25 shape this module. All four were established by
//! running it, not by reading its documentation, and each one is a way a
//! reasonable implementation silently destroys somebody's music.
//!
//! 1. **A save replaces the whole tag.** Writing a `Tag` carrying only an
//!    artist wipes the title, album, genre, comment, track *and any embedded
//!    cover art*. Our contract is the opposite — a field whose box is clear is
//!    not written and whatever the file already carries is left intact — so
//!    every write here is a read-modify-write, and the read **clones the
//!    existing tag** rather than copying fields into a fresh one.
//!
//!    Cloning the generic `Tag` is enough only where lofty keeps what its
//!    `ItemKey` table cannot name: ID3v2 and MP4 carry that in a companion
//!    the generic API cannot see, so a clone keeps an MP4's cover and a
//!    `TXXX` frame. **Vorbis comments and APE have no companion.** Their
//!    generic view is a split that throws the unnamed half away — a FLAC
//!    rip's `CUESHEET`, a user's own key, an APE cover stored as a binary
//!    item — and saving it rebuilds the whole block from what is left. So
//!    those two are read as their concrete tag, split, edited, and merged
//!    back with the remainder they came with ([`Native`]).
//!
//! 2. **`TagType::remove_from_path` does not work.** On every format tried it
//!    returns `Err("failed to write to file")` — `EINVAL` — and changes
//!    nothing. Saving an *empty* tag of the same type does the removal exactly,
//!    leaving the audio byte-identical.
//!
//! 3. **An empty tag of an unsupported type panics.** The writability guard is
//!    `!is_writable() && !self.is_empty()`, so an empty tag skips it and
//!    reaches a format writer that dispatches on the *file's* type: an empty
//!    `Id3v1` tag saved to a FLAC hits `unreachable!("tag type verified
//!    beforehand")`. Asking for the same thing on an Ogg silently wipes the
//!    Vorbis comments instead. Nothing here calls a writer without first asking
//!    [`lofty::file::FileType::tag_support`].
//!
//! 4. **The format must be decided from content, once.** `read_from_path`
//!    trusts the extension and `save_to_path` verifies from content, and for
//!    Musepack SV4-6 they disagree — the read says Musepack, the write says
//!    MPEG. One content-derived `FileType` is carried through both halves here.

use std::path::Path;

use lofty::ape::ApeTag;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OggPictureStorage, OpusFile, SpeexFile, VorbisFile};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, MergeTag, SplitTag, Tag, TagItem, TagType};
use serde::{Deserialize, Serialize};

use super::lyrics3;

/// The seven fields the Music Tag Writer offers, in the dialog's own order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicField {
    Track,
    Title,
    Artist,
    Album,
    Year,
    Genre,
    Comment,
}

impl MusicField {
    /// Top to bottom as the dialog has them.
    pub const ALL: [Self; 7] = [
        Self::Track,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Year,
        Self::Genre,
        Self::Comment,
    ];

    /// The dialog's captions. `Comm`, not `Comment` — that is what it says.
    pub fn label(self) -> &'static str {
        match self {
            Self::Track => "Track",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::Genre => "Genre",
            Self::Comment => "Comm",
        }
    }

    fn key(self) -> ItemKey {
        match self {
            Self::Track => ItemKey::TrackNumber,
            Self::Title => ItemKey::TrackTitle,
            Self::Artist => ItemKey::TrackArtist,
            Self::Album => ItemKey::AlbumTitle,
            Self::Year => ItemKey::RecordingDate,
            Self::Genre => ItemKey::Genre,
            Self::Comment => ItemKey::Comment,
        }
    }

    /// Whether a value is fit to write, and what to write.
    ///
    /// Only `Track` is fussy, and it has to be. lofty parses the track
    /// numerically on the way out, and what it does with something that is not
    /// a number is neither consistent nor safe: on an MP3 `"3/12"` and `"A1"`
    /// are dropped, on a FLAC `"3/12"` becomes `"3"` and `"A1"` survives, and
    /// over a tag that already has a numeric track **a non-numeric value is
    /// written as `0`**. A user mapping a filename part to Track on a vinyl rip
    /// would get track 0 across the album, silently, with no undo.
    ///
    /// Whether this value will survive being written, so the operation can say
    /// so in the preview rather than leaving the user to find out.
    pub fn writable(self, value: &str) -> bool {
        self.normalise(value).is_some()
    }

    /// So the value is normalised here or it is not written at all.
    fn normalise(self, value: &str) -> Option<String> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if self != Self::Track {
            return Some(value.to_owned());
        }
        // `007` is a legitimate way to type 7; `3/12` and `A1` are not numbers.
        let digits = value.trim_start_matches('0');
        let digits = if digits.is_empty() { "0" } else { digits };
        digits
            .chars()
            .all(|c| c.is_ascii_digit())
            .then(|| digits.to_owned())
    }
}

/// The ID3v1 genre table — 192 entries, the standard list plus the Winamp
/// extensions.
///
/// lofty's own ID3v1 writer maps a genre string against exactly this table, and
/// a genre outside it is written as *absent*: the tag lands, the genre is
/// silently gone. So the dropdown offers this list and nothing else, and a
/// genre picked from it is guaranteed to survive.
///
/// A shorter or misspelt list would be a *bug* rather than a behaviour: a
/// genre outside the numeric table vanishes on an ID3v1 write. Taking the
/// standard list is also what D6 asks for: our own data.
pub const GENRES: &[&str] = &lofty::id3::v1::GENRES;

/// One field to write.
///
/// A journal payload, so no `deny_unknown_fields`: a later build adding a field
/// here must not make this one call the journal corrupt (D73).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldWrite {
    pub field: MusicField,
    pub value: String,
}

/// The tag blocks Remove Tags offers, as the dialog lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagKind {
    Id3v1,
    Id3v2,
    /// *"Lyrics v1 & v2"* — one checkbox covering both.
    Lyrics3,
}

impl TagKind {
    pub const ALL: [Self; 3] = [Self::Id3v1, Self::Id3v2, Self::Lyrics3];

    pub fn label(self) -> &'static str {
        match self {
            Self::Id3v1 => "ID3v1",
            Self::Id3v2 => "ID3v2",
            Self::Lyrics3 => "Lyrics v1 & v2",
        }
    }

    fn tag_type(self) -> Option<TagType> {
        match self {
            Self::Id3v1 => Some(TagType::Id3v1),
            Self::Id3v2 => Some(TagType::Id3v2),
            // Not a `TagType` at all — lofty cannot see one. See `lyrics3`.
            Self::Lyrics3 => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("this is not a music file, or its format is not one we can write")]
    NotAudio,
    #[error("{0}")]
    Lofty(String),
    #[error("{0}")]
    Lyrics(#[from] lyrics3::Error),
}

/// The format, decided **from content**, and its primary tag type.
///
/// Content rather than extension because the two disagree in practice — for
/// Musepack SV4-6 lofty's extension path says Musepack and its content path
/// says MPEG, and since a save always verifies from content, a write decided by
/// extension goes through the wrong writer.
fn primary(path: &Path) -> Result<(FileType, TagType), WriteError> {
    let file_type = Probe::open(path)
        .map_err(|e| WriteError::Lofty(e.to_string()))?
        .guess_file_type()
        .map_err(|e| WriteError::Lofty(e.to_string()))?
        .file_type()
        .ok_or(WriteError::NotAudio)?;
    let tag_type = file_type.primary_tag_type();
    if !file_type.tag_support(tag_type).is_readable() {
        return Err(WriteError::NotAudio);
    }
    Ok((file_type, tag_type))
}

/// The block to write into: the one that is there, or a new one **seeded from
/// whatever else the file carries**.
///
/// The seeding is the part that is easy to leave out and expensive to leave
/// out. A legacy MP3 with only an ID3v1 trailer — title, artist, album, year,
/// track — gets an ID3v2 written, because that is the policy (D70). Build that
/// block from the one field the user ticked and every player that prefers
/// ID3v2, which is most of them, now shows a file that has lost everything
/// else. The ID3v1 block still holds it all, so nothing is *gone*; it is
/// simply invisible, which is worse, because it looks like the tagger ate it.
///
/// `re_map` re-types the clone, and the generic `ItemKey`s carry across
/// unchanged — that is what makes them format-agnostic.
fn existing_or_seeded(tagged: &lofty::file::TaggedFile, tag_type: TagType) -> Tag {
    if let Some(existing) = tagged.tag(tag_type) {
        return existing.clone();
    }
    match tagged.tags().iter().find(|t| t.tag_type() != tag_type) {
        Some(other) => {
            let mut seed = other.clone();
            seed.re_map(tag_type);
            seed
        }
        None => Tag::new(tag_type),
    }
}

/// The key `field` uses inside `tag_type`.
///
/// One exception, and it is invisible without checking. A cloned ID3v1 tag
/// exposes its year as [`ItemKey::Year`], and lofty's ID3v1 writer reads that
/// key first — so inserting [`ItemKey::RecordingDate`] beside it writes the new
/// year into the block and the writer keeps using the old one. The user ticks
/// Year, sets 1988, and the trailer still says 1975.
fn key_in(field: MusicField, tag_type: TagType) -> ItemKey {
    match (field, tag_type) {
        (MusicField::Year, TagType::Id3v1) => ItemKey::Year,
        _ => field.key(),
    }
}

/// Whether this value survives being written into this kind of block.
///
/// ID3v1 stores the track in a **single byte**. Handing it 1994 — one
/// mis-mapped Setup Parts slot away, from a name like `1994 - Song.mp3` — does
/// not fail: it writes **0** over whatever real track number was there. The
/// ID3v2 block takes the value happily, so the field is written there and
/// skipped here, leaving the existing byte alone rather than zeroing it.
fn fits(field: MusicField, value: &str, tag_type: TagType) -> bool {
    if field != MusicField::Track || tag_type != TagType::Id3v1 {
        return true;
    }
    matches!(value.parse::<u32>(), Ok(n) if (1..=255).contains(&n))
}

/// Sets one field in a generic tag.
///
/// Every field but one is a plain replace. **Comment is the exception**,
/// because an ID3v2 tag holds several `COMM` frames told apart by their
/// description, and only the one with *no* description is the comment a
/// player shows. iTunes keeps its Sound Check (`iTunNORM`) and gapless
/// (`iTunSMPB`) data in described ones. lofty's `insert_text` removes every
/// item under the key, so it would delete those too; this removes only the
/// undescribed comment. Every other format's comments have no description,
/// where the two behave the same.
fn put(tag: &mut Tag, key: ItemKey, value: String) {
    if key == ItemKey::Comment {
        tag.retain(|item| !(item.key() == ItemKey::Comment && item.description().is_empty()));
        tag.push(TagItem::new(ItemKey::Comment, ItemValue::Text(value)));
    } else {
        tag.insert_text(key, value);
    }
}

/// A lofty (or I/O) failure, as the text the row shows.
fn lofty(error: impl std::fmt::Display) -> WriteError {
    WriteError::Lofty(error.to_string())
}

/// The primary tag in its own format, for the two formats whose generic view
/// loses what it cannot name (point 1 of the module docs).
enum Native {
    /// A FLAC, kept whole: its pictures are blocks of their own beside the
    /// comments, and saving the comments alone would drop them. The `ID3v2`
    /// tag lofty tolerates on a FLAC is taken off the in-memory copy, because
    /// a FLAC save writes an ID3v2 by *removing* it from the file.
    Flac(Box<FlacFile>),
    /// An Ogg's comment header — Vorbis, Opus and Speex alike.
    Ogg(VorbisComments),
    /// WavPack, Musepack and Monkey's Audio.
    Ape(ApeTag),
}

impl Native {
    /// The concrete primary tag, or `None` for a format that needs no such
    /// care — or a file that has no primary tag yet, where there is nothing
    /// unnamed to keep and the generic path seeds a new block.
    fn read(path: &Path, file_type: FileType) -> Result<Option<Self>, WriteError> {
        let mut file = std::fs::File::open(path).map_err(lofty)?;
        let options = ParseOptions::new().read_properties(false);
        Ok(match file_type {
            FileType::Flac => {
                let mut flac = FlacFile::read_from(&mut file, options).map_err(lofty)?;
                if flac.vorbis_comments().is_none() {
                    return Ok(None);
                }
                flac.remove_id3v2();
                Some(Self::Flac(Box::new(flac)))
            }
            FileType::Vorbis => Some(Self::Ogg(
                VorbisFile::read_from(&mut file, options)
                    .map_err(lofty)?
                    .remove_vorbis_comments(),
            )),
            FileType::Opus => Some(Self::Ogg(
                OpusFile::read_from(&mut file, options)
                    .map_err(lofty)?
                    .remove_vorbis_comments(),
            )),
            FileType::Speex => Some(Self::Ogg(
                SpeexFile::read_from(&mut file, options)
                    .map_err(lofty)?
                    .remove_vorbis_comments(),
            )),
            FileType::WavPack => lofty::wavpack::WavPackFile::read_from(&mut file, options)
                .map_err(lofty)?
                .remove_ape()
                .map(Self::Ape),
            FileType::Mpc => lofty::musepack::MpcFile::read_from(&mut file, options)
                .map_err(lofty)?
                .remove_ape()
                .map(Self::Ape),
            FileType::Ape => lofty::ape::ApeFile::read_from(&mut file, options)
                .map_err(lofty)?
                .remove_ape()
                .map(Self::Ape),
            _ => None,
        })
    }

    /// Sets `fields` through lofty's own split and merge, and saves.
    ///
    /// The split hands the fields it can name to a generic `Tag` and keeps the
    /// rest; the merge puts the edited `Tag` back beside that rest. Pictures
    /// inside Vorbis comments are lifted out first and put back as they were:
    /// the merge re-derives each picture's dimensions from its bytes and drops
    /// one it cannot measure, where leaving them alone keeps every one.
    fn write(self, path: &Path, fields: &[(MusicField, String)]) -> Result<(), WriteError> {
        let edit = |tag: &mut Tag| {
            for (field, value) in fields {
                put(tag, key_in(*field, tag.tag_type()), value.clone());
            }
        };
        let edit_comments = |mut comments: VorbisComments| {
            let pictures = comments.remove_pictures();
            let (rest, mut tag) = comments.split_tag();
            edit(&mut tag);
            let mut merged = rest.merge_tag(tag);
            for (picture, info) in pictures {
                // With the information supplied this cannot fail; it only
                // declines a second icon, which the format forbids anyway.
                let _ = merged.insert_picture(picture, Some(info));
            }
            merged
        };
        match self {
            Self::Flac(mut flac) => {
                let comments = flac.remove_vorbis_comments().unwrap_or_default();
                flac.set_vorbis_comments(edit_comments(comments));
                flac.save_to_path(path, WriteOptions::default())
                    .map_err(lofty)
            }
            Self::Ogg(comments) => edit_comments(comments)
                .save_to_path(path, WriteOptions::default())
                .map_err(lofty),
            Self::Ape(ape) => {
                let (rest, mut tag) = ape.split_tag();
                edit(&mut tag);
                rest.merge_tag(tag)
                    .save_to_path(path, WriteOptions::default())
                    .map_err(lofty)
            }
        }
    }
}

/// Writes `fields` into `path`, leaving every other field alone.
///
/// Returns the fields that were actually written — a value that will not
/// normalise is skipped rather than written wrong, and the caller says so.
pub fn write_fields(path: &Path, fields: &[FieldWrite]) -> Result<Vec<MusicField>, WriteError> {
    let (file_type, tag_type) = primary(path)?;

    let tagged = Probe::open(path)
        .map_err(|e| WriteError::Lofty(e.to_string()))?
        .guess_file_type()
        .map_err(|e| WriteError::Lofty(e.to_string()))?
        .read()
        .map_err(|e| WriteError::Lofty(e.to_string()))?;

    let wanted: Vec<(MusicField, String)> = fields
        .iter()
        .filter_map(|FieldWrite { field, value }| {
            let value = field.normalise(value)?;
            fits(*field, &value, tag_type).then_some((*field, value))
        })
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    match Native::read(path, file_type)? {
        Some(native) => native.write(path, &wanted)?,
        None => {
            // **Clone, never rebuild.** Copying the enabled fields into a
            // fresh `Tag` is the obvious implementation of per-field
            // checkboxes and it silently deletes embedded cover art on every
            // format — on MP4 through a companion the generic API does not
            // even show.
            let mut tag = existing_or_seeded(&tagged, tag_type);
            for (field, value) in &wanted {
                put(&mut tag, key_in(*field, tag_type), value.clone());
            }
            tag.save_to_path(path, WriteOptions::default())
                .map_err(|e| WriteError::Lofty(e.to_string()))?;
        }
    }
    let written = wanted.iter().map(|(field, _)| *field).collect();

    // The user's choice, in "update mode": ID3v2 always,
    // ID3v1 only where one is already there. A file that has a v1 trailer keeps
    // it accurate for players that read it first; a file without one is never
    // given a lossy tag it did not ask for — v1 truncates at 30 characters and
    // turns anything outside latin-1 into `?`, both silently.
    if file_type == FileType::Mpeg
        && let Some(existing) = tagged.tag(TagType::Id3v1).cloned()
    {
        let mut v1 = existing;
        for FieldWrite { field, value } in fields {
            if let Some(value) = field.normalise(value)
                && fits(*field, &value, TagType::Id3v1)
            {
                put(&mut v1, key_in(*field, TagType::Id3v1), value);
            }
        }
        v1.save_to_path(path, WriteOptions::default())
            .map_err(|e| WriteError::Lofty(e.to_string()))?;
    }

    super::audio::forget_all();
    Ok(written)
}

/// Removes `kinds` from `path`. Returns what was actually removed.
///
/// Only what is present is removed: a kind that is not there is not an
/// error, and neither is a kind this format cannot carry.
pub fn remove_tags(path: &Path, kinds: &[TagKind]) -> Result<Vec<TagKind>, WriteError> {
    let (file_type, _) = primary(path)?;
    let mut removed = Vec::new();

    for kind in kinds {
        let Some(tag_type) = kind.tag_type() else {
            // Lyrics3, which lofty cannot see. Only MP3 carries one.
            if file_type == FileType::Mpeg && lyrics3::remove(path)? {
                removed.push(*kind);
            }
            continue;
        };

        // **The guard that stops this crashing.** An empty tag skips lofty's
        // writability check and reaches a writer that dispatches on the file's
        // format: an empty ID3v1 saved to a FLAC panics outright, and the same
        // request on an Ogg wipes the Vorbis comments and returns `Ok`.
        if !file_type.tag_support(tag_type).is_readable() {
            continue;
        }

        // Present-check before every strip, not only for tidiness. Stripping an
        // MP4 that has already been stripped deletes the mandatory `hdlr` box
        // and leaves a file lofty itself can no longer parse — every write
        // returning `Ok` the whole way down.
        let tagged = Probe::open(path)
            .map_err(|e| WriteError::Lofty(e.to_string()))?
            .guess_file_type()
            .map_err(|e| WriteError::Lofty(e.to_string()))?
            .read()
            .map_err(|e| WriteError::Lofty(e.to_string()))?;
        if tagged.tag(tag_type).is_none() {
            continue;
        }

        Tag::new(tag_type)
            .save_to_path(path, WriteOptions::default())
            .map_err(|e| WriteError::Lofty(e.to_string()))?;
        removed.push(*kind);
    }

    super::audio::forget_all();
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::testing::Mp3;
    use tempfile::TempDir;

    fn field(field: MusicField, value: &str) -> FieldWrite {
        FieldWrite {
            field,
            value: value.to_owned(),
        }
    }

    fn tags(path: &Path) -> std::sync::Arc<crate::meta::audio::AudioTags> {
        crate::meta::audio::forget_all();
        crate::meta::audio::tags_of(path).expect("still readable audio")
    }

    /// The contract: a field whose box is clear is not written, and the value
    /// the file already has, if any, is left intact.
    ///
    /// This is the test the whole module exists for. lofty's save replaces the
    /// tag wholesale, so the obvious implementation — build a `Tag`, set the
    /// enabled fields, save — passes a round-trip check on the fields it wrote
    /// and destroys every other one.
    #[test]
    fn writing_one_field_leaves_every_other_field_intact() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("OldArtist", "OldTitle")
            .frame("TALB", "OldAlbum")
            .frame("TCON", "Jazz")
            .frame("COMM", "OldComment")
            .frame("TRCK", "7")
            .write(dir.path(), "t.mp3");

        let written = write_fields(&path, &[field(MusicField::Artist, "NewArtist")]).unwrap();
        assert_eq!(written, [MusicField::Artist]);

        let after = tags(&path);
        assert_eq!(after.artist.as_deref(), Some("NewArtist"));
        assert_eq!(after.title.as_deref(), Some("OldTitle"), "title was wiped");
        assert_eq!(after.album.as_deref(), Some("OldAlbum"), "album was wiped");
        assert_eq!(after.genre.as_deref(), Some("Jazz"), "genre was wiped");
        assert_eq!(after.comment.as_deref(), Some("OldComment"));
        assert_eq!(after.track.as_deref(), Some("7"), "track was wiped");
    }

    /// Whether `needle` occurs anywhere in `path`'s bytes.
    fn contains(path: &Path, needle: &[u8]) -> bool {
        std::fs::read(path)
            .unwrap()
            .windows(needle.len())
            .any(|w| w == needle)
    }

    /// Comment is the one field an MP3 can hold several of. iTunes keeps its
    /// gapless-playback (`iTunSMPB`) and Sound Check (`iTunNORM`) data in
    /// `COMM` frames of their own, told apart by their description, so
    /// writing the comment a player shows must leave those alone — lofty's
    /// generic insert replaces every `COMM` at once.
    #[test]
    fn writing_the_comment_leaves_the_described_comment_frames_alone() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B")
            .frame("COMM:iTunNORM", " 00000A2F 00000B3C")
            .frame("COMM:iTunSMPB", " 00000000 00000210")
            .frame("COMM", "old comment")
            .write(dir.path(), "itunes.mp3");

        let written = write_fields(&path, &[field(MusicField::Comment, "new comment")]).unwrap();
        assert_eq!(written, [MusicField::Comment]);

        assert!(contains(&path, b"iTunNORM"), "Sound Check was deleted");
        assert!(contains(&path, b" 00000A2F 00000B3C"));
        assert!(contains(&path, b"iTunSMPB"), "the gapless data was deleted");
        assert!(!contains(&path, b"old comment"), "the old comment stayed");
        assert_eq!(tags(&path).comment.as_deref(), Some("new comment"));
    }

    /// A file with no tag at all gets one.
    #[test]
    fn a_file_with_no_tag_gets_one() {
        let dir = TempDir::new().unwrap();
        let path = Mp3 {
            audio: true,
            ..Default::default()
        }
        .write(dir.path(), "bare.mp3");

        write_fields(
            &path,
            &[
                field(MusicField::Artist, "A"),
                field(MusicField::Title, "B"),
            ],
        )
        .unwrap();
        let after = tags(&path);
        assert_eq!(after.artist.as_deref(), Some("A"));
        assert_eq!(after.title.as_deref(), Some("B"));
    }

    /// Every field, round-tripped through our own reader.
    #[test]
    fn every_field_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = Mp3 {
            audio: true,
            ..Default::default()
        }
        .write(dir.path(), "all.mp3");

        write_fields(
            &path,
            &[
                field(MusicField::Track, "4"),
                field(MusicField::Title, "One"),
                field(MusicField::Artist, "Metallica"),
                field(MusicField::Album, "And Justice For All"),
                field(MusicField::Year, "1988"),
                field(MusicField::Genre, "Metal"),
                field(MusicField::Comment, "a comment"),
            ],
        )
        .unwrap();

        let after = tags(&path);
        assert_eq!(after.track.as_deref(), Some("4"));
        assert_eq!(after.title.as_deref(), Some("One"));
        assert_eq!(after.artist.as_deref(), Some("Metallica"));
        assert_eq!(after.album.as_deref(), Some("And Justice For All"));
        assert_eq!(after.year.as_deref(), Some("1988"));
        assert_eq!(after.genre.as_deref(), Some("Metal"));
        assert_eq!(after.comment.as_deref(), Some("a comment"));
    }

    /// The track trap, pinned at the level that can still do something about
    /// it. lofty writes `0` over a real track number when handed something it
    /// cannot parse, so a vinyl rip's `A1` would zero a whole album.
    #[test]
    fn a_track_that_is_not_a_number_is_not_written_at_all() {
        let dir = TempDir::new().unwrap();
        for bad in ["A1", "3/12", "one", "-4", " "] {
            let path = Mp3::tagged("A", "B")
                .frame("TRCK", "7")
                .write(dir.path(), "t.mp3");
            let written = write_fields(&path, &[field(MusicField::Track, bad)]).unwrap();
            assert!(written.is_empty(), "{bad:?} was written");
            assert_eq!(
                tags(&path).track.as_deref(),
                Some("7"),
                "{bad:?} disturbed the track that was already there"
            );
        }
    }

    #[test]
    fn a_track_written_with_leading_zeros_is_normalised_here_not_by_lofty() {
        let dir = TempDir::new().unwrap();
        for (given, want) in [("007", "7"), ("4", "4"), ("0", "0"), (" 12 ", "12")] {
            let path = Mp3::tagged("A", "B").write(dir.path(), "t.mp3");
            write_fields(&path, &[field(MusicField::Track, given)]).unwrap();
            assert_eq!(tags(&path).track.as_deref(), Some(want), "{given:?}");
        }
    }

    /// An enabled field whose template rendered to nothing writes nothing.
    /// lofty's own behaviour here is destructive — an empty value *drops* the
    /// field — so "enabled but empty" must never reach it.
    #[test]
    fn an_empty_value_is_skipped_rather_than_clearing_the_field() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("Keep", "Me").write(dir.path(), "t.mp3");
        let written = write_fields(&path, &[field(MusicField::Artist, "")]).unwrap();
        assert!(written.is_empty());
        assert_eq!(tags(&path).artist.as_deref(), Some("Keep"));
    }

    /// The user's choice, in update mode.
    #[test]
    fn id3v1_is_refreshed_where_it_exists_and_never_created_where_it_does_not() {
        let dir = TempDir::new().unwrap();

        let mut with_v1 = Mp3::tagged("OldArtist", "OldTitle");
        with_v1.id3v1 = Some((
            "V1Title".into(),
            "V1Artist".into(),
            "V1Album".into(),
            "1999".into(),
            None,
        ));
        let path = with_v1.write(dir.path(), "hasv1.mp3");
        write_fields(&path, &[field(MusicField::Artist, "NewArtist")]).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(
            &raw[raw.len() - 128..][..3],
            b"TAG",
            "the v1 trailer went away"
        );
        assert!(
            String::from_utf8_lossy(&raw[raw.len() - 128..]).contains("NewArtist"),
            "the v1 trailer still says the old artist"
        );

        let path = Mp3::tagged("OldArtist", "OldTitle").write(dir.path(), "nov1.mp3");
        write_fields(&path, &[field(MusicField::Artist, "NewArtist")]).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert!(
            raw.len() < 128 || &raw[raw.len() - 128..][..3] != b"TAG",
            "a lossy v1 tag was created where there was none"
        );
    }

    #[test]
    fn a_file_that_is_not_audio_is_an_error_rather_than_a_silent_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"this is not a music file at all").unwrap();
        assert!(matches!(
            write_fields(&path, &[field(MusicField::Artist, "A")]),
            Err(WriteError::NotAudio) | Err(WriteError::Lofty(_))
        ));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"this is not a music file at all"
        );
    }

    // --- Removal ------------------------------------------------------------

    /// The acceptance criterion: *"untagger leaves audio stream intact
    /// (byte-compare past tag blocks)"*.
    #[test]
    fn removing_every_tag_leaves_the_audio_byte_identical() {
        let dir = TempDir::new().unwrap();
        let mut mp3 = Mp3::tagged("A", "B");
        mp3.id3v1 = Some(("T".into(), "A".into(), "Al".into(), "1999".into(), None));
        let path = mp3.write(dir.path(), "t.mp3");

        let removed = remove_tags(&path, &TagKind::ALL).unwrap();
        assert!(removed.contains(&TagKind::Id3v1));
        assert!(removed.contains(&TagKind::Id3v2));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            crate::meta::testing::mpeg_frames(),
            "the audio stream is not what it was"
        );
    }

    #[test]
    fn removing_one_kind_leaves_the_others_alone() {
        let dir = TempDir::new().unwrap();
        let mut mp3 = Mp3::tagged("A", "B");
        mp3.id3v1 = Some(("T".into(), "V1A".into(), "Al".into(), "1999".into(), None));
        let path = mp3.write(dir.path(), "t.mp3");

        assert_eq!(
            remove_tags(&path, &[TagKind::Id3v1]).unwrap(),
            [TagKind::Id3v1]
        );
        let after = tags(&path);
        assert_eq!(after.artist.as_deref(), Some("A"), "the ID3v2 tag went too");
        let raw = std::fs::read(&path).unwrap();
        assert_ne!(&raw[raw.len() - 128..][..3], b"TAG");
    }

    /// *"if present"* — nothing there is not an error, and running twice is
    /// not either.
    #[test]
    fn removing_a_tag_that_is_not_there_does_nothing_and_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        let path = Mp3 {
            audio: true,
            ..Default::default()
        }
        .write(dir.path(), "bare.mp3");
        let before = std::fs::read(&path).unwrap();

        assert!(remove_tags(&path, &TagKind::ALL).unwrap().is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), before);

        // And twice over a file that did have one.
        let path = Mp3::tagged("A", "B").write(dir.path(), "t.mp3");
        assert_eq!(remove_tags(&path, &TagKind::ALL).unwrap(), [TagKind::Id3v2]);
        let once = std::fs::read(&path).unwrap();
        assert!(remove_tags(&path, &TagKind::ALL).unwrap().is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), once);
    }

    /// The Lyrics3 checkbox reaches the hand-rolled stripper.
    #[test]
    fn the_lyrics_checkbox_removes_a_block_lofty_cannot_see() {
        let dir = TempDir::new().unwrap();
        let mut file = crate::meta::testing::id3v2(&[("TPE1", "A")]);
        let audio_at = file.len();
        file.extend_from_slice(&crate::meta::testing::mpeg_frames());
        let mut block = Vec::from(*b"LYRICSBEGIN");
        block.extend_from_slice(b"LYR00005hello");
        let size = block.len();
        block.extend_from_slice(format!("{size:06}").as_bytes());
        block.extend_from_slice(b"LYRICS200");
        file.extend_from_slice(&block);
        let path = dir.path().join("lyr.mp3");
        std::fs::write(&path, &file).unwrap();

        assert_eq!(
            remove_tags(&path, &[TagKind::Lyrics3]).unwrap(),
            [TagKind::Lyrics3]
        );
        let after = std::fs::read(&path).unwrap();
        assert_eq!(
            after.len(),
            audio_at + crate::meta::testing::mpeg_frames().len()
        );
        // And the ID3v2 tag it was asked to leave alone is still there.
        assert_eq!(tags(&path).artist.as_deref(), Some("A"));
    }
}

#[cfg(test)]
mod hazard_tests {
    use super::*;
    use crate::meta::testing::Mp3;
    use tempfile::TempDir;

    fn field(field: MusicField, value: &str) -> FieldWrite {
        FieldWrite {
            field,
            value: value.to_owned(),
        }
    }

    fn block(path: &Path, tag_type: TagType) -> Option<Tag> {
        Probe::open(path)
            .unwrap()
            .read()
            .unwrap()
            .tag(tag_type)
            .cloned()
    }

    /// A legacy MP3 carrying only an ID3v1 trailer. The policy writes an ID3v2
    /// (D70), and building that block from the one ticked field would leave
    /// every ID3v2-preferring player — which is most of them — showing a file
    /// that had lost its title, album, year and track.
    ///
    /// Nothing would be *gone*: the ID3v1 block still holds it all. It would
    /// simply be invisible, which is worse, because it looks like the tagger
    /// ate it.
    #[test]
    fn a_new_block_is_seeded_from_the_one_the_file_already_had() {
        let dir = TempDir::new().unwrap();
        let mut legacy = Mp3 {
            audio: true,
            ..Default::default()
        };
        legacy.id3v1 = Some((
            "Nothing Else Matters".into(),
            "Metallica".into(),
            "Black Album".into(),
            "1991".into(),
            Some(8),
        ));
        let path = legacy.write(dir.path(), "legacy.mp3");
        assert!(
            block(&path, TagType::Id3v2).is_none(),
            "no v2 to begin with"
        );

        write_fields(&path, &[field(MusicField::Artist, "NewArtist")]).unwrap();

        let v2 = block(&path, TagType::Id3v2).expect("a v2 block was written");
        assert_eq!(v2.artist().as_deref(), Some("NewArtist"));
        assert_eq!(
            v2.title().as_deref(),
            Some("Nothing Else Matters"),
            "the new block must carry what the old one holds"
        );
        assert_eq!(v2.album().as_deref(), Some("Black Album"));
        assert_eq!(v2.get_string(ItemKey::TrackNumber), Some("8"));
    }

    /// The user ticks Year, sets 1988, and the ID3v1 trailer keeps saying 1975.
    ///
    /// A cloned ID3v1 tag exposes its year as `ItemKey::Year`, and the writer
    /// reads that key first — so inserting `RecordingDate` beside it puts the
    /// new value in the block and changes nothing about what gets written.
    #[test]
    fn the_year_reaches_the_id3v1_block_and_not_only_the_id3v2_one() {
        let dir = TempDir::new().unwrap();
        let mut both = Mp3::tagged("A", "B").frame("TDRC", "1975");
        both.id3v1 = Some(("T".into(), "A".into(), "Al".into(), "1975".into(), None));
        let path = both.write(dir.path(), "both.mp3");

        write_fields(&path, &[field(MusicField::Year, "1988")]).unwrap();

        assert_eq!(
            block(&path, TagType::Id3v2)
                .unwrap()
                .get_string(ItemKey::RecordingDate),
            Some("1988")
        );
        assert_eq!(
            block(&path, TagType::Id3v1)
                .unwrap()
                .get_string(ItemKey::Year),
            Some("1988"),
            "the trailer still says the old year"
        );
    }

    /// ID3v1's track is one byte. 1994 — one mis-mapped Setup Parts slot away,
    /// from a name like `1994 - Song.mp3` — does not fail: it writes **0** over
    /// whatever real track was there. The ID3v2 takes it, so the field is
    /// written there and left alone here.
    #[test]
    fn a_track_too_large_for_id3v1_does_not_zero_the_one_that_is_there() {
        let dir = TempDir::new().unwrap();
        let mut mp3 = Mp3::tagged("A", "B");
        mp3.id3v1 = Some(("T".into(), "A".into(), "Al".into(), "1999".into(), Some(8)));
        let path = mp3.write(dir.path(), "big.mp3");

        let written = write_fields(&path, &[field(MusicField::Track, "1994")]).unwrap();
        assert_eq!(written, [MusicField::Track], "ID3v2 can hold it");

        assert_eq!(
            block(&path, TagType::Id3v2)
                .unwrap()
                .get_string(ItemKey::TrackNumber),
            Some("1994")
        );
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(
            raw[raw.len() - 2],
            8,
            "the ID3v1 track byte was zeroed rather than left alone"
        );

        // And one that does fit still lands in both.
        let path = mp3.write(dir.path(), "small.mp3");
        write_fields(&path, &[field(MusicField::Track, "12")]).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(raw[raw.len() - 2], 12);
    }
}
