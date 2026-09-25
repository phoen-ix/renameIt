//! Finding and removing a Lyrics3 v1 or v2 block.
//!
//! Remove Tags' third checkbox. The block is found by its own markers —
//! `LYRICSBEGIN`, `LYRICSEND`, `LYRICS200` and the `IND0000200` indications
//! field every real v2 tag opens with.
//!
//! **Hand-rolled because lofty cannot see one.** Its `find_lyrics3v2`
//! (`src/id3/mod.rs:75`, identical in 0.25.0 and 0.25.1) reads a 15-byte footer
//! and then compares `&lyrics3v2[7..]` — eight bytes — against the nine-byte
//! `b"LYRICS200"`, and parses the size out of `[..7]` where the format
//! specifies six digits. The comparison is a length mismatch that can never be
//! true, so lofty never detects a Lyrics3 tag at all. Both indices are off by
//! one; the correct ones are `[6..]` and `[..6]`.
//!
//! The format:
//!
//! * **v1** is `LYRICSBEGIN`, the lyrics, `LYRICSEND`. No size field anywhere.
//!   The spec caps the *lyrics text* at 5100 bytes, so the whole block is at
//!   most `11 + 5100 + 9` = 5120 — and it is 5120, not 5100, that bounds the
//!   backward search, or a conforming maximum-size block is missed.
//! * **v2** is `LYRICSBEGIN`, a run of `id`+`00000`-length+data fields, then a
//!   six-digit ASCII size and `LYRICS200`. The size counts from `LYRICSBEGIN`
//!   up to but excluding the size digits, so the block starts `size + 15` bytes
//!   before its end.
//!
//! Either sits at the very end of the file, *before* an ID3v1 trailer if there
//! is one.
//!
//! Only the head and tail of the file are ever read, and a block is removed by
//! truncating and re-appending whatever followed it — so a 40 MB file costs the
//! same as a 40 KB one, and the audio stream is out of reach by construction.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The ID3v1 trailer is exactly 128 bytes and starts with `TAG`.
const ID3V1_LEN: u64 = 128;

const BEGIN: &[u8] = b"LYRICSBEGIN";
const END_V1: &[u8] = b"LYRICSEND";
const END_V2: &[u8] = b"LYRICS200";

/// *"The maximum length of the lyrics is 5100 bytes"* — and the lyrics are what
/// that caps, so the block itself runs to `BEGIN + 5100 + END_V1`.
const V1_TEXT_MAX: usize = 5100;
const V1_BLOCK_MAX: usize = BEGIN.len() + V1_TEXT_MAX + END_V1.len();

/// v2's footer: six ASCII digits plus `LYRICS200`.
const V2_FOOTER: usize = 6 + 9;

/// The largest block v2's six-digit size can describe, footer included.
const V2_BLOCK_MAX: u64 = 999_999 + V2_FOOTER as u64;

/// What is read from the end of the file first: enough for the largest v1
/// block, an ID3v1 trailer, and slack.
///
/// Not enough for v2, whose size field allows almost a megabyte — and
/// timestamped lyrics pass 6 KB easily. A v2 footer that points further back
/// than this is re-read at the size it gives (see `locate`), so the common
/// case costs 6 KB and the long one costs exactly its own length.
const TAIL: u64 = V1_BLOCK_MAX as u64 + 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// A footer is plainly there and the block's start could not be
    /// established.
    ///
    /// Distinct from "no block", and the distinction matters: the user ticked
    /// *"Lyrics v1 & v2"*, the file visibly ends in `LYRICS200`, and reporting
    /// `Ok(false)` would tell them the run succeeded having done nothing.
    #[error(
        "this file carries a Lyrics3 tag whose start could not be found — it looks damaged, so nothing was removed"
    )]
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    V1,
    V2,
}

/// Where a block sits, as offsets into the whole file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub start: u64,
    pub end: u64,
    pub version: Version,
}

