//! The nine shipped scripts, one fixture test each.
//!
//! Several of them have edges that look like typos and are not — a script that
//! quietly "fixed" them would be a different script wearing the same name, and
//! somebody's rename would come out different from the one they had been
//! getting for years. Each such edge is named where it is checked.

use ren_core::ops::Script;
use ren_core::{ListOptions, Pipeline, PlannedOp, RunSettings, list, plan};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Where the shipped ports live in the repo.
fn shipped_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("data/scripts")
}

struct Fixture {
    dir: TempDir,
    aux: TempDir,
}

impl Fixture {
    fn new(names: &[&str]) -> Self {
        Self::with_bodies(&names.iter().map(|n| (*n, "body")).collect::<Vec<_>>())
    }

    fn with_bodies(files: &[(&str, &str)]) -> Self {
        let dir = TempDir::new().unwrap();
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).unwrap();
        }
        Self {
            dir,
            aux: TempDir::new().unwrap(),
        }
    }

    fn aux_file(&self, name: &str, body: &str) -> PathBuf {
        let path = self.aux.path().join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn op(&self, script: &str) -> Script {
        Script::new(script).in_dir(shipped_dir())
    }

    fn plan_of(&self, op: Script) -> ren_core::Plan {
        self.plan_seeded(op, 0)
    }

    fn plan_seeded(&self, op: Script, seed: u64) -> ren_core::Plan {
        let entries = list(self.dir.path(), ListOptions::default()).unwrap();
        let mut pipeline = Pipeline::new().then(op);
        pipeline.settings = RunSettings {
            seed,
            ..Default::default()
        };
        plan(&entries, &pipeline, ren_platform::host().as_ref())
    }

    /// Every row's new name, in listing order.
    fn names(&self, script: &str, args: &str) -> Vec<String> {
        let plan = self.plan_of(self.op(script).with_args(args));
        for item in &plan.items {
            assert!(
                !matches!(item.state, ren_core::RowState::Error(_)),
                "{script}: {:?}",
                item.state
            );
        }
        plan.items.iter().map(|i| i.new_name.clone()).collect()
    }
}

/// Every shipped script compiles, has a description, and defines `rename`.
///
/// The cheapest guard against a port that was never run: a syntax error would
/// otherwise only show up when somebody chose that script in the picker.
#[test]
fn all_nine_shipped_scripts_compile_and_carry_a_header() {
    let (scripts, legacy) = ren_core::script::ScriptStore::new(shipped_dir()).list();
    assert_eq!(scripts.len(), 9, "the shipped set is nine scripts");
    assert!(legacy.is_empty(), "no .frs should ship");

    for entry in &scripts {
        let source = std::fs::read_to_string(&entry.path).unwrap();
        let compiled = ren_core::script::compile(&source)
            .unwrap_or_else(|e| panic!("{} does not compile: {e}", entry.name));
        assert!(
            !compiled.header().description.is_empty(),
            "{} has no description",
            entry.name
        );
        assert!(
            source.contains("rename ="),
            "{} defines no rename function",
            entry.name
        );
    }
}

/// And the shipped set is exactly these nine, by name.
#[test]
fn the_shipped_set_is_the_expected_nine() {
    let (scripts, _) = ren_core::script::ScriptStore::new(shipped_dir()).list();
    let mut names: Vec<&str> = scripts.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "CSV List Rename",
            "Create Mp3 Playlist",
            "Example - Base for New Script",
            "Get HTML XML Tags",
            "Insert Space Before Caps",
            "Length of Filename",
            "Safe Characters",
            "Swap Around",
            "Unique Random Number",
        ]
    );
}

// --- 1. Swap Around ----------------------------------------------------------

/// > *"For example 'AA - BB' can be swapped around ' - ' to give 'BB - AA'."*
#[test]
fn swap_around_swaps_the_worked_example() {
    let fixture = Fixture::new(&["AA - BB.txt"]);
    assert_eq!(fixture.names("Swap Around", " - "), ["BB - AA.txt"]);
}

/// `InStr(..., vbTextCompare)` matches case-insensitively, but the separator
/// written back into the middle comes from `Mid(Filename, ...)` — so the
/// filename's own casing survives, not the argument's.
#[test]
fn swap_around_matches_case_insensitively_and_keeps_the_filenames_casing() {
    let fixture = Fixture::new(&["AA AND BB.txt"]);
    assert_eq!(fixture.names("Swap Around", " and "), ["BB AND AA.txt"]);
}

