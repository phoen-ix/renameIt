//! `ren-cli --preset` — the headless half of M4's acceptance criterion.
//!
//! *"presets persist and reload across restarts; `ren-cli --preset file.toml`
//! runs headless."* The restart half is only honestly testable across a real
//! process boundary, which is what these do: one invocation writes, a second,
//! entirely separate one reads.

use std::path::Path;
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

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn write_preset(&self, file: &str, text: &str) -> std::path::PathBuf {
        let path = self.presets.path().join(file);
        std::fs::write(&path, text).unwrap();
        path
    }

    /// A three-operation pipeline: Replace → Casing → Add Counter.
    fn three_op_preset(&self) -> std::path::PathBuf {
        self.write_preset(
            "photo-cleanup.toml",
            r#"
[preset]
name = "Photo cleanup"
description = "Underscores out, title case, numbered."

[settings.counter]
auto_pad = false

[[step]]
op = "replace"
find = "_"
replace = " "

[[step]]
op = "casing"
scope = "name"
mode = "title"

[[step]]
op = "add_counter"
separator = ". "
"#,
        )
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        bin()
            .args(args)
            .arg("--journal-dir")
            .arg(self.journal.path())
            .output()
            .expect("ren-cli should run")
    }

    fn journals(&self) -> usize {
        std::fs::read_dir(self.journal.path())
            .map(|read| {
                read.filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                    .count()
            })
            .unwrap_or(0)
    }
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// M4's acceptance criterion, end to end and out of process.
#[test]
fn a_three_operation_preset_runs_headless_and_undoes_as_one_transaction() {
    let fixture = Fixture::new(&["my_holiday_photo.jpg", "another_one.jpg"]);
    let preset = fixture.three_op_preset();

    let applied = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert!(applied.status.success(), "{}", stderr(&applied));

    assert_eq!(
        fixture.names(),
        ["1. Another One.jpg", "2. My Holiday Photo.jpg"]
    );
    assert_eq!(fixture.journals(), 1, "three operations, one transaction");

    let undone = fixture.run(&["undo"]);
    assert!(undone.status.success(), "{}", stderr(&undone));
    assert_eq!(
        fixture.names(),
        ["another_one.jpg", "my_holiday_photo.jpg"],
        "one undo reverts the whole batch"
    );
}

/// The "across restarts" half: one process writes the preset folder, another
/// reads it, with nothing shared but the directory.
#[test]
fn a_preset_survives_a_restart_and_runs_by_name() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let source = fixture.three_op_preset();

    // A separate folder, so importing is a real copy.
    let library = TempDir::new().unwrap();
    let imported = fixture.run(&[
        "presets",
        "import",
        source.to_str().unwrap(),
        "--preset-dir",
        library.path().to_str().unwrap(),
    ]);
    assert!(imported.status.success(), "{}", stderr(&imported));

    // A brand-new process, which knows only where the folder is.
    let listed = fixture.run(&[
        "presets",
        "list",
        "--preset-dir",
        library.path().to_str().unwrap(),
    ]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert!(
        stdout(&listed).contains("Photo cleanup"),
        "{}",
        stdout(&listed)
    );

    // And it runs by name, the way `/r "preset name"` does.
    let applied = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        "Photo cleanup",
        "--preset-dir",
        library.path().to_str().unwrap(),
    ]);
    assert!(applied.status.success(), "{}", stderr(&applied));
    assert_eq!(fixture.names(), ["1. A B.txt"]);
}

/// A job file brings its own source; a preset borrows yours.
#[test]
fn a_preset_needs_a_directory_and_a_job_file_refuses_one() {
    let fixture = Fixture::new(&["a.txt"]);
    let preset = fixture.three_op_preset();

    // A preset borrows the caller's source, so it needs one — and since M7
    // there are three ways to give it: a folder, --list or --file. The message
    // names all three rather than only the folder.
    let no_dir = fixture.run(&["preview", "--preset", preset.to_str().unwrap()]);
    assert!(!no_dir.status.success());
    let message = stderr(&no_dir);
    for expected in ["folder", "--list", "--file"] {
        assert!(
            message.contains(expected),
            "{expected} missing from: {message}"
        );
    }

    let job = fixture.write_preset(
        "job.toml",
        &format!(
            "[source]\ndir = {:?}\n\n[[step]]\nop = \"space_trim\"\n",
            fixture.dir.path()
        ),
    );
    let with_dir = fixture.run(&[
        "preview",
        fixture.dir.path().to_str().unwrap(),
        "--job",
        job.to_str().unwrap(),
    ]);
    // clap refuses the pair itself now, with its own wording, and exit 1.
    assert_eq!(with_dir.status.code(), Some(1));
    assert!(
        stderr(&with_dir).contains("cannot be used with"),
        "{}",
        stderr(&with_dir)
    );
}

