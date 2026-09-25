//! Reading the date a photograph was taken.
//!
//! The case this exists for: photographs edited in an image editor carry the
//! date of the edit as their modified date, and Set Date with the Exif source
//! is how they get the date they were taken back. So the point is the shutter
//! time, not the last edit — which is what fixes the order the three
//! candidate tags are tried in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use super::cache::{MetaCache, Stamp};

/// Below this an image cannot carry an Exif block of any kind.
///
/// **P50**: *"Every reader is gated on a minimum size, set at the smallest
/// thing that could possibly be real."* `meta::audio` has had one since M6
/// step 2; this reader went from `fs::metadata` straight to the parse, so the
/// policy was true of one reader and not the other. A JPEG is `FF D8` plus an
/// APP1 segment; the smallest thing that could carry a date is well past this.
///
/// Set at the smallest plausible thing rather than a comfortable round number,
/// for the reason P50 gives: a threshold that rejects a legitimate file to save
/// time on an illegitimate one is a bug waiting for somebody with a very small
/// image, and its failure mode is indistinguishable from "no Exif".
const MIN_IMAGE_BYTES: u64 = 16;

use chrono::{NaiveDate, NaiveDateTime};

/// The "smart" order.
///
/// `DateTimeOriginal` first because it is the shutter time and nothing rewrites
/// it. `DateTime` (IFD0) last because that is precisely the one an image editor
/// overwrites — the value Set Date is recovering *from*.
const DATE_TAGS: [exif::Tag; 3] = [
    exif::Tag::DateTimeOriginal,
    exif::Tag::DateTimeDigitized,
    exif::Tag::DateTime,
];

/// Extensions worth opening when peeking inside a folder.
const IMAGE_EXTENSIONS: [&str; 8] = ["jpg", "jpeg", "jpe", "jfif", "tif", "tiff", "heic", "heif"];

/// The Exif date of one file, or `None`.
///
/// `None` covers every reason at once — not an image, no APP1 segment, a blank
/// value, a malformed one. To a renamer they are the same fact, and none of
/// them is an error: a folder of 500 photos with three PNGs in it must still
/// run, which it could not if this blocked (P4 stops on any row error).
pub fn date_of(path: &Path) -> Option<NaiveDateTime> {
    date_at(path, Stamp::stat(path)?)
}

/// The same, for a listed entry — the parallel pass's way in, keyed on the
/// entry's own stamp so it costs no syscall (see `meta::cache`). A folder row
/// answers with the first image inside it.
pub fn date_of_entry(entry: &crate::model::FileEntry) -> Option<NaiveDateTime> {
    let stamp = Stamp::of_entry(entry);
    if entry.is_dir {
        folder_date_at(&entry.path, stamp)
    } else {
        date_at(&entry.path, stamp)
    }
}

fn date_at(path: &Path, stamp: Stamp) -> Option<NaiveDateTime> {
    // A folder has no Exif of its own: the only way it gets a date is the
    // peek, which is `folder_date_at`. Answered before the cache, because the
    // peek caches under the same path — and where the two stamps agree, a
    // cached refusal here answered the peek, or the peek's date answered Set
    // Date with the peek switched off.
    if stamp.is_dir || stamp.len < MIN_IMAGE_BYTES {
        return None;
    }
    dates().get_or_read(path, stamp, || read_uncached(path))
}

/// This also works on folders: the first image inside supplies the Exif date.
///
/// First by name among the direct children, so two runs agree. Cached on the
/// folder's own mtime, so a 10 000-folder listing costs one pass rather than
/// one per keystroke.
pub fn folder_date(dir: &Path) -> Option<NaiveDateTime> {
    folder_date_at(dir, Stamp::stat(dir)?)
}

fn folder_date_at(dir: &Path, stamp: Stamp) -> Option<NaiveDateTime> {
    dates().get_or_read(dir, stamp, || {
        // "the first image": the first that actually yields a date, so a
        // thumbnail with no Exif does not veto the photo beside it.
        candidates_in(dir, stamp)
            .iter()
            .find_map(|path| date_of(path))
    })
}

