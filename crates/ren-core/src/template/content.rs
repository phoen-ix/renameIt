//! The two tags that read a file's bytes: `<Crc32>` and `<DetectedExt>`.
//!
//! **Cached (P44).** The parallel pass renders a template once per file per
//! keystroke, in any card, so `<Crc32>` used to re-hash every byte of every
//! file on each edit — a folder of videos was re-read in full per keystroke,
//! and twice for a template that named the tag twice. Both answers now sit
//! behind the same stamp-keyed cache as every other reader, keyed on the
//! listing's own length and mtime: a file rewritten in place gets a new stamp
//! when it is relisted, and F9 forgets everything (D140).
//!
//! **Only a regular file is read.** A named pipe is listed like any other file,
//! with a length of 0, and opening one blocks until something writes to it —
//! which wedged the single preview worker until the app was restarted. A
//! character device such as `/dev/zero` would be hashed forever. The other
//! readers stop at a minimum-size gate a pipe never passes; these two accept
//! any length, so they check the file type instead. The check is one `stat`,
//! taken only on a cache miss, just before the file is opened anyway.

use std::path::Path;
use std::sync::OnceLock;

use crate::meta::cache::{MetaCache, Stamp};
use crate::model::FileEntry;

/// `<Crc32>`: eight uppercase hex digits, zero-padded so every checksum has the
/// same width.
pub(crate) fn crc32_of_entry(entry: &FileEntry) -> Option<String> {
    let stamp = Stamp::of_entry(entry);
    if stamp.is_dir {
        return None;
    }
    CRC32
        .get_or_init(Default::default)
        .get_or_read(&entry.path, stamp, || crc32(&entry.path))
        .map(|crc| format!("{crc:08X}"))
}

/// `<DetectedExt>`: the extension the file's *content* says it should have.
pub(crate) fn detected_extension_of_entry(entry: &FileEntry) -> Option<String> {
    let stamp = Stamp::of_entry(entry);
    if stamp.is_dir {
        return None;
    }
    DETECTED
        .get_or_init(Default::default)
        .get_or_read(&entry.path, stamp, || detected_extension(&entry.path))
        .map(str::to_owned)
}

static CRC32: OnceLock<MetaCache<Option<u32>>> = OnceLock::new();
/// `infer` names its extensions with `'static` strings, so a hit copies
/// nothing until the tag renders it.
static DETECTED: OnceLock<MetaCache<Option<&'static str>>> = OnceLock::new();

/// Drops every cached read — for a refresh that must see files changed behind
/// the listing's back (D140), and for tests.
pub fn forget_all() {
    if let Some(cache) = CRC32.get() {
        cache.clear();
    }
    if let Some(cache) = DETECTED.get() {
        cache.clear();
    }
}

/// Follows a symlink, as opening the file would.
fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn crc32(path: &Path) -> Option<u32> {
    use std::io::Read;

    if !is_regular_file(path) {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return None,
        }
    }
    Some(hasher.finalize())
}

fn detected_extension(path: &Path) -> Option<&'static str> {
    if !is_regular_file(path) {
        return None;
    }
    infer::get_from_path(path)
        .ok()
        .flatten()
        .map(|kind| kind.extension())
}
