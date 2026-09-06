//! Running a plan against the filesystem.

use std::path::PathBuf;

use ren_platform::Platform;

use super::ExecError;
use super::journal::{Journal, Record, default_journal_dir};
use crate::effect::{Before, Effect, TimeSet, Undoability};
use crate::plan::{Plan, PlannedOp, RenameKind};

#[derive(Debug, Clone)]
pub struct ApplyOptions {
    /// Run the planner and the journal-less log, touch nothing on disk.
    pub simulate: bool,
    pub journal_dir: PathBuf,
    /// **P2, as an engine invariant.** An action that cannot be undone does not
    /// run unless the caller has said, in as many words, that it may.
    ///
    /// Defaults to `false`, so a caller that has not thought about it fails
    /// closed. That is deliberate: the GUI's confirmation and the CLI's flag are
    /// two doors onto the same room, and neither should be the only lock.
    pub allow_irreversible: bool,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            simulate: false,
            journal_dir: default_journal_dir(),
            allow_irreversible: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub txn: Option<String>,
    pub journal: Option<PathBuf>,
    pub renamed: Vec<(PathBuf, PathBuf)>,
    /// Subfolders this run created (D31), parents first.
    pub created_dirs: Vec<PathBuf>,
    /// Files a script asked for (M7), and whether each replaced something.
    pub wrote: Vec<(PathBuf, bool)>,
    pub failed: Vec<(PathBuf, String)>,
    /// Metadata changes this run made, with the line the log shows for each.
    pub acted: Vec<(PathBuf, String)>,
    /// Changes a reverse replay could actually put back.
    ///
    /// Zero for a run that only wrote or removed tags — and a run with nothing
    /// to put back must not be offered for undo, or one Undo press spends
    /// itself restoring nothing and the batch beneath it moves out of reach.
    /// The engine's `latest_undoable` reaches the same conclusion from the
    /// journal; this is the same fact where the GUI can see it.
    pub reversible: usize,
    pub simulated: bool,
    /// Lines a script asked to be logged — whatever its `done()` returned, and
    /// any warning from a `done()` that failed (P59).
    ///
    /// Copied from the plan rather than produced here, because that is where
    /// scripts run. Carried on the report so a front end has **one** place to
    /// read what a run has to say, which is the whole reason D77 exists: a
    /// channel nothing reads is a change the user is never told about.
    pub notes: Vec<String>,
}

impl ApplyReport {
    pub fn is_success(&self) -> bool {
        self.failed.is_empty()
    }

    /// Files this run modified.
    ///
    /// **Files, not actions.** `acted` carries one entry per `PlannedOp::Act`
    /// because the log wants a line each, so two cards over one file are two
    /// entries and one modified file. Every number the user reads is a row
    /// count — `Plan::changed`, `acted`, `affected` and `irreversible` all are,
    /// and so is the `item(s)` in the refusal they may have just been shown.
    /// Reporting actions here made one run quote two totals for one file.
    pub fn modified(&self) -> usize {
        self.acted
            .iter()
            .map(|(path, _)| path)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }
}

