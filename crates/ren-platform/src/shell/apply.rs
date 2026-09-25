//! Writing the plan to the registry — **D7**, widened by the preset menu.
//!
//! Right-clicking files in Windows Explorer offers a RenameIt menu.
//!
//! Everything under `HKEY_CURRENT_USER\Software\Classes`, which is the whole
//! design: **no administrator**, nothing an installer has to own, and
//! uninstalling is deleting the keys we wrote. A per-machine entry under
//! `HKLM` needs elevation to add *and* to remove, and strands itself on an
//! account that can no longer clean it up.
//!
//! There is almost nothing to read here, and that is the point: [`plan`] has
//! already decided every key, name and value, and this file is a loop over two
//! enum arms plus the three `Reg*` calls they need. Everything that used to be
//! judgement in this file — which classes, what the command says, what is
//! quoted — moved to the module that CI can run.
//!
//! [`plan`]: super::plan

use std::path::Path;

use super::plan::{MENU_VERSION, MenuPreset, ShellPlan, Step, VERSION_VALUE, Value};
use crate::{PlatformError, Result};

/// Where a plan is written, under `HKEY_CURRENT_USER`.
///
/// Empty in production, a scratch subtree in tests. It is a parameter because
/// the version of this file before the preset menu **toggled the user's real
/// Explorer menu from a unit test** — harmless while there was one such test,
/// and a race the moment there were two, because cargo runs a crate's tests in
/// parallel. The precedent is `417b1e2`, where the counter lock had to cover
/// every test that decodes.
type Under<'a> = &'a str;

/// Production: the plan's own key paths, unprefixed.
const LIVE: Under<'static> = "";

/// Whether the current menu layout is installed.
///
/// **Not "does the verb key exist"**: a D132 install has a key at exactly
/// `…\shell\RenameIt` too, because that is where its single *"Open with
/// &RenameIt"* item lived. The marker is what tells the two apart, and it is
/// written last on each root, so a plan interrupted half way reports false and
/// gets repaired rather than reporting true and staying broken.
pub fn is_registered() -> bool {
    installed_under(LIVE)
}

fn installed_under(under: Under<'_>) -> bool {
    ShellPlan::verb_roots()
        .iter()
        .all(|root| reg::dword(&format!("{under}{root}"), VERSION_VALUE) == Some(MENU_VERSION))
}

