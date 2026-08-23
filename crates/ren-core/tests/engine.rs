//! End-to-end engine tests: plan → apply → undo against a real tempdir.
//!
//! These encode M0's acceptance criterion — "`apply` + `undo` round-trips a
//! tempdir byte-exactly" — plus the conflict rules the planner owes the GUI.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use ren_core::exec::{Journal, Record};
use ren_core::model::Scope;
use ren_core::ops::{EvalCx, NameTransform, OpError};
use ren_core::{
    ApplyOptions, ConflictKind, Effect, ListOptions, Pipeline, PlannedOp, RowState, Step,
    StepConfig, TimeSet, TimeStamp, Undoability, apply, list, plan, undo_last,
};
use tempfile::TempDir;

/// Renames by table lookup, so tests can construct chains and cycles that the
/// M0 append-suffix operation cannot express.
#[derive(Debug)]
struct MapNames(BTreeMap<String, String>);

impl MapNames {
    fn new(pairs: &[(&str, &str)]) -> Self {
        Self(
            pairs
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect(),
        )
    }
}

impl NameTransform for MapNames {
    fn id(&self) -> &'static str {
        "test_map_names"
    }

    fn summary(&self) -> String {
        format!("Map {} name(s)", self.0.len())
    }

    fn apply<'a>(&self, subject: &'a str, _cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        Ok(match self.0.get(subject) {
            Some(to) => Cow::Owned(to.clone()),
            None => Cow::Borrowed(subject),
        })
    }
}

fn pipeline_of(transform: impl NameTransform + 'static, scope: Scope) -> Pipeline {
    Pipeline::new().with(Step::Name(Box::new(transform)), StepConfig::scoped(scope))
}

/// `(name -> contents)` for every file in a directory, so a tree can be
/// compared byte-for-byte before and after.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

fn mtimes(dir: &Path) -> BTreeMap<String, SystemTime> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.metadata().unwrap().modified().unwrap(),
            )
        })
        .collect()
}

struct Fixture {
    dir: TempDir,
    journal: TempDir,
    /// Files a test needs on disk but *not* in the listing — a CSV list, above
    /// all. Written into `dir` it would be a listed entry itself, and every
    /// count and conflict in the test would quietly shift.
    aux: TempDir,
}

impl Fixture {
    fn new(names: &[&str]) -> Self {
        let dir = TempDir::new().unwrap();
        for (i, name) in names.iter().enumerate() {
            std::fs::write(dir.path().join(name), format!("contents of {i}")).unwrap();
        }
        Self {
            dir,
            journal: TempDir::new().unwrap(),
            aux: TempDir::new().unwrap(),
        }
    }

    /// Writes a file outside the listed directory and returns its path.
    fn aux_file(&self, name: &str, body: &str) -> std::path::PathBuf {
        let path = self.aux.path().join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn options(&self) -> ApplyOptions {
        ApplyOptions {
            simulate: false,
            journal_dir: self.journal.path().to_path_buf(),
            allow_irreversible: false,
        }
    }

    /// The same, with P2's consent given — what the GUI's confirmation and the
    /// CLI's `--allow-irreversible` both amount to.
    fn options_allowing_irreversible(&self) -> ApplyOptions {
        ApplyOptions {
            allow_irreversible: true,
            ..self.options()
        }
    }

    fn entries(&self) -> Vec<ren_core::FileEntry> {
        list(self.dir.path(), ListOptions::default()).unwrap()
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }

    fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = snapshot(self.dir.path()).into_keys().collect();
        v.sort();
        v
    }
}

// --- M0 acceptance -----------------------------------------------------------

#[test]
fn apply_then_undo_restores_the_tree_exactly() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["one.txt", "two.txt", "Ünïcödé 🎵.mp3"]);

    let before = snapshot(fixture.dir.path());
    let before_mtimes = mtimes(fixture.dir.path());

    let entries = fixture.entries();
    let pipeline = pipeline_of(ren_core::AppendSuffix::new("_v2"), Scope::Name);
    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.changed(), 3);
    assert!(plan.is_executable());

    // The preview promise: the name shown is the name written.
    let previewed: Vec<String> = plan.items.iter().map(|i| i.new_name.clone()).collect();

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success());
    assert_eq!(report.renamed.len(), 3);

    let mut on_disk = fixture.names();
    let mut expected = previewed.clone();
    on_disk.sort();
    expected.sort();
    assert_eq!(on_disk, expected, "previewed name != on-disk name");

    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undo.is_complete(), "skipped: {:?}", undo.skipped);
    assert_eq!(undo.restored.len(), 3);

    assert_eq!(
        snapshot(fixture.dir.path()),
        before,
        "contents or names drifted"
    );
    assert_eq!(mtimes(fixture.dir.path()), before_mtimes, "mtimes drifted");
}

#[test]
fn simulate_writes_nothing_but_reports_the_full_plan() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let before = snapshot(fixture.dir.path());

    let entries = fixture.entries();
    let pipeline = pipeline_of(ren_core::AppendSuffix::new("_x"), Scope::Name);
    let plan = plan(&entries, &pipeline, platform.as_ref());

    let options = ApplyOptions {
        simulate: true,
        ..fixture.options()
    };
    let report = apply(&plan, platform.as_ref(), &options).unwrap();

    assert!(report.simulated);
    assert_eq!(report.renamed.len(), 2);
    assert!(report.txn.is_none(), "simulation must not open a journal");
    assert_eq!(snapshot(fixture.dir.path()), before);
    assert!(Journal::list(fixture.journal.path()).unwrap().is_empty());
}

// --- Conflict rules (P4) -----------------------------------------------------

#[test]
fn two_items_landing_on_one_name_block_the_run() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(MapNames::new(&[("a", "same"), ("b", "same")]), Scope::Name);

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.conflicts(), 2);
    assert!(matches!(
        plan.items[0].state,
        RowState::Conflict(ConflictKind::DuplicateTarget { .. })
    ));
    assert!(!plan.is_executable());
    assert!(apply(&plan, platform.as_ref(), &fixture.options()).is_err());
}

#[test]
fn renaming_onto_an_untouched_file_is_a_conflict() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "keep.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(MapNames::new(&[("a", "keep")]), Scope::Name);

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.conflicts(), 1);
    assert!(!plan.is_executable());
}

#[test]
fn renaming_onto_a_file_that_is_not_listed_is_a_conflict() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt"]);
    std::fs::write(fixture.dir.path().join("bystander.txt"), b"not listed").unwrap();

    // List only `a.txt`, so `bystander.txt` is invisible to the planner.
    let entries: Vec<_> = fixture
        .entries()
        .into_iter()
        .filter(|e| e.file_name == "a.txt")
        .collect();
    let pipeline = pipeline_of(MapNames::new(&[("a", "bystander")]), Scope::Name);

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert!(matches!(
        plan.items[0].state,
        RowState::Conflict(ConflictKind::TargetExists)
    ));
}

#[test]
fn an_illegal_name_is_reported_before_anything_is_written() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt"]);
    let entries = fixture.entries();
    // `..` is the parent directory on every platform we support.
    let pipeline = pipeline_of(MapNames::new(&[("a.txt", "..")]), Scope::Both);

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert!(
        matches!(
            plan.items[0].state,
            RowState::Conflict(ConflictKind::InvalidName(_))
        ),
        "{:?}",
        plan.items[0].state
    );
}

/// D31: a separator in a produced name is a move into a subfolder, and every
/// component of it is checked like a name of its own.
#[test]
fn a_produced_separator_moves_the_file_into_a_subfolder() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(MapNames::new(&[("a", "sub/dir")]), Scope::Name);

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.items[0].state, RowState::Changed);
    assert_eq!(plan.items[0].target, fixture.path("sub").join("dir.txt"));
    assert_eq!(
        plan.ops[0],
        ren_core::PlannedOp::CreateDir {
            path: fixture.path("sub")
        }
    );
}

/// A produced name may go down, never up or out.
#[test]
fn a_produced_name_may_not_escape_its_folder() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let entries = fixture.entries();

    for escape in ["../up", "/absolute", "./here"] {
        let pipeline = pipeline_of(MapNames::new(&[("a", escape)]), Scope::Name);
        let plan = plan(&entries, &pipeline, platform.as_ref());
        assert!(
            matches!(
                plan.items[0].state,
                RowState::Conflict(ConflictKind::InvalidName(_))
            ),
            "{escape:?} should be rejected, got {:?}",
            plan.items[0].state
        );
    }
}

// --- Ordering ----------------------------------------------------------------

#[test]
fn a_rename_chain_is_ordered_so_no_step_collides() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let entries = fixture.entries();
    // a -> b and b -> c: `b` must vacate before `a` claims it.
    let pipeline = pipeline_of(MapNames::new(&[("a", "b"), ("b", "c")]), Scope::Name);

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.conflicts(), 0, "{:?}", plan.items);
    assert!(plan.is_executable());

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(fixture.names(), vec!["b.txt".to_string(), "c.txt".into()]);

    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undo.is_complete(), "{:?}", undo.skipped);
    assert_eq!(fixture.names(), vec!["a.txt".to_string(), "b.txt".into()]);
}

/// P6: a swap is resolved inside one run, and undo still works afterwards. A
/// two-pass mode that breaks undo is not an acceptable answer.
#[test]
fn a_swap_is_resolved_in_one_run_via_a_temp_name() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(MapNames::new(&[("a", "b"), ("b", "a")]), Scope::Name);

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.conflicts(), 0, "{:?}", plan.items);
    assert!(plan.is_executable());
    // Two real renames plus the stage/finish pair that makes them possible.
    assert_eq!(plan.ops.len(), 3);

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(fixture.names(), vec!["a.txt".to_string(), "b.txt".into()]);
    // The contents actually swapped.
    assert_eq!(
        std::fs::read(fixture.dir.path().join("a.txt")).unwrap(),
        b"contents of 1"
    );

    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undo.is_complete(), "{:?}", undo.skipped);
    assert_eq!(
        std::fs::read(fixture.dir.path().join("a.txt")).unwrap(),
        b"contents of 0",
        "undo must unwind the temp-name hop too"
    );
    // No temp file left behind.
    assert_eq!(fixture.names(), vec!["a.txt".to_string(), "b.txt".into()]);
}

#[test]
fn a_three_way_rotation_is_resolved_in_one_run() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["1.txt", "2.txt", "3.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(
        MapNames::new(&[("1", "2"), ("2", "3"), ("3", "1")]),
        Scope::Name,
    );

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.conflicts(), 0, "{:?}", plan.items);

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(
        fixture.names(),
        vec!["1.txt".to_string(), "2.txt".into(), "3.txt".into()]
    );
    // 1 -> 2 means "2.txt" now holds what "1.txt" held.
    assert_eq!(
        std::fs::read(fixture.dir.path().join("2.txt")).unwrap(),
        b"contents of 0"
    );

    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undo.is_complete(), "{:?}", undo.skipped);
    assert_eq!(
        std::fs::read(fixture.dir.path().join("1.txt")).unwrap(),
        b"contents of 0"
    );
}

/// Two independent cycles in one batch each get their own temp name.
#[test]
fn two_separate_cycles_are_broken_independently() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "b.txt", "x.txt", "y.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(
        MapNames::new(&[("a", "b"), ("b", "a"), ("x", "y"), ("y", "x")]),
        Scope::Name,
    );

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.conflicts(), 0, "{:?}", plan.items);

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(
        fixture.names(),
        vec![
            "a.txt".to_string(),
            "b.txt".into(),
            "x.txt".into(),
            "y.txt".into()
        ]
    );
}

/// A chain feeding into a cycle: the chain link must wait for the cycle to
/// unwind, which only works because staging releases the victim's dependents.
#[test]
fn a_chain_hanging_off_a_cycle_still_resolves() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let entries = fixture.entries();
    // a<->b is a cycle; c wants b's old name, which only frees up at the end.
    let pipeline = pipeline_of(
        MapNames::new(&[("a", "b"), ("b", "a"), ("c", "d")]),
        Scope::Name,
    );

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.conflicts(), 0, "{:?}", plan.items);
    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(
        fixture.names(),
        vec!["a.txt".to_string(), "b.txt".into(), "d.txt".into()]
    );
}

