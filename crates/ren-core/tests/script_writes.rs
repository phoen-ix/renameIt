//! A script's `done()` asks for a file; the executor writes it.
//!
//! The M7 acceptance M7 words as *"explicit opt-in API for the
//! playlist script's file writing — gated behind a per-script user
//! confirmation"*. What this file pins is that the gate is **structural**
//! rather than a convention:
//!
//! * a script cannot write during a preview, because `rename` and `done` only
//!   ever run while *planning* and planning produces a request, not a file;
//! * a simulation writes nothing, for the same reason every other planned
//!   operation does not;
//! * creating a file is undoable and overwriting one is not, and the second
//!   needs P2's consent before the executor will start.
//!
//! The alternative would be a script that opens a file for writing directly,
//! truncating whatever was there without asking — with nothing between a
//! preview and that call but the script remembering to check a preview flag at
//! the top of every function.

use ren_core::exec::{Journal, Record};
use ren_core::ops::Script;
use ren_core::{ApplyOptions, ListOptions, Pipeline, PlannedOp, Undoability, apply, list, plan};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    scripts: TempDir,
    journal: TempDir,
}

/// A path as a Koto string literal.
///
/// Koto processes escapes inside both quote styles, and a Windows path is
/// mostly backslashes: `C:\\Users\\…` reaches the parser as `\U`, which is not
/// an escape it knows, so every test that pasted a path straight into a script
/// failed there and only there. Production never hits this — a script builds
/// its path from `fr.path` at run time, which is a value rather than source.
fn koto_literal(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
}

impl Fixture {
    fn new(names: &[&str]) -> Self {
        let dir = TempDir::new().unwrap();
        for (i, name) in names.iter().enumerate() {
            std::fs::write(dir.path().join(name), format!("contents of {i}")).unwrap();
        }
        Self {
            dir,
            scripts: TempDir::new().unwrap(),
            journal: TempDir::new().unwrap(),
        }
    }

    /// A script that renames nothing and asks for one file at `target`.
    fn writer(&self, target: &Path, contents: &str) -> Script {
        self.script(
            "Writer",
            &format!(
                "rename = || ''\n\
                 done = ||\n  \
                   {{path: '{}', contents: '{}', log: 'wrote the file'}}\n",
                koto_literal(target),
                contents
            ),
        )
    }

    fn script(&self, name: &str, source: &str) -> Script {
        ren_core::script::store::forget_all();
        std::fs::write(self.scripts.path().join(format!("{name}.koto")), source).unwrap();
        Script::new(name).in_dir(self.scripts.path())
    }

    fn plan_with(&self, op: Script) -> ren_core::Plan {
        let entries = list(self.dir.path(), ListOptions::default()).unwrap();
        let pipeline = Pipeline::new().then(op);
        plan(&entries, &pipeline, ren_platform::host().as_ref())
    }

    fn options(&self) -> ApplyOptions {
        ApplyOptions {
            simulate: false,
            journal_dir: self.journal.path().to_path_buf(),
            allow_irreversible: false,
            ..Default::default()
        }
    }

    fn allowing_irreversible(&self) -> ApplyOptions {
        ApplyOptions {
            allow_irreversible: true,
            ..self.options()
        }
    }
}

fn writes_of(plan: &ren_core::Plan) -> Vec<(PathBuf, Undoability)> {
    plan.ops
        .iter()
        .filter_map(|op| match op {
            PlannedOp::WriteFile {
                path, undoability, ..
            } => Some((path.clone(), *undoability)),
            _ => None,
        })
        .collect()
}

/// The headline: planning produces a *request*, and only `apply` writes.
#[test]
fn planning_a_script_that_writes_produces_no_file() {
    let fixture = Fixture::new(&["a.txt"]);
    let target = fixture.dir.path().join("list.m3u");

    let plan = fixture.plan_with(fixture.writer(&target, "#EXTM3U"));

    assert_eq!(
        writes_of(&plan),
        [(target.clone(), Undoability::Full)],
        "the plan carries the request"
    );
    assert!(
        !target.exists(),
        "planning wrote a file — a preview must not be able to"
    );
    assert_eq!(plan.notes, ["wrote the file"]);
}

/// And the preview runs the plan builder over and over without ever writing,
/// which is the property the whole design exists for.
#[test]
fn previewing_repeatedly_never_writes() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let target = fixture.dir.path().join("list.m3u");
    let op = fixture.writer(&target, "#EXTM3U");

    for _ in 0..5 {
        let plan = fixture.plan_with(op.clone());
        assert_eq!(writes_of(&plan).len(), 1);
    }
    assert!(!target.exists());
}

#[test]
fn applying_writes_the_file_the_script_asked_for() {
    let fixture = Fixture::new(&["a.txt"]);
    let target = fixture.dir.path().join("list.m3u");
    let plan = fixture.plan_with(fixture.writer(&target, "#EXTM3U"));

    let report = apply(&plan, ren_platform::host().as_ref(), &fixture.options()).unwrap();

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "#EXTM3U");
    assert_eq!(report.wrote, [(target, false)], "created, not replaced");
}

