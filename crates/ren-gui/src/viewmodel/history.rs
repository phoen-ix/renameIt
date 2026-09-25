//! Executing a plan, undoing it, and offering to clean up after a crash.
//!
//! Thin over `ren_core::exec` — the engine already has the transactional half
//! (P9: one undo step reverts one executed batch). This adds what a UI needs:
//! a list for the Undo button (newest first) and About's session count, a log to show, and the startup check
//! M1 built `exec::unfinished` for.

use std::path::PathBuf;

use ren_core::exec::{
    ApplyOptions, ApplyReport, ExecError, UndoReport, Unfinished, default_journal_dir,
};
use ren_core::{Plan, apply};
use ren_platform::Platform;

/// One finished batch, for the Undo button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub txn: String,
    pub journal: PathBuf,
    pub renamed: usize,
    /// Files whose metadata this batch changed. A batch can rename nothing and
    /// still have done a great deal.
    pub modified: usize,
    pub simulated: bool,
}

impl Batch {
    pub fn label(&self) -> String {
        let what = describe_counts(self.renamed, self.modified);
        if self.simulated {
            format!("Simulated: {what}")
        } else {
            what
        }
    }
}

/// "Renamed 3 items", "Modified 2 items", "Renamed 3 and modified 2 items" —
/// the past tense of `status_bar::plan_clauses`, with the same suppression
/// rule.
///
/// A tag-only run used to report *"Renamed 0 item(s)"*, which is the exact
/// failure `blocked_reason` was fixed for one milestone earlier: an action-only
/// pipeline changes no names, and saying so as though nothing happened is
/// simply false.
pub fn describe_counts(renamed: usize, modified: usize) -> String {
    match (renamed, modified) {
        (r, 0) => format!("Renamed {}", ren_core::plural(r, "item")),
        (0, m) => format!("Modified {}", ren_core::plural(m, "item")),
        (r, m) => format!("Renamed {r} and modified {}", ren_core::plural(m, "item")),
    }
}

/// A line for the execute log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogLine {
    Renamed {
        from: String,
        to: String,
    },
    Failed {
        path: String,
        error: String,
    },
    Restored {
        from: String,
        to: String,
    },
    Skipped {
        path: String,
        reason: String,
    },
    /// A file a script asked for. `replaced` is the half that matters: creating
    /// one can be undone, replacing one cannot (D99), and the log is the only
    /// place that distinction is visible after the fact.
    Wrote {
        path: String,
        replaced: bool,
    },
    /// A change undo could not take back. **Not** a skip: a skip means undo
    /// looked and declined, while this one was never reversible to begin with
    /// (D54), so it must not make the run read as a failure.
    Irreversible {
        path: String,
        what: String,
    },
    /// A metadata change — the third thing a run does, and until now the log
    /// had no way to say it. `ApplyReport.acted`'s own doc promises *"the line
    /// the log shows for each"*, and the log was the one thing never asking.
    Modified {
        path: String,
        what: String,
    },
    /// A subfolder the run created on the way (D31).
    CreatedDir {
        path: String,
    },
    /// A file a script wrote, which the undo deleted again.
    RemovedFile {
        path: String,
    },
    /// A folder the run created, which the undo removed again.
    RemovedDir {
        path: String,
    },
    /// A folder the run created and the undo left, because something else is
    /// in it now (D31's safety half). Not a failure, but not nothing either:
    /// the user expected it to go.
    KeptDir {
        path: String,
    },
    Note(String),
}

/// A journal the startup scan could not judge, for the recovery banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalProblem {
    pub path: PathBuf,
    /// True when a run in another window holds it (`ExecError::JournalInUse`)
    /// — a batch in progress, not a damaged file, and never one to roll back.
    pub live: bool,
    /// What went wrong, for a journal that is not live.
    pub message: String,
}

#[derive(Debug)]
pub struct History {
    pub journal_dir: PathBuf,
    /// Newest last, so `pop` is the next undo.
    pub batches: Vec<Batch>,
    pub log: Vec<LogLine>,
    /// Changes the last undo could not take back, for the status line.
    last_irreversible: usize,
    /// What the last undo moved, and to where — for the thumbnail cache.
    last_restored: Vec<(PathBuf, PathBuf)>,
    /// Transactions that never finished, found at startup.
    pub unfinished: Vec<Unfinished>,
    /// Journals the same scan could not read, or that another window is
    /// still writing. Kept apart from `unfinished` because neither can be
    /// rolled back from here — and shown, because one of them used to hide
    /// every other unfinished batch without a word.
    pub problems: Vec<JournalProblem>,
}

