//! Decoding a picture small enough to draw in a list.
//!
//! The sibling of [`super::image`] and deliberately not part of it: that module
//! reads headers only, and this one decodes every pixel. Keeping them apart
//! keeps that true, and keeps the one expensive thing in `meta` obvious.
//!
//! It lives in `ren-core` rather than beside the widget that draws it for one
//! reason: `image` is already a dependency here, and the GUI never needs to
//! learn what a `DynamicImage` is. It receives RGBA bytes, which is what
//! `egui::ColorImage::from_rgba_unmultiplied` takes.
//!
//! **Nothing here is cached.** Every other reader in `meta` is, because they
//! are reached from the parallel evaluation pass once per file per keystroke
//! (P44). This one is reached from a UI worker, for the rows on screen, and its
//! results are megabytes rather than four fields — a `CACHE_CAPACITY` map that
//! clears wholesale would be a memory leak with a cliff. The cache that holds
//! these is bounded by *bytes*, and it lives with the textures.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use image::{DynamicImage, ImageDecoder, ImageReader, Limits};

/// The largest picture worth opening, per edge.
///
/// **P76.** A 64 000 × 64 000 PNG is a valid file of about four kilobytes and
/// asks for twelve gigabytes of RGBA. The gate is on the *declared* dimensions,
/// so it costs a header read and refuses before anything is allocated.
const MAX_SOURCE_EDGE: u32 = 30_000;

/// The most one decode may allocate: the decoded picture, plus — for a JPEG —
/// the file itself, which `image`'s JPEG decoder holds in memory whole.
///
/// With at most four decode threads the worst-case transient is 1 GiB, on a
/// machine with enough cores to have four of them. Nothing else full-size is
/// allocated: the orientation is applied after the picture is scaled down.
const MAX_ALLOC: u64 = 256 * 1024 * 1024;

/// Below this there is no header to read — **P50**, the same gate and the same
/// number as every other reader in this module.
const MIN_IMAGE_BYTES: u64 = 16;

/// One picture, decoded small.
///
/// The bytes are RGBA8, row-major, no padding — exactly what
/// `egui::ColorImage::from_rgba_unmultiplied` wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Thumbnail {
    /// What this costs to hold. The cache is bounded by the sum of these.
    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }
}

/// Why a file has no thumbnail.
///
/// Carried rather than collapsed to `None` because the tile says which, and
/// because *"too large"* and *"not a picture"* are different answers to give a
/// user staring at a blank square.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoThumbnail {
    /// Not one of the formats D122 claims, or too small to hold a header.
    NotAPicture,
    /// Bigger than this program will decode (P76).
    TooLarge,
    /// Truncated, corrupt, or gone between the listing and the decode.
    Unreadable,
    /// The decoder panicked. `image` aims not to, and does not promise it.
    Panicked,
}

/// The size a picture becomes when it is scaled to fit a square box.
///
/// Pure, so every shape is testable without a file. Never returns a zero edge:
/// a 1000 × 3 panorama still has a row of pixels, and a zero-height buffer is
/// one `ColorImage` refuses.
pub fn fit(source: (u32, u32), max_edge: u32) -> (u32, u32) {
    let (w, h) = source;
    if w == 0 || h == 0 {
        return (0, 0);
    }
    // Already small enough: leave it alone rather than blowing it up. A 8×8
    // icon drawn at 64 is the *drawing's* job, and upscaling here would cost
    // sixteen times the memory to hold the same picture.
    if w <= max_edge && h <= max_edge {
        return (w, h);
    }
    if w >= h {
        (
            max_edge,
            ((h as u64 * max_edge as u64) / w as u64).max(1) as u32,
        )
    } else {
        (
            ((w as u64 * max_edge as u64) / h as u64).max(1) as u32,
            max_edge,
        )
    }
}