#[test]
fn a_plan_is_deterministic_so_two_runs_agree() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(MapNames::new(&[("a", "b"), ("b", "a")]), Scope::Name);

    let first = plan(&entries, &pipeline, platform.as_ref());
    let second = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(first.ops, second.ops);
}

// --- Journal + undo ----------------------------------------------------------

#[test]
fn the_journal_records_intent_before_the_rename_happens() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(ren_core::AppendSuffix::new("_x"), Scope::Name);
    let plan = plan(&entries, &pipeline, platform.as_ref());

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    let lines = Journal::read(report.journal.as_ref().unwrap()).unwrap();

    let plan_at = lines
        .iter()
        .position(|l| matches!(l.record, Record::PlanRename { .. }))
        .expect("a plan_rename record");
    let done_at = lines
        .iter()
        .position(|l| matches!(l.record, Record::Completed { .. }))
        .expect("a completed record");
    assert!(plan_at < done_at, "intent must be durable before the act");
    assert!(matches!(lines[0].record, Record::Begin { .. }));
    assert!(matches!(
        lines.last().unwrap().record,
        Record::Commit { .. }
    ));
}

#[test]
fn undo_skips_entries_something_else_has_touched() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(ren_core::AppendSuffix::new("_x"), Scope::Name);
    let plan = plan(&entries, &pipeline, platform.as_ref());
    apply(&plan, platform.as_ref(), &fixture.options()).unwrap();

    // Simulate another program moving one of the renamed files away.
    std::fs::rename(
        fixture.dir.path().join("a_x.txt"),
        fixture.dir.path().join("moved-by-someone-else.txt"),
    )
    .unwrap();

    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert_eq!(undo.restored.len(), 1);
    assert_eq!(undo.skipped.len(), 1);
    assert!(!undo.is_complete());
    // The untouched one came back; the moved one was left alone, not clobbered.
    assert_eq!(
        fixture.names(),
        vec!["b.txt".to_string(), "moved-by-someone-else.txt".into()]
    );
}

#[test]
fn a_transaction_cannot_be_undone_twice() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a.txt"]);
    let entries = fixture.entries();
    let pipeline = pipeline_of(ren_core::AppendSuffix::new("_x"), Scope::Name);
    let plan = plan(&entries, &pipeline, platform.as_ref());
    apply(&plan, platform.as_ref(), &fixture.options()).unwrap();

    ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    let second = ren_core::undo_last(platform.as_ref(), fixture.journal.path());
    assert!(second.is_err(), "the same batch was offered for undo twice");
}

/// A folder renamed in the same run as a **swap inside it**.
///
/// Found by `a_run_over_a_folder_and_its_contents_previews_truly_and_undoes_
/// exactly` on a fresh seed: invert the casing of a folder holding `README` and
/// `readme`. The two files swap, which needs a temp name — and the folder had
/// nothing blocking it, so it was ready on the first pass and renamed **first**,
/// leaving every path underneath naming a folder that no longer existed. The
/// run stopped part-way, which is the one outcome P1 exists to prevent.
///
/// Reproduced here with a **swap of digits** rather than of case: `README` and
/// `readme` are two files on Linux and one on Windows, so the seed that found
/// this cannot be the fixture that pins it. Swap Mode gives the same 2-cycle on
/// any filesystem, and renames the folder in the same pipeline.
///
/// Two things were missing, and this fails without either: a folder waits for
/// everything inside it, and a cycle's second half runs before the folder above
/// it moves.
#[test]
fn a_folder_renames_after_a_swap_inside_it_has_finished() {
    let platform = ren_platform::host();
    let dir = tempfile::TempDir::new().unwrap();
    let inner = dir.path().join("1");
    std::fs::create_dir(&inner).unwrap();
    for name in ["1.txt", "2.txt"] {
        std::fs::write(inner.join(name), b"x").unwrap();
    }

    let entries = ren_core::listing::list(
        dir.path(),
        ListOptions {
            folders: true,
            subfolders: true,
            ..Default::default()
        },
    )
    .unwrap();
    // 1 ↔ 2 everywhere: the folder `1` becomes `2`, and the two files inside
    // trade names.
    let pipeline = pipeline_of(
        ren_core::ops::Replace::new("1", "2").swap(true),
        Scope::Both,
    );
    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert!(plan.is_executable(), "{:?}", plan.items);

    let report = apply(&plan, platform.as_ref(), &ren_core::ApplyOptions::default()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);

    let mut left: Vec<String> = std::fs::read_dir(dir.path().join("2"))
        .expect("the folder took its new name")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["1.txt", "2.txt"], "both survived the swap");
}

// --- M3 acceptance -----------------------------------------------------------

/// The documented collision warning: *"renaming is sequential, so '+1 to all
/// numbers' collides with not-yet-renamed files"*, and its workaround was two
/// passes (add 101, then subtract 100). M1's topological ordering makes the
/// warning obsolete — this is the test that says so.
#[test]
fn adding_one_to_every_number_no_longer_collides() {
    use ren_core::ops::{NumberAction, NumberTarget, ReNumber};

    let platform = ren_platform::host();
    let names: Vec<String> = (1..=12).map(|i| format!("File {i:02}.txt")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let fixture = Fixture::new(&refs);
    let before = snapshot(fixture.dir.path());

    let op = ReNumber::new(NumberTarget::All, NumberAction::Add)
        .with_operand("1")
        .keeping_length(true);
    let pipeline = pipeline_of(op, Scope::Name);

    let entries = fixture.entries();
    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.conflicts(), 0, "{:?}", plan.items);
    assert!(plan.is_executable());

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(fixture.names()[0], "File 02.txt");
    assert_eq!(fixture.names()[11], "File 13.txt");

    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undo.is_complete(), "{:?}", undo.skipped);
    assert_eq!(snapshot(fixture.dir.path()), before, "undo must be exact");
}

/// A shuffled set renumbers to a clean `01..NN` sequence in listing order, and
/// undo puts every byte back — M3's stated acceptance criterion.
#[test]
fn a_shuffled_set_renumbers_to_a_clean_sequence_and_round_trips() {
    use ren_core::ops::{AddCounter, CounterPlacement};
    use ren_core::{CounterSetup, RunSettings};

    let platform = ren_platform::host();
    let names = [
        "07 zebra.txt",
        "02 apple.txt",
        "11 mango.txt",
        "04 pear.txt",
        "09 fig.txt",
    ];
    let fixture = Fixture::new(&names);
    let before = snapshot(fixture.dir.path());

    // Listing order is name order, so the counter follows the sorted names.
    let mut entries = fixture.entries();
    entries.sort_by(|a, b| a.file_name.cmp(&b.file_name));

    let op = AddCounter::new(CounterPlacement::First, " ").replacing("<Mid-3>");
    let mut pipeline = pipeline_of(op, Scope::Name);
    pipeline.settings = RunSettings {
        counter: CounterSetup {
            auto_pad: false,
            pad: 2,
            ..Default::default()
        },
        ..Default::default()
    };

    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert_eq!(plan.conflicts(), 0, "{:?}", plan.items);
    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);

    assert_eq!(
        fixture.names(),
        [
            "01 apple.txt",
            "02 pear.txt",
            "03 zebra.txt",
            "04 fig.txt",
            "05 mango.txt",
        ]
    );

    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undo.is_complete(), "{:?}", undo.skipped);
    assert_eq!(snapshot(fixture.dir.path()), before);
}

/// D31 end to end: `<\\>` sorts files into folders, and undo takes the folders
/// away again.
#[test]
fn the_subfolder_tag_moves_files_and_undo_removes_the_folders_it_made() {
    use ren_core::ops::{AddRemove, AddRemoveMode};

    let platform = ren_platform::host();
    let fixture = Fixture::new(&["alpha.txt", "beta.txt", "acorn.txt"]);
    let before = snapshot(fixture.dir.path());

    // "<FLetter>/" in front of the name: one folder per initial.
    let op = AddRemove {
        mode: AddRemoveMode::Add,
        insert: "<FLetter><\\>".into(),
        add_pos: 0,
        ..Default::default()
    };
    let entries = fixture.entries();
    let plan = plan(&entries, &pipeline_of(op, Scope::Both), platform.as_ref());
    assert_eq!(plan.conflicts(), 0, "{:?}", plan.items);

    // Two folders, each created once, both before any rename.
    let creations: Vec<_> = plan
        .ops
        .iter()
        .take_while(|op| matches!(op, ren_core::PlannedOp::CreateDir { .. }))
        .collect();
    assert_eq!(creations.len(), 2, "{:?}", plan.ops);

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert!(fixture.path("A").join("alpha.txt").is_file());
    assert!(fixture.path("A").join("acorn.txt").is_file());
    assert!(fixture.path("B").join("beta.txt").is_file());
    assert!(
        snapshot(fixture.dir.path()).is_empty(),
        "nothing left at the top"
    );

    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undo.is_complete(), "{:?}", undo.skipped);
    assert_eq!(undo.removed_dirs.len(), 2, "{:?}", undo);
    assert!(undo.kept_dirs.is_empty());
    assert!(!fixture.path("A").exists());
    assert_eq!(snapshot(fixture.dir.path()), before);
}

/// The safety half of D31: a folder the user has since put something into is
/// kept, not deleted.
#[test]
fn undo_keeps_a_created_folder_that_is_no_longer_empty() {
    use ren_core::ops::{AddRemove, AddRemoveMode};

    let platform = ren_platform::host();
    let fixture = Fixture::new(&["alpha.txt"]);

    let op = AddRemove {
        mode: AddRemoveMode::Add,
        insert: "sorted<\\>".into(),
        add_pos: 0,
        ..Default::default()
    };
    let entries = fixture.entries();
    let plan = plan(&entries, &pipeline_of(op, Scope::Both), platform.as_ref());
    apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(fixture.path("sorted").join("alpha.txt").is_file());

    // Something else moves in before the undo.
    std::fs::write(fixture.path("sorted").join("notes.txt"), b"mine").unwrap();

    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undo.is_complete(), "the file itself comes back");
    assert!(fixture.path("alpha.txt").is_file());
    assert_eq!(undo.removed_dirs, Vec::<std::path::PathBuf>::new());
    assert_eq!(undo.kept_dirs, vec![fixture.path("sorted")]);
    assert!(fixture.path("sorted").join("notes.txt").is_file());
}

/// A rename into a subfolder is simulated without creating anything.
#[test]
fn simulating_a_subfolder_move_creates_no_folders() {
    use ren_core::ops::{AddRemove, AddRemoveMode};

    let platform = ren_platform::host();
    let fixture = Fixture::new(&["alpha.txt"]);
    let op = AddRemove {
        mode: AddRemoveMode::Add,
        insert: "sorted<\\>".into(),
        add_pos: 0,
        ..Default::default()
    };
    let entries = fixture.entries();
    let plan = plan(&entries, &pipeline_of(op, Scope::Both), platform.as_ref());

    let options = ApplyOptions {
        simulate: true,
        ..fixture.options()
    };
    let report = apply(&plan, platform.as_ref(), &options).unwrap();
    assert_eq!(report.created_dirs, vec![fixture.path("sorted")]);
    assert!(!fixture.path("sorted").exists());
}

// --- M4 acceptance -----------------------------------------------------------