impl Default for History {
    fn default() -> Self {
        Self::in_dir(default_journal_dir())
    }
}

impl History {
    pub fn in_dir(journal_dir: PathBuf) -> Self {
        let mut history = Self {
            journal_dir,
            batches: Vec::new(),
            log: Vec::new(),
            last_irreversible: 0,
            last_restored: Vec::new(),
            unfinished: Vec::new(),
            problems: Vec::new(),
        };
        history.rescan();
        history
    }

    /// Looks for unfinished transactions again.
    ///
    /// At startup, and after a run that stopped part-way, so the banner
    /// offers the rollback now rather than on the next start. Cheap: a
    /// finished journal is judged by its last line.
    pub fn rescan(&mut self) {
        let (unfinished, problems) = ren_core::exec::unfinished(&self.journal_dir);
        self.unfinished = unfinished;
        self.problems = problems
            .into_iter()
            .map(|(path, error)| JournalProblem {
                live: matches!(error, ExecError::JournalInUse { .. }),
                message: error.to_string(),
                path,
            })
            .collect();
    }

    pub fn can_undo(&self) -> bool {
        self.batches.iter().any(|b| !b.simulated)
    }

    /// Runs a plan and records the batch.
    ///
    /// Returns the report rather than `()`: `apply` stops at the first failure
    /// and leaves what already happened undoable, so "did it work" is not a
    /// yes/no question. The caller needs `renamed.len()` to report honestly and
    /// `failed` to decide what to do next.
    /// `allow_irreversible` is the app saying the user was asked and said yes
    /// (P2). It is threaded rather than defaulted so the confirmation cannot be
    /// forgotten: `ApplyOptions` fails closed.
    pub fn run(
        &mut self,
        plan: &Plan,
        platform: &dyn Platform,
        simulate: bool,
        allow_irreversible: bool,
    ) -> Result<ApplyReport, ExecError> {
        let options = self.run_options(simulate, allow_irreversible);
        let report = apply(plan, platform, &options)?;
        self.record_run(&report, simulate);
        Ok(report)
    }

    /// What a run needs from here, for a worker that performs it elsewhere.
    ///
    /// The other half is [`Self::record_run`], with the report the worker
    /// hands back. `run` is the two composed. F2's single inline rename
    /// composes them itself, because an F2 that failed renamed nothing and
    /// records nothing: the last run's log stays on screen.
    pub fn run_options(&self, simulate: bool, allow_irreversible: bool) -> ApplyOptions {
        ApplyOptions {
            simulate,
            journal_dir: self.journal_dir.clone(),
            allow_irreversible,
            ..Default::default()
        }
    }

    /// Records a finished run: the log, and the batch Undo will revert.
    pub fn record_run(&mut self, report: &ApplyReport, simulate: bool) {
        self.log.clear();
        if simulate {
            self.log
                .push(LogLine::Note("SIMULATION — nothing was written".into()));
        }
        // Folders first: they exist before the files land in them.
        for path in &report.created_dirs {
            self.log.push(LogLine::CreatedDir {
                path: file_name(path),
            });
        }
        for (from, to) in &report.renamed {
            self.log.push(LogLine::Renamed {
                from: file_name(from),
                to: file_name(to),
            });
        }
        // Actions run after every rename (P46), so these paths are the names
        // the files ended up with — the same ones `renamed` reports as `to`.
        for (path, what) in &report.acted {
            self.log.push(LogLine::Modified {
                path: file_name(path),
                what: what.clone(),
            });
        }
        for (path, replaced) in &report.wrote {
            self.log.push(LogLine::Wrote {
                path: file_name(path),
                replaced: *replaced,
            });
        }
        // Whatever a script's `done()` had to say, in the same log.
        for note in &report.notes {
            self.log.push(LogLine::Note(note.clone()));
        }
        for (path, error) in &report.failed {
            self.log.push(LogLine::Failed {
                path: file_name(path),
                error: error.clone(),
            });
        }

        // Only if a reverse replay could put something back. A run that only
        // rewrote tags is recorded in the log but must not arm Undo: pressing
        // it would restore nothing, report success, and push the last batch
        // that *could* be undone out of reach.
        if let Some(txn) = report.txn.clone().filter(|_| report.reversible > 0) {
            self.batches.push(Batch {
                txn,
                journal: report.journal.clone().unwrap_or_default(),
                renamed: report.renamed.len(),
                modified: report.modified(),
                simulated: false,
            });
        } else if simulate {
            self.batches.push(Batch {
                txn: String::new(),
                journal: PathBuf::new(),
                renamed: report.renamed.len(),
                modified: report.modified(),
                simulated: true,
            });
        }
    }

