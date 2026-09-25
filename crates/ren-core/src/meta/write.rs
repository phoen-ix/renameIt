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
//!
//! ## Every change is made to a copy
//!
//! lofty saves by rewriting the file where it lies. For an ID3v2 tag, and for
//! every FLAC, it reads everything after the old tag into memory, truncates
//! the file to nothing and writes the lot back. An error after the truncation
//! — a full disk, a network share that drops, a USB stick pulled out — left a
//! file with no audio in it, and nothing here can be undone (P2).
//!
//! So nothing here writes the file itself. [`rewrite`] copies it to a sibling
//! in the same folder, named [`SCRATCH_PREFIX`] plus the process id, a counter
//! and the file's own extension (lofty's read falls back to the extension
//! when the content does not decide the format, so the copy must read as the
//! file does). lofty and [`lyrics3`] change the copy. The copy is given the
//! file's modified date back, flushed to disk, and swapped in by
//! [`Platform::replace_file`]. A failure at any step before the swap removes
//! the copy and leaves the file exactly as it was. A crash leaves the file
//! whole, old or new, with at worst a stray copy beside it.
//!
//! What the file keeps across the swap:
//!
//! * **Its modified date.** The copy gets it before the swap, so writing tags
//!   does not change the file's date. Best effort: where the filesystem
//!   refuses, the file has the date the write gave it, as an in-place write
//!   would have. The date stays the same and a tag edited to one of the same
//!   length leaves the length the same too, so the caches keyed on those two
//!   are cleared after every change ([`forget_what_was_read`]).
//! * **On Windows**, `ReplaceFileW` keeps the created date, the attributes,
//!   the ACLs and the alternate data streams.
//! * **On Unix**, the swap is a `rename`. The copy has the file's permission
//!   bits, because `std::fs::copy` carries them. Its owner is the user running
//!   the app, and its created date, where the filesystem keeps one, is the
//!   copy's.
//! * **Hard links are not kept, on either platform.** The file becomes a new
//!   file under its name, and any other name for the old one keeps the old
//!   tags.
//!
//! Two refusals keep the swap from reaching further than an in-place write
//! did. A file this process cannot open for writing is refused before the copy
//! is made. On Unix, replacing a file takes write permission on the folder,
//! not on the file, so without the check a read-only file would be retagged.
//! And a symbolic link is refused. The swap would replace the link itself
//! with a file, and the tags belong to the file it points to, which no row
//! names. That matches Set Attributes, which refuses a link's read-only bit
//! rather than change its target. The planner refuses both first, as a
//! conflict on the link's row (`ConflictKind::TagsThroughLink`, D241), so a
//! run never reaches this refusal part-way; it stays as the backstop for a
//! file that became a link after the preview.
//!
//! The copy needs the file's size in free space on its volume. A Music Tagger
//! with nothing to write and a Remove Tags with nothing to remove make no copy.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lofty::ape::ApeTag;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OggPictureStorage, OpusFile, SpeexFile, VorbisFile};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, MergeTag, SplitTag, Tag, TagItem, TagType};
use ren_platform::{Capability, Platform, PlatformError, TimeChange};
use serde::{Deserialize, Serialize};

use super::lyrics3;

/// The seven fields Music Tagger offers, in the order its card lists them.
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
    /// Top to bottom as the card has them.
    pub const ALL: [Self; 7] = [
        Self::Track,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Year,
        Self::Genre,
        Self::Comment,
    ];

    /// The card's captions. Comment is `Comm`, the name of its ID3v2 frame,
    /// which keeps the column of seven captions narrow.
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

