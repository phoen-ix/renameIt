//! What the updater needs from the operating system (D250): one HTTPS GET, a
//! SHA-256 of a file, where this copy was installed, and a way to start the
//! installer.
//!
//! Windows only. Every other platform keeps the trait's defaults, and the app
//! offers no update check there.
//!
//! **WinHTTP and CNG rather than a crate.** Both are part of Windows, so there
//! is nothing new to audit (D2). There is no C to compile either, which matters
//! because `ring` or `aws-lc` would break the Linux-hosted Windows clippy gate.
//! And the request goes through the machine's proxy settings and certificate
//! store, which is what an office network expects of a program that goes
//! online.

/// The uninstall entry `packaging/renameit.nsi` writes, under `HKCU` for a
/// per-user install or `HKLM` for a per-machine one.
///
/// Its `InstallLocation` is what tells an installed copy from a portable one.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\RenameIt";

/// A DWORD under [`UNINSTALL_KEY`]: 1 when the installer's "Right-click menu in
/// Explorer" box was ticked. Each user's first start then sets the menu up for
/// them, since the menu is per user and an elevated installer reaches only its
/// own account (D252).
pub const CONTEXT_MENU_VALUE: &str = "ContextMenu";

/// `HKLM\<POLICY_KEY>\<POLICY_VALUE>` = 1 turns every update check off, so an
/// administrator can decide this for a whole fleet. The same switch the
/// sibling apps have.
pub const POLICY_KEY: &str = r"SOFTWARE\Policies\RenameIt";
pub const POLICY_VALUE: &str = "DisableUpdateCheck";

/// What a request answered, apart from its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpHead {
    pub status: u16,
    /// The `Location` header, present on a redirect.
    pub location: Option<String>,
}

/// This copy was put here by the installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallRecord {
    /// Installed for all users, under `Program Files`. Replacing it needs an
    /// administrator.
    pub per_machine: bool,
    /// The installer was asked for the right-click menu ([`CONTEXT_MENU_VALUE`]).
    pub context_menu: bool,
}

/// Whether `recorded`, a path the registry holds, names `actual`.
///
/// Compared as text, ignoring case and a trailing separator, because both are
/// Windows paths written by different programs. For an `InstallLocation` that
/// is the whole rule: a copy in another folder counts as portable, so a
/// portable copy on a machine that also has RenameIt installed never updates
/// the installed one instead of itself.
///
/// Outside `imp` so the rule is tested on every platform.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn same_path(recorded: &str, actual: &str) -> bool {
    let tidy = |s: &str| s.trim_end_matches(['\\', '/']).to_lowercase();
    !recorded.trim().is_empty() && tidy(recorded) == tidy(actual)
}

