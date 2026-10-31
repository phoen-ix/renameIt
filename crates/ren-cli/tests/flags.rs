//! Wave 2: the command-line flags no other suite runs through the binary —
//! `recover` in both forms, `--scope both`, the modern listing flags (the
//! legacy suite reaches them only through `/d` and `/f`) and `--seed`.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_ren-cli");

fn tree(names: &[&str]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in names {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }
    dir
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
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

/// A run that stopped between renaming a file and journalling it: `recover`
/// names it and leaves it, `recover --rollback` puts it back, and a second
/// `recover` finds nothing left to do.
#[test]
fn recover_reports_an_unfinished_run_and_rollback_puts_it_back() {
    use ren_core::exec::{Journal, Record};

    let dir = tree(&["b.txt"]);
    let journal = TempDir::new().unwrap();
    let mut live = Journal::create(journal.path()).unwrap();
    live.write(Record::Begin {
        platform: "test".to_owned(),
        items: 1,
    })
    .unwrap();
    live.write(Record::PlanRename {
        seq: 0,
        from: dir.path().join("a.txt"),
        to: dir.path().join("b.txt"),
    })
    .unwrap();
    drop(live);

    let report = run(journal.path(), &["recover"]);
    assert_eq!(report.status.code(), Some(0));
    let out = stdout(&report);
    assert!(out.contains("1 in flight"), "{out}");
    assert!(out.contains("re-run with --rollback"), "{out}");
    assert_eq!(names(dir.path()), ["b.txt"], "a report changes nothing");

    let rollback = run(journal.path(), &["recover", "--rollback"]);
    let out = stdout(&rollback);
    assert_eq!(rollback.status.code(), Some(0), "{out}");
    assert!(out.contains("rolled back 1 change"), "{out}");
    assert_eq!(names(dir.path()), ["a.txt"]);

    let again = run(journal.path(), &["recover"]);
    assert!(
        stdout(&again).contains("no unfinished transactions"),
        "{}",
        stdout(&again)
    );
}

#[test]
fn the_both_scope_reaches_the_extension_too() {
    let dir = tree(&["a.txt"]);
    let journal = TempDir::new().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    let applied = run(
        journal.path(),
        &["apply", &path, "--suffix", "_v2", "--scope", "both"],
    );
    assert_eq!(applied.status.code(), Some(0), "{}", stdout(&applied));
    assert_eq!(names(dir.path()), ["a.txt_v2"]);
}

/// `--folders` adds the folders, `--no-files` takes the files away.
#[test]
fn the_listing_flags_choose_files_and_folders() {
    let dir = tree(&["file.txt", "sub/inner.txt"]);
    let journal = TempDir::new().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    let both = stdout(&run(journal.path(), &["preview", &path, "--folders"]));
    assert!(both.contains("file_renamed.txt"), "{both}");
    assert!(both.contains("sub_renamed"), "{both}");

    let folders = stdout(&run(
        journal.path(),
        &["preview", &path, "--folders", "--no-files"],
    ));
    assert!(!folders.contains("file_renamed.txt"), "{folders}");
    assert!(folders.contains("sub_renamed"), "{folders}");
}

/// The same `--seed` gives the same random names, and another seed others.
#[test]
fn a_seed_makes_random_tags_reproducible() {
    let dir = tree(&["a.txt", "b.txt", "c.txt"]);
    let journal = TempDir::new().unwrap();
    let presets = TempDir::new().unwrap();
    let preset = presets.path().join("random.toml");
    std::fs::write(
        &preset,
        "[preset]\nname = \"Random\"\n\n[[step]]\nop = \"add_remove\"\ninsert = \"<Rnd6>-\"\n",
    )
    .unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    let preset = preset.to_string_lossy().into_owned();
    let preview = |seed: &str| {
        stdout(&run(
            journal.path(),
            &["preview", &path, "--preset", &preset, "--seed", seed],
        ))
    };

    let first = preview("42");
    assert!(first.contains("-a.txt"), "{first}");
    assert!(!first.contains("<Rnd6>"), "the tag is expanded: {first}");
    assert_eq!(first, preview("42"));
    assert_ne!(first, preview("43"));
}

/// The legacy `/s` reaches into subfolders; the compat suite only checks the
/// translation, and the help text names it.
#[test]
fn the_legacy_subfolder_switch_lists_nested_files() {
    let dir = tree(&["top.txt", "sub/inner.txt"]);
    let journal = TempDir::new().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    let flat = stdout(&run(journal.path(), &["/p", &path]));
    assert!(flat.contains("top_renamed.txt"), "{flat}");
    assert!(!flat.contains("inner_renamed.txt"), "{flat}");

    let deep = stdout(&run(journal.path(), &["/p", &path, "/s"]));
    assert!(deep.contains("inner_renamed.txt"), "{deep}");
}
