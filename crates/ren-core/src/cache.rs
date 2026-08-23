//! Compile-once storage for the expensive parts of an operation.
//!
//! Operations are pure functions of a string, but some of them need a compiled
//! regex to do the job — and `apply` runs once *per file per keystroke*.
//! Compiling on every call turned a 10 000-file plan into 155 ms against a
//! 50 ms budget; compiling once per operation brings it back under.
//!
//! [`Cached`] is the seam. It is invisible to serde, equality and `Clone` —
//! configuration is what identifies an operation, not whatever it has compiled
//! so far.

use std::sync::OnceLock;

/// A value derived from an operation's configuration, computed on first use.
pub struct Cached<T> {
    cell: OnceLock<T>,
}

impl<T> Cached<T> {
    pub fn new() -> Self {
        Self {
            cell: OnceLock::new(),
        }
    }

    /// Returns the cached value, computing it on the first call.
    pub fn get_or_init(&self, build: impl FnOnce() -> T) -> &T {
        self.cell.get_or_init(build)
    }
}

impl<T> Default for Cached<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A clone starts with an empty cache: it is the configuration that was copied,
/// not the work.
impl<T> Clone for Cached<T> {
    fn clone(&self) -> Self {
        Self::new()
    }
}

/// Two operations are equal when their configuration is, regardless of what
/// either has compiled.
impl<T> PartialEq for Cached<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<T> Eq for Cached<T> {}

impl<T> std::fmt::Debug for Cached<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.cell.get().is_some() {
            "Cached(ready)"
        } else {
            "Cached(empty)"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_value_is_built_once_and_reused() {
        let builds = AtomicUsize::new(0);
        let cached: Cached<u32> = Cached::new();
        for _ in 0..5 {
            let value = cached.get_or_init(|| {
                builds.fetch_add(1, Ordering::SeqCst);
                42
            });
            assert_eq!(*value, 42);
        }
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_clone_starts_empty_but_compares_equal() {
        let cached: Cached<u32> = Cached::new();
        cached.get_or_init(|| 1);
        let copy = cached.clone();
        assert_eq!(cached, copy);
        assert!(format!("{copy:?}").contains("empty"));
        assert!(format!("{cached:?}").contains("ready"));
    }
}
