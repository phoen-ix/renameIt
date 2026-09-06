//! Crash recovery.
//!
//! A transaction that has neither `Commit` nor `Undone` never finished — the
//! process died, the machine lost power, or a rename is still in flight. The
//! write-ahead journal means the renames that *did* happen are on disk, so the
//! batch can be rolled back rather than left half-applied.
//!
//! `ren-cli recover` surfaces this; M2 calls the same functions at GUI startup.

use std::path::{Path, PathBuf};

use ren_platform::Platform;

use super::ExecError;
use super::journal::{Journal, Line, Record};
use super::undo::{UndoReport, undo_transaction};

/// An unfinished transaction found on disk.
/// One change that was announced and never confirmed.
///
/// The file this names is the one the user has to look at, and until M6 it was
/// the only thing the report did **not** carry: `Unfinished` aggregated
/// everything into two counts and threw the paths away, so a crashed tag write
/// produced "1 batch did not finish" and no way to find out which file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlight {
    /// The name the file started under.
    ///
    /// For a rename that is deliberately the *old* name: it is what the user
    /// knows the file by, and the file may be sitting under either one — which
    /// is exactly what makes it in flight.
    pub path: PathBuf,
    /// The operation's stable id, or `"rename"`.
    pub op: String,
    /// Whether this one rewrote the file's contents.
    ///
    /// A rename in flight is a file under one of two names and nothing worse; a
    /// tag write in flight is a file that may be **half-written**, because
    /// lofty rewrites in place. They need different sentences.
    pub rewrote_contents: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfinished {
    pub txn: String,
    pub journal: PathBuf,
    /// Changes the journal says completed, and which rolling back would undo.
    pub completed: usize,
    /// Changes written ahead but never confirmed, named rather than counted.
    pub in_flight: Vec<InFlight>,
}

impl Unfinished {
    /// Whether any in-flight change rewrote a file's contents, so the caller
    /// knows whether to say "may be half-written" or merely "under one of two
    /// names".
    pub fn any_contents_rewritten(&self) -> bool {
        self.in_flight.iter().any(|f| f.rewrote_contents)
    }
}

/// Every transaction in `journal_dir` that never reached a terminal record.
///
/// Newest first, so a caller offering "roll back?" asks about the most recent
/// damage first.
pub fn unfinished(journal_dir: &Path) -> Result<Vec<Unfinished>, ExecError> {
    let mut out = Vec::new();
    for path in Journal::list(journal_dir)? {
        let lines = Journal::read(&path)?;
        let terminal = lines
            .iter()
            .any(|l| matches!(l.record, Record::Commit { .. } | Record::Undone { .. }));
        if terminal {
            continue;
        }

        // A journal from a newer build lists here and is then refused by
        // `rollback`, which offers the user a button that cannot work. Better
        // not to offer it: `Line::understood()` is the same check undo makes.
        if !lines.iter().all(Line::understood) {
            continue;
        }

        // Which seqs finished, so the announced-but-unconfirmed ones are what
        // is left. Every announced change counts, not only renames: an action
        // written ahead but never confirmed is in flight exactly as a rename
        // is, and counting one kind would make a crashed metadata batch look
        // finished.
        let settled: std::collections::HashSet<u64> = lines
            .iter()
            .filter_map(|l| match l.record {
                Record::Completed { seq } | Record::Failed { seq, .. } => Some(seq),
                _ => None,
            })
            .collect();

        let mut in_flight = Vec::new();
        for line in &lines {
            let (seq, path, op, rewrote) = match &line.record {
                Record::PlanRename { seq, from, .. } => (*seq, from.clone(), "rename", false),
                Record::PlanAct { seq, path, op, .. } => (*seq, path.clone(), op.as_str(), false),
                // The one that can leave a damaged file rather than an
                // ambiguous name.
                Record::PlanIrreversible { seq, path, op, .. } => {
                    (*seq, path.clone(), op.as_str(), true)
                }
                _ => continue,
            };
            if !settled.contains(&seq) {
                in_flight.push(InFlight {
                    path,
                    op: op.to_owned(),
                    rewrote_contents: rewrote,
                });
            }
        }

        out.push(Unfinished {
            txn: lines.first().map(|l| l.txn.clone()).unwrap_or_default(),
            journal: path,
            // Saturating, because this reads a file from disk at GUI startup
            // and `apply.rs` already relies on it being so: two `Failed`
            // lines sharing a `seq` — a truncated or hand-edited journal —
            // must be a wrong count, never a panic before the window opens.
            completed: settled.len().saturating_sub(
                lines
                    .iter()
                    .filter(|l| matches!(l.record, Record::Failed { .. }))
                    .count(),
            ),
            in_flight,
        });
    }
    Ok(out)
}

