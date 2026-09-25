//! The data the engine renames, and how a filename is sliced.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// One listed file or folder. Immutable for the lifetime of a listing snapshot.
///
/// Timestamps are captured with the rest of the metadata in a single `stat`,
/// so the file table can sort by date without a second pass over the disk.
/// `None` means the filesystem did not report that time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: PathBuf,
    /// The name as text — **lossy**, and [`Self::name_is_lossy`] says when.
    ///
    /// A filename is not text. On Unix it is bytes, on Windows it is UTF-16
    /// that may contain unpaired surrogates, and neither is obliged to be
    /// valid Unicode. Every operation, template and regex in the engine works
    /// on `&str`, so the name has to become one somewhere — and where that
    /// conversion is lossy this field holds U+FFFD where the disk holds
    /// something else.
    ///
    /// [`Self::path`] is always byte-exact, so the file can still be found,
    /// renamed and undone. What must not happen is a *new* name being derived
    /// from this one: that would write the replacement character to disk and
    /// lose what was there. `plan` refuses it — see [`Self::name_is_lossy`].
    pub file_name: String,
    /// Whether [`Self::file_name`] lost information.
    ///
    /// **D50 already made this argument one layer up**, for bytes read out of
    /// a CSV: *"`String::from_utf8_lossy` is the wrong fallback here: it would
    /// turn `Björk` into `Bj<?>rk` and rename a file to it."* The same
    /// reasoning was never applied to the `OsStr` the listing starts from, and
    /// the result was a run that mangled the name and then aborted the whole
    /// batch on the journal write.
    #[serde(default)]
    pub name_is_lossy: bool,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
}

/// A name as text, and whether saying so lost anything.
///
/// One place, so the two constructors cannot disagree about it.
pub(crate) fn name_of(raw: &std::ffi::OsStr) -> (String, bool) {
    match raw.to_str() {
        Some(text) => (text.to_owned(), false),
        None => (raw.to_string_lossy().into_owned(), true),
    }
}

impl FileEntry {
    /// An entry read from disk, with one `stat`.
    ///
    /// `symlink_metadata`, not `metadata`: the row describes the directory
    /// entry as it is, the way the Files/Folders chips (walkdir's own file
    /// type) and every other probe in the engine already do. Following the
    /// link listed a symlink to a folder under the Files chip and then called
    /// it a folder, and dropped a dangling one as unreadable (P63's failure)
    /// when it is a real, renameable entry.
    pub fn from_path(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let metadata = std::fs::symlink_metadata(&path)?;
        Self::from_metadata(path, &metadata)
    }

    /// An entry from a `stat` somebody else already took — the listing walk,
    /// whose `walkdir` record is free on Windows and one syscall on Unix. A
    /// second `stat` per entry was the whole cost of listing a folder on the
    /// platform that ships first.
    pub fn from_metadata(
        path: impl Into<PathBuf>,
        metadata: &std::fs::Metadata,
    ) -> std::io::Result<Self> {
        let path = path.into();
        let raw = path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} has no file name", path.display()),
            )
        })?;
        let (file_name, name_is_lossy) = name_of(raw);
        Ok(Self {
            path,
            file_name,
            name_is_lossy,
            is_dir: metadata.is_dir(),
            size: if metadata.is_dir() { 0 } else { metadata.len() },
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            accessed: metadata.accessed().ok(),
        })
    }

    /// An entry that is not backed by disk — for tests, benchmarks and the
    /// synthetic listings the performance harness generates.
    ///
    /// Its stamp is empty (size 0, no mtime), and the metadata readers key on
    /// the entry's stamp rather than on a fresh `stat` (see `meta::cache`) —
    /// so a synthetic entry over a *real* file reads as an empty file with no
    /// tags. A test that wants the file read builds the entry with
    /// [`Self::from_path`].
    pub fn synthetic(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (file_name, name_is_lossy) = path.file_name().map(name_of).unwrap_or_default();
        Self {
            path,
            file_name,
            name_is_lossy,
            is_dir: false,
            size: 0,
            modified: None,
            created: None,
            accessed: None,
        }
    }

    pub fn parent(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new(""))
    }

    /// Stem and extension: a file's split at the **last** period, a folder's
    /// not at all. See [`split_name`].
    pub fn split(&self) -> (&str, Option<&str>) {
        split_name(&self.file_name, self.is_dir)
    }
}

