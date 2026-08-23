//! A minimal JPEG carrying whichever Exif date tags a test asks for.

// A minimal JPEG carrying whichever date tags a test asks for.
//
// Built byte by byte rather than committed as a fixture. CI has no network,
// and — decisively — a synthesised blob can produce the cases one
// photograph cannot:
// original-only, digitized-only, DateTime-only, all three disagreeing, and
// an APP1 segment carrying no date at all.

/// Tag ids, little-endian TIFF.
const DATE_TIME: u16 = 0x0132;
const EXIF_IFD_POINTER: u16 = 0x8769;
const DATE_TIME_ORIGINAL: u16 = 0x9003;
const DATE_TIME_DIGITIZED: u16 = 0x9004;
const ASCII: u16 = 2;

fn entry(out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32) {
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&value.to_le_bytes());
}

/// `original`, `digitized` and `modified` are `"YYYY:MM:DD HH:MM:SS"`.
pub fn jpeg_with_exif(
    original: Option<&str>,
    digitized: Option<&str>,
    modified: Option<&str>,
) -> Vec<u8> {
    // TIFF block: header, IFD0, the Exif sub-IFD, then the string pool.
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II\x2a\x00"); // little-endian, magic 42
    tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 at offset 8

    // Lay the strings out first so the offsets are known.
    let ifd0_count = 1 + u16::from(modified.is_some());
    let ifd0_len = 2 + 12 * u32::from(ifd0_count) + 4;
    let sub_count = u16::from(original.is_some()) + u16::from(digitized.is_some());
    let sub_len = 2 + 12 * u32::from(sub_count) + 4;
    let mut pool_at = 8 + ifd0_len + sub_len;

    let mut pool = Vec::new();
    let mut place = |text: Option<&str>| -> Option<u32> {
        let text = text?;
        let at = pool_at;
        pool.extend_from_slice(text.as_bytes());
        pool.push(0);
        pool_at += text.len() as u32 + 1;
        Some(at)
    };
    let modified_at = place(modified);
    let original_at = place(original);
    let digitized_at = place(digitized);

    // IFD0: the sub-IFD pointer, plus DateTime when asked for. Tags must be
    // in ascending order, and 0x0132 sorts before 0x8769.
    tiff.extend_from_slice(&ifd0_count.to_le_bytes());
    if let (Some(text), Some(at)) = (modified, modified_at) {
        entry(&mut tiff, DATE_TIME, ASCII, text.len() as u32 + 1, at);
    }
    entry(&mut tiff, EXIF_IFD_POINTER, 4, 1, 8 + ifd0_len);
    tiff.extend_from_slice(&0u32.to_le_bytes()); // no IFD1

    // The Exif sub-IFD: 0x9003 then 0x9004.
    tiff.extend_from_slice(&sub_count.to_le_bytes());
    if let (Some(text), Some(at)) = (original, original_at) {
        entry(
            &mut tiff,
            DATE_TIME_ORIGINAL,
            ASCII,
            text.len() as u32 + 1,
            at,
        );
    }
    if let (Some(text), Some(at)) = (digitized, digitized_at) {
        entry(
            &mut tiff,
            DATE_TIME_DIGITIZED,
            ASCII,
            text.len() as u32 + 1,
            at,
        );
    }
    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.extend_from_slice(&pool);

    // Wrap it: SOI, APP1 holding "Exif\0\0" + the TIFF block, EOI.
    let mut app1 = Vec::from(*b"Exif\0\0");
    app1.extend_from_slice(&tiff);
    let mut out = Vec::from(*b"\xFF\xD8");
    out.extend_from_slice(b"\xFF\xE1");
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&app1);
    out.extend_from_slice(b"\xFF\xD9");
    out
}

// --- TIFFs built by hand, for the two cases no encoder in the tree emits ----
//
// A **bilevel** TIFF, because 1-bit is the only depth among our formats where
// the file's own colour type and the one a decode produces disagree — which is
// what `<Depthb>` and the RGBA-length check both need. And a TIFF that
// *claims* to be enormous, because a decompression bomb is a header, not a
// file, and the whole point is that we refuse it before reading further.

