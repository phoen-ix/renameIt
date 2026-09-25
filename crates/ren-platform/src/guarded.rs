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
//!
//! **Lexical, after `.` and `..` are resolved.** The comparison is on the text
//! of the path, never on what the disk says, so `/home/../etc` is folded to
//! `/etc` first — otherwise a `..` walks straight past a prefix match. A
//! symbolic link into a guarded folder is not followed: the guard answers for
//! the path it was given, which is the path the journal will record.
//!
//! [`first_guarded`] is the one check both front ends make before a run, so
//! the GUI's Rename button and `ren-cli apply` refuse the same folders.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use crate::{NamingRules, Platform};

/// The Windows list. `%SystemRoot%` and the two program-files roots, plus the
/// machine-wide `ProgramData`, taken from the environment where there is one
/// so a machine with Windows on `D:` is covered too.
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
    cell.get_or_init(|| roots().iter().map(|root| fold_text(root, rules)).collect())
}

/// Case folded by `rules`, `/` as the separator, no trailing separator — so
/// `C:\Windows\` and `c:/windows` are one string, and the root `/` is `""`.
fn fold_text(text: &str, rules: &NamingRules) -> String {
    rules
        .fold(text)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned()
}

/// `.` dropped and `..` applied to the text, without asking the disk.
///
/// `..` at the root stays at the root, which is how every OS reads it. A
/// relative path keeps a `..` it has nothing to apply to; a front end makes
/// its paths absolute before asking.
fn lexically_normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True if `folded` is a guarded root or lives under one.
fn under_a_root(folded: &str, roots: &[String]) -> bool {
    roots.iter().any(|root| {
        // Equal, or a *path* prefix — `/usrlocal` must not match `/usr`, which
        // a bare `starts_with` would let through.
        folded == root
            || (folded.len() > root.len()
                && folded.starts_with(root.as_str())
                && folded.as_bytes()[root.len()] == b'/')
    })
}

/// True if a guarded root sits directly in `folded` — `/` holds `/usr`,
/// `C:\` holds `C:\Windows`.
fn holds_a_root(folded: &str, roots: &[String]) -> bool {
    roots.iter().any(|root| {
        root.rsplit_once('/')
            .is_some_and(|(parent, _)| parent == folded)
    })
}

/// True if `path` is a guarded root or lives under one.
pub fn is_system_folder(path: &Path, rules: &NamingRules) -> bool {
    let normal = lexically_normal(path);
    under_a_root(
        &fold_text(&normal.to_string_lossy(), rules),
        folded_roots(rules),
    )
}

/// The first folder a run over `paths` would touch that the OS needs left
/// alone, or `None`.
///
/// `dir` is the folder being browsed, when there is one: it is checked even
/// when `paths` is empty, so an empty `C:\Windows` is refused before anybody
/// adds an operation and wonders why nothing happened.
///
/// Every *distinct parent* is checked, so the usual case — one folder, ten
/// thousand files — is one check. And an entry that **is** a guarded root is
/// caught too: `/` with folders included lists `/usr` itself, whose parent is
/// nothing special. That second check only runs for entries whose parent
/// directly holds a root, so it costs nothing in an ordinary folder.
///
/// The answer is the folder as [`is_system_folder`] saw it, `..` resolved, so
/// the reason a front end prints names the real place.
pub fn first_guarded<'a>(
    platform: &dyn Platform,
    dir: Option<&Path>,
    paths: impl IntoIterator<Item = &'a Path>,
) -> Option<PathBuf> {
    if let Some(dir) = dir {
        let dir = lexically_normal(dir);
        if platform.is_system_folder(&dir) {
            return Some(dir);
        }
    }

    #[derive(Clone, Copy)]
    enum Parent {
        Guarded,
        HoldsARoot,
        Clear,
    }

    let mut seen: BTreeMap<&Path, Parent> = BTreeMap::new();
    for path in paths {
        let Some(parent) = path.parent() else {
            continue;
        };
        let verdict = *seen.entry(parent).or_insert_with(|| {
            let normal = lexically_normal(parent);
            let rules = platform.naming_rules(&normal);
            if platform.is_system_folder(&normal) {
                Parent::Guarded
            } else if holds_a_root(
                &fold_text(&normal.to_string_lossy(), rules),
                folded_roots(rules),
            ) {
                Parent::HoldsARoot
            } else {
                Parent::Clear
            }
        });
        match verdict {
            Parent::Guarded => return Some(lexically_normal(parent)),
            Parent::HoldsARoot => {
                let normal = lexically_normal(path);
                if platform.is_system_folder(&normal) {
                    return Some(normal);
                }
            }
            Parent::Clear => {}
        }
    }
    None
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

    /// `std::path::absolute` keeps `..` on Unix, so `ren-cli apply ../../etc`
    /// from a home folder arrives as `/home/mk/../../etc` — a path that
    /// matches no prefix until the `..` are applied.
    #[test]
    #[cfg(unix)]
    fn a_parent_step_does_not_walk_past_the_guard() {
        assert!(is_system_folder(Path::new("/home/mk/../../etc"), RULES));
        assert!(is_system_folder(Path::new("/home/./../usr/lib"), RULES));
        assert!(
            is_system_folder(Path::new("/../../etc"), RULES),
            "above / is /"
        );
        assert!(!is_system_folder(Path::new("/etc/../home/mk"), RULES));
    }

    #[test]
    fn lexical_normalisation_applies_dots_without_the_disk() {
        assert_eq!(
            lexically_normal(Path::new("/a/./b/../c")),
            Path::new("/a/c")
        );
        assert_eq!(lexically_normal(Path::new("/..")), Path::new("/"));
        assert_eq!(lexically_normal(Path::new("../a")), Path::new("../a"));
        assert_eq!(lexically_normal(Path::new("a/../../b")), Path::new("../b"));
    }

    /// The shared check both front ends call: the browsed folder, each
    /// distinct parent, and an entry that is itself a guarded root.
    #[test]
    #[cfg(unix)]
    fn the_first_guarded_folder_is_found_wherever_it_hides() {
        let platform = crate::host();
        let platform = platform.as_ref();
        let none: [&Path; 0] = [];

        assert_eq!(
            first_guarded(platform, Some(Path::new("/etc")), none),
            Some(PathBuf::from("/etc")),
            "an empty guarded folder is refused before anything is listed"
        );
        assert_eq!(
            first_guarded(platform, Some(Path::new("/home/mk/../../usr")), none),
            Some(PathBuf::from("/usr")),
            "and the answer names the real folder"
        );
        assert_eq!(
            first_guarded(
                platform,
                None,
                [Path::new("/home/mk/a.txt"), Path::new("/etc/hosts")]
            ),
            Some(PathBuf::from("/etc")),
            "a Free Select list reaching into one"
        );
        assert_eq!(
            first_guarded(
                platform,
                Some(Path::new("/")),
                [Path::new("/home"), Path::new("/usr")]
            ),
            Some(PathBuf::from("/usr")),
            "listing / with folders offers /usr itself for renaming"
        );
        assert_eq!(
            first_guarded(
                platform,
                Some(Path::new("/home/mk")),
                [Path::new("/home/mk/usr"), Path::new("/home/mk/etc/x")]
            ),
            None,
            "a folder *named* like a root is somebody's own"
        );
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

    #[test]
    #[cfg(windows)]
    fn a_parent_step_does_not_walk_past_the_guard_on_windows() {
        assert!(is_system_folder(
            Path::new(r"C:\Users\mk\..\..\Windows"),
            RULES
        ));
    }
}
