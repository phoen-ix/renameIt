//! Unix implementation.
//!
//! Deliberately explicit about what it cannot do (P5): setting the created date
//! and the DOS hidden/system/archive bits report
//! [`PlatformError::CapabilityUnsupported`] instead of quietly succeeding.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::Command;

use filetime::FileTime;

use crate::{
    AttributeChange, Capability, CaseProbeCache, CaseSensitivity, FileAttributes, FileTimes,
    NamingRules, Platform, PlatformError, Result, TimeChange, read_times,
};

const PLATFORM: &str = "Linux/Unix";

/// Which naming rules a path's *volume* actually enforces.
///
/// The trait has always documented `naming_rules` as "the rules for the volume
/// `path` lives on" and both implementations ignored the argument, so a file on
/// a FAT stick or a mounted NTFS drive was validated with POSIX rules — which
/// allow `:`, `?`, `*` and control characters. The rename then either failed at
/// the syscall or, worse, succeeded and produced a name the volume's other
/// operating system cannot open. **M9** lists this as open work.
///
/// Read-only and lock-free after the first call: `/proc/mounts` is parsed once
/// into a `OnceLock`. Deliberately *not* the `CaseProbeCache` treatment — that
/// one answers its question by creating a file in the directory it is asked
/// about, which is not something a preview that reruns on every keystroke may
/// do to a user's folders.
mod mounts {
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use crate::{NamingRules, naming};

    /// Filesystems that enforce the DOS/Windows naming rules whatever host
    /// they are mounted on.
    ///
    /// Conservative on purpose: a wrong entry here *refuses* names that used
    /// to be legal, so only types that really are FAT- or NTFS-shaped are
    /// listed. `fuseblk` is how ntfs-3g reports itself; a FUSE mount that is
    /// something else entirely shows as `fuse.<name>` and is not matched.
    const DOS_LIKE: &[&str] = &["ntfs", "ntfs3", "fuseblk", "vfat", "msdos", "exfat"];

    fn table() -> &'static Vec<(PathBuf, &'static NamingRules)> {
        static TABLE: OnceLock<Vec<(PathBuf, &'static NamingRules)>> = OnceLock::new();
        TABLE.get_or_init(|| {
            let text = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
            parse(&text)
        })
    }

    /// `/proc/mounts` is `device mountpoint fstype options dump pass`, with
    /// octal escapes in the two path-ish fields.
    fn parse(text: &str) -> Vec<(PathBuf, &'static NamingRules)> {
        text.lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let _device = fields.next()?;
                let mount = unescape(fields.next()?);
                let fstype = fields.next()?;
                DOS_LIKE
                    .contains(&fstype)
                    .then(|| (PathBuf::from(mount), &naming::WINDOWS))
            })
            .collect()
    }

    /// `\040` for space, `\011` tab, `\012` newline, `\134` backslash.
    fn unescape(field: &str) -> String {
        let mut out = String::with_capacity(field.len());
        let mut chars = field.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            let digits: String = chars.clone().take(3).collect();
            match u32::from_str_radix(&digits, 8)
                .ok()
                .and_then(char::from_u32)
            {
                Some(decoded) if digits.len() == 3 => {
                    out.push(decoded);
                    for _ in 0..3 {
                        chars.next();
                    }
                }
                _ => out.push('\\'),
            }
        }
        out
    }

    pub(super) fn rules_for(path: &Path) -> &'static NamingRules {
        longest_match(table(), path).unwrap_or(&naming::POSIX)
    }

    /// The deepest mount point that is a prefix of `path` wins — `/mnt/stick`
    /// has to beat `/`, or every path on the machine takes the root's rules.
    fn longest_match(
        table: &'static [(PathBuf, &'static NamingRules)],
        path: &Path,
    ) -> Option<&'static NamingRules> {
        table
            .iter()
            .filter(|(mount, _)| path.starts_with(mount))
            .max_by_key(|(mount, _)| mount.components().count())
            .map(|(_, rules)| *rules)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const SAMPLE: &str = "\
/dev/sda2 / ext4 rw,relatime 0 0
/dev/sdb1 /mnt/photos vfat rw,relatime 0 0
/dev/sdc1 /mnt/backup\\040drive ntfs3 rw 0 0
host:/export /mnt/net nfs4 rw 0 0
/dev/sdd1 /mnt/photos/deep exfat rw 0 0
";

        #[test]
        fn only_dos_shaped_filesystems_are_listed() {
            let table = parse(SAMPLE);
            let mounts: Vec<&std::path::Path> = table.iter().map(|(m, _)| m.as_path()).collect();
            assert_eq!(
                mounts,
                [
                    Path::new("/mnt/photos"),
                    Path::new("/mnt/backup drive"),
                    Path::new("/mnt/photos/deep"),
                ],
                "ext4 and nfs4 keep POSIX rules; the escaped space is decoded"
            );
        }

        #[test]
        fn the_deepest_mount_point_wins() {
            let table: &'static _ = Box::leak(Box::new(parse(SAMPLE)));
            // A nested mount must not be answered by its parent.
            let deep = longest_match(table, Path::new("/mnt/photos/deep/DCIM/a.jpg"));
            assert!(std::ptr::eq(deep.unwrap(), &naming::WINDOWS));
            let shallow = longest_match(table, Path::new("/mnt/photos/DCIM/a.jpg"));
            assert!(std::ptr::eq(shallow.unwrap(), &naming::WINDOWS));
            // And an ordinary path matches nothing, so it falls back to POSIX.
            assert!(longest_match(table, Path::new("/home/mk/a.txt")).is_none());
        }

        /// The point of the whole module: a name that is fine on ext4 and
        /// catastrophic on FAT is refused *on the stick* and allowed at home.
        #[test]
        fn a_name_legal_on_ext4_is_refused_on_a_fat_stick() {
            let table: &'static _ = Box::leak(Box::new(parse(SAMPLE)));
            let on_stick =
                longest_match(table, Path::new("/mnt/photos/holiday")).unwrap_or(&naming::POSIX);
            let at_home =
                longest_match(table, Path::new("/home/mk/holiday")).unwrap_or(&naming::POSIX);
            assert!(on_stick.validate_component("what?.txt").is_err());
            assert!(at_home.validate_component("what?.txt").is_ok());
        }
    }
}