/// Importing a job file as a preset is meant to work — same schema (D18) — but
/// the folder it names is not the user's.
#[test]
fn a_preset_ignores_a_source_section_and_says_so() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let elsewhere = TempDir::new().unwrap();
    let preset = fixture.write_preset(
        "with-source.toml",
        &format!(
            "[source]\ndir = {:?}\n\n[[step]]\nop = \"replace\"\nfind = \"_\"\nreplace = \" \"\n",
            elsewhere.path()
        ),
    );

    let output = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("ignoring the [source]"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fixture.names(), ["a b.txt"], "it renamed *our* folder");
}

/// The listing flags mean something for a preset, because it has no source.
#[test]
fn the_listing_flags_apply_to_a_preset() {
    let fixture = Fixture::new(&["keep_me.txt", "keep_me.mp3"]);
    let preset = fixture.write_preset(
        "underscores.toml",
        "[preset]\nname = \"Underscores\"\n\n[[step]]\nop = \"replace\"\nfind = \"_\"\nreplace = \" \"\n",
    );

    let output = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--pattern",
        "*.mp3",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fixture.names(), ["keep me.mp3", "keep_me.txt"]);
}

#[test]
fn an_unknown_preset_name_lists_the_ones_that_exist() {
    let fixture = Fixture::new(&["a.txt"]);
    fixture.three_op_preset();

    let output = fixture.run(&[
        "preview",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        "Nope",
        "--preset-dir",
        fixture.presets.path().to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(message.contains("Nope"), "{message}");
    assert!(message.contains("Photo cleanup"), "{message}");
}

/// `<Ask>` on stdin is fine for a person and hostile to a script, so it can
/// also be answered on the command line.
#[test]
fn an_ask_in_a_preset_can_be_answered_on_the_command_line() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let preset = fixture.write_preset(
        "prefix.toml",
        r#"
[preset]
name = "Add prefix"

[[step]]
op = "add_remove"
mode = "add"
insert = "<Ask> "
add_pos = 0
"#,
    );

    let output = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--answer",
        "0=Holiday",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fixture.names(), ["Holiday one.txt", "Holiday two.txt"]);
}

