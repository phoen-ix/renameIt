//! Folders the operating system needs left alone.
//!
//! System locations — the Windows folder, the program folders and their
//! equivalents — that a batch rename must not be turned loose on.
//!
//! Hiding them would be the quiet answer. We **refuse** them, and say so — see
//! D127. Undo
//! is this project's answer to almost every mistake, and this is the one case
//! where it is not: a rename inside `C:\Windows` succeeds, the journal records
//! it exactly, and the machine stops booting before anybody presses Ctrl+Z.
//!
//! Prefix matching on a folded path, not a set of exact paths: `C:\Windows` is
//! guarded and so is everything under it, because the danger is the contents
//! rather than the folder.

use std::path::Path;
use std::sync::OnceLock;

use crate::NamingRules;

/// The Windows list. `%SystemRoot%` and the two program-files roots, plus the
/// per-user store, taken from the environment where there is one so a machine
/// with Windows on `D:` is covered too.
#[cfg(windows)]
fn roots() -> Vec<String> {
    let mut out = Vec::new();
    for key in [
        "SystemRoot",
        "windir",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ] {
        if let Ok(value) = std::env::var(key)
            && !value.is_empty()
        {
            out.push(value);
        }
    }
    // The conventional locations, always — not only when *every* variable is
    // missing. A process started with a stripped environment that still had
    // `ProgramData` in it used to guard that alone and leave `C:\Windows`
    // open, which is the one folder D127 exists for. A machine with Windows
    // on `D:` is covered by the variables above; this covers the machine
    // that has lost them.
    out.push(r"C:\Windows".to_owned());
    out.push(r"C:\Program Files".to_owned());
    out
}

/// The Unix list. Deliberately not `/home` or `/Users`: this guards the
/// operating system, not the user's own files, and a tool that refused to
/// rename anything under `/home` would be refusing its entire purpose.
#[cfg(not(windows))]
fn roots() -> Vec<String> {
    [
        "/bin",
        "/boot",
        "/dev",
        "/etc",
        "/lib",
        "/lib32",
        "/lib64",
        "/proc",
        "/sbin",
        "/sys",
        "/usr",
        "/var",
        // macOS.
        "/System",
        "/Library",
        "/Applications",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// The roots, folded by `rules` and with their separators normalised, built
/// once per folding rule.
///
/// `is_system_folder` is asked once per distinct parent per listing, and it
/// used to rebuild and fold the whole list on every call — fifteen
/// allocations, or five environment lookups, per folder. Two cells because
/// there are two ways to fold: a table folds case or it does not, and that
/// is the only part of `NamingRules` the answer depends on.
fn folded_roots(rules: &NamingRules) -> &'static [String] {
    static INSENSITIVE: OnceLock<Vec<String>> = OnceLock::new();
    static SENSITIVE: OnceLock<Vec<String>> = OnceLock::new();
    let cell = if rules.case_insensitive {
        &INSENSITIVE
    } else {
        &SENSITIVE
    };
    cell.get_or_init(|| {
        roots()
            .iter()
            .map(|root| {
                rules
                    .fold(root)
                    .replace('\\', "/")
                    .trim_end_matches('/')
                    .to_owned()
            })
            .collect()
    })
}

/// True if `path` is a guarded root or lives under one.
pub fn is_system_folder(path: &Path, rules: &NamingRules) -> bool {
    let folded = rules.fold(&path.to_string_lossy());
    let folded = folded.replace('\\', "/");
    let folded = folded.trim_end_matches('/');

    folded_roots(rules).iter().any(|root| {
        // Equal, or a *path* prefix — `/usrlocal` must not match `/usr`, which
        // a bare `starts_with` would let through.
        folded == root
            || (folded.len() > root.len()
                && folded.starts_with(root.as_str())
                && folded.as_bytes()[root.len()] == b'/')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    const RULES: &NamingRules = &crate::POSIX;
    #[cfg(windows)]
    const RULES: &NamingRules = &crate::WINDOWS;

    #[test]
    #[cfg(unix)]
    fn the_operating_system_is_guarded_and_the_users_own_files_are_not() {
        assert!(is_system_folder(Path::new("/usr"), RULES));
        assert!(is_system_folder(Path::new("/usr/share/doc"), RULES));
        assert!(is_system_folder(Path::new("/etc/"), RULES));

        // The whole point of the tool.
        assert!(!is_system_folder(Path::new("/home/mk/photos"), RULES));
        assert!(!is_system_folder(Path::new("/tmp/scratch"), RULES));
    }

    /// A bare `starts_with` would guard `/usrlocal`, which is somebody's
    /// perfectly ordinary folder.
    #[test]
    #[cfg(unix)]
    fn a_prefix_must_end_at_a_separator() {
        assert!(!is_system_folder(Path::new("/usrlocal"), RULES));
        assert!(!is_system_folder(Path::new("/etcetera/notes"), RULES));
        assert!(is_system_folder(Path::new("/usr/local"), RULES));
    }

    /// Windows folds case, so `c:\windows` and `C:\WINDOWS` are one answer.
    #[test]
    #[cfg(windows)]
    fn the_windows_folder_is_guarded_however_it_is_spelled() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
        assert!(is_system_folder(Path::new(&root), RULES));
        assert!(is_system_folder(Path::new(&root.to_lowercase()), RULES));
        assert!(is_system_folder(
            &Path::new(&root).join("System32").join("drivers"),
            RULES
        ));
    }

    #[test]
    #[cfg(windows)]
    fn an_ordinary_folder_is_not_guarded() {
        assert!(!is_system_folder(Path::new(r"D:\Photos\2009"), RULES));
        assert!(!is_system_folder(
            Path::new(r"C:\Users\mk\Documents"),
            RULES
        ));
    }
}
