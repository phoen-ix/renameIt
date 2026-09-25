//! Unix implementation.
//!
//! Deliberately explicit about what it cannot do (P5): setting the created date
//! and the DOS hidden/system/archive bits report
//! [`PlatformError::CapabilityUnsupported`] instead of quietly succeeding.
//!
//! **A symbolic link is acted on as the link it is.** The listing shows a
//! link's own dates and mode, and a rename renames the link, so reading and
//! writing dates and the read-only bit here does the same — the file it points
//! at is not on any row, and changing it would make the preview and the disk
//! disagree.

use std::ffi::OsStr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use filetime::FileTime;

use crate::{
    AttributeChange, Capability, CaseProbeCache, CaseSensitivity, FileAttributes, FileTimes,
    NamingRules, Platform, PlatformError, Result, TimeChange, read_times, visibility::is_dotfile,
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
/// **Re-read every [`FRESH_FOR`](mounts::FRESH_FOR), on a thread of its own.**
/// The table is the planner's per-row lookup, so it is cached — but the app
/// runs for hours and plugging a camera card in while it is open is the
/// ordinary case, so a table read once at startup answered POSIX for every
/// card mounted since. The lookup itself is one atomic load: asking the clock
/// whether the table had gone stale, from every rayon thread on every row,
/// cost more than the rest of the plan on a machine whose clock read is a
/// syscall (37 ms became 60 ms). A read of `/proc/self/mounts` is a few
/// kilobytes. Deliberately *not* the `CaseProbeCache` treatment — that one
/// answers its question by creating a file in the directory it is asked
/// about, which is not something a preview that reruns on every keystroke may
/// do to a user's folders.
mod mounts {
    use std::path::{Path, PathBuf};
    use std::sync::Once;
    use std::sync::atomic::{AtomicPtr, Ordering};
    use std::time::Duration;

    use crate::{NamingRules, naming};

    /// How often the mount table is read again: a card mounted a moment ago
    /// is seen within this, and an idle app pays one small read per period.
    pub(super) const FRESH_FOR: Duration = Duration::from_secs(2);

    type Table = Vec<(PathBuf, &'static NamingRules)>;

    /// Filesystems that enforce the DOS/Windows naming rules whatever host
    /// they are mounted on.
    ///
    /// Conservative on purpose: a wrong entry here *refuses* names that used
    /// to be legal, so only types that really are FAT- or NTFS-shaped are
    /// listed. `fuseblk` is how ntfs-3g reports itself; a FUSE mount that is
    /// something else entirely shows as `fuse.<name>` and is not matched.
    const DOS_LIKE: &[&str] = &["ntfs", "ntfs3", "fuseblk", "vfat", "msdos", "exfat"];

    /// The current table.
    ///
    /// **A table is never freed.** A lookup holds a `&'static` to whichever
    /// one was current when it looked, with no lock and no count, so one that
    /// is replaced has to stay valid. A new table is only kept when the mounts
    /// actually changed — a re-read that finds the same mounts keeps the one
    /// there is — so what is kept is one small table per mount or unmount the
    /// process has seen.
    struct Cache {
        current: AtomicPtr<Table>,
    }

    impl Cache {
        const fn new() -> Self {
            Self {
                current: AtomicPtr::new(std::ptr::null_mut()),
            }
        }

        fn load(&self) -> Option<&'static Table> {
            // SAFETY: null, or a leaked `Box` that is never freed.
            unsafe { self.current.load(Ordering::Acquire).as_ref() }
        }

        /// Makes `table` current, unless it lists what the current one does.
        /// Only ever called by one thread at a time: the first lookup, then
        /// the watcher.
        fn store(&self, table: Table) {
            if self.load().is_some_and(|current| same(current, &table)) {
                return;
            }
            self.current
                .store(Box::into_raw(Box::new(table)), Ordering::Release);
        }
    }

    fn same(a: &Table, b: &Table) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|((pa, ra), (pb, rb))| pa == pb && std::ptr::eq(*ra, *rb))
    }

    /// `None` where there is no mount table to read — not Linux, or no
    /// `/proc` — and so nothing to watch.
    fn read() -> Option<Table> {
        std::fs::read_to_string("/proc/self/mounts")
            .ok()
            .map(|text| parse(&text))
    }

    fn table() -> &'static Table {
        static CACHE: Cache = Cache::new();
        static WATCH: Once = Once::new();
        if let Some(table) = CACHE.load() {
            return table;
        }
        // The first lookup reads the table itself — every caller waits for
        // it here — and starts the thread that keeps it current. A thread
        // that cannot be started leaves the table as first read, which is
        // what every build before this one did.
        WATCH.call_once(|| {
            let Some(first) = read() else {
                CACHE.store(Vec::new());
                return;
            };
            CACHE.store(first);
            let _ = std::thread::Builder::new()
                .name("renameit-mounts".to_owned())
                .spawn(|| {
                    loop {
                        std::thread::sleep(FRESH_FOR);
                        if let Some(table) = read() {
                            CACHE.store(table);
                        }
                    }
                });
        });
        static EMPTY: Table = Vec::new();
        CACHE.load().unwrap_or(&EMPTY)
    }

    /// `/proc/mounts` is `device mountpoint fstype options dump pass`, with
    /// octal escapes in the two path-ish fields.
    ///
    /// The table holds every DOS-shaped mount, and every *other* mount that
    /// sits underneath one — an ext4 volume bind-mounted inside an NTFS
    /// tree, say — so the longest-prefix lookup can fall back to POSIX
    /// there rather than answering with the parent's rules. Every other
    /// mount is left out: on a machine with no DOS volume the table is
    /// empty and the planner's per-row lookup scans nothing, and a table
    /// that listed `/` would make every path a hit.
    fn parse(text: &str) -> Vec<(PathBuf, &'static NamingRules)> {
        let mounts: Vec<(PathBuf, bool)> = text
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let _device = fields.next()?;
                let mount = unescape(fields.next()?);
                let fstype = fields.next()?;
                Some((PathBuf::from(mount), DOS_LIKE.contains(&fstype)))
            })
            .collect();
        mounts
            .iter()
            .filter_map(|(mount, dos)| {
                if *dos {
                    Some((mount.clone(), &naming::WINDOWS))
                } else if mounts.iter().any(|(other, other_dos)| {
                    *other_dos && mount != other && mount.starts_with(other)
                }) {
                    Some((mount.clone(), &naming::POSIX))
                } else {
                    None
                }
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

    /// Looked up by an absolute path, because the table's mount points are
    /// absolute and the match is on the text: `./x` on a FAT stick would
    /// otherwise match nothing and get POSIX rules.
    pub(super) fn rules_for(path: &Path) -> &'static NamingRules {
        let table = table();
        if table.is_empty() {
            return &naming::POSIX;
        }
        let absolute;
        let path = if path.is_absolute() {
            path
        } else {
            absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            &absolute
        };
        longest_match(table, path).unwrap_or(&naming::POSIX)
    }

    /// The deepest mount point that is a prefix of `path` wins — `/mnt/stick`
    /// has to beat `/`, or every path on the machine takes the root's rules.
    fn longest_match(
        table: &[(PathBuf, &'static NamingRules)],
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
/dev/sde1 /mnt/photos/linux ext4 rw 0 0
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
                    Path::new("/mnt/photos/linux"),
                ],
                "the root and nfs4 are left out; the ext4 volume *under* the stick is kept, \
                 as POSIX, so it is not answered by its parent; the escaped space is decoded"
            );
            let (_, linux) = &table[3];
            assert!(std::ptr::eq(*linux, &naming::POSIX));
        }

        /// A Linux volume mounted inside a FAT tree keeps its own rules: a
        /// `?` in a name there is legal, and the parent must not refuse it.
        #[test]
        fn a_posix_volume_under_a_dos_one_keeps_posix_rules() {
            let table = parse(SAMPLE);
            let inside = longest_match(&table, Path::new("/mnt/photos/linux/what?.txt"));
            assert!(std::ptr::eq(inside.unwrap(), &naming::POSIX));
        }

        #[test]
        fn the_deepest_mount_point_wins() {
            let table = parse(SAMPLE);
            // A nested mount must not be answered by its parent.
            let deep = longest_match(&table, Path::new("/mnt/photos/deep/DCIM/a.jpg"));
            assert!(std::ptr::eq(deep.unwrap(), &naming::WINDOWS));
            let shallow = longest_match(&table, Path::new("/mnt/photos/DCIM/a.jpg"));
            assert!(std::ptr::eq(shallow.unwrap(), &naming::WINDOWS));
            // And an ordinary path matches nothing, so it falls back to POSIX.
            assert!(longest_match(&table, Path::new("/home/mk/a.txt")).is_none());
        }

        /// The point of the whole module: a name that is fine on ext4 and
        /// catastrophic on FAT is refused *on the stick* and allowed at home.
        #[test]
        fn a_name_legal_on_ext4_is_refused_on_a_fat_stick() {
            let table = parse(SAMPLE);
            let on_stick =
                longest_match(&table, Path::new("/mnt/photos/holiday")).unwrap_or(&naming::POSIX);
            let at_home =
                longest_match(&table, Path::new("/home/mk/holiday")).unwrap_or(&naming::POSIX);
            assert!(on_stick.validate_component("what?.txt").is_err());
            assert!(at_home.validate_component("what?.txt").is_ok());
        }

        /// A stick mounted after the first read is in the table the next
        /// read keeps; the same mounts read again keep the table there is.
        #[test]
        fn a_changed_mount_table_replaces_the_old_one() {
            let cache = Cache::new();
            assert!(cache.load().is_none());

            cache.store(Vec::new());
            let first = cache.load().unwrap();
            assert!(first.is_empty());

            cache.store(parse(SAMPLE));
            let later = cache.load().unwrap();
            assert_eq!(later.len(), 4, "the card mounted in between");
            assert!(
                first.is_empty(),
                "and what an earlier lookup holds is intact"
            );

            cache.store(parse(SAMPLE));
            assert!(
                std::ptr::eq(later, cache.load().unwrap()),
                "nothing changed, nothing kept"
            );
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

/// Same device + inode: `to` *is* `from` under another spelling.
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// `to` is `from` renamed in case only, on a volume that folds case — so
/// looking `to` up finds `from` itself.
///
/// All three are needed. Same inode alone is also two **hard links** to one
/// file, on any filesystem, and `rename(2)` between two links of one file is
/// specified to do nothing and succeed — a rename journalled as done that did
/// not happen. So the names must sit in one folder and differ only in case,
/// which a pair of hard links on a case-sensitive volume can also do, and
/// that case is caught by the temporary-name route failing its second step.
fn is_case_only_alias(from: &Path, to: &Path) -> bool {
    let (Some(a), Some(b)) = (from.file_name(), to.file_name()) else {
        return false;
    };
    from.parent() == to.parent() && a != b && fold_equal(a, b) && is_same_file(from, to)
}

fn fold_equal(a: &OsStr, b: &OsStr) -> bool {
    match (a.to_str(), b.to_str()) {
        (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
        _ => a
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b.as_encoded_bytes()),
    }
}

/// A case-only rename on a volume that folds case, in two steps.
///
/// The Linux VFS looks `to` up, finds the same file as `from`, and returns
/// success having changed nothing — so on a FAT or exFAT card `IMG.JPG` →
/// `IMG.jpg` was journalled as done and stayed `IMG.JPG`. Through a
/// temporary sibling the second step is an ordinary rename onto a name that
/// no longer resolves to anything.
///
/// If the second step is refused, the first is put back, so a refusal
/// changes nothing. A crash *between* the two leaves the file under the
/// temporary name, which recovery does not know to look for — a window of one
/// syscall, and the file's contents are untouched.
fn rename_through_temp(from: &Path, to: &Path) -> Result<()> {
    let temp = park(from)?;
    if let Err(error) = rename_noreplace(&temp, to) {
        let _ = rename_noreplace(&temp, from);
        return Err(error);
    }
    Ok(())
}

/// Moves `from` to an unused hidden name beside it, and says which.
fn park(from: &Path) -> Result<PathBuf> {
    let pid = std::process::id();
    for n in 0..64 {
        let temp = from.with_file_name(format!(".renameit-case-{pid}-{n}"));
        match rename_noreplace(from, &temp) {
            Ok(()) => return Ok(temp),
            Err(PlatformError::TargetExists { .. }) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(PlatformError::io(
        from,
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "no free temporary name for a case-only rename",
        ),
    ))
}

/// `rename` that fails rather than replaces, as atomically as the filesystem
/// allows (P13).
#[cfg(target_os = "linux")]
fn rename_noreplace(from: &Path, to: &Path) -> Result<()> {
    match renameat2_noreplace(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(PlatformError::TargetExists {
                path: to.to_path_buf(),
            })
        }
        // The filesystem, or a kernel older than 3.15, does not do the flag.
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)
            ) =>
        {
            link_then_unlink(from, to)
        }
        Err(e) => Err(PlatformError::io(from, e)),
    }
}