/// The milestone's own criterion: *"a 3-op pipeline previews composed result
/// live, executes atomically, undoes as one transaction."*
///
/// The interesting assertions are the counting ones. Three operations over two
/// files is **one** journal and **two** renames, not six — because `plan()`
/// composes the steps into one final name per file before anything is written.
#[test]
fn a_three_operation_pipeline_executes_and_undoes_as_one_transaction() {
    use ren_core::ops::{AddCounter, CaseMode, Casing, CounterPlacement, Replace};
    use ren_core::{CounterSetup, RunSettings};

    let platform = ren_platform::host();
    let fixture = Fixture::new(&["my_holiday_photo.JPG", "another_one.JPG"]);
    let before = snapshot(fixture.dir.path());

    let mut pipeline = Pipeline::new()
        .then(Replace::new("_", " "))
        .then_scoped(Casing::new(CaseMode::Title), Scope::Name)
        .then(AddCounter::new(CounterPlacement::First, ". "));
    pipeline.settings = RunSettings {
        counter: CounterSetup {
            auto_pad: false,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut entries = fixture.entries();
    entries.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    let plan = plan(&entries, &pipeline, platform.as_ref());

    // One composed name per file — not one per step.
    assert_eq!(plan.items.len(), 2);
    assert_eq!(plan.items[0].new_name, "1. Another One.JPG");
    assert_eq!(plan.items[1].new_name, "2. My Holiday Photo.JPG");
    assert!(plan.is_executable());

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(report.renamed.len(), 2, "two files, not six renames");

    // One transaction: exactly one journal, whatever the step count.
    let journals = Journal::list(fixture.journal.path()).unwrap();
    assert_eq!(journals.len(), 1, "a run is one transaction");
    let lines = Journal::read(&journals[0]).unwrap();
    assert_eq!(
        lines
            .iter()
            .filter(|l| matches!(l.record, Record::Begin { .. }))
            .count(),
        1
    );

    // And one undo puts everything back (P9).
    let undo = ren_core::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undo.is_complete(), "{:?}", undo.skipped);
    assert_eq!(snapshot(fixture.dir.path()), before);
    assert!(
        ren_core::undo_last(platform.as_ref(), fixture.journal.path()).is_err(),
        "one undo reverts one batch, and there was only one"
    );
}

/// Disabling one card takes it out of the composed result and leaves the rest
/// alone — the engine half of "reflected instantly in the preview".
#[test]
fn disabling_one_step_of_three_changes_only_that_step() {
    use ren_core::ops::{CaseMode, Casing, Replace};

    let platform = ren_platform::host();
    let fixture = Fixture::new(&["my_song.mp3"]);
    let entries = fixture.entries();

    let steps = |middle_enabled: bool| {
        Pipeline::new()
            .then(Replace::new("_", " "))
            .with(
                Step::Name(Box::new(Casing::new(CaseMode::Upper))),
                StepConfig {
                    enabled: middle_enabled,
                    ..StepConfig::scoped(Scope::Name)
                },
            )
            .then(Replace::new(" ", "-"))
    };

    let on = plan(&entries, &steps(true), platform.as_ref());
    assert_eq!(on.items[0].new_name, "MY-SONG.mp3");

    let off = plan(&entries, &steps(false), platform.as_ref());
    assert_eq!(off.items[0].new_name, "my-song.mp3");
}

/// Order is meaning: the same two operations the other way round give a
/// different name.
#[test]
fn reordering_two_operations_changes_the_result() {
    use ren_core::ops::{CaseMode, Casing, Replace};

    let platform = ren_platform::host();
    let fixture = Fixture::new(&["a_b.txt"]);
    let entries = fixture.entries();

    // Title case, then replace: the replacement text survives as typed.
    let first = Pipeline::new()
        .then_scoped(Casing::new(CaseMode::Title), Scope::Name)
        .then(Replace::new("_", " and "));
    assert_eq!(
        plan(&entries, &first, platform.as_ref()).items[0].new_name,
        "A and B.txt"
    );

    // Replace, then title case: the replacement is title-cased too.
    let second = Pipeline::new()
        .then(Replace::new("_", " and "))
        .then_scoped(Casing::new(CaseMode::Title), Scope::Name);
    assert_eq!(
        plan(&entries, &second, platform.as_ref()).items[0].new_name,
        "A And B.txt"
    );
}

/// D31's sharp edge, found by the M4 property tests: the folder a move wants
/// may already exist as an ordinary file. Renaming `(` to `(/(` cannot work,
/// and the executor would discover that halfway through the batch.
#[test]
fn a_subfolder_blocked_by_a_file_of_the_same_name_is_a_conflict() {
    let platform = ren_platform::host();
    let fixture = Fixture::new(&["report", "notes.txt"]);
    let entries = fixture.entries();

    // "notes.txt" -> "report/notes.txt", but "report" is a file.
    let pipeline = pipeline_of(
        MapNames::new(&[("notes.txt", "report/notes.txt")]),
        Scope::Both,
    );
    let plan = plan(&entries, &pipeline, platform.as_ref());

    let blocked = plan
        .items
        .iter()
        .find(|i| i.source.ends_with("notes.txt"))
        .expect("the moved file");
    assert!(
        matches!(
            blocked.state,
            RowState::Conflict(ConflictKind::BlockedByFile { .. })
        ),
        "{:?}",
        blocked.state
    );
    assert!(!plan.is_executable(), "P4: a conflict blocks the whole run");

    // Nothing was attempted, so nothing was half-done.
    assert!(apply(&plan, platform.as_ref(), &fixture.options()).is_err());
    assert_eq!(fixture.names(), ["notes.txt", "report"]);
}

// --- M5 acceptance -----------------------------------------------------------

/// CSV List Rename's worked example, end to end on real files.
///
/// M5 names this as acceptance, and it is worth having at this level as
/// well as at the unit level: it proves the list survives planning, conflict
/// detection and the executor, not merely `apply` on a string. It also pins
/// three behaviours the prose never states — matching is
/// case-insensitive by default (`Lorem` finds `lorem.txt`), the match is
/// against the stem so `.txt` survives, and an unmatched row (`amet`) and an
/// unmatched file (`not in list.txt`) are both silently left alone.
#[test]
fn the_csv_example_from_the_manual_renames_verbatim() {
    let fixture = Fixture::new(&[
        "dolor.txt",
        "ipsum.txt",
        "lorem.txt",
        "not in list.txt",
        "sit.txt",
    ]);
    let list = fixture.aux_file(
        "list.csv",
        "Old,New\nLorem,Some\nipsum,example\ndolor,text\nsit,I just made\namet,up\n",
    );

    let platform = ren_platform::host();
    let pipeline = pipeline_of(ren_core::ops::CsvList::new(&list), Scope::Name);
    let entries = fixture.entries();
    let plan = plan(&entries, &pipeline, platform.as_ref());

    assert_eq!(plan.changed(), 4, "one listed file is not in the list");
    assert_eq!(plan.conflicts(), 0);
    assert_eq!(plan.errors(), 0);

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert_eq!(report.renamed.len(), 4);
    assert_eq!(
        fixture.names(),
        [
            "I just made.txt",
            "Some.txt",
            "example.txt",
            "not in list.txt",
            "text.txt",
        ]
    );

    // P9: one apply is one transaction is one undo.
    let undone = undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert_eq!(undone.restored.len(), 4);
    assert_eq!(
        fixture.names(),
        [
            "dolor.txt",
            "ipsum.txt",
            "lorem.txt",
            "not in list.txt",
            "sit.txt",
        ]
    );
}

/// The Filename Editor's line count has to match the listing, and M5
/// makes a mismatch a validation error — so it must block the run through P4's
/// existing gate rather than needing one of its own.
#[test]
fn a_filename_editor_that_does_not_match_the_listing_blocks_the_run() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let platform = ren_platform::host();

    let short = pipeline_of(ren_core::ops::FilenameEditor::new("One\nTwo"), Scope::Name);
    let plan_short = plan(&fixture.entries(), &short, platform.as_ref());
    assert_eq!(plan_short.errors(), 3, "every row carries the reason");
    assert!(!plan_short.is_executable());
    assert!(apply(&plan_short, platform.as_ref(), &fixture.options()).is_err());
    assert_eq!(fixture.names(), ["a.txt", "b.txt", "c.txt"]);

    let exact = pipeline_of(
        ren_core::ops::FilenameEditor::new("One\nTwo\nThree"),
        Scope::Name,
    );
    let plan_exact = plan(&fixture.entries(), &exact, platform.as_ref());
    assert!(plan_exact.is_executable());
    apply(&plan_exact, platform.as_ref(), &fixture.options()).unwrap();
    assert_eq!(fixture.names(), ["One.txt", "Three.txt", "Two.txt"]);
}

// --- The side-effect seam ----------------------------------------------------
//
// Proved with a test-only action, before either real one exists. The trait is
// public, so the whole plan → journal → apply → undo path is provable without
// waiting for Set Attributes or Set Date to be written — and when they arrive,
// what is left to test about them is their *semantics* rather than the
// machinery underneath.

/// Sets the modified time to a fixed instant, for every file it is given.
#[derive(Debug)]
struct StampModified(SystemTime);

impl ren_core::ops::SideEffectAction for StampModified {
    fn id(&self) -> &'static str {
        "stamp_modified"
    }

    fn summary(&self) -> String {
        "Stamp the modified date".to_owned()
    }

    fn effect(&self, _cx: &EvalCx<'_>) -> Result<Option<Effect>, OpError> {
        Ok(Some(Effect::Times(TimeSet {
            modified: Some(TimeStamp::from_system(self.0)),
            ..Default::default()
        })))
    }

    fn describe(&self, _effect: &Effect) -> String {
        "Modified → a fixed instant".to_owned()
    }

    fn undoable(&self) -> Undoability {
        Undoability::Journaled
    }
}

/// Only ever asks for something Unix cannot do, so the pre-flight has a target.
///
/// `#[cfg(unix)]` because that is the only platform where it is unsupported —
/// on Windows the created date is perfectly writable, and
/// `setting_the_dates_changes_them_on_disk_and_undo_puts_them_back` covers it
/// there through the capability query.
#[cfg(unix)]
#[derive(Debug)]
struct StampCreated(SystemTime);

#[cfg(unix)]
impl ren_core::ops::SideEffectAction for StampCreated {
    fn id(&self) -> &'static str {
        "stamp_created"
    }
    fn summary(&self) -> String {
        "Stamp the created date".to_owned()
    }
    fn effect(&self, _cx: &EvalCx<'_>) -> Result<Option<Effect>, OpError> {
        Ok(Some(Effect::Times(TimeSet {
            created: Some(TimeStamp::from_system(self.0)),
            ..Default::default()
        })))
    }
    fn describe(&self, _effect: &Effect) -> String {
        "Created → a fixed instant".to_owned()
    }

    fn undoable(&self) -> Undoability {
        Undoability::Journaled
    }
}

fn stamp() -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000)
}

fn action_pipeline(action: impl ren_core::ops::SideEffectAction + 'static) -> Pipeline {
    Pipeline::new().with(Step::Action(Box::new(action)), StepConfig::default())
}

/// The failure this guards against is silent: an action-only row would land in
/// `Unchanged`, `plan.changed()` would be 0, `apply` would return early on an
/// empty `ops`, and the run would do nothing while reporting success.
#[test]
fn an_action_only_plan_is_not_a_no_op() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let platform = ren_platform::host();
    let pipeline = action_pipeline(StampModified(stamp()));
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());

    assert_eq!(plan.changed(), 0, "no name changes");
    assert_eq!(plan.acted(), 2);
    assert_eq!(plan.affected(), 2);
    assert_eq!(plan.unchanged(), 0, "acted rows are not untouched");
    assert!(
        !plan.ops.is_empty(),
        "or apply() returns early and does nothing"
    );
    assert!(plan.is_executable());

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert_eq!(report.acted.len(), 2);
    assert!(report.is_success());
    for name in ["a.txt", "b.txt"] {
        assert_eq!(
            std::fs::metadata(fixture.path(name))
                .unwrap()
                .modified()
                .unwrap(),
            stamp(),
        );
    }
}

/// M5's acceptance: *"journaled undo restores previous dates/attributes"*.
#[test]
fn journalled_undo_restores_the_previous_dates() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let platform = ren_platform::host();
    let before = mtimes(fixture.dir.path());

    let pipeline = action_pipeline(StampModified(stamp()));
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert_ne!(mtimes(fixture.dir.path()), before, "nothing happened");

    let report = undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert_eq!(report.reverted.len(), 2);
    assert!(report.is_complete(), "{:?}", report.skipped);
    assert_eq!(
        mtimes(fixture.dir.path()),
        before,
        "undo must put the previous dates back"
    );
}