/// Preview never prompts, so a scripted preview cannot hang. An unanswered
/// `<Ask>` leaves the name alone rather than renaming around the gap.
#[test]
fn previewing_a_preset_with_an_ask_never_prompts_and_changes_nothing() {
    let fixture = Fixture::new(&["one.txt"]);
    let preset = fixture.write_preset(
        "prefix.toml",
        "[preset]\nname = \"Add prefix\"\n\n[[step]]\nop = \"add_remove\"\nmode = \"add\"\ninsert = \"<Ask> \"\nadd_pos = 0\n",
    );

    let output = fixture.run(&[
        "preview",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    // Nothing to do, so the preview reports no executable change — but it
    // returned, which is the point.
    assert!(
        stdout(&output).contains("0 to rename"),
        "{}",
        stdout(&output)
    );
    assert_eq!(fixture.names(), ["one.txt"]);
}

/// A **New** preset starts empty, so an empty preset must be legal.
#[test]
fn an_empty_preset_runs_and_renames_nothing() {
    let fixture = Fixture::new(&["a.txt"]);
    let preset = fixture.write_preset("empty.toml", "[preset]\nname = \"Empty\"\n");

    let output = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fixture.names(), ["a.txt"]);
}

/// A job file is a preset with a source (D18/D33), so the shipped examples
/// import unchanged.
#[test]
fn a_shipped_example_imports_as_a_preset() {
    let fixture = Fixture::new(&["a.txt"]);
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cleanup.toml");

    let output = fixture.run(&[
        "presets",
        "import",
        example.to_str().unwrap(),
        "--preset-dir",
        fixture.presets.path().to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("dropped the [source]"),
        "{}",
        stderr(&output)
    );

    let listed = fixture.run(&[
        "presets",
        "list",
        "--preset-dir",
        fixture.presets.path().to_str().unwrap(),
    ]);
    assert!(stdout(&listed).contains("cleanup"), "{}", stdout(&listed));
}

#[test]
fn export_writes_a_file_that_import_reads_back() {
    let fixture = Fixture::new(&["a.txt"]);
    let source = fixture.three_op_preset();
    let library = TempDir::new().unwrap();
    let shared = TempDir::new().unwrap();

    fixture.run(&[
        "presets",
        "import",
        source.to_str().unwrap(),
        "--preset-dir",
        library.path().to_str().unwrap(),
    ]);

    let out = shared.path().join("shared.toml");
    let exported = fixture.run(&[
        "presets",
        "export",
        "Photo cleanup",
        out.to_str().unwrap(),
        "--preset-dir",
        library.path().to_str().unwrap(),
    ]);
    assert!(exported.status.success(), "{}", stderr(&exported));
    assert!(out.is_file());

    let shown = fixture.run(&[
        "presets",
        "show",
        "Photo cleanup",
        "--preset-dir",
        library.path().to_str().unwrap(),
    ]);
    assert!(
        stdout(&shown).contains("Photo cleanup"),
        "{}",
        stdout(&shown)
    );
    assert!(!stdout(&shown).contains("[source]"), "{}", stdout(&shown));
}

/// The shipped music preset, end to end over real files.
///
/// Worth a whole-process test rather than another unit one, because the two
/// things it proves only meet each other here: the documented `<\>` really moves
/// a file into a subfolder, and a slash that arrives *inside a tag value* —
/// AC/DC — really does not (D61). Getting the second one wrong scatters a music
/// library across directories and reports nothing.
#[test]
fn the_shipped_music_preset_sorts_an_album_without_being_scattered_by_a_slash() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::new(&[]);
    Mp3::tagged("AC/DC", "Highway to Hell")
        .frame("TALB", "Highway to Hell")
        .frame("TRCK", "6")
        .write(fixture.dir.path(), "track6.mp3");
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/music-rename.toml");

    let output = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        example.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));

    // Two levels of folder from the two `<\>`, and exactly two — the artist's
    // own slash became a dash rather than a third.
    let sorted = fixture
        .dir
        .path()
        .join("AC-DC")
        .join("Highway to Hell")
        .join("06. Highway to Hell.mp3");
    assert!(sorted.is_file(), "expected {}", sorted.display());
    assert_eq!(fixture.names(), ["AC-DC"]);
}

/// P2 as something a script cannot forget: a job file carrying a tagger step
/// fails closed, names the flag that would allow it, and touches nothing.
#[test]
fn a_job_file_that_writes_tags_fails_closed_until_the_flag_is_given() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::new(&[]);
    Mp3::tagged("Old", "Old").write(fixture.dir.path(), "Metallica - One.mp3");
    let before = std::fs::read(fixture.dir.path().join("Metallica - One.mp3")).unwrap();

    let preset = fixture.write_preset(
        "tagger.toml",
        r#"
[preset]
name = "Tag from filename"

[settings]
parts = "<%1> - <%2>"

[[step]]
op = "music_tagger"
artist = "<%1>"
title = "<%2>"
"#,
    );

    let refused = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert!(!refused.status.success(), "{}", stdout(&refused));
    let said = format!("{}{}", stdout(&refused), stderr(&refused));
    assert!(said.contains("cannot be undone"), "{said}");
    assert_eq!(
        std::fs::read(fixture.dir.path().join("Metallica - One.mp3")).unwrap(),
        before,
        "it refused and wrote anyway"
    );

    // And with the flag it runs.
    let allowed = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--allow-irreversible",
    ]);
    assert!(allowed.status.success(), "{}", stderr(&allowed));
    assert!(
        stdout(&allowed).contains("modified 1 item"),
        "{}",
        stdout(&allowed)
    );
    ren_core::meta::audio::forget_all();
    let tags = ren_core::meta::audio::tags_of(&fixture.dir.path().join("Metallica - One.mp3"))
        .expect("still audio");
    assert_eq!(tags.artist.as_deref(), Some("Metallica"));
    assert_eq!(tags.title.as_deref(), Some("One"));
}