/// Executes every rename in `plan`, in the order the planner produced.
///
/// Refuses to start if the plan has conflicts or errors (P4) — partial batches
/// are exactly how a renamer eats data.
pub fn apply(
    plan: &Plan,
    platform: &dyn Platform,
    options: &ApplyOptions,
) -> Result<ApplyReport, ExecError> {
    if !plan.is_executable() {
        return Err(ExecError::Blocked {
            conflicts: plan.conflicts(),
            errors: plan.errors(),
        });
    }

    // Before the journal exists, before anything is touched: a run that cannot
    // be taken back needs to have been asked for (P2). A simulation is exempt —
    // it performs no syscall at all, so there is nothing to consent to.
    if !options.simulate && !options.allow_irreversible {
        let irreversible = plan.irreversible();
        if irreversible > 0 {
            return Err(ExecError::Irreversible {
                items: irreversible,
            });
        }
    }

    let mut report = ApplyReport {
        simulated: options.simulate,
        notes: plan.notes.clone(),
        ..Default::default()
    };

    if options.simulate {
        for op in &plan.ops {
            match op {
                // A simulation reports what the user asked for, not the
                // temp-name bookkeeping that makes it possible.
                PlannedOp::Rename { from, to, kind } if *kind != RenameKind::CycleStage => {
                    report.renamed.push((from.clone(), to.clone()));
                }
                PlannedOp::Rename { .. } => {}
                PlannedOp::CreateDir { path } => report.created_dirs.push(path.clone()),
                PlannedOp::WriteFile {
                    path, undoability, ..
                } => report
                    .wrote
                    .push((path.clone(), !undoability.is_reversible())),
                // Reported, but *not* read: simulating must perform no syscall
                // at all, and reading a before-image for a run that writes
                // nothing would be one `stat` per file for nothing.
                PlannedOp::Act { path, describe, .. } => {
                    report.acted.push((path.clone(), describe.clone()));
                }
            }
        }
        return Ok(report);
    }

    if plan.ops.is_empty() {
        return Ok(report);
    }

    let journal = Journal::create(&options.journal_dir)?;
    apply_journalled(plan, platform, journal, report)
}

