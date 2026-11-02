//! Building the file list the engine works on.
//!
//! Browser mode: a base path, the Files / Folders / Subfolders chips, and the
//! pattern box. Free Select — an arbitrary set of
//! paths from anywhere — is just a `Vec<FileEntry>` the caller assembles, so it
//! needs nothing here.

use std::path::Path;

use walkdir::WalkDir;

use crate::matcher::MatchSpec;
use crate::model::FileEntry;

/// The Files / Folders / Subfolders chips plus the pattern box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOptions {
    pub files: bool,
    pub folders: bool,
    pub subfolders: bool,
    /// The pattern box — `*.mp3`, with `*.*` meaning everything.
    ///
    /// A **mask** over the whole file name, not a search, which is the same
    /// reading P19 settled for wildcards in the include filter. `None` lists
    /// everything.
    pub pattern: Option<MatchSpec>,
    /// Hidden even when `pattern` lets it through — the Filter box's
    /// *Exclude* (P121).
    pub exclude: Option<MatchSpec>,
    /// Test the full path as well as the name, for both masks.
    pub filter_whole_path: bool,
    pub filter_case: bool,
    /// Whether write-protected, hidden and system entries are listed.
    ///
    /// All three default to **true**. See D126: a renamer that silently
    /// omits rows is the failure this project has already fixed once (P63), and
    /// the switches are here so the user can narrow it deliberately.
    pub hidden: bool,
    pub system: bool,
    pub read_only: bool,
    /// Which entries the pattern box filters when both files and folders are
    /// listed.
    pub pattern_applies: PatternScope,
}

/// Which kinds of entry the pattern box filters.
///
/// Only matters when the listing holds both files and folders: then the mask
/// can apply to files only, to folders only, or to both.
///
/// `Both` is the default, because it is what the box appears to do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PatternScope {
    #[default]
    Both,
    Files,
    Folders,
}

impl PatternScope {
    fn applies_to(self, is_dir: bool) -> bool {
        match self {
            Self::Both => true,
            Self::Files => !is_dir,
            Self::Folders => is_dir,
        }
    }
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            files: true,
            folders: false,
            subfolders: false,
            pattern: None,
            exclude: None,
            filter_whole_path: false,
            filter_case: false,
            hidden: true,
            system: true,
            read_only: true,
            pattern_applies: PatternScope::default(),
        }
    }
}

impl ListOptions {
    /// Whether the three visibility switches let this entry through.
    ///
    /// An entry whose metadata could not be read is **shown**: it is a real row
    /// in the folder, and hiding it because a `stat` failed would be the
    /// silent-subset failure P63 exists to prevent.
    fn shows(&self, path: &Path, metadata: &std::fs::Metadata) -> bool {
        if self.hidden && self.system && self.read_only {
            return true; // The default; nothing to ask.
        }
        let seen = ren_platform::visibility(path, metadata);
        (self.hidden || !seen.hidden)
            && (self.system || !seen.system)
            && (self.read_only || !seen.read_only)
    }

    /// Whether the walk goes into this folder at all.
    ///
    /// A hidden or system folder that is not shown is not walked either: its
    /// contents are hidden with it. The files inside are not hidden
    /// themselves — `.git/HEAD` has no dot of its own, and Windows does not
    /// inherit the attribute — so dropping only the folder's row listed
    /// everything behind it. Read-only does not prune: a read-only folder's
    /// children are not read-only. A folder whose metadata cannot be read is
    /// walked, for the reason [`Self::shows`] shows such an entry.
    fn enters(&self, entry: &walkdir::DirEntry) -> bool {
        if (self.hidden && self.system) || !entry.file_type().is_dir() {
            return true;
        }
        let Ok(metadata) = entry.metadata() else {
            return true;
        };
        let seen = ren_platform::visibility(entry.path(), &metadata);
        (self.hidden || !seen.hidden) && (self.system || !seen.system)
    }

    /// Interprets what the user typed in the pattern box. Empty, `*` and `*.*`
    /// all mean "everything", so they compile to no filter at all.
    pub fn with_pattern(mut self, text: &str) -> Self {
        let trimmed = text.trim();
        self.pattern = match trimmed {
            "" | "*" | "*.*" => None,
            other => Some(MatchSpec::auto(other)),
        };
        self
    }

    /// The Filter box read as a regular expression; empty lists everything.
    pub fn with_regex_pattern(mut self, text: &str) -> Self {
        self.pattern = (!text.trim().is_empty()).then(|| MatchSpec::Regex(text.to_owned()));
        self
    }

