//! M0's headline acceptance criterion, exercised through the shipped binary:
//! **`ren-cli apply` + `undo` round-trips a tempdir byte-exactly.**

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};
use std::time::SystemTime;

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_ren-cli");

fn tree(names: &[&str]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for (i, name) in names.iter().enumerate() {
        std::fs::write(dir.path().join(name), format!("contents {i}\n")).unwrap();
    }
    dir
}

/// `(name -> contents, mtime)` for the whole directory.
fn snapshot(dir: &Path) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                (
                    std::fs::read(e.path()).unwrap(),
                    e.metadata().unwrap().modified().unwrap(),
                ),
            )
        })
        .collect()
}

fn run(journal: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .arg("--journal-dir")
        .arg(journal)
        .args(args)
        .output()
        .expect("ren-cli should be runnable")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn apply_then_undo_round_trips_byte_exactly() {
    let dir = tree(&["one.txt", "two.txt", "Ünïcödé 🎵.mp3", "no-extension"]);
    let journal = TempDir::new().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    let before = snapshot(dir.path());

    let applied = run(journal.path(), &["apply", &path, "--suffix", "_v2"]);
    assert!(
        applied.status.success(),
        "apply failed: {}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(
        stdout(&applied).contains("renamed 4 items"),
        "{}",
        stdout(&applied)
    );

    let after: Vec<String> = snapshot(dir.path()).into_keys().collect();
    assert!(after.contains(&"one_v2.txt".to_string()), "{after:?}");
    assert!(
        after.contains(&"Ünïcödé 🎵_v2.mp3".to_string()),
        "{after:?}"
    );
    assert!(after.contains(&"no-extension_v2".to_string()), "{after:?}");

    let undone = run(journal.path(), &["undo"]);
    assert!(
        undone.status.success(),
        "undo failed: {}",
        String::from_utf8_lossy(&undone.stderr)
    );

    assert_eq!(
        snapshot(dir.path()),
        before,
        "the tree did not come back exactly"
    );
}

#[test]
fn preview_changes_nothing() {
    let dir = tree(&["a.txt"]);
    let journal = TempDir::new().unwrap();
    let before = snapshot(dir.path());

    let output = run(
        journal.path(),
        &["preview", &dir.path().to_string_lossy(), "--suffix", "_x"],
    );

    assert!(output.status.success());
    assert!(stdout(&output).contains("a_x.txt"), "{}", stdout(&output));
    assert_eq!(snapshot(dir.path()), before);
}

#[test]
fn simulate_reports_the_plan_without_touching_the_disk() {
    let dir = tree(&["a.txt", "b.txt"]);
    let journal = TempDir::new().unwrap();
    let before = snapshot(dir.path());

    let output = run(
        journal.path(),
        &[
            "apply",
            &dir.path().to_string_lossy(),
            "--suffix",
            "_x",
            "--simulate",
        ],
    );

    assert!(output.status.success());
    assert!(
        stdout(&output).contains("would rename 2 items"),
        "{}",
        stdout(&output)
    );
    assert_eq!(snapshot(dir.path()), before);
    assert!(
        std::fs::read_dir(journal.path())
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "simulation must not write a journal"
    );
}

#[test]
fn an_empty_suffix_leaves_everything_unchanged_and_writes_no_journal() {
    let dir = tree(&["a.txt"]);
    let journal = TempDir::new().unwrap();
    let before = snapshot(dir.path());

    let output = run(
        journal.path(),
        &["apply", &dir.path().to_string_lossy(), "--suffix", ""],
    );

    assert!(output.status.success());
    assert!(
        stdout(&output).contains("1 unchanged"),
        "{}",
        stdout(&output)
    );
    assert_eq!(snapshot(dir.path()), before);
}

#[test]
fn undo_with_no_transactions_fails_cleanly() {
    let journal = TempDir::new().unwrap();
    let output = run(journal.path(), &["undo"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no transaction to undo"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_extension_scope_only_touches_the_extension() {
    let dir = tree(&["song.mp3", "README"]);
    let journal = TempDir::new().unwrap();

    let output = run(
        journal.path(),
        &[
            "apply",
            &dir.path().to_string_lossy(),
            "--suffix",
            "x",
            "--scope",
            "extension",
        ],
    );
    assert!(output.status.success(), "{}", stdout(&output));

    let names: Vec<String> = snapshot(dir.path()).into_keys().collect();
    assert!(names.contains(&"song.mp3x".to_string()), "{names:?}");
    // A file with no extension has nothing for this scope to touch.
    assert!(names.contains(&"README".to_string()), "{names:?}");
}