impl Block {
    /// How many bytes it occupies. Never zero — a block is at least its two
    /// markers — so there is no `is_empty` to go with it.
    pub fn size(self) -> u64 {
        self.end - self.start
    }
}

/// The end of any ID3v2 tag at the front of the file.
///
/// **The floor the backward search must never cross.** Without it, a file whose
/// ID3v2 tag quotes `LYRICSBEGIN` — a `USLT` lyrics frame is the obvious way
/// that happens — and which happens to end in `LYRICSEND` would have everything
/// between the two deleted, audio included. The window is 5 KB, so this is only
/// reachable on a short file; it is also the only path in this module that
/// could ever destroy audio, which is reason enough for the fifteen lines.
fn id3v2_end(head: &[u8]) -> u64 {
    if head.len() < 10 || &head[..3] != b"ID3" {
        return 0;
    }
    let syncsafe = head[6..10]
        .iter()
        .fold(0u64, |acc, b| (acc << 7) | u64::from(b & 0x7F));
    // Bit 4 of the flags byte is the footer-present flag, which adds ten more.
    let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
    10 + syncsafe + footer
}

/// The last `BEGIN` in `window`, or `None`.
///
/// **Last, not first**, and the choice is load-bearing. A `LYRICSBEGIN`
/// occurring earlier than the true one can only come from outside the block —
/// audio bytes, or a frame in the tag ahead of it — and cutting from there
/// deletes real data. One occurring *later* can only be inside the block's own
/// text, which the v1 spec permits (it forbids only `LYRICSEND` there), and
/// cutting from there leaves a few junk bytes behind.
///
/// So the two failure modes are not symmetric: searching backwards can
/// under-strip, searching forwards can destroy audio. In the one operation the
/// app cannot undo, that is not a close call.
fn last_begin(window: &[u8]) -> Option<usize> {
    window.windows(BEGIN.len()).rposition(|w| w == BEGIN)
}

/// Finds the block ending at `end`.
///
/// `tail` holds the file's last bytes and `base` is the offset `tail[0]`
/// corresponds to; `floor` is the absolute offset the search must not cross.
fn find_in(tail: &[u8], base: u64, end: u64, floor: u64) -> Result<Option<Block>, Error> {
    let Some(rel_end) = end.checked_sub(base).and_then(|n| usize::try_from(n).ok()) else {
        return Ok(None);
    };
    let Some(region) = tail.get(..rel_end) else {
        return Ok(None);
    };
    let rel_floor = usize::try_from(floor.saturating_sub(base))
        .unwrap_or(0)
        .min(region.len());

    // The bounded backward search both versions fall back on.
    let search = |cap: usize| -> Option<usize> {
        let lo = region.len().saturating_sub(cap).max(rel_floor);
        last_begin(&region[lo..]).map(|at| lo + at)
    };

    if region.len() >= V2_FOOTER && region.ends_with(END_V2) {
        // The size says where the block begins; this checks it was telling the
        // truth before a single byte is removed. A file's last fifteen bytes
        // are exactly what a truncated download corrupts.
        let digits = &region[region.len() - V2_FOOTER..region.len() - END_V2.len()];
        let by_size = std::str::from_utf8(digits)
            .ok()
            .and_then(|d| d.parse::<usize>().ok())
            .and_then(|size| region.len().checked_sub(size + V2_FOOTER))
            .filter(|&start| start >= rel_floor && region[start..].starts_with(BEGIN));

        // The header search is the recovery path for a size that lies, so it
        // scans everything read — which `locate` has already widened to what
        // a size that fits the file asked for.
        let start = by_size.or_else(|| search(region.len()));
        return match start {
            Some(start) => Ok(Some(Block {
                start: base + start as u64,
                end,
                version: Version::V2,
            })),
            // The footer is there and the header is not. Something ate it.
            None => Err(Error::Corrupt),
        };
    }

    if region.ends_with(END_V1) {
        return match search(V1_BLOCK_MAX) {
            Some(start) => Ok(Some(Block {
                start: base + start as u64,
                end,
                version: Version::V1,
            })),
            // `LYRICSEND` with no header within the format's own maximum is not
            // a Lyrics3 tag — it is nine bytes that happen to say so.
            None => Ok(None),
        };
    }

    Ok(None)
}