#[cfg(not(target_os = "linux"))]
fn rename_noreplace(from: &Path, to: &Path) -> Result<()> {
    probe_then_rename(from, to)
}

/// The syscall itself, not glibc's `renameat2` wrapper: the wrapper only
/// exists from glibc 2.28, and linking it would stop the release binary from
/// starting on an older distribution for the sake of one call.
#[cfg(target_os = "linux")]
fn renameat2_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both strings are NUL-terminated and outlive the call; the
    // integer arguments are widened to the register size `syscall` reads.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::c_long::from(libc::AT_FDCWD),
            from.as_ptr(),
            libc::c_long::from(libc::AT_FDCWD),
            to.as_ptr(),
            libc::c_long::from(libc::RENAME_NOREPLACE),
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// The fallback for a filesystem without `RENAME_NOREPLACE`: a new link
/// cannot be created over an existing name, so for a file this is as safe as
/// the flag. A folder cannot be hard-linked, and some filesystems (FAT, many
/// SMB servers) refuse links outright; those take the probe.
#[cfg(target_os = "linux")]
fn link_then_unlink(from: &Path, to: &Path) -> Result<()> {
    let is_dir = from.symlink_metadata().is_ok_and(|md| md.is_dir());
    if !is_dir {
        match std::fs::hard_link(from, to) {
            Ok(()) => {
                return match std::fs::remove_file(from) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        // Two names for one file is not a rename; take the
                        // new one back and report the old one's problem.
                        let _ = std::fs::remove_file(to);
                        Err(PlatformError::io(from, e))
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(PlatformError::TargetExists {
                    path: to.to_path_buf(),
                });
            }
            Err(_) => {}
        }
    }
    probe_then_rename(from, to)
}

