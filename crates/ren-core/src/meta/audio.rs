//! Reading the tags and properties a music file carries.
//!
//! **Reading** goes by content: any listed file whose bytes lofty recognises
//! — MP3, Ogg Vorbis, Opus, Speex, FLAC, Musepack, WavPack, Monkey's Audio,
//! MP4/M4A — answers the music tags, whatever it is called. **Writing and
//! removing** (Music Tagger, Remove Tags) touch only a file whose extension
//! is in [`AUDIO_EXTENSIONS`] (P56); a `.mp4` or `.m4r` reads its tags but is
//! never written to.
//!
//! WMA and TTA are **not** supported — lofty handles neither. Recorded rather
//! than quietly absent: a WMA reads as untagged,
//! which is indistinguishable from a file that genuinely has no tags, so the
//! deviation has to live somewhere a reader will find it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};

use super::cache::{MetaCache, Stamp};
use super::folder;
use super::names;

/// The extensions of the music files the operations write to, and the ones a
/// folder peek opens.
///
/// Two jobs. For Music Tagger and Remove Tags it is the preview's cheap gate
/// (P56): a file outside it is a row left alone, and the executor then checks
/// the content. For the folder peek it is what stops a folder of ten thousand
/// documents being probed one by one. Neither is a claim about reading:
/// `tags_of` sniffs content, so a file named `song` with an ID3 block still
/// reads.
pub const AUDIO_EXTENSIONS: [&str; 13] = [
    "mp3", "mp2", "mp1", "ogg", "oga", "spx", "opus", "flac", "mpc", "wv", "m4a", "m4b", "m4p",
];

/// Below this a file cannot carry a tag block of any kind: an ID3v2 header is
/// ten bytes and needs a frame after it, and every other container's magic is
/// longer than that.
///
/// The gate exists for a specific reason — the perf tests write ten thousand
/// **zero-byte** `.mp3` files, and without it a music pipeline over that corpus
/// is ten thousand futile parses inside the preview, once per keystroke. It is
/// set at the smallest thing that could possibly be real rather than at
/// something merely plausible: a threshold that rejects a legitimate file to
/// save time on an illegitimate one is a bug waiting for the day somebody has a
/// very short tag.
const MIN_AUDIO_BYTES: u64 = 16;

/// Everything one music file has to say, read once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioTags {
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    pub year: Option<String>,
    pub comment: Option<String>,
    pub genre: Option<String>,
    /// The track number as text.
    ///
    /// lofty parses `TRCK` numerically: the `"7/12"` form reads as `"7"` — which
    /// suits us, since `<TrackN>` is the track number without padding and
    /// rendering `7/12` there would be a surprise — and a
    /// non-numeric track such as a vinyl rip's `"A1"` does not survive the
    /// parse at all, so the tag is simply *missing* for that file. Kept as text
    /// rather than a number anyway, so nothing is lost if another format
    /// stores something lofty is willing to hand over whole.
    pub track: Option<String>,
    /// The `<ID3-*>` names, keyed by the canonical spelling in
    /// [`super::names::ID3`].
    pub extended: BTreeMap<&'static str, String>,
    pub properties: AudioProperties,
}

/// What the audio itself says, as opposed to what someone wrote about it.
///
/// Every field is optional, and that is the whole point. lofty reports `0` for
/// a bitrate it could not determine — which would make `<Bitrate>` render `0`.
/// A confident wrong answer is worse than no answer, and P32 already has a way
/// to say "no answer": the tag is *missing* and the file is left alone.
///
/// The MPEG header fields (`<Mpeg>`, `<Layer>`, `<SMode>`) are not here, and
/// that is D64: lofty's generic properties drop them, reading them means
/// opening every MP3 a second time, and a default filled in their place
/// would call every MPEG-2 Layer II mono file "MPEG-1, Layer 3, Stereo".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioProperties {
    pub duration: Option<Duration>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u8>,
}