/// Decodes `path` and scales it to fit a `max_edge` square.
///
/// Everything hostile a file can do is refused here rather than survived
/// later: see [`NoThumbnail`], and the two `set_limits` calls below.
pub fn thumbnail(path: &Path, max_edge: u32) -> Result<Thumbnail, NoThumbnail> {
    if !is_image(path) {
        return Err(NoThumbnail::NotAPicture);
    }
    let len = match std::fs::metadata(path) {
        Ok(stamp) if stamp.is_file() && stamp.len() >= MIN_IMAGE_BYTES => stamp.len(),
        Ok(_) => return Err(NoThumbnail::NotAPicture),
        Err(_) => return Err(NoThumbnail::Unreadable),
    };

    // `image` aims not to panic on malformed input and does not guarantee it,
    // and this runs on a worker whose death would not look like a crash — it
    // would look like every later `settle()` stalling for its full deadline
    // (P75). A caught panic is just another file without a picture.
    let decoded =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode(path, len, max_edge)));

    match decoded {
        Ok(result) => {
            match &result {
                Ok(_) => DECODES.fetch_add(1, Ordering::Relaxed),
                Err(NoThumbnail::TooLarge) => REFUSALS.fetch_add(1, Ordering::Relaxed),
                Err(_) => 0,
            };
            result
        }
        Err(_) => {
            REFUSALS.fetch_add(1, Ordering::Relaxed);
            Err(NoThumbnail::Panicked)
        }
    }
}

/// A picture too big to decode, or a file that is not a picture at all.
///
/// Kept apart because the counters do: `refusals_so_far` counts what we
/// declined, and a corrupt file is not something anyone declined.
fn too_large_or_unreadable(error: image::ImageError) -> NoThumbnail {
    match error {
        image::ImageError::Limits(_) => NoThumbnail::TooLarge,
        _ => NoThumbnail::Unreadable,
    }
}

