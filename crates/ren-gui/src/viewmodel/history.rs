//! Executing a plan, undoing it, and offering to clean up after a crash.
//!
//! Thin over `ren_core::exec` — the engine already has the transactional half
//! (P9: one undo step reverts one executed batch). This adds what a UI needs:
//! a list to put in the Undo dropdown, a log to show, and the startup check
//! M1 built `exec::unfinished` for.

use std::path::PathBuf;

use ren_core::exec::{ApplyOptions, ApplyReport, ExecError, Unfinished, default_journal_dir};
use ren_core::{Plan, apply};
use ren_platform::Platform;

/// One finished batch, for the Undo dropdown.
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

/// *"Renamed 3 item(s)"*, *"Modified 2 item(s)"*, *"Renamed 3 and modified 2
/// item(s)"* — the past tense of `status_bar::plan_clauses`, with the same
/// suppression rule.
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
    Note(String),
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
}

impl Default for History {
    fn default() -> Self {
        Self::in_dir(default_journal_dir())
    }
}

impl History {
    pub fn in_dir(journal_dir: PathBuf) -> Self {
        let unfinished = ren_core::exec::unfinished(&journal_dir).unwrap_or_default();
        Self {
            journal_dir,
            batches: Vec::new(),
            log: Vec::new(),
            last_irreversible: 0,
            last_restored: Vec::new(),
            unfinished,
        }
    }

    pub fn can_undo(&self) -> bool {
        self.batches.iter().any(|b| !b.simulated)
    }

    /// Runs a plan, recording what happened.
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
        let options = ApplyOptions {
            simulate,
            journal_dir: self.journal_dir.clone(),
            allow_irreversible,
        };
        let report = apply(plan, platform, &options)?;

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
        Ok(report)
    }

    /// Reverts the most recent real batch.
    pub fn undo(&mut self, platform: &dyn Platform) -> Result<(), ExecError> {
        let position = self
            .batches
            .iter()
            .rposition(|b| !b.simulated)
            .ok_or_else(|| ExecError::NothingToUndo(self.journal_dir.clone()))?;
        // Read, not removed — an engine error must leave the batch on the
        // stack, or a failed undo silently costs the user the only handle they
        // had on it.
        let batch = self.batches[position].clone();
        let report = ren_core::exec::undo_transaction(&batch.journal, platform)?;
        self.batches.remove(position);
        self.log.clear();
        for (from, to) in &report.restored {
            self.log.push(LogLine::Restored {
                from: file_name(from),
                to: file_name(to),
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
        self.last_irreversible = report.irreversible.len();
        self.last_restored = report.restored.clone();
        Ok(())
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

    /// Rolls back everything the crash-recovery check found.
    pub fn recover(&mut self, platform: &dyn Platform) -> Result<(), ExecError> {
        self.log.clear();
        // Drained one at a time rather than `mem::take`n up front: with the
        // whole list moved out, one failure and a `?` would drop every
        // remaining transaction, the banner would vanish, and they would never
        // be offered again.
        while let Some(item) = self.unfinished.first().cloned() {
            let report = ren_core::exec::rollback(&item, platform)?;
            self.unfinished.remove(0);
            self.log.push(LogLine::Note(format!(
                "recovered transaction {}: {} rolled back",
                item.txn,
                ren_core::plural(report.restored.len(), "change")
            )));
            // D77's rule, on the other path: a change that could never be
            // taken back is reported here too, not only by `undo`.
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
        }
        Ok(())
    }

    /// Dismisses the recovery offer without touching anything.
    pub fn dismiss_recovery(&mut self) {
        self.unfinished.clear();
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
        history.undo(ren_platform::host().as_ref()).unwrap();

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
        assert!(history.undo(ren_platform::host().as_ref()).is_err());
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
        let journal = history.batches[0].journal.clone();
        let text = std::fs::read_to_string(&journal).unwrap();
        let truncated: String = text
            .lines()
            .filter(|l| !l.contains("\"commit\""))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(&journal, truncated).unwrap();

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
        let journal = history.batches[0].journal.clone();
        let text = std::fs::read_to_string(&journal).unwrap();
        let truncated: String = text
            .lines()
            .filter(|l| !l.contains("\"commit\""))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(&journal, truncated).unwrap();

        let mut restarted = History::in_dir(fixture.journal.path().to_path_buf());
        restarted.recover(ren_platform::host().as_ref()).unwrap();

        assert_eq!(names(&fixture), ["a_1.txt", "a_2.txt"]);
        assert!(restarted.unfinished.is_empty());
    }

    /// A tag-only batch used to render "Renamed 0 item(s)" — the same lie the
    /// post-run status told, waiting in the Undo dropdown for whenever that
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

        // Make the engine fail: the journal it needs is gone.
        let journal = history.batches.last().unwrap().journal.clone();
        std::fs::remove_file(&journal).unwrap();

        assert!(history.undo(platform.as_ref()).is_err());
        assert!(
            history.can_undo(),
            "a failed undo must not consume the batch"
        );
    }
}