/// A name without the separator is left alone rather than skipped or
/// mangled.
#[test]
fn swap_around_leaves_a_name_without_the_separator_alone() {
    let fixture = Fixture::new(&["nothing here.txt"]);
    assert_eq!(fixture.names("Swap Around", " - "), ["nothing here.txt"]);
}

/// First occurrence only — `InStr` starts at 1 and is not repeated.
#[test]
fn swap_around_uses_only_the_first_separator() {
    let fixture = Fixture::new(&["A - B - C.txt"]);
    assert_eq!(fixture.names("Swap Around", " - "), ["B - C - A.txt"]);
}

// --- 2. Length of Filename ---------------------------------------------------

/// > *"By default it only shows the length of the filename."* Including the
/// > extension: the script reads `FullFilename`, not `Filename`.
#[test]
fn length_of_filename_counts_the_whole_name_including_the_extension() {
    let fixture = Fixture::new(&["abcd.txt"]);
    assert_eq!(fixture.names("Length of Filename", ""), ["8.txt"]);
}

/// Type 1 in the arguments to also include the path. An exact string
/// comparison, so only `1` works.
#[test]
fn length_of_filename_adds_the_path_only_for_exactly_one() {
    let fixture = Fixture::new(&["abcd.txt"]);
    let with_path: usize = fixture.names("Length of Filename", "1")[0]
        .trim_end_matches(".txt")
        .parse()
        .unwrap();
    assert!(with_path > 8, "the path was not counted: {with_path}");
    assert_eq!(
        fixture.names("Length of Filename", "true"),
        ["8.txt"],
        "anything but the exact string 1 is ignored"
    );
}

// --- 3. Safe Characters ------------------------------------------------------

/// > *"for example å becomes a and ö becomes o"* — the description's own
/// > example, which is also the reason the source file is CP1252.
#[test]
fn safe_characters_transliterates_the_descriptions_own_example() {
    let fixture = Fixture::new(&["Björk å ö.txt"]);
    assert_eq!(fixture.names("Safe Characters", ""), ["Bjork a o.txt"]);
}

/// The three entries that look inconsistent and are deliberate. Asserted
/// together, because each one looks like a typo on its own and a tidy-minded
/// edit would "fix" all three.
#[test]
fn safe_characters_keeps_its_deliberately_inconsistent_entries() {
    let fixture = Fixture::new(&["Þ ß × Ð.txt"]);
    assert_eq!(
        fixture.names("Safe Characters", ""),
        ["th SS x D.txt"],
        "uppercase thorn maps to lowercase 'th' while sharp s maps to 'SS', \
         and the multiplication sign becomes an x"
    );
}

/// Multi-character expansions, and a character the table does not cover
/// passing through untouched.
#[test]
fn safe_characters_expands_ligatures_and_leaves_the_rest_alone() {
    let fixture = Fixture::new(&["Æon æon Œuvre.txt"]);
    assert_eq!(
        fixture.names("Safe Characters", ""),
        ["AEon aeon Œuvre.txt"],
        "Œ is not in the table"
    );
}

// --- 4. Insert Space Before Caps ---------------------------------------------

#[test]
fn insert_space_before_caps_splits_run_together_words() {
    let fixture = Fixture::new(&["SomeDownloadedFile.txt"]);
    assert_eq!(
        fixture.names("Insert Space Before Caps", ""),
        ["Some Downloaded File.txt"]
    );
}

/// > *"and some other characters"* — `(`, `[`, `-` and `&` each get a space,
/// > and a run of digits gets one space in front of the run rather than each
/// > digit.
#[test]
fn insert_space_before_caps_handles_digits_and_punctuation() {
    let fixture = Fixture::new(&["Movie2024[HD].txt"]);
    assert_eq!(
        fixture.names("Insert Space Before Caps", ""),
        ["Movie 2024 [HD].txt"],
        "one space before the digit run, one before the bracket, and none \
         inside HD because the character after '[' is skipped"
    );
}