fn decode(path: &Path, len: u64, max_edge: u32) -> Result<Thumbnail, NoThumbnail> {
    // `Limits` is `#[non_exhaustive]`, so it is built by assignment rather
    // than by literal — which also means a future field arrives at its own
    // default rather than silently unset.
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_EDGE);
    limits.max_image_height = Some(MAX_SOURCE_EDGE);
    limits.max_alloc = Some(MAX_ALLOC);

    let mut reader = ImageReader::open(path)
        .map_err(|_| NoThumbnail::Unreadable)?
        // The extension got us here; the magic bytes decide. A `.png` that is
        // really a JPEG is common enough that trusting the name would refuse a
        // file that reads perfectly.
        .with_guessed_format()
        .map_err(|_| NoThumbnail::Unreadable)?;

    // **Before any of them: a JPEG's own bytes.** `image`'s JPEG decoder reads
    // its whole input into memory inside `into_decoder`, before a limit below
    // can act — for a file only *named* `.jpg` too, since the extension
    // decides when the magic says nothing. So the file's length is charged to
    // the same budget as the pixels, and a JPEG bigger than the budget is
    // refused from its length alone.
    if reader.format() == Some(image::ImageFormat::Jpeg) {
        limits.reserve(len).map_err(|_| NoThumbnail::TooLarge)?;
    }

    // Three gates follow, and only the middle one is load-bearing. Saying
    // which is which matters, because the obvious reading of this API is that
    // the first is enough and it is not.
    //
    // **One: the reader.** `ImageReader::make_decoder`'s own doc says "for all
    // formats except PNG, the limits are ignored and can be set with
    // `ImageDecoder::set_limits` after calling this function" — so an
    // implementation that sets this and stops is unprotected everywhere except
    // PNG. In practice `image` 0.25's TIFF decoder does enforce the dimension
    // limits when it is constructed, which is why the wide-picture test below
    // passes with this line deleted. Kept as the documented way to limit the
    // one decoder that cannot be limited later.
    reader.limits(limits.clone());
    // Two different failures come back from here — a picture too big, and a
    // file that is not one — and the tile says which. `ImageError::Limits` is
    // the only one that means hostile.
    let mut decoder = reader.into_decoder().map_err(too_large_or_unreadable)?;

    // So the other half, and in this order: `reserve` charges the output buffer
    // against the budget *before* a byte of pixel data is read, using
    // `total_bytes()`, which the header already told us.
    //
    // **Two: the allocation.** This is the gate that does the work, and the
    // only one of the three a test here can distinguish. Dimension limits do
    // not catch the shape that matters most — a picture whose *edges* are legal
    // and whose *allocation* is not. 20 000 × 20 000 is inside the 30 000-edge
    // limit and asks for 1.6 GB. `reserve` refuses it from the header, before a
    // byte of pixel data is read.
    limits
        .reserve(decoder.total_bytes())
        .map_err(|_| NoThumbnail::TooLarge)?;
    // **Three: the decoder's own working memory.** No test here discriminates
    // this — `reserve` above fires first for every case we can build — and it
    // is kept anyway: it is the documented way to bound what a decoder
    // allocates *internally*, which is not the output buffer and is not
    // something the two gates above see. An undemonstrated guard on a security
    // boundary is worth its line; pretending it is tested would not be.
    decoder
        .set_limits(limits)
        .map_err(too_large_or_unreadable)?;

    // Read before the pixels, because `from_decoder` consumes it. JPEG and TIFF
    // are the two of our six formats that carry one.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    let image = DynamicImage::from_decoder(decoder).map_err(|_| NoThumbnail::Unreadable)?;

    let (width, height) = fit((image.width(), image.height()), max_edge);
    if width == 0 || height == 0 {
        return Err(NoThumbnail::Unreadable);
    }
    // `thumbnail` is the fast integer box filter — the right trade for a
    // downscale of this ratio, where a Lanczos pass would cost far more and
    // show nothing at 128 pixels.
    let mut scaled = image.thumbnail(width, height);
    drop(image);
    // Not a nicety: without it every photograph taken on a phone is sideways,
    // which is the first thing anybody notices (P77). Applied to the small
    // picture, not the decoded one: a quarter turn copies the whole image, and
    // at full size that copy doubled the decode's peak memory. The box is
    // square, so turning after scaling gives the same dimensions.
    scaled.apply_orientation(orientation);
    let rgba = scaled.to_rgba8();

    Ok(Thumbnail {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Whether this path is worth handing to [`thumbnail`].
///
/// The same list [`super::image`] reads headers from, so the two can never
/// disagree about what a picture is — and so a folder of ten thousand text
/// files is never opened hopefully.
pub fn is_image(path: &Path) -> bool {
    super::folder::has_extension(path, &super::image::IMAGE_EXTENSIONS)
}

/// How many files have actually been decoded.
///
/// **P52**'s instrument, and here it does more work than usual: the texture
/// cache's hit rate is observable *only* through this counter, because nothing
/// caches pixels on this side. A stopwatch cannot tell a warm cache from a busy
/// machine, and here it cannot tell one from a small folder either.
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub fn decodes_so_far() -> usize {
    DECODES.load(Ordering::Relaxed)
}

/// How many files were refused for being hostile rather than broken.
///
/// Counted apart from failures deliberately: a guard that refuses silently is
/// indistinguishable from a corrupt file, and the only other test for it is one
/// whose failure mode is the machine swapping.
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub fn refusals_so_far() -> usize {
    REFUSALS.load(Ordering::Relaxed)
}

static DECODES: AtomicUsize = AtomicUsize::new(0);
static REFUSALS: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Serialises **every test that decodes**, not only the ones that read a
    /// counter.
    ///
    /// `decodes_so_far` and `refusals_so_far` are process-wide — that is the
    /// point of them — and `cargo test` runs this module's tests in parallel.
    /// Locking only the tests that assert on a delta is not enough and was
    /// wrong for two commits: a sibling that merely decodes a picture bumps the
    /// same counter, so the delta was a race with whichever test happened to
    /// run alongside. Nothing outside this module touches them, so one lock
    /// held by every caller of `thumbnail` makes the assertions exact rather
    /// than generous.
    static COUNTERS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Holds the lock and shrugs off a sibling's panic: a poisoned lock here
    /// means another test failed, not that this one cannot run.
    fn counting() -> std::sync::MutexGuard<'static, ()> {
        COUNTERS.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_thumbnail_fits_the_box_and_keeps_its_shape() {
        assert_eq!(fit((200, 50), 64), (64, 16));
        assert_eq!(fit((50, 200), 64), (16, 64));
        assert_eq!(fit((100, 100), 64), (64, 64));
    }

    /// A panorama still has a row of pixels. Rounding it to zero gives an empty
    /// buffer, which `ColorImage` refuses — a blank tile with no explanation.
    #[test]
    fn a_very_wide_picture_still_has_a_row_of_pixels() {
        assert_eq!(fit((1000, 3), 64), (64, 1));
        assert_eq!(fit((3, 1000), 64), (1, 64));
    }

    /// Upscaling would cost sixteen times the memory to hold the same picture.
    #[test]
    fn a_picture_smaller_than_the_box_is_not_blown_up() {
        assert_eq!(fit((8, 8), 64), (8, 8));
        assert_eq!(fit((0, 10), 64), (0, 0));
    }

    #[test]
    fn a_real_picture_is_decoded_to_four_bytes_a_pixel() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.png");
        image::GrayImage::new(200, 50).save(&path).unwrap();

        let thumb = thumbnail(&path, 64).expect("a readable png");
        assert_eq!((thumb.width, thumb.height), (64, 16));
        assert_eq!(
            thumb.rgba.len(),
            64 * 16 * 4,
            "RGBA whatever the file stored"
        );
        assert_eq!(thumb.bytes(), thumb.rgba.len());
    }

    /// A one-bit TIFF stores one bit a pixel and decodes to eight. The buffer
    /// is four bytes a pixel either way, which is what the caller relies on.
    #[test]
    fn a_bilevel_source_still_comes_back_rgba() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "scan.tiff",
            &super::super::testing::image::bilevel_tiff(),
        );

        let thumb = thumbnail(&path, 64).expect("a readable tiff");
        assert_eq!(thumb.rgba.len() as u32, thumb.width * thumb.height * 4);
    }

    #[test]
    fn a_file_that_is_not_a_picture_is_refused_without_being_opened() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();
        let before = decodes_so_far();

        let text = write(&dir, "notes.txt", b"just text");
        assert_eq!(thumbnail(&text, 64), Err(NoThumbnail::NotAPicture));

        // A format D122 declined, named like one we take.
        let psd = write(&dir, "art.psd", &[0u8; 512]);
        assert_eq!(thumbnail(&psd, 64), Err(NoThumbnail::NotAPicture));

        // The return value alone cannot discriminate — `image` would refuse a
        // PSD too. The counter is what proves we never opened it.
        assert_eq!(decodes_so_far(), before, "nothing was decoded");
    }

    /// **P50**, the same gate every other reader has.
    #[test]
    fn something_too_small_to_hold_a_header_is_not_opened() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();
        let before = decodes_so_far();
        let tiny = write(&dir, "tiny.png", b"x");
        assert_eq!(thumbnail(&tiny, 64), Err(NoThumbnail::NotAPicture));
        assert_eq!(decodes_so_far(), before);
    }

    #[test]
    fn a_truncated_picture_fails_without_panicking() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();
        let mut bytes = Vec::new();
        image::GrayImage::new(64, 64)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes.truncate(bytes.len() * 6 / 10);

        let path = write(&dir, "half.png", &bytes);
        assert_eq!(thumbnail(&path, 64), Err(NoThumbnail::Unreadable));
    }

    #[test]
    fn a_file_that_vanished_is_a_failure_not_a_panic() {
        let _counting = counting();
        assert_eq!(
            thumbnail(std::path::Path::new("/definitely/not/here.png"), 64),
            Err(NoThumbnail::Unreadable)
        );
    }

    /// A picture too wide to be one: 64 000 × 64 000, in a few hundred bytes.
    ///
    /// The easy half. Note what this test does **not** prove: deleting either
    /// `reader.limits` or `decoder.set_limits` leaves it green, because
    /// `reserve` catches this file too. The gate below is the one that is
    /// actually pinned.
    #[test]
    fn a_picture_wider_than_any_screen_is_refused() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "wide.tiff",
            &super::super::testing::image::tiff_claiming(64_000, 64_000),
        );

        let refusals = refusals_so_far();
        assert_eq!(thumbnail(&path, 64), Err(NoThumbnail::TooLarge));
        assert_eq!(
            refusals_so_far(),
            refusals + 1,
            "refused for being hostile, not for being broken"
        );
    }

    /// **The important half**, and the one a careful-looking implementation
    /// still gets wrong.
    ///
    /// 20 000 × 20 000 is *inside* the 30 000-per-edge limit, so every
    /// dimension check passes — and it asks for 1.6 GB of RGBA. Only
    /// `limits.reserve(decoder.total_bytes())` refuses it, and only because it
    /// charges the output buffer against the budget from the header, before a
    /// byte of pixel data is read.
    ///
    /// Delete that `reserve` and this goes red while
    /// `a_picture_wider_than_any_screen_is_refused` stays green — which is
    /// exactly how a bomb ships past a test suite that looks like it covers
    /// this.
    #[test]
    fn a_picture_that_fits_the_limits_but_not_the_memory_is_refused() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "bomb.tiff",
            &super::super::testing::image::tiff_claiming(20_000, 20_000),
        );

        let refusals = refusals_so_far();
        assert_eq!(thumbnail(&path, 64), Err(NoThumbnail::TooLarge));
        assert_eq!(refusals_so_far(), refusals + 1);
    }

    /// **P77.** A photograph taken on a phone is stored landscape with a tag
    /// saying which way up it goes. Ignore the tag and every portrait photo in
    /// the grid is on its side — the first thing anybody notices.
    ///
    /// Asserted through the output *dimensions*, which is the one pixel-level
    /// property that is visible to a headless test: orientation 6 is a quarter
    /// turn, so a 40 × 20 source comes back taller than it is wide.
    #[test]
    fn a_photograph_marked_rotated_comes_back_the_right_way_up() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();

        let upright = write(
            &dir,
            "upright.jpg",
            &super::super::testing::image::jpeg_rotated(40, 20, 1),
        );
        let thumb = thumbnail(&upright, 64).expect("a readable jpeg");
        assert!(
            thumb.width > thumb.height,
            "landscape stays landscape: {thumb:?}"
        );

        let turned = write(
            &dir,
            "turned.jpg",
            &super::super::testing::image::jpeg_rotated(40, 20, 6),
        );
        let thumb = thumbnail(&turned, 64).expect("a readable jpeg");
        assert!(
            thumb.height > thumb.width,
            "a quarter turn swaps them: {thumb:?}"
        );
    }

    /// `image`'s JPEG decoder holds the whole file in memory before the
    /// limits above are even set, so a JPEG's own length is charged against
    /// the same budget as the pixels. A 300 MB "photograph" — or a video named
    /// `.jpg` — is refused from its length, not read.
    #[test]
    fn a_jpeg_larger_than_the_decode_budget_is_refused_unread() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "huge.jpg",
            &super::super::testing::image::jpeg_rotated(40, 20, 1),
        );
        // Sparse: the length is real and the disk is not spent.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_ALLOC + 1)
            .unwrap();

        let refusals = refusals_so_far();
        assert_eq!(thumbnail(&path, 64), Err(NoThumbnail::TooLarge));
        assert_eq!(refusals_so_far(), refusals + 1);
    }

    /// A refusal and a corruption are different answers, and the counters only
    /// mean something if the code can tell them apart.
    #[test]
    fn a_corrupt_file_is_unreadable_rather_than_refused() {
        let _counting = counting();
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "junk.tiff",
            &[
                0x49, 0x49, 0x2a, 0x00, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        );

        let refusals = refusals_so_far();
        assert_eq!(thumbnail(&path, 64), Err(NoThumbnail::Unreadable));
        assert_eq!(refusals_so_far(), refusals, "nothing was declined");
    }
}