/// The nine tags a minimal uncompressed TIFF needs, little-endian.
///
/// `strip_bytes` is what the header *claims*; the caller decides whether to
/// supply that many. That gap is the difference between the two builders here.
fn tiff(width: u32, height: u32, bits: u16, strip_bytes: u32, pixels: &[u8]) -> Vec<u8> {
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

    // Tags must be in ascending order; a SHORT value sits inline in the low
    // half of the 4-byte field, so a dimension past 65 535 has to be a LONG.
    let dimension = |v: u32| if v > u16::MAX as u32 { LONG } else { SHORT };
    for (tag, kind, value) in [
        (256u16, dimension(width), width), // ImageWidth
        (257, dimension(height), height),  // ImageLength
        (258, SHORT, bits as u32),         // BitsPerSample
        (259, SHORT, 1),                   // Compression: none
        (262, SHORT, 0),                   // Photometric: white is zero
        (273, LONG, DATA_AT),              // StripOffsets
        (277, SHORT, 1),                   // SamplesPerPixel
        (278, dimension(height), height),  // RowsPerStrip: one strip
        (279, LONG, strip_bytes),          // StripByteCounts
    ] {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes()); // count
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
    assert_eq!(out.len() as u32, DATA_AT, "the strip offset must be right");
    out.extend_from_slice(pixels);
    out
}

/// An 8×8 one-bit TIFF, uncompressed. Eight rows of one byte.
pub fn bilevel_tiff() -> Vec<u8> {
    tiff(8, 8, 1, 8, &[0b1010_1010; 8])
}

/// A **decompression bomb**: a few hundred bytes claiming to be `width` ×
/// `height` of 8-bit grey.
///
/// At 64 000 × 64 000 that is sixteen gigabytes of RGBA asked for by a file
/// that fits in a packet. Nothing supplies the pixels, deliberately — a reader
/// that gets far enough to notice they are missing has already lost.
pub fn tiff_claiming(width: u32, height: u32) -> Vec<u8> {
    let claimed = (width as u64 * height as u64).min(u32::MAX as u64) as u32;
    tiff(width, height, 8, claimed, &[0u8; 8])
}

/// A real, decodable JPEG carrying an Exif **Orientation** tag.
///
/// [`jpeg_with_exif`] cannot serve here: it is a header and nothing else — SOI,
/// APP1, EOI — which is everything the Exif *reader* needs and no image at all.
/// A decoder needs pixels, so this encodes a real one and splices the APP1 in
/// after SOI, which is where a camera puts it.
///
/// `orientation` is the Exif value: 1 is upright, 6 is rotated 90° clockwise,
/// 8 is 90° anticlockwise. 6 and 8 are the ones that swap width and height,
/// which is what makes the effect assertable without looking at pixels.
pub fn jpeg_rotated(width: u32, height: u32, orientation: u16) -> Vec<u8> {
    const ORIENTATION: u16 = 0x0112;
    const SHORT: u16 = 3;

    let mut raw = Vec::new();
    ::image::ImageBuffer::<::image::Rgb<u8>, _>::from_fn(width, height, |x, _| {
        // Not flat grey: a JPEG of one colour compresses to almost nothing and
        // makes a poor stand-in for a photograph.
        ::image::Rgb([(x * 7) as u8, 128, 200])
    })
    .write_to(
        &mut std::io::Cursor::new(&mut raw),
        ::image::ImageFormat::Jpeg,
    )
    .expect("encoding a jpeg");

    // One IFD0 entry, no sub-IFD, no string pool.
    let mut tiff = Vec::from(*b"II\x2a\x00");
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    entry(&mut tiff, ORIENTATION, SHORT, 1, u32::from(orientation));
    tiff.extend_from_slice(&0u32.to_le_bytes()); // no IFD1

    let mut app1 = Vec::from(*b"Exif\0\0");
    app1.extend_from_slice(&tiff);

    let mut out = raw[..2].to_vec(); // SOI
    out.extend_from_slice(b"\xFF\xE1");
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&app1);
    out.extend_from_slice(&raw[2..]);
    out
}
