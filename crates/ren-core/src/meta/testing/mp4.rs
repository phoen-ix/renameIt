//! MP4 / M4A: nested atoms, and an `ilst` tag block.
//!
//! The most demanding container in the corpus, and the one with the most to
//! prove. Three things established by reading lofty rather than guessing:
//!
//! * The **first** atom must be `ftyp` with a length of at least 12, or the
//!   file is not recognised at all.
//! * `moov` must exist — its absence is `Mp4ParseError::missing_moov`.
//! * **The write path is stricter than the read path.** `meta::write` reads
//!   with properties on and no fallback, and lofty's property reader refuses a
//!   file with no audio track: it needs some `trak.mdia` carrying an `hdlr`
//!   whose handler type is `soun`, *and* an `mdhd` beside it. A fixture without
//!   those reads perfectly through `tags_of` and then fails every write, which
//!   is a confusing half-hour if you have not been told.
//!
//! Atoms are `[u32 BE size][4-byte type][payload]`, nestable, no checksums
//! anywhere — so the arithmetic is all in the sizes.

/// One atom. The size counts the eight-byte header as well as the payload.
fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// An `ilst` entry: an atom named for the field, holding a `data` atom.
///
/// The `data` atom's flags say what the bytes are; `1` is UTF-8 text. The
/// four zero bytes after it are the locale, which nothing reads.
fn ilst_text(kind: &[u8; 4], value: &str) -> Vec<u8> {
    let mut data = 1u32.to_be_bytes().to_vec(); // version 0, flags 1 = UTF-8
    data.extend_from_slice(&0u32.to_be_bytes()); // locale
    data.extend_from_slice(value.as_bytes());
    atom(kind, &atom(b"data", &data))
}

/// The media header: timescale and duration, which is what gives a duration.
fn mdhd() -> Vec<u8> {
    let mut out = vec![0u8; 4]; // version 0, no flags
    out.extend_from_slice(&0u32.to_be_bytes()); // creation time
    out.extend_from_slice(&0u32.to_be_bytes()); // modification time
    out.extend_from_slice(&44_100u32.to_be_bytes()); // timescale
    out.extend_from_slice(&44_100u32.to_be_bytes()); // duration — one second
    out.extend_from_slice(&0x55C4u16.to_be_bytes()); // language, "und"
    out.extend_from_slice(&0u16.to_be_bytes()); // pre-defined
    atom(b"mdhd", &out)
}

/// The handler reference. lofty skips eight bytes past the atom header and
/// reads four: this is the only place `soun` may sit.
fn hdlr_soun() -> Vec<u8> {
    let mut out = vec![0u8; 4]; // version 0, no flags
    out.extend_from_slice(&0u32.to_be_bytes()); // pre-defined
    out.extend_from_slice(b"soun"); // handler type
    out.extend_from_slice(&[0u8; 12]); // reserved
    out.push(0); // empty name
    atom(b"hdlr", &out)
}

/// How to build one M4A.
///
/// Named `.m4a` rather than `.mp4` on purpose when written: `mp4` is **not** in
/// [`crate::meta::audio::AUDIO_EXTENSIONS`], and both mutation operations gate
/// on that list by name before opening anything — so a `.mp4` fixture would be
/// invisible to the tagger and the untagger while reading perfectly.
#[derive(Debug, Default, Clone)]
pub struct M4a {
    /// `(atom fourcc, value)` — `©ART`, `©nam`, `©alb` and friends.
    pub fields: Vec<([u8; 4], String)>,
    /// Bytes of `mdat`. Not real AAC; nothing here decodes audio.
    pub audio: Vec<u8>,
}

impl M4a {
    pub fn tagged(artist: &str, title: &str) -> Self {
        Self {
            fields: vec![
                (*b"\xa9ART", artist.to_owned()),
                (*b"\xa9nam", title.to_owned()),
            ],
            audio: vec![0xCC; 512],
        }
    }

    pub fn field(mut self, kind: &[u8; 4], value: &str) -> Self {
        self.fields.push((*kind, value.to_owned()));
        self
    }