/// Writes the whole menu, pointing at `exe`, with one item per preset.
pub fn register(exe: &Path, presets: &[MenuPreset<'_>]) -> Result<()> {
    apply(&ShellPlan::build(exe, presets), LIVE)
}

/// Removes it. Not an error if it is already gone — a user who cleaned up by
/// hand should not be told they failed.
pub fn unregister() -> Result<()> {
    apply(&ShellPlan::uninstall(), LIVE)
}

/// The whole of the Windows-specific behaviour.
fn apply(plan: &ShellPlan, under: Under<'_>) -> Result<()> {
    for step in &plan.steps {
        match step {
            Step::DeleteTree { key } => reg::delete_tree(&format!("{under}{key}"))?,
            Step::Set { key, name, value } => {
                let key = format!("{under}{key}");
                match value {
                    Value::Sz(text) => reg::set_string(&key, name, text)?,
                    Value::Dword(n) => reg::set_dword(&key, name, *n)?,
                }
            }
        }
    }
    Ok(())
}

mod reg {
    use super::{PlatformError, Result};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
        RRF_RT_REG_DWORD, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW,
        RegSetValueExW,
    };
    #[cfg(test)]
    use windows_sys::Win32::System::Registry::{KEY_READ, RegOpenKeyExW};

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn failed(path: &str, code: u32) -> PlatformError {
        PlatformError::io(
            Path::new(path),
            std::io::Error::from_raw_os_error(code as i32),
        )
    }

    /// Only the tests ask this. Production asks the version marker instead,
    /// because a key that exists is the exact thing a D132 install also has.
    #[cfg(test)]
    pub fn key_exists(path: &str) -> bool {
        let wide = wide(path);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `key` is only
        // read when the call reports success.
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide.as_ptr(), 0, KEY_READ, &mut key) };
        if status == ERROR_SUCCESS {
            // SAFETY: opened above and not used again.
            unsafe { RegCloseKey(key) };
            return true;
        }
        false
    }

    /// One `REG_DWORD`, or `None` for absent, of another type, or unreadable.
    ///
    /// The three are one answer on purpose: every one of them means "not the
    /// menu this build writes", and the caller's next move — write it again —
    /// is the same for all three.
    pub fn dword(path: &str, name: &str) -> Option<u32> {
        let path_w = wide(path);
        let name_w = wide(name);
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: both buffers are NUL-terminated and outlive the call;
        // `RRF_RT_REG_DWORD` makes the API itself reject any other type, so the
        // 4 bytes it writes into `value` are a DWORD or it writes nothing.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                path_w.as_ptr(),
                name_w.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&raw mut value).cast(),
                &mut size,
            )
        };
        (status == ERROR_SUCCESS).then_some(value)
    }

    /// Creates `path` if needed and sets one `REG_SZ` value. An empty `name` is
    /// the key's default value, which is where a shell verb's command lives.
    pub fn set_string(path: &str, name: &str, value: &str) -> Result<()> {
        let value_w = wide(value);
        // The length is in *bytes* and includes the terminating NUL, which is
        // what `REG_SZ` means by a string.
        let bytes = std::mem::size_of_val(value_w.as_slice()) as u32;
        set(path, name, REG_SZ, value_w.as_ptr().cast(), bytes)
    }

    /// The same, for the separator flag and the version marker.
    pub fn set_dword(path: &str, name: &str, value: u32) -> Result<()> {
        set(
            path,
            name,
            REG_DWORD,
            (&raw const value).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    }

    /// # Safety
    ///
    /// `data` must point at `bytes` readable bytes in the layout `kind`
    /// describes. Private, and both callers derive `bytes` from the value they
    /// pass.
    fn set(path: &str, name: &str, kind: u32, data: *const u8, bytes: u32) -> Result<()> {
        let path_w = wide(path);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: every pointer is to a NUL-terminated buffer that outlives the
        // call; the two nulls are the documented "no class, no disposition".
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path_w.as_ptr(),
                0,
                std::ptr::null_mut(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(failed(path, status));
        }

        let name_w = wide(name);
        // SAFETY: `key` is open, the buffers outlive the call, and `bytes` is
        // the true length of what `data` points at — see this function's own
        // safety note.
        let status = unsafe { RegSetValueExW(key, name_w.as_ptr(), 0, kind, data, bytes) };
        // SAFETY: opened above, not used after this.
        unsafe { RegCloseKey(key) };

        if status != ERROR_SUCCESS {
            return Err(failed(path, status));
        }
        Ok(())
    }

    /// Deletes a key and everything under it. Already-absent is success.
    pub fn delete_tree(path: &str) -> Result<()> {
        let wide = wide(path);
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide.as_ptr()) };
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        Err(failed(path, status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Where every test but one writes.
    ///
    /// A subtree of our own under `HKCU`, so a test run cannot disturb the
    /// developer's Explorer menu and two tests cannot disturb each other. The
    /// plan's keys are appended to it whole, which makes the scratch paths long
    /// and unmistakable — `…\RenameItTests\<case>\Software\Classes\…` is
    /// nobody's real key.
    const SCRATCH: &str = r"Software\RenameItTests\";

    /// The one test that touches the live keys takes this.
    ///
    /// Cargo runs a crate's tests in parallel, so without it a second live test
    /// would race the first — and the failure mode is not a red test, it is the
    /// developer's context menu left in whichever state lost.
    static LIVE_KEYS: Mutex<()> = Mutex::new(());

    fn scratch(case: &str) -> String {
        format!("{SCRATCH}{case}\\")
    }

    fn presets() -> [MenuPreset<'static>; 2] {
        [
            MenuPreset {
                name: "Rock & Roll",
                file: Path::new(r"C:\presets\Rock & Roll.toml"),
            },
            MenuPreset {
                name: "50% off",
                file: Path::new(r"C:\presets\50% off.toml"),
            },
        ]
    }

    /// Cleans up whether the body panicked or not, so one failing assertion
    /// cannot leave a scratch subtree behind for the next run to inherit.
    fn in_scratch(case: &str, body: impl FnOnce(&str)) {
        let under = scratch(case);
        let _ = reg::delete_tree(&format!("{SCRATCH}{case}"));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&under)));
        let _ = reg::delete_tree(&format!("{SCRATCH}{case}"));
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    /// Every step, applied and then read back. The plan's own tests say what
    /// the layout should be; this says the registry now holds it.
    #[test]
    fn every_value_the_plan_holds_reaches_the_registry() {
        in_scratch("roundtrip", |under| {
            let plan = ShellPlan::build(Path::new(r"C:\Apps\renameit.exe"), &presets());
            apply(&plan, under).expect("HKCU needs no elevation");

            for step in &plan.steps {
                let Step::Set { key, name, value } = step else {
                    continue;
                };
                let key = format!("{under}{key}");
                match value {
                    Value::Sz(_) => assert!(reg::key_exists(&key), "{key}"),
                    Value::Dword(n) => {
                        assert_eq!(reg::dword(&key, name), Some(*n), "{key}\\{name}")
                    }
                }
            }
        });
    }

    /// A key name with `&`, `%` and spaces in it is written and found again —
    /// the characters `plan` deliberately does *not* sanitise, on the grounds
    /// that a registry key name may hold them.
    #[test]
    fn a_preset_name_with_punctuation_survives_the_write() {
        in_scratch("punctuation", |under| {
            apply(
                &ShellPlan::build(Path::new(r"C:\Apps\renameit.exe"), &presets()),
                under,
            )
            .unwrap();
            for name in ["100_Rock & Roll", "101_50% off"] {
                assert!(
                    reg::key_exists(&format!(
                        r"{under}Software\Classes\RenameIt.ContextMenu\shell\{name}\command"
                    )),
                    "{name}"
                );
            }
        });
    }

    /// Installing twice must leave one menu, not two — which is what the
    /// deletes at the head of every plan are for. A preset that was removed
    /// between the two runs must be gone from the menu, because nothing else
    /// would ever remove its key: the registry has no "delete what I did not
    /// write this time".
    #[test]
    fn a_second_install_removes_the_items_the_first_one_left() {
        in_scratch("rewrite", |under| {
            let exe = Path::new(r"C:\Apps\renameit.exe");
            apply(&ShellPlan::build(exe, &presets()), under).unwrap();
            let gone = format!(r"{under}Software\Classes\RenameIt.ContextMenu\shell\101_50% off");
            assert!(reg::key_exists(&gone));

            apply(&ShellPlan::build(exe, &presets()[..1]), under).unwrap();
            assert!(!reg::key_exists(&gone), "a stale item nothing would remove");
        });
    }

    /// The upgrade path. A D132 install has a `(Default)` and a `\command`
    /// directly under `…\shell\RenameIt`, and a cascade parent carrying either
    /// renders as a plain non-cascading item — so a menu written on top of one
    /// without deleting it first would look installed and behave like the old
    /// version.
    #[test]
    fn a_d132_single_verb_is_replaced_rather_than_written_around() {
        in_scratch("upgrade", |under| {
            let root = format!(r"{under}Software\Classes\AllFilesystemObjects\shell\RenameIt");
            reg::set_string(&root, "", "Open with &RenameIt").unwrap();
            reg::set_string(&format!(r"{root}\command"), "", r#""C:\old.exe" "%1""#).unwrap();
            assert!(!installed_under(under), "the old shape is not this menu");

            apply(
                &ShellPlan::build(Path::new(r"C:\Apps\renameit.exe"), &presets()),
                under,
            )
            .unwrap();

            assert!(installed_under(under));
            assert!(
                !reg::key_exists(&format!(r"{root}\command")),
                "the old command key collapses the cascade into a plain item"
            );
        });
    }

    /// Absent, wrong-typed and stale-version all answer the same way, because
    /// the caller's next move is the same for all three: write it again.
    #[test]
    fn the_version_marker_is_what_answers_is_it_installed() {
        in_scratch("marker", |under| {
            let plan = ShellPlan::build(Path::new(r"C:\Apps\renameit.exe"), &presets());
            apply(&plan, under).unwrap();
            assert!(installed_under(under));

            let root = format!(r"{under}Software\Classes\Drive\shell\RenameIt");
            reg::set_dword(&root, VERSION_VALUE, MENU_VERSION + 1).unwrap();
            assert!(!installed_under(under), "a layout we did not write");

            reg::set_string(&root, VERSION_VALUE, "1").unwrap();
            assert!(!installed_under(under), "the right number, the wrong type");
        });
    }

    #[test]
    fn removing_the_menu_takes_the_containers_with_it() {
        in_scratch("uninstall", |under| {
            apply(
                &ShellPlan::build(Path::new(r"C:\Apps\renameit.exe"), &presets()),
                under,
            )
            .unwrap();
            apply(&ShellPlan::uninstall(), under).unwrap();

            assert!(!installed_under(under));
            for key in ShellPlan::owned_keys() {
                assert!(!reg::key_exists(&format!("{under}{key}")), "{key}");
            }
            // Twice is not an error.
            apply(&ShellPlan::uninstall(), under).unwrap();
        });
    }

    /// The real keys, once, under a lock.
    ///
    /// It has to exist: every other test here proves the writer works on a
    /// subtree, and none of them proves the paths it writes in earnest are
    /// reachable. `Software\Classes` is not an ordinary key — it is a merged
    /// view of `HKCU` and `HKLM` — and a scratch copy of it is an ordinary key
    /// that would not have noticed.
    ///
    /// **Skipped on a machine that has the menu installed.** Putting it back
    /// afterwards would mean registering this test binary and these fake
    /// presets in its place, which is a broken menu until the app next starts.
    /// A machine without it is left without it, even if an assertion fails
    /// half-way.
    #[test]
    fn the_menu_registers_and_unregisters_under_the_current_user() {
        let _lock = LIVE_KEYS.lock().unwrap_or_else(|e| e.into_inner());

        if is_registered() {
            eprintln!(
                "skipped: the RenameIt menu is installed here, and this test would replace it"
            );
            return;
        }
        let exe = std::env::current_exe().unwrap();

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            register(&exe, &presets()).expect("HKCU needs no elevation");
            assert!(is_registered());

            unregister().expect("removing our own keys");
            assert!(!is_registered());
            unregister().expect("already gone is success");
        }));
        let _ = unregister();
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }
}