/// The same peek, for any Exif **field**.
///
/// The folder peek was first written for the date, but nothing in it is
/// specific to the date — and a folder answering `<ExifDate>` while
/// `<Exif-Model>` came back empty is an inconsistency the user has no way to
/// explain. Same rule as [`folder_date`]: first by name among the direct
/// children, and the first that actually *yields the field*, so an image with
/// no Exif does not veto the photograph beside it (P51).
///
/// The per-file field set is cached by `fields_of`, and the folder's sorted
/// candidate list is cached on the folder's own mtime by [`candidates_in`] —
/// it used to be a `read_dir` plus a sort per folder row per keystroke.
pub fn folder_field(dir: &Path, name: &str) -> Option<String> {
    folder_field_at(dir, Stamp::stat(dir)?, name)
}

/// [`folder_field`] for a listed folder, without the `stat`.
pub fn folder_field_of_entry(entry: &crate::model::FileEntry, name: &str) -> Option<String> {
    folder_field_at(&entry.path, Stamp::of_entry(entry), name)
}

fn folder_field_at(dir: &Path, stamp: Stamp, name: &str) -> Option<String> {
    candidates_in(dir, stamp)
        .iter()
        .find_map(|path| field_of(path, name))
}

/// The images directly inside `dir`, sorted by name, cached on the folder's
/// mtime — which changes when an entry is added or removed, so the list can
/// only be stale in the direction of a file that has since appeared (D130).
fn candidates_in(dir: &Path, stamp: Stamp) -> Arc<[PathBuf]> {
    static CANDIDATES: OnceLock<MetaCache<Arc<[PathBuf]>>> = OnceLock::new();
    CANDIDATES
        .get_or_init(Default::default)
        .get_or_read(dir, stamp, || {
            let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                // A subfolder named `2019.jpg` is not a photograph.
                .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
                .map(|e| e.path())
                .filter(|p| super::folder::has_extension(p, &IMAGE_EXTENSIONS))
                .collect();
            names.sort();
            names.into()
        })
}