/// P9: one apply is one transaction is one undo, even when the run both renames
/// and acts. The action runs on the file's *final* path, and undo takes it back
/// before the rename — which is what makes the recorded path still resolve.
#[test]
fn a_run_that_renames_and_acts_is_one_transaction_and_one_undo() {
    let fixture = Fixture::new(&["a.txt"]);
    let platform = ren_platform::host();
    let before = mtimes(fixture.dir.path());

    let pipeline = Pipeline::new()
        .then(ren_core::ops::Replace::new("a", "z"))
        .with(
            Step::Action(Box::new(StampModified(stamp()))),
            StepConfig::default(),
        );
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    assert_eq!(plan.changed(), 1);
    assert_eq!(plan.acted(), 1);
    assert_eq!(plan.affected(), 1, "one row, counted once");

    // The action is journalled against the name the file will have.
    assert!(
        plan.ops.iter().any(|op| matches!(
            op,
            PlannedOp::Act { path, .. } if path.ends_with("z.txt")
        )),
        "{:?}",
        plan.ops
    );

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert_eq!(report.renamed.len(), 1);
    assert_eq!(report.acted.len(), 1);
    assert_eq!(fixture.names(), ["z.txt"]);

    let undone = undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert!(undone.is_complete(), "{:?}", undone.skipped);
    assert_eq!(fixture.names(), ["a.txt"]);
    assert_eq!(mtimes(fixture.dir.path()), before);
}

/// Simulating must perform **no** syscall, including the before-image read —
/// that would be one `stat` per file for a run that writes nothing.
#[test]
fn simulating_an_action_reads_nothing_and_writes_nothing() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let platform = ren_platform::host();
    let before = mtimes(fixture.dir.path());

    let pipeline = action_pipeline(StampModified(stamp()));
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    let report = apply(
        &plan,
        platform.as_ref(),
        &ApplyOptions {
            simulate: true,
            journal_dir: fixture.journal.path().to_path_buf(),
            allow_irreversible: false,
        },
    )
    .unwrap();

    assert!(report.simulated);
    assert_eq!(report.acted.len(), 2, "it still says what it would do");
    assert!(report.txn.is_none());
    assert!(Journal::list(fixture.journal.path()).unwrap().is_empty());
    assert_eq!(mtimes(fixture.dir.path()), before);
}

/// The capability pre-flight (P4/P5). M5's Linux assertion, moved from
/// a run-time failure to a blocked plan: the alternative is opening a journal,
/// failing on file 1, and leaving the rest of a 10 000-file batch untouched
/// behind a half-open transaction.
#[cfg(unix)]
#[test]
fn setting_the_created_date_blocks_the_run_before_anything_is_written() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let platform = ren_platform::host();
    let before = mtimes(fixture.dir.path());

    let pipeline = action_pipeline(StampCreated(stamp()));
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());

    assert_eq!(plan.conflicts(), 2);
    assert!(!plan.is_executable());
    for item in &plan.items {
        match &item.state {
            RowState::Conflict(kind @ ConflictKind::Unsupported { capability, .. }) => {
                assert_eq!(*capability, ren_platform::Capability::CreatedTime);
                // The same words the platform layer itself would have used.
                assert!(kind.to_string().contains("is not supported on"), "{kind}");
            }
            other => panic!("expected an Unsupported conflict, got {other:?}"),
        }
    }

    assert!(apply(&plan, platform.as_ref(), &fixture.options()).is_err());
    assert!(Journal::list(fixture.journal.path()).unwrap().is_empty());
    assert_eq!(mtimes(fixture.dir.path()), before);
}

/// A crash between the write-ahead record and its confirmation must count the
/// action as in flight, exactly as it would a rename. `recover::unfinished`
/// used to count only `PlanRename`, so a crashed metadata batch looked
/// finished.
#[test]
fn an_announced_action_counts_as_in_flight() {
    let dir = TempDir::new().unwrap();
    let mut journal = Journal::create(dir.path()).unwrap();
    journal
        .write(Record::Begin {
            platform: "test".into(),
            items: 1,
        })
        .unwrap();
    // Written ahead, and then the process dies: no `Completed`, no `Commit`.
    journal
        .write(Record::PlanAct {
            seq: 0,
            path: "/x/a.txt".into(),
            op: "stamp_modified".into(),
            change: Effect::Times(TimeSet {
                modified: Some(TimeStamp { secs: 5, nanos: 0 }),
                ..Default::default()
            }),
            before: ren_core::Before::Times(TimeSet {
                modified: Some(TimeStamp { secs: 1, nanos: 0 }),
                ..Default::default()
            }),
        })
        .unwrap();

    let unfinished = ren_core::exec::unfinished(dir.path()).unwrap();
    assert_eq!(unfinished.len(), 1);
    assert_eq!(
        unfinished[0].in_flight.len(),
        1,
        "an announced-but-unconfirmed action is in flight"
    );
    assert_eq!(unfinished[0].in_flight[0].op, "stamp_modified");
    assert_eq!(
        unfinished[0].in_flight[0].path,
        std::path::PathBuf::from("/x/a.txt")
    );
    assert!(
        !unfinished[0].any_contents_rewritten(),
        "a metadata edit leaves the file's contents alone"
    );
}

// --- Set Attributes ----------------------------------------------------------

/// M5's acceptance: *"attributes actually change"* and *"journaled undo restores
/// previous attributes"*.
///
/// Read-only is the one bit Unix can write, so the assertion is real on both
/// runners; the three DOS bits are guarded by `supports` so the Windows runner
/// proves them and Linux does not pretend to.
#[test]
fn setting_an_attribute_changes_it_on_disk_and_undo_puts_it_back() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let platform = ren_platform::host();
    let before: Vec<_> = fixture
        .names()
        .iter()
        .map(|n| platform.get_attributes(&fixture.path(n)).unwrap())
        .collect();
    assert!(
        before.iter().all(|a| !a.read_only),
        "fixture starts writable"
    );

    // The documented CD example: clear write protection, leave the rest grey.
    let mut op = ren_core::ops::SetAttributes {
        read_only: Some(true),
        ..Default::default()
    };
    if platform.supports(ren_platform::Capability::HiddenAttribute) {
        op.hidden = Some(true);
    }

    let pipeline = Pipeline::new().with(Step::Action(Box::new(op)), StepConfig::default());
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    assert_eq!(plan.acted(), 2);
    assert!(plan.is_executable());

    apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    for name in ["a.txt", "b.txt"] {
        let now = platform.get_attributes(&fixture.path(name)).unwrap();
        assert!(now.read_only, "{name} should be read-only now");
        if platform.supports(ren_platform::Capability::HiddenAttribute) {
            assert!(now.hidden, "{name} should be hidden now");
        }
    }

    let report = undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert_eq!(report.reverted.len(), 2);
    assert!(report.is_complete(), "{:?}", report.skipped);
    for (name, was) in ["a.txt", "b.txt"].iter().zip(&before) {
        assert_eq!(
            platform.get_attributes(&fixture.path(name)).unwrap(),
            *was,
            "undo must put {name}'s attributes back"
        );
    }
}

/// A card with every box left grey does nothing at all — the same reading P34
/// gives Replace with an empty Find box.
#[test]
fn a_set_attributes_card_with_every_box_grey_touches_nothing() {
    let fixture = Fixture::new(&["a.txt"]);
    let platform = ren_platform::host();
    let pipeline = Pipeline::new().with(
        Step::Action(Box::new(ren_core::ops::SetAttributes::default())),
        StepConfig::default(),
    );
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());

    assert_eq!(plan.acted(), 0, "an empty effect produces no action");
    assert_eq!(plan.affected(), 0);
    assert!(plan.ops.is_empty());
}

// --- Set Date & Time ---------------------------------------------------------

fn set_date_to(when: chrono::NaiveDateTime, targets: ren_core::ops::DateTargets) -> Pipeline {
    let op = ren_core::ops::SetDate {
        date: ren_core::ops::WallClock::from_naive(when),
        targets,
        ..Default::default()
    };
    Pipeline::new().with(Step::Action(Box::new(op)), StepConfig::default())
}

fn wall(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, s)
        .unwrap()
}

/// M5's acceptance: *"all three date kinds actually change"* and *"journaled undo
/// restores previous dates"*.
///
/// Every stamp this platform can write is asked for, so the Windows runner
/// proves all three and Linux proves the two it has — rather than one body
/// asserting something untrue on either.
#[test]
fn setting_the_dates_changes_them_on_disk_and_undo_puts_them_back() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let platform = ren_platform::host();
    let before: Vec<_> = ["a.txt", "b.txt"]
        .iter()
        .map(|n| platform.get_times(&fixture.path(n)).unwrap())
        .collect();

    let targets = ren_core::ops::DateTargets {
        created: platform.supports(ren_platform::Capability::CreatedTime),
        accessed: true,
        modified: true,
    };
    let when = wall(2008, 2, 17, 11, 23, 50);
    let pipeline = set_date_to(when, targets);
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    assert_eq!(plan.acted(), 2);
    assert!(plan.is_executable(), "{:?}", plan.items[0].state);

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert_eq!(report.acted.len(), 2);

    let expected = ren_core::datetime::localise(&chrono::Local, when).unwrap();
    for name in ["a.txt", "b.txt"] {
        let now = platform.get_times(&fixture.path(name)).unwrap();
        for (stamp, which, asked) in [
            (now.created, "created", targets.created),
            (now.accessed, "accessed", targets.accessed),
            (now.modified, "modified", targets.modified),
        ] {
            if !asked {
                continue;
            }
            let stamp = ren_core::TimeStamp::from_system(stamp.expect("reported"));
            assert_eq!(
                stamp.secs, expected.secs,
                "{name}'s {which} date should have changed"
            );
        }
    }

    let undone = undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    assert_eq!(undone.reverted.len(), 2);
    assert!(undone.is_complete(), "{:?}", undone.skipped);
    for (name, was) in ["a.txt", "b.txt"].iter().zip(&before) {
        let now = platform.get_times(&fixture.path(name)).unwrap();
        assert_eq!(
            now.modified.map(ren_core::TimeStamp::from_system),
            was.modified.map(ren_core::TimeStamp::from_system),
            "undo must put {name}'s modified date back"
        );
        if targets.created {
            assert_eq!(
                now.created.map(ren_core::TimeStamp::from_system),
                was.created.map(ren_core::TimeStamp::from_system),
                "undo must put {name}'s created date back"
            );
        }
    }
}

/// The component mask, on a real file: only the year moves.
#[test]
fn changing_only_the_year_leaves_the_rest_of_the_date_alone() {
    let fixture = Fixture::new(&["a.txt"]);
    let platform = ren_platform::host();

    // Give the file a date we can reason about.
    let start = wall(1999, 5, 9, 10, 18, 5);
    apply(
        &plan(
            &fixture.entries(),
            &set_date_to(start, ren_core::ops::DateTargets::default()),
            platform.as_ref(),
        ),
        platform.as_ref(),
        &fixture.options(),
    )
    .unwrap();

    let op = ren_core::ops::SetDate {
        date: ren_core::ops::WallClock::from_naive(wall(2008, 2, 17, 11, 23, 50)),
        change: ren_core::DateComponents {
            year: true,
            month: false,
            day: false,
            hour: false,
            minute: false,
            second: false,
        },
        ..Default::default()
    };
    let pipeline = Pipeline::new().with(Step::Action(Box::new(op)), StepConfig::default());
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    apply(&plan, platform.as_ref(), &fixture.options()).unwrap();

    let now = platform.get_times(&fixture.path("a.txt")).unwrap();
    let expected =
        ren_core::datetime::localise(&chrono::Local, wall(2008, 5, 9, 10, 18, 5)).unwrap();
    assert_eq!(
        ren_core::TimeStamp::from_system(now.modified.unwrap()).secs,
        expected.secs,
        "only the year should have moved"
    );
}

// --- Budget ------------------------------------------------------------------
//
// The CI perf gate cannot see any of M5's work: `spike_preview_headless` drives
// `ren_gui::spike`, which builds its own hand-rolled step and never touches
// `OpKind` or `plan()`, and the criterion bench is `harness = false` so
// `cargo test` never runs it. These are plain timing tests in the same spirit
// as the GUI's — generous, because a debug build on a shared runner is slow,
// but they would catch a per-file file read or a per-file reparse, which is
// what they exist for.

