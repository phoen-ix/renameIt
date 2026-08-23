//! Music files built byte by byte, for the tests that read them.
//!
//! The same bargain M5 struck for `jpeg_with_exif`: nothing binary is ever
//! committed, and a synthesised file can express what a real one cannot — an
//! ID3v1-only track, a v1 and a v2 tag that disagree, a track number written
//! `7/12`, a file that is audio and carries no tags at all.
//!
//! It also keeps the read tests honest. If the fixtures were written by lofty
//! they would prove that lofty can read what lofty wrote; written here, they
//! prove that our reader understands the format.

pub mod ape;
pub mod flac;
pub mod image;
pub mod mp3;
pub mod mp4;
pub mod ogg;

pub use ape::WavPack;
pub use flac::Flac;
pub use image::{bilevel_tiff, jpeg_rotated, jpeg_with_exif, tiff_claiming};
pub use mp3::{FRAMES, Mp3, id3v1, id3v2, mpeg_frames};
pub use mp4::M4a;
pub use ogg::OggVorbis;