/// The tag blocks Remove Tags offers, as its card lists them.
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
    /// Refused rather than followed (module docs).
    #[error(
        "this is a symbolic link: its tags are in the file it points to, which is not changed through a link"
    )]
    Link,
    #[error("{0}")]
    Lofty(String),
    #[error("{0}")]
    Lyrics(#[from] lyrics3::Error),
    /// A step of changing a copy rather than the file failed. The file is as
    /// it was.
    #[error("{step}: {detail}")]
    Copy { step: &'static str, detail: String },
    /// The swap took the file out of its place and could not move the copy
    /// in (`PlatformError::ReplacedButNotMoved`): the copy, with the new tags
    /// already in it, is the only copy of the file left. It is kept, never
    /// cleaned up, and named, so the user can rename it back.
    #[error(
        "could not put the copy in the file's place, and the file is no longer there: its \
         contents, with the new tags, are in {} — rename that to {} to get it back ({detail})",
        .copy.display(),
        .target.file_name().unwrap_or_default().to_string_lossy()
    )]
    Stranded {
        copy: PathBuf,
        target: PathBuf,
        detail: String,
    },
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

/// A failure in one step of [`rewrite`].
fn step(step: &'static str, detail: impl std::fmt::Display) -> WriteError {
    WriteError::Copy {
        step,
        detail: detail.to_string(),
    }
}

const COPYING: &str = "could not copy the file to write the tags into";

/// The start of a copy's name. The process id, a counter and the file's
/// extension follow.
///
/// Public so that a front end can say what the file beside an interrupted
/// tag write is: a crash between the copy and the swap leaves one, and the
/// file it was made from is whole.
pub const SCRATCH_PREFIX: &str = "__renameit-tags-";

/// A copy of a file, beside it, which is removed again unless it is kept.
///
/// Removed on drop, so an error from any step, a panic included, cleans up
/// after itself. Kept when it took the file's place, and when a swap that
/// failed half-way left it the only copy of the file
/// ([`WriteError::Stranded`]).
struct Scratch {
    path: PathBuf,
    keep: bool,
}

impl Scratch {
    /// How many names are tried. Each name is new to this process, so one is
    /// only ever taken by a stray copy from an earlier process that had the
    /// same id.
    const ATTEMPTS: u32 = 100;