fn many(dir: &Path, count: usize) -> Vec<ren_core::FileEntry> {
    for i in 0..count {
        std::fs::write(dir.join(format!("track_{i:05}.mp3")), b"").unwrap();
    }
    list(dir, ListOptions::default()).unwrap()
}

#[test]
fn an_action_pipeline_plans_ten_thousand_files_inside_the_budget() {
    let dir = TempDir::new().unwrap();
    let entries = many(dir.path(), 10_000);
    let platform = ren_platform::host();
    let pipeline = set_date_to(
        wall(2008, 2, 17, 11, 23, 50),
        ren_core::ops::DateTargets::default(),
    );

    let started = std::time::Instant::now();
    let plan = plan(&entries, &pipeline, platform.as_ref());
    let elapsed = started.elapsed();

    assert_eq!(plan.acted(), 10_000);
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "planning 10 000 files with an action took {elapsed:?}"
    );
}

/// The one that would catch a CSV re-read per file: 10 000 files against a
/// 10 000-row list is 100 million comparisons if the lookup is a scan, and
/// 10 000 file reads if the parse is not shared.
#[test]
fn a_csv_pipeline_plans_ten_thousand_files_against_a_ten_thousand_row_list() {
    let dir = TempDir::new().unwrap();
    let entries = many(dir.path(), 10_000);
    let list_dir = TempDir::new().unwrap();
    let csv = list_dir.path().join("list.csv");
    let body: String = (0..10_000)
        .map(|i| format!("track_{i:05},renamed_{i:05}\n"))
        .collect();
    std::fs::write(&csv, body).unwrap();

    let platform = ren_platform::host();
    let pipeline = pipeline_of(ren_core::ops::CsvList::new(&csv), Scope::Name);

    let started = std::time::Instant::now();
    let plan = plan(&entries, &pipeline, platform.as_ref());
    let elapsed = started.elapsed();

    assert_eq!(plan.changed(), 10_000);
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "planning 10 000 files against a 10 000-row list took {elapsed:?}"
    );
}

// --- P2: changes that cannot be taken back -----------------------------------
//
// Proved here with a test-only action, before the first real one exists. M6's
// Music Tagger and Remove Tags rewrite a file's contents and keep nothing, so
// there is no journal entry that could put it back. The machinery that has to
// be right is the machinery that *refuses* — and it is much easier to trust
// after it has been watched refusing something.

/// Claims to be irreversible while actually only setting the modified date, so
/// a test can watch the engine's behaviour without needing a real tag writer.
#[derive(Debug)]
struct PretendIrreversible(SystemTime);

impl ren_core::ops::SideEffectAction for PretendIrreversible {
    fn id(&self) -> &'static str {
        "pretend_irreversible"
    }
    fn summary(&self) -> String {
        "Do something that cannot be undone".to_owned()
    }
    fn effect(&self, _cx: &EvalCx<'_>) -> Result<Option<Effect>, OpError> {
        Ok(Some(Effect::Times(TimeSet {
            modified: Some(TimeStamp::from_system(self.0)),
            ..Default::default()
        })))
    }
    fn describe(&self, _effect: &Effect) -> String {
        "Rewrote the file".to_owned()
    }
    fn undoable(&self) -> Undoability {
        Undoability::None
    }
}

fn irreversible_pipeline() -> Pipeline {
    Pipeline::new().with(
        Step::Action(Box::new(PretendIrreversible(stamp()))),
        StepConfig::default(),
    )
}

/// P2 as an engine invariant: an irreversible run is refused **before** a
/// journal exists, so a caller who has not thought about it fails closed.
#[test]
fn an_irreversible_run_is_refused_until_it_is_explicitly_allowed() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let platform = ren_platform::host();
    let before = mtimes(fixture.dir.path());

    let plan = plan(
        &fixture.entries(),
        &irreversible_pipeline(),
        platform.as_ref(),
    );
    assert_eq!(plan.undoability(), Undoability::None);
    assert_eq!(plan.irreversible(), 2);
    assert!(
        plan.is_executable(),
        "it is runnable, just not without asking"
    );

    let err = apply(&plan, platform.as_ref(), &fixture.options())
        .expect_err("must refuse without permission");
    assert!(
        matches!(err, ren_core::ExecError::Irreversible { items: 2 }),
        "{err:?}"
    );
    // Nothing was touched, and no journal was even opened.
    assert_eq!(mtimes(fixture.dir.path()), before);
    assert!(Journal::list(fixture.journal.path()).unwrap().is_empty());

    // With consent it runs.
    let report = apply(
        &plan,
        platform.as_ref(),
        &fixture.options_allowing_irreversible(),
    )
    .unwrap();
    assert_eq!(report.acted.len(), 2);
    assert_ne!(mtimes(fixture.dir.path()), before);
}

/// The sharpest trap M6 had to survive. Undoing a batch that contains an
/// irreversible change must report it as such — **not** as a skip. A skip means
/// "something looked wrong so I did not touch it", makes `is_complete()` false,
/// and makes `ren-cli undo` exit non-zero for a batch whose renames came back
/// perfectly.
#[test]
fn undoing_an_irreversible_change_reports_it_rather_than_failing() {
    let fixture = Fixture::new(&["a.txt"]);
    let platform = ren_platform::host();

    let pipeline = Pipeline::new()
        .then(ren_core::ops::Replace::new("a", "z"))
        .with(
            Step::Action(Box::new(PretendIrreversible(stamp()))),
            StepConfig::default(),
        );
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    assert_eq!(plan.changed(), 1);
    assert_eq!(plan.irreversible(), 1);

    apply(
        &plan,
        platform.as_ref(),
        &fixture.options_allowing_irreversible(),
    )
    .unwrap();
    assert_eq!(fixture.names(), ["z.txt"]);

    let report = undo_last(platform.as_ref(), fixture.journal.path()).unwrap();
    // The rename came back...
    assert_eq!(report.restored.len(), 1);
    assert_eq!(fixture.names(), ["a.txt"]);
    // ...the tag write is named as unrecoverable...
    assert_eq!(report.irreversible.len(), 1);
    assert!(
        report.irreversible[0].1.contains("cannot be undone"),
        "{:?}",
        report.irreversible
    );
    // ...and none of that is a failure.
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert!(
        report.is_complete(),
        "an irreversible change is not an incomplete undo"
    );
}

/// A crash between the write-ahead record and its confirmation must count an
/// irreversible change as in flight, exactly as it does the other two kinds.
#[test]
fn an_announced_irreversible_change_counts_as_in_flight() {
    let dir = TempDir::new().unwrap();
    let mut journal = Journal::create(dir.path()).unwrap();
    journal
        .write(Record::Begin {
            platform: "test".into(),
            items: 1,
        })
        .unwrap();
    journal
        .write(Record::PlanIrreversible {
            seq: 0,
            path: "/x/a.mp3".into(),
            op: "music_tagger".into(),
            change: Effect::Times(TimeSet::default()),
        })
        .unwrap();

    let unfinished = ren_core::exec::unfinished(dir.path()).unwrap();
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].in_flight.len(), 1);
    // The whole point of naming them: this is the file that may be
    // half-written, and it is the only one in the batch that is.
    assert_eq!(
        unfinished[0].in_flight[0].path,
        std::path::PathBuf::from("/x/a.mp3")
    );
    assert_eq!(unfinished[0].in_flight[0].op, "music_tagger");
    assert!(
        unfinished[0].any_contents_rewritten(),
        "a tag write rewrites the file in place, so it may be half-written"
    );
}

/// Worst-case, not first-case: a batch that renames a thousand files and
/// rewrites one tag block is a batch that cannot be fully undone, and that is
/// the sentence the confirmation has to say.
#[test]
fn a_runs_undoability_is_its_worst_action() {
    let fixture = Fixture::new(&["a.txt"]);
    let platform = ren_platform::host();

    let both = Pipeline::new()
        .with(
            Step::Action(Box::new(StampModified(stamp()))),
            StepConfig::default(),
        )
        .with(
            Step::Action(Box::new(PretendIrreversible(stamp()))),
            StepConfig::default(),
        );
    let mixed = plan(&fixture.entries(), &both, platform.as_ref());
    assert_eq!(mixed.undoability(), Undoability::None);

    // And a run with no actions at all is fully undoable.
    let renames = pipeline_of(ren_core::ops::Replace::new("a", "z"), Scope::Name);
    let only_renames = plan(&fixture.entries(), &renames, platform.as_ref());
    assert_eq!(only_renames.undoability(), Undoability::Full);
}

/// Every operation the engine knows must agree with its own effect about
/// whether it can be taken back — the carried value and the derived one are two
/// readings of one fact, and they must not drift.
#[test]
fn every_action_agrees_with_its_effect_about_undoability() {
    let entry = ren_core::FileEntry::synthetic("/files/a.txt");
    let cx = EvalCx::simple(&entry, 0, 1);
    let mut checked = 0;

    for op in ren_core::OpKind::all() {
        let ren_core::ops::StepRef::Action(action) = op.as_step() else {
            continue;
        };
        // Configured to actually do something, or `effect` returns None.
        let effect = match op {
            ren_core::OpKind::SetAttributes(_) => {
                Effect::Attributes(ren_platform::AttributeChange {
                    read_only: Some(false),
                    ..Default::default()
                })
            }
            _ => match action.effect(&cx) {
                Ok(Some(effect)) => effect,
                _ => continue,
            },
        };
        assert_eq!(
            action.undoable(),
            effect.undoability(),
            "{} disagrees with its own effect",
            op.name()
        );
        checked += 1;
    }
    assert!(checked > 0, "no action was actually checked");
}

/// The one that would catch a music tag being read per keystroke instead of
/// once.
///
/// The CI perf gate cannot: it drives synthetic `FileEntry`s over paths that do
/// not exist, so no amount of per-file IO would move it. These are real files
/// on disk, read through the real resolver, and the second pass is the one that
/// matters — it is what a keystroke costs once the cache is warm.
#[test]
fn a_music_pipeline_reads_ten_thousand_files_once_not_once_per_keystroke() {
    use ren_core::meta::testing::Mp3;

    let dir = TempDir::new().unwrap();
    for i in 0..10_000 {
        // Tags without audio frames: ~40 bytes each, and the whole read path
        // still runs. The corpus is written in well under a second.
        Mp3 {
            id3v2: vec![("TPE1", format!("Artist {i:05}")), ("TIT2", "Title".into())],
            audio: false,
            ..Default::default()
        }
        .write(dir.path(), &format!("track_{i:05}.mp3"));
    }
    let entries = list(dir.path(), ListOptions::default()).unwrap();
    assert_eq!(entries.len(), 10_000);

    let platform = ren_platform::host();
    let pipeline = pipeline_of(
        ren_core::ops::FreeFormat::new("<Artist> - <Title>"),
        Scope::Name,
    );

    ren_core::meta::audio::forget_all();
    let started = std::time::Instant::now();

    let before = ren_core::meta::audio::parses_so_far();
    let first = plan(&entries, &pipeline, platform.as_ref());
    let cold = ren_core::meta::audio::parses_so_far() - before;
    assert_eq!(
        first.changed(),
        10_000,
        "every file has tags to rename from"
    );
    assert!(cold >= 10_000, "the first pass has to actually read them");

    let before = ren_core::meta::audio::parses_so_far();
    let second = plan(&entries, &pipeline, platform.as_ref());
    let warm = ren_core::meta::audio::parses_so_far() - before;
    assert_eq!(second.changed(), 10_000);

    // A count, not a timing ratio: the planner's own work dominates either way,
    // so a stopwatch cannot tell a warm cache from a busy machine. The slack is
    // for other tests reading their own handful of files in parallel — the
    // failure this guards against is ten thousand, not ten.
    assert!(
        warm < 100,
        "a second pass re-read {warm} files — the cache is not holding, which \
         means every keystroke re-parses the whole folder"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "two passes over 10 000 tagged files took {:?}",
        started.elapsed()
    );
}

