//! WavPack carrying an APEv2 tag.
//!
//! The fourth tag type, and the one with the quietest failure mode: the
//! generic-`Tag`→APE converter walks `tag.items()` and never looks at
//! `tag.pictures()`, so **embedded art handed to an APE format is dropped on
//! the floor** — no error, `Ok(())` returned, and reading back through the same
//! generic API reports zero pictures, which is consistent with what is on disk
//! and therefore looks correct. Only a byte scan catches it.
//!
//! WavPack rather than Musepack because its block header is 32 flat bytes with
//! no packet framing, and because `wv` is in
//! [`crate::meta::audio::AUDIO_EXTENSIONS`] while `ape` and `mpc`'s SV8
//! container are respectively absent and fiddly.

/// An APEv2 header or footer. The two are the same 32 bytes; only a flag bit
/// says which.
///
/// `size` counts the items **and this footer**, but never the optional header
/// — the one asymmetry in the format, and the reason a reader that gets it
/// wrong lands mid-item rather than failing cleanly.
fn ape_footer(size: u32, items: u32, is_header: bool) -> Vec<u8> {
    let mut out = Vec::from(*b"APETAGEX");
    out.extend_from_slice(&2000u32.to_le_bytes()); // version 2.000
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&items.to_le_bytes());
    // Bit 31 marks a header; bit 29 says a header is present.
    let flags: u32 = if is_header { 0xA000_0000 } else { 0x8000_0000 };
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&[0u8; 8]); // reserved, must be zero
    debug_assert_eq!(out.len(), 32);
    out
}

/// A complete APEv2 tag: header, items, footer.
pub fn ape_tag(fields: &[(&str, &str)]) -> Vec<u8> {
    let items: Vec<(&str, &[u8], bool)> = fields
        .iter()
        .map(|(key, value)| (*key, value.as_bytes(), false))
        .collect();
    ape_tag_items(&items)
}

/// The same, with each item's value as bytes and a flag for **binary** —
/// bits 1–2 of an item's flags say what its value is, and `1` there is the
/// binary kind cover art is stored as.
fn ape_tag_items(fields: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut items = Vec::new();
    for (key, value, binary) in fields {
        items.extend_from_slice(&(value.len() as u32).to_le_bytes());
        let flags: u32 = if *binary { 1 << 1 } else { 0 }; // 0 is UTF-8 text
        items.extend_from_slice(&flags.to_le_bytes());
        items.extend_from_slice(key.as_bytes());
        items.push(0); // keys are null-terminated
        items.extend_from_slice(value);
    }
    let size = (items.len() + 32) as u32;
    let count = fields.len() as u32;

    let mut out = ape_footer(size, count, true);
    out.extend_from_slice(&items);
    out.extend_from_slice(&ape_footer(size, count, false));
    out
}

/// How to build one WavPack file.
#[derive(Debug, Default, Clone)]
pub struct WavPack {
    pub fields: Vec<(&'static str, String)>,
    /// Binary items, after the text ones — `Cover Art (Front)` is the one a
    /// real file carries: a file name, a NUL, then the image bytes.
    pub binary: Vec<(&'static str, Vec<u8>)>,
}

impl WavPack {
    pub fn tagged(artist: &str, title: &str) -> Self {
        Self {
            fields: vec![("Artist", artist.to_owned()), ("Title", title.to_owned())],
            binary: Vec::new(),
        }
    }

    pub fn field(mut self, key: &'static str, value: &str) -> Self {
        self.fields.push((key, value.to_owned()));
        self
    }

    pub fn binary(mut self, key: &'static str, value: &[u8]) -> Self {
        self.binary.push((key, value.to_vec()));
        self
    }

    /// A single WavPack block header: `wvpk`, then the sizes and flags lofty
    /// reads for its properties.
    fn block(&self) -> Vec<u8> {
        let mut out = Vec::from(*b"wvpk");
        out.extend_from_slice(&24u32.to_le_bytes()); // block size, header-only
        out.extend_from_slice(&0x0410u16.to_le_bytes()); // version, in range
        out.push(0); // track number
        out.push(0); // index number
        out.extend_from_slice(&44_100u32.to_le_bytes()); // total samples
        out.extend_from_slice(&0u32.to_le_bytes()); // block index
        out.extend_from_slice(&44_100u32.to_le_bytes()); // block samples
        // Flags: 16-bit stereo at 44.1 kHz, final block.
        out.extend_from_slice(&0x0080_0C01u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // crc, unverified
        out
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut out = self.block();
        if !self.fields.is_empty() || !self.binary.is_empty() {
            let items: Vec<(&str, &[u8], bool)> = self
                .fields
                .iter()
                .map(|(k, v)| (*k, v.as_bytes(), false))
                .chain(self.binary.iter().map(|(k, v)| (*k, v.as_slice(), true)))
                .collect();
            out.extend_from_slice(&ape_tag_items(&items));
        }
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

    #[test]
    fn lofty_identifies_it_and_reads_what_we_wrote() {
        use lofty::file::TaggedFileExt;
        use lofty::prelude::*;
        use lofty::tag::TagType;

        let dir = TempDir::new().unwrap();
        let path = WavPack::tagged("Metallica", "One")
            .field("Album", "And Justice For All")
            .write(dir.path(), "t.wv");

        let file = lofty::probe::Probe::open(&path)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .expect("a WavPack lofty can parse");
        assert_eq!(file.file_type(), lofty::file::FileType::WavPack);

        let tag = file.tag(TagType::Ape).expect("the APEv2 tag we wrote");
        assert_eq!(tag.artist().as_deref(), Some("Metallica"));
        assert_eq!(tag.title().as_deref(), Some("One"));
        assert_eq!(tag.album().as_deref(), Some("And Justice For All"));
    }

    #[test]
    fn our_reader_sees_the_same_tags() {
        let dir = TempDir::new().unwrap();
        let path = WavPack::tagged("Metallica", "One").write(dir.path(), "t.wv");
        crate::meta::audio::forget_all();
        let tags = crate::meta::audio::tags_of(&path).expect("readable audio");
        assert_eq!(tags.artist.as_deref(), Some("Metallica"));
        assert_eq!(tags.title.as_deref(), Some("One"));
    }

    /// The size field counts the items and the footer but not the header. Get
    /// it wrong and a reader lands mid-item, so it is worth an arithmetic
    /// check that does not depend on lofty agreeing with us.
    #[test]
    fn the_footer_size_counts_the_items_and_itself() {
        let tag = ape_tag(&[("Artist", "A")]);
        let size = u32::from_le_bytes(tag[12..16].try_into().unwrap()) as usize;
        assert_eq!(size, tag.len() - 32, "the header is excluded");
        assert!(tag.starts_with(b"APETAGEX"));
        assert!(tag.ends_with(&[0u8; 8]));
    }
}