    fn beside(original: &Path) -> Result<Self, WriteError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = original.parent().unwrap_or(Path::new(""));
        for _ in 0..Self::ATTEMPTS {
            let mut name = OsString::from(format!(
                "{SCRATCH_PREFIX}{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            if let Some(extension) = original.extension() {
                name.push(".");
                name.push(extension);
            }
            let path = dir.join(name);
            // The name is claimed before the copy, because `fs::copy` would
            // replace whatever already had it.
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => {
                    let scratch = Self { path, keep: false };
                    // The contents and the permission bits, and on Windows
                    // the attributes. A file made read-only after `rewrite`
                    // checked it gives a read-only copy, which lofty then
                    // fails to write.
                    std::fs::copy(original, &scratch.path).map_err(|e| step(COPYING, e))?;
                    return Ok(scratch);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(step(COPYING, e)),
            }
        }
        Err(step(COPYING, "every name tried for the copy is taken"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Refuses what must not be written, before anything is read.
///
/// A symbolic link, for the reasons in the module docs. Anything else that is
/// not a regular file is not music: a named pipe would block the read that
/// follows until something wrote to it.
fn a_regular_file(path: &Path) -> Result<(), WriteError> {
    let metadata = std::fs::symlink_metadata(path).map_err(lofty)?;
    if metadata.file_type().is_symlink() {
        return Err(WriteError::Link);
    }
    if !metadata.is_file() {
        return Err(WriteError::NotAudio);
    }
    Ok(())
}

/// Runs `edit` on a copy of `path` and swaps the copy in (module docs).
///
/// `path` is only read until the swap. An error from any step, `edit`'s
/// included, removes the copy and leaves `path` as it was — except a swap
/// that took `path` away and could not move the copy in, which keeps the
/// copy and says where it is ([`WriteError::Stranded`]).
fn rewrite<T>(
    path: &Path,
    platform: &dyn Platform,
    edit: impl FnOnce(&Path) -> Result<T, WriteError>,
) -> Result<T, WriteError> {
    // The permission an in-place write needed and a swap does not: on Unix a
    // rename asks the folder, not the file. Opened, not written, so nothing
    // about the file changes.
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| step("this file cannot be written", e))?;
    let modified = platform
        .get_times(path)
        .ok()
        .and_then(|times| times.modified)
        .filter(|_| platform.supports(Capability::ModifiedTime));

    let mut scratch = Scratch::beside(path)?;
    let value = edit(&scratch.path)?;

    if modified.is_some() {
        // Best effort: a date that cannot be kept does not stop the write.
        let _ = platform.set_times(
            &scratch.path,
            TimeChange {
                modified,
                ..TimeChange::default()
            },
        );
    }
    // Flushed before the swap: a rename can reach the disk before the data it
    // names, and a power cut then leaves an empty file under the old name.
    // Opened for writing because Windows flushes only a handle that can write.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&scratch.path)
        .and_then(|file| file.sync_all())
        .map_err(|e| step("could not save the copy to disk", e))?;
    // The folder is not flushed after the swap. A power cut before its new
    // entry reaches the disk brings back the old file, which is whole, and a
    // folder cannot be opened to flush it on Windows.
    match platform.replace_file(&scratch.path, path) {
        Ok(()) => {}
        Err(PlatformError::ReplacedButNotMoved { source, .. }) => {
            scratch.keep = true;
            return Err(WriteError::Stranded {
                copy: scratch.path.clone(),
                target: path.to_path_buf(),
                detail: source.to_string(),
            });
        }
        Err(e) => return Err(step("could not put the copy in the file's place", e)),
    }
    scratch.keep = true;
    Ok(value)
}

/// Drops what was read from files, after one of them changed.
///
/// The swap keeps the file's modified date, and a tag edited to one of the
/// same length keeps the file's length. The audio tags and `<Crc32>` are
/// cached against those two, so they would go on answering with what the
/// file said before.
fn forget_what_was_read() {
    super::audio::forget_all();
    crate::template::content::forget_all();
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

/// The file as lofty reads it, every tag it carries.
fn read_tagged(path: &Path) -> Result<lofty::file::TaggedFile, WriteError> {
    Probe::open(path)
        .map_err(lofty)?
        .guess_file_type()
        .map_err(lofty)?
        .read()
        .map_err(lofty)
}

/// Writes `fields` into `path`, leaving every other field alone.
///
/// Returns the fields that were actually written — a value that will not
/// normalise is skipped rather than written wrong, and the caller says so.
/// The write goes into a copy that `platform` swaps in (module docs), so a
/// failure leaves the file as it was.
pub fn write_fields(
    path: &Path,
    fields: &[FieldWrite],
    platform: &dyn Platform,
) -> Result<Vec<MusicField>, WriteError> {
    a_regular_file(path)?;
    let (file_type, tag_type) = primary(path)?;

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

    rewrite(path, platform, |copy| {
        write_into(copy, file_type, tag_type, fields, &wanted)
    })?;
    forget_what_was_read();
    Ok(wanted.iter().map(|(field, _)| *field).collect())
}

/// [`write_fields`]' two saves, into the copy at `path`.
fn write_into(
    path: &Path,
    file_type: FileType,
    tag_type: TagType,
    fields: &[FieldWrite],
    wanted: &[(MusicField, String)],
) -> Result<(), WriteError> {
    let tagged = read_tagged(path)?;

    match Native::read(path, file_type)? {
        Some(native) => native.write(path, wanted)?,
        None => {
            // **Clone, never rebuild.** Copying the enabled fields into a
            // fresh `Tag` is the obvious implementation of per-field
            // checkboxes and it silently deletes embedded cover art on every
            // format — on MP4 through a companion the generic API does not
            // even show.
            let mut tag = existing_or_seeded(&tagged, tag_type);
            for (field, value) in wanted {
                put(&mut tag, key_in(*field, tag_type), value.clone());
            }
            tag.save_to_path(path, WriteOptions::default())
                .map_err(lofty)?;
        }
    }

    // The user's choice (D70): ID3v2 always, and
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
            .map_err(lofty)?;
    }
    Ok(())
}

/// Removes `kinds` from `path`. Returns what was actually removed.
///
/// Only what is present is removed: a kind that is not there is not an
/// error, and neither is a kind this format cannot carry. What is removed is
/// removed from a copy that `platform` swaps in (module docs), so a failure
/// leaves the file as it was, and a file with nothing to remove is not
/// copied at all.
pub fn remove_tags(
    path: &Path,
    kinds: &[TagKind],
    platform: &dyn Platform,
) -> Result<Vec<TagKind>, WriteError> {
    a_regular_file(path)?;
    let (file_type, _) = primary(path)?;
    let present = present(path, file_type, kinds)?;
    if present.is_empty() {
        return Ok(Vec::new());
    }

    let removed = rewrite(path, platform, |copy| strip(copy, &present))?;
    forget_what_was_read();
    Ok(removed)
}

/// The kinds in `kinds` that `path` carries and its format can lose, each
/// once, in the order asked.
///
/// Read from the file itself, before any copy is made. The three blocks are
/// independent — removing one does not remove or reveal another — so what is
/// present here is what is present in the copy when its turn comes.
fn present(
    path: &Path,
    file_type: FileType,
    kinds: &[TagKind],
) -> Result<Vec<TagKind>, WriteError> {
    // **The guard that stops this crashing.** An empty tag skips lofty's
    // writability check and reaches a writer that dispatches on the file's
    // format: an empty ID3v1 saved to a FLAC panics outright, and the same
    // request on an Ogg wipes the Vorbis comments and returns `Ok`.
    let carried = |tag_type: TagType| file_type.tag_support(tag_type).is_readable();
    let tagged = if kinds
        .iter()
        .any(|kind| kind.tag_type().is_some_and(carried))
    {
        Some(read_tagged(path)?)
    } else {
        None
    };

    let mut present = Vec::new();
    for &kind in kinds {
        if present.contains(&kind) {
            continue;
        }
        let here = match kind.tag_type() {
            // Lyrics3, which lofty cannot see. Only MP3 carries one.
            None => file_type == FileType::Mpeg && lyrics3::find(path)?.is_some(),
            // Present-check before every strip, not only for tidiness.
            // Stripping an MP4 that has already been stripped deletes the
            // mandatory `hdlr` box and leaves a file lofty itself can no
            // longer parse — every write returning `Ok` the whole way down.
            Some(tag_type) => {
                carried(tag_type)
                    && tagged
                        .as_ref()
                        .is_some_and(|tagged| tagged.tag(tag_type).is_some())
            }
        };
        if here {
            present.push(kind);
        }
    }
    Ok(present)
}

/// [`remove_tags`]' removals, from the copy at `path`.
///
/// Saving an *empty* tag of a type is what removes it (module docs, point 2).
fn strip(path: &Path, present: &[TagKind]) -> Result<Vec<TagKind>, WriteError> {
    let mut removed = Vec::new();
    for &kind in present {
        match kind.tag_type() {
            None => {
                if lyrics3::remove(path)? {
                    removed.push(kind);
                }
            }
            Some(tag_type) => {
                Tag::new(tag_type)
                    .save_to_path(path, WriteOptions::default())
                    .map_err(lofty)?;
                removed.push(kind);
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::testing::Mp3;
    use crate::test_platform::Spy;
    use ren_platform::host;
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

        let written = write_fields(
            &path,
            &[field(MusicField::Artist, "NewArtist")],
            host().as_ref(),
        )
        .unwrap();
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

        let written = write_fields(
            &path,
            &[field(MusicField::Comment, "new comment")],
            host().as_ref(),
        )
        .unwrap();
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
            host().as_ref(),
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
            host().as_ref(),
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
            let written =
                write_fields(&path, &[field(MusicField::Track, bad)], host().as_ref()).unwrap();
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
            write_fields(&path, &[field(MusicField::Track, given)], host().as_ref()).unwrap();
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
        let written =
            write_fields(&path, &[field(MusicField::Artist, "")], host().as_ref()).unwrap();
        assert!(written.is_empty());
        assert_eq!(tags(&path).artist.as_deref(), Some("Keep"));
    }

    /// The user's choice (D70): ID3v1 is refreshed where there is one.
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
        write_fields(
            &path,
            &[field(MusicField::Artist, "NewArtist")],
            host().as_ref(),
        )
        .unwrap();
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
        write_fields(
            &path,
            &[field(MusicField::Artist, "NewArtist")],
            host().as_ref(),
        )
        .unwrap();
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
            write_fields(&path, &[field(MusicField::Artist, "A")], host().as_ref()),
            Err(WriteError::NotAudio) | Err(WriteError::Lofty(_))
        ));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"this is not a music file at all"
        );
    }

    // --- Removal ------------------------------------------------------------

    /// Removing tags must leave the audio stream intact, byte for byte past
    /// the tag blocks.
    #[test]
    fn removing_every_tag_leaves_the_audio_byte_identical() {
        let dir = TempDir::new().unwrap();
        let mut mp3 = Mp3::tagged("A", "B");
        mp3.id3v1 = Some(("T".into(), "A".into(), "Al".into(), "1999".into(), None));
        let path = mp3.write(dir.path(), "t.mp3");

        let removed = remove_tags(&path, &TagKind::ALL, host().as_ref()).unwrap();
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
            remove_tags(&path, &[TagKind::Id3v1], host().as_ref()).unwrap(),
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

        assert!(
            remove_tags(&path, &TagKind::ALL, host().as_ref())
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);

        // And twice over a file that did have one.
        let path = Mp3::tagged("A", "B").write(dir.path(), "t.mp3");
        assert_eq!(
            remove_tags(&path, &TagKind::ALL, host().as_ref()).unwrap(),
            [TagKind::Id3v2]
        );
        let once = std::fs::read(&path).unwrap();
        assert!(
            remove_tags(&path, &TagKind::ALL, host().as_ref())
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read(&path).unwrap(), once);
    }

    /// Every name in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// `after` is `before`, said without printing two files byte by byte.
    fn assert_unchanged(path: &Path, before: &[u8], why: &str) {
        let after = std::fs::read(path).unwrap();
        assert!(
            after == before,
            "{why}: {} bytes where there were {}",
            after.len(),
            before.len()
        );
    }

    /// A disk that fills up part-way through a write, reproduced for real.
    ///
    /// lofty saves an MP3's ID3v2 by reading everything after the old tag,
    /// truncating the file to nothing and writing it all back. A write that
    /// fails after the truncation, because the disk is full or the stick was
    /// pulled, used to leave a file with no audio in it, with no undo. Now
    /// the new tags go into a copy, and only a finished copy takes the file's
    /// place.
    ///
    /// A file-size limit (`ulimit -f`) makes the same failure on demand: the
    /// file is small enough to copy, and a 16 KB comment makes the write go
    /// past the limit. The limit covers a whole process, so the test runs
    /// itself again in a child process with the limit set. `SIGXFSZ` is
    /// ignored there, so the write gets an error rather than a signal that
    /// kills the process.
    #[cfg(unix)]
    #[test]
    fn a_write_that_fails_part_way_leaves_the_file_as_it_was() {
        const CHILD: &str = "RENAMEIT_TEST_FSIZE_CHILD";
        const NAME: &str =
            "meta::write::tests::a_write_that_fails_part_way_leaves_the_file_as_it_was";
        if std::env::var_os(CHILD).is_none() {
            // Eight blocks is 4 KB in `dash` and 8 KB in `bash`. The fixture
            // is under both, and the file after the write is over both.
            let child = std::process::Command::new("sh")
                .args(["-c", r#"trap '' XFSZ; ulimit -f 8 && exec "$0" "$@""#])
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--test-threads=1"])
                .env(CHILD, "1")
                .output()
                .expect("the test binary runs itself");
            let stdout = String::from_utf8_lossy(&child.stdout);
            assert!(
                child.status.success() && stdout.contains("1 passed"),
                "the run under a file-size limit failed:\n{stdout}{}",
                String::from_utf8_lossy(&child.stderr)
            );
            return;
        }

        // In the child.
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B").write(dir.path(), "t.mp3");
        let before = std::fs::read(&path).unwrap();
        assert!(before.len() < 4096, "the fixture must fit under the limit");

        let huge = "x".repeat(16 * 1024);
        let result = write_fields(&path, &[field(MusicField::Comment, &huge)], host().as_ref());
        // lofty's own failure, part-way through its write: the copy was made.
        assert!(
            matches!(result, Err(WriteError::Lofty(_))),
            "the limit was not reached where it should be: {result:?}"
        );
        assert_unchanged(
            &path,
            &before,
            "a write that failed part-way changed the file",
        );
        assert_eq!(names(dir.path()), ["t.mp3"], "the copy was left behind");
    }

    /// The last step failing — the swap — is as harmless as the first.
    #[test]
    fn a_copy_that_cannot_take_the_files_place_leaves_the_file_as_it_was() {
        let dir = TempDir::new().unwrap();
        let mut mp3 = Mp3::tagged("A", "B");
        mp3.id3v1 = Some(("T".into(), "A".into(), "Al".into(), "1999".into(), None));
        let path = mp3.write(dir.path(), "t.mp3");
        let before = std::fs::read(&path).unwrap();
        let platform = Spy {
            fail_swap: true,
            ..Spy::default()
        };

        let error = write_fields(&path, &[field(MusicField::Artist, "New")], &platform)
            .expect_err("the swap fails");
        assert!(
            error
                .to_string()
                .contains("could not put the copy in the file's place"),
            "{error}"
        );
        assert_unchanged(&path, &before, "a write whose swap failed changed the file");

        remove_tags(&path, &TagKind::ALL, &platform).expect_err("the swap fails");
        assert_unchanged(
            &path,
            &before,
            "a removal whose swap failed changed the file",
        );

        assert_eq!(platform.swaps(), 2);
        assert_eq!(names(dir.path()), ["t.mp3"], "a copy was left behind");
    }

    /// **A swap that removed the file and could not move the copy in keeps
    /// the copy.** Windows' `ReplaceFileW` can fail after taking the file out
    /// of its place (`ERROR_UNABLE_TO_MOVE_REPLACEMENT`), and when the move
    /// back fails too the copy is the only copy of the file. Cleaning it up
    /// as after any other failed swap deleted the file for good; it is kept,
    /// and the error names it so the user can rename it back.
    #[test]
    fn a_swap_that_strands_the_copy_keeps_it_and_names_it() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B").write(dir.path(), "t.mp3");
        let platform = Spy {
            strand_swap: true,
            ..Spy::default()
        };

        let error = write_fields(&path, &[field(MusicField::Artist, "New")], &platform)
            .expect_err("the swap strands the copy");
        let WriteError::Stranded { copy, .. } = &error else {
            panic!("an ordinary failure, whose copy would be cleaned up: {error:?}");
        };
        assert!(copy.exists(), "the only copy of the file was deleted");
        assert!(
            error.to_string().contains(&copy.display().to_string()),
            "the error does not say where the file is: {error}"
        );
        assert!(!path.exists());

        // Renamed back as the error says, it is the file with its new tags.
        std::fs::rename(copy, &path).unwrap();
        crate::meta::audio::forget_all();
        let tags = crate::meta::audio::tags_of(&path).unwrap();
        assert_eq!(tags.artist.as_deref(), Some("New"));
        assert_eq!(names(dir.path()), ["t.mp3"]);
    }

    /// Writing tags leaves the file's modified date alone, and on Unix its
    /// permission bits: the swap puts a new file under the name, and it must
    /// look like the old one in everything but its tags.
    #[test]
    fn a_tag_change_keeps_the_files_modified_date() {
        let dir = TempDir::new().unwrap();
        let mut mp3 = Mp3::tagged("A", "B");
        mp3.id3v1 = Some(("T".into(), "A".into(), "Al".into(), "1999".into(), None));
        let path = mp3.write(dir.path(), "t.mp3");
        let then =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(then)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        let modified = || std::fs::metadata(&path).unwrap().modified().unwrap();

        write_fields(&path, &[field(MusicField::Artist, "New")], host().as_ref()).unwrap();
        assert_eq!(
            tags(&path).artist.as_deref(),
            Some("New"),
            "nothing was written"
        );
        assert_eq!(modified(), then, "writing tags moved the date");

        assert_eq!(
            remove_tags(&path, &[TagKind::Id3v1], host().as_ref()).unwrap(),
            [TagKind::Id3v1]
        );
        assert_eq!(modified(), then, "removing a tag moved the date");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o640, "the permission bits changed");
        }
        assert_eq!(names(dir.path()), ["t.mp3"]);
    }

