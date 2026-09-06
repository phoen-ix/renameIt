//! The legacy command line, driven through the real binary.
//!
//! The unit tests in `src/compat.rs` prove the *translation*; these prove that
//! what it translates into actually does the thing — including the two
//! switches the compat layer cannot check on its own, `/l` and `/k`, which
//! touch the filesystem.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ren-cli"))
}

struct Fixture {
    dir: TempDir,
    journal: TempDir,
    presets: TempDir,
}

impl Fixture {
    fn new(names: &[&str]) -> Self {
        let dir = TempDir::new().unwrap();
        for (i, name) in names.iter().enumerate() {
            std::fs::write(dir.path().join(name), format!("payload {i}")).unwrap();
        }
        Self {
            dir,
            journal: TempDir::new().unwrap(),
            presets: TempDir::new().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A list file of full paths, as `/l` expects.
    fn list_file(&self, names: &[&str]) -> PathBuf {
        let path = self.presets.path().join("files.txt");
        let body: String = names
            .iter()
            .map(|n| format!("{}\n", self.dir.path().join(n).display()))
            .collect();
        std::fs::write(&path, body).unwrap();
        path
    }

    fn preset(&self, file: &str, body: &str) -> PathBuf {
        let path = self.presets.path().join(file);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        bin()
            .args(args)
            .arg("--journal-dir")
            .arg(self.journal.path())
            .output()
            .expect("ren-cli should run")
    }

    /// Without `--journal-dir`, for the legacy lines that must translate
    /// exactly as written.
    fn run_bare(&self, args: &[&str]) -> std::process::Output {
        bin().args(args).output().expect("ren-cli should run")
    }
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &std::process::Output) -> i32 {
    output.status.code().expect("a normal exit")
}

/// A preset every file lands on the same name through — the simplest way to
/// produce the conflict P4 says must block a run.
const COLLIDING: &str = r#"
[[step]]
op = "free_format"
pattern = "same"
"#;

/// Appends to the whole name, extension included, so a folder and a file both
/// get a predictable result — `<FullName>` is one tag rather than a stem and an
/// extension that a folder does not have.
const SUFFIXED: &str = r#"
[[step]]
op = "free_format"
pattern = "<FullName>-done"
"#;

// --- /l and /k, the two that touch the filesystem ------------------------

/// > *"/l - Allows you to specify a text file containing files and folders
/// > (full paths, one file or folder per line). The files and folders will be
/// > loaded into the Free Select mode."*
#[test]
fn a_list_file_renames_exactly_the_files_it_names() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "untouched.txt"]);
    let list = fixture.list_file(&["a.txt", "b.txt"]);
    let preset = fixture.preset("suffix.toml", SUFFIXED);

    let out = fixture.run(&[
        "apply",
        "--list",
        list.to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        fixture.names(),
        ["a.txt-done", "b.txt-done", "untouched.txt"],
        "free select renames the list and nothing else"
    );
}