/// Any Exif field, by the name the standard gives it.
///
/// A passthrough rather than an enumeration: `kamadak-exif` knows about 161
/// tags including the whole GPS group, and matching against the parsed file's
/// own field names delivers every one of them for the cost of this function —
/// and picks up whatever a future version of the crate adds, for free.
///
/// Vendor makernotes are the gap: several hundred Canon/Nikon/Olympus
/// makernote fields are not decoded by `kamadak-exif`, and are not offered.
///
/// Values are rendered from their typed components rather than through
/// `Field::display_value()`, which quotes ASCII (`"Canon"`, with the quotes)
/// and writes rationals as `1/125` — a slash, which D31 would read as a
/// subfolder move.
pub fn field_of(path: &Path, name: &str) -> Option<String> {
    let fields = fields_of(path)?;
    fields
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// Every primary-IFD field of one image, read **once**.
///
/// Cached for the reason P44 gives — this is reached from the parallel
/// evaluation pass, once per file per keystroke — and cached as the whole *set*
/// rather than per field, because a pattern like
/// `<Exif-Make> <Exif-Model>` would otherwise parse the same photograph twice
/// per file per keystroke and a five-tag pattern five times. It was uncached
/// entirely until M6 step 7, which made `<Exif-Name>` the one reader in the
/// engine that reopened its file on every keystroke.
pub fn fields_of(path: &Path) -> Option<Arc<BTreeMap<String, String>>> {
    fields_at(path, Stamp::stat(path)?)
}

/// [`fields_of`] for a listed file, without the `stat`.
pub fn fields_of_entry(entry: &crate::model::FileEntry) -> Option<Arc<BTreeMap<String, String>>> {
    fields_at(&entry.path, Stamp::of_entry(entry))
}

/// [`field_of`] for a listed file, without the `stat`.
pub fn field_of_entry(entry: &crate::model::FileEntry, name: &str) -> Option<String> {
    let fields = fields_of_entry(entry)?;
    fields
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn fields_at(path: &Path, stamp: Stamp) -> Option<Arc<BTreeMap<String, String>>> {
    if stamp.is_dir || stamp.len < MIN_IMAGE_BYTES {
        return None;
    }
    fields().get_or_read(path, stamp, || read_fields(path).map(Arc::new))
}

fn read_fields(path: &Path) -> Option<BTreeMap<String, String>> {
    PARSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let exif = open_exif(path)?;

    Some(
        exif.fields()
            .filter(|field| field.ifd_num == exif::In::PRIMARY)
            .map(|field| (field.tag.to_string(), value_text(&field.value)))
            .collect(),
    )
}

/// One field's value as text.
///
/// Deliberately not `display_value()`: it wraps ASCII in quotes and prints a
/// rational as `num/den`, and both of those end up in a filename.
fn value_text(value: &exif::Value) -> String {
    fn join(parts: Vec<String>) -> String {
        parts.join(" ")
    }
    match value {
        exif::Value::Ascii(parts) => join(
            parts
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
                .collect(),
        ),
        exif::Value::Byte(v) => join(v.iter().map(u8::to_string).collect()),
        exif::Value::Short(v) => join(v.iter().map(u16::to_string).collect()),
        exif::Value::Long(v) => join(v.iter().map(u32::to_string).collect()),
        exif::Value::SByte(v) => join(v.iter().map(i8::to_string).collect()),
        exif::Value::SShort(v) => join(v.iter().map(i16::to_string).collect()),
        exif::Value::SLong(v) => join(v.iter().map(i32::to_string).collect()),
        exif::Value::Float(v) => join(v.iter().map(f32::to_string).collect()),
        exif::Value::Double(v) => join(v.iter().map(f64::to_string).collect()),
        // A rational is where `display_value` would write `1/125`. Rendered as
        // a decimal instead, because a slash in a name is a subfolder move.
        exif::Value::Rational(v) => join(
            v.iter()
                .map(|r| ratio(r.num.into(), r.denom.into()))
                .collect(),
        ),
        exif::Value::SRational(v) => join(
            v.iter()
                .map(|r| ratio(r.num.into(), r.denom.into()))
                .collect(),
        ),
        // Never rendered. A MakerNote is tens of kilobytes of binary and
        // `display_value` would hex-dump the lot into a filename.
        exif::Value::Undefined(..) | exif::Value::Unknown(..) => String::new(),
    }
}

/// A rational as a decimal, never as `num/den`.
fn ratio(num: i64, denom: i64) -> String {
    if denom == 0 {
        // Exif writes 0/0 for "unknown". Saying nothing is better than `NaN`.
        return String::new();
    }
    trim_decimal(num as f64 / denom as f64)
}

/// `2.8` rather than `2.8000000001`, and `100` rather than `100.0`.
fn trim_decimal(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

fn read_uncached(path: &Path) -> Option<NaiveDateTime> {
    let exif = open_exif(path)?;

    // Always the primary IFD: the thumbnail carries its own dates, and they are
    // not the photograph's.
    DATE_TAGS.iter().find_map(|tag| {
        let field = exif.get_field(*tag, exif::In::PRIMARY)?;
        match &field.value {
            exif::Value::Ascii(parts) => parts.iter().find_map(|bytes| parse(bytes)),
            _ => None,
        }
    })
}

/// How much of a TIFF-based file is read for its Exif first.
///
/// Camera RAWs — CR2, NEF, ARW, DNG — are TIFFs, and their metadata sits at
/// the front, ahead of tens of megabytes of sensor data.
const TIFF_PREFIX: u64 = 1024 * 1024;

/// The largest TIFF-based file read whole when its metadata is not inside
/// [`TIFF_PREFIX`]. A TIFF may put its directory after the image — scanning
/// software often does — and such a file below this size still answers.
/// Above it the answer is "no Exif": a gigabyte scan read whole, eight at a
/// time from the parallel pass, is not a price a filename preview can pay.
const MAX_WHOLE_TIFF: u64 = 64 * 1024 * 1024;

fn open_exif(path: &Path) -> Option<exif::Exif> {
    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    exif_from(&mut std::io::BufReader::new(file), len)
}

/// The Exif in a file `len` bytes long.
///
/// kamadak-exif's `read_from_container` walks a JPEG, PNG, HEIF or WebP to
/// its Exif block, but reads anything with TIFF magic **whole** before
/// parsing it. So a TIFF-based file is handled here: the first MiB is parsed
/// on its own, which is where a camera writes, and only a file whose metadata
/// is not in there is read further — whole, if it is small enough.
fn exif_from<R: std::io::BufRead + std::io::Seek>(reader: &mut R, len: u64) -> Option<exif::Exif> {
    use std::io::{Read, SeekFrom};

    let magic = reader.fill_buf().ok()?;
    if !(magic.starts_with(b"II*\0") || magic.starts_with(b"MM\0*")) {
        return exif::Reader::new().read_from_container(reader).ok();
    }

    let mut head = Vec::new();
    reader
        .by_ref()
        .take(TIFF_PREFIX)
        .read_to_end(&mut head)
        .ok()?;
    let whole = (head.len() as u64) < TIFF_PREFIX;
    if let Ok(exif) = exif::Reader::new().read_raw(head) {
        return Some(exif);
    }
    if whole || len > MAX_WHOLE_TIFF {
        return None;
    }
    reader.seek(SeekFrom::Start(0)).ok()?;
    let mut all = Vec::new();
    reader.take(MAX_WHOLE_TIFF).read_to_end(&mut all).ok()?;
    exif::Reader::new().read_raw(all).ok()
}

/// `"YYYY:MM:DD HH:MM:SS"`, via the crate's own parser.
///
/// Zone offsets (`OffsetTimeOriginal`, EXIF 2.31+) are deliberately ignored:
/// the Exif date is treated as local wall-clock time, consistent with how
/// `<Date>` already renders and with what a camera actually records.
fn parse(bytes: &[u8]) -> Option<NaiveDateTime> {
    let parsed = exif::DateTime::from_ascii(bytes).ok()?;
    NaiveDate::from_ymd_opt(
        i32::from(parsed.year),
        u32::from(parsed.month),
        u32::from(parsed.day),
    )?
    .and_hms_opt(
        u32::from(parsed.hour),
        u32::from(parsed.minute),
        u32::from(parsed.second),
    )
}

/// The P40 pattern the regex and CSV caches use, and for the same reason:
/// `OpKind::to_step` clones, and a clone resets `Cached` by design (D21). An
/// op-local cache alone would mean opening 10 000 JPEGs **per keystroke** on
/// the preview worker. Shared with every other reader through
/// [`super::cache::MetaCache`]; keying on the mtime means Set Date's own write
/// invalidates the entry for free.
static READ: OnceLock<MetaCache<Option<NaiveDateTime>>> = OnceLock::new();

/// The same cache, for the whole field set rather than the date.
///
/// Separate only because the two answer different questions about the same
/// file and a shared value type would be a needless `enum`.
static FIELDS: OnceLock<MetaCache<Option<Fields>>> = OnceLock::new();

/// Every primary-IFD field of one image, shared between hits.
type Fields = Arc<BTreeMap<String, String>>;

fn dates() -> &'static MetaCache<Option<NaiveDateTime>> {
    READ.get_or_init(Default::default)
}

fn fields() -> &'static MetaCache<Option<Fields>> {
    FIELDS.get_or_init(Default::default)
}

/// How many files have actually been parsed.
///
/// The instrument `meta::audio` has had since M6 step 3, added here for the
/// same reason: a timing ratio cannot tell a warm cache from a busy machine,
/// and P52 records that the identical capacity flaw was latent in this cache
/// while nothing was watching it.
static PARSES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub fn parses_so_far() -> usize {
    PARSES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Empties both caches.
pub fn forget_all() {
    if let Some(cache) = READ.get() {
        cache.clear();
    }
    if let Some(cache) = FIELDS.get() {
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::jpeg_with_exif;
    use super::*;
    use tempfile::TempDir;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn the_shutter_time_is_read_from_a_jpeg() {
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "photo.jpg",
            &jpeg_with_exif(Some("2008:02:17 11:23:50"), None, None),
        );
        assert_eq!(date_of(&path), Some(at(2008, 2, 17, 11, 23, 50)));
    }

    /// The order is the edited-photo case: an image editor rewrites
    /// `DateTime`, so the shutter time has to win.
    #[test]
    fn the_original_date_wins_over_the_one_an_editor_rewrote() {
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "edited.jpg",
            &jpeg_with_exif(
                Some("2008:02:17 11:23:50"),
                Some("2009:03:18 12:24:51"),
                Some("2024:01:01 00:00:00"),
            ),
        );
        assert_eq!(
            date_of(&path),
            Some(at(2008, 2, 17, 11, 23, 50)),
            "DateTimeOriginal, not the editor's DateTime"
        );
    }

    #[test]
    fn digitized_is_the_second_choice_and_datetime_the_third() {
        let dir = TempDir::new().unwrap();
        let digitized = write(
            &dir,
            "b.jpg",
            &jpeg_with_exif(
                None,
                Some("2009:03:18 12:24:51"),
                Some("2024:01:01 00:00:00"),
            ),
        );
        assert_eq!(date_of(&digitized), Some(at(2009, 3, 18, 12, 24, 51)));

        let only_datetime = write(
            &dir,
            "c.jpg",
            &jpeg_with_exif(None, None, Some("2024:01:01 00:00:00")),
        );
        assert_eq!(date_of(&only_datetime), Some(at(2024, 1, 1, 0, 0, 0)));
    }

    /// Every "no date here" reason is the same fact, and none of them is an
    /// error — otherwise one PNG would block a folder of photographs.
    #[test]
    fn a_file_with_no_exif_date_is_simply_undated() {
        let dir = TempDir::new().unwrap();
        assert_eq!(date_of(&write(&dir, "notes.txt", b"hello")), None);
        assert_eq!(
            date_of(&write(&dir, "empty.jpg", &jpeg_with_exif(None, None, None))),
            None,
            "an APP1 segment carrying no date at all"
        );
        assert_eq!(date_of(Path::new("/nowhere/at/all.jpg")), None);
    }

    /// A folder takes the date of the first image inside it — first by name,
    /// so two runs agree.
    ///
    /// This test and the one below it are the two that catch a file's size
    /// gate being applied to the folder itself, which once rejected every
    /// folder on Windows, where a directory's length is 0. A folder's stamp
    /// now records 0 on every platform (`Stamp::stat`), so they catch it here
    /// too.
    #[test]
    fn a_folder_takes_the_date_of_the_first_image_inside_it() {
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("holiday");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("readme.txt"), b"not an image").unwrap();
        std::fs::write(
            inner.join("b.jpg"),
            jpeg_with_exif(Some("2009:03:18 12:24:51"), None, None),
        )
        .unwrap();
        std::fs::write(
            inner.join("a.jpg"),
            jpeg_with_exif(Some("2008:02:17 11:23:50"), None, None),
        )
        .unwrap();

        assert_eq!(
            folder_date(&inner),
            Some(at(2008, 2, 17, 11, 23, 50)),
            "a.jpg sorts first"
        );
    }

    /// An image with no date must not veto the one beside it.
    #[test]
    fn a_folder_skips_an_image_that_carries_no_date() {
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("mixed");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("a.jpg"), jpeg_with_exif(None, None, None)).unwrap();
        std::fs::write(
            inner.join("b.jpg"),
            jpeg_with_exif(Some("2008:02:17 11:23:50"), None, None),
        )
        .unwrap();

        assert_eq!(folder_date(&inner), Some(at(2008, 2, 17, 11, 23, 50)));
    }

    /// A folder answers `<Exif-*>` the same way it answers `<ExifDate>`.
    ///
    /// The folder peek was first written for the date, but nothing in the
    /// rule is specific to the date — and a folder that answered
    /// `<ExifDate>` while `<Exif-DateTimeOriginal>` came back empty is an
    /// inconsistency the user has no way to explain.
    #[test]
    fn a_folder_answers_a_field_from_the_first_image_inside_it() {
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("holiday");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("readme.txt"), b"not an image").unwrap();
        std::fs::write(
            inner.join("b.jpg"),
            jpeg_with_exif(Some("2009:03:18 12:24:51"), None, None),
        )
        .unwrap();
        std::fs::write(
            inner.join("a.jpg"),
            jpeg_with_exif(Some("2008:02:17 11:23:50"), None, None),
        )
        .unwrap();
        forget_all();

        // Same file the date peek picks, and the same first-by-name rule.
        assert_eq!(
            folder_field(&inner, "DateTimeOriginal").as_deref(),
            Some("2008:02:17 11:23:50"),
            "a.jpg sorts first"
        );
        // A field nothing inside carries is absent, not an error.
        assert_eq!(folder_field(&inner, "Model"), None);
    }

    /// And an image that lacks the field does not veto the one beside it (P51).
    #[test]
    fn a_folder_field_skips_an_image_that_does_not_carry_it() {
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("mixed");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("a.jpg"), jpeg_with_exif(None, None, None)).unwrap();
        std::fs::write(
            inner.join("b.jpg"),
            jpeg_with_exif(Some("2008:02:17 11:23:50"), None, None),
        )
        .unwrap();
        forget_all();

        assert_eq!(
            folder_field(&inner, "DateTimeOriginal").as_deref(),
            Some("2008:02:17 11:23:50")
        );
    }

    /// Set Date's folder peek, switched off and on for one folder.
    ///
    /// Peek off asks for the folder's own date, which it does not have; peek
    /// on asks for the first image inside. Both are asked about the same path,
    /// and where the two stamps agree — Windows reports a folder's length as
    /// 0, as the listing does — a shared cache entry answered the second
    /// question with the first one's answer.
    #[test]
    fn a_folder_has_no_date_of_its_own_whichever_question_came_first() {
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("holiday");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(
            inner.join("a.jpg"),
            jpeg_with_exif(Some("2008:02:17 11:23:50"), None, None),
        )
        .unwrap();
        let entry = crate::model::FileEntry::from_path(&inner).unwrap();
        let peeked = Some(at(2008, 2, 17, 11, 23, 50));

        for peek_first in [true, false] {
            forget_all();
            if peek_first {
                assert_eq!(date_of_entry(&entry), peeked);
                assert_eq!(date_of(&inner), None, "the peek's answer leaked");
            } else {
                assert_eq!(date_of(&inner), None);
                assert_eq!(date_of_entry(&entry), peeked, "the refusal leaked");
            }
        }
    }

    /// A reader over `head` followed by zeros up to `len`, counting the bytes
    /// it hands out — a camera RAW without the disk space.
    struct Padded {
        head: Vec<u8>,
        len: u64,
        at: u64,
        served: std::rc::Rc<std::cell::Cell<u64>>,
    }

    impl std::io::Read for Padded {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let left = self.len.saturating_sub(self.at);
            let n = (buf.len() as u64).min(left) as usize;
            for (i, byte) in buf[..n].iter_mut().enumerate() {
                let at = self.at as usize + i;
                *byte = self.head.get(at).copied().unwrap_or(0);
            }
            self.at += n as u64;
            self.served.set(self.served.get() + n as u64);
            Ok(n)
        }
    }

    impl std::io::Seek for Padded {
        fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
            self.at = match to {
                std::io::SeekFrom::Start(at) => at,
                std::io::SeekFrom::End(back) => self.len.saturating_add_signed(back),
                std::io::SeekFrom::Current(by) => self.at.saturating_add_signed(by),
            };
            Ok(self.at)
        }
    }

    /// The TIFF block inside the fixture's APP1 segment: a TIFF-magic file
    /// carrying Exif, the shape of every CR2, NEF, ARW and DNG.
    fn tiff_with_date(date: &str) -> Vec<u8> {
        let jpeg = jpeg_with_exif(Some(date), None, None);
        let at = jpeg.windows(6).position(|w| w == b"Exif\0\0").unwrap() + 6;
        jpeg[at..jpeg.len() - 2].to_vec()
    }

    /// A camera RAW is a TIFF, and kamadak-exif reads any TIFF-magic file
    /// whole before looking at it: the shipped Exif-date preset over a card of
    /// 45 MB raws read every byte of every one, eight at a time. The metadata
    /// sits at the front, so the front is what is read.
    #[test]
    fn a_large_tiff_is_read_for_its_exif_without_reading_it_whole() {
        let served = std::rc::Rc::new(std::cell::Cell::new(0));
        let len = 200 * 1024 * 1024;
        let raw = Padded {
            head: tiff_with_date("2008:02:17 11:23:50"),
            len,
            at: 0,
            served: served.clone(),
        };
        let exif = exif_from(&mut std::io::BufReader::new(raw), len).expect("its Exif");
        assert!(
            exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
                .is_some()
        );
        assert!(
            served.get() <= 2 * 1024 * 1024,
            "read {} bytes of a {len}-byte file",
            served.get()
        );
    }

    #[test]
    fn an_empty_folder_has_no_date() {
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("empty");
        std::fs::create_dir(&inner).unwrap();
        assert_eq!(folder_date(&inner), None);
    }
}
