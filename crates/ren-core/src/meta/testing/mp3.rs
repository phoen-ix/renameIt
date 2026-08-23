//! MP3: MPEG frames, ID3v2.4 and the ID3v1 trailer.

/// MPEG-1 Layer III frames: 128 kbps, 44 100 Hz, stereo, all silence.
///
/// `FF FB` is the sync word plus MPEG-1 Layer III with no CRC; `90` is the
/// bitrate and sample-rate nibbles; `00` is stereo with no padding. The frame
/// length is `144 * bitrate / rate` = 417 bytes.
///
/// **Several frames, not one.** A single frame is a coincidence — two bytes of
/// `FF FB` turn up in ordinary data often enough that no parser will trust
/// them alone, and lofty reports no properties at all for a one-frame file. A
/// run of them is what makes it audio.
pub const FRAMES: usize = 8;

pub fn mpeg_frames() -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..FRAMES {
        let mut frame = vec![0u8; 417];
        frame[0] = 0xFF;
        frame[1] = 0xFB;
        frame[2] = 0x90;
        frame[3] = 0x00;
        out.extend_from_slice(&frame);
    }
    out
}

/// ID3v2.4 sizes are "syncsafe": seven bits per byte, so the high bit can never
/// collide with an MPEG sync word.
fn syncsafe(value: u32) -> [u8; 4] {
    [
        ((value >> 21) & 0x7F) as u8,
        ((value >> 14) & 0x7F) as u8,
        ((value >> 7) & 0x7F) as u8,
        (value & 0x7F) as u8,
    ]
}

/// One ID3v2.4 frame: id, size, flags, then the body.
///
/// `COMM` is not a plain text frame — it carries a three-byte language and a
/// null-terminated short description before its text — and neither is `USLT`.
/// Getting that wrong produces a frame a parser skips, which looks exactly like
/// a tag that was never written.
fn text_frame(id: &str, value: &str) -> Vec<u8> {
    let mut body = vec![0x03]; // UTF-8
    if matches!(id, "COMM" | "USLT") {
        body.extend_from_slice(b"eng");
        body.push(0x00); // empty short description
    }
    body.extend_from_slice(value.as_bytes());
    let mut out = Vec::from(id.as_bytes());
    out.extend_from_slice(&syncsafe(body.len() as u32));
    out.extend_from_slice(&[0x00, 0x00]); // no frame flags
    out.extend_from_slice(&body);
    out
}

/// An ID3v2.4 tag carrying `frames`, as `(frame id, value)` pairs.
///
/// Frame ids rather than friendly names on purpose: a test that says `"TPE1"`
/// is a test about the format, and it fails loudly if the mapping underneath
/// ever changes meaning.
pub fn id3v2(frames: &[(&str, &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (id, value) in frames {
        body.extend_from_slice(&text_frame(id, value));
    }
    let mut out = Vec::from(*b"ID3");
    out.extend_from_slice(&[0x04, 0x00]); // v2.4.0
    out.push(0x00); // no tag flags
    out.extend_from_slice(&syncsafe(body.len() as u32));
    out.extend_from_slice(&body);
    out
}

/// The 128-byte ID3v1 trailer: `TAG`, then fixed-width latin-1 fields.
///
/// Every field is truncated to its width rather than rejected, because that is
/// what the format does and what makes v1's 30-character limit visible in a
/// test instead of theoretical.
pub fn id3v1(title: &str, artist: &str, album: &str, year: &str, track: Option<u8>) -> Vec<u8> {
    fn field(out: &mut Vec<u8>, value: &str, width: usize) {
        let mut bytes: Vec<u8> = value.bytes().take(width).collect();
        bytes.resize(width, 0);
        out.extend_from_slice(&bytes);
    }
    let mut out = Vec::from(*b"TAG");
    field(&mut out, title, 30);
    field(&mut out, artist, 30);
    field(&mut out, album, 30);
    field(&mut out, year, 4);
    match track {
        // ID3v1.1 puts the track number in the last two bytes of the comment.
        Some(n) => {
            field(&mut out, "", 28);
            out.push(0x00);
            out.push(n);
        }
        None => field(&mut out, "", 30),
    }
    out.push(0xFF); // genre 255 = none
    out
}

/// How to build one MP3.
#[derive(Debug, Default, Clone)]
pub struct Mp3 {
    /// `(frame id, value)` pairs for the ID3v2.4 tag at the front.
    pub id3v2: Vec<(&'static str, String)>,
    /// `(title, artist, album, year, track)` for the ID3v1 trailer.
    pub id3v1: Option<(String, String, String, String, Option<u8>)>,
    /// Whether to include a real MPEG frame. Without one lofty can still read
    /// the tags but reports no properties — which is the two-tier split the
    /// fixture corpus needs.
    pub audio: bool,
}

impl Mp3 {
    /// The ordinary case: a v2 tag, real audio, no v1 trailer.
    pub fn tagged(artist: &str, title: &str) -> Self {
        Self {
            id3v2: vec![("TPE1", artist.to_owned()), ("TIT2", title.to_owned())],
            audio: true,
            ..Default::default()
        }
    }

    pub fn frame(mut self, id: &'static str, value: &str) -> Self {
        self.id3v2.push((id, value.to_owned()));
        self
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.id3v2.is_empty() {
            let frames: Vec<(&str, &str)> = self
                .id3v2
                .iter()
                .map(|(id, value)| (*id, value.as_str()))
                .collect();
            out.extend_from_slice(&id3v2(&frames));
        }
        if self.audio {
            out.extend_from_slice(&mpeg_frames());
        }
        if let Some((title, artist, album, year, track)) = &self.id3v1 {
            out.extend_from_slice(&id3v1(title, artist, album, year, *track));
        }
        out
    }

    /// Writes it and returns the path.
    pub fn write(&self, dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, self.bytes()).expect("write fixture");
        path
    }
}