/// Splits a listed name into stem and extension — a file's at its last period
/// ([`split_file_name`]), a folder's not at all.
///
/// **A folder has no extension.** `Vol. 2`, `Mr. Robot Season 1`,
/// `2024.01.15 Trip` and `regex-1.13.1` are names, and a file manager renames
/// them as one: sliced like a file, every card scoped to the name reached only
/// the text before the last period, so Replace `.` → `_` turned
/// `regex-1.13.1` into `regex-1_13.1`. The name is the whole name, the
/// extension is absent — so [`Scope::Extension`] leaves a folder alone the
/// way it leaves `README` alone (P12), and `<Name>` / `<Ext>` and the include
/// filter's stem agree. A macOS bundle (`Photos.app`) is a folder too, so its
/// `.app` is part of the name — one rule for every folder is worth that.
pub fn split_name(name: &str, is_dir: bool) -> (&str, Option<&str>) {
    if is_dir {
        (name, None)
    } else {
        split_file_name(name)
    }
}

/// Splits a file name into stem and extension at the last period.
///
/// For a listed row use [`split_name`], which knows that a folder has none.
///
/// The rule is literal — the extension is everything after the last period —
/// with two exceptions that Unix names make common:
///
/// * A leading period (`.gitignore`) is treated as part of the *name*, not as
///   an empty stem with a `gitignore` extension. This is the sane reading (P1).
/// * `.` and `..` never have an extension.
pub fn split_file_name(file_name: &str) -> (&str, Option<&str>) {
    if file_name == "." || file_name == ".." {
        return (file_name, None);
    }
    match file_name.rfind('.') {
        Some(0) | None => (file_name, None),
        Some(i) => (&file_name[..i], Some(&file_name[i + 1..])),
    }
}

/// A file name plus the byte range of the part an operation may rewrite.
///
/// Scoping nests: [`Scope`] carves the extension away, then the pre-processor
/// narrows further. Holding the whole name plus a range (rather than three
/// separate `&str`s) is what makes narrowing free — the prefix and suffix grow
/// by moving the boundaries, with no allocation and no way to lose the context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject<'a> {
    whole: &'a str,
    range: Range<usize>,
}

impl<'a> Subject<'a> {
    /// The whole name is in scope.
    pub fn whole(name: &'a str) -> Self {
        Self {
            whole: name,
            range: 0..name.len(),
        }
    }

    /// Panics unless `range` lies on character boundaries of `name`.
    pub fn new(name: &'a str, range: Range<usize>) -> Self {
        debug_assert!(name.is_char_boundary(range.start) && name.is_char_boundary(range.end));
        Self { whole: name, range }
    }

    pub fn prefix(&self) -> &'a str {
        &self.whole[..self.range.start]
    }

    /// Where the active slice sits in the whole name, in bytes.
    ///
    /// The narrowing is already done by the time anyone asks, so this is the
    /// answer to "which part of this name did the engine hand the operation" —
    /// which is what Visual Assist shows and what `Pipeline::subject_at`
    /// returns.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// The part the operation sees, and the only part it may change.
    pub fn active(&self) -> &'a str {
        &self.whole[self.range.clone()]
    }

    pub fn suffix(&self) -> &'a str {
        &self.whole[self.range.end..]
    }

    /// Narrows to a sub-range of the *active* slice, given in bytes relative to
    /// its start. Everything trimmed away joins the prefix or the suffix.
    pub fn narrow(&self, inner: Range<usize>) -> Subject<'a> {
        let start = self.range.start + inner.start;
        let end = self.range.start + inner.end;
        debug_assert!(end <= self.range.end);
        Subject::new(self.whole, start..end)
    }

    /// Rebuilds the full name with `replacement` in place of the active slice.
    pub fn reassemble(&self, replacement: &str) -> String {
        let prefix = self.prefix();
        let suffix = self.suffix();
        let mut out = String::with_capacity(prefix.len() + replacement.len() + suffix.len());
        out.push_str(prefix);
        out.push_str(replacement);
        out.push_str(suffix);
        out
    }
}

/// Which part of the file name an operation is allowed to see.
///
/// Slicing is the *engine's* job, not each operation's — every operation then
/// becomes a trivially testable string transform (`docs/DESIGN.md` Part 1 §2).
/// **D19:** this applies uniformly to every operation, including Casing and
/// Space Trimming.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Process Name: the stem only.
    #[default]
    Name,
    /// Process Extension: the text after the last period only.
    Extension,
    /// Both boxes ticked: the whole file name, period included.
    Both,
}

