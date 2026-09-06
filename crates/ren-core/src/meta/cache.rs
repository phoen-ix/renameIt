//! The one cache every metadata reader sits behind (P44).
//!
//! Five readers — audio tags, Exif, image headers, HTML titles, folder
//! counts — each carried a private copy of the same shape: a process-wide map
//! from *path plus length plus mtime* to whatever was read, cleared wholesale
//! when full (P52). Two things were wrong with every copy, and fixing them
//! five times is how they drift apart again.
//!
//! **The key cost a `stat` per lookup.** Each reader called
//! `std::fs::metadata` to build its key, from inside rayon's parallel pass,
//! once per file per keystroke — for a stamp the listing already holds.
//! `FileEntry` carries `size` and `modified` from the walk, and D139/D140
//! define the listing as the snapshot: F9 relists and forgets. So a lookup
//! made on behalf of a listed entry uses the entry's own stamp and touches no
//! syscall; only a path with no entry behind it (the folder peek's candidates)
//! is stat-ed.
//!
//! **The map was behind a `Mutex`.** Eight rayon threads queued on one lock
//! for every `<Artist>` row of every keystroke, and the whole point of the
//! parallel pass is that they do not wait on each other. A hit is a read, so
//! this is a `RwLock`: readers share, and only a miss takes the write side.
//!
//! The map is keyed by path alone and stores the stamp beside the value. A
//! lookup therefore allocates nothing — `HashMap<PathBuf, _>` answers a
//! `&Path` — and a file that changed replaces its own entry rather than
//! leaving the old one behind to count against the capacity.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Duration;

use crate::model::FileEntry;

/// What makes a cached read stale: the file's length and its mtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub len: u64,
    pub modified: Option<Duration>,
    pub is_dir: bool,
}

impl Stamp {
    /// The listing's own record of the file — no syscall.
    pub(crate) fn of_entry(entry: &FileEntry) -> Self {
        Self {
            len: entry.size,
            modified: entry
                .modified
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()),
            is_dir: entry.is_dir,
        }
    }

    /// A fresh `stat`, for a path nothing listed: a folder peek's candidate,
    /// a test. Follows symlinks, as the readers always have.
    pub(crate) fn stat(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            len: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()),
            is_dir: metadata.is_dir(),
        })
    }
}

/// A process-wide, stamp-checked cache of one reader's answers.
///
/// `V` is what the reader returns — usually an `Option` of an `Arc`, so a
/// hit is a refcount bump and a miss is remembered too (D136's lesson: not
/// caching a refusal means asking again every frame).
pub(crate) struct MetaCache<V> {
    map: RwLock<HashMap<PathBuf, (Stamp, V)>>,
}

impl<V> Default for MetaCache<V> {
    fn default() -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
        }
    }
}

impl<V: Clone> MetaCache<V> {
    /// The cached answer for `path` at `stamp`, or `read()`'s, remembered.
    pub(crate) fn get_or_read(&self, path: &Path, stamp: Stamp, read: impl FnOnce() -> V) -> V {
        if let Ok(map) = self.map.read()
            && let Some((known, value)) = map.get(path)
            && *known == stamp
        {
            return value.clone();
        }

        let value = read();
        if let Ok(mut map) = self.map.write() {
            // Wholesale, as ever (P52): the capacity is above every listing
            // the app is designed for, so this is a leak stopper, not an
            // eviction policy.
            if map.len() >= super::CACHE_CAPACITY && !map.contains_key(path) {
                map.clear();
            }
            map.insert(path.to_path_buf(), (stamp, value.clone()));
        }
        value
    }

    /// Forgets everything (D140).
    pub(crate) fn clear(&self) {
        if let Ok(mut map) = self.map.write() {
            map.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn stamp(len: u64) -> Stamp {
        Stamp {
            len,
            modified: Some(Duration::from_secs(1)),
            is_dir: false,
        }
    }

    /// The same path at the same stamp is read once; a changed stamp is read
    /// again and *replaces* the entry rather than sitting beside it.
    #[test]
    fn a_hit_is_the_stamp_agreeing_and_a_change_replaces_the_entry() {
        let cache: MetaCache<Option<u64>> = MetaCache::default();
        let reads = AtomicUsize::new(0);
        let read = |value: u64| {
            reads.fetch_add(1, Ordering::SeqCst);
            Some(value)
        };
        let path = Path::new("/x/a.mp3");

        assert_eq!(cache.get_or_read(path, stamp(10), || read(1)), Some(1));
        assert_eq!(cache.get_or_read(path, stamp(10), || read(2)), Some(1));
        assert_eq!(reads.load(Ordering::SeqCst), 1, "one parse for one stamp");

        assert_eq!(cache.get_or_read(path, stamp(11), || read(3)), Some(3));
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        assert_eq!(cache.map.read().unwrap().len(), 1, "replaced, not added");
    }

    /// A refusal is an answer too, so a file that reads as nothing is not
    /// re-read on every keystroke.
    #[test]
    fn a_none_is_cached_like_any_other_answer() {
        let cache: MetaCache<Option<u64>> = MetaCache::default();
        let reads = AtomicUsize::new(0);
        let path = Path::new("/x/not-music.txt");
        for _ in 0..3 {
            let value = cache.get_or_read(path, stamp(5), || {
                reads.fetch_add(1, Ordering::SeqCst);
                None
            });
            assert_eq!(value, None);
        }
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    /// The listing's stamp and a fresh `stat` agree, which is what lets the
    /// parallel pass skip the syscall.
    #[test]
    fn the_entry_stamp_matches_the_stat_stamp() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"hello").unwrap();
        let entry = FileEntry::from_path(&path).unwrap();
        assert_eq!(Stamp::of_entry(&entry), Stamp::stat(&path).unwrap());
    }
}
