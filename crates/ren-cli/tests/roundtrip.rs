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
    assert!(
        std::fs::read_dir(journal.path())
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "a plan with nothing to do must not open a journal"
    );
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

/// A relative folder is journalled as the absolute path it meant.
///
/// `./a.txt` in the journal resolved against wherever `undo` ran: from
/// another folder it found nothing, skipped everything and stamped the batch
/// undone for good — or renamed a same-named file there that the user never
/// touched.
#[test]
fn apply_from_a_relative_folder_undoes_from_anywhere() {
    let dir = tree(&["a.txt", "b.txt"]);
    let elsewhere = tree(&["a_x.txt"]);
    let journal = TempDir::new().unwrap();
    let before = snapshot(dir.path());
    let decoy = snapshot(elsewhere.path());

    let applied = Command::new(BIN)
        .current_dir(dir.path())
        .arg("--journal-dir")
        .arg(journal.path())
        .args(["apply", ".", "--suffix", "_x"])
        .output()
        .unwrap();
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );

    let undone = Command::new(BIN)
        .current_dir(elsewhere.path())
        .arg("--journal-dir")
        .arg(journal.path())
        .arg("undo")
        .output()
        .unwrap();
    assert!(
        undone.status.success(),
        "{}",
        String::from_utf8_lossy(&undone.stderr)
    );
    assert_eq!(
        snapshot(dir.path()),
        before,
        "the batch came back where it ran"
    );
    assert_eq!(
        snapshot(elsewhere.path()),
        decoy,
        "and nothing in the folder undo ran from was touched"
    );
}

/// The system-folder guard is the product's, not the GUI's: a preview of
/// `/etc` is refused with exit 2 and names the folder, however the path is
/// spelled — `..` included.
#[cfg(unix)]
#[test]
fn a_system_folder_is_refused_by_the_cli_too() {
    let journal = TempDir::new().unwrap();
    let scratch = TempDir::new().unwrap();
    let depth = scratch.path().components().count() - 1;
    let dotted = format!("{}etc", "../".repeat(depth));

    for (cwd, path) in [(Path::new("/"), "/etc"), (scratch.path(), dotted.as_str())] {
        let output = Command::new(BIN)
            .current_dir(cwd)
            .arg("--journal-dir")
            .arg(journal.path())
            .args(["preview", path, "--suffix", "_x"])
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{path}: {err}");
        assert!(
            err.contains("/etc"),
            "{path}: the reason names the folder: {err}"
        );
        assert!(err.contains("--allow-system-folders"), "{err}");
    }

    // The switch the GUI has, for the user who means it. Preview only.
    let allowed = run(
        journal.path(),
        &[
            "preview",
            "/etc",
            "--suffix",
            "_x",
            "--allow-system-folders",
        ],
    );
    assert_ne!(
        allowed.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    assert!(
        stdout(&allowed).contains("to rename"),
        "{}",
        stdout(&allowed)
    );
}

/// `std::env::args` panics on an argument that is not Unicode — exit 101,
/// outside every code the CLI promises — on exactly the names a renamer
/// exists to fix.
#[cfg(unix)]
#[test]
fn an_argument_that_is_not_unicode_is_a_path_like_any_other() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = TempDir::new().unwrap();
    let odd = dir.path().join(OsStr::from_bytes(b"caf\xe9.txt"));
    std::fs::write(&odd, b"x").unwrap();
    let journal = TempDir::new().unwrap();

    let missing = Command::new(BIN)
        .arg("preview")
        .arg(dir.path().join(OsStr::from_bytes(b"gone\xe9")))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1), "a usage error, not a panic");

    // Reaches the engine, which leaves a name it cannot build on alone and
    // says so — rather than the process dying before clap ever ran.
    let applied = Command::new(BIN)
        .arg("--journal-dir")
        .arg(journal.path())
        .args(["apply", "--suffix", "_x", "--file"])
        .arg(&odd)
        .output()
        .unwrap();
    assert_eq!(
        applied.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(
        stdout(&applied).contains("left alone"),
        "{}",
        stdout(&applied)
    );
    assert!(odd.exists());
}