impl Scope {
    /// Carves out the part of `name` this scope covers.
    ///
    /// `None` means this scope does not apply to this row at all — asking for
    /// the extension of `README` must leave `README` alone, not append to it.
    /// `is_dir` because a folder has no extension ([`split_name`]); the flag
    /// is a parameter rather than a default so every caller has to say which
    /// kind of row it is slicing.
    pub fn slice(self, name: &str, is_dir: bool) -> Option<Subject<'_>> {
        let (stem, ext) = split_name(name, is_dir);
        match self {
            Scope::Both => Some(Subject::whole(name)),
            Scope::Name => Some(Subject::new(name, 0..stem.len())),
            // The prefix keeps the separating period.
            Scope::Extension => ext.map(|e| Subject::new(name, name.len() - e.len()..name.len())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_is_everything_after_the_last_period() {
        assert_eq!(split_file_name("song.mp3"), ("song", Some("mp3")));
        assert_eq!(
            split_file_name("archive.tar.gz"),
            ("archive.tar", Some("gz"))
        );
        assert_eq!(split_file_name("README"), ("README", None));
    }

    #[test]
    fn a_leading_period_belongs_to_the_name() {
        assert_eq!(split_file_name(".gitignore"), (".gitignore", None));
        assert_eq!(split_file_name("."), (".", None));
        assert_eq!(split_file_name(".."), ("..", None));
        assert_eq!(split_file_name(".config.toml"), (".config", Some("toml")));
    }

    #[test]
    fn a_trailing_period_yields_an_empty_extension() {
        assert_eq!(split_file_name("weird."), ("weird", Some("")));
    }

    /// `(prefix, active, suffix)` — the shape the tests care about.
    fn parts<'a>(subject: &Subject<'a>) -> (&'a str, &'a str, &'a str) {
        (subject.prefix(), subject.active(), subject.suffix())
    }

    /// A file, as opposed to a folder — the second argument of `slice`.
    const FILE: bool = false;
    const FOLDER: bool = true;

    #[test]
    fn scope_name_hides_the_extension_from_the_operation() {
        let s = Scope::Name.slice("song.mp3", FILE).unwrap();
        assert_eq!(parts(&s), ("", "song", ".mp3"));
        assert_eq!(s.reassemble("SONG"), "SONG.mp3");
    }

    #[test]
    fn scope_name_covers_the_whole_name_when_there_is_no_extension() {
        let s = Scope::Name.slice("README", FILE).unwrap();
        assert_eq!(parts(&s), ("", "README", ""));
    }

    #[test]
    fn scope_extension_hides_the_stem_from_the_operation() {
        let s = Scope::Extension.slice("song.mp3", FILE).unwrap();
        assert_eq!(parts(&s), ("song.", "mp3", ""));
        assert_eq!(s.reassemble("MP3"), "song.MP3");
    }

    #[test]
    fn scope_extension_does_not_apply_when_there_is_no_extension() {
        assert!(Scope::Extension.slice("README", FILE).is_none());
    }

    #[test]
    fn scope_both_shows_the_whole_name() {
        let s = Scope::Both.slice("song.mp3", FILE).unwrap();
        assert_eq!(parts(&s), ("", "song.mp3", ""));
        assert_eq!(s.reassemble("a.b"), "a.b");
    }

    /// A folder's name is all name: the period in `Vol. 2` is not an
    /// extension separator.
    #[test]
    fn a_folder_has_no_extension() {
        assert_eq!(split_name("Vol. 2", FOLDER), ("Vol. 2", None));
        assert_eq!(split_name("Vol. 2", FILE), ("Vol", Some(" 2")));
        let s = Scope::Name.slice("regex-1.13.1", FOLDER).unwrap();
        assert_eq!(parts(&s), ("", "regex-1.13.1", ""));
        assert!(Scope::Extension.slice("Vol. 2", FOLDER).is_none());
        assert_eq!(
            parts(&Scope::Both.slice("Vol. 2", FOLDER).unwrap()),
            ("", "Vol. 2", "")
        );

        let mut entry = FileEntry::synthetic("/music/Vol. 2");
        assert_eq!(entry.split(), ("Vol", Some(" 2")));
        entry.is_dir = true;
        assert_eq!(entry.split(), ("Vol. 2", None));
    }

    #[test]
    fn narrowing_moves_the_trimmed_text_into_prefix_and_suffix() {
        let s = Scope::Name.slice("abcdef.txt", FILE).unwrap();
        assert_eq!(parts(&s), ("", "abcdef", ".txt"));

        let inner = s.narrow(2..4);
        assert_eq!(parts(&inner), ("ab", "cd", "ef.txt"));
        assert_eq!(inner.reassemble("XY"), "abXYef.txt");
    }

    #[test]
    fn narrowing_composes() {
        let s = Subject::whole("0123456789");
        let once = s.narrow(2..8);
        let twice = once.narrow(1..3);
        assert_eq!(parts(&twice), ("012", "34", "56789"));
        assert_eq!(twice.reassemble("--"), "012--56789");
    }

    #[test]
    fn narrowing_to_nothing_is_an_insertion_point() {
        let s = Subject::whole("abc");
        let empty = s.narrow(3..3);
        assert_eq!(parts(&empty), ("abc", "", ""));
        assert_eq!(empty.reassemble("!"), "abc!");
    }
}