    /// The batch Undo would revert, and where it sits: what a worker needs
    /// to perform the undo elsewhere. The batch is read, not removed — an
    /// engine error must leave it on the stack, or a failed undo silently
    /// costs the user the only handle they had on it. [`Self::record_undo`]
    /// removes it once the report is in.
    pub fn undo_target(&self) -> Result<(usize, Batch), ExecError> {
        let position = self
            .batches
            .iter()
            .rposition(|b| !b.simulated)
            .ok_or_else(|| ExecError::NothingToUndo(self.journal_dir.clone()))?;
        Ok((position, self.batches[position].clone()))
    }

    /// Records a finished undo of the batch at `position`.
    pub fn record_undo(&mut self, position: usize, report: &UndoReport) {
        if position < self.batches.len() {
            self.batches.remove(position);
        }
        self.log.clear();
        self.log_undo(report);
        self.last_irreversible = report.irreversible.len();
        self.last_restored = report.restored.clone();
    }

    /// Everything an undo report has to say, in the log (D77: a field no
    /// front end reads is a change the user is never told about).
    fn log_undo(&mut self, report: &UndoReport) {
        for (from, to) in &report.restored {
            self.log.push(LogLine::Restored {
                from: file_name(from),
                to: file_name(to),
            });
        }
        for (path, what) in &report.reverted {
            self.log.push(LogLine::Modified {
                path: file_name(path),
                what: what.clone(),
            });
        }
        for path in &report.removed_files {
            self.log.push(LogLine::RemovedFile {
                path: file_name(path),
            });
        }
        for path in &report.removed_dirs {
            self.log.push(LogLine::RemovedDir {
                path: file_name(path),
            });
        }
        for path in &report.kept_dirs {
            self.log.push(LogLine::KeptDir {
                path: file_name(path),
            });
        }
        // D54 built this bucket precisely so it could be said out loud. Undo
        // that restored the names of a mixed batch and left its tag writes in
        // place must not report "done" and stop there.
        for (path, what) in &report.irreversible {
            self.log.push(LogLine::Irreversible {
                path: file_name(path),
                what: what.clone(),
            });
        }
        for (path, reason) in &report.skipped {
            self.log.push(LogLine::Skipped {
                path: file_name(path),
                reason: reason.clone(),
            });
        }
        // The files are back, and the journal does not know it: the next
        // undo of this batch would find nothing where it expects it.
        if let Some(reason) = &report.not_recorded {
            self.log.push(LogLine::Note(format!(
                "The undo happened, but it could not be written into the journal ({reason}). \
                 The files are back; the journal still lists this batch as undoable."
            )));
        }
    }

    /// An undo of the batch at `position` failed before touching anything.
    /// Returns what to tell the user when the batch left the stack.
    ///
    /// **Most failures keep the batch** — a failed attempt must not cost the
    /// user the only handle they had on it. Two are permanent, and keeping
    /// the batch then jams Undo on it for the rest of the session, with every
    /// older batch out of reach behind it: the batch was undone somewhere
    /// else (`ren-cli undo`, another window, which share this journal folder
    /// by design, D131), or its journal is gone.
    pub fn undo_failed(&mut self, position: usize, error: &ExecError) -> Option<String> {
        let journal = &self.batches.get(position)?.journal;
        let reason = match error {
            ExecError::NothingToUndo(path) if path == journal => {
                "That batch was already undone elsewhere, so it is off the Undo list"
            }
            ExecError::Io { path, source }
                if path == journal && source.kind() == std::io::ErrorKind::NotFound =>
            {
                "That batch's journal is gone, so it cannot be undone and is off the Undo list"
            }
            _ => return None,
        };
        self.batches.remove(position);
        Some(reason.to_owned())
    }

