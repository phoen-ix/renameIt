//! What an image says about itself: its size, its colour depth, its comment.
//!
//! The image tags:
//!
//! > *`<Width>` - Image Width*
//! > *`<Height>` - Image Height*
//! > *`<Depth>` - Image Color Depth (Colors)*
//! > *`<Depthb>` - Image Color Depth (Bits)*
//! > *`<JpgComment>` - Jpeg Comment*
//!
//! The four size/depth tags would mean the same things about a video frame.
//! We answer for images only; the movie half stays deferred.
//!
//! Header-only, like every other reader here: `image` decodes just enough to
//! answer, and `jpeg_comment` walks segment lengths without touching the
//! entropy-coded data. Cached the way `meta::exif` is, and for the same reason
//! — this is reached from the parallel evaluation pass, once per file per
//! keystroke (P44).

use std::io::{BufReader, Read};

use image::ImageDecoder;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use super::cache::{MetaCache, Stamp};

/// Below this there is no header to read. **P50**, the same gate and the same
/// reasoning as `meta::exif`: set at the smallest thing that could be real
/// rather than a comfortable round number.
const MIN_IMAGE_BYTES: u64 = 16;

/// Extensions worth opening — exactly the formats the `image` feature list
/// enables, so a file we cannot read is never opened hopefully.
///
/// PSD, PSP, AGP, ILBM/IFF and PCX are deliberately absent. See D122: those
/// are formats without a maintained Rust decoder in the licence class D2
/// allows, and claiming them would mean answering `None` for a file the docs
/// said we handled.
pub const IMAGE_EXTENSIONS: [&str; 10] = [
    "bmp", "gif", "jpg", "jpeg", "jpe", "jfif", "png", "tif", "tiff", "tga",
];

/// A comment longer than this is not a filename component anybody wants, and
/// reading it into memory on every keystroke is not free.
const MAX_COMMENT_BYTES: usize = 4096;

/// One image's header, read once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    /// Bits per pixel as the *file* stores them, not as a decode would produce
    /// them: a 1-bit bitmap answers 1, not the 8 it would expand to.
    pub bits: u16,
    /// The JPEG `COM` segment, if there is one and it is not blank.
    pub comment: Option<String>,
}

impl ImageInfo {
    /// *"Color Depth (Colors)"* — the palette size the bit depth implies.
    ///
    /// Saturating at 63 bits keeps the shift defined; nothing real comes close,
    /// and a wrong-but-finite answer beats a panic in a filename preview.
    pub fn colours(&self) -> u64 {
        1u64 << self.bits.min(63)
    }
}

/// Everything about one image, cached on its path, length and mtime.
pub fn info_of(path: &Path) -> Option<Arc<ImageInfo>> {
    info_at(path, Stamp::stat(path)?)
}

/// The same, for a listed entry — the parallel pass's way in, keyed on the
/// entry's own stamp so it costs no syscall (see `meta::cache`). A folder row
/// answers with the first image inside it.
pub fn info_of_entry(entry: &crate::model::FileEntry) -> Option<Arc<ImageInfo>> {
    let stamp = Stamp::of_entry(entry);
    if entry.is_dir {
        folder_info_at(&entry.path, stamp)
    } else {
        info_at(&entry.path, stamp)
    }
}

fn info_at(path: &Path, stamp: Stamp) -> Option<Arc<ImageInfo>> {
    if !stamp.is_dir && stamp.len < MIN_IMAGE_BYTES {
        return None;
    }
    cache().get_or_read(path, stamp, || read_uncached(path).map(Arc::new))
}

/// The same, for a folder: the first image inside it.
///
/// **P51**, the rule `<ExifDate>` and `<Artist>` already follow: the first
/// music or image file inside the folder speaks for it — and the first that
/// actually *answers*, so an unreadable file does not veto the photograph
/// beside it.
pub fn folder_info(dir: &Path) -> Option<Arc<ImageInfo>> {
    folder_info_at(dir, Stamp::stat(dir)?)
}

/// Cached on the folder's own mtime, like `exif::folder_date`: a listing of
/// folders would otherwise re-read every directory on every keystroke.
fn folder_info_at(dir: &Path, stamp: Stamp) -> Option<Arc<ImageInfo>> {
    cache().get_or_read(dir, stamp, || {
        super::folder::first_inside(dir, &IMAGE_EXTENSIONS, info_of)
    })
}

/// How many files have actually had their header parsed.
///
/// The instrument `meta::audio` and `meta::exif` have both had since M6, added
/// here for the reason **P52** gives and one more: F9 now clears this cache
/// (D140), and a count is the only way to see that it did. A timing ratio
/// cannot tell a warm cache from a busy machine.
#[doc(hidden)]
pub fn parses_so_far() -> usize {
    PARSES.load(std::sync::atomic::Ordering::Relaxed)
}