    /// Keeping the date is a courtesy, not the change the user asked for, so a
    /// filesystem that will not set it does not stop the write.
    #[test]
    fn a_date_that_cannot_be_kept_does_not_stop_the_write() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B").write(dir.path(), "t.mp3");
        let platform = Spy {
            fail_dates: true,
            ..Spy::default()
        };
        write_fields(&path, &[field(MusicField::Artist, "New")], &platform).unwrap();
        assert_eq!(tags(&path).artist.as_deref(), Some("New"));
        assert_eq!(names(dir.path()), ["t.mp3"]);
    }

    /// A file that cannot be written in place is not replaced either.
    ///
    /// On Unix the swap needs write permission on the folder, not on the
    /// file, so without the check a read-only file would be retagged where an
    /// in-place write was refused.
    #[test]
    fn a_read_only_file_is_refused_rather_than_replaced() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B").write(dir.path(), "t.mp3");
        let mut read_only = std::fs::metadata(&path).unwrap().permissions();
        read_only.set_readonly(true);
        std::fs::set_permissions(&path, read_only).unwrap();
        if std::fs::OpenOptions::new().write(true).open(&path).is_ok() {
            eprintln!("this user can write a read-only file, so there is nothing to test");
            return;
        }
        let before = std::fs::read(&path).unwrap();
        let platform = Spy::default();

        let error = write_fields(&path, &[field(MusicField::Artist, "New")], &platform)
            .expect_err("a read-only file");
        assert!(error.to_string().contains("cannot be written"), "{error}");
        remove_tags(&path, &TagKind::ALL, &platform).expect_err("a read-only file");

        assert_unchanged(&path, &before, "a read-only file was changed");
        assert_eq!(platform.swaps(), 0);
        assert_eq!(names(dir.path()), ["t.mp3"]);
    }

    /// A link is refused, and the file it points to is left alone: the swap
    /// would put a file where the link was, and the tags are not the link's.
    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_is_refused_and_the_file_it_points_to_left_alone() {
        let dir = TempDir::new().unwrap();
        let target = Mp3::tagged("A", "B").write(dir.path(), "real.mp3");
        let link = dir.path().join("link.mp3");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let before = std::fs::read(&target).unwrap();

        assert!(matches!(
            write_fields(&link, &[field(MusicField::Artist, "New")], host().as_ref()),
            Err(WriteError::Link)
        ));
        assert!(matches!(
            remove_tags(&link, &TagKind::ALL, host().as_ref()),
            Err(WriteError::Link)
        ));

        assert_unchanged(&target, &before, "the link's target was changed");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(names(dir.path()), ["link.mp3", "real.mp3"]);
    }

    /// A Music Tagger with nothing to write and a Remove Tags with nothing to
    /// remove copy nothing: a copy costs the whole file, and a run over a
    /// folder of files with no ID3v1 would otherwise copy every one for
    /// nothing.
    #[test]
    fn nothing_to_change_makes_no_copy() {
        let dir = TempDir::new().unwrap();
        let path = Mp3 {
            audio: true,
            ..Default::default()
        }
        .write(dir.path(), "bare.mp3");
        let platform = Spy::default();

        assert!(
            write_fields(&path, &[field(MusicField::Artist, " ")], &platform)
                .unwrap()
                .is_empty()
        );
        assert!(
            remove_tags(&path, &TagKind::ALL, &platform)
                .unwrap()
                .is_empty()
        );
        assert_eq!(platform.swaps(), 0);
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
            remove_tags(&path, &[TagKind::Lyrics3], host().as_ref()).unwrap(),
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
    use ren_platform::host;
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

        write_fields(
            &path,
            &[field(MusicField::Artist, "NewArtist")],
            host().as_ref(),
        )
        .unwrap();

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

        write_fields(&path, &[field(MusicField::Year, "1988")], host().as_ref()).unwrap();

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

        let written =
            write_fields(&path, &[field(MusicField::Track, "1994")], host().as_ref()).unwrap();
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
        write_fields(&path, &[field(MusicField::Track, "12")], host().as_ref()).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(raw[raw.len() - 2], 12);
    }
}