/// The first character is never given a space — `skip_next = true 'always skip
/// first'`.
#[test]
fn insert_space_before_caps_never_leads_with_a_space() {
    let fixture = Fixture::new(&["ABCd.txt"]);
    let out = &fixture.names("Insert Space Before Caps", "")[0];
    assert!(!out.starts_with(' '), "{out}");
}

// --- 5. Get HTML XML Tags ----------------------------------------------------

#[test]
fn get_html_xml_tags_takes_the_name_from_the_tag() {
    let fixture = Fixture::with_bodies(&[(
        "page.html",
        "<html><head><title>The Real Title</title></head></html>",
    )]);
    assert_eq!(
        fixture.names("Get HTML XML Tags", "title"),
        ["The Real Title.html"]
    );
}

/// **A deviation, not a quirk kept.**
///
/// Folding only the haystack and leaving the argument alone would mean `TITLE`
/// matches nothing at all and the user gets silence. Ours matches
/// case-insensitively on both sides.
///
/// Every other oddity in this script is kept deliberately; this one is
/// not, because "your argument must be lower-case or nothing happens" is a
/// defect rather than a behaviour, and reproducing it would mean reproducing
/// the fold-then-slice bug that caused it — folding the document and then
/// indexing the *original* with the copy's offsets, which is wrong wherever a
/// character's lower-case form has a different UTF-8 width. See D106.
#[test]
fn get_html_xml_tags_matches_the_tag_name_case_insensitively() {
    let fixture = Fixture::with_bodies(&[("page.html", "<title>Something</title>")]);
    assert_eq!(
        fixture.names("Get HTML XML Tags", "TITLE"),
        ["Something.html"],
        "an uppercase argument finds a lowercase tag"
    );

    // And the document's own casing does not matter either.
    let fixture = Fixture::with_bodies(&[("page.html", "<TiTle>Mixed</TiTle>")]);
    assert_eq!(fixture.names("Get HTML XML Tags", "title"), ["Mixed.html"]);
}

/// The offsets a search returns must be valid for the string they will slice.
///
/// This is the regression test for the fold-then-slice bug: a character before
/// the tag whose lower-case form is a *different number of bytes* shifted every
/// offset, so the extracted value came back cut in the wrong place — or the
/// slice landed off a character boundary and failed outright.
#[test]
fn get_html_xml_tags_survives_a_character_that_changes_width_when_folded() {
    // U+0130 LATIN CAPITAL LETTER I WITH DOT ABOVE is two bytes and lower-cases
    // to three, so every byte offset past it moves.
    let fixture = Fixture::with_bodies(&[(
        "page.html",
        "<p>\u{0130}\u{0130}\u{0130}</p><title>Correct</title>",
    )]);
    assert_eq!(
        fixture.names("Get HTML XML Tags", "title"),
        ["Correct.html"]
    );
}

/// `if posStop = 0 then posStop = instr(..., "</ " & Args & ">")` — a closing
/// tag with a space after the slash is accepted.
#[test]
fn get_html_xml_tags_accepts_a_closing_tag_with_a_space() {
    let fixture = Fixture::with_bodies(&[("page.html", "<title>Spaced</ title>")]);
    assert_eq!(fixture.names("Get HTML XML Tags", "title"), ["Spaced.html"]);
}

/// > *"Return is limited to 64 characters with newlines stripped."*
#[test]
fn get_html_xml_tags_cuts_to_sixty_four_characters() {
    let long = "x".repeat(200);
    let fixture = Fixture::with_bodies(&[("page.html", &format!("<title>{long}</title>"))]);
    let out = &fixture.names("Get HTML XML Tags", "title")[0];
    assert_eq!(out.trim_end_matches(".html").len(), 64, "{out}");
}

// --- 6. Unique Random Number -------------------------------------------------

/// > *"generates a random number within a range, where each number will be
/// > unique"*.
#[test]
fn unique_random_number_gives_every_file_a_different_number_from_the_range() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt", "d.txt"]);
    let mut numbers: Vec<i64> = fixture
        .names("Unique Random Number", "5-20")
        .iter()
        .map(|n| n.trim_end_matches(".txt").parse().unwrap())
        .collect();
    assert_eq!(numbers.len(), 4);
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), 4, "two files got the same number");
    assert!(
        numbers.iter().all(|n| (5..=20).contains(n)),
        "outside the range: {numbers:?}"
    );
}