static PARSES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn read_uncached(path: &Path) -> Option<ImageInfo> {
    PARSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let reader = image::ImageReader::open(path)
        .ok()?
        // The extension got us here; the magic bytes decide. A `.png` that is
        // really a JPEG is common enough that trusting the name would answer
        // `None` for a file that is perfectly readable.
        .with_guessed_format()
        .ok()?;
    let is_jpeg = reader.format() == Some(image::ImageFormat::Jpeg);
    let decoder = reader.into_decoder().ok()?;

    let (width, height) = decoder.dimensions();
    // `original_color_type` rather than `color_type`: the second reports what a
    // decode would *produce*, so every 1-bit and 4-bit image would answer 8.
    let bits = decoder.original_color_type().bits_per_pixel();
    drop(decoder);

    Some(ImageInfo {
        width,
        height,
        bits,
        comment: is_jpeg.then(|| jpeg_comment(path)).flatten(),
    })
}

/// The JPEG `COM` segment.
///
/// Walked here rather than taken from `image`, which does not surface it. A
/// JPEG is a chain of `FF <marker> <u16 length> <payload>` segments; the ones
/// without a payload are the restart markers and `SOI`, and `SOS` starts the
/// entropy-coded data, where segment lengths stop meaning anything. So the walk
/// is short by construction — it never reaches the image itself.
///
/// The standard gives `COM` no encoding, and in practice it is ASCII or UTF-8.
/// Lossy conversion is the honest reading: a comment we cannot decode becomes
/// visible replacement characters rather than a silently missing tag.
fn jpeg_comment(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);

    let mut pair = [0u8; 2];
    reader.read_exact(&mut pair).ok()?;
    if pair != [0xFF, 0xD8] {
        return None; // Not SOI, so not a JPEG we can walk.
    }

    let mut byte = [0u8; 1];
    loop {
        reader.read_exact(&mut byte).ok()?;
        if byte[0] != 0xFF {
            return None; // Out of step with the segment chain.
        }
        // Any number of 0xFF bytes may pad the gap before a marker.
        let mut marker = 0xFFu8;
        while marker == 0xFF {
            reader.read_exact(&mut byte).ok()?;
            marker = byte[0];
        }

        match marker {
            0x01 | 0xD0..=0xD8 => continue, // Standalone: no length follows.
            0xD9 | 0xDA => return None,     // EOI, or the scan starts here.
            _ => {}
        }

        reader.read_exact(&mut pair).ok()?;
        let length = u16::from_be_bytes(pair) as usize;
        let payload = length.checked_sub(2)?;

        if marker != 0xFE {
            std::io::copy(
                &mut reader.by_ref().take(payload as u64),
                &mut std::io::sink(),
            )
            .ok()?;
            continue;
        }

        if payload > MAX_COMMENT_BYTES {
            return None;
        }
        let mut buffer = vec![0u8; payload];
        reader.read_exact(&mut buffer).ok()?;
        // Trailing NULs are common — writers pad the segment.
        let text = String::from_utf8_lossy(&buffer)
            .trim_matches(|c: char| c.is_whitespace() || c == '\0')
            .to_owned();
        return (!text.is_empty()).then_some(text);
    }
}

/// Shared with every other reader through [`super::cache::MetaCache`], and
/// sized by the same constant: the preview budget is written for 10 000
/// files, so the capacity has to be comfortably past that or the cache serves
/// almost no hits.
static READ: OnceLock<MetaCache<Option<Arc<ImageInfo>>>> = OnceLock::new();

fn cache() -> &'static MetaCache<Option<Arc<ImageInfo>>> {
    READ.get_or_init(Default::default)
}