    /// Hides what this matches; empty hides nothing (P121).
    pub fn excluding(mut self, text: &str, regex: bool) -> Self {
        let trimmed = text.trim();
        self.exclude = (!trimmed.is_empty()).then(|| {
            if regex {
                MatchSpec::Regex(text.to_owned())
            } else {
                MatchSpec::auto(trimmed)
            }
        });
        self
    }

    pub fn testing_whole_path(mut self, yes: bool) -> Self {
        self.filter_whole_path = yes;
        self
    }

    pub fn matching_case(mut self, yes: bool) -> Self {
        self.filter_case = yes;
        self
    }
}

/// One entry the walk could not use, and why.
///
/// Reported rather than fatal (P63). A folder the account cannot read, or a file
/// that vanished between the walk and the `stat`, is a fact about that entry —
/// not a reason the other four hundred should be invisible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListProblem {
    pub path: std::path::PathBuf,
    pub error: String,
}

impl std::fmt::Display for ListProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.error)
    }
}

/// Lists `root`, sorted by path so previews and plans are deterministic.
///
/// Anything unreadable inside the tree is skipped and dropped; use
/// [`list_reporting`] where the caller can say so.
pub fn list(root: &Path, options: ListOptions) -> std::io::Result<Vec<FileEntry>> {
    list_reporting(root, options).map(|(entries, _)| entries)
}

/// The same walk, with what it could not read handed back.
///
/// **The root itself still hard-errors.** A base path that does not exist or
/// cannot be opened is the user's own question answered wrongly, and returning
/// an empty list for it would look like an empty folder. Everything *inside*
/// the tree is a problem rather than a failure.
pub fn list_reporting(
    root: &Path,
    options: ListOptions,
) -> std::io::Result<(Vec<FileEntry>, Vec<ListProblem>)> {
    list_reporting_with(root, options, &|| false)
}