/// Seeding from the clock would reseed on every preview, so the numbers in the
/// preview column would not be the numbers written to disk. This seeds from the
/// run, so two previews of the same run agree — and two different runs still
/// differ.
#[test]
fn unique_random_number_is_reproducible_within_a_run_and_varies_between_runs() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);
    let names = |seed: u64| -> Vec<String> {
        fixture
            .plan_seeded(fixture.op("Unique Random Number").with_args("1-99"), seed)
            .items
            .iter()
            .map(|i| i.new_name.clone())
            .collect()
    };

    assert_eq!(names(7), names(7), "the same run must preview the same");
    assert_ne!(
        names(7),
        names(8),
        "a different run must shuffle differently"
    );
}

/// > *"If you get an error there is probably a problem with the range you have
/// > typed in."* It goes on the row rather than into a dialog, which is where
/// > an error about a file belongs.
#[test]
fn unique_random_number_reports_a_bad_range_on_the_row() {
    let fixture = Fixture::new(&["a.txt"]);
    for args in ["", "not a range", "5-", "10-1", "1-999999"] {
        let plan = fixture.plan_of(fixture.op("Unique Random Number").with_args(args));
        assert!(
            matches!(plan.items[0].state, ren_core::RowState::Error(_)),
            "{args:?} was accepted as a range"
        );
    }
}

/// More files than numbers: the surplus is skipped, leaving those names
/// alone.
#[test]
fn unique_random_number_leaves_the_surplus_files_alone() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let out = fixture.names("Unique Random Number", "1-2");
    let untouched: Vec<&String> = out
        .iter()
        .filter(|n| n.starts_with(char::is_alphabetic))
        .collect();
    assert_eq!(
        untouched.len(),
        1,
        "exactly one file ran out of numbers: {out:?}"
    );
}

// --- 7. CSV List Rename ------------------------------------------------------

#[test]
fn csv_list_rename_renames_from_the_list() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let csv = fixture.aux_file("list.csv", "one,first\ntwo,second\n");
    assert_eq!(
        fixture.names("CSV List Rename", &csv.display().to_string()),
        ["first.txt", "second.txt"]
    );
}

/// The divergence from the native operation, which the plan said to assert
/// rather than paper over: the script's loop has no early exit, so on a
/// duplicate old name the **last** row wins, where the native
/// one uses *"the first one found"*.
#[test]
fn csv_list_rename_lets_the_last_duplicate_win_where_the_native_operation_takes_the_first() {
    let fixture = Fixture::new(&["one.txt"]);
    let csv = fixture.aux_file("dupes.csv", "one,first\none,second\n");

    assert_eq!(
        fixture.names("CSV List Rename", &csv.display().to_string()),
        ["second.txt"],
        "the script keeps scanning after a match"
    );

    // And the native operation, over the same list, disagrees — deliberately.
    let entries = list(fixture.dir.path(), ListOptions::default()).unwrap();
    let native = Pipeline::new().then(ren_core::ops::CsvList::new(&csv));
    let plan = plan(&entries, &native, ren_platform::host().as_ref());
    assert_eq!(plan.items[0].new_name, "first.txt");
}

/// Rows without exactly two fields are skipped, which is how a header line
/// survives contact with this script.
#[test]
fn csv_list_rename_skips_rows_that_are_not_two_fields() {
    let fixture = Fixture::new(&["one.txt"]);
    let csv = fixture.aux_file("header.csv", "old,new,note\none,first\n");
    assert_eq!(
        fixture.names("CSV List Rename", &csv.display().to_string()),
        ["first.txt"]
    );
}

// --- 8. Example - Base for New Script -----------------------------------------

/// The template's job is to demonstrate the shape: a counter that survives from
/// one file to the next, a `format_tags` call, and a `done` that says something.
#[test]
fn the_example_script_counts_across_files_and_renders_a_tag() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let plan = fixture.plan_of(fixture.op("Example - Base for New Script"));

    assert!(
        plan.items[0].new_name.starts_with("File 1 is"),
        "{:?}",
        plan.items[0].new_name
    );
    assert!(
        plan.items[1].new_name.starts_with("File 2 is"),
        "{:?}",
        plan.items[1].new_name
    );
    assert_eq!(plan.notes, ["The example script looked at 2 file(s)."]);
}

