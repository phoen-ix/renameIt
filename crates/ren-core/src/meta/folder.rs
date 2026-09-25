//! Peeking inside a folder for the file that speaks for it.
//!
//! The music and image tags can 'peek' inside a folder: the first music or
//! image file inside it supplies the information, and the folder is named from
//! that.
//!
//! Shared by both readers because they want exactly the same rule and would
//! otherwise disagree about what "first" means — which is the sort of
//! difference nobody notices until two tags on one folder contradict each other.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use super::cache::{MetaCache, Stamp};

/// The first direct child with one of `extensions` that `read` can make
/// something of.
///
/// Two decisions worth being explicit about:
///
/// * **First by name, not by directory order.** `read_dir` yields whatever the
///   filesystem feels like, which differs between ext4 and NTFS and between two
///   runs on the same machine. A folder has to rename to the same thing twice.
/// * **The first that *yields* something**, not the first that matches the
///   extension. A cover image with no Exif date, or a 0-byte `.mp3`, must not
///   veto the real file sitting next to it.
///
/// The extension list is a cheap filter, not a correctness claim: it is what
/// stops a folder of ten thousand text files being opened one by one. The
/// reader still decides whether what it opened is really what it wanted.
pub(crate) fn first_inside<T>(
    dir: &Path,
    extensions: &[&str],
    mut read: impl FnMut(&Path) -> Option<T>,
) -> Option<T> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        // A subfolder named `Live.mp3` is not a track.
        .filter(|entry| entry.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| has_extension(path, extensions))
        .collect();
    candidates.sort();
    candidates.iter().find_map(|path| read(path))
}

/// Case-insensitively, because `PHOTO.JPG` is the same kind of file as
/// `photo.jpg` and a camera will happily produce either.
pub(crate) fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        let lower = e.to_ascii_lowercase();
        extensions.contains(&lower.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, body: &[u8]) {
        std::fs::write(dir.path().join(name), body).unwrap();
    }

    /// Not `read_dir` order: two runs on two filesystems have to agree.
    #[test]
    fn the_first_file_is_the_first_by_name() {
        let dir = TempDir::new().unwrap();
        for name in ["zebra.mp3", "apple.mp3", "middle.mp3"] {
            write(&dir, name, b"x");
        }
        let found = first_inside(dir.path(), &["mp3"], |p| {
            Some(p.file_name()?.to_string_lossy().into_owned())
        });
        assert_eq!(found.as_deref(), Some("apple.mp3"));
    }

    /// A file that matches the extension but says nothing must not veto the one
    /// beside it — a cover image with no Exif, a zero-byte track.
    #[test]
    fn a_file_that_yields_nothing_does_not_veto_the_next_one() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.mp3", b"");
        write(&dir, "b.mp3", b"real");
        let found = first_inside(dir.path(), &["mp3"], |p| {
            let body = std::fs::read(p).ok()?;
            (!body.is_empty()).then(|| String::from_utf8_lossy(&body).into_owned())
        });
        assert_eq!(found.as_deref(), Some("real"));
    }

    #[test]
    fn other_kinds_of_file_are_not_opened_at_all() {
        let dir = TempDir::new().unwrap();
        write(&dir, "notes.txt", b"x");
        write(&dir, "song.mp3", b"x");
        let mut opened = Vec::new();
        let found = first_inside(dir.path(), &["mp3"], |p| {
            opened.push(p.file_name().unwrap().to_string_lossy().into_owned());
            Some(())
        });
        assert!(found.is_some());
        assert_eq!(opened, ["song.mp3"], "the text file was never opened");
    }

    #[test]
    fn an_extension_matches_whatever_case_it_is_written_in() {
        let dir = TempDir::new().unwrap();
        write(&dir, "PHOTO.JPG", b"x");
        assert!(first_inside(dir.path(), &["jpg"], |_| Some(())).is_some());
        assert!(has_extension(Path::new("/a/b.JpEg"), &["jpeg"]));
        assert!(!has_extension(Path::new("/a/b"), &["jpeg"]));
    }

    /// `<FirstFileInFolder>` on a file row asks about the file's own folder,
    /// so a flat listing asks the same folder once per row per keystroke. It
    /// is cached on the folder's mtime like the other folder answers (P44):
    /// with the mtime put back after a new file appears, the old answer
    /// stands, and once the mtime moves the new one does (D130).
    ///
    /// Unix only for the fixture's sake: putting a folder's mtime back needs a
    /// handle with write-attribute access, which `File::open` on Windows does
    /// not ask for.
    #[cfg(unix)]
    #[test]
    fn the_first_file_is_read_once_per_folder_mtime() {
        let dir = TempDir::new().unwrap();
        write(&dir, "b.txt", b"x");
        forget_all();
        assert_eq!(first_file(dir.path(), true).as_deref(), Some("b.txt"));

        let folder = std::fs::File::open(dir.path()).unwrap();
        let mtime = folder.metadata().unwrap().modified().unwrap();
        write(&dir, "a.txt", b"x");
        folder.set_modified(mtime).unwrap();
        assert_eq!(
            first_file(dir.path(), true).as_deref(),
            Some("b.txt"),
            "the folder was read again at the same mtime"
        );
        assert_eq!(first_file(dir.path(), false).as_deref(), Some("b"));

        folder
            .set_modified(mtime + std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(first_file(dir.path(), true).as_deref(), Some("a.txt"));
    }

    #[test]
    fn an_empty_or_unreadable_folder_says_nothing() {
        let dir = TempDir::new().unwrap();
        assert_eq!(first_inside(dir.path(), &["mp3"], |_| Some(())), None);
        assert_eq!(
            first_inside(Path::new("/nowhere/at/all"), &["mp3"], |_| Some(())),
            None
        );
    }
}