    /// How many changes the last undo could not take back.
    pub fn last_irreversible(&self) -> usize {
        self.last_irreversible
    }

    /// Which files the last undo moved, and where each went.
    ///
    /// Kept here for the same reason `last_irreversible` is: `undo` returns
    /// nothing but a result, and the caller needs a fact about what happened.
    /// The thumbnail cache follows these so an undo of four hundred renames
    /// does not re-decode four hundred pictures (D139).
    pub fn last_restored(&self) -> &[(PathBuf, PathBuf)] {
        &self.last_restored
    }

    /// Records a rollback of the unfinished transactions, as the worker
    /// performed it: one result per transaction, in order, stopping at the
    /// first failure.
    ///
    /// A transaction leaves the banner only once its own rollback succeeded,
    /// so a failure part-way leaves the rest on offer rather than dropping
    /// them where they would never be offered again. Returns the first
    /// failure's words.
    pub fn record_rollback(
        &mut self,
        results: Vec<(Unfinished, Result<UndoReport, ExecError>)>,
    ) -> Result<(), String> {
        self.log.clear();
        let mut restored = Vec::new();
        for (item, result) in results {
            let report = match result {
                Ok(report) => report,
                Err(error) => {
                    self.last_restored = restored;
                    return Err(error.to_string());
                }
            };
            self.unfinished.retain(|u| u.journal != item.journal);
            self.log.push(LogLine::Note(format!(
                "recovered transaction {}: {} rolled back",
                item.txn,
                ren_core::plural(report.restored.len(), "change")
            )));
            // D77's rule, on the other path: whatever an undo reports is
            // reported here too.
            self.log_undo(&report);
            restored.extend(report.restored);
        }
        self.last_restored = restored;
        Ok(())
    }

