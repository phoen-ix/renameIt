//! The Script folder.
//!
//! Scripts live in one per-user directory, seeded on first use from the nine
//! we ship as our own data (D6).
//!
//! # Legacy `.frs` files are listed, not hidden
//!
//! A user may arrive with a folder of `.frs` files — VBScript for a
//! Windows-only COM host, which this build cannot run (D5). The temptation is
//! to filter them out of the listing, which would be worse: the folder would
//! look empty and the user would have no idea why.
//!
//! So they are surfaced as [`Legacy`] entries with the one thing that helps:
//! a pointer at `docs/MIGRATION-legacy-scripts.md`. Silence would be the only
//! genuinely unhelpful answer.

use super::header::Header;
use std::path::{Path, PathBuf};

/// Where scripts live for this user.
pub fn default_script_dir() -> PathBuf {
    ren_platform::app_data_dir("RenameIt").join("scripts")
}

/// One runnable script in the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEntry {
    /// The file stem — what the picker shows and what a preset stores.
    ///
    /// The stem rather than a name inside the file: a script carries no name
    /// field. Two scripts cannot share a stem, so it is also a key.
    pub name: String,
    pub header: Header,
    pub path: PathBuf,
}

/// A `.frs` file: a legacy VBScript this build cannot run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Legacy {
    pub name: String,
    pub path: PathBuf,
    /// Its own `description=` line, if it parses. Shown so the entry says what
    /// the script *was* rather than only that it is dead.
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ScriptStore {
    dir: PathBuf,
}

impl ScriptStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn user() -> Self {
        Self::new(default_script_dir())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every script in the folder, plus the `.frs` files that cannot run.
    ///
    /// A folder that does not exist yet is an empty list rather than an error —
    /// the rule `PresetStore::list` and `Journal::list` already follow, because
    /// a first run has no folder.
    ///
    /// Note what is *not* here: this does not compile anything. A script with a
    /// syntax error still has a name and a description, and it must still
    /// appear in the picker — otherwise the only way to discover the error is
    /// that the script silently is not there. Compilation happens when the
    /// operation runs, and its error lands on the row.
    pub fn list(&self) -> (Vec<ScriptEntry>, Vec<Legacy>) {
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return (Vec::new(), Vec::new());
        };

        let mut scripts = Vec::new();
        let mut legacy = Vec::new();
        for path in read.filter_map(Result::ok).map(|e| e.path()) {
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            match path.extension().and_then(|e| e.to_str()) {
                Some(ext) if ext.eq_ignore_ascii_case("koto") => {
                    // Unreadable is skipped rather than reported: the folder is
                    // scanned on every picker open, and a transient read error
                    // is not something to put in front of the user.
                    if let Ok(source) = std::fs::read_to_string(&path) {
                        scripts.push(ScriptEntry {
                            name,
                            header: Header::parse(&source),
                            path,
                        });
                    }
                }
                Some(ext) if ext.eq_ignore_ascii_case("frs") => {
                    legacy.push(Legacy {
                        description: frs_description(&path),
                        name,
                        path,
                    });
                }
                _ => {}
            }
        }
        scripts.sort_by_key(|s| sort_key(&s.name));
        legacy.sort_by_key(|l| sort_key(&l.name));
        (scripts, legacy)
    }

    /// Read one script by the name a preset stored.
    pub fn source(&self, name: &str) -> std::io::Result<String> {
        std::fs::read_to_string(self.path_of(name))
    }

    /// Where a script of this name would live.
    pub fn path_of(&self, name: &str) -> PathBuf {
        self.dir.join(name).with_extension("koto")
    }

    /// Put the shipped scripts in the folder, without touching anything.
    ///
    /// Returns how many were written. Called at startup, so a fresh install
    /// has the nine worked examples in the user's script folder.
    ///
    /// **A script that is already there is never replaced.** Losing somebody's
    /// edits on upgrade is not acceptable; a user who wants the shipped version
    /// back can delete their copy.
    pub fn seed_defaults(&self) -> std::io::Result<usize> {
        std::fs::create_dir_all(&self.dir)?;
        let mut written = 0;
        for (name, source) in DEFAULTS {
            let path = self.path_of(name);
            if !path.exists() {
                std::fs::write(&path, source)?;
                written += 1;
            }
        }
        Ok(written)
    }
}