/// D54 built `UndoReport.irreversible` so a mixed batch could say which half
/// came back. Nothing read it for a whole milestone, so undo reported success
/// and never mentioned the tag writes that stayed.
#[test]
fn undo_says_which_changes_it_could_not_take_back() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::new(&[]);
    Mp3::tagged("Old", "Old").write(fixture.dir.path(), "song.mp3");
    let preset = fixture.write_preset(
        "mixed.toml",
        r#"
[preset]
name = "Mixed"

[[step]]
op = "replace"
find = "song"
replace = "tune"

[[step]]
op = "music_tagger"
artist = "Written"
"#,
    );

    let run = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--allow-irreversible",
    ]);
    assert!(run.status.success(), "{}", stderr(&run));
    assert_eq!(fixture.names(), ["tune.mp3"]);

    let undone = fixture.run(&["undo"]);
    assert!(
        undone.status.success(),
        "an irreversible change is not a failure (D54): {}",
        stderr(&undone)
    );
    assert_eq!(
        fixture.names(),
        ["song.mp3"],
        "the rename did not come back"
    );

    let said = stdout(&undone);
    assert!(
        said.contains("still applied"),
        "undo must say what stayed: {said}"
    );
    // Named at the path it was written to. The reverse replay reverts the tag
    // write's record before the rename that produced that name, so this is the
    // file as it was when the change landed — which is what "still applied"
    // is about.
    assert!(said.contains("tune.mp3"), "and name it: {said}");
    assert!(said.contains("music_tagger"), "and say which step: {said}");
    // The tag really is still what the run wrote — that is what it is saying.
    ren_core::meta::audio::forget_all();
    assert_eq!(
        ren_core::meta::audio::tags_of(&fixture.dir.path().join("song.mp3"))
            .unwrap()
            .artist
            .as_deref(),
        Some("Written")
    );
}

/// Every shipped example must actually load — they are documentation people
/// copy from, so one that has drifted from the schema is worse than none.
#[test]
fn every_shipped_example_loads() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("examples/") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        seen += 1;
        let fixture = Fixture::new(&["a.txt"]);
        let output = fixture.run(&[
            "presets",
            "import",
            path.to_str().unwrap(),
            "--preset-dir",
            fixture.presets.path().to_str().unwrap(),
        ]);
        assert!(
            output.status.success(),
            "{} did not import: {}",
            path.display(),
            stderr(&output)
        );
    }
    assert!(seen >= 4, "expected the shipped examples, found {seen}");
}

// --- --answer has to land, and every <Ask> has to be answered ------------

const ASKING: &str = "[preset]\nname = \"Add prefix\"\n\n[[step]]\nop = \"add_remove\"\nmode = \"add\"\ninsert = \"<Ask> \"\nadd_pos = 0\n";

/// A mistyped slot used to be stored and ignored: `--answer 10=Holiday`
/// meant for slot 1, the real slot unanswered, and the run renamed nothing
/// with exit 0. A slot the pipeline does not ask for is refused, naming the
/// ones it does.
#[test]
fn an_answer_for_a_slot_nobody_asks_for_is_refused() {
    let fixture = Fixture::new(&["one.txt"]);
    let preset = fixture.write_preset("prefix.toml", ASKING);

    for answer in ["10=Holiday", "1=Holiday"] {
        let output = fixture.run(&[
            "apply",
            fixture.dir.path().to_str().unwrap(),
            "--preset",
            preset.to_str().unwrap(),
            "--answer",
            answer,
        ]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{answer}: {}",
            stderr(&output)
        );
        assert_eq!(fixture.names(), ["one.txt"]);
    }
    let output = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
        "--answer",
        "1=Holiday",
    ]);
    assert!(stderr(&output).contains("<Ask>"), "{}", stderr(&output));
}

/// Under a scheduler stdin is closed, so a question nobody can answer is a
/// refusal — not a run that leaves every name alone and exits 0.
#[test]
fn an_ask_that_stdin_cannot_answer_stops_the_run() {
    let fixture = Fixture::new(&["one.txt"]);
    let preset = fixture.write_preset("prefix.toml", ASKING);

    let output = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("--answer"), "{}", stderr(&output));
    assert_eq!(fixture.names(), ["one.txt"]);
    assert_eq!(fixture.journals(), 0, "nothing ran");
}

// --- presets export -------------------------------------------------------