    pub fn bytes(&self) -> Vec<u8> {
        // ftyp first, always — lofty refuses anything else in that position.
        let mut ftyp = Vec::from(*b"M4A ");
        ftyp.extend_from_slice(&0u32.to_be_bytes()); // minor version
        ftyp.extend_from_slice(b"M4A mp42isom"); // compatible brands
        let mut out = atom(b"ftyp", &ftyp);

        // moov.trak.mdia.{mdhd,hdlr} — what the *write* path insists on.
        let mut mdia = mdhd();
        mdia.extend_from_slice(&hdlr_soun());
        let trak = atom(b"trak", &atom(b"mdia", &mdia));

        let mut moov = trak;
        if !self.fields.is_empty() {
            let mut ilst = Vec::new();
            for (kind, value) in &self.fields {
                ilst.extend_from_slice(&ilst_text(kind, value));
            }
            // `meta` is a full atom: four bytes of version and flags first.
            let mut meta = vec![0u8; 4];
            meta.extend_from_slice(&atom(b"ilst", &ilst));
            moov.extend_from_slice(&atom(b"udta", &atom(b"meta", &meta)));
        }
        out.extend_from_slice(&atom(b"moov", &moov));
        out.extend_from_slice(&atom(b"mdat", &self.audio));
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
        let path = M4a::tagged("Metallica", "One")
            .field(b"\xa9alb", "And Justice For All")
            .write(dir.path(), "t.m4a");

        let file = lofty::probe::Probe::open(&path)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .expect("an MP4 lofty can parse");
        assert_eq!(file.file_type(), lofty::file::FileType::Mp4);

        let tag = file.tag(TagType::Mp4Ilst).expect("the ilst we wrote");
        assert_eq!(tag.artist().as_deref(), Some("Metallica"));
        assert_eq!(tag.title().as_deref(), Some("One"));
        assert_eq!(tag.album().as_deref(), Some("And Justice For All"));
    }

    #[test]
    fn our_reader_sees_the_same_tags() {
        let dir = TempDir::new().unwrap();
        let path = M4a::tagged("Metallica", "One").write(dir.path(), "t.m4a");
        crate::meta::audio::forget_all();
        let tags = crate::meta::audio::tags_of(&path).expect("readable audio");
        assert_eq!(tags.artist.as_deref(), Some("Metallica"));
        assert_eq!(tags.title.as_deref(), Some("One"));
    }

    /// The trap this fixture exists to avoid, pinned so nobody removes the
    /// `hdlr`/`mdhd` pair as redundant: without an audio track the file still
    /// *reads*, because `tags_of` falls back to a tags-only parse — and every
    /// *write* to it fails, because `meta::write` has no such fallback.
    #[test]
    fn a_file_with_no_audio_track_reads_but_cannot_be_written() {
        use crate::meta::write::{FieldWrite, MusicField, write_fields};

        let dir = TempDir::new().unwrap();
        let mut bytes = Vec::new();
        let mut ftyp = Vec::from(*b"M4A ");
        ftyp.extend_from_slice(&0u32.to_be_bytes());
        ftyp.extend_from_slice(b"M4A mp42isom");
        bytes.extend_from_slice(&atom(b"ftyp", &ftyp));
        // A `moov` with no `trak` at all.
        bytes.extend_from_slice(&atom(b"moov", &[]));
        let path = dir.path().join("trackless.m4a");
        std::fs::write(&path, &bytes).unwrap();

        crate::meta::audio::forget_all();
        assert!(
            crate::meta::audio::tags_of(&path).is_some(),
            "the read path falls back and still answers"
        );
        assert!(
            write_fields(
                &path,
                &[FieldWrite {
                    field: MusicField::Artist,
                    value: "X".into()
                }]
            )
            .is_err(),
            "the write path reads with properties on, and there is no audio track"
        );
    }

    /// `.mp4` is not in `AUDIO_EXTENSIONS`, so the operations skip it by name
    /// before opening anything. Pinned because the fixture reads perfectly and
    /// the omission is otherwise invisible.
    #[test]
    fn the_mp4_extension_itself_is_not_one_the_operations_look_at() {
        assert!(crate::meta::audio::AUDIO_EXTENSIONS.contains(&"m4a"));
        assert!(!crate::meta::audio::AUDIO_EXTENSIONS.contains(&"mp4"));
    }
}