/// A folder that is not there is named, not just "No such file or
/// directory" — the one line a scheduled job's log has to explain itself.
#[test]
fn a_missing_folder_is_named() {
    let journal = TempDir::new().unwrap();
    let scratch = TempDir::new().unwrap();
    let gone = scratch.path().join("typo-dir");
    let output = run(journal.path(), &["preview", &gone.to_string_lossy()]);
    assert_eq!(output.status.code(), Some(1));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("typo-dir"), "{err}");
}

/// On Linux `/d` is a folder at the root, and after a subcommand it is a
/// path — never the legacy "folders only" switch.
#[test]
fn a_modern_line_is_never_translated() {
    let journal = TempDir::new().unwrap();
    let output = run(journal.path(), &["preview", "/d"]);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(!err.contains("legacy switches translated"), "{err}");
}

/// The same file named twice is renamed once. Twice, the second rename's
/// source was already gone: a partial run and a `failed:` line for a file
/// that had in fact been renamed.
#[test]
fn a_file_named_twice_is_renamed_once() {
    let dir = tree(&["a.txt"]);
    let journal = TempDir::new().unwrap();
    let file = dir.path().join("a.txt");
    let file = file.to_string_lossy();
    let dotted = format!("{}/./a.txt", dir.path().display());

    let output = run(
        journal.path(),
        &[
            "apply", "--suffix", "_x", "--file", &file, "--file", &dotted,
        ],
    );
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{err}");
    assert!(err.contains("more than once"), "{err}");
    let names: Vec<String> = snapshot(dir.path()).into_keys().collect();
    assert_eq!(names, ["a_x.txt"]);
}

/// A name holding a control character cannot rewrite the preview a user
/// reads before applying: it is shown escaped.
#[cfg(unix)]
#[test]
fn a_control_character_in_a_name_is_shown_escaped() {
    let dir = tree(&["evil\u{1b}[2K\rinnocent.txt"]);
    let journal = TempDir::new().unwrap();
    let output = run(
        journal.path(),
        &["preview", &dir.path().to_string_lossy(), "--suffix", "_x"],
    );
    assert!(output.status.success());
    assert!(
        !output.stdout.contains(&0x1b),
        "no raw escape reaches the terminal"
    );
    assert!(!output.stdout.contains(&b'\r'), "and no carriage return");
    assert!(
        stdout(&output).contains("evil\\u{1b}[2K\\rinnocent.txt"),
        "{}",
        stdout(&output)
    );
}

/// `--help` is where a script author looks for what the exit codes mean and
/// which legacy switches exist — and it must not cite decision numbers a
/// reader cannot look up.
#[test]
fn the_help_documents_exit_codes_and_the_legacy_switches() {
    let journal = TempDir::new().unwrap();
    let help = run(journal.path(), &["--help"]);
    let text = stdout(&help);
    assert!(text.contains("Exit codes"), "{text}");
    assert!(text.contains("recover"), "{text}");
    for switch in ["/p", "/l", "/r", "/f", "/d", "/s", "/k", "/x"] {
        assert!(text.contains(switch), "{switch} missing: {text}");
    }
    for command in [&["preview", "--help"][..], &["apply", "--help"]] {
        let text = stdout(&run(journal.path(), command));
        for id in ["P16", "D101", "(P2)", "D127"] {
            assert!(!text.contains(id), "{id} leaks into {command:?}: {text}");
        }
        assert!(text.contains("--allow-system-folders"), "{text}");
    }
}

/// A journal another run is still writing is refused by `undo` with exit 2 —
/// nothing touched, and it will work once that run finishes — and `recover`
/// reports it as running rather than as a problem.
#[test]
fn a_journal_in_use_is_refused_by_undo_and_left_alone_by_recover() {
    use ren_core::exec::{Journal, Record};

    let journal = TempDir::new().unwrap();
    let mut live = Journal::create(journal.path()).unwrap();
    live.write(Record::Begin {
        platform: "test".to_owned(),
        items: 0,
    })
    .unwrap();

    let undo = run(
        journal.path(),
        &["undo", "--txn", &live.path().to_string_lossy()],
    );
    let err = String::from_utf8_lossy(&undo.stderr);
    assert_eq!(undo.status.code(), Some(2), "{err}");
    assert!(err.contains("another window"), "{err}");

    let recover = run(journal.path(), &["recover"]);
    let err = String::from_utf8_lossy(&recover.stderr);
    assert_eq!(recover.status.code(), Some(0), "{err}");
    assert!(err.contains("another window"), "{err}");
    drop(live);
}
