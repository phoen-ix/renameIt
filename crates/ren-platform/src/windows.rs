//! Windows implementation.
//!
//! This is the platform that ships first (D3), and the only one that can set a
//! created date (P5) or the DOS hidden/system/archive bits.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, GetFileAttributesW,
    INVALID_FILE_ATTRIBUTES, MoveFileExW, OPEN_EXISTING, SetFileAttributesW, SetFileTime,
};
use windows_sys::Win32::UI::Shell::{SHCNE_UPDATEDIR, SHCNF_PATHW, SHChangeNotify};

use crate::{
    AttributeChange, Capability, CaseProbeCache, CaseSensitivity, FileAttributes, FileTimes,
    NamingRules, Platform, PlatformError, Result, TimeChange, naming, read_times,
};

const PLATFORM: &str = "Windows";

/// 100-nanosecond intervals between 1601-01-01 (FILETIME) and 1970-01-01 (Unix).
const EPOCH_DIFFERENCE_100NS: i64 = 116_444_736_000_000_000;

/// The four bits we own. Everything else in the attribute word is preserved.
const MANAGED_ATTRIBUTES: u32 = FILE_ATTRIBUTE_READONLY
    | FILE_ATTRIBUTE_HIDDEN
    | FILE_ATTRIBUTE_SYSTEM
    | FILE_ATTRIBUTE_ARCHIVE;

#[derive(Debug, Default)]
pub struct WindowsPlatform {
    case_cache: CaseProbeCache,
}

impl WindowsPlatform {
    pub fn new() -> Self {
        Self::default()
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// The same buffer, prefixed `\\?\` so the call is not held to `MAX_PATH`.
///
/// Every Win32 path argument here is one we built ourselves, and the ANSI-era
/// 260-character limit applies to all of them: a rename that would produce a
/// longer path fails with `ERROR_PATH_NOT_FOUND` — which reads as "the folder
/// is gone" and is nothing of the sort. `std::fs` already does this internally,
/// so only *our own* calls were short-changed; a run over a deep tree could
/// therefore rename half a folder and fail on the rest for no visible reason.
///
/// Chosen over a `longPathAware` manifest because the manifest only works where
/// the machine's `LongPathsEnabled` registry value is already on, which is not
/// something a renamer can require of the folder it is pointed at.
///
/// **Not** used for `SHChangeNotify` or the `explorer.exe /select,` argument:
/// the shell wants a path a user could have typed, and a verbatim one either
/// confuses it or shows up in the address bar.
///
/// Falls back to `wide` whenever the prefix is not one there is a verbatim form
/// of — a relative path, a device path, or one that is verbatim already.
fn wide_verbatim(path: &Path) -> Vec<u16> {
    match verbatim(path) {
        Some(full) => full.encode_wide().chain(std::iter::once(0)).collect(),
        None => wide(path),
    }
}

/// `C:\a\b` → `\\?\C:\a\b`, `\\server\share\a` → `\\?\UNC\server\share\a`.
///
/// Normalised here rather than by Windows, because that is the trade: a
/// verbatim path is handed to the filesystem as-is, so `.`, `..`, a doubled
/// separator and a forward slash all stop being rewritten for us and start
/// being literal name characters. Rebuilding from `components()` — which drops
/// `.` and normalises the separator — and popping for `..` is the same lexical
/// rule Win32 applies, which is why it is safe to do without touching the disk.
fn verbatim(path: &Path) -> Option<std::ffi::OsString> {
    use std::ffi::{OsStr, OsString};
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return None; // Relative: there is no verbatim form.
    };

    let mut out = OsString::from(r"\\?\");
    match prefix.kind() {
        // Already verbatim, or a device path (`\\.\PhysicalDrive0`): leave it.
        Prefix::Verbatim(_)
        | Prefix::VerbatimUNC(..)
        | Prefix::VerbatimDisk(_)
        | Prefix::DeviceNS(_) => return None,
        Prefix::Disk(_) => out.push(prefix.as_os_str()),
        Prefix::UNC(server, share) => {
            out.push("UNC\\");
            out.push(server);
            out.push("\\");
            out.push(share);
        }
    }

    // `C:foo` is relative to the drive's current directory, which is process
    // state a verbatim path cannot carry.
    if components.next() != Some(Component::RootDir) {
        return None;
    }

    let mut parts: Vec<&OsStr> = Vec::new();
    for component in components {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?; // Above the root: not a path we can express.
            }
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }

    if parts.is_empty() {
        out.push("\\"); // The root itself is `\\?\C:\`, never `\\?\C:`.
    }
    for part in parts {
        out.push("\\");
        out.push(part);
    }
    Some(out)
}

fn last_error(path: &Path) -> PlatformError {
    PlatformError::io(path, std::io::Error::last_os_error())
}

fn to_filetime(time: SystemTime) -> FILETIME {
    let intervals = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => EPOCH_DIFFERENCE_100NS.saturating_add((d.as_nanos() / 100) as i64),
        Err(e) => EPOCH_DIFFERENCE_100NS.saturating_sub((e.duration().as_nanos() / 100) as i64),
    };
    let raw = intervals.max(0) as u64;
    FILETIME {
        dwLowDateTime: raw as u32,
        dwHighDateTime: (raw >> 32) as u32,
    }
}