/// A simulation performs no syscall at all, so it reports the write and does
/// not do it — the same bargain every other planned operation makes.
#[test]
fn simulating_reports_the_write_without_making_it() {
    let fixture = Fixture::new(&["a.txt"]);
    let target = fixture.dir.path().join("list.m3u");
    let plan = fixture.plan_with(fixture.writer(&target, "#EXTM3U"));

    let options = ApplyOptions {
        simulate: true,
        ..fixture.options()
    };
    let report = apply(&plan, ren_platform::host().as_ref(), &options).unwrap();

    assert_eq!(report.wrote, [(target.clone(), false)]);
    assert!(!target.exists());
}

/// Creating a file is reversible: undo takes it away again and the folder is
/// as it was.
#[test]
fn undo_removes_a_file_the_run_created() {
    let fixture = Fixture::new(&["a.txt"]);
    let target = fixture.dir.path().join("list.m3u");
    let plan = fixture.plan_with(fixture.writer(&target, "#EXTM3U"));

    apply(&plan, ren_platform::host().as_ref(), &fixture.options()).unwrap();
    assert!(target.exists());

    let report =
        ren_core::undo_last(ren_platform::host().as_ref(), fixture.journal.path()).unwrap();

    assert_eq!(report.removed_files, std::slice::from_ref(&target));
    assert!(!target.exists(), "undo left the file behind");
}

// --- Overwriting: the half that cannot be taken back -------------------------

#[test]
fn overwriting_an_existing_file_is_planned_as_irreversible() {
    let fixture = Fixture::new(&["a.txt"]);
    let target = fixture.dir.path().join("list.m3u");
    std::fs::write(&target, "the old playlist").unwrap();

    let plan = fixture.plan_with(fixture.writer(&target, "#EXTM3U"));

    assert_eq!(writes_of(&plan), [(target, Undoability::None)]);
    assert_eq!(
        plan.irreversible(),
        1,
        "counted where the confirmation reads it"
    );
    assert_eq!(plan.undoability(), Undoability::None);
}

/// P2's gate, on a run whose only irreversible part is a file being replaced.
/// Without consent the executor refuses **before** opening a journal, so the
/// old contents are still there to look at.
#[test]
fn overwriting_without_consent_is_refused_before_anything_is_touched() {
    let fixture = Fixture::new(&["a.txt"]);
    let target = fixture.dir.path().join("list.m3u");
    std::fs::write(&target, "the old playlist").unwrap();
    let plan = fixture.plan_with(fixture.writer(&target, "#EXTM3U"));

    let error = apply(&plan, ren_platform::host().as_ref(), &fixture.options()).unwrap_err();

    assert!(
        matches!(error, ren_core::ExecError::Irreversible { items: 1 }),
        "{error:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "the old playlist",
        "the file was touched despite the refusal"
    );
    assert_eq!(
        std::fs::read_dir(fixture.journal.path()).unwrap().count(),
        0,
        "a journal was opened for a run that was refused"
    );
}

#[test]
fn overwriting_with_consent_replaces_it_and_undo_says_it_cannot_help() {
    let fixture = Fixture::new(&["a.txt"]);
    let target = fixture.dir.path().join("list.m3u");
    std::fs::write(&target, "the old playlist").unwrap();
    let plan = fixture.plan_with(fixture.writer(&target, "#EXTM3U"));

    let report = apply(
        &plan,
        ren_platform::host().as_ref(),
        &fixture.allowing_irreversible(),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "#EXTM3U");
    assert_eq!(report.wrote, [(target.clone(), true)], "replaced");
    assert_eq!(report.reversible, 0, "nothing here can be put back");

    // The journal records that something was destroyed, and undo reports it
    // rather than silently doing nothing or claiming success.
    let journals: Vec<_> = ren_core::exec::Journal::list(fixture.journal.path()).unwrap();
    let lines = Journal::read(&journals[0]).unwrap();
    assert!(
        lines
            .iter()
            .any(|l| matches!(&l.record, Record::PlanWriteFile { replaced: true, .. })),
        "the overwrite is not in the journal"
    );

    // And undo is not *offered* at all, which is stronger than offering one
    // that does nothing. D54/D77 established the rule for tag writes — a run
    // with nothing to put back must not consume an Undo press, or the batch
    // beneath it moves out of reach — and a run whose only change was replacing
    // a file inherits it without a line of new code.
    let error =
        ren_core::undo_last(ren_platform::host().as_ref(), fixture.journal.path()).unwrap_err();
    assert!(
        matches!(error, ren_core::ExecError::NothingToUndo(_)),
        "{error:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "#EXTM3U",
        "undo must not delete a file it did not create"
    );
}

