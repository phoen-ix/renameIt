//! Crash recovery.
//!
//! A transaction that has neither `Commit` nor `Undone` never finished — the
//! process died or the machine lost power part-way. The write-ahead journal
//! means the changes that *did* happen are on disk, so the batch can be rolled
//! back rather than left half-applied. A run that is still going on in another
//! window has neither record either; its journal is locked, and is reported
//! as in use rather than offered (see [`ExecError::JournalInUse`]).
//!
//! `ren-cli recover` surfaces this, and the GUI calls the same functions at
//! startup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ren_platform::Platform;

use super::ExecError;
use super::journal::{Journal, Line, Record};
use super::undo::{UndoReport, undo_transaction};

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
    /// is exactly what makes it in flight. For the second half of a broken
    /// cycle that is the name before the temp name, not the temp name.
    pub path: PathBuf,
    /// The operation's stable id, `"rename"`, or `"write"` for a file a
    /// script asked for.
    pub op: String,
    /// Whether this one rewrote the file's contents.
    ///
    /// A rename in flight is a file under one of two names and nothing worse; a
    /// tag write or a script's write in flight is a file that may be
    /// **half-written**. They need different sentences.
    pub rewrote_contents: bool,
}

/// An unfinished transaction found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfinished {
    pub txn: String,
    pub journal: PathBuf,
    /// Changes the journal says completed, and which rolling back would undo.
    pub completed: usize,
    /// Changes written ahead but never confirmed, named rather than counted.
    pub in_flight: Vec<InFlight>,
}

/// Every transaction in `journal_dir` that never reached a terminal record,
/// and every journal that could not be judged.
///
/// Newest first, so a caller offering "roll back?" asks about the most recent
/// damage first.
///
/// **One journal cannot hide the others.** A journal that cannot be read, or
/// that a run in another window still holds ([`ExecError::JournalInUse`]),
/// goes in the second list with its path, and the scan carries on. It used to
/// fail the whole scan — which the GUI swallowed, so the recovery banner
/// simply never appeared for any of them.
///
/// **A finished journal is judged by its tail.** `Commit` and `Undone` are
/// always the last record written, so a journal ending in one is set aside
/// without being parsed line by line — startup used to parse every journal
/// ever written, in full.
pub fn unfinished(journal_dir: &Path) -> (Vec<Unfinished>, Vec<(PathBuf, ExecError)>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let paths = match Journal::list(journal_dir) {
        Ok(paths) => paths,
        Err(error) => {
            problems.push((journal_dir.to_path_buf(), error));
            return (out, problems);
        }
    };
    for path in paths {
        match examine(&path) {
            Ok(Some(found)) => out.push(found),
            Ok(None) => {}
            Err(error) => problems.push((path, error)),
        }
    }
    (out, problems)
}