/// Rolls an unfinished transaction back.
///
/// This is undo with a different name: the same reverse replay, the same
/// staleness pre-flight, and the same `Undone` stamp afterwards so the
/// transaction is never offered twice. Entries whose file no longer matches the
/// journal are reported and skipped rather than forced.
pub fn rollback(unfinished: &Unfinished, platform: &dyn Platform) -> Result<UndoReport, ExecError> {
    undo_transaction(&unfinished.journal, platform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::journal::Journal;
    use tempfile::TempDir;

    /// Writes a journal that stops after `Completed`, exactly as a crash would
    /// leave it.
    fn crashed_transaction(dir: &Path, from: &Path, to: &Path) -> PathBuf {
        let mut journal = Journal::create(dir).unwrap();
        journal
            .write(Record::Begin {
                platform: "test".into(),
                items: 2,
            })
            .unwrap();
        journal
            .write(Record::PlanRename {
                seq: 0,
                from: from.to_path_buf(),
                to: to.to_path_buf(),
            })
            .unwrap();
        journal.write(Record::Completed { seq: 0 }).unwrap();
        // A second rename that was announced but never confirmed.
        journal
            .write(Record::PlanRename {
                seq: 1,
                from: from.with_file_name("second"),
                to: to.with_file_name("second-new"),
            })
            .unwrap();
        journal.path().to_path_buf()
    }

    #[test]
    fn an_unclosed_transaction_is_reported() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        crashed_transaction(
            journals.path(),
            &tree.path().join("a.txt"),
            &tree.path().join("b.txt"),
        );

        let found = unfinished(journals.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].completed, 1);
        assert_eq!(found[0].in_flight.len(), 1, "the announced rename");
        // Named, not counted: this is the file the user has to go and look at.
        // The name it started under — it may currently be under either.
        assert_eq!(found[0].in_flight[0].path, tree.path().join("second"));
        assert_eq!(found[0].in_flight[0].op, "rename");
        assert!(
            !found[0].any_contents_rewritten(),
            "a rename leaves a file under one of two names, not a damaged one"
        );
    }

    #[test]
    fn a_committed_transaction_is_not_offered_for_recovery() {
        let journals = TempDir::new().unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        journal.write(Record::Completed { seq: 0 }).unwrap();
        journal
            .write(Record::Commit {
                renamed: 1,
                failed: 0,
            })
            .unwrap();

        assert!(unfinished(journals.path()).unwrap().is_empty());
    }

    #[test]
    fn rolling_back_restores_the_renames_that_happened() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let from = tree.path().join("a.txt");
        let to = tree.path().join("b.txt");

        // The crash left the file under its new name.
        std::fs::write(&to, b"payload").unwrap();
        crashed_transaction(journals.path(), &from, &to);

        let platform = ren_platform::host();
        let found = unfinished(journals.path()).unwrap();
        let report = rollback(&found[0], platform.as_ref()).unwrap();

        assert_eq!(report.restored.len(), 1);
        assert!(from.exists(), "the original name is back");
        assert!(!to.exists());
        assert_eq!(std::fs::read(&from).unwrap(), b"payload");
    }

    /// Two `Failed` lines for one `seq` — a journal that was concatenated or
    /// hand-edited — used to underflow the completed count, which is a panic
    /// in debug and nonsense in release, and this runs before the window
    /// opens. Now it is a count of zero and a transaction still offered.
    #[test]
    fn a_journal_with_duplicate_failures_still_reads() {
        let journals = TempDir::new().unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        journal
            .write(Record::Begin {
                platform: "test".into(),
                items: 1,
            })
            .unwrap();
        journal
            .write(Record::PlanRename {
                seq: 0,
                from: PathBuf::from("/x/a"),
                to: PathBuf::from("/x/b"),
            })
            .unwrap();
        for _ in 0..2 {
            journal
                .write(Record::Failed {
                    seq: 0,
                    error: "twice".into(),
                })
                .unwrap();
        }

        let found = unfinished(journals.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].completed, 0);
    }

    #[test]
    fn a_rolled_back_transaction_is_not_offered_again() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let from = tree.path().join("a.txt");
        let to = tree.path().join("b.txt");
        std::fs::write(&to, b"payload").unwrap();
        crashed_transaction(journals.path(), &from, &to);

        let platform = ren_platform::host();
        let found = unfinished(journals.path()).unwrap();
        rollback(&found[0], platform.as_ref()).unwrap();

        assert!(unfinished(journals.path()).unwrap().is_empty());
    }

    #[test]
    fn recovery_skips_files_something_else_has_moved() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let from = tree.path().join("a.txt");
        let to = tree.path().join("b.txt");
        // The journal says b.txt should exist; it does not.
        crashed_transaction(journals.path(), &from, &to);

        let platform = ren_platform::host();
        let found = unfinished(journals.path()).unwrap();
        let report = rollback(&found[0], platform.as_ref()).unwrap();

        assert!(report.restored.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(!report.is_complete());
    }
}