/// Only used by tests — the read path goes through `std::fs`.
#[cfg(test)]
fn from_filetime(ft: FILETIME) -> SystemTime {
    let raw = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    let unix_100ns = raw as i64 - EPOCH_DIFFERENCE_100NS;
    if unix_100ns >= 0 {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(unix_100ns as u64 * 100)
    } else {
        SystemTime::UNIX_EPOCH - std::time::Duration::from_nanos(unix_100ns.unsigned_abs() * 100)
    }
}

fn raw_attributes(path: &Path) -> Result<u32> {
    let wide = wide_verbatim(path);
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call.
    let attrs = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        return Err(last_error(path));
    }
    Ok(attrs)
}

/// Opens a handle with just enough access to call `SetFileTime`.
/// `FILE_FLAG_BACKUP_SEMANTICS` is what makes directories openable.
fn open_for_time_write(path: &Path) -> Result<HANDLE> {
    let wide = wide_verbatim(path);
    // SAFETY: `wide` is NUL-terminated and outlives the call; a null security
    // descriptor and null template handle are the documented defaults.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error(path));
    }
    Ok(handle)
}

impl Platform for WindowsPlatform {
    fn name(&self) -> &'static str {
        PLATFORM
    }

    /// Windows does all eight: `SetFileTime` writes all three stamps and
    /// `SetFileAttributesW` all four bits. Nothing is missing on Windows.
    fn capabilities(&self) -> &'static [Capability] {
        &Capability::ALL
    }

    fn context_menu_installed(&self) -> Option<bool> {
        Some(crate::shell::is_registered())
    }

    fn set_context_menu(
        &self,
        installed: bool,
        presets: &[crate::shell::MenuPreset<'_>],
    ) -> Result<()> {
        if !installed {
            return crate::shell::unregister();
        }
        // The path a shell verb records has to be the executable *as it is
        // now*: a portable copy moved to another folder must re-register, and
        // a stale verb that launches a program that is no longer there is
        // worse than no verb.
        let exe = std::env::current_exe().map_err(|e| PlatformError::io(Path::new("."), e))?;
        crate::shell::register(&exe, presets)
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let from_w = wide_verbatim(from);
        let to_w = wide_verbatim(to);
        // Flags = 0: no MOVEFILE_REPLACE_EXISTING. Unlike `std::fs::rename`,
        // this fails rather than destroying the destination — and it still
        // performs case-only renames correctly on NTFS.
        // SAFETY: both buffers are NUL-terminated and outlive the call.
        let ok = unsafe { MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), 0) };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                return Err(PlatformError::TargetExists {
                    path: to.to_path_buf(),
                });
            }
            return Err(PlatformError::io(from, err));
        }
        Ok(())
    }

    fn get_attributes(&self, path: &Path) -> Result<FileAttributes> {
        let attrs = raw_attributes(path)?;
        Ok(FileAttributes {
            read_only: attrs & FILE_ATTRIBUTE_READONLY != 0,
            hidden: attrs & FILE_ATTRIBUTE_HIDDEN != 0,
            system: attrs & FILE_ATTRIBUTE_SYSTEM != 0,
            archive: attrs & FILE_ATTRIBUTE_ARCHIVE != 0,
        })
    }

    fn set_attributes(&self, path: &Path, change: AttributeChange) -> Result<()> {
        if change.is_empty() {
            return Ok(());
        }
        let current_raw = raw_attributes(path)?;
        let current = FileAttributes {
            read_only: current_raw & FILE_ATTRIBUTE_READONLY != 0,
            hidden: current_raw & FILE_ATTRIBUTE_HIDDEN != 0,
            system: current_raw & FILE_ATTRIBUTE_SYSTEM != 0,
            archive: current_raw & FILE_ATTRIBUTE_ARCHIVE != 0,
        };
        let wanted = change.apply_to(current);

        let mut raw = current_raw & !MANAGED_ATTRIBUTES;
        if wanted.read_only {
            raw |= FILE_ATTRIBUTE_READONLY;
        }
        if wanted.hidden {
            raw |= FILE_ATTRIBUTE_HIDDEN;
        }
        if wanted.system {
            raw |= FILE_ATTRIBUTE_SYSTEM;
        }
        if wanted.archive {
            raw |= FILE_ATTRIBUTE_ARCHIVE;
        }
        // SetFileAttributesW rejects an empty attribute word; NORMAL means "no
        // other attributes" and is only valid on its own.
        if raw == 0 {
            raw = FILE_ATTRIBUTE_NORMAL;
        }

        let wide = wide_verbatim(path);
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        let ok = unsafe { SetFileAttributesW(wide.as_ptr(), raw) };
        if ok == 0 {
            return Err(last_error(path));
        }
        Ok(())
    }

    fn get_times(&self, path: &Path) -> Result<FileTimes> {
        read_times(path)
    }

    fn set_times(&self, path: &Path, change: TimeChange) -> Result<()> {
        if change.is_empty() {
            return Ok(());
        }
        let created = change.created.map(to_filetime);
        let accessed = change.accessed.map(to_filetime);
        let modified = change.modified.map(to_filetime);

        let handle = open_for_time_write(path)?;
        // SAFETY: `handle` is valid until CloseHandle below; each pointer is
        // either null (meaning "leave unchanged") or points to a live FILETIME.
        let ok = unsafe {
            SetFileTime(
                handle,
                created.as_ref().map_or(std::ptr::null(), |t| t as *const _),
                accessed
                    .as_ref()
                    .map_or(std::ptr::null(), |t| t as *const _),
                modified
                    .as_ref()
                    .map_or(std::ptr::null(), |t| t as *const _),
            )
        };
        let err = (ok == 0).then(std::io::Error::last_os_error);
        // SAFETY: `handle` came from CreateFileW and is closed exactly once.
        unsafe {
            CloseHandle(handle);
        }
        match err {
            Some(e) => Err(PlatformError::io(path, e)),
            None => Ok(()),
        }
    }

    fn naming_rules(&self, _path: &Path) -> &'static NamingRules {
        &naming::WINDOWS
    }

    fn case_sensitivity(&self, dir: &Path) -> CaseSensitivity {
        // Windows 10 1803+ can flag a directory case-sensitive for WSL. Probing
        // is cheap and correct in both worlds, so we probe rather than assume.
        self.case_cache.get(dir)
    }

    fn reveal_in_file_manager(&self, path: &Path) -> Result<()> {
        // A folder opens; a file opens its folder with the file selected.
        let arg = if path.is_dir() {
            path.display().to_string()
        } else {
            format!("/select,{}", path.display())
        };
        Command::new("explorer.exe")
            .arg(arg)
            .spawn()
            .map(|_| ())
            .map_err(|e| PlatformError::io(path, e))
    }

    fn notify_shell_changed(&self, path: &Path) {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let wide = wide(dir);
        // SAFETY: `wide` is a NUL-terminated UTF-16 path, matching SHCNF_PATHW;
        // dwItem2 is unused for SHCNE_UPDATEDIR.
        unsafe {
            SHChangeNotify(
                SHCNE_UPDATEDIR as i32,
                SHCNF_PATHW,
                wide.as_ptr() as *const c_void,
                std::ptr::null(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn filetime_conversion_round_trips() {
        for offset_secs in [0u64, 1, 1_000_000, 1_700_000_000] {
            let t = SystemTime::UNIX_EPOCH + Duration::from_secs(offset_secs);
            assert_eq!(from_filetime(to_filetime(t)), t);
        }
    }

    #[test]
    fn the_unix_epoch_maps_to_the_documented_filetime_constant() {
        let ft = to_filetime(SystemTime::UNIX_EPOCH);
        let raw = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        assert_eq!(raw, EPOCH_DIFFERENCE_100NS as u64);
    }

    // --- long paths (M8) ---------------------------------------------------
    //
    // These only compile and only run on Windows, so a Linux session cannot
    // discriminate on them at all — CI's `windows-latest` job is the check.

    fn verbatim_str(raw: &str) -> Option<String> {
        verbatim(Path::new(raw)).map(|s| s.to_string_lossy().into_owned())
    }

    #[test]
    fn a_drive_path_takes_the_verbatim_prefix() {
        assert_eq!(verbatim_str(r"C:\a\b").as_deref(), Some(r"\\?\C:\a\b"));
    }

    #[test]
    fn a_share_becomes_the_unc_form_rather_than_a_doubled_prefix() {
        assert_eq!(
            verbatim_str(r"\\server\share\a").as_deref(),
            Some(r"\\?\UNC\server\share\a")
        );
    }

    /// A verbatim path prefixed again would name a folder called `?`.
    #[test]
    fn a_path_that_is_already_verbatim_is_left_alone() {
        assert_eq!(verbatim_str(r"\\?\C:\a"), None);
    }

    /// `\\.\PhysicalDrive0` is not a filesystem path and must not be rewritten.
    #[test]
    fn a_device_path_is_left_alone() {
        assert_eq!(verbatim_str(r"\\.\PhysicalDrive0"), None);
    }

    /// No prefix, or a drive-relative `C:foo`, has no verbatim form — both fall
    /// back to the plain buffer rather than producing a wrong one.
    #[test]
    fn a_relative_path_has_no_verbatim_form() {
        assert_eq!(verbatim_str(r"a\b"), None);
        assert_eq!(verbatim_str(r"C:foo"), None);
    }

    /// The whole reason `verbatim` rebuilds instead of concatenating: past the
    /// prefix, Windows stops normalising and these all become literal names.
    #[test]
    fn the_normalisation_windows_stops_doing_is_done_here() {
        assert_eq!(
            verbatim_str(r"C:/a/./b//../c").as_deref(),
            Some(r"\\?\C:\a\c")
        );
    }

    #[test]
    fn the_root_keeps_its_separator() {
        assert_eq!(verbatim_str(r"C:\").as_deref(), Some(r"\\?\C:\"));
    }

    /// The end-to-end check, and the one that would have caught the bug: a
    /// rename whose path is past `MAX_PATH`. Before `wide_verbatim` this failed
    /// with `ERROR_PATH_NOT_FOUND`, which reads as "the folder is gone".
    #[test]
    fn a_rename_past_max_path_succeeds() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut deep = temp.path().to_path_buf();
        while deep.as_os_str().len() < 300 {
            deep.push("a".repeat(60));
        }
        std::fs::create_dir_all(&deep).expect("std::fs already handles long paths");

        let from = deep.join("before.txt");
        let to = deep.join("after.txt");
        std::fs::write(&from, b"x").unwrap();
        assert!(
            from.as_os_str().len() > 260,
            "the test tree is not deep enough"
        );

        let platform = WindowsPlatform::new();
        platform
            .rename(&from, &to)
            .expect("a long path is not a missing one");
        assert!(to.exists());

        // And the other three calls, on the same path.
        platform.get_attributes(&to).unwrap();
        platform
            .set_attributes(
                &to,
                AttributeChange {
                    archive: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        platform
            .set_times(
                &to,
                TimeChange {
                    modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
                    ..Default::default()
                },
            )
            .unwrap();
    }
}
