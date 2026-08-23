//! Whether a file is hidden, system or write-protected — as this OS means it.
//!
//! The File System settings page: show write-protected, hidden and system
//! files and folders. By default write-protected files and folders are
//! included, System and Hidden ones are not.
//!
//! Three switches, and all three mean different things on the two platforms, so
//! this is `ren-platform`'s job (D3). Free functions rather than `Platform`
//! methods: `ren_core::listing::list` has no platform in hand and threading one
//! through would touch every call site to answer a question that has exactly
//! one answer per build.
//!
//! Read from a `Metadata` the caller already has, never from a fresh `stat`.
//! The listing walk holds one per entry, and a second syscall per file to
//! decide whether to show it would cost a 10 000-file listing more than the
//! walk itself.

use std::fs::Metadata;
use std::path::Path;

/// What this OS says about one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Visibility {
    pub hidden: bool,
    /// Always false off Windows: no other filesystem we support has the bit,
    /// and inventing a meaning for it would make a preset behave differently
    /// depending on where it ran.
    pub system: bool,
    pub read_only: bool,
}

/// Reads the three from an entry the caller has already stat'd.
#[cfg(windows)]
pub fn visibility(_path: &Path, metadata: &Metadata) -> Visibility {
    use std::os::windows::fs::MetadataExt;

    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const READONLY: u32 = 0x1;

    let attributes = metadata.file_attributes();
    Visibility {
        hidden: attributes & HIDDEN != 0,
        system: attributes & SYSTEM != 0,
        read_only: attributes & READONLY != 0,
    }
}

/// The same question, answered the way this platform asks it.
///
/// **Hidden is a leading dot**, which is a convention rather than a bit — so it
/// is read from the *name*, and a `Metadata` cannot answer it. **System does
/// not exist**, and saying so beats guessing at `/proc` or `/sys`.
/// **Write-protected** is the owner write permission, which is the closest
/// thing to the DOS read-only bit and the thing that actually stops a rename.
#[cfg(not(windows))]
pub fn visibility(path: &Path, metadata: &Metadata) -> Visibility {
    Visibility {
        hidden: path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.')),
        system: false,
        read_only: metadata.permissions().readonly(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    #[cfg(unix)]
    fn a_leading_dot_is_what_hidden_means_here() {
        let dir = TempDir::new().unwrap();
        let hidden = dir.path().join(".secret");
        let plain = dir.path().join("plain.txt");
        std::fs::write(&hidden, b"x").unwrap();
        std::fs::write(&plain, b"x").unwrap();

        assert!(visibility(&hidden, &std::fs::metadata(&hidden).unwrap()).hidden);
        assert!(!visibility(&plain, &std::fs::metadata(&plain).unwrap()).hidden);
    }

    /// No filesystem we support outside Windows carries the bit, and a preset
    /// that behaved differently depending on where it ran would be worse than
    /// one that never hides anything for this reason.
    #[test]
    #[cfg(unix)]
    fn nothing_is_a_system_file_here() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"x").unwrap();
        assert!(!visibility(&path, &std::fs::metadata(&path).unwrap()).system);
    }

    #[test]
    #[cfg(unix)]
    fn write_protection_is_the_permission_that_stops_a_rename() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"x").unwrap();
        assert!(!visibility(&path, &std::fs::metadata(&path).unwrap()).read_only);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(visibility(&path, &std::fs::metadata(&path).unwrap()).read_only);
    }
}