/// The nine ports, compiled into the binary as our own data (D6).
///
/// Embedded rather than installed beside the executable so that a portable
/// build is one file.
pub const DEFAULTS: &[(&str, &str)] = &[
    (
        "Create Mp3 Playlist",
        include_str!("../../data/scripts/Create Mp3 Playlist.koto"),
    ),
    (
        "CSV List Rename",
        include_str!("../../data/scripts/CSV List Rename.koto"),
    ),
    (
        "Example - Base for New Script",
        include_str!("../../data/scripts/Example - Base for New Script.koto"),
    ),
    (
        "Get HTML XML Tags",
        include_str!("../../data/scripts/Get HTML XML Tags.koto"),
    ),
    (
        "Insert Space Before Caps",
        include_str!("../../data/scripts/Insert Space Before Caps.koto"),
    ),
    (
        "Length of Filename",
        include_str!("../../data/scripts/Length of Filename.koto"),
    ),
    (
        "Safe Characters",
        include_str!("../../data/scripts/Safe Characters.koto"),
    ),
    (
        "Swap Around",
        include_str!("../../data/scripts/Swap Around.koto"),
    ),
    (
        "Unique Random Number",
        include_str!("../../data/scripts/Unique Random Number.koto"),
    ),
];

/// Compiled scripts, keyed by the file and its stamp.
///
/// Exactly the second layer [`crate::csv_table::load`] needed and for the same
/// reason: `Cached` resets on clone (D21) — which is what makes editing a card
/// take effect — and the GUI clones the whole pipeline on every keystroke. Left
/// to `Cached` alone, every card would re-read *and re-compile* its script per
/// keystroke.
///
/// Keyed on length and mtime as well as path, so saving the script in an editor
/// picks the change up while typing in an unrelated box does not.
type Cache = std::sync::Mutex<
    std::collections::HashMap<
        CacheKey,
        std::sync::Arc<Result<super::Compiled, super::ScriptError>>,
    >,
>;
static COMPILED: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();

/// Bounded for the reason the regex and CSV caches are: a user typing into a
/// picker produces a new key per keystroke and nothing would evict them.
const COMPILED_CAPACITY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    path: PathBuf,
    len: u64,
    modified: Option<std::time::Duration>,
}

/// Read and compile `path`, or hand back the copy already compiled for it.
pub fn load(path: &Path) -> std::sync::Arc<Result<super::Compiled, super::ScriptError>> {
    let stamp = std::fs::metadata(path).ok();
    let key = CacheKey {
        path: path.to_path_buf(),
        len: stamp.as_ref().map_or(0, std::fs::Metadata::len),
        modified: stamp
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()),
    };

    let cache = COMPILED.get_or_init(Default::default);
    if let Ok(map) = cache.lock()
        && let Some(hit) = map.get(&key)
    {
        return hit.clone();
    }

    let compiled = std::sync::Arc::new(match std::fs::read_to_string(path) {
        // Named rather than passed through as an io error: "no such file" says
        // nothing about *which* script the card is pointing at.
        Err(_) => Err(super::ScriptError::Compile(format!(
            "no script named '{}' in {}",
            path.file_stem().unwrap_or_default().to_string_lossy(),
            path.parent().unwrap_or(path).display()
        ))),
        Ok(source) => super::compile(&source),
    });
    if let Ok(mut map) = cache.lock() {
        if map.len() >= COMPILED_CAPACITY {
            map.clear();
        }
        map.insert(key, compiled.clone());
    }
    compiled
}

/// Drop the compile cache. Tests only — a stale entry would otherwise leak
/// between them, since the key includes an mtime that a fast test can reuse.
pub fn forget_all() {
    if let Some(cache) = COMPILED.get()
        && let Ok(mut map) = cache.lock()
    {
        map.clear();
    }
}

fn sort_key(name: &str) -> (String, String) {
    (name.to_lowercase(), name.to_owned())
}