    /// Dismisses the recovery offer without touching anything.
    pub fn dismiss_recovery(&mut self) {
        self.unfinished.clear();
        self.problems.clear();
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ren_core::model::Scope;
    use ren_core::ops::Replace;
    use ren_core::{ListOptions, Pipeline, list, plan};
    use tempfile::TempDir;

    struct Fixture {
        dir: TempDir,
        journal: TempDir,
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        for name in ["a_1.txt", "a_2.txt"] {
            std::fs::write(dir.path().join(name), b"payload").unwrap();
        }
        Fixture {
            dir,
            journal: TempDir::new().unwrap(),
        }
    }

    fn planned(fixture: &Fixture) -> Plan {
        let entries = list(fixture.dir.path(), ListOptions::default()).unwrap();
        let pipeline = Pipeline::new().then_scoped(Replace::new("_", "-"), Scope::Name);
        plan(&entries, &pipeline, ren_platform::host().as_ref())
    }

    /// Undo as the app does it: the target, the engine, the record.
    fn undo(history: &mut History) -> Result<(), ExecError> {
        let (position, batch) = history.undo_target()?;
        let report =
            ren_core::exec::undo_transaction(&batch.journal, ren_platform::host().as_ref());
        match report {
            Ok(report) => {
                history.record_undo(position, &report);
                Ok(())
            }
            Err(error) => {
                history.undo_failed(position, &error);
                Err(error)
            }
        }
    }

    /// Rollback as the app does it, on this thread.
    fn recover(history: &mut History) -> Result<(), String> {
        let results = history
            .unfinished
            .iter()
            .map(|item| {
                (
                    item.clone(),
                    ren_core::exec::rollback(item, ren_platform::host().as_ref()),
                )
            })
            .collect();
        history.record_rollback(results)
    }

    /// Strips the Commit so the batch looks as a crash would leave it.
    fn crash(journal: &std::path::Path) {
        let text = std::fs::read_to_string(journal).unwrap();
        let truncated: String = text
            .lines()
            .filter(|l| !l.contains("\"commit\""))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(journal, truncated).unwrap();
    }

    fn names(fixture: &Fixture) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(fixture.dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn running_a_plan_records_a_batch_and_a_log() {
        let fixture = fixture();
        let mut history = History::in_dir(fixture.journal.path().to_path_buf());
        assert!(!history.can_undo());

        history
            .run(
                &planned(&fixture),
                ren_platform::host().as_ref(),
                false,
                false,
            )
            .unwrap();

        assert_eq!(names(&fixture), ["a-1.txt", "a-2.txt"]);
        assert_eq!(history.batches.len(), 1);
        assert!(history.can_undo());
        assert_eq!(history.log.len(), 2);
        assert!(history.batches[0].label().contains('2'));
    }

    #[test]
    fn undo_restores_the_tree_and_pops_the_batch() {
        let fixture = fixture();
        let mut history = History::in_dir(fixture.journal.path().to_path_buf());
        history
            .run(
                &planned(&fixture),
                ren_platform::host().as_ref(),
                false,
                false,
            )
            .unwrap();
        undo(&mut history).unwrap();

        assert_eq!(names(&fixture), ["a_1.txt", "a_2.txt"]);
        assert!(!history.can_undo());
        assert!(
            history
                .log
                .iter()
                .all(|l| matches!(l, LogLine::Restored { .. }))
        );
    }

    #[test]
    fn a_simulation_writes_nothing_and_cannot_be_undone() {
        let fixture = fixture();
        let mut history = History::in_dir(fixture.journal.path().to_path_buf());
        history
            .run(
                &planned(&fixture),
                ren_platform::host().as_ref(),
                true,
                false,
            )
            .unwrap();

        assert_eq!(names(&fixture), ["a_1.txt", "a_2.txt"], "nothing written");
        assert!(!history.can_undo());
        assert!(matches!(history.log[0], LogLine::Note(_)));
        assert!(history.batches[0].label().starts_with("Simulated"));
    }

    #[test]
    fn undoing_with_no_batches_is_an_error_not_a_panic() {
        let fixture = fixture();
        let mut history = History::in_dir(fixture.journal.path().to_path_buf());
        assert!(undo(&mut history).is_err());
    }

    #[test]
    fn a_crashed_transaction_is_offered_at_startup_and_can_be_dismissed() {
        let fixture = fixture();
        // Run a batch, then hand-strip the terminal record so it looks crashed.
        let mut history = History::in_dir(fixture.journal.path().to_path_buf());
        history
            .run(
                &planned(&fixture),
                ren_platform::host().as_ref(),
                false,
                false,
            )
            .unwrap();
        crash(&history.batches[0].journal);

        let mut restarted = History::in_dir(fixture.journal.path().to_path_buf());
        assert_eq!(restarted.unfinished.len(), 1);

        restarted.dismiss_recovery();
        assert!(restarted.unfinished.is_empty());
        assert_eq!(
            names(&fixture),
            ["a-1.txt", "a-2.txt"],
            "dismissing changes nothing"
        );
    }

    #[test]
    fn recovering_rolls_the_crashed_transaction_back() {
        let fixture = fixture();
        let mut history = History::in_dir(fixture.journal.path().to_path_buf());
        history
            .run(
                &planned(&fixture),
                ren_platform::host().as_ref(),
                false,
                false,
            )
            .unwrap();
        crash(&history.batches[0].journal);

        let mut restarted = History::in_dir(fixture.journal.path().to_path_buf());
        recover(&mut restarted).unwrap();

        assert_eq!(names(&fixture), ["a_1.txt", "a_2.txt"]);
        assert!(restarted.unfinished.is_empty());
    }

    /// A tag-only batch used to render "Renamed 0 item(s)" — the same lie the
    /// post-run status told, waiting on the Undo list for whenever that
    /// gets wired.
    #[test]
    fn a_batch_is_labelled_by_what_it_actually_did() {
        assert_eq!(describe_counts(3, 0), "Renamed 3 items");
        assert_eq!(describe_counts(1, 0), "Renamed 1 item");
        assert_eq!(describe_counts(0, 2), "Modified 2 items");
        assert_eq!(describe_counts(3, 2), "Renamed 3 and modified 2 items");
        // The one case where "renamed 0" is the honest answer.
        assert_eq!(describe_counts(0, 0), "Renamed 0 items");

        let batch = Batch {
            txn: "t".into(),
            journal: PathBuf::new(),
            renamed: 0,
            modified: 2,
            simulated: false,
        };
        assert_eq!(batch.label(), "Modified 2 items");
        assert_eq!(
            Batch {
                simulated: true,
                ..batch
            }
            .label(),
            "Simulated: Modified 2 items"
        );
    }

    /// An undo that fails must leave the batch on the stack. Removing it first
    /// meant one failed attempt cost the user the only handle they had on it.
    #[test]
    fn a_failed_undo_leaves_the_batch_where_it_was() {
        let fixture = fixture();
        let mut history = History::in_dir(fixture.journal.path().to_path_buf());
        let platform = ren_platform::host();
        history
            .run(&planned(&fixture), platform.as_ref(), false, false)
            .unwrap();
        assert!(history.can_undo());

        // Make the engine fail for a while: another window holds the journal.
        let journal = history.batches.last().unwrap().journal.clone();
        let held = std::fs::File::open(&journal).unwrap();
        held.lock().unwrap();

        let error = undo(&mut history).unwrap_err();
        assert!(matches!(error, ExecError::JournalInUse { .. }), "{error}");
        assert!(
            history.can_undo(),
            "a failed undo must not consume the batch"
        );
        drop(held);
        undo(&mut history).unwrap();
        assert_eq!(names(&fixture), ["a_1.txt", "a_2.txt"]);
    }

    /// **Two failures are permanent**, and a batch kept for them jams Undo on
    /// itself with every older batch behind it: undone elsewhere, or its
    /// journal gone.
    #[test]
    fn a_batch_that_can_never_be_undone_here_leaves_the_stack() {
        let fixture = fixture();
        let mut history = History::in_dir(fixture.journal.path().to_path_buf());
        let platform = ren_platform::host();
        history
            .run(&planned(&fixture), platform.as_ref(), false, false)
            .unwrap();
        ren_core::exec::undo_last(platform.as_ref(), fixture.journal.path()).unwrap();

        let error = undo(&mut history).unwrap_err();
        assert!(matches!(error, ExecError::NothingToUndo(_)), "{error}");
        assert!(!history.can_undo(), "undone elsewhere");

        history
            .run(&planned(&fixture), platform.as_ref(), false, false)
            .unwrap();
        std::fs::remove_file(&history.batches.last().unwrap().journal).unwrap();
        undo(&mut history).unwrap_err();
        assert!(!history.can_undo(), "journal gone");
    }

    /// A journal nobody can read, and one a run in another window still
    /// holds, are both named — and neither hides the unfinished batch beside
    /// them, which one unreadable file used to do.
    #[test]
    fn unreadable_and_live_journals_are_reported_beside_the_unfinished_ones() {
        let fixture = fixture();
        let mut history = History::in_dir(fixture.journal.path().to_path_buf());
        history
            .run(
                &planned(&fixture),
                ren_platform::host().as_ref(),
                false,
                false,
            )
            .unwrap();
        crash(&history.batches[0].journal);
        std::fs::write(
            fixture.journal.path().join("broken.jsonl"),
            "not json\n{}\n",
        )
        .unwrap();
        let live = ren_core::exec::Journal::create(fixture.journal.path()).unwrap();

        let restarted = History::in_dir(fixture.journal.path().to_path_buf());
        assert_eq!(restarted.unfinished.len(), 1, "still offered");
        assert_eq!(restarted.problems.len(), 2, "{:?}", restarted.problems);
        assert_eq!(
            restarted.problems.iter().filter(|p| p.live).count(),
            1,
            "{:?}",
            restarted.problems
        );
        drop(live);
    }

    /// What an undo removed, and what it had to leave, reaches the log.
    #[test]
    fn an_undo_logs_the_folders_it_removed_and_kept() {
        let mut history = History::in_dir(TempDir::new().unwrap().path().to_path_buf());
        let report = UndoReport {
            removed_files: vec![PathBuf::from("/p/list.m3u")],
            removed_dirs: vec![PathBuf::from("/p/2019")],
            kept_dirs: vec![PathBuf::from("/p/2020")],
            not_recorded: Some("disk full".into()),
            ..Default::default()
        };
        history.record_undo(0, &report);
        assert!(history.log.contains(&LogLine::RemovedFile {
            path: "list.m3u".into()
        }));
        assert!(history.log.contains(&LogLine::RemovedDir {
            path: "2019".into()
        }));
        assert!(history.log.contains(&LogLine::KeptDir {
            path: "2020".into()
        }));
        assert!(
            history
                .log
                .iter()
                .any(|l| matches!(l, LogLine::Note(n) if n.contains("disk full")))
        );
    }
}