/// `/k` deletes the `/l` list file once it has been read.
///
/// Ours waits for the run to have succeeded first. That is deliberate: `/k` is
/// not a rename, so it is not journalled and there is no
/// undo for it — and a batch that failed is one somebody will want to run
/// again from the same list.
#[test]
fn the_list_file_is_deleted_once_the_run_has_happened() {
    let fixture = Fixture::new(&["a.txt"]);
    let list = fixture.list_file(&["a.txt"]);
    let preset = fixture.preset("suffix.toml", SUFFIXED);

    let out = fixture.run(&[
        "apply",
        "--list",
        list.to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--delete-list",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!list.exists(), "the list file should be gone");
}

/// The shape a shell integration emits: `/l "%l" /k` with no `/r`, which
/// translates to `preview --delete-list`.
///
/// Deleting only on `apply` would leak that temp file on every invocation,
/// forever. `/k` is conditioned on the list having been *read*, and a
/// successful preview has read it.
#[test]
fn a_successful_preview_consumes_the_list_too() {
    let fixture = Fixture::new(&["a.txt"]);
    let list = fixture.list_file(&["a.txt"]);

    let out = fixture.run(&["/l", list.to_str().unwrap(), "/k"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!list.exists(), "the list file should be gone");
    assert_eq!(fixture.names(), ["a.txt"], "but nothing was renamed");
}

/// A simulation performs no filesystem call at all, so it must not delete the
/// one file the whole run was driven from.
#[test]
fn a_simulation_never_deletes_the_list_file() {
    let fixture = Fixture::new(&["a.txt"]);
    let list = fixture.list_file(&["a.txt"]);
    let preset = fixture.preset("suffix.toml", SUFFIXED);

    let out = fixture.run(&[
        "apply",
        "--list",
        list.to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--delete-list",
        "--simulate",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(list.exists(), "a simulation must leave everything alone");
    assert_eq!(fixture.names(), ["a.txt"]);
}

/// And a run that was refused leaves the list where it was, so it can be run
/// again after the pipeline is fixed.
#[test]
fn a_blocked_run_keeps_the_list_file() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let list = fixture.list_file(&["a.txt", "b.txt"]);
    let preset = fixture.preset("collide.toml", COLLIDING);

    let out = fixture.run(&[
        "apply",
        "--list",
        list.to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--delete-list",
    ]);
    assert_eq!(code(&out), 2, "a blocked plan is exit 2: {}", stderr(&out));
    assert!(list.exists(), "nothing ran, so nothing was consumed");
    assert_eq!(fixture.names(), ["a.txt", "b.txt"]);
}

/// A list naming something that is not there is a usage error, and it says
/// which line was wrong rather than only that something was.
#[test]
fn a_list_naming_a_missing_file_says_which_one() {
    let fixture = Fixture::new(&["a.txt"]);
    let list = fixture.presets.path().join("files.txt");
    std::fs::write(&list, "/nowhere/at/all/ghost.txt\n").unwrap();

    let out = fixture.run(&["preview", "--list", list.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("ghost.txt"), "{}", stderr(&out));
}

/// Blank lines and `#` comments are an extension — a generated list is also
/// the thing a person hand-edits when something went wrong.
#[test]
fn a_list_file_tolerates_blank_lines_and_comments() {
    let fixture = Fixture::new(&["a.txt"]);
    let list = fixture.presets.path().join("files.txt");
    std::fs::write(
        &list,
        format!(
            "# the ones that matter\n\n{}\n\n",
            fixture.path().join("a.txt").display()
        ),
    )
    .unwrap();
    let preset = fixture.preset("suffix.toml", SUFFIXED);

    let out = fixture.run(&[
        "apply",
        "--list",
        list.to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(fixture.names(), ["a.txt-done"]);
}

/// PowerShell's `Out-File` writes a byte-order mark, and a list file is what a
/// script feeds `/l`. The mark is not whitespace, so `trim` alone left the
/// first path as `\u{feff}C:\…` — missing, and printed indistinguishably from
/// the real one.
#[test]
fn a_list_file_with_a_byte_order_mark_still_names_its_first_file() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let list = fixture.presets.path().join("files.txt");
    std::fs::write(
        &list,
        format!(
            "\u{feff}{}\r\n{}\r\n",
            fixture.path().join("a.txt").display(),
            fixture.path().join("b.txt").display()
        ),
    )
    .unwrap();
    let preset = fixture.preset("suffix.toml", SUFFIXED);

    let out = fixture.run(&[
        "apply",
        "--list",
        list.to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(fixture.names(), ["a.txt-done", "b.txt-done"]);
}

// --- The legacy command line, end to end ---------------------------------

/// The canonical scheduled-task line, run for real:
/// `/p c:\download\*.* /r "my preset" /f`
#[test]
fn the_scheduled_task_example_renames() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let preset = fixture.preset("suffix.toml", SUFFIXED);
    let masked = format!("{}/*.txt", fixture.path().display());

    let out = fixture.run(&["/p", &masked, "/r", preset.to_str().unwrap(), "/f"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(fixture.names(), ["one.txt-done", "two.txt-done"]);
    assert!(
        stderr(&out).contains("legacy switches translated"),
        "the translation is always announced: {}",
        stderr(&out)
    );
}

/// Without a preset (`/r`), the command line only loads the specified path.
///
/// Headless, "load and show" is a preview — so nothing is renamed.
#[test]
fn a_legacy_line_without_a_preset_only_lists() {
    let fixture = Fixture::new(&["one.txt"]);
    let out = fixture.run(&["/p", fixture.path().to_str().unwrap()]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(fixture.names(), ["one.txt"], "a preview renames nothing");
}

/// `--verbose` explains each switch, and without it the run stays quiet enough
/// to pipe.
#[test]
fn verbose_explains_what_each_switch_became() {
    let fixture = Fixture::new(&["one.txt"]);

    let quiet = fixture.run(&["/p", fixture.path().to_str().unwrap(), "/x"]);
    assert!(!stderr(&quiet).contains("no window"), "{}", stderr(&quiet));

    let loud = fixture.run(&["/p", fixture.path().to_str().unwrap(), "/x", "--verbose"]);
    let message = stderr(&loud);
    assert!(message.contains("/x"), "{message}");
    assert!(message.contains("window"), "{message}");
}

/// `/d` alone means folders instead of files, which is the pair-reading the
/// compat layer exists for — checked here against a real listing.
#[test]
fn the_include_switches_change_what_is_listed() {
    let fixture = Fixture::new(&["a.txt"]);
    std::fs::create_dir(fixture.path().join("sub")).unwrap();
    let preset = fixture.preset("suffix.toml", SUFFIXED);

    let out = fixture.run(&[
        "/p",
        fixture.path().to_str().unwrap(),
        "/d",
        "/r",
        preset.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        fixture.names(),
        ["a.txt", "sub-done"],
        "/d without /f is folders and not files"
    );
}

/// Bare paths, the Send To case: loaded into free select and only listed.
#[test]
fn bare_paths_are_loaded_and_listed() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let a = fixture.path().join("a.txt");
    let out = fixture.run_bare(&[a.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("legacy switches translated"),
        "{}",
        stderr(&out)
    );
    assert_eq!(fixture.names(), ["a.txt", "b.txt"], "nothing was renamed");
}

// --- Exit codes ----------------------------------------------------------

/// P4: a plan with conflicts is refused, and refusing is not success.
#[test]
fn a_conflicting_plan_exits_blocked_from_both_preview_and_apply() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let preset = fixture.preset("collide.toml", COLLIDING);

    for command in ["preview", "apply"] {
        let out = fixture.run(&[
            command,
            fixture.path().to_str().unwrap(),
            "--preset",
            preset.to_str().unwrap(),
        ]);
        assert_eq!(code(&out), 2, "{command}: {}", stderr(&out));
    }
    assert_eq!(fixture.names(), ["a.txt", "b.txt"], "nothing was touched");
}

/// A clean run is zero, and an unusable command line is one — so a caller can
/// tell "I asked for the wrong thing" from "the rename was refused".
#[test]
fn success_is_zero_and_a_bad_command_line_is_one() {
    let fixture = Fixture::new(&["a.txt"]);
    let preset = fixture.preset("suffix.toml", SUFFIXED);

    let good = fixture.run(&[
        "apply",
        fixture.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert_eq!(code(&good), 0, "{}", stderr(&good));

    let bad = fixture.run(&["preview", "--preset", preset.to_str().unwrap()]);
    assert_eq!(code(&bad), 1, "no source is a usage error");
}

// --- The channels a run reports through ----------------------------------
//
// D77's rule: a channel nothing reads is a change the user is never told
// about. M7 added two — the files a script asks for, and whatever its `done()`
// returned — and both were populated and read by nobody until the audit said
// so. These are the tests that keep them wired.

/// A preset that runs the shipped playlist script over two MP3s.
fn playlist_preset(fixture: &Fixture) -> PathBuf {
    fixture.preset(
        "playlist.toml",
        "[[step]]
op = \"script\"
script = \"Create Mp3 Playlist\"
args = \"\"
",
    )
}

fn shipped_scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ren-core/data/scripts")
        .canonicalize()
        .expect("the shipped scripts")
}

#[test]
fn a_file_a_script_asked_for_is_reported_and_written() {
    let fixture = Fixture::new(&["one.mp3", "two.mp3"]);
    let preset = playlist_preset(&fixture);

    let previewed = fixture.run(&[
        "preview",
        fixture.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--script-dir",
        shipped_scripts().to_str().unwrap(),
    ]);
    assert_eq!(code(&previewed), 0, "{}", stderr(&previewed));
    assert!(
        stdout(&previewed).contains("would write"),
        "a preview must say what it would write:\n{}",
        stdout(&previewed)
    );
    assert!(
        !fixture.path().join("Playlist.m3u").exists(),
        "and must not write it"
    );

    let applied = fixture.run(&[
        "apply",
        fixture.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--script-dir",
        shipped_scripts().to_str().unwrap(),
    ]);
    assert_eq!(code(&applied), 0, "{}", stderr(&applied));
    let written = fixture.path().join("Playlist.m3u");
    assert!(written.exists(), "{}", stdout(&applied));
    assert!(
        stdout(&applied).contains("wrote"),
        "the write has to be reported:\n{}",
        stdout(&applied)
    );
    // And whatever `done()` returned, which goes to the log.
    assert!(
        stdout(&applied).contains("Created mp3 playlist"),
        "the script's own message is missing:\n{}",
        stdout(&applied)
    );
    let body = std::fs::read_to_string(&written).unwrap();
    assert!(body.starts_with("#EXTM3U"), "{body}");
}

// --- Exit codes, continued -----------------------------------------------

/// clap's own exit code for a parse error is 2, which is `Exit::Blocked`.
/// A caller must be able to tell a typo from a refused rename.
#[test]
fn a_command_line_clap_rejects_is_usage_not_blocked() {
    let fixture = Fixture::new(&["a.txt"]);
    for line in [
        vec!["preview", "--no-such-flag"],
        vec!["prevew", "/tmp"],
        vec!["apply", "--preset"],
    ] {
        let out = fixture.run(&line);
        assert_eq!(code(&out), 1, "{line:?} should be a usage error");
    }
}

/// `--help` and `--version` are requests, not failures.
#[test]
fn help_and_version_exit_zero() {
    let fixture = Fixture::new(&["a.txt"]);
    for line in [vec!["--help"], vec!["--version"], vec!["preview", "--help"]] {
        let out = fixture.run_bare(&line);
        assert_eq!(code(&out), 0, "{line:?}");
    }
    // Including the alternative spellings.
    let out = fixture.run_bare(&["/?"]);
    assert_eq!(code(&out), 0);
}

/// When a translated line fails to parse, clap prints usage for a grammar the
/// caller never typed — so the translation is named first.
#[test]
fn a_translated_line_that_does_not_parse_says_what_it_became() {
    let fixture = Fixture::new(&["a.txt"]);
    let out = fixture.run_bare(&["/p", fixture.path().to_str().unwrap(), "--no-such-flag"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("did not parse"), "{}", stderr(&out));
}