/// The same walk, abandoned when `stop` says so.
///
/// For a caller that walks on a thread of its own and may have been asked
/// for a different folder since: the walk checks between entries and returns
/// what it has, which the caller then throws away. `Interrupted` rather than
/// a partial listing, so the two cannot be confused — a listing that stops
/// early is a listing that quietly lost rows (P63).
pub fn list_reporting_with(
    root: &Path,
    options: ListOptions,
    stop: &dyn Fn() -> bool,
) -> std::io::Result<(Vec<FileEntry>, Vec<ListProblem>)> {
    // Answered before the walk, so an unreadable root is still an error rather
    // than an empty listing with one problem beside it.
    std::fs::read_dir(root)?;

    let walker = WalkDir::new(root)
        .min_depth(1)
        .max_depth(if options.subfolders { usize::MAX } else { 1 })
        .sort_by_file_name();

    let invalid = |e: crate::regex_flavor::RegexError| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string())
    };
    let compile = |spec: &Option<MatchSpec>| {
        spec.as_ref()
            .map(|spec| spec.compile_for_filter(options.filter_case))
            .transpose()
            .map_err(invalid)
    };
    let pattern = compile(&options.pattern)?;
    let exclude = compile(&options.exclude)?;
    // The name, and with *whole path* the full path too (P121).
    let hits = |matcher: &crate::matcher::Matcher, entry: &walkdir::DirEntry| {
        let name = entry.file_name().to_string_lossy();
        Ok::<bool, std::io::Error>(
            matcher.is_match(&name).map_err(invalid)?
                || (options.filter_whole_path
                    && matcher
                        .is_match(&entry.path().to_string_lossy())
                        .map_err(invalid)?),
        )
    };

    let mut entries = Vec::new();
    let mut problems = Vec::new();
    for entry in walker
        .into_iter()
        .filter_entry(|entry| options.enters(entry))
    {
        if stop() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "the listing was abandoned for a newer one",
            ));
        }
        // A folder the account cannot descend into used to abort everything:
        // one `System Volume Information` at a drive root, and the user saw an
        // empty table and a red message. Explorer lists what it can, and so do
        // we (P63).
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                problems.push(ListProblem {
                    path: error.path().unwrap_or(root).to_path_buf(),
                    error: error.to_string(),
                });
                continue;
            }
        };
        let is_dir = entry.file_type().is_dir();
        if (is_dir && !options.folders) || (!is_dir && !options.files) {
            continue;
        }
        if options.pattern_applies.applies_to(is_dir) {
            if let Some(pattern) = &pattern
                && !hits(pattern, &entry)?
            {
                continue;
            }
            if let Some(exclude) = &exclude
                && hits(exclude, &entry)?
            {
                continue;
            }
        }
        // One `stat` per entry, after the pattern so a masked-out file costs
        // none. On Windows walkdir's record is the `FindNextFileW` data and
        // this is free; on Unix it is a `symlink_metadata`. Either way it is
        // taken once and serves both the visibility switches and the row —
        // the row used to `stat` again on its own, which on Windows was the
        // expensive open-handle call and the whole cost of the listing.
        //
        // A file that vanished between the walk and the `stat` is one row
        // missing, not a dead listing (P63).
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                problems.push(ListProblem {
                    path: entry.path().to_path_buf(),
                    error: error.to_string(),
                });
                continue;
            }
        };
        if !options.shows(entry.path(), &metadata) {
            continue;
        }
        match FileEntry::from_metadata(entry.path(), &metadata) {
            Ok(file) => entries.push(file),
            Err(error) => problems.push(ListProblem {
                path: entry.path().to_path_buf(),
                error: error.to_string(),
            }),
        }
    }
    Ok((entries, problems))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn names_of(root: &Path, options: ListOptions) -> Vec<String> {
        list(root, options)
            .unwrap()
            .into_iter()
            .map(|e| e.file_name)
            .collect()
    }

    /// A symlink is listed as the entry it is, not as what it points at: a
    /// dangling one is a real, renameable row rather than a problem, and one
    /// pointing at a folder is a file row — which is what the Files chip
    /// already admitted it as, from walkdir's own file type. Following the
    /// link made the two disagree, and `<DirFiles>` on such a row counted
    /// the target folder's contents.
    #[cfg(unix)]
    #[test]
    fn a_symlink_is_a_row_about_the_link_and_not_its_target() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("plain.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        std::fs::write(dir.path().join("folder/inside.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("nowhere", dir.path().join("dangling")).unwrap();
        std::os::unix::fs::symlink("folder", dir.path().join("to-folder")).unwrap();

        let (entries, problems) = list_reporting(dir.path(), ListOptions::default()).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        let names: Vec<&str> = entries.iter().map(|e| e.file_name.as_str()).collect();
        assert_eq!(names, ["dangling", "plain.txt", "to-folder"]);
        assert!(
            entries.iter().all(|e| !e.is_dir),
            "a link to a folder is not a folder row"
        );

        // And the same answer from the constructor Free Select uses.
        let linked = FileEntry::from_path(dir.path().join("to-folder")).unwrap();
        assert!(!linked.is_dir);
        assert!(FileEntry::from_path(dir.path().join("dangling")).is_ok());
    }

    /// The write-protected switch, and what happens when it is unticked.
    /// Cross-platform: `set_readonly` clears every write bit here (no write
    /// bit for anyone is what counts as write-protected, D126) and sets
    /// `FILE_ATTRIBUTE_READONLY` on Windows, which is exactly the pair
    /// `ren_platform::visibility` reads.
    #[test]
    fn the_write_protected_switch_takes_rows_out_of_the_listing() {
        let dir = TempDir::new().unwrap();
        for name in ["plain.txt", "locked.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let locked = dir.path().join("locked.txt");
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&locked, perms).unwrap();

        // Shown by default, deliberately (D126).
        assert_eq!(
            names_of(dir.path(), ListOptions::default()),
            ["locked.txt", "plain.txt"]
        );

        let narrowed = ListOptions {
            read_only: false,
            ..Default::default()
        };
        assert_eq!(names_of(dir.path(), narrowed), ["plain.txt"]);

        // Or the tempdir cannot be cleaned up on Windows.
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        #[expect(
            clippy::permissions_set_readonly_false,
            reason = "restoring the fixture"
        )]
        perms.set_readonly(false);
        std::fs::set_permissions(&locked, perms).unwrap();
    }

    /// A leading dot is what hidden means here; on Windows it is an attribute,
    /// which `ren_platform::visibility`'s own tests cover.
    #[test]
    #[cfg(unix)]
    fn the_hidden_switch_takes_dotfiles_out_of_the_listing() {
        let dir = TempDir::new().unwrap();
        for name in [".secret", "plain.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert_eq!(
            names_of(dir.path(), ListOptions::default()),
            [".secret", "plain.txt"]
        );
        assert_eq!(
            names_of(
                dir.path(),
                ListOptions {
                    hidden: false,
                    ..Default::default()
                }
            ),
            ["plain.txt"]
        );
    }

    /// A hidden folder's contents are hidden with it. The files inside are
    /// not hidden themselves — `.git/HEAD` has no dot of its own — so a walk
    /// that only dropped the folder's own row still listed, and renamed,
    /// every file in the repository behind it.
    #[cfg(unix)]
    #[test]
    fn with_hidden_off_a_hidden_folder_is_not_walked() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".git/refs")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), b"x").unwrap();
        std::fs::write(dir.path().join(".git/refs/main"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), b"x").unwrap();

        let deep = ListOptions {
            folders: true,
            subfolders: true,
            ..Default::default()
        };
        assert_eq!(
            names_of(dir.path(), deep.clone()),
            [".git", "HEAD", "refs", "main", "src", "main.rs"],
            "shown by default (D126)"
        );
        assert_eq!(
            names_of(
                dir.path(),
                ListOptions {
                    hidden: false,
                    ..deep
                }
            ),
            ["src", "main.rs"]
        );
    }

    /// With files and folders both listed, the mask can be told to leave
    /// folders alone.
    #[test]
    fn the_pattern_can_be_told_to_leave_folders_alone() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("holiday")).unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("b.mp3"), b"x").unwrap();

        let both = ListOptions {
            folders: true,
            ..Default::default()
        }
        .with_pattern("*.txt");
        assert_eq!(names_of(dir.path(), both), ["a.txt"], "the default");

        let files_only = ListOptions {
            folders: true,
            pattern_applies: PatternScope::Files,
            ..Default::default()
        }
        .with_pattern("*.txt");
        assert_eq!(
            names_of(dir.path(), files_only),
            ["a.txt", "holiday"],
            "the folder is no longer masked out"
        );

        let folders_only = ListOptions {
            folders: true,
            pattern_applies: PatternScope::Folders,
            ..Default::default()
        }
        .with_pattern("holiday");
        assert_eq!(
            names_of(dir.path(), folders_only),
            ["a.txt", "b.mp3", "holiday"],
            "and now the files are not"
        );
    }

    /// One unreadable folder must not empty the listing.
    ///
    /// At a Windows drive root this is guaranteed, not hypothetical:
    /// `System Volume Information` and `$RECYCLE.BIN` refuse an ordinary
    /// account, and the whole table used to come back blank because of them.
    #[test]
    #[cfg(unix)]
    fn an_unreadable_folder_costs_its_own_rows_and_no_others() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"b").unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("hidden.txt"), b"x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let options = ListOptions {
            subfolders: true,
            ..Default::default()
        };
        let (entries, problems) = list_reporting(dir.path(), options).expect("the root is fine");

        let names: Vec<&str> = entries.iter().map(|e| e.file_name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.txt"], "the readable files still list");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].path.ends_with("locked"),
            "the problem names the folder: {problems:?}"
        );

        // Restore so TempDir can clean up.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The root itself is different: a base path that cannot be opened is the
    /// user's own question answered wrongly, and an empty list would read as an
    /// empty folder.
    #[test]
    fn an_unreadable_root_is_still_an_error() {
        let missing = std::path::Path::new("/nowhere/at/all/definitely-not");
        assert!(list_reporting(missing, ListOptions::default()).is_err());
        assert!(list(missing, ListOptions::default()).is_err());
    }

    fn tree() -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("b.txt"), b"b").unwrap();
        std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("c.txt"), b"c").unwrap();
        dir
    }

    #[test]
    fn files_only_by_default_and_sorted() {
        let dir = tree();
        let names: Vec<_> = list(dir.path(), ListOptions::default())
            .unwrap()
            .into_iter()
            .map(|e| e.file_name)
            .collect();
        assert_eq!(names, ["a.txt", "b.txt"]);
    }

    #[test]
    fn folders_can_be_listed_too() {
        let dir = tree();
        let names: Vec<_> = list(
            dir.path(),
            ListOptions {
                files: false,
                folders: true,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|e| e.file_name)
        .collect();
        assert_eq!(names, ["sub"]);
    }

    #[test]
    fn subfolders_recurse() {
        let dir = tree();
        let names: Vec<_> = list(
            dir.path(),
            ListOptions {
                subfolders: true,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|e| e.file_name)
        .collect();
        assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
    }

    /// "Base path textbox + pattern textbox (e.g. `*.mp3`; `*.*` = all)"
    #[test]
    fn the_pattern_box_masks_the_listing() {
        let dir = TempDir::new().unwrap();
        for name in ["song.mp3", "song.flac", "notes.txt", "mp3 notes.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }

        let names = |pattern: &str| -> Vec<String> {
            list(dir.path(), ListOptions::default().with_pattern(pattern))
                .unwrap()
                .into_iter()
                .map(|e| e.file_name)
                .collect()
        };

        assert_eq!(names("*.mp3"), ["song.mp3"]);
        // A mask, not a search: "mp3 notes.txt" contains "mp3" but does not
        // match the pattern.
        assert_eq!(names("*.txt"), ["mp3 notes.txt", "notes.txt"]);
        assert_eq!(names("song.*"), ["song.flac", "song.mp3"]);
    }

    #[test]
    fn an_empty_or_wildcard_pattern_lists_everything() {
        let dir = tree();
        for pattern in ["", "   ", "*", "*.*"] {
            let options = ListOptions::default().with_pattern(pattern);
            assert!(options.pattern.is_none(), "{pattern:?} should not filter");
            let names: Vec<_> = list(dir.path(), options)
                .unwrap()
                .into_iter()
                .map(|e| e.file_name)
                .collect();
            assert_eq!(names, ["a.txt", "b.txt"], "pattern {pattern:?}");
        }
    }

    #[test]
    fn the_pattern_box_is_case_insensitive() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("SONG.MP3"), b"x").unwrap();
        let names: Vec<_> = list(dir.path(), ListOptions::default().with_pattern("*.mp3"))
            .unwrap()
            .into_iter()
            .map(|e| e.file_name)
            .collect();
        assert_eq!(names, ["SONG.MP3"]);
    }

    /// The Filter box and its ▾ extras (P121), over one folder.
    fn filtered(options: ListOptions) -> Vec<String> {
        let dir = TempDir::new().unwrap();
        for name in ["a.jpg", "a_old.jpg", "B.JPG", "c.png"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("old")).unwrap();
        let mut names: Vec<_> = list(dir.path(), options.clone())
            .unwrap()
            .into_iter()
            .map(|e| e.file_name)
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_filter_box_takes_an_exclude_a_regex_and_case() {
        let jpg = || ListOptions::default().with_pattern("*.jpg");
        assert_eq!(filtered(jpg()), ["B.JPG", "a.jpg", "a_old.jpg"]);
        assert_eq!(
            filtered(jpg().excluding("*_old*", false)),
            ["B.JPG", "a.jpg"]
        );
        assert_eq!(filtered(jpg().matching_case(true)), ["a.jpg", "a_old.jpg"]);
        assert_eq!(
            filtered(ListOptions::default().with_regex_pattern(r"^a.*\.jpg$")),
            ["a.jpg", "a_old.jpg"]
        );
        assert_eq!(
            filtered(ListOptions::default().excluding(r"\.png$", true)),
            ["B.JPG", "a.jpg", "a_old.jpg"]
        );
        assert_eq!(
            filtered(ListOptions::default().excluding("  ", false)).len(),
            4
        );
    }

    /// With *whole path* the folder a file sits in counts too.
    #[test]
    fn the_whole_path_reaches_the_folder_names() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("keep")).unwrap();
        std::fs::create_dir(dir.path().join("skip")).unwrap();
        std::fs::write(dir.path().join("keep/x.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("skip/y.txt"), b"x").unwrap();
        let options = ListOptions {
            subfolders: true,
            ..Default::default()
        }
        .excluding("skip", false);
        let names = |o: ListOptions| -> Vec<String> {
            list(dir.path(), o)
                .unwrap()
                .into_iter()
                .map(|e| e.file_name)
                .collect()
        };
        assert_eq!(
            names(options.clone()).len(),
            2,
            "the name alone: nothing excluded"
        );
        assert_eq!(names(options.testing_whole_path(true)), ["x.txt"]);
    }

    /// The exclude follows *the pattern applies to* like the include does.
    #[test]
    fn the_exclude_respects_what_the_filter_applies_to() {
        let options = ListOptions {
            folders: true,
            pattern_applies: PatternScope::Files,
            ..Default::default()
        }
        .excluding("*old*", false);
        assert_eq!(filtered(options), ["B.JPG", "a.jpg", "c.png", "old"]);
    }

    #[test]
    fn timestamps_are_captured_with_the_listing() {
        let dir = tree();
        let entries = list(dir.path(), ListOptions::default()).unwrap();
        assert!(
            entries.iter().all(|e| e.modified.is_some()),
            "the table sorts by date, so it needs these from the listing"
        );
    }
}