// --- Folder contents, for the `<Dir…>` tags (M8) ------------------------------

/// What a folder holds — the Folders tag group: `<DirSize>` (the size of the
/// files in it), `<DirFiles>` (how many files) and `<DirDirs>` (how many
/// folders), and the `S`-prefixed trio, which are the same three counted
/// through subfolders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirStats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
}

/// Counts what is in `dir`, optionally through its subfolders.
///
/// Cached on the folder's own mtime, like `meta::exif::folder_date` and with
/// the same known limit: a directory's mtime changes when an entry is added or
/// removed, **not** when a file inside it grows. So `<DirSize>` can be stale
/// after an edit that changed no names. The alternative is re-walking a tree
/// per file per keystroke, which is the cost P44 exists to refuse; F9 relists
/// and clears it.
pub fn stats(dir: &Path, recursive: bool) -> Option<DirStats> {
    stats_at(dir, Stamp::stat(dir)?, recursive)
}

/// [`stats`] for a listed entry, without the `stat` (see `meta::cache`). A
/// `<Dir…>` tag on a *file* row describes the folder the file is in (D130),
/// and that folder is not listed, so a file row still costs one `stat`.
pub fn stats_of_entry(entry: &crate::model::FileEntry, recursive: bool) -> Option<DirStats> {
    if entry.is_dir {
        stats_at(&entry.path, Stamp::of_entry(entry), recursive)
    } else {
        stats(entry.parent(), recursive)
    }
}

fn stats_at(dir: &Path, stamp: Stamp, recursive: bool) -> Option<DirStats> {
    if !stamp.is_dir {
        return None;
    }
    let cache = if recursive { &DEEP } else { &SHALLOW };
    cache
        .get_or_init(Default::default)
        .get_or_read(dir, stamp, || walk(dir, recursive))
}

fn walk(dir: &Path, recursive: bool) -> Option<DirStats> {
    let mut out = DirStats::default();
    let walker = walkdir::WalkDir::new(dir)
        .min_depth(1)
        .max_depth(if recursive { usize::MAX } else { 1 });

    for entry in walker {
        // A subfolder the account cannot read costs its own rows and no others,
        // the same rule the listing follows (P63). A count that refused to
        // answer because of one locked folder would be less useful than one
        // that is honest about covering what it could reach.
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_dir() {
            out.dirs += 1;
        } else {
            out.files += 1;
            out.bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Some(out)
}

/// `<FirstFileInFolder>`: the name of the first file in `dir`.
///
/// First **by name**, for the reason `first_inside` gives: `read_dir` order is
/// whatever the filesystem feels like, and a folder has to rename to the same
/// thing twice. Files only — a subfolder is not a file — and `with_extension`
/// chooses between the tag's two spellings.
///
/// Cached on the folder's own stamp, like [`stats`] and with the same known
/// limit (D130). On a file row `dir` is the file's own folder, so a flat
/// listing asks one folder once per row per keystroke — ten thousand
/// `read_dir`s of ten thousand names each, uncached.
pub fn first_file(dir: &Path, with_extension: bool) -> Option<String> {
    let stamp = Stamp::stat(dir)?;
    if !stamp.is_dir {
        return None;
    }
    let first = FIRST
        .get_or_init(Default::default)
        .get_or_read(dir, stamp, || first_name(dir))?;
    if with_extension {
        return Some(first.to_string());
    }
    Some(crate::split_file_name(&first).0.to_owned())
}

fn first_name(dir: &Path) -> Option<Arc<str>> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .min()
        .map(Arc::from)
}

/// One cache per depth, both shared through [`super::cache::MetaCache`]: the
/// shallow and the recursive count of one folder are two answers, and a key
/// that carried the flag would need a map keyed on something other than the
/// path.
static SHALLOW: OnceLock<MetaCache<Option<DirStats>>> = OnceLock::new();
static DEEP: OnceLock<MetaCache<Option<DirStats>>> = OnceLock::new();

/// Each folder's first file name, which both `<FirstFileInFolder>` spellings
/// derive from.
static FIRST: OnceLock<MetaCache<Option<Arc<str>>>> = OnceLock::new();

/// Drops every cached count and name: F9's full refresh (D140), and tests,
/// where two tempdirs can reuse a path.
pub fn forget_all() {
    for cache in [&SHALLOW, &DEEP] {
        if let Some(cache) = cache.get() {
            cache.clear();
        }
    }
    if let Some(cache) = FIRST.get() {
        cache.clear();
    }
}