/// The run itself, once a journal is open.
///
/// Split from [`apply`] so a test can hand in a journal that fails part-way
/// through — the one failure that turns into [`ExecError::Interrupted`] and
/// that no amount of `chmod` can produce on an already-open file.
fn apply_journalled(
    plan: &Plan,
    platform: &dyn Platform,
    mut journal: Journal,
    mut report: ApplyReport,
) -> Result<ApplyReport, ExecError> {
    report.txn = Some(journal.txn().to_owned());
    report.journal = Some(journal.path().to_path_buf());
    journal.write(Record::Begin {
        platform: platform.name().to_owned(),
        items: plan.ops.len(),
    })?;

    // From here on a journal write that fails is `ExecError::Interrupted`: the
    // run has started, files may have moved, and the caller has to be told
    // which — see the variant. `completed` is what the message says.
    let mut completed = 0usize;

    // Every physical change is journalled, temp-name hops included, so undo
    // unwinds a broken cycle by replaying them in reverse (P6).
    for (seq, op) in plan.ops.iter().enumerate() {
        let seq = seq as u64;
        // Write-ahead: the intent is durable before the filesystem is touched.
        let (subject, outcome) = match op {
            PlannedOp::CreateDir { path } => {
                journal
                    .write(Record::PlanCreateDir {
                        seq,
                        path: path.clone(),
                    })
                    .map_err(|e| interrupted(&journal, completed, e))?;
                (
                    path.clone(),
                    std::fs::create_dir(path).map_err(|e| e.to_string()),
                )
            }
            PlannedOp::WriteFile {
                path,
                contents,
                undoability,
            } => {
                // `replaced` comes from the plan's decision rather than a fresh
                // `stat`, so the file the user consented to overwrite is the
                // file the journal says was overwritten. Re-checking here would
                // let the two disagree.
                let replaced = !undoability.is_reversible();
                journal
                    .write(Record::PlanWriteFile {
                        seq,
                        path: path.clone(),
                        replaced,
                    })
                    .map_err(|e| interrupted(&journal, completed, e))?;
                (
                    path.clone(),
                    std::fs::write(path, contents).map_err(|e| e.to_string()),
                )
            }
            PlannedOp::Rename { from, to, .. } => {
                journal
                    .write(Record::PlanRename {
                        seq,
                        from: from.clone(),
                        to: to.clone(),
                    })
                    .map_err(|e| interrupted(&journal, completed, e))?;
                (
                    from.clone(),
                    platform.rename(from, to).map_err(|e| e.to_string()),
                )
            }
            // An irreversible change: the intent is journalled so a crash
            // leaves a record of what was attempted, but there is no
            // before-image because there is nothing to keep.
            PlannedOp::Act {
                path,
                op,
                effect,
                undoability: Undoability::None,
                ..
            } => {
                journal
                    .write(Record::PlanIrreversible {
                        seq,
                        path: path.clone(),
                        op: (*op).to_owned(),
                        change: effect.clone(),
                    })
                    .map_err(|e| interrupted(&journal, completed, e))?;
                (path.clone(), write_effect(path, platform, effect))
            }
            PlannedOp::Act {
                path, op, effect, ..
            } => {
                // The write-ahead invariant needs more here than it does for a
                // rename. "Set modified to X" does not say what X replaced, so
                // the intent alone cannot be inverted — the before-image is
                // read first and journalled *with* the intent.
                match read_before(platform, path, effect) {
                    // Every journalled action has a before-image, because the
                    // one kind that has none took the branch above.
                    Ok(Some(before)) => {
                        journal
                            .write(Record::PlanAct {
                                seq,
                                path: path.clone(),
                                op: (*op).to_owned(),
                                change: effect.clone(),
                                before,
                            })
                            .map_err(|e| interrupted(&journal, completed, e))?;
                        (path.clone(), write_effect(path, platform, effect))
                    }
                    // No before-image, no attempt. A change that cannot be
                    // undone is worse than a change that did not happen, and
                    // nothing was written — so the `Failed` record stands alone
                    // without a `PlanAct`, which `recover::unfinished`'s
                    // saturating arithmetic already tolerates.
                    Err(e) => (path.clone(), Err(e.to_string())),
                    // An effect with no before-image reaching the reversible
                    // branch means its `undoability()` and the action's
                    // `undoable()` disagree — a bug, refused rather than
                    // written.
                    Ok(None) => (
                        path.clone(),
                        Err(
                            "this change reports itself as undoable but keeps no before-image"
                                .to_owned(),
                        ),
                    ),
                }
            }
        };

        match outcome {
            Ok(()) => {
                // The change has happened whether or not this line lands, so
                // it counts before the write, not after.
                completed += 1;
                journal
                    .write(Record::Completed { seq })
                    .map_err(|e| interrupted(&journal, completed, e))?;
                match op {
                    PlannedOp::CreateDir { path } => {
                        report.reversible += 1;
                        report.created_dirs.push(path.clone());
                    }
                    PlannedOp::WriteFile {
                        path, undoability, ..
                    } => {
                        platform.notify_shell_changed(path);
                        if undoability.is_reversible() {
                            report.reversible += 1;
                        }
                        report
                            .wrote
                            .push((path.clone(), !undoability.is_reversible()));
                    }
                    // The same filter the simulate branch above applies, and
                    // for the same reason — it was missing here, so two files
                    // swapping names reported **three** renames and the status
                    // bar could say "Renamed 3 of 2". The journal still records
                    // every hop, because undo needs them; the *report* is what
                    // the user reads.
                    PlannedOp::Rename { from, to, kind } => {
                        platform.notify_shell_changed(to);
                        report.reversible += 1;
                        if *kind != RenameKind::CycleStage {
                            report.renamed.push((from.clone(), to.clone()));
                        }
                    }
                    PlannedOp::Act {
                        path,
                        describe,
                        undoability,
                        ..
                    } => {
                        platform.notify_shell_changed(path);
                        if undoability.is_reversible() {
                            report.reversible += 1;
                        }
                        report.acted.push((path.clone(), describe.clone()));
                    }
                }
            }
            Err(message) => {
                journal
                    .write(Record::Failed {
                        seq,
                        error: message.clone(),
                    })
                    .map_err(|e| interrupted(&journal, completed, e))?;
                report.failed.push((subject, message));
                // Stop at the first failure: whatever already happened stays
                // undoable, and the user decides what to do next.
                break;
            }
        }
    }

    journal
        .write(Record::Commit {
            renamed: report.renamed.len(),
            failed: report.failed.len(),
        })
        .map_err(|e| interrupted(&journal, completed, e))?;

    Ok(report)
}

/// A journal write failed after the run started.
fn interrupted(journal: &Journal, completed: usize, source: ExecError) -> ExecError {
    ExecError::Interrupted {
        journal: journal.path().to_path_buf(),
        txn: journal.txn().to_owned(),
        completed,
        source: Box::new(source),
    }
}

