//! FLAC: `fLaC`, a STREAMINFO block, and a Vorbis comment block.
//!
//! The cheapest of the non-MP3 containers to build honestly, and the reason it
//! is first: FLAC carries **Vorbis comments with no page framing**. Ogg wraps
//! the identical tag block in CRC-checked pages; FLAC just length-prefixes it.
//! So this file proves our reader and writer against `TagType::VorbisComments`
//! without any container arithmetic in the way.
//!
//! A metadata block is `[1 byte: last-block flag | type][3 bytes: length BE]`
//! followed by the payload. No checksums anywhere.

/// A Vorbis comment payload: vendor string, then `count` `KEY=value` entries,
/// every length **little**-endian — the one place FLAC is not big-endian, and
/// exactly the sort of detail a hand-built fixture exists to pin.
pub fn vorbis_comment(vendor: &str, fields: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    out.extend_from_slice(vendor.as_bytes());
    out.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for (key, value) in fields {
        let entry = format!("{key}={value}");
        out.extend_from_slice(&(entry.len() as u32).to_le_bytes());
        out.extend_from_slice(entry.as_bytes());
    }
    out
}

/// The 34-byte STREAMINFO payload. 44.1 kHz, stereo, 16-bit, one second.
fn stream_info() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&4096u16.to_be_bytes()); // min block size
    out.extend_from_slice(&4096u16.to_be_bytes()); // max block size
    out.extend_from_slice(&[0, 0, 0]); // min frame size, unknown
    out.extend_from_slice(&[0, 0, 0]); // max frame size, unknown
    // 20 bits sample rate | 3 bits channels-1 | 5 bits bits-per-sample-1 |
    // 36 bits total samples, packed into 64.
    let packed: u64 = (44_100u64 << 44) | (1u64 << 41) | (15u64 << 36) | 44_100;
    out.extend_from_slice(&packed.to_be_bytes());
    out.extend_from_slice(&[0u8; 16]); // MD5 of the unencoded audio, unknown
    debug_assert_eq!(out.len(), 34);
    out
}

fn block(last: bool, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![if last { 0x80 | kind } else { kind }];
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
    out.extend_from_slice(payload);
    out
}

/// How to build one FLAC.
#[derive(Debug, Default, Clone)]
pub struct Flac {
    /// `(KEY, value)` pairs for the Vorbis comment block. Absent when empty.
    pub fields: Vec<(&'static str, String)>,
    /// Bytes of "audio" after the metadata. Not real FLAC frames — nothing in
    /// the engine decodes audio, and the tests that matter compare this region
    /// byte for byte to prove a tag write never reached it.
    pub audio: Vec<u8>,
    /// A front cover, as a `PICTURE` block of its own. FLAC keeps pictures
    /// beside the comment block rather than inside it, which is exactly what
    /// a comments-only save forgets.
    pub cover: Option<Vec<u8>>,
}

/// A `PICTURE` block's payload: type 3 (front cover), a MIME type, no
/// description, zero dimensions — "unknown", which the format allows — and
/// the bytes.
fn picture(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&3u32.to_be_bytes());
    let mime = b"image/jpeg";
    out.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    out.extend_from_slice(mime);
    out.extend_from_slice(&0u32.to_be_bytes()); // description length
    for _ in 0..4 {
        out.extend_from_slice(&0u32.to_be_bytes()); // width, height, depth, colours
    }
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    out
}

impl Flac {
    /// The ordinary case: artist and title, and a run of audio to protect.
    pub fn tagged(artist: &str, title: &str) -> Self {
        Self {
            fields: vec![("ARTIST", artist.to_owned()), ("TITLE", title.to_owned())],
            audio: vec![0xAA; 512],
            cover: None,
        }
    }

    pub fn cover(mut self, data: &[u8]) -> Self {
        self.cover = Some(data.to_vec());
        self
    }

    pub fn field(mut self, key: &'static str, value: &str) -> Self {
        self.fields.push((key, value.to_owned()));
        self
    }

    /// Where the audio starts, so a test can compare that region across a write.
    pub fn audio_offset(&self) -> usize {
        self.bytes().len() - self.audio.len()
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::from(*b"fLaC");
        let has_tags = !self.fields.is_empty();
        let has_cover = self.cover.is_some();
        out.extend_from_slice(&block(!has_tags && !has_cover, 0, &stream_info()));
        if let Some(cover) = &self.cover {
            out.extend_from_slice(&block(!has_tags, 6, &picture(cover)));
        }
        if has_tags {
            let fields: Vec<(&str, &str)> =
                self.fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
            out.extend_from_slice(&block(true, 4, &vorbis_comment("renameit", &fields)));
        }
        out.extend_from_slice(&self.audio);
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

    /// The fixture has to be a FLAC to lofty before it can prove anything about
    /// our reader.
    #[test]
    fn lofty_identifies_it_and_reads_what_we_wrote() {
        use lofty::file::TaggedFileExt;
        use lofty::prelude::*;
        use lofty::tag::TagType;

        let dir = TempDir::new().unwrap();
        let path = Flac::tagged("Metallica", "One")
            .field("ALBUM", "And Justice For All")
            .write(dir.path(), "t.flac");

        let file = lofty::probe::Probe::open(&path)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .expect("a FLAC lofty can parse");
        assert_eq!(file.file_type(), lofty::file::FileType::Flac);

        let tag = file
            .tag(TagType::VorbisComments)
            .expect("the comment block we wrote");
        assert_eq!(tag.artist().as_deref(), Some("Metallica"));
        assert_eq!(tag.title().as_deref(), Some("One"));
        assert_eq!(tag.album().as_deref(), Some("And Justice For All"));
    }

    /// And our own reader agrees, which is the thing the corpus exists for.
    #[test]
    fn our_reader_sees_the_same_tags() {
        let dir = TempDir::new().unwrap();
        let path = Flac::tagged("Metallica", "One").write(dir.path(), "t.flac");
        crate::meta::audio::forget_all();
        let tags = crate::meta::audio::tags_of(&path).expect("readable audio");
        assert_eq!(tags.artist.as_deref(), Some("Metallica"));
        assert_eq!(tags.title.as_deref(), Some("One"));
    }

    /// A FLAC with no comment block at all — the "file with no tags" case the
    /// MP3 corpus already covers, so the two formats can be compared.
    #[test]
    fn a_flac_with_no_comment_block_is_still_audio() {
        let dir = TempDir::new().unwrap();
        let path = Flac {
            audio: vec![0xAA; 512],
            ..Default::default()
        }
        .write(dir.path(), "bare.flac");
        crate::meta::audio::forget_all();
        let tags = crate::meta::audio::tags_of(&path).expect("still audio (P49)");
        assert_eq!(tags.artist, None);
    }

    /// `audio_offset` has to be right, or the byte-compare tests are comparing
    /// the wrong region and would pass no matter what a write did.
    #[test]
    fn the_audio_region_is_where_it_says_it_is() {
        let flac = Flac::tagged("A", "B");
        let bytes = flac.bytes();
        assert_eq!(&bytes[flac.audio_offset()..], &flac.audio[..]);
        assert!(bytes.starts_with(b"fLaC"));
    }
}