#[cfg(windows)]
pub(crate) mod imp {
    use std::ffi::c_void;
    use std::io::{self, Read, Write};
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Networking::WinHttp::{
        INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
        WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
        WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION, WINHTTP_QUERY_STATUS_CODE,
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
        WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
        WinHttpSetTimeouts,
    };
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCryptCreateHash, BCryptDestroyHash,
        BCryptFinishHash, BCryptHashData,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};

    use super::{
        CONTEXT_MENU_VALUE, HttpHead, InstallRecord, POLICY_KEY, POLICY_VALUE, UNINSTALL_KEY,
        same_path,
    };
    use crate::{PlatformError, Result};

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A WinHTTP handle, closed when dropped.
    struct Internet(*mut c_void);

    impl Drop for Internet {
        fn drop(&mut self) {
            // SAFETY: a handle WinHTTP returned, closed exactly once.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }

    fn opened(raw: *mut c_void) -> io::Result<Internet> {
        if raw.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Internet(raw))
        }
    }

    fn ok(result: i32) -> io::Result<()> {
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn http_get(url: &str, follow_redirects: bool, body: &mut dyn Write) -> Result<HttpHead> {
        get(url, follow_redirects, body).map_err(|e| PlatformError::io(url, e))
    }

    fn get(url: &str, follow_redirects: bool, body: &mut dyn Write) -> io::Result<HttpHead> {
        let rest = url
            .strip_prefix("https://")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "only https is fetched"))?;
        let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let path = if path.is_empty() { "/" } else { path };
        let agent = wide(concat!("RenameIt/", env!("CARGO_PKG_VERSION")));
        let (host, path, verb) = (wide(host), wide(path), wide("GET"));

        // SAFETY, for every call below: each buffer is NUL-terminated and
        // outlives the call, each handle is live until its `Internet` drops
        // (request, then connection, then session: reverse declaration order),
        // and each out-pointer points at a local of the size passed with it.
        unsafe {
            let session = opened(WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                null(),
                null(),
                0,
            ))?;
            // Resolve, connect, send, receive. The receive timeout is per
            // read, not for the whole body, so a slow download that is still
            // moving is never cut off.
            ok(WinHttpSetTimeouts(
                session.0, 15_000, 15_000, 30_000, 30_000,
            ))?;
            let connection = opened(WinHttpConnect(
                session.0,
                host.as_ptr(),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            ))?;
            let request = opened(WinHttpOpenRequest(
                connection.0,
                verb.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                WINHTTP_FLAG_SECURE,
            ))?;
            if !follow_redirects {
                let policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
                ok(WinHttpSetOption(
                    request.0,
                    WINHTTP_OPTION_REDIRECT_POLICY,
                    (&raw const policy).cast(),
                    size_of::<u32>() as u32,
                ))?;
            }
            ok(WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0))?;
            ok(WinHttpReceiveResponse(request.0, null_mut()))?;

            let mut status: u32 = 0;
            let mut size = size_of::<u32>() as u32;
            ok(WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&raw mut status).cast(),
                &mut size,
                null_mut(),
            ))?;
            let location = header(request.0, WINHTTP_QUERY_LOCATION);

            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let mut read = 0u32;
                ok(WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut read,
                ))?;
                if read == 0 {
                    break;
                }
                body.write_all(&buffer[..read as usize])?;
            }
            Ok(HttpHead {
                status: status as u16,
                location,
            })
        }
    }

    /// One response header as text, or `None` if there is none.
    ///
    /// # Safety
    ///
    /// `request` must be a live request handle whose response has arrived.
    unsafe fn header(request: *mut c_void, level: u32) -> Option<String> {
        let mut size = 0u32;
        // SAFETY: a null buffer asks only for the size, in bytes.
        unsafe { WinHttpQueryHeaders(request, level, null(), null_mut(), &mut size, null_mut()) };
        if size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        // SAFETY: `buffer` holds at least `size` bytes.
        let read = unsafe {
            WinHttpQueryHeaders(
                request,
                level,
                null(),
                buffer.as_mut_ptr().cast(),
                &mut size,
                null_mut(),
            )
        };
        if read == 0 {
            return None;
        }
        buffer.truncate(size as usize / 2);
        String::from_utf16(&buffer).ok()
    }

    /// A CNG hash object, destroyed when dropped.
    struct Hash(BCRYPT_HASH_HANDLE);

    impl Drop for Hash {
        fn drop(&mut self) {
            // SAFETY: a handle `BCryptCreateHash` returned, destroyed once.
            unsafe { BCryptDestroyHash(self.0) };
        }
    }

    pub fn sha256_file(path: &Path) -> Result<[u8; 32]> {
        let failed = |status: i32| {
            PlatformError::io(path, io::Error::other(format!("CNG status {status:#x}")))
        };
        let mut file = std::fs::File::open(path).map_err(|e| PlatformError::io(path, e))?;
        let mut hash = Hash(null_mut());
        // SAFETY: the SHA-256 pseudo-handle needs no provider to be opened;
        // a null hash object asks CNG to allocate and own it.
        let status = unsafe {
            BCryptCreateHash(
                BCRYPT_SHA256_ALG_HANDLE,
                &mut hash.0,
                null_mut(),
                0,
                null(),
                0,
                0,
            )
        };
        if status != 0 {
            return Err(failed(status));
        }
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|e| PlatformError::io(path, e))?;
            if read == 0 {
                break;
            }
            // SAFETY: the first `read` bytes of `buffer` were just filled.
            let status = unsafe { BCryptHashData(hash.0, buffer.as_ptr(), read as u32, 0) };
            if status != 0 {
                return Err(failed(status));
            }
        }
        let mut digest = [0u8; 32];
        // SAFETY: SHA-256 writes exactly 32 bytes.
        let status = unsafe { BCryptFinishHash(hash.0, digest.as_mut_ptr(), 32, 0) };
        if status != 0 {
            return Err(failed(status));
        }
        Ok(digest)
    }

    /// A `REG_SZ` (or `REG_EXPAND_SZ`, unexpanded) value, or `None`.
    pub(crate) fn reg_string(root: HKEY, path: &str, name: &str) -> Option<String> {
        let (path, name) = (wide(path), wide(name));
        let mut size = 0u32;
        // SAFETY: a null buffer asks only for the size, in bytes.
        let status = unsafe {
            RegGetValueW(
                root,
                path.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        // SAFETY: `buffer` holds at least `size` bytes.
        let status = unsafe {
            RegGetValueW(
                root,
                path.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        // `size` counts the terminating NUL, which `RRF_RT_REG_SZ` guarantees.
        buffer.truncate((size as usize / 2).saturating_sub(1));
        String::from_utf16(&buffer).ok()
    }

    fn reg_dword(root: HKEY, path: &str, name: &str) -> Option<u32> {
        let (path, name) = (wide(path), wide(name));
        let mut value = 0u32;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: `RRF_RT_REG_DWORD` makes the API refuse any other type, so
        // it writes four bytes into `value` or nothing.
        let status = unsafe {
            RegGetValueW(
                root,
                path.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&raw mut value).cast(),
                &mut size,
            )
        };
        (status == ERROR_SUCCESS).then_some(value)
    }

    /// The per-user entry first, matching the installer, which looks there
    /// first when it decides what an upgrade replaces.
    pub fn install_record() -> Option<InstallRecord> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?.to_string_lossy().into_owned();
        [(HKEY_CURRENT_USER, false), (HKEY_LOCAL_MACHINE, true)]
            .into_iter()
            .find_map(|(root, per_machine)| {
                let recorded = reg_string(root, UNINSTALL_KEY, "InstallLocation")?;
                same_path(&recorded, &dir).then(|| InstallRecord {
                    per_machine,
                    context_menu: reg_dword(root, UNINSTALL_KEY, CONTEXT_MENU_VALUE) == Some(1),
                })
            })
    }

    pub fn updates_disabled_by_policy() -> bool {
        reg_dword(HKEY_LOCAL_MACHINE, POLICY_KEY, POLICY_VALUE) == Some(1)
    }

    /// Starts `installer` and returns at once, without waiting for it.
    ///
    /// `ShellExecuteExW` rather than `std::process::Command`, because `runas`
    /// is a shell verb: it is what shows the administrator prompt, and
    /// `CreateProcess` would only fail with "elevation required". A declined
    /// prompt comes back as `ERROR_CANCELLED`.
    pub fn launch_installer(installer: &Path, args: &[&str], elevate: bool) -> Result<()> {
        shell_execute(
            installer,
            if elevate { "runas" } else { "open" },
            &args.join(" "),
        )
    }

    /// `ShellExecuteExW` with `verb`: "open" on a document starts its default
    /// program, as Explorer's double-click does (P115).
    pub(crate) fn shell_execute(path: &Path, verb: &str, parameters: &str) -> Result<()> {
        let file = wide(&path.to_string_lossy());
        let parameters = wide(parameters);
        let verb = wide(verb);
        // SAFETY: an all-zero SHELLEXECUTEINFOW is the documented "nothing
        // set"; every pointer then set is a NUL-terminated buffer that outlives
        // the call.
        let ok = unsafe {
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
            // The app exits straight after this, and an asynchronous launch
            // could still be in flight when it does.
            info.fMask = SEE_MASK_NOASYNC;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = parameters.as_ptr();
            info.nShow = 1; // SW_SHOWNORMAL
            ShellExecuteExW(&mut info)
        };
        if ok == 0 {
            return Err(PlatformError::io(path, io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Removes the Explorer menu if it launches this executable.
    ///
    /// The menu is one per user, whichever copy of the program wrote it last.
    /// So the uninstaller removes it only when it points at the copy being
    /// uninstalled, and a portable copy that owns it keeps it.
    pub fn release_context_menu() -> Result<()> {
        let root = &crate::shell::ShellPlan::verb_roots()[0];
        let Some(icon) = reg_string(HKEY_CURRENT_USER, root, "Icon") else {
            return Ok(());
        };
        let Some(menu_exe) = crate::shell::plan::exe_from_icon(&icon) else {
            return Ok(());
        };
        let Ok(exe) = std::env::current_exe() else {
            return Ok(());
        };
        if same_path(menu_exe, &exe.to_string_lossy()) {
            crate::shell::unregister()
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_location_matches_its_folder_whatever_the_case_or_trailing_slash() {
        assert!(same_path(
            r"C:\Users\a\AppData\Local\Programs\RenameIt\",
            r"c:\users\a\appdata\local\programs\renameit"
        ));
        assert!(!same_path(
            r"C:\Program Files\RenameIt",
            r"D:\Tools\RenameIt"
        ));
        assert!(!same_path("", r"C:\"), "an empty record is no record");
    }

    /// FIPS 180-2's first example, through CNG. Windows only, so CI's
    /// `windows-latest` job is the check.
    #[cfg(windows)]
    #[test]
    fn the_digest_of_abc_is_the_published_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("abc");
        std::fs::write(&file, b"abc").unwrap();
        let digest: String = imp::sha256_file(&file)
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
