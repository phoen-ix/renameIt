//! Filenames that are not valid Unicode.
//!
//! A filename is not text. On Unix it is bytes; on Windows it is UTF-16 that
//! may contain unpaired surrogates. Neither is obliged to be valid Unicode,
//! and both turn up in the wild — a file copied off an old drive, written by a
//! program with a different locale, or arriving over Samba.
//!
//! Before this suite existed, such a file did two things in order: the listing
//! silently replaced the offending byte with U+FFFD, and then the run
//! **panicked** on the journal write, because `serde`'s `impl Serialize for
//! Path` refuses a non-UTF-8 path and the call site said
//! `.expect("journal records are always serialisable")`. In the GUI that took
//! the window down mid-batch.
//!
//! Nothing here could be reached by the property suite: its generator builds
//! names as Rust `String`s, so an invalid name is unreachable by construction.
#![cfg(unix)]

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use ren_core::ops::{AddRemove, AddRemoveMode, OpKind};
use ren_core::{
    ApplyOptions, ListOptions, Pipeline, Plan, PlanItem, PlannedOp, RenameKind, RowState, Scope,
    StepConfig, apply, list, plan, undo_last,
};
use tempfile::TempDir;

/// `café.txt` with the `é` as a single Latin-1 byte — not valid UTF-8.
fn latin1_name() -> &'static OsStr {
    OsStr::from_bytes(b"caf\xE9.txt")
}

fn suffix_pipeline() -> Pipeline {
    let mut pipeline = Pipeline::new();
    pipeline.push(
        OpKind::AddRemove(AddRemove {
            mode: AddRemoveMode::Add,
            insert: "_v2".into(),
            add_pos: 0,
            add_backwards: true,
            ..Default::default()
        })
        .to_step(),
        StepConfig {
            scope: Scope::Name,
            ..Default::default()
        },
    );
    pipeline
}

fn names_in(dir: &Path) -> Vec<PathBuf> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| PathBuf::from(e.unwrap().file_name()))
        .collect();
    names.sort();
    names
}

/// The row is listed, is left alone, and says so — it is not mangled and it
/// does not take the run down.
#[test]
fn a_name_that_is_not_utf8_is_left_alone_rather_than_mangled() {
    let dir = TempDir::new().unwrap();
    let journal = TempDir::new().unwrap();
    std::fs::write(dir.path().join(latin1_name()), b"payload").unwrap();
    std::fs::write(dir.path().join("ordinary.txt"), b"payload").unwrap();

    let platform = ren_platform::host();
    let entries = list(dir.path(), ListOptions::default()).unwrap();
    assert_eq!(entries.len(), 2, "the odd file is listed, not dropped");

    let odd = entries
        .iter()
        .find(|e| e.path.file_name() == Some(latin1_name()))
        .expect("listed");
    assert!(
        odd.name_is_lossy,
        "the listing has to admit it lost something"
    );
    assert_eq!(
        odd.path.file_name(),
        Some(latin1_name()),
        "the path stays byte-exact even though the name could not"
    );

    let p = plan(&entries, &suffix_pipeline(), platform.as_ref());
    let odd_item = p
        .items
        .iter()
        .find(|i| i.source.file_name() == Some(latin1_name()))
        .unwrap();
    assert_eq!(odd_item.state, RowState::Unchanged, "left alone");
    assert_eq!(odd_item.target, odd_item.source, "and not retargeted");

    // P63: said out loud rather than silently skipped.
    assert!(
        p.notes.iter().any(|n| n.contains("not valid Unicode")),
        "{:?}",
        p.notes
    );

    // And the *other* file still renames — one odd entry costs its own row and
    // no others.
    assert!(p.is_executable(), "{:?}", p.items);
    let report = apply(
        &p,
        platform.as_ref(),
        &ApplyOptions {
            journal_dir: journal.path().to_path_buf(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.is_success());

    assert_eq!(
        names_in(dir.path()),
        vec![
            PathBuf::from(latin1_name()),
            PathBuf::from("ordinary_v2.txt")
        ],
        "the odd name is byte-for-byte what it was"
    );
}

/// The repair path: the file *can* be renamed, by giving it a name directly —
/// which is what the GUI's F2 does, building its own one-item plan.
///
/// This is the whole point of leaving the row listed instead of dropping it.
/// And it has to survive undo, which is what forced the journal to learn a
/// lossless path encoding.
#[test]
fn such_a_file_can_be_renamed_by_hand_and_undone_exactly() {
    let dir = TempDir::new().unwrap();
    let journal = TempDir::new().unwrap();
    let source = dir.path().join(latin1_name());
    std::fs::write(&source, b"payload").unwrap();

    let platform = ren_platform::host();
    let target = dir.path().join("cafe.txt");
    let hand_written = Plan {
        items: vec![PlanItem {
            index: 0,
            source: source.clone(),
            new_name: "cafe.txt".to_owned(),
            target: target.clone(),
            state: RowState::Changed,
            actions: Vec::new(),
        }],
        ops: vec![PlannedOp::Rename {
            from: source.clone(),
            to: target.clone(),
            kind: RenameKind::Direct,
        }],
        notes: Vec::new(),
        blockers: Vec::new(),
    };

    let report = apply(
        &hand_written,
        platform.as_ref(),
        &ApplyOptions {
            journal_dir: journal.path().to_path_buf(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(names_in(dir.path()), vec![PathBuf::from("cafe.txt")]);

    let undo = undo_last(platform.as_ref(), journal.path()).unwrap();
    assert!(undo.is_complete(), "skipped: {:?}", undo.skipped);

    // The bytes, not the lossy string. Comparing `to_string_lossy` would pass
    // even if the byte had been replaced, which is exactly the bug that shipped.
    assert_eq!(
        names_in(dir.path())
            .iter()
            .map(|n| n.as_os_str().as_bytes().to_vec())
            .collect::<Vec<_>>(),
        vec![latin1_name().as_bytes().to_vec()],
        "undo has to put the original bytes back, not an approximation"
    );
}

/// A perfectly ordinary name inside a folder whose *own* name is not UTF-8.
///
/// This is the case that used to poison a whole directory: `entry.path` is
/// non-UTF-8 for every file under it, so every rename in the folder aborted on
/// the journal write even though every filename rendered perfectly.
#[test]
fn a_folder_whose_name_is_not_utf8_does_not_poison_the_files_in_it() {
    let dir = TempDir::new().unwrap();
    let journal = TempDir::new().unwrap();
    let odd_folder = dir.path().join(OsStr::from_bytes(b"h\xE9liday"));
    std::fs::create_dir(&odd_folder).unwrap();
    std::fs::write(odd_folder.join("photo.jpg"), b"payload").unwrap();

    let platform = ren_platform::host();
    let entries = list(
        &odd_folder,
        ListOptions {
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].name_is_lossy, "the file's own name is fine");

    let p = plan(&entries, &suffix_pipeline(), platform.as_ref());
    assert!(p.is_executable(), "{:?}", p.items);

    let report = apply(
        &p,
        platform.as_ref(),
        &ApplyOptions {
            journal_dir: journal.path().to_path_buf(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(names_in(&odd_folder), vec![PathBuf::from("photo_v2.jpg")]);

    // And undo still works, which needs the folder's own odd bytes in the
    // journal.
    let undo = undo_last(platform.as_ref(), journal.path()).unwrap();
    assert!(undo.is_complete(), "skipped: {:?}", undo.skipped);
    assert_eq!(names_in(&odd_folder), vec![PathBuf::from("photo.jpg")]);
}