/// Reads the head and tail, and locates the block, on a handle the caller owns.
fn locate(file: &mut std::fs::File, len: u64) -> Result<Option<Block>, Error> {
    let mut head = [0u8; 10];
    file.seek(SeekFrom::Start(0))?;
    let head = match file.read(&mut head)? {
        n if n == head.len() => &head[..],
        n => &head[..n],
    };
    let floor = id3v2_end(head);

    let take = len.min(TAIL);
    let base = len - take;
    let mut tail = vec![0u8; usize::try_from(take).unwrap_or(0)];
    file.seek(SeekFrom::Start(base))?;
    file.read_exact(&mut tail)?;

    // A Lyrics3 block sits *before* an ID3v1 trailer, so that is where to start
    // looking backwards from.
    let has_id3v1 =
        tail.len() as u64 >= ID3V1_LEN && &tail[tail.len() - ID3V1_LEN as usize..][..3] == b"TAG";
    let end = if has_id3v1 { len - ID3V1_LEN } else { len };

    // A v2 block longer than the window: read back exactly as far as its size
    // says, if the file has room for that above the ID3v2 tag and the format
    // allows it. A size that fails either test is one `find_in` treats as a
    // lie, recovering from the header search or reporting the damage.
    if let Some(size) = v2_size(&tail, base, end) {
        let reach = size + V2_FOOTER as u64;
        if reach > end - base && reach <= V2_BLOCK_MAX && reach <= end.saturating_sub(floor) {
            let base = end - reach;
            let mut wider = vec![0u8; usize::try_from(len - base).unwrap_or(0)];
            file.seek(SeekFrom::Start(base))?;
            file.read_exact(&mut wider)?;
            return find_in(&wider, base, end, floor);
        }
    }

    find_in(&tail, base, end, floor)
}

/// The size a v2 footer ending at `end` states, if one is there and says a
/// number.
fn v2_size(tail: &[u8], base: u64, end: u64) -> Option<u64> {
    let rel_end = usize::try_from(end.checked_sub(base)?).ok()?;
    let region = tail.get(..rel_end)?;
    if region.len() < V2_FOOTER || !region.ends_with(END_V2) {
        return None;
    }
    let digits = &region[region.len() - V2_FOOTER..region.len() - END_V2.len()];
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// The block in `path`, if there is one.
pub fn find(path: &Path) -> Result<Option<Block>, Error> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    locate(&mut file, len)
}