/// Drops every cached read. Tests only: two tempdirs can reuse a path.
pub fn forget_all() {
    if let Some(cache) = READ.get() {
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A 2×3 8-bit greyscale PNG, written by the same crate that reads it.
    fn png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let buffer = image::GrayImage::new(2, 3);
        buffer.save(&path).unwrap();
        path
    }

    /// A JPEG with a `COM` segment spliced in right after `SOI`, which is where
    /// a writer puts one.
    fn jpeg_with_comment(dir: &Path, name: &str, comment: Option<&str>) -> PathBuf {
        let path = dir.join(name);
        let mut raw = Vec::new();
        image::ImageBuffer::<image::Rgb<u8>, _>::new(4, 4)
            .write_to(
                &mut std::io::Cursor::new(&mut raw),
                image::ImageFormat::Jpeg,
            )
            .unwrap();

        if let Some(text) = comment {
            let mut out = raw[..2].to_vec(); // SOI
            let length = (text.len() + 2) as u16;
            out.extend_from_slice(&[0xFF, 0xFE]);
            out.extend_from_slice(&length.to_be_bytes());
            out.extend_from_slice(text.as_bytes());
            out.extend_from_slice(&raw[2..]);
            raw = out;
        }
        std::fs::write(&path, raw).unwrap();
        path
    }

    /// An 8x8 **bilevel** TIFF, uncompressed, built by hand.
    ///
    /// Written out byte by byte because no encoder in the tree emits 1-bit —
    /// and 1-bit is the only depth among our formats where the file's own
    /// colour type and the one a decode would produce disagree. Without it
    /// nothing here could tell `original_color_type` from `color_type`, and a
    /// fax-style scan would answer `<Depthb>` 8.
    fn bilevel_tiff(dir: &Path, name: &str) -> PathBuf {
        const IFD_AT: u32 = 8;
        const ENTRIES: u16 = 9;
        // header + count + entries + next-IFD pointer
        const DATA_AT: u32 = IFD_AT + 2 + ENTRIES as u32 * 12 + 4;
        const SHORT: u16 = 3;
        const LONG: u16 = 4;

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"II"); // little-endian
        out.extend_from_slice(&42u16.to_le_bytes());
        out.extend_from_slice(&IFD_AT.to_le_bytes());
        out.extend_from_slice(&ENTRIES.to_le_bytes());

        // Tags must be in ascending order; a SHORT value sits inline in the
        // low half of the 4-byte field.
        for (tag, kind, value) in [
            (256u16, SHORT, 8u32), // ImageWidth
            (257, SHORT, 8),       // ImageLength
            (258, SHORT, 1),       // BitsPerSample
            (259, SHORT, 1),       // Compression: none
            (262, SHORT, 0),       // Photometric: white is zero
            (273, LONG, DATA_AT),  // StripOffsets
            (277, SHORT, 1),       // SamplesPerPixel
            (278, SHORT, 8),       // RowsPerStrip
            (279, LONG, 8),        // StripByteCounts: 8 rows of 1 byte
        ] {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes()); // count
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        assert_eq!(out.len() as u32, DATA_AT, "the strip offset must be right");
        out.extend_from_slice(&[0b1010_1010; 8]);

        let path = dir.join(name);
        std::fs::write(&path, out).unwrap();
        path
    }

    /// The depth a *file* stores, not the depth a decode would produce.
    #[test]
    fn a_bilevel_image_is_one_bit_and_two_colours() {
        forget_all();
        let dir = TempDir::new().unwrap();
        let info = info_of(&bilevel_tiff(dir.path(), "scan.tiff")).expect("a readable tiff");
        assert_eq!((info.width, info.height), (8, 8));
        assert_eq!(info.bits, 1, "a decode would expand this to 8");
        assert_eq!(info.colours(), 2);
    }

    #[test]
    fn an_image_reports_its_size_and_depth() {
        forget_all();
        let dir = TempDir::new().unwrap();
        let info = info_of(&png(dir.path(), "a.png")).expect("a readable png");
        assert_eq!((info.width, info.height), (2, 3));
        assert_eq!(info.bits, 8, "8-bit greyscale");
        assert_eq!(info.colours(), 256);
    }

    /// *"Color Depth (Colors)"* is the palette the bits imply, so a 24-bit
    /// image answers with a number nobody would type by hand.
    #[test]
    fn colours_follow_from_bits() {
        for (bits, colours) in [(1u16, 2u64), (8, 256), (24, 16_777_216)] {
            let info = ImageInfo {
                width: 1,
                height: 1,
                bits,
                comment: None,
            };
            assert_eq!(info.colours(), colours);
        }
    }

    #[test]
    fn a_jpeg_comment_is_read_and_its_absence_is_not_an_error() {
        forget_all();
        let dir = TempDir::new().unwrap();
        let with = jpeg_with_comment(dir.path(), "with.jpg", Some("Holiday 2009"));
        assert_eq!(
            info_of(&with).unwrap().comment.as_deref(),
            Some("Holiday 2009")
        );

        let without = jpeg_with_comment(dir.path(), "without.jpg", None);
        assert_eq!(info_of(&without).unwrap().comment, None);
        // And it still answered the rest, which is the point of `None` here.
        assert_eq!(info_of(&without).unwrap().width, 4);
    }

    /// The walk must never reach the entropy-coded data, where a `FF FE` pair
    /// is just two bytes of a compressed image and not a comment at all.
    #[test]
    fn the_walk_stops_at_the_scan() {
        forget_all();
        let dir = TempDir::new().unwrap();
        let path = jpeg_with_comment(dir.path(), "plain.jpg", None);
        assert_eq!(jpeg_comment(&path), None);
    }

    /// A PNG has no `COM` segment to find, and asking is not a failure.
    #[test]
    fn only_a_jpeg_is_asked_for_a_comment() {
        forget_all();
        let dir = TempDir::new().unwrap();
        assert_eq!(info_of(&png(dir.path(), "b.png")).unwrap().comment, None);
    }

    /// P51: a folder answers with the first image inside it, by name.
    #[test]
    fn a_folder_takes_the_first_image_inside_it() {
        forget_all();
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("holiday");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("readme.txt"), b"not an image").unwrap();
        image::GrayImage::new(9, 9)
            .save(inner.join("b.png"))
            .unwrap();
        image::GrayImage::new(4, 5)
            .save(inner.join("a.png"))
            .unwrap();

        let info = folder_info(&inner).expect("the folder peeks inside");
        assert_eq!((info.width, info.height), (4, 5), "a.png sorts first");
    }

    /// P50: something far too small to be an image is not opened.
    #[test]
    fn a_file_too_small_to_be_an_image_is_not_read() {
        forget_all();
        let dir = TempDir::new().unwrap();
        let tiny = dir.path().join("tiny.png");
        std::fs::write(&tiny, b"x").unwrap();
        assert_eq!(info_of(&tiny), None);
    }
}
