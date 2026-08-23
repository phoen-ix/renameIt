//! Ogg Vorbis: the same Vorbis comment block as FLAC, wrapped in Ogg pages.
//!
//! This is the page-framed path, and it exists in the corpus because the
//! container changes the *behaviour*, not just the bytes. lofty's Ogg reader
//! yields its comment tag as `unwrap_or_default()`, so an Ogg file always
//! reports a tag whether or not one was written — which is why "remove the
//! ID3v2 from this .ogg" is a request that must never reach lofty, and why
//! `meta::write`'s support guard is the thing standing between a user and a
//! wiped comment header.
//!
//! Two constraints, both established by reading lofty rather than guessing,
//! and both invisible until you get them wrong:
//!
//! * **The CRC is never checked on read.** `ogg_pager` parses the checksum
//!   into a field and never compares it, so a fixture needs no CRC arithmetic
//!   at all. (Ogg's polynomial is not the reflected one `crc32fast` implements,
//!   so reusing that crate would have been wrong as well as unnecessary.)
//! * **Detection reads only the first 36 bytes**, and looks for `b"vorbis"` at
//!   `[29..35]`. Byte 26 is the lacing-table length and byte 27 is its first
//!   entry, so the identification page must have **exactly one segment** — the
//!   packet must be under 255 bytes and occupy a single lacing entry. Get that
//!   wrong and the file is not recognised as audio at all, which looks exactly
//!   like a file with no tags.

use super::flac::vorbis_comment;

/// One Ogg page. `flags`: bit 1 is "beginning of stream", bit 2 "end".
///
/// The lacing table is the packet length in 255-byte runs; a packet whose
/// length is an exact multiple of 255 needs a trailing zero entry, which is
/// why the loop always pushes a final remainder.
fn page(serial: u32, sequence: u32, flags: u8, granule: u64, packet: &[u8]) -> Vec<u8> {
    let mut lacing = Vec::new();
    let mut left = packet.len();
    while left >= 255 {
        lacing.push(255u8);
        left -= 255;
    }
    lacing.push(left as u8);

    let mut out = Vec::from(*b"OggS");
    out.push(0); // stream structure version
    out.push(flags);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // checksum — never verified
    out.push(lacing.len() as u8);
    out.extend_from_slice(&lacing);
    out.extend_from_slice(packet);
    out
}

/// The Vorbis identification packet: `01 "vorbis"` then the stream parameters.
fn identification() -> Vec<u8> {
    let mut out = vec![0x01];
    out.extend_from_slice(b"vorbis");
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.push(2); // channels
    out.extend_from_slice(&44_100u32.to_le_bytes()); // sample rate
    out.extend_from_slice(&0u32.to_le_bytes()); // bitrate maximum
    out.extend_from_slice(&128_000u32.to_le_bytes()); // bitrate nominal
    out.extend_from_slice(&0u32.to_le_bytes()); // bitrate minimum
    out.push(0xB8); // block sizes
    out.push(0x01); // framing flag
    out
}

/// How to build one Ogg Vorbis file.
#[derive(Debug, Default, Clone)]
pub struct OggVorbis {
    pub fields: Vec<(&'static str, String)>,
}

impl OggVorbis {
    pub fn tagged(artist: &str, title: &str) -> Self {
        Self {
            fields: vec![("ARTIST", artist.to_owned()), ("TITLE", title.to_owned())],
        }
    }

    pub fn field(mut self, key: &'static str, value: &str) -> Self {
        self.fields.push((key, value.to_owned()));
        self
    }

    pub fn bytes(&self) -> Vec<u8> {
        const SERIAL: u32 = 0x1234_5678;

        let fields: Vec<(&str, &str)> = self.fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let mut comment = vec![0x03];
        comment.extend_from_slice(b"vorbis");
        comment.extend_from_slice(&vorbis_comment("renameit", &fields));
        comment.push(0x01); // framing flag

        // The setup packet is counted and never parsed — lofty reads three
        // packets and only interprets the first two — so a stub is honest
        // rather than lazy.
        let mut setup = vec![0x05];
        setup.extend_from_slice(b"vorbis");
        setup.extend_from_slice(&[0u8; 16]);

        let mut out = page(SERIAL, 0, 0x02, 0, &identification());
        out.extend_from_slice(&page(SERIAL, 1, 0, 0, &comment));
        out.extend_from_slice(&page(SERIAL, 2, 0x04, 44_100, &setup));
        out
    }

    pub fn write(&self, dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, self.bytes()).expect("write fixture");
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// The detection rule, pinned where it can be read. lofty looks for
    /// `b"vorbis"` at bytes 29..35 of the file, which only lines up when the
    /// identification page carries exactly one lacing segment.
    #[test]
    fn the_identification_page_is_shaped_the_way_detection_expects() {
        let bytes = OggVorbis::tagged("A", "B").bytes();
        assert_eq!(&bytes[..4], b"OggS");
        assert_eq!(bytes[26], 1, "exactly one lacing segment");
        assert_eq!(
            &bytes[29..35],
            b"vorbis",
            "this is the window lofty's quick type guess reads"
        );
    }

    #[test]
    fn lofty_identifies_it_and_reads_what_we_wrote() {
        use lofty::file::TaggedFileExt;
        use lofty::prelude::*;
        use lofty::tag::TagType;

        let dir = TempDir::new().unwrap();
        let path = OggVorbis::tagged("Metallica", "One")
            .field("ALBUM", "And Justice For All")
            .write(dir.path(), "t.ogg");

        let file = lofty::probe::Probe::open(&path)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .expect("an Ogg Vorbis lofty can parse");
        assert_eq!(file.file_type(), lofty::file::FileType::Vorbis);

        let tag = file
            .tag(TagType::VorbisComments)
            .expect("the comment packet");
        assert_eq!(tag.artist().as_deref(), Some("Metallica"));
        assert_eq!(tag.title().as_deref(), Some("One"));
        assert_eq!(tag.album().as_deref(), Some("And Justice For All"));
    }

    #[test]
    fn our_reader_sees_the_same_tags() {
        let dir = TempDir::new().unwrap();
        let path = OggVorbis::tagged("Metallica", "One").write(dir.path(), "t.ogg");
        crate::meta::audio::forget_all();
        let tags = crate::meta::audio::tags_of(&path).expect("readable audio");
        assert_eq!(tags.artist.as_deref(), Some("Metallica"));
        assert_eq!(tags.title.as_deref(), Some("One"));
        // Never MPEG, whatever the properties say (D58).
        assert_eq!(tags.properties.mpeg, None);
    }
}