/// One journal: unfinished, finished (`None`), or not readable.
fn examine(path: &Path) -> Result<Option<Unfinished>, ExecError> {
    let mut file = Journal::open_idle(path)?;
    if Journal::ends_finished(&mut file, path)? {
        return Ok(None);
    }
    let lines = Journal::read_from(&mut file, path)?;
    let terminal = lines
        .iter()
        .any(|l| matches!(l.record, Record::Commit { .. } | Record::Undone { .. }));
    if terminal {
        return Ok(None);
    }

    // A journal from a newer build lists here and is then refused by
    // `rollback`, which offers the user a button that cannot work. Better not
    // to offer it: `Line::understood()` is the same check undo makes.
    if !lines.iter().all(Line::understood) {
        return Ok(None);
    }

    // A journal that announced nothing touched nothing: the `Begin` could not
    // be followed — the disk filled up first, say. Not a batch that "did not
    // finish", and a rollback of it could only report zero changes.
    let announced = lines.iter().any(|l| {
        matches!(
            l.record,
            Record::PlanRename { .. }
                | Record::PlanCreateDir { .. }
                | Record::PlanWriteFile { .. }
                | Record::PlanAct { .. }
                | Record::PlanIrreversible { .. }
        )
    });
    if !announced {
        return Ok(None);
    }

    // Which seqs finished, so the announced-but-unconfirmed ones are what is
    // left. Every announced change counts, not only renames: an action written
    // ahead but never confirmed is in flight exactly as a rename is, and
    // counting one kind would make a crashed metadata batch look finished.
    let settled: std::collections::HashSet<u64> = lines
        .iter()
        .filter_map(|l| match l.record {
            Record::Completed { seq } | Record::Failed { seq, .. } => Some(seq),
            _ => None,
        })
        .collect();

    // Where a temp name came from: the stage `a → tmp` names `a` for the
    // finish `tmp → b`. Only an *earlier* rename's target counts — a later one
    // moving onto this row's name is a different file taking its place.
    let parked_from: HashMap<&Path, (u64, &Path)> = lines
        .iter()
        .filter_map(|l| match &l.record {
            Record::PlanRename { seq, from, to } => Some((to.as_path(), (*seq, from.as_path()))),
            _ => None,
        })
        .collect();

    let mut in_flight = Vec::new();
    for line in &lines {
        let (seq, path, op, rewrote) = match &line.record {
            Record::PlanRename { seq, from, .. } => {
                let known_as = match parked_from.get(from.as_path()) {
                    Some(&(stage, original)) if stage < *seq => original,
                    _ => from.as_path(),
                };
                (*seq, known_as.to_path_buf(), "rename", false)
            }
            Record::PlanAct { seq, path, op, .. } => (*seq, path.clone(), op.as_str(), false),
            // The ones that can leave a damaged file rather than an ambiguous
            // name. A script's write, create or overwrite, may have stopped
            // part-way — and an overwrite's old contents are gone either way.
            Record::PlanIrreversible { seq, path, op, .. } => {
                (*seq, path.clone(), op.as_str(), true)
            }
            Record::PlanWriteFile { seq, path, .. } => (*seq, path.clone(), "write", true),
            // Not named: a folder creation in flight leaves no file to look
            // at — the folder is there, empty, or it is not — and rollback
            // removes it if it is.
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

    Ok(Some(Unfinished {
        txn: lines.first().map(|l| l.txn.clone()).unwrap_or_default(),
        journal: path.to_path_buf(),
        // Saturating, because this reads a file from disk at GUI startup and
        // `apply.rs` already relies on it being so: two `Failed` lines sharing
        // a `seq` — a truncated or hand-edited journal — must be a wrong count,
        // never a panic before the window opens.
        completed: settled.len().saturating_sub(
            lines
                .iter()
                .filter(|l| matches!(l.record, Record::Failed { .. }))
                .count(),
        ),
        in_flight,
    }))
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

    /// What a scan offers for rollback, asserting nothing went unread.
    fn offered(dir: &Path) -> Vec<Unfinished> {
        let (found, problems) = unfinished(dir);
        assert!(problems.is_empty(), "{problems:?}");
        found
    }

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

        let found = offered(journals.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].completed, 1);
        assert_eq!(found[0].in_flight.len(), 1, "the announced rename");
        // Named, not counted: this is the file the user has to go and look at.
        // The name it started under — it may currently be under either.
        assert_eq!(found[0].in_flight[0].path, tree.path().join("second"));
        assert_eq!(found[0].in_flight[0].op, "rename");
        assert!(
            !found[0].in_flight.iter().any(|f| f.rewrote_contents),
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
        drop(journal);

        assert!(offered(journals.path()).is_empty());
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
        let found = offered(journals.path());
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
        drop(journal);

        let found = offered(journals.path());
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
        let found = offered(journals.path());
        rollback(&found[0], platform.as_ref()).unwrap();

        assert!(offered(journals.path()).is_empty());
    }

    /// A journal whose *middle* is damaged, beside one that genuinely did not
    /// finish. The damaged one is reported on its own; the unfinished one is
    /// still offered. A single unreadable file used to fail the whole scan —
    /// which the GUI swallowed, so the banner simply never appeared.
    #[test]
    fn one_unreadable_journal_does_not_hide_the_others() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        std::fs::write(
            journals.path().join("00000000001-000000-1.jsonl"),
            concat!(
                r#"{"txn":"t","n":0,"v":5,"kind":"begin","platform":"test","items":1}"#,
                "\n",
                "this line is not a record\n",
                r#"{"txn":"t","n":2,"v":5,"kind":"plan_rename","seq":0,"from":"/x/a","to":"/x/b"}"#,
                "\n",
            ),
        )
        .unwrap();
        crashed_transaction(
            journals.path(),
            &tree.path().join("a.txt"),
            &tree.path().join("b.txt"),
        );

        let (found, problems) = unfinished(journals.path());
        assert_eq!(found.len(), 1, "the readable one is still offered");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            matches!(problems[0].1, ExecError::CorruptJournal { line: 2, .. }),
            "{problems:?}"
        );
    }

    /// A run still going on in another window is not a crash. Its journal is
    /// locked for as long as it is being written, so it is reported as in
    /// use — never offered for a rollback that would reverse its renames
    /// while it keeps going — and undo refuses it the same way.
    #[test]
    fn a_journal_still_being_written_is_in_use_not_unfinished() {
        let journals = TempDir::new().unwrap();
        let mut live = Journal::create(journals.path()).unwrap();
        live.write(Record::PlanRename {
            seq: 0,
            from: "/x/a".into(),
            to: "/x/b".into(),
        })
        .unwrap();
        live.write(Record::Completed { seq: 0 }).unwrap();

        let (found, problems) = unfinished(journals.path());
        assert!(found.is_empty(), "{found:?}");
        assert!(
            matches!(problems[..], [(_, ExecError::JournalInUse { .. })]),
            "{problems:?}"
        );
        assert_eq!(Journal::latest_undoable(journals.path()).unwrap(), None);
        let refused = undo_transaction(live.path(), ren_platform::host().as_ref());
        assert!(
            matches!(refused, Err(ExecError::JournalInUse { .. })),
            "{refused:?}"
        );

        // Once the run is over — or the process gone — it is a journal like
        // any other.
        let path = live.path().to_path_buf();
        drop(live);
        assert_eq!(offered(journals.path()).len(), 1);
        assert_eq!(
            Journal::latest_undoable(journals.path()).unwrap(),
            Some(path)
        );
    }

    /// A journal whose last record is `Commit` or `Undone` is finished, and
    /// that is all startup needs to know about it: the terminal record is
    /// always the last one written, so the tail answers the question without
    /// parsing a ten-thousand-rename journal line by line. A damaged line
    /// further up is then nobody's business at startup.
    #[test]
    fn a_finished_journal_is_judged_by_its_last_record() {
        let journals = TempDir::new().unwrap();
        std::fs::write(
            journals.path().join("00000000001-000000-1.jsonl"),
            concat!(
                r#"{"txn":"t","n":0,"v":5,"kind":"begin","platform":"test","items":1}"#,
                "\n",
                "this line is not a record\n",
                r#"{"txn":"t","n":2,"v":5,"kind":"commit","renamed":1,"failed":0}"#,
                "\n",
            ),
        )
        .unwrap();
        assert!(offered(journals.path()).is_empty());
    }

    /// A journal that announced nothing touched nothing. A `Begin` that could
    /// not be followed — the disk filled up before the first intent — is not
    /// a batch that "did not finish", and offering to roll it back offered a
    /// button that could only report zero changes.
    #[test]
    fn a_journal_that_announced_nothing_is_not_unfinished() {
        let journals = TempDir::new().unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        journal
            .write(Record::Begin {
                platform: "test".into(),
                items: 3,
            })
            .unwrap();
        drop(journal);
        assert!(offered(journals.path()).is_empty());
    }

    /// A script's write that was interrupted may have left a half-written
    /// file — and for an overwrite the old contents are already gone. D83
    /// names such a file; this record kind was the one it did not.
    #[test]
    fn an_interrupted_write_is_named_as_possibly_half_written() {
        let journals = TempDir::new().unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        journal
            .write(Record::PlanWriteFile {
                seq: 0,
                path: "/x/list.m3u".into(),
                replaced: true,
                written: None,
            })
            .unwrap();
        drop(journal);

        let found = offered(journals.path());
        assert_eq!(found[0].in_flight.len(), 1, "{:?}", found[0].in_flight);
        assert_eq!(found[0].in_flight[0].path, Path::new("/x/list.m3u"));
        assert!(found[0].in_flight[0].rewrote_contents);
    }

    /// The second half of a broken cycle moves `__renameit-tmp-0` to its real
    /// name. Interrupted there, the file is under the temp name or the target,
    /// and neither is a name the user knows it by — the name it started under,
    /// which the stage recorded, is.
    #[test]
    fn a_cycle_finish_in_flight_is_named_by_the_file_it_started_as() {
        let journals = TempDir::new().unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        for (seq, from, to) in [(0, "/x/a", "/x/__renameit-tmp-0"), (1, "/x/b", "/x/a")] {
            journal
                .write(Record::PlanRename {
                    seq,
                    from: from.into(),
                    to: to.into(),
                })
                .unwrap();
            journal.write(Record::Completed { seq }).unwrap();
        }
        journal
            .write(Record::PlanRename {
                seq: 2,
                from: "/x/__renameit-tmp-0".into(),
                to: "/x/b".into(),
            })
            .unwrap();
        drop(journal);

        let found = offered(journals.path());
        assert_eq!(found[0].in_flight.len(), 1);
        assert_eq!(found[0].in_flight[0].path, Path::new("/x/a"));
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
        let found = offered(journals.path());
        let report = rollback(&found[0], platform.as_ref()).unwrap();

        assert!(report.restored.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(!report.is_complete());
    }
}