/// Reads exactly the state `effect` is about to replace.
///
/// `None` for an effect that replaces nothing recoverable, which is the same
/// fact `Undoability::None` states — pinned equal by a test, so the two cannot
/// drift apart.
pub(crate) fn read_before(
    platform: &dyn Platform,
    path: &std::path::Path,
    effect: &Effect,
) -> Result<Option<Before>, ren_platform::PlatformError> {
    Ok(match effect {
        Effect::Attributes(_) => Some(Before::Attributes(platform.get_attributes(path)?)),
        Effect::Times(_) => Some(Before::Times(TimeSet::of(platform.get_times(path)?))),
        // Not "we failed to read it" — there is nothing to read. A tag block
        // that has been rewritten is gone, so an image of it would be a
        // fiction, and the executor never asks: the `Undoability::None` arm
        // above journals the intent and skips this entirely.
        Effect::WriteTags { .. } | Effect::RemoveTags { .. } => None,
        // Nor for something written by a newer build. Nothing gets this far —
        // `Undoability::None` sends it down the branch above — and inventing a
        // before-image for it would be the one way this could go wrong.
        Effect::Unknown => None,
    })
}

pub(crate) fn write_effect(
    path: &std::path::Path,
    platform: &dyn Platform,
    effect: &Effect,
) -> Result<(), String> {
    match effect {
        Effect::Attributes(change) => platform
            .set_attributes(path, *change)
            .map_err(|e| e.to_string()),
        Effect::Times(times) => platform
            .set_times(path, times.to_change())
            .map_err(|e| e.to_string()),
        // Not through `Platform`: writing a tag is the same file IO on every
        // OS, so D3's rule points the other way — putting it behind the trait
        // would mean two implementations of one thing.
        Effect::WriteTags { fields, .. } => crate::meta::write::write_fields(path, fields)
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Effect::RemoveTags { kinds } => crate::meta::write::remove_tags(path, kinds)
            .map(|_| ())
            .map_err(|e| e.to_string()),
        // Refused, loudly. A change we cannot name is a change we must not
        // perform, and the planner never produces one — this can only arrive
        // from a journal written by a newer build.
        Effect::Unknown => {
            Err("this change was written by a newer version and cannot be performed".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Replace;
    use crate::{Pipeline, plan};

    /// A journal that dies after the second rename has happened is the one
    /// failure with files already moved behind it, and it has to say so:
    /// `Interrupted`, with the count and the transaction, never the generic
    /// I/O error a refused command line gets.
    #[test]
    fn a_journal_that_fails_mid_run_reports_an_interrupted_run() {
        let dir = tempfile::TempDir::new().unwrap();
        for name in ["a_1.txt", "a_2.txt", "a_3.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let entries = crate::list(dir.path(), Default::default()).unwrap();
        let pipeline = Pipeline::new().then(Replace::new("_", "-"));
        let platform = ren_platform::host();
        let plan = plan(&entries, &pipeline, platform.as_ref());
        assert_eq!(plan.ops.len(), 3);

        // Records: 0 Begin, 1 Plan, 2 Completed, 3 Plan, 4 Completed, 5 Plan…
        // Failing from record 5 means two renames landed and were confirmed,
        // and the third was never announced.
        let journals = tempfile::TempDir::new().unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        journal.fail_writes_from(5);
        let report = ApplyReport::default();

        let error = apply_journalled(&plan, platform.as_ref(), journal, report)
            .expect_err("the journal failure must surface");
        match &error {
            ExecError::Interrupted {
                completed, source, ..
            } => {
                assert_eq!(*completed, 2);
                assert!(
                    matches!(**source, ExecError::Io { .. }),
                    "the cause travels: {source}"
                );
            }
            other => panic!("expected Interrupted, got {other}"),
        }
        assert!(error.to_string().contains("stopped after 2 change(s)"));

        // And the files really did move — which is the whole point of the
        // distinct variant.
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a-1.txt", "a-2.txt", "a_3.txt"]);

        // The open journal is left behind for recovery, with both renames
        // confirmed, so `unfinished` can offer to take them back.
        let unfinished = crate::exec::unfinished(journals.path()).unwrap();
        assert_eq!(unfinished.len(), 1);
        assert_eq!(unfinished[0].completed, 2);
    }
}