/// **Undo must never delete a file it did not create.**
///
/// The case the tests above cannot reach, and the one that matters most. An
/// overwrite-only run is not offered for undo at all, so its `replaced: true`
/// record is never walked — which means every test above passes even with the
/// guard that reads that flag removed. Found by doing exactly that.
///
/// A run that *also renames* does have work to put back, so undo runs, reaches
/// the record, and the flag is the only thing standing between the user and
/// losing a playlist they wrote themselves.
#[test]
fn undo_of_a_run_that_also_renamed_leaves_the_overwritten_file_alone() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    // *The user's own* playlist, in the listed folder — the only place a
    // script may write (P60 as amended) — and left out of the renaming, so
    // the write replaces it on every volume rather than depending on whether
    // `list.m3u` → `LIST.m3u` is one file or two.
    let target = fixture.dir.path().join("list.m3u");
    std::fs::write(&target, "the playlist the user wrote").unwrap();

    // Renames *and* overwrites, so the transaction has reversible work in it.
    let op = fixture.script(
        "RenameAndWrite",
        &format!(
            "rename = ||\n  \
               if fr.full_filename.ends_with '.txt'\n    \
                 return fr.filename.to_uppercase()\n  \
               ''\n\
             done = || {{path: '{}', contents: 'ours'}}\n",
            koto_literal(&target)
        ),
    );
    let plan = fixture.plan_with(op);
    apply(
        &plan,
        ren_platform::host().as_ref(),
        &fixture.allowing_irreversible(),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "ours");

    let report =
        ren_core::undo_last(ren_platform::host().as_ref(), fixture.journal.path()).unwrap();

    assert_eq!(report.restored.len(), 2, "the renames came back");
    assert!(
        report.removed_files.is_empty(),
        "undo deleted a file it did not create: {:?}",
        report.removed_files
    );
    assert!(
        target.exists(),
        "the user's own playlist was deleted by an undo"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "ours",
        "the contents cannot be restored, but the file must survive"
    );
    assert_eq!(
        report.irreversible.len(),
        1,
        "the overwrite is reported as something undo could not take back"
    );
}

/// The other side of that rule: a run that *created* the file does have work to
/// put back, so it is offered — even though the two runs differ only in whether
/// something was already there.
#[test]
fn a_run_that_created_a_file_is_offered_for_undo_where_an_overwrite_is_not() {
    let fixture = Fixture::new(&["a.txt"]);
    let target = fixture.dir.path().join("list.m3u");
    let plan = fixture.plan_with(fixture.writer(&target, "#EXTM3U"));

    apply(&plan, ren_platform::host().as_ref(), &fixture.options()).unwrap();
    let report =
        ren_core::undo_last(ren_platform::host().as_ref(), fixture.journal.path()).unwrap();
    assert_eq!(report.removed_files, [target]);
}

// --- Refusals ----------------------------------------------------------------

/// A relative path would resolve against the process's working directory,
/// which for the GUI is wherever it happened to be launched from. Refused, and
/// the refusal is a note rather than a silent drop.
#[test]
fn a_relative_path_is_refused_with_a_note() {
    let fixture = Fixture::new(&["a.txt"]);
    let op = fixture.script(
        "Sneaky",
        "rename = || ''\ndone = || {path: 'somewhere.txt', contents: 'x'}",
    );

    let plan = fixture.plan_with(op);

    assert!(writes_of(&plan).is_empty(), "a relative path was planned");
    assert_eq!(plan.notes.len(), 1);
    assert!(plan.notes[0].contains("full path"), "{:?}", plan.notes);
}

/// A `done` that throws is a warning and the batch carries on: the renames
/// themselves already succeeded, so failing them retroactively would be
/// wrong.
#[test]
fn a_done_that_fails_is_a_note_and_the_renames_still_happen() {
    let fixture = Fixture::new(&["a.txt"]);
    let op = fixture.script(
        "Boom",
        "rename = || fr.filename.to_uppercase()\ndone = || throw 'no'",
    );

    let plan = fixture.plan_with(op);

    assert_eq!(plan.changed(), 1, "the rename survived the broken done()");
    assert_eq!(plan.items[0].new_name, "A.txt");
    assert!(plan.is_executable(), "a note must not block the run");
    assert_eq!(plan.notes.len(), 1);
    assert!(plan.notes[0].contains("done()"), "{:?}", plan.notes);
}

/// `done` is optional, and a script that only renames asks for nothing.
#[test]
fn a_script_that_writes_nothing_plans_nothing_extra() {
    let fixture = Fixture::new(&["a.txt"]);
    let op = fixture.script("Plain", "rename = || fr.filename.to_uppercase()");

    let plan = fixture.plan_with(op);

    assert!(writes_of(&plan).is_empty());
    assert!(plan.notes.is_empty());
    assert_eq!(
        plan.irreversible(),
        0,
        "an ordinary rename needs no consent"
    );
}

/// `done` runs once per plan, not once per row and not twice.
#[test]
fn done_runs_once_however_many_files_there_are() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let target = fixture.dir.path().join("list.m3u");
    let plan = fixture.plan_with(fixture.writer(&target, "#EXTM3U"));

    assert_eq!(writes_of(&plan).len(), 1);
    assert_eq!(plan.notes.len(), 1);
}