impl AudioTags {
    /// The leading number of `track`, for `<Track>`'s zero padding.
    ///
    /// `"7/12"` is 7 and `"A1"` is nothing — the second deliberately, because a
    /// track "number" that is not a number cannot be padded to a width, and
    /// guessing would put `01` on a record whose sleeve says `A1`.
    pub fn track_number(&self) -> Option<u32> {
        let raw = self.track.as_deref()?.trim();
        let digits: String = raw.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    /// A named `<ID3-*>` value.
    pub fn extended(&self, name: &str) -> Option<&str> {
        self.extended
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The tags of one file, or `None`.
///
/// `None` covers every reason at once — not audio, no tag block, a truncated
/// one, a format we do not support. P45's reading, transplanted: to a renamer
/// they are the same fact and none of them is an error, because P4 stops the
/// whole run on any row error and a folder of 500 tracks with three `.txt`
/// files in it must still run.
pub fn tags_of(path: &Path) -> Option<Arc<AudioTags>> {
    tags_at(path, Stamp::stat(path)?)
}

/// The same, for a listed entry — the parallel pass's way in.
///
/// The entry's own size and mtime are the key, so this costs no syscall: the
/// listing is the snapshot (D139, D140), and a `stat` per file per keystroke
/// was most of what a music pipeline paid over a plain one. A folder row
/// answers with the first music file inside it.
pub fn tags_of_entry(entry: &crate::model::FileEntry) -> Option<Arc<AudioTags>> {
    let stamp = Stamp::of_entry(entry);
    if entry.is_dir {
        folder_tags_at(&entry.path, stamp)
    } else {
        tags_at(&entry.path, stamp)
    }
}

fn tags_at(path: &Path, stamp: Stamp) -> Option<Arc<AudioTags>> {
    // A folder's tags come only from the peek, which caches under the same
    // path; see `exif::date_at` for what sharing that entry did.
    if stamp.is_dir || stamp.len < MIN_AUDIO_BYTES {
        return None;
    }
    cache().get_or_read(path, stamp, || read_uncached(path))
}

/// The first music file inside a folder speaks for the folder: its tags are
/// the folder's tags.
pub fn folder_tags(dir: &Path) -> Option<Arc<AudioTags>> {
    folder_tags_at(dir, Stamp::stat(dir)?)
}

fn folder_tags_at(dir: &Path, stamp: Stamp) -> Option<Arc<AudioTags>> {
    cache().get_or_read(dir, stamp, || {
        folder::first_inside(dir, &AUDIO_EXTENSIONS, tags_of)
    })
}

/// How many files have actually been parsed.
///
/// Exists for one test, and it is a test worth having an instrument for: a
/// timing ratio cannot tell "the cache is working" from "the machine is busy",
/// and the planner's own work dominates either way. A count of parses is
/// deterministic and says exactly the thing that matters.
static PARSES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub fn parses_so_far() -> usize {
    PARSES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Empties the cache.
///
/// Called after a transaction that wrote or removed tags. The key includes the
/// file's length and mtime, so a rewrite usually invalidates itself — but
/// "usually" is not good enough when the next thing the user sees is a preview
/// of what they just changed, and a same-length rewrite inside one mtime tick
/// is exactly what a tag update is.
pub fn forget_all() {
    if let Some(cache) = READ.get() {
        cache.clear();
    }
}

fn read_uncached(path: &Path) -> Option<Arc<AudioTags>> {
    PARSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let probe = Probe::open(path).ok()?.guess_file_type().ok()?;
    // Content lofty does not recognise is not audio, whatever its name.
    probe.file_type()?;

    // Properties first, then tags without them if that failed.
    //
    // Not belt and braces: lofty gives up on the *whole* parse when it cannot
    // make sense of the audio frames, so one damaged file would otherwise read
    // as having no tags at all — and a folder of slightly-broken rips would
    // render `" - .mp3"` for every row, which then collide and block the run.
    // The tags are usually intact even when the audio is not.
    let with_properties = probe
        .options(ParseOptions::new().read_cover_art(false))
        .read()
        .ok();
    let tagged = match with_properties {
        Some(file) => Some(file),
        None => Probe::open(path)
            .ok()?
            .guess_file_type()
            .ok()?
            .options(
                ParseOptions::new()
                    .read_cover_art(false)
                    .read_properties(false),
            )
            .read()
            .ok(),
    }?;

    let mut tags = AudioTags {
        properties: properties_of(&tagged),
        ..Default::default()
    };
    tags.artist = first_of(&tagged, &[ItemKey::TrackArtist, ItemKey::AlbumArtist]);
    tags.title = first_of(&tagged, &[ItemKey::TrackTitle]);
    tags.album = first_of(&tagged, &[ItemKey::AlbumTitle]);
    tags.comment = comment_of(&tagged);
    tags.genre = first_of(&tagged, &[ItemKey::Genre]);
    tags.track = first_of(&tagged, &[ItemKey::TrackNumber]);
    tags.year = year_of(&tagged);

    for (name, key) in names::ID3 {
        if let Some(value) = first_of(&tagged, &[*key]) {
            tags.extended.insert(name, value);
        }
    }

    // Anything lofty could open as audio is worth an answer, even with no tags
    // at all: `<Length>` and `<Bitrate>` come from the audio, not from a tag,
    // and an untagged track still has a duration. The individual tags report
    // themselves absent, which is what P32 wants.
    Some(Arc::new(tags))
}

/// The first of `keys` any of the file's tags answers to.
///
/// Walks **every** tag rather than asking `primary_tag()`, which returns `None`
/// for an MP3 carrying only an ID3v1 block — that is, for most of a legacy
/// library. Preference order is lofty's own `tags()` order, which puts the
/// richer tag first.
fn first_of(tagged: &lofty::file::TaggedFile, keys: &[ItemKey]) -> Option<String> {
    let tags: &[Tag] = tagged.tags();
    keys.iter().find_map(|key| {
        tags.iter().find_map(|tag| {
            let value = tag.get_string(*key)?.trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
    })
}

/// `<Comment>` — the comment a player shows.
///
/// An ID3v2 tag can hold several `COMM` frames, told apart by a description.
/// The one with no description is the user's; iTunes keeps machine-written
/// data — Sound Check in `iTunNORM`, gapless playback in `iTunSMPB` — in
/// described ones, and on a file with no comment of its own that hex would
/// otherwise become the name. So an undescribed comment wins, a described one
/// is the fallback, and iTunes' own are never an answer. Every other format's
/// comments have no description, so there this is simply the first comment.
fn comment_of(tagged: &lofty::file::TaggedFile) -> Option<String> {
    let comments = || {
        tagged
            .tags()
            .iter()
            .flat_map(Tag::items)
            .filter(|item| item.key() == ItemKey::Comment)
    };
    let text = |item: &lofty::tag::TagItem| {
        let value = item.value().text()?.trim();
        (!value.is_empty()).then(|| value.to_owned())
    };
    comments()
        .filter(|item| item.description().is_empty())
        .find_map(text)
        .or_else(|| {
            comments()
                .filter(|item| !item.description().starts_with("iTun"))
                .find_map(text)
        })
}

/// `<Year>` — four digits, whatever shape the tag stores them in.
///
/// ID3v2.4 has no year frame: `TDRC` is a full recording *date*, so an MP3
/// would otherwise yield `"2012-03-04"` from `<Year>`. Taking the leading four
/// digits is what makes the tag mean what its name says on every format.
fn year_of(tagged: &lofty::file::TaggedFile) -> Option<String> {
    let raw = first_of(
        tagged,
        &[ItemKey::Year, ItemKey::RecordingDate, ItemKey::ReleaseDate],
    )?;
    let digits: String = raw.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() == 4 {
        Some(digits)
    } else {
        Some(raw)
    }
}

fn properties_of(tagged: &lofty::file::TaggedFile) -> AudioProperties {
    let properties = tagged.properties();
    AudioProperties {
        // Zero is lofty's "I could not tell", not a duration.
        duration: Some(properties.duration()).filter(|d| !d.is_zero()),
        bitrate_kbps: properties
            .audio_bitrate()
            .or_else(|| properties.overall_bitrate())
            .filter(|b| *b > 0),
        sample_rate_hz: properties.sample_rate().filter(|r| *r > 0),
        channels: properties.channels().filter(|c| *c > 0),
    }
}

/// Every file read so far, keyed on what would make the answer stale.
///
/// The P44 pattern, shared with every other reader through
/// [`super::cache::MetaCache`]: `OpKind::to_step` clones and a clone resets
/// `Cached` (D21), so an operation-local cache would re-read every file on
/// every keystroke.
static READ: OnceLock<MetaCache<Option<Arc<AudioTags>>>> = OnceLock::new();

fn cache() -> &'static MetaCache<Option<Arc<AudioTags>>> {
    READ.get_or_init(Default::default)
}

#[cfg(test)]
mod tests {
    use super::super::testing::Mp3;
    use super::*;
    use tempfile::TempDir;

    fn read(dir: &TempDir, name: &str, mp3: &Mp3) -> Option<Arc<AudioTags>> {
        let path = mp3.write(dir.path(), name);
        forget_all();
        tags_of(&path)
    }

    #[test]
    fn the_named_tags_are_read_from_an_id3v2_block() {
        let dir = TempDir::new().unwrap();
        let mp3 = Mp3::tagged("Metallica", "Nothing Else Matters")
            .frame("TALB", "Metallica")
            .frame("TRCK", "8")
            .frame("TDRC", "1991")
            .frame("TCON", "Metal")
            .frame("COMM", "a comment");
        let tags = read(&dir, "song.mp3", &mp3).expect("tags");

        assert_eq!(tags.artist.as_deref(), Some("Metallica"));
        assert_eq!(tags.title.as_deref(), Some("Nothing Else Matters"));
        assert_eq!(tags.album.as_deref(), Some("Metallica"));
        assert_eq!(tags.track.as_deref(), Some("8"));
        assert_eq!(tags.year.as_deref(), Some("1991"));
    }

    /// ID3v2.4 has no year frame — `TDRC` is a full date — so `<Year>` has to
    /// mean what its name says rather than rendering `2012-03-04`.
    #[test]
    fn year_is_four_digits_even_when_the_tag_holds_a_whole_date() {
        let dir = TempDir::new().unwrap();
        let mp3 = Mp3::tagged("A", "B").frame("TDRC", "2012-03-04T10:15");
        let tags = read(&dir, "dated.mp3", &mp3).expect("tags");
        assert_eq!(tags.year.as_deref(), Some("2012"));
    }

    /// `primary_tag()` answers `None` for a file carrying only ID3v1 — that is,
    /// for most of a legacy library. Walking every tag is what makes those
    /// files readable.
    #[test]
    fn an_id3v1_only_file_is_not_untagged() {
        let dir = TempDir::new().unwrap();
        let mp3 = Mp3 {
            id3v1: Some((
                "Enter Sandman".into(),
                "Metallica".into(),
                "Metallica".into(),
                "1991".into(),
                Some(1),
            )),
            audio: true,
            ..Default::default()
        };
        let tags = read(&dir, "legacy.mp3", &mp3).expect("an ID3v1 tag is a tag");
        assert_eq!(tags.artist.as_deref(), Some("Metallica"));
        assert_eq!(tags.title.as_deref(), Some("Enter Sandman"));
    }

    /// `<Comment>` is the comment a player shows — the `COMM` frame with no
    /// description. iTunes' own frames (`iTunNORM`, `iTunSMPB`) are hex a
    /// machine wrote, and on a file with no comment of its own they must not
    /// land in the filename.
    #[test]
    fn the_comment_is_the_one_a_player_shows_and_not_itunes_data() {
        let dir = TempDir::new().unwrap();
        let machine_only = Mp3::tagged("A", "B").frame("COMM:iTunNORM", " 00000A2F 00000B3C");
        let tags = read(&dir, "machine.mp3", &machine_only).expect("tags");
        assert_eq!(
            tags.comment, None,
            "iTunes' Sound Check data is not a comment"
        );

        let both = Mp3::tagged("A", "B")
            .frame("COMM:iTunSMPB", " 00000000 00000210")
            .frame("COMM", "the real one");
        let tags = read(&dir, "both.mp3", &both).expect("tags");
        assert_eq!(tags.comment.as_deref(), Some("the real one"));
    }

    /// `<Track>` pads and `<TrackN>` does not, so the raw value is kept and the
    /// number taken from it — which is also what makes `7/12` work.
    #[test]
    fn a_track_number_survives_however_it_is_written() {
        let dir = TempDir::new().unwrap();
        // lofty parses TRCK as a number: `7/12` reads as `7` — which is what
        // `<TrackN>`, the unpadded track number, should render anyway — and
        // a non-numeric track like a vinyl rip's `A1` does not survive the
        // parse at all. Recorded rather than worked around: the tag is
        // *missing* for such a file, which is what P32 wants, and inventing a
        // number for it would put `01` on a record whose sleeve says `A1`.
        for (raw, stored, number) in [
            ("7", Some("7"), Some(7)),
            ("7/12", Some("7"), Some(7)),
            ("A1", None, None),
        ] {
            let mp3 = Mp3::tagged("A", "B").frame("TRCK", raw);
            let tags = read(&dir, "t.mp3", &mp3).expect("tags");
            assert_eq!(tags.track.as_deref(), stored, "stored, for {raw}");
            assert_eq!(tags.track_number(), number, "number, for {raw}");
        }
    }

    #[test]
    fn the_extended_id3_names_are_read_by_their_canonical_spelling() {
        let dir = TempDir::new().unwrap();
        let mp3 = Mp3::tagged("A", "B")
            .frame("TPE2", "Various Artists")
            .frame("TCOM", "Ennio Morricone")
            .frame("TOWN", "somebody");
        let tags = read(&dir, "extended.mp3", &mp3).expect("tags");

        assert_eq!(tags.extended("AlbumArtist"), Some("Various Artists"));
        assert_eq!(tags.extended("Composer"), Some("Ennio Morricone"));
        // the historical misspelling for TOWN.
        assert_eq!(tags.extended("FilOowner"), Some("somebody"));
        assert_eq!(tags.extended("Conductor"), None);
    }

    /// Every reason a file is not readable audio is the same fact, and none is
    /// an error: P4 stops the run on any row error, so a folder of tracks with
    /// a readme in it must still run.
    #[test]
    fn anything_that_is_not_audio_is_simply_untagged() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"hello".repeat(40)).unwrap();
        assert_eq!(tags_of(&dir.path().join("notes.txt")), None);
        assert_eq!(tags_of(Path::new("/nowhere/at/all.mp3")), None);
    }

    /// Audio with no tags is still audio: `<Length>` and `<Bitrate>` come from
    /// the sound, not from anything anybody wrote. So the file answers, and it
    /// is the individual *tags* that report themselves absent.
    #[test]
    fn a_track_with_no_tags_still_has_properties() {
        let dir = TempDir::new().unwrap();
        let bare = Mp3 {
            audio: true,
            ..Default::default()
        };
        let tags = read(&dir, "bare.mp3", &bare).expect("audio is audio");
        assert_eq!(tags.artist, None);
        assert_eq!(tags.title, None);
        assert!(tags.extended.is_empty());
        assert_eq!(tags.properties.sample_rate_hz, Some(44_100));
    }

    /// The perf corpus is ten thousand zero-byte `.mp3` files. Without the size
    /// gate a music pipeline over it is ten thousand futile parses inside the
    /// preview, once per keystroke.
    #[test]
    fn a_file_too_small_to_hold_a_tag_is_never_parsed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.mp3");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(tags_of(&path), None);

        std::fs::write(&path, vec![0u8; (MIN_AUDIO_BYTES - 1) as usize]).unwrap();
        forget_all();
        assert_eq!(tags_of(&path), None);
    }

    /// And the gate must never be the *reason* a file fails.
    ///
    /// The smallest fixture that reads at all is a bare 34-byte ID3v2 tag, so
    /// the gate has to sit below that with room to spare. A threshold picked
    /// for convenience — 128, say, because that is an ID3v1 trailer — would
    /// swallow it, and the failure would look exactly like a file with no tags.
    #[test]
    fn the_size_gate_sits_below_the_smallest_file_that_reads() {
        let dir = TempDir::new().unwrap();
        let small = Mp3 {
            id3v2: vec![("TPE1", "A".into()), ("TIT2", "B".into())],
            audio: false,
            ..Default::default()
        };
        let size = small.bytes().len() as u64;
        assert!(
            MIN_AUDIO_BYTES < size,
            "the gate ({MIN_AUDIO_BYTES}) would reject a real {size}-byte tag"
        );
        let tags = read(&dir, "small.mp3", &small).expect("a short tag is still a tag");
        assert_eq!(tags.artist.as_deref(), Some("A"));
    }

    /// lofty reports `0` for a bitrate it could not work out. A confident wrong
    /// answer is worse than no answer — P32 already knows how to say "no
    /// answer".
    #[test]
    fn properties_it_could_not_determine_are_absent_rather_than_zero() {
        let dir = TempDir::new().unwrap();
        // Tags but no audio frames: nothing to measure.
        let tagless_audio = Mp3 {
            id3v2: vec![("TPE1", "A".into()), ("TIT2", "B".into())],
            audio: false,
            ..Default::default()
        };
        let tags = read(&dir, "no-frames.mp3", &tagless_audio).expect("tags");
        let props = tags.properties;
        assert_eq!(props.duration, None, "no frames, no duration");
        assert_eq!(props.bitrate_kbps, None);
        assert_eq!(props.sample_rate_hz, None);
        assert_eq!(props.channels, None);
        // And the tags themselves came through regardless.
        assert_eq!(tags.artist.as_deref(), Some("A"));
    }

    #[test]
    fn real_audio_frames_yield_real_properties() {
        let dir = TempDir::new().unwrap();
        let tags = read(&dir, "audio.mp3", &Mp3::tagged("A", "B")).expect("tags");
        let props = tags.properties;
        assert_eq!(props.sample_rate_hz, Some(44_100));
    }

    /// The first music file inside a folder supplies the folder's tags.
    #[test]
    fn a_folder_takes_the_tags_of_the_first_music_file_inside_it() {
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("album");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("readme.txt"), b"not music").unwrap();
        Mp3::tagged("Second", "B").write(&inner, "b.mp3");
        Mp3::tagged("First", "A").write(&inner, "a.mp3");
        forget_all();

        let tags = folder_tags(&inner).expect("a folder speaks through its first track");
        assert_eq!(tags.artist.as_deref(), Some("First"));
    }

    /// A symlink row is read through to the track it points at — the size
    /// gate included. The link itself is only as long as the path it holds,
    /// which is under the gate, so the row read as untagged.
    #[cfg(unix)]
    #[test]
    fn a_short_symlink_to_a_tagged_track_reads_the_track() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("lib")).unwrap();
        Mp3::tagged("Linked", "Song").write(&dir.path().join("lib"), "a.mp3");
        let link = dir.path().join("link.mp3");
        std::os::unix::fs::symlink("lib/a.mp3", &link).unwrap();
        forget_all();

        let entry = crate::model::FileEntry::from_path(&link).unwrap();
        assert!(entry.size < MIN_AUDIO_BYTES, "the link itself is tiny");
        let tags = tags_of_entry(&entry).expect("the target's tags");
        assert_eq!(tags.artist.as_deref(), Some("Linked"));
    }

    /// The read is shared process-wide, or a 10 000-file listing is 10 000
    /// opens per keystroke (P44).
    ///
    /// Retried, and the retry is not papering over anything. The cache is
    /// process-wide by design and twenty-odd tests across the crate call
    /// [`forget_all`]; `cargo test` runs them on several threads, so one of
    /// them clearing the cache between these two reads makes them miss for a
    /// reason that has nothing to do with the property under test. A genuine
    /// caching regression fails *every* attempt, so the assertion still bites —
    /// it is only the interleaving that is retried.
    #[test]
    fn the_same_file_is_read_once() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B").write(dir.path(), "shared.mp3");
        let shared = (0..8).any(|_| {
            let first = tags_of(&path).expect("tags");
            let second = tags_of(&path).expect("tags");
            Arc::ptr_eq(&first, &second)
        });
        assert!(shared, "the file was read twice on every attempt");
    }
}