/// Check, then rename: the one form every filesystem supports, and the one
/// with a window between the two in which another process's new file at `to`
/// is replaced. Only reached where nothing atomic is available.
fn probe_then_rename(from: &Path, to: &Path) -> Result<()> {
    if to.symlink_metadata().is_ok() {
        return Err(PlatformError::TargetExists {
            path: to.to_path_buf(),
        });
    }
    std::fs::rename(from, to).map_err(|e| PlatformError::io(from, e))
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
        // A name that is already taken is refused before anything is tried —
        // unless the name taken is `from`'s own, spelled in another case.
        if to.symlink_metadata().is_ok() {
            if is_case_only_alias(from, to) {
                return rename_through_temp(from, to);
            }
            return Err(PlatformError::TargetExists {
                path: to.to_path_buf(),
            });
        }
        rename_noreplace(from, to)
    }

    fn replace_file(&self, temp: &Path, target: &Path) -> Result<()> {
        // Refused when there is nothing to replace, as `ReplaceFileW` is, so
        // the two platforms agree on what the call means.
        target
            .symlink_metadata()
            .map_err(|e| PlatformError::io(target, e))?;
        std::fs::rename(temp, target).map_err(|e| PlatformError::io(temp, e))
    }

    fn get_attributes(&self, path: &Path) -> Result<FileAttributes> {
        let md = std::fs::symlink_metadata(path).map_err(|e| PlatformError::io(path, e))?;
        Ok(FileAttributes {
            read_only: md.permissions().readonly(),
            hidden: is_dotfile(path),
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
        let md = std::fs::symlink_metadata(path).map_err(|e| PlatformError::io(path, e))?;
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
        // `chmod` follows a link to its target, and Linux has no `lchmod`:
        // the link's own mode is fixed and ignored. Changing the target
        // instead would change a file no row is about. Refused as the
        // capability this platform does not advertise, so the words match
        // the conflict the preview already showed for the row (D241).
        if md.file_type().is_symlink() {
            return Err(PlatformError::CapabilityUnsupported {
                capability: Capability::LinkReadOnly,
                platform: PLATFORM,
            });
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
        if change.accessed.is_none() && change.modified.is_none() {
            return Ok(());
        }
        let md = std::fs::symlink_metadata(path).map_err(|e| PlatformError::io(path, e))?;
        if md.file_type().is_symlink() {
            // The link's own times. There is no "leave this one alone" for a
            // link, so the stamp not asked for is written back as it is.
            let accessed = change.accessed.map_or_else(
                || FileTime::from_last_access_time(&md),
                FileTime::from_system_time,
            );
            let modified = change.modified.map_or_else(
                || FileTime::from_last_modification_time(&md),
                FileTime::from_system_time,
            );
            return filetime::set_symlink_file_times(path, accessed, modified)
                .map_err(|e| PlatformError::io(path, e));
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
            Ok(mut child) => {
                // Reaped on a thread of its own: std never waits for a dropped
                // child, and each exited `xdg-open` would otherwise stay a
                // zombie until the app quits.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                Ok(())
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The route a case-insensitive volume takes, run where it can be: the
    /// two steps leave exactly the new name.
    #[test]
    fn a_rename_through_a_temporary_name_lands_on_the_new_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let from = dir.path().join("IMG_0001.JPG");
        let to = dir.path().join("IMG_0001.jpg");
        std::fs::write(&from, b"photo").unwrap();

        rename_through_temp(&from, &to).unwrap();

        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(
            names,
            ["IMG_0001.jpg"],
            "nothing left under a temporary name"
        );
        assert_eq!(std::fs::read(&to).unwrap(), b"photo");
    }

    /// The fallback for a filesystem without `RENAME_NOREPLACE` is as
    /// no-clobber as the flag: a link cannot be made over an existing name.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_link_fallback_moves_a_file_and_never_replaces_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let from = dir.path().join("a.txt");
        let taken = dir.path().join("taken.txt");
        std::fs::write(&from, b"mine").unwrap();
        std::fs::write(&taken, b"theirs").unwrap();

        let err = link_then_unlink(&from, &taken).expect_err("occupied");
        assert!(matches!(err, PlatformError::TargetExists { .. }), "{err:?}");
        assert_eq!(std::fs::read(&taken).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&from).unwrap(), b"mine");

        let to = dir.path().join("b.txt");
        link_then_unlink(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"mine");

        // A folder cannot be linked, so it takes the probe.
        let folder = dir.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        link_then_unlink(&folder, &dir.path().join("moved")).unwrap();
        assert!(dir.path().join("moved").is_dir());
    }

    /// The flag itself, on the filesystem the tests run on.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_atomic_rename_refuses_an_existing_target() {
        let dir = tempfile::TempDir::new().unwrap();
        let from = dir.path().join("a.txt");
        let taken = dir.path().join("taken.txt");
        std::fs::write(&from, b"mine").unwrap();
        std::fs::write(&taken, b"theirs").unwrap();

        match rename_noreplace(&from, &taken) {
            Err(PlatformError::TargetExists { .. }) => {}
            other => panic!("expected TargetExists, got {other:?}"),
        }
        assert_eq!(std::fs::read(&taken).unwrap(), b"theirs");
    }

    #[test]
    fn a_case_only_alias_needs_one_folder_and_a_case_only_difference() {
        assert!(fold_equal(OsStr::new("IMG.JPG"), OsStr::new("img.jpg")));
        assert!(fold_equal(OsStr::new("ÉTÉ.txt"), OsStr::new("été.txt")));
        assert!(!fold_equal(OsStr::new("a.txt"), OsStr::new("b.txt")));
        // Not the same file at all: never an alias, however the names compare.
        let dir = tempfile::TempDir::new().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("A.txt");
        std::fs::write(&a, b"1").unwrap();
        std::fs::write(&b, b"2").unwrap();
        assert!(!is_case_only_alias(&a, &b));
    }
}