/// TO is any path the user typed, and there is no save dialog to ask: a
/// `notes.txt` typed for `notes.toml` became TOML with no undo.
#[test]
fn export_never_replaces_a_file_unless_told_to() {
    let fixture = Fixture::new(&["a.txt"]);
    let source = fixture.three_op_preset();
    let dir = fixture.presets.path().join("store");
    let imported = fixture.run(&[
        "presets",
        "import",
        source.to_str().unwrap(),
        "--preset-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(imported.status.success(), "{}", stderr(&imported));

    let notes = fixture.presets.path().join("notes.txt");
    std::fs::write(&notes, b"my notes").unwrap();
    let export = |force: bool| {
        let mut line = vec![
            "presets",
            "export",
            "Photo cleanup",
            notes.to_str().unwrap(),
            "--preset-dir",
            dir.to_str().unwrap(),
        ];
        if force {
            line.push("--force");
        }
        fixture.run(&line)
    };

    let refused = export(false);
    assert_eq!(refused.status.code(), Some(1));
    assert!(stderr(&refused).contains("--force"), "{}", stderr(&refused));
    assert_eq!(std::fs::read(&notes).unwrap(), b"my notes");

    let forced = export(true);
    assert!(forced.status.success(), "{}", stderr(&forced));
    assert!(
        std::fs::read_to_string(&notes)
            .unwrap()
            .contains("Photo cleanup")
    );
}

// --- What undo says -------------------------------------------------------

/// A folder the run created and something else now lives in is kept, and
/// undo says so — an empty-looking year folder that survived an undo is
/// otherwise a mystery.
#[test]
fn undo_names_a_folder_it_kept() {
    let fixture = Fixture::new(&["a.txt"]);
    let preset = fixture.write_preset(
        "move.toml",
        "[[step]]\nop = \"add_remove\"\nscope = \"both\"\nmode = \"add\"\ninsert = \"new<\\\\>\"\nadd_pos = 0\n",
    );
    let run = fixture.run(&[
        "apply",
        fixture.dir.path().to_str().unwrap(),
        "--preset",
        preset.to_str().unwrap(),
    ]);
    assert!(run.status.success(), "{}", stderr(&run));
    let created = fixture.dir.path().join("new");
    assert!(created.join("a.txt").exists(), "{}", stdout(&run));
    std::fs::write(created.join("mine.txt"), b"the user's own").unwrap();

    let undone = fixture.run(&["undo"]);
    assert!(undone.status.success(), "{}", stderr(&undone));
    let said = stdout(&undone);
    assert!(said.contains("kept folder"), "{said}");
    assert!(said.contains("new"), "{said}");
    assert!(fixture.dir.path().join("a.txt").exists());
}

// --- The shipped examples do what they say --------------------------------

/// Every example, previewed over a folder of the kinds of file it is for.
///
/// Importing them only proved they parse, which is how `camera-import.toml`
/// shipped turning `IMG_0001.jpg` into `2021-07-04 1jpg` — and a screenshot
/// into " 2png". So every renamed row that had an extension must still have
/// one, and a file without an Exif date must be left alone by the camera
/// example, as it promises.
#[test]
fn every_shipped_example_previews_without_losing_an_extension() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut seen = 0;
    for entry in std::fs::read_dir(&examples).expect("examples/") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        seen += 1;
        let fixture = Fixture::new(&[
            "notes_file.txt",
            "Metallica - One.txt",
            "shot.png",
            "x cant y.txt",
        ]);
        std::fs::write(
            fixture.dir.path().join("IMG_0001.jpg"),
            ren_core::meta::testing::jpeg_with_exif(Some("2021:07:04 10:11:12"), None, None),
        )
        .unwrap();

        let output = fixture.run(&[
            "preview",
            fixture.dir.path().to_str().unwrap(),
            "--preset",
            path.to_str().unwrap(),
        ]);
        let name = path.file_name().unwrap().to_string_lossy();
        let said = stdout(&output);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{name}: {said}{}",
            stderr(&output)
        );

        for line in said.lines() {
            let Some(row) = line.trim_start().strip_prefix("→") else {
                continue;
            };
            let (from, to) = row.trim().split_once("  ->  ").expect("a rename row");
            let to = to.split("  [").next().unwrap();
            if Path::new(from).extension().is_some() {
                assert!(
                    Path::new(to).extension().is_some_and(|e| !e.is_empty()),
                    "{name}: {from} -> {to} lost its extension"
                );
            }
        }

        // Its comment promises the shipped fifty-one Batch Replace rules;
        // it used to list three of its own, which replace them (D35).
        if name == "cleanup.toml" {
            assert!(said.contains("X Can't Y.txt"), "{said}");
        }
        if name == "camera-import.toml" {
            assert!(said.contains("2021-07-04 1.jpg"), "{said}");
            for untouched in ["shot.png", "notes_file.txt"] {
                assert!(
                    !said.contains(&format!("{untouched}  ->")),
                    "{untouched} has no Exif date and must be left alone: {said}"
                );
            }
        }
    }
    assert!(seen >= 4, "expected the shipped examples, found {seen}");
}