/// Removes it, if there is one. `Ok(false)` means there was nothing to remove.
///
/// Only a block that is present is removed: a file without one is left alone
/// and is not an error, which is also what P45 says about every other reader
/// here.
/// A file that carries a *damaged* one is a reported error rather than a silent
/// no-op: it is visibly tagged, and saying nothing happened would be false.
///
/// One handle, opened read+write, so the offsets that are written at are the
/// ones that were searched for. Two opens would leave a window in which
/// something else rewrites the file and the write lands inside the audio.
pub fn remove(path: &Path) -> Result<bool, Error> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let len = file.metadata()?.len();
    let Some(block) = locate(&mut file, len)? else {
        return Ok(false);
    };

    // Whatever follows the block — the ID3v1 trailer, or nothing.
    let mut rest = Vec::new();
    file.seek(SeekFrom::Start(block.end))?;
    file.read_to_end(&mut rest)?;

    // Truncate *first*, then re-append. The other order overwrites the block's
    // header with the trailer and only then shortens the file, so a crash in
    // between leaves the block's footer sitting above a header that is already
    // gone — a file that still plays, still looks tagged, and can never be
    // cleaned up. This order's crash window holds a valid audio file that has
    // lost its 128-byte ID3v1 tag, and re-running is a clean no-op.
    file.set_len(block.start)?;
    file.seek(SeekFrom::Start(block.start))?;
    file.write_all(&rest)?;
    file.flush()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn v2_block(text: &str) -> Vec<u8> {
        let mut block = Vec::from(BEGIN);
        block.extend_from_slice(format!("LYR{:05}{text}", text.len()).as_bytes());
        let size = block.len();
        block.extend_from_slice(format!("{size:06}").as_bytes());
        block.extend_from_slice(END_V2);
        block
    }

    fn v1_block(text: &str) -> Vec<u8> {
        let mut block = Vec::from(BEGIN);
        block.extend_from_slice(text.as_bytes());
        block.extend_from_slice(END_V1);
        block
    }

    fn id3v1() -> Vec<u8> {
        let mut out = Vec::from(*b"TAG");
        out.resize(ID3V1_LEN as usize, 0);
        out
    }

    fn build(block: &[u8], with_id3v1: bool) -> (Vec<u8>, Vec<u8>) {
        let audio = crate::meta::testing::mpeg_frames();
        let mut file = audio.clone();
        file.extend_from_slice(block);
        if with_id3v1 {
            file.extend_from_slice(&id3v1());
        }
        (file, audio)
    }

    fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// M6's acceptance for Remove Tags, for all four combinations: the audio
    /// stream is left intact, byte for byte, past the tag blocks.
    #[test]
    fn a_block_is_removed_and_the_audio_is_byte_identical() {
        let dir = TempDir::new().unwrap();
        for (label, block) in [("v2", v2_block("hello")), ("v1", v1_block("hello"))] {
            for with_id3v1 in [false, true] {
                let (file, audio) = build(&block, with_id3v1);
                let path = write(&dir, &format!("{label}{with_id3v1}.mp3"), &file);

                assert!(
                    remove(&path).unwrap(),
                    "{label}/{with_id3v1}: nothing found"
                );
                let after = std::fs::read(&path).unwrap();

                assert_eq!(
                    &after[..audio.len()],
                    &audio[..],
                    "{label}/{with_id3v1}: the audio stream changed"
                );
                assert_eq!(
                    after.len(),
                    audio.len() + if with_id3v1 { ID3V1_LEN as usize } else { 0 },
                    "{label}/{with_id3v1}: wrong number of bytes left"
                );
                if with_id3v1 {
                    assert_eq!(&after[after.len() - ID3V1_LEN as usize..][..3], b"TAG");
                }
                assert!(!remove(&path).unwrap(), "{label}: removed something twice");
                assert_eq!(std::fs::read(&path).unwrap(), after);
            }
        }
    }

    /// v2 has a six-digit size, so a block can run to 999 999 bytes — far past
    /// the tail window that bounds v1. Timestamped lyrics reach 20 KB without
    /// trying, and such a block used to be reported as damaged and left in
    /// place.
    #[test]
    fn a_v2_block_larger_than_the_tail_window_is_found_and_removed() {
        let dir = TempDir::new().unwrap();
        let block = v2_block(&"[00:01.00]la la la\n".repeat(1000));
        assert!(
            block.len() as u64 > TAIL,
            "the block must outgrow the window"
        );
        for with_id3v1 in [false, true] {
            let (file, audio) = build(&block, with_id3v1);
            let path = write(&dir, &format!("long{with_id3v1}.mp3"), &file);

            let found = find(&path).unwrap().expect("a block");
            assert_eq!(found.version, Version::V2);
            assert_eq!(found.size(), block.len() as u64);

            assert!(remove(&path).unwrap(), "{with_id3v1}: nothing removed");
            let after = std::fs::read(&path).unwrap();
            assert_eq!(&after[..audio.len()], &audio[..], "the audio changed");
            assert_eq!(
                after.len(),
                audio.len() + if with_id3v1 { ID3V1_LEN as usize } else { 0 },
                "{with_id3v1}: wrong number of bytes left"
            );
        }
    }

    /// A conforming block at the format's own maximum. The window has to be
    /// `BEGIN + 5100 + END`, not 5100 — and the failure is *silent*, because a
    /// block that is not found leaves the file untouched and the run reports
    /// success having done nothing.
    #[test]
    fn a_v1_block_at_the_documented_maximum_is_still_found() {
        let dir = TempDir::new().unwrap();
        let block = v1_block(&"x".repeat(V1_TEXT_MAX));
        assert_eq!(block.len(), V1_BLOCK_MAX);
        let (file, audio) = build(&block, false);
        let path = write(&dir, "max.mp3", &file);

        assert!(remove(&path).unwrap(), "a maximum-size block was missed");
        assert_eq!(std::fs::read(&path).unwrap(), audio);
    }

    #[test]
    fn a_file_with_no_lyrics_block_is_left_exactly_alone() {
        let dir = TempDir::new().unwrap();
        let audio = crate::meta::testing::mpeg_frames();
        for (name, bytes) in [
            ("plain.mp3", audio.clone()),
            ("withid3v1.mp3", [audio.clone(), id3v1()].concat()),
            ("empty.mp3", Vec::new()),
            ("tiny.mp3", b"LYRICS200".to_vec()),
        ] {
            let path = write(&dir, name, &bytes);
            assert!(!remove(&path).unwrap(), "{name}");
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name} was modified");
        }
    }

    /// A size field that lies must delete nothing on its own authority. Where
    /// the header is still findable the block is recovered exactly; where it is
    /// not, the file is left alone and the *user is told* — reporting `Ok(false)`
    /// for a file that visibly ends in `LYRICS200` would be a lie.
    #[test]
    fn a_corrupt_size_is_recovered_or_reported_but_never_guessed() {
        let dir = TempDir::new().unwrap();
        let good = v2_block("hello");

        for (name, size) in [
            ("huge.mp3", "999999"),
            ("wrong.mp3", "001000"),
            ("notnum.mp3", "12ab56"),
            ("zero.mp3", "000000"),
        ] {
            // A real block whose size digits have been corrupted.
            let mut block = good.clone();
            let at = block.len() - V2_FOOTER;
            block[at..at + 6].copy_from_slice(size.as_bytes());
            let (file, audio) = build(&block, false);
            let path = write(&dir, name, &file);

            assert!(remove(&path).unwrap(), "{name}: the header was findable");
            assert_eq!(
                std::fs::read(&path).unwrap(),
                audio,
                "{name}: recovered the wrong span"
            );
        }

        // And with the header gone there is nothing to recover from.
        let mut block = good.clone();
        block[..BEGIN.len()].copy_from_slice(b"XXXXXXXXXXX");
        let (file, _) = build(&block, false);
        let path = write(&dir, "headerless.mp3", &file);
        assert!(
            matches!(find(&path), Err(Error::Corrupt)),
            "a footer with no header must be reported, not ignored"
        );
        assert!(matches!(remove(&path), Err(Error::Corrupt)));
        assert_eq!(std::fs::read(&path).unwrap(), file, "it lost bytes anyway");
    }

    /// The one path in this module that could destroy audio, and the floor that
    /// closes it: an ID3v2 lyrics frame quoting `LYRICSBEGIN`, on a short file
    /// that happens to end in `LYRICSEND`.
    #[test]
    fn the_search_never_reaches_into_the_id3v2_tag_ahead_of_it() {
        let dir = TempDir::new().unwrap();
        let mut file = crate::meta::testing::id3v2(&[("USLT", "LYRICSBEGIN is how one starts")]);
        file.extend_from_slice(&crate::meta::testing::mpeg_frames());
        file.extend_from_slice(END_V1);
        let path = write(&dir, "uslt.mp3", &file);

        assert_eq!(find(&path).unwrap(), None, "it reached into the ID3v2 tag");
        assert!(!remove(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), file, "audio was destroyed");
    }

    /// A `LYRICSBEGIN` inside the lyrics is legal — the v1 spec forbids only
    /// `LYRICSEND` there — and taking the *last* one leaves a few junk bytes
    /// rather than cutting from something outside the block. Pinned so the
    /// trade-off is a decision rather than an accident.
    #[test]
    fn a_nested_header_under_strips_rather_than_over_strips() {
        let dir = TempDir::new().unwrap();
        let (file, audio) = build(&v1_block("one LYRICSBEGIN two"), false);
        let path = write(&dir, "nested.mp3", &file);

        assert!(remove(&path).unwrap());
        let after = std::fs::read(&path).unwrap();
        assert_eq!(&after[..audio.len()], &audio[..], "the audio must survive");
        // The cut is taken at the nested header, so what survives is the
        // block's own prefix — junk at the tail, and nothing more.
        assert_eq!(
            &after[audio.len()..],
            b"LYRICSBEGINone ",
            "the leftover must be the prefix, and only the prefix"
        );
    }

    #[test]
    fn a_v1_end_marker_with_no_header_finds_nothing() {
        let dir = TempDir::new().unwrap();
        let mut file = crate::meta::testing::mpeg_frames();
        file.extend_from_slice(END_V1);
        let path = write(&dir, "orphan.mp3", &file);
        assert_eq!(find(&path).unwrap(), None);
        assert!(!remove(&path).unwrap());
    }

    #[test]
    fn a_v1_header_beyond_the_formats_maximum_is_not_found() {
        let dir = TempDir::new().unwrap();
        let mut file = Vec::from(BEGIN);
        file.extend_from_slice(&vec![b'x'; V1_BLOCK_MAX + 100]);
        file.extend_from_slice(END_V1);
        let path = write(&dir, "far.mp3", &file);
        assert_eq!(find(&path).unwrap(), None);
    }

    #[test]
    fn the_version_is_reported_and_the_block_is_measured() {
        let dir = TempDir::new().unwrap();
        let block = v2_block("hello");
        let (file, audio) = build(&block, true);
        let path = write(&dir, "m.mp3", &file);
        let found = find(&path).unwrap().expect("a block");
        assert_eq!(found.version, Version::V2);
        assert_eq!(found.start, audio.len() as u64);
        assert_eq!(found.size(), block.len() as u64);
        assert_eq!(found.end, (audio.len() + block.len()) as u64);

        let path = write(&dir, "n.mp3", &build(&v1_block("hello"), false).0);
        assert_eq!(find(&path).unwrap().unwrap().version, Version::V1);
    }

    /// Only the head and tail are touched, so a block on a large file costs the
    /// same as one on a small file.
    #[test]
    fn a_large_file_is_not_read_whole() {
        let dir = TempDir::new().unwrap();
        let mut file = vec![0u8; 4 * 1024 * 1024];
        file.extend_from_slice(&v2_block("hello"));
        let path = write(&dir, "big.mp3", &file);
        let found = find(&path).unwrap().expect("a block");
        assert_eq!(found.start, 4 * 1024 * 1024);
        assert!(remove(&path).unwrap());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 4 * 1024 * 1024);
    }

    /// The ID3v2 length arithmetic, including the footer flag that adds ten.
    #[test]
    fn the_floor_is_the_end_of_the_id3v2_tag() {
        assert_eq!(id3v2_end(b"no tag here"), 0);
        assert_eq!(id3v2_end(&[]), 0);
        // Syncsafe 0x00 0x00 0x02 0x01 = 257 bytes of frames, plus the header.
        assert_eq!(id3v2_end(b"ID3\x04\x00\x00\x00\x00\x02\x01"), 10 + 257);
        // With the footer flag (bit 4) set, ten more.
        assert_eq!(id3v2_end(b"ID3\x04\x00\x10\x00\x00\x02\x01"), 10 + 257 + 10);
        // And it agrees with the builder the fixtures use.
        let tag = crate::meta::testing::id3v2(&[("TPE1", "Metallica")]);
        assert_eq!(id3v2_end(&tag[..10]), tag.len() as u64);
    }
}