// --- M6 step 5: the two irreversible operations ------------------------------

/// A minimal FLAC — `fLaC` plus a STREAMINFO block. Enough for lofty to call it
/// one, which is all these tests need it to be.
fn minimal_flac() -> Vec<u8> {
    let mut out = Vec::from(*b"fLaC");
    let mut info = Vec::new();
    info.extend_from_slice(&4096u16.to_be_bytes());
    info.extend_from_slice(&4096u16.to_be_bytes());
    info.extend_from_slice(&[0, 0, 0]);
    info.extend_from_slice(&[0, 0, 0]);
    // 20 bits rate | 3 bits channels-1 | 5 bits bits-per-sample-1 | 36 bits samples
    let packed: u64 = (44_100u64 << 44) | (1u64 << 41) | (15u64 << 36) | 44_100;
    info.extend_from_slice(&packed.to_be_bytes());
    info.extend_from_slice(&[0u8; 16]);
    out.push(0x80);
    out.extend_from_slice(&(info.len() as u32).to_be_bytes()[1..]);
    out.extend_from_slice(&info);
    out
}

fn tagger_pipeline(op: ren_core::ops::MusicTagger) -> Pipeline {
    Pipeline::new().with(Step::Action(Box::new(op)), StepConfig::default())
}

/// M6's headline acceptance for the tagger, through the *real* executor rather
/// than the write primitives: the filename becomes the tags.
#[test]
fn the_tagger_writes_the_filename_into_the_file() {
    use ren_core::PartsSpec;
    use ren_core::meta::testing::Mp3;
    use ren_core::ops::MusicTagger;
    use ren_core::template::TextTemplate;

    let fixture = Fixture::new(&[]);
    Mp3::tagged("Wrong", "Wrong")
        .frame("TALB", "Keep This Album")
        .write(fixture.dir.path(), "Metallica - One.mp3");
    ren_core::meta::audio::forget_all();

    let op = MusicTagger {
        artist: Some(TextTemplate::new("<%1>")),
        title: Some(TextTemplate::new("<%2>")),
        ..Default::default()
    };
    let mut pipeline = tagger_pipeline(op);
    pipeline.settings = ren_core::RunSettings {
        parts: PartsSpec::new("<%1> - <%2>"),
        ..Default::default()
    };
    let platform = ren_platform::host();
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());

    assert_eq!(plan.irreversible(), 1);
    let report = apply(
        &plan,
        platform.as_ref(),
        &fixture.options_allowing_irreversible(),
    )
    .expect("allowed");
    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(report.acted.len(), 1);
    assert_eq!(report.acted[0].1, "Title → One, Artist → Metallica");

    ren_core::meta::audio::forget_all();
    let tags = ren_core::meta::audio::tags_of(&fixture.path("Metallica - One.mp3")).unwrap();
    assert_eq!(tags.artist.as_deref(), Some("Metallica"));
    assert_eq!(tags.title.as_deref(), Some("One"));
    // And the field nobody enabled is exactly where it was.
    assert_eq!(tags.album.as_deref(), Some("Keep This Album"));
    // The name is untouched: this operation writes tags, it does not rename.
    assert_eq!(fixture.names(), ["Metallica - One.mp3"]);
}

/// The untagger's acceptance criterion, stated as acceptance: *"untagger leaves
/// audio stream intact (byte-compare past tag blocks)"*.
#[test]
fn the_untagger_strips_tags_and_leaves_the_audio_byte_identical() {
    use ren_core::meta::testing::{Mp3, mpeg_frames};
    use ren_core::ops::RemoveTags;

    let fixture = Fixture::new(&[]);
    let mut mp3 = Mp3::tagged("A", "B").frame("TALB", "C");
    mp3.id3v1 = Some(("T".into(), "A".into(), "Al".into(), "1999".into(), None));
    mp3.write(fixture.dir.path(), "t.mp3");
    ren_core::meta::audio::forget_all();

    let op = RemoveTags {
        id3v1: true,
        id3v2: true,
        lyrics: true,
    };
    let platform = ren_platform::host();
    let plan = plan(
        &fixture.entries(),
        &Pipeline::new().with(Step::Action(Box::new(op)), StepConfig::default()),
        platform.as_ref(),
    );
    let report = apply(
        &plan,
        platform.as_ref(),
        &fixture.options_allowing_irreversible(),
    )
    .expect("allowed");
    assert!(report.is_success(), "{:?}", report.failed);

    assert_eq!(
        std::fs::read(fixture.path("t.mp3")).unwrap(),
        mpeg_frames(),
        "the audio stream is not what it was"
    );
}

/// **The crash guard.** An empty tag of a type a format cannot carry skips
/// lofty's own writability check — that check is `!is_writable() &&
/// !is_empty()`, so an empty tag sails past it — and reaches a writer which
/// dispatches on the *file's* format. An empty ID3v1 saved to a FLAC lands on
/// `unreachable!("tag type verified beforehand")` and takes the process with
/// it, mid-batch, in the one operation that has no undo.
///
/// A user with a mixed folder and the ID3v1 box ticked is not an exotic case;
/// it is the ordinary one.
///
/// `meta::write::remove_tags` stops it twice — a `tag_support` check and a
/// presence check — and either alone is enough, which is deliberate for a
/// change nobody can take back. Removing *both* makes this test fail with
/// lofty's panic verbatim; removing either one leaves it passing.
#[test]
fn removing_a_tag_a_format_cannot_carry_skips_it_rather_than_crashing() {
    use ren_core::meta::testing::Mp3;
    use ren_core::ops::RemoveTags;

    let fixture = Fixture::new(&[]);
    std::fs::write(fixture.path("a.flac"), minimal_flac()).unwrap();
    Mp3::tagged("A", "B").write(fixture.dir.path(), "b.mp3");
    let flac_before = std::fs::read(fixture.path("a.flac")).unwrap();
    ren_core::meta::audio::forget_all();

    let op = RemoveTags {
        id3v1: true,
        id3v2: true,
        lyrics: true,
    };
    let platform = ren_platform::host();
    let plan = plan(
        &fixture.entries(),
        &Pipeline::new().with(Step::Action(Box::new(op)), StepConfig::default()),
        platform.as_ref(),
    );
    let report = apply(
        &plan,
        platform.as_ref(),
        &fixture.options_allowing_irreversible(),
    )
    .expect("allowed");

    assert!(report.is_success(), "{:?}", report.failed);
    assert_eq!(
        std::fs::read(fixture.path("a.flac")).unwrap(),
        flac_before,
        "the FLAC should have been left exactly alone"
    );
    // And the MP3 beside it really was processed, so this is not passing by
    // doing nothing at all.
    ren_core::meta::audio::forget_all();
    assert!(
        ren_core::meta::audio::tags_of(&fixture.path("b.mp3"))
            .unwrap()
            .artist
            .is_none()
    );
}

/// The mixed batch D54 exists for: renames come back, the tag write is reported
/// as irreversible rather than as a skip, and the undo still succeeds.
#[test]
fn undo_after_a_mixed_batch_restores_the_names_and_reports_the_tag_write() {
    use ren_core::meta::testing::Mp3;
    use ren_core::ops::{MusicTagger, Replace};
    use ren_core::template::TextTemplate;

    let fixture = Fixture::new(&[]);
    Mp3::tagged("Old", "Old").write(fixture.dir.path(), "song.mp3");
    ren_core::meta::audio::forget_all();

    let op = MusicTagger {
        artist: Some(TextTemplate::new("Written")),
        ..Default::default()
    };
    let pipeline = Pipeline::new()
        .with(
            Step::Name(Box::new(Replace::new("song", "tune"))),
            StepConfig::default(),
        )
        .with(Step::Action(Box::new(op)), StepConfig::default());

    let platform = ren_platform::host();
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    let report = apply(
        &plan,
        platform.as_ref(),
        &fixture.options_allowing_irreversible(),
    )
    .expect("allowed");
    assert_eq!(fixture.names(), ["tune.mp3"]);
    ren_core::meta::audio::forget_all();
    assert_eq!(
        ren_core::meta::audio::tags_of(&fixture.path("tune.mp3"))
            .unwrap()
            .artist
            .as_deref(),
        Some("Written")
    );

    let undone =
        ren_core::exec::undo_transaction(&report.journal.clone().unwrap(), platform.as_ref())
            .expect("undo runs");

    assert_eq!(
        fixture.names(),
        ["song.mp3"],
        "the rename did not come back"
    );
    assert_eq!(undone.irreversible.len(), 1, "{undone:?}");
    assert!(
        undone.skipped.is_empty(),
        "an irreversible change is not a skip (D54): {undone:?}"
    );
    assert!(
        undone.is_complete(),
        "a correct undo must still exit SUCCESS: {undone:?}"
    );
    // The tag is still what the run wrote — that is what irreversible means.
    ren_core::meta::audio::forget_all();
    assert_eq!(
        ren_core::meta::audio::tags_of(&fixture.path("song.mp3"))
            .unwrap()
            .artist
            .as_deref(),
        Some("Written")
    );
}

/// P46: actions run after every rename, so a file that moved into a subfolder
/// is tagged at the path it ended up at, not the one it started from.
#[test]
fn a_file_that_moved_is_tagged_where_it_landed() {
    use ren_core::meta::testing::Mp3;
    use ren_core::ops::{FreeFormat, MusicTagger};
    use ren_core::template::TextTemplate;

    let fixture = Fixture::new(&[]);
    Mp3::tagged("Old", "Old").write(fixture.dir.path(), "song.mp3");
    ren_core::meta::audio::forget_all();

    let op = MusicTagger {
        artist: Some(TextTemplate::new("Written")),
        ..Default::default()
    };
    let pipeline = Pipeline::new()
        .with(
            Step::Name(Box::new(FreeFormat::new("albums<\\><Name>"))),
            StepConfig::scoped(Scope::Name),
        )
        .with(Step::Action(Box::new(op)), StepConfig::default());

    let platform = ren_platform::host();
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    let report = apply(
        &plan,
        platform.as_ref(),
        &fixture.options_allowing_irreversible(),
    )
    .expect("allowed");
    assert!(report.is_success(), "{:?}", report.failed);

    let moved = fixture.dir.path().join("albums").join("song.mp3");
    assert!(moved.is_file(), "expected {}", moved.display());
    ren_core::meta::audio::forget_all();
    assert_eq!(
        ren_core::meta::audio::tags_of(&moved)
            .unwrap()
            .artist
            .as_deref(),
        Some("Written")
    );
}

/// D44's rule, one level deeper than M5 took it: a record *kind* we know can
/// carry a payload we do not.
///
/// Without `Effect::Unknown` this line fails to deserialise, `Journal::read`
/// calls the whole file corrupt, and — because the GUI reads every journal at
/// startup — the application does not open. One file from a future build would
/// have been enough.
#[test]
fn a_journal_from_a_newer_build_still_reads_and_is_not_acted_on() {
    let fixture = Fixture::new(&[]);
    let path = fixture.journal.path().join("future.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"txn":"t","n":0,"v":9,"kind":"begin","platform":"unix","items":1}"#,
            "\n",
            r#"{"txn":"t","n":1,"v":9,"kind":"plan_irreversible","seq":0,"path":"/tmp/x.mp3","op":"transmute_tags","change":{"effect":"transmute","fields":[{"whatever":1}]}}"#,
            "\n",
            r#"{"txn":"t","n":2,"v":9,"kind":"completed","seq":0}"#,
            "\n",
        ),
    )
    .unwrap();

    let lines = Journal::read(&path).expect("a newer journal must still read");
    assert_eq!(lines.len(), 3);

    let act = lines
        .iter()
        .find(|l| matches!(l.record, Record::PlanIrreversible { .. }))
        .expect("the record kind is one we know");
    let Record::PlanIrreversible { change, .. } = &act.record else {
        unreachable!()
    };
    assert_eq!(
        *change,
        Effect::Unknown,
        "the payload should degrade, not fail"
    );
    assert!(
        !act.understood(),
        "and a line from the future must not be acted on"
    );
    // The safest reading of a change we cannot name.
    assert_eq!(change.undoability(), Undoability::None);
    assert!(
        !change.is_empty(),
        "unknown is not the same as nothing to do"
    );
}

