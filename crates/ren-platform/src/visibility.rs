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
/// is read from the *name*, and a `Metadata` cannot answer it. Read as a byte,
/// so a dotfile whose name is not valid UTF-8 is hidden too. **System does
/// not exist**, and saying so beats guessing at `/proc` or `/sys`.
/// **Write-protected** means no write bit is set for anyone
/// (`Permissions::readonly`), the closest thing to the DOS read-only bit. On
/// Unix it protects the file's *contents*, not its name: whether a file can
/// be renamed is decided by the folder it is in, never by its own mode.
#[cfg(not(windows))]
pub fn visibility(path: &Path, metadata: &Metadata) -> Visibility {
    Visibility {
        hidden: is_dotfile(path),
        system: false,
        read_only: metadata.permissions().readonly(),
    }
}

/// A leading `.` in the last component, tested on the bytes rather than
/// through `to_str`, which is `None` for exactly the names a renamer exists
/// to fix.
#[cfg(not(windows))]
pub(crate) fn is_dotfile(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().first() == Some(&b'.'))
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

    /// `to_str` is `None` for a name that is not UTF-8, and the dot was
    /// only looked for through it — so `.caf\xe9` showed with hidden off.
    #[test]
    #[cfg(unix)]
    fn a_dotfile_whose_name_is_not_utf8_is_hidden_too() {
        use std::os::unix::ffi::OsStrExt;

        let dir = TempDir::new().unwrap();
        let hidden = dir.path().join(std::ffi::OsStr::from_bytes(b".caf\xe9"));
        let plain = dir.path().join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
        std::fs::write(&hidden, b"x").unwrap();
        std::fs::write(&plain, b"x").unwrap();

        assert!(visibility(&hidden, &std::fs::metadata(&hidden).unwrap()).hidden);
        assert!(!visibility(&plain, &std::fs::metadata(&plain).unwrap()).hidden);
    }

    /// No write bit for anyone, which is what `Permissions::readonly` reads.
    /// A file the owner alone cannot write is not write-protected: somebody
    /// can still write it.
    #[test]
    #[cfg(unix)]
    fn write_protected_means_no_write_bit_for_anyone() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"x").unwrap();
        assert!(!visibility(&path, &std::fs::metadata(&path).unwrap()).read_only);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(visibility(&path, &std::fs::metadata(&path).unwrap()).read_only);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o464)).unwrap();
        assert!(!visibility(&path, &std::fs::metadata(&path).unwrap()).read_only);
    }
}