// --- 9. Create Mp3 Playlist ---------------------------------------------------

fn playlist_of(plan: &ren_core::Plan) -> Option<(PathBuf, String)> {
    plan.ops.iter().find_map(|op| match op {
        PlannedOp::WriteFile { path, contents, .. } => Some((path.clone(), contents.clone())),
        _ => None,
    })
}

/// > *"This script creates a playlist for the selected Mp3 files."* And it
/// > renames nothing while doing it — every row comes back untouched.
#[test]
fn create_mp3_playlist_writes_an_m3u_and_renames_nothing() {
    let fixture = Fixture::new(&["one.mp3", "two.mp3"]);
    let op = fixture.op("Create Mp3 Playlist");
    let plan = fixture.plan_of(op);

    assert_eq!(plan.changed(), 0, "the playlist script renames nothing");

    let (path, contents) = playlist_of(&plan).expect("a playlist was planned");
    assert_eq!(path, fixture.dir.path().join("Playlist.m3u"));
    // CRLF, because VBScript's `WriteLine` writes CRLF and an .m3u is a
    // Windows-era format. Asserted rather than assumed: `\n` alone passed for
    // a while, and the difference is invisible in a diff.
    assert!(contents.starts_with("#EXTM3U\r\n"), "{contents:?}");
    assert!(contents.ends_with("\r\n"), "{contents:?}");
    assert!(
        !contents.contains("\n\n"),
        "no bare LF anywhere: {contents:?}"
    );
    assert!(contents.contains("one.mp3"), "{contents}");
    assert!(contents.contains("two.mp3"), "{contents}");
    assert!(contents.contains("#EXTINF:"), "{contents}");
}

/// > *"You can enter a filename as input argument, or else it will be called
/// > Playlist.m3u."*
#[test]
fn create_mp3_playlist_takes_its_name_from_the_arguments() {
    let fixture = Fixture::new(&["one.mp3"]);
    let named = |args: &str| -> PathBuf {
        let op = fixture.op("Create Mp3 Playlist").with_args(args);
        playlist_of(&fixture.plan_of(op)).expect("a playlist").0
    };

    assert_eq!(named("Road Trip.m3u").file_name().unwrap(), "Road Trip.m3u");
    assert_eq!(
        named("Road Trip").file_name().unwrap(),
        "Road Trip.m3u",
        "the extension is added when it is missing"
    );
    assert_eq!(
        named(".m3u").file_name().unwrap(),
        ".m3u.m3u",
        "the suffix is only checked when the argument is longer than four \
         characters, so exactly '.m3u' gains a second one"
    );
}

/// `right(FullFilename,4) <> ".mp3"` is a case-sensitive comparison, so an
/// uppercase extension is not included.
#[test]
fn create_mp3_playlist_ignores_uppercase_extensions_and_other_files() {
    let fixture = Fixture::new(&["keep.mp3", "SKIP.MP3", "notes.txt"]);
    let op = fixture.op("Create Mp3 Playlist");
    let (_, contents) = playlist_of(&fixture.plan_of(op)).expect("a playlist");

    assert!(contents.contains("keep.mp3"), "{contents}");
    assert!(!contents.contains("SKIP.MP3"), "{contents}");
    assert!(!contents.contains("notes.txt"), "{contents}");
}

/// A single MP3 still gets a playlist. Skipping it would be an off-by-one
/// dressed up as a decision, so this is on the record rather than discovered
/// later.
#[test]
fn create_mp3_playlist_writes_a_one_track_playlist() {
    let fixture = Fixture::new(&["only.mp3"]);
    let op = fixture.op("Create Mp3 Playlist");
    let (_, contents) = playlist_of(&fixture.plan_of(op)).expect("a one-track playlist");
    assert!(contents.contains("only.mp3"), "{contents}");
}

/// No MP3s at all is not an error, and writes nothing.
#[test]
fn create_mp3_playlist_writes_nothing_when_there_are_no_mp3s() {
    let fixture = Fixture::new(&["notes.txt"]);
    let op = fixture.op("Create Mp3 Playlist");
    let plan = fixture.plan_of(op);
    assert!(playlist_of(&plan).is_none());
    assert_eq!(plan.notes, ["No MP3 files, so no playlist was written."]);
}