/// The other half of D73, and the one that is easy to undo by accident.
///
/// `Effect::Unknown` covers a payload *shape* this build does not know. It does
/// nothing about a payload shape it does know that has grown a **field** — and
/// `#[serde(deny_unknown_fields)]`, which every operation struct carries for
/// good reason, turns exactly that into `CorruptJournal`. A journal is read by
/// builds other than the one that wrote it, and the read runs as the app opens,
/// so one added field in a later version would stop this one starting.
///
/// The rule: operation structs are strict, journal payloads are tolerant.
#[test]
fn a_journal_payload_that_has_grown_a_field_still_reads() {
    let fixture = Fixture::new(&[]);
    let path = fixture.journal.path().join("grown.jsonl");

    for (what, line) in [
        (
            "a field inside TimeSet",
            r#"{"txn":"t","n":1,"v":3,"kind":"plan_act","seq":0,"path":"/x","op":"set_date","change":{"effect":"times","modified":{"secs":1,"nanos":0}},"before":{"of":"times","modified":{"secs":0,"nanos":0},"leap_seconds":3}}"#,
        ),
        (
            "a field inside FieldWrite",
            r#"{"txn":"t","n":1,"v":4,"kind":"plan_irreversible","seq":0,"path":"/x","op":"music_tagger","change":{"effect":"write_tags","fields":[{"field":"artist","value":"A","encoding":"utf16"}]}}"#,
        ),
        (
            "a field inside AttributeChange",
            r#"{"txn":"t","n":1,"v":3,"kind":"plan_act","seq":0,"path":"/x","op":"set_attributes","change":{"effect":"attributes","hidden":true,"compressed":false},"before":{"of":"attributes","hidden":false}}"#,
        ),
    ] {
        std::fs::write(&path, format!("{line}\n")).unwrap();
        Journal::read(&path).unwrap_or_else(|e| panic!("{what}: {e}"));
    }

    // And the strictness that *is* wanted is still there, on the job-file side:
    // a typo in a preset is an error rather than a silently ignored key.
    assert!(
        toml::from_str::<ren_core::ops::RemoveTags>("id3v1 = true\nid3v3 = true").is_err(),
        "operation structs must stay strict"
    );
}

// --- M6 step 6: recovery, undo slots, and reporting --------------------------

/// The window between the rename syscall and its `Completed` record.
///
/// `apply` renames and *then* journals the completion, so a crash in between
/// leaves the file under its **new** name with nothing saying it landed. Undo's
/// reverse walk iterated `completed` only, so those files stayed renamed for
/// good — while `Unfinished.in_flight`'s doc comment promised *"a file that may
/// be under either name — recovery checks before touching it"*. Nothing checked.
#[test]
fn a_rename_interrupted_before_its_completed_record_is_rolled_back() {
    let fixture = Fixture::new(&[]);
    // The state a crash in that window leaves: the file is under its new name.
    std::fs::write(fixture.path("b.txt"), b"contents").unwrap();

    let mut journal = Journal::create(fixture.journal.path()).unwrap();
    journal
        .write(Record::Begin {
            platform: "test".into(),
            items: 1,
        })
        .unwrap();
    journal
        .write(Record::PlanRename {
            seq: 0,
            from: fixture.path("a.txt"),
            to: fixture.path("b.txt"),
        })
        .unwrap();
    // No `Completed`, no `Commit` — the process died here.
    drop(journal);

    let found = ren_core::exec::unfinished(fixture.journal.path()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].in_flight.len(), 1);

    let report = ren_core::exec::rollback(&found[0], ren_platform::host().as_ref()).unwrap();
    assert_eq!(
        fixture.names(),
        ["a.txt"],
        "the interrupted rename was left in place"
    );
    assert_eq!(report.restored.len(), 1);
    assert!(report.is_complete());
}

/// The other side of it: a rename announced and never actually performed leaves
/// nothing to do, and must not be reported as a skip — there is nothing about
/// it to report.
#[test]
fn a_rename_that_never_happened_is_not_reported_as_a_skip() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut journal = Journal::create(fixture.journal.path()).unwrap();
    journal
        .write(Record::Begin {
            platform: "test".into(),
            items: 1,
        })
        .unwrap();
    journal
        .write(Record::PlanRename {
            seq: 0,
            from: fixture.path("a.txt"),
            to: fixture.path("b.txt"),
        })
        .unwrap();
    drop(journal);

    let found = ren_core::exec::unfinished(fixture.journal.path()).unwrap();
    let report = ren_core::exec::rollback(&found[0], ren_platform::host().as_ref()).unwrap();
    assert_eq!(fixture.names(), ["a.txt"], "nothing should have moved");
    assert!(report.restored.is_empty());
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert!(report.is_complete());
}

/// D54's bucket exists so a *mixed* batch can say which half came back. A batch
/// with no reversible half at all was still offered for undo: it consumed the
/// undo slot, restored nothing, and reported success, because `is_complete()`
/// reads only `skipped`.
#[test]
fn a_run_that_only_wrote_tags_is_not_offered_for_undo() {
    let fixture = Fixture::new(&[]);
    let mut journal = Journal::create(fixture.journal.path()).unwrap();
    journal
        .write(Record::Begin {
            platform: "test".into(),
            items: 1,
        })
        .unwrap();
    journal
        .write(Record::PlanIrreversible {
            seq: 0,
            path: fixture.path("a.mp3"),
            op: "remove_tags".into(),
            change: Effect::RemoveTags {
                kinds: vec![ren_core::meta::write::TagKind::Id3v2],
            },
        })
        .unwrap();
    journal.write(Record::Completed { seq: 0 }).unwrap();
    journal
        .write(Record::Commit {
            renamed: 0,
            failed: 0,
        })
        .unwrap();
    drop(journal);

    assert_eq!(
        Journal::latest_undoable(fixture.journal.path()).unwrap(),
        None,
        "a run with nothing to put back must not be offered for undo"
    );

    // And a batch that *did* rename something still is.
    let fixture2 = Fixture::new(&[]);
    let mut journal = Journal::create(fixture2.journal.path()).unwrap();
    journal
        .write(Record::Begin {
            platform: "test".into(),
            items: 2,
        })
        .unwrap();
    journal
        .write(Record::PlanIrreversible {
            seq: 0,
            path: fixture2.path("a.mp3"),
            op: "remove_tags".into(),
            change: Effect::RemoveTags {
                kinds: vec![ren_core::meta::write::TagKind::Id3v2],
            },
        })
        .unwrap();
    journal.write(Record::Completed { seq: 0 }).unwrap();
    journal
        .write(Record::PlanRename {
            seq: 1,
            from: fixture2.path("a.mp3"),
            to: fixture2.path("b.mp3"),
        })
        .unwrap();
    journal.write(Record::Completed { seq: 1 }).unwrap();
    drop(journal);
    assert!(
        Journal::latest_undoable(fixture2.journal.path())
            .unwrap()
            .is_some(),
        "a mixed batch has a half that can be put back"
    );
}

/// The temp-name hops a cycle needs are bookkeeping, not renames the user
/// asked for. The *simulate* branch already filtered them, with a comment
/// saying why; the executing branch did not — so two files swapping names
/// reported three renames, the status bar could say "Renamed 3 of 2", and a
/// simulation disagreed with the run it was simulating.
#[test]
fn a_swap_reports_two_renames_not_three() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let platform = ren_platform::host();
    let pipeline = pipeline_of(MapNames::new(&[("a", "b"), ("b", "a")]), Scope::Name);
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());

    // Three physical operations, because a swap needs a temp name (P6).
    assert_eq!(plan.ops.len(), 3);

    let simulated = apply(
        &plan,
        platform.as_ref(),
        &ApplyOptions {
            simulate: true,
            ..fixture.options()
        },
    )
    .unwrap();
    let real = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();

    assert_eq!(fixture.names(), ["a.txt", "b.txt"]);
    assert_eq!(real.renamed.len(), 2, "the user asked for two renames");
    assert_eq!(
        simulated.renamed.len(),
        real.renamed.len(),
        "a simulation must report what the run reports"
    );
}

/// One file, two action cards: the gate quotes rows, so the result must too.
/// `acted` keeps one entry per action, because the log wants a line each.
#[test]
fn one_run_quotes_one_number_for_one_file() {
    let fixture = Fixture::new(&["a.txt"]);
    let platform = ren_platform::host();
    let pipeline = Pipeline::new()
        .with(
            Step::Action(Box::new(StampModified(stamp()))),
            StepConfig::default(),
        )
        // A second card on the same file. Two of a kind rather than one of
        // each, because `StampCreated` needs a capability Linux does not have
        // (P5) and the run would be blocked before it could be counted.
        .with(
            Step::Action(Box::new(StampModified(stamp()))),
            StepConfig::default(),
        );
    let plan = plan(&fixture.entries(), &pipeline, platform.as_ref());
    assert_eq!(plan.acted(), 1, "one file");

    let report = apply(&plan, platform.as_ref(), &fixture.options()).unwrap();
    assert_eq!(report.acted.len(), 2, "two actions, for the log");
    assert_eq!(
        report.modified(),
        plan.acted(),
        "the number the user reads must be the one the gate quoted"
    );
}

/// The Exif half of the guard `meta::audio` has had since step 3.
///
/// `<Exif-Name>` was the one reader in the engine with **no cache at all**: it
/// opened and fully parsed the photograph on every call, from the parallel
/// evaluation pass, once per file per keystroke — and a pattern naming two Exif
/// fields parsed it twice. P44 says anything expensive goes behind the
/// process-wide mtime-keyed cache; this one simply did not, and nothing was
/// watching, because P52's lesson had only ever been applied to audio.
///
/// A count, not a stopwatch, for the reason P52 records: the planner's own work
/// dominates the wall clock either way.
#[test]
fn an_image_pipeline_reads_each_photograph_once_however_many_tags_name_it() {
    use ren_core::meta::testing::jpeg_with_exif;

    let dir = TempDir::new().unwrap();
    for i in 0..200 {
        std::fs::write(
            dir.path().join(format!("photo_{i:03}.jpg")),
            jpeg_with_exif(Some("2020:01:02 03:04:05"), None, None),
        )
        .unwrap();
    }
    let entries = list(dir.path(), ListOptions::default()).unwrap();
    assert_eq!(entries.len(), 200);

    let platform = ren_platform::host();
    // Two Exif tags in one pattern: uncached, that was two parses per file.
    let pipeline = pipeline_of(
        ren_core::ops::FreeFormat::new("<Exif-Make>-<Exif-Model>-<Name>"),
        Scope::Name,
    );

    ren_core::meta::exif::forget_all();
    let before = ren_core::meta::exif::parses_so_far();
    let first = plan(&entries, &pipeline, platform.as_ref());
    let cold = ren_core::meta::exif::parses_so_far() - before;

    // The positive control: without it a reader that answered nothing at all
    // would look brilliantly cached.
    assert!(
        cold >= 200,
        "the first pass has to actually read them: {cold}"
    );
    assert!(
        cold < 300,
        "{cold} parses for 200 files — a pattern naming two Exif tags is \
         parsing each photograph once per tag"
    );

    let before = ren_core::meta::exif::parses_so_far();
    let second = plan(&entries, &pipeline, platform.as_ref());
    let warm = ren_core::meta::exif::parses_so_far() - before;
    assert_eq!(first.items.len(), second.items.len());
    assert!(
        warm < 20,
        "a second pass re-read {warm} photographs — every keystroke reparses \
         the whole folder"
    );
}