/// The `description=` line out of a legacy `.frs`.
///
/// Line 2 by convention, but searched for rather than indexed: the point is to
/// say something useful about a file we cannot run, and being strict about
/// where the line sits would only produce more blanks.
///
/// Read as bytes and decoded leniently on purpose: a legacy script is often
/// CP1252 rather than UTF-8, so `read_to_string` fails on it outright — which
/// would blank the description of exactly the scripts whose subject is
/// non-ASCII characters.
fn frs_description(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    String::from_utf8_lossy(&bytes)
        .lines()
        .take(8)
        .find_map(|line| {
            let (key, rest) = line.split_once('=')?;
            key.trim()
                .eq_ignore_ascii_case("description")
                .then(|| rest.trim().to_owned())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn a_folder_that_does_not_exist_is_an_empty_list() {
        let store = ScriptStore::new("/nonexistent/nowhere");
        assert_eq!(store.list().0, Vec::new());
    }

    #[test]
    fn it_lists_scripts_with_their_descriptions_in_order() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "Swap Around.koto",
            "# description: swaps\nrename = || ''",
        );
        write(
            dir.path(),
            "Insert Space.koto",
            "# description: spaces\nrename = || ''",
        );
        write(dir.path(), "notes.txt", "not a script");

        let (scripts, legacy) = ScriptStore::new(dir.path()).list();
        assert_eq!(
            scripts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["Insert Space", "Swap Around"],
            "sorted case-insensitively"
        );
        assert_eq!(scripts[0].header.description, "spaces");
        assert!(legacy.is_empty(), "a .txt is not a legacy script");
    }

    /// A script that will not compile still has to appear, or the only symptom
    /// of a typo is that the script vanishes from the picker.
    #[test]
    fn a_script_that_does_not_compile_is_still_listed() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "Broken.koto",
            "# description: broken\nrename = |||( <<",
        );
        let (scripts, _) = ScriptStore::new(dir.path()).list();
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].header.description, "broken");
    }

    #[test]
    fn legacy_frs_files_are_listed_separately_with_their_own_description() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "Swap Around.frs",
            "language=vbscript\r\n\
             description=This script provides an easy way to swap two parts.\r\n\
             Function Rename()\r\n",
        );
        let (scripts, legacy) = ScriptStore::new(dir.path()).list();
        assert!(scripts.is_empty(), "an .frs is not runnable");
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].name, "Swap Around");
        assert_eq!(
            legacy[0].description,
            "This script provides an easy way to swap two parts."
        );
    }

    /// A first run gets the nine examples, and a second run leaves them alone.
    #[test]
    fn the_shipped_scripts_are_written_once_and_never_overwritten() {
        let dir = TempDir::new().unwrap();
        let store = ScriptStore::new(dir.path());

        assert_eq!(store.seed_defaults().unwrap(), 9);
        assert_eq!(store.list().0.len(), 9);

        // Somebody edits one of them.
        let edited = store.path_of("Swap Around");
        std::fs::write(&edited, "# description: mine now\nrename = || 'x'").unwrap();

        assert_eq!(store.seed_defaults().unwrap(), 0, "nothing was missing");
        assert!(
            std::fs::read_to_string(&edited)
                .unwrap()
                .contains("mine now"),
            "an upgrade overwrote a script the user had edited"
        );
    }

    /// The embedded copies are the files in the repo, so a port cannot be
    /// edited on disk and quietly not shipped.
    #[test]
    fn the_embedded_scripts_match_the_files_on_disk() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/scripts");
        for (name, embedded) in DEFAULTS {
            let on_disk = std::fs::read_to_string(repo.join(name).with_extension("koto"))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(*embedded, on_disk, "{name} differs from the shipped file");
        }
    }

    /// A CP1252 `.frs` is not valid UTF-8. Reading it as a string fails
    /// outright, which would blank the description of exactly the scripts
    /// whose subject is non-ASCII characters.
    #[test]
    fn a_legacy_script_in_a_legacy_codepage_still_gets_a_description() {
        let dir = TempDir::new().unwrap();
        // "…for example å becomes a" with å as CP1252 0xE5, not UTF-8.
        let mut bytes = b"language=vbscript\r\ndescription=for example ".to_vec();
        bytes.push(0xE5);
        bytes.extend_from_slice(b" becomes a\r\n");
        std::fs::write(dir.path().join("Safe Characters.frs"), &bytes).unwrap();

        assert!(
            std::fs::read_to_string(dir.path().join("Safe Characters.frs")).is_err(),
            "the fixture must genuinely not be UTF-8, or this test proves nothing"
        );

        let (_, legacy) = ScriptStore::new(dir.path()).list();
        assert_eq!(legacy.len(), 1);
        assert!(
            legacy[0].description.starts_with("for example"),
            "{}",
            legacy[0].description
        );
    }
}