#[derive(Debug, Default)]
pub struct UnixPlatform {
    case_cache: CaseProbeCache,
}

impl UnixPlatform {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Same device + inode, i.e. `to` *is* `from` under another spelling. Happens on
/// case-insensitive volumes mounted on Linux, where a case-only rename must not
/// be mistaken for a collision.
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// What Linux/Unix can actually write.
///
/// No created time (there is no portable API to set a birth time) and none of
/// the three DOS bits. `RevealInFileManager` is listed because the
/// implementation genuinely tries — a missing `xdg-open` is reported at the
/// point of use rather than guessed at here.
const CAPABILITIES: &[Capability] = &[
    Capability::AccessedTime,
    Capability::ModifiedTime,
    Capability::ReadOnlyAttribute,
    Capability::RevealInFileManager,
];

impl Platform for UnixPlatform {
    fn name(&self) -> &'static str {
        PLATFORM
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        // `std::fs::rename` clobbers an existing destination. Probe first.
        //
        // This is a TOCTOU window: another process can create `to` between the
        // probe and the rename. Closing it needs `renameat2(RENAME_NOREPLACE)`,
        // which is not supported on every filesystem and therefore needs a
        // fallback path — deferred to M1 with the rest of the executor.
        if to.symlink_metadata().is_ok() && !is_same_file(from, to) {
            return Err(PlatformError::TargetExists {
                path: to.to_path_buf(),
            });
        }
        std::fs::rename(from, to).map_err(|e| PlatformError::io(from, e))
    }

    fn get_attributes(&self, path: &Path) -> Result<FileAttributes> {
        let md = std::fs::metadata(path).map_err(|e| PlatformError::io(path, e))?;
        let hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        Ok(FileAttributes {
            read_only: md.permissions().readonly(),
            hidden,
            system: false,
            archive: false,
        })
    }

    fn set_attributes(&self, path: &Path, change: AttributeChange) -> Result<()> {
        // Against what the file already is, not against the request in the
        // abstract: setting `hidden` on a file whose name already begins with a
        // dot asks for no change at all, and rejecting it would break undo on
        // every dotfile. See `AttributeChange::required_capabilities_from`.
        let current = self.get_attributes(path)?;
        if let Some(capability) = self.unsupported(&change.required_capabilities_from(current)) {
            return Err(PlatformError::CapabilityUnsupported {
                capability,
                platform: PLATFORM,
            });
        }
        let Some(read_only) = change.read_only else {
            return Ok(());
        };
        let md = std::fs::metadata(path).map_err(|e| PlatformError::io(path, e))?;
        let mode = md.permissions().mode();
        // Not `Permissions::set_readonly`, which on Unix sets **all three**
        // write bits when clearing: a 0644 file round-tripped through
        // set-then-clear would come back 0666, and that round trip is exactly
        // what undo does. Clearing grants the owner and nobody else, so the
        // common 0644 and 0755 restore exactly and the error, when there is
        // one, is always in the narrowing direction.
        let next = if read_only {
            mode & !0o222
        } else {
            mode | 0o200
        };
        if next == mode {
            return Ok(());
        }
        std::fs::set_permissions(path, PermissionsExt::from_mode(next))
            .map_err(|e| PlatformError::io(path, e))
    }

    fn get_times(&self, path: &Path) -> Result<FileTimes> {
        read_times(path)
    }

    fn set_times(&self, path: &Path, change: TimeChange) -> Result<()> {
        // P5: there is no portable API to write a birth time on Linux. Say so —
        // via the capability table, so this can never drift from what
        // `capabilities()` advertises to the planner and the GUI.
        if let Some(capability) = self.unsupported(&change.required_capabilities()) {
            return Err(PlatformError::CapabilityUnsupported {
                capability,
                platform: PLATFORM,
            });
        }
        match (change.accessed, change.modified) {
            (None, None) => Ok(()),
            (Some(a), None) => filetime::set_file_atime(path, FileTime::from_system_time(a))
                .map_err(|e| PlatformError::io(path, e)),
            (None, Some(m)) => filetime::set_file_mtime(path, FileTime::from_system_time(m))
                .map_err(|e| PlatformError::io(path, e)),
            (Some(a), Some(m)) => filetime::set_file_times(
                path,
                FileTime::from_system_time(a),
                FileTime::from_system_time(m),
            )
            .map_err(|e| PlatformError::io(path, e)),
        }
    }

    fn naming_rules(&self, path: &Path) -> &'static NamingRules {
        mounts::rules_for(path)
    }

    fn case_sensitivity(&self, dir: &Path) -> CaseSensitivity {
        self.case_cache.get(dir)
    }

    fn reveal_in_file_manager(&self, path: &Path) -> Result<()> {
        let target = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        match Command::new("xdg-open").arg(target).spawn() {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(PlatformError::CapabilityUnsupported {
                    capability: Capability::RevealInFileManager,
                    platform: PLATFORM,
                })
            }
            Err(e) => Err(PlatformError::io(target, e)),
        }
    }

    fn notify_shell_changed(&self, _path: &Path) {
        // Nothing to do: Linux file managers use inotify.
    }
}