/// P50 says *every* reader is gated on a minimum size. The audio one has been
/// since step 2; this one was not, so a folder of tiny files was opened once
/// each for nothing.
#[test]
fn a_file_too_small_to_hold_an_exif_block_is_never_parsed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("tiny.jpg");
    std::fs::write(&path, b"\xFF\xD8\xFF\xD9").unwrap();

    ren_core::meta::exif::forget_all();
    let before = ren_core::meta::exif::parses_so_far();
    assert_eq!(ren_core::meta::exif::field_of(&path, "Make"), None);
    assert_eq!(
        ren_core::meta::exif::parses_so_far(),
        before,
        "the gate is what stops it, not the parser failing"
    );

    // And the gate is never itself the reason a real file fails.
    let real = dir.path().join("real.jpg");
    std::fs::write(
        &real,
        ren_core::meta::testing::jpeg_with_exif(Some("2020:01:02 03:04:05"), None, None),
    )
    .unwrap();
    ren_core::meta::exif::forget_all();
    assert!(ren_core::meta::exif::fields_of(&real).is_some());
}

// --- M8: a folder and its own contents in one run ----------------------------

/// Folders + Subfolders lists a folder *before* the files inside it, so the
/// planner used to rename the folder first and every rename underneath it then
/// failed on a path that no longer existed. Two folders deep, because one level
/// passes by accident under several wrong orderings.
#[test]
fn a_folder_and_the_files_inside_it_rename_in_one_run() {
    let platform = ren_platform::host();
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("my_folder/my_sub")).unwrap();
    std::fs::write(dir.path().join("my_folder/my_file.txt"), b"one").unwrap();
    std::fs::write(dir.path().join("my_folder/my_sub/my_deep.txt"), b"two").unwrap();
    std::fs::write(dir.path().join("my_top.txt"), b"three").unwrap();

    let entries = list(
        dir.path(),
        ListOptions {
            folders: true,
            subfolders: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(entries.len(), 5, "folder, subfolder and three files");

    let pipeline = Pipeline::new().with(
        Step::Name(Box::new(ren_core::Replace::new("_", " "))),
        StepConfig::scoped(Scope::Name),
    );
    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert!(plan.is_executable(), "{:?}", plan.items);
    assert_eq!(plan.changed(), 5);

    // The preview promise is the *final* resting place, not the halfway house
    // the row's own rename leaves it in.
    let previewed: Vec<std::path::PathBuf> = plan
        .items
        .iter()
        .filter(|i| i.state.is_changed())
        .map(|i| i.target.clone())
        .collect();
    assert!(
        previewed.contains(&dir.path().join("my folder/my sub/my deep.txt")),
        "{previewed:?}"
    );

    let journal = TempDir::new().unwrap();
    let report = apply(
        &plan,
        platform.as_ref(),
        &ApplyOptions {
            journal_dir: journal.path().to_path_buf(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);

    for target in &previewed {
        assert!(target.exists(), "the preview promised {target:?}");
    }
    assert_eq!(
        std::fs::read(dir.path().join("my folder/my sub/my deep.txt")).unwrap(),
        b"two"
    );

    // And it all goes back.
    undo_last(platform.as_ref(), journal.path()).unwrap();
    assert!(dir.path().join("my_folder/my_sub/my_deep.txt").exists());
    assert!(!dir.path().join("my folder").exists());
}

/// `<\\>` moves a file into a subfolder of the folder it is already in, and for
/// a *folder* row that subfolder can be the folder itself. No filesystem can
/// do it, so P4 says block the run rather than fail on the way through.
///
/// Found by the M8 property generator, the moment it started putting folders in
/// the tree: before this the plan reported executable and `apply` came back
/// with `EINVAL` on one row and a half-finished batch.
#[test]
fn a_folder_cannot_be_moved_inside_itself() {
    let platform = ren_platform::host();
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("holiday")).unwrap();
    std::fs::write(dir.path().join("holiday/a.txt"), b"x").unwrap();

    let entries = list(
        dir.path(),
        ListOptions {
            folders: true,
            subfolders: false,
            files: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(entries.len(), 1);

    let pipeline = Pipeline::new().with(
        Step::Name(Box::new(MapNames::new(&[("holiday", "holiday/holiday")]))),
        StepConfig::scoped(Scope::Name),
    );
    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert!(!plan.is_executable());
    assert!(
        matches!(
            plan.items[0].state,
            RowState::Conflict(ConflictKind::IntoItself)
        ),
        "{:?}",
        plan.items[0].state
    );
    // And the folder is untouched.
    assert!(dir.path().join("holiday/a.txt").exists());
}

/// `host()`, but the volume folds case the way Windows does.
///
/// Windows ships first (CLAUDE.md) and the collision check compares *folded*
/// paths, so "does `SUB` collide with a wanted `sub`" is a question Linux
/// cannot answer by running the real platform — but it is pure logic, and
/// `NamingRules` already carries both answers. Everything except the folding
/// rules is the host's.
#[derive(Debug)]
struct WindowsFolding(std::sync::Arc<dyn ren_platform::Platform>);

impl WindowsFolding {
    fn new() -> Self {
        Self(ren_platform::host())
    }
}

impl ren_platform::Platform for WindowsFolding {
    fn naming_rules(&self, _path: &Path) -> &'static ren_platform::NamingRules {
        &ren_platform::WINDOWS
    }
    fn case_sensitivity(&self, _dir: &Path) -> ren_platform::CaseSensitivity {
        ren_platform::CaseSensitivity::Insensitive
    }
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn capabilities(&self) -> &'static [ren_platform::Capability] {
        self.0.capabilities()
    }
    fn rename(&self, from: &Path, to: &Path) -> ren_platform::Result<()> {
        self.0.rename(from, to)
    }
    fn get_attributes(&self, path: &Path) -> ren_platform::Result<ren_platform::FileAttributes> {
        self.0.get_attributes(path)
    }
    fn set_attributes(
        &self,
        path: &Path,
        change: ren_platform::AttributeChange,
    ) -> ren_platform::Result<()> {
        self.0.set_attributes(path, change)
    }
    fn get_times(&self, path: &Path) -> ren_platform::Result<ren_platform::FileTimes> {
        self.0.get_times(path)
    }
    fn set_times(&self, path: &Path, change: ren_platform::TimeChange) -> ren_platform::Result<()> {
        self.0.set_times(path, change)
    }
    fn reveal_in_file_manager(&self, path: &Path) -> ren_platform::Result<()> {
        self.0.reveal_in_file_manager(path)
    }
    fn notify_shell_changed(&self, path: &Path) {
        self.0.notify_shell_changed(path);
    }
}

/// The collision is folded, so on Windows `SUB` and `sub` are the same name.
///
/// A raw `PathBuf` comparison would pass every test on this machine and miss
/// the collision on the platform that ships.
#[test]
fn a_name_that_differs_only_in_case_from_a_wanted_folder_still_collides() {
    let platform = WindowsFolding::new();
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("holiday")).unwrap();
    std::fs::write(dir.path().join("1"), b"x").unwrap();

    let entries = list(
        dir.path(),
        ListOptions {
            folders: true,
            subfolders: true,
            ..Default::default()
        },
    )
    .unwrap();

    // `sub` as a folder, `SUB` as a filename.
    let pipeline = pipeline_of(
        MapNames::new(&[("holiday", "sub/holiday"), ("1", "SUB")]),
        Scope::Both,
    );
    let plan = plan(&entries, &pipeline, &platform);

    let one = plan
        .items
        .iter()
        .find(|i| i.source.ends_with("1"))
        .expect("the file is in the listing");
    assert!(
        matches!(
            one.state,
            RowState::Conflict(ConflictKind::NeededAsFolder { .. })
        ),
        "{:?}",
        one.state
    );
    assert!(!plan.is_executable());
}

/// And the same run on a case-sensitive volume is *not* a collision, because
/// there `SUB` and `sub` really are two different names.
#[test]
fn a_case_sensitive_volume_lets_the_same_two_names_coexist() {
    let platform = ren_platform::host();
    if platform.naming_rules(Path::new("/")).fold("A") == "a" {
        return; // A case-insensitive host cannot answer this one.
    }
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("holiday")).unwrap();
    std::fs::write(dir.path().join("1"), b"x").unwrap();

    let entries = list(
        dir.path(),
        ListOptions {
            folders: true,
            subfolders: true,
            ..Default::default()
        },
    )
    .unwrap();

    let pipeline = pipeline_of(
        MapNames::new(&[("holiday", "sub/holiday"), ("1", "SUB")]),
        Scope::Both,
    );
    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert!(plan.is_executable(), "{:?}", plan.items);
}

/// A row whose new name is a folder the same run has to create is a conflict.
///
/// The third way a name can be occupied, and the one the planner could not
/// see. `detect_duplicate_targets` compares rows against each other and these
/// two rows have *different* targets; `detect_existing_targets` reads the
/// destination directory, and the directory does not exist yet. So the plan
/// came out `is_executable() == true` carrying both
///
///     CreateDir { path: ".../sub" }
///     Rename { from: ".../1", to: ".../sub" }
///
/// and failed on whichever ran second — after opening a journal. Found by the
/// property suite, reported after 1.0, fixed here.
#[test]
fn a_name_that_is_also_a_folder_this_run_creates_is_a_conflict() {
    let platform = ren_platform::host();
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("holiday")).unwrap();
    std::fs::write(dir.path().join("1"), b"x").unwrap();

    let entries = list(
        dir.path(),
        ListOptions {
            folders: true,
            subfolders: true,
            ..Default::default()
        },
    )
    .unwrap();

    // `holiday` moves into a new `sub`, so the run has to create `.../sub`.
    // `1` is renamed to `sub`, so a file wants that exact name.
    let pipeline = pipeline_of(
        MapNames::new(&[("holiday", "sub/holiday"), ("1", "sub")]),
        Scope::Both,
    );
    let plan = plan(&entries, &pipeline, platform.as_ref());

    let one = plan
        .items
        .iter()
        .find(|i| i.source.ends_with("1"))
        .expect("the file is in the listing");
    assert!(
        matches!(
            one.state,
            RowState::Conflict(ConflictKind::NeededAsFolder { .. })
        ),
        "{:?}",
        one.state
    );
    assert!(!plan.is_executable(), "P4: a conflict blocks the whole run");

    // And the plan does not carry a rename for the row it has refused.
    assert!(
        !plan.ops.iter().any(|op| matches!(
            op,
            PlannedOp::Rename { from, .. } if from.ends_with("1")
        )),
        "{:?}",
        plan.ops
    );

    // Nothing ran.
    assert!(dir.path().join("1").exists());
    assert!(!dir.path().join("sub").exists());
}

/// The same shape without the collision still works — the check must not
/// block every run that creates a folder.
#[test]
fn a_folder_this_run_creates_is_fine_when_no_row_claims_its_name() {
    let platform = ren_platform::host();
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("holiday")).unwrap();
    std::fs::write(dir.path().join("1"), b"x").unwrap();

    let entries = list(
        dir.path(),
        ListOptions {
            folders: true,
            subfolders: true,
            ..Default::default()
        },
    )
    .unwrap();

    let pipeline = pipeline_of(
        MapNames::new(&[("holiday", "sub/holiday"), ("1", "one")]),
        Scope::Both,
    );
    let plan = plan(&entries, &pipeline, platform.as_ref());
    assert!(plan.is_executable(), "{:?}", plan.items);
    assert!(
        plan.ops
            .iter()
            .any(|op| matches!(op, PlannedOp::CreateDir { path } if path.ends_with("sub"))),
        "{:?}",
        plan.ops
    );
}

/// A **file** whose `<\>` target lands under its own name is blocked by a
/// file — itself — not by "a folder cannot be moved inside itself".
///
/// The two checks overlap on exactly this shape, and the order between them
/// decides which message the user gets. A file is not a folder, so the folder
/// message would be a wrong answer confidently given.
#[test]
fn a_file_moved_under_its_own_name_is_blocked_by_a_file_not_by_itself() {
    let platform = ren_platform::host();
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("("), b"x").unwrap();

    let entries = list(dir.path(), ListOptions::default()).unwrap();
    let pipeline = pipeline_of(MapNames::new(&[("(", "(/(")]), Scope::Both);
    let plan = plan(&entries, &pipeline, platform.as_ref());

    assert!(
        matches!(
            plan.items[0].state,
            RowState::Conflict(ConflictKind::BlockedByFile { .. })
        ),
        "{:?}",
        plan.items[0].state
    );
    assert!(!plan.is_executable());
}
