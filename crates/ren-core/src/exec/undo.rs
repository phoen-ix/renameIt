//! Reverse replay of a journalled transaction.
//!
//! P9: one undo step reverts one executed batch. Entries whose current state no
//! longer matches the journal are reported and skipped rather than forced: an
//! undo is only safe while no other program has changed the names in between.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ren_platform::Platform;

use super::ExecError;
use super::journal::{Journal, Record, Written};
use crate::effect::{Before, Effect, TimeSet, TimeStamp};

#[derive(Debug, Clone, Default)]
pub struct UndoReport {
    pub txn: String,
    pub journal: PathBuf,
    /// `(current name, restored name)` pairs, in the order they were reverted.
    ///
    /// **One pair per file**, however many hops it took. A file that broke a
    /// cycle went `a → __renameit-tmp-0 → b`, and undo walks both hops back;
    /// reported hop by hop, a two-file swap "restored 3 items" and named a
    /// temp file the user never saw. The hops are joined into `(b, a)`.
    pub restored: Vec<(PathBuf, PathBuf)>,
    pub skipped: Vec<(PathBuf, String)>,
    /// `(path, what was put back)` for every metadata change reverted.
    pub reverted: Vec<(PathBuf, String)>,
    /// `(path, what was done)` for every change this undo **could not** take
    /// back, because nothing was kept (P2).
    ///
    /// Deliberately not a skip. A skip means "something looked wrong, so I did
    /// not touch it" and makes `is_complete()` false; this means "there was
    /// never anything to put back", which is not a failure of the undo and must
    /// not make `ren-cli undo` exit non-zero for a batch whose renames came
    /// back perfectly.
    pub irreversible: Vec<(PathBuf, String)>,
    /// Files a script wrote and this undo removed again (M7).
    pub removed_files: Vec<PathBuf>,
    /// Subfolders the transaction created and this undo removed again (D31).
    pub removed_dirs: Vec<PathBuf>,
    /// Folders it created but left alone, because something else is in them
    /// now. Not a failure — deleting a folder the user has since filled would
    /// be far worse than leaving an empty-looking one behind. A folder that is
    /// already gone is in neither list: there was nothing to keep or remove.
    pub kept_dirs: Vec<PathBuf>,
    /// The undo happened but could not be recorded in the journal — the
    /// `Undone` stamp failed (a full disk, a folder turned read-only).
    ///
    /// Carried on the report rather than returned as an error, because by
    /// then the files *have* moved and the caller has to say which: an error
    /// dropped this report, and the next press replayed the batch, found
    /// every file already back, and reported them all as skipped. Makes
    /// [`Self::is_complete`] false, so `ren-cli undo` exits 3 (D103) — the
    /// transaction still reads as undoable, and whoever automates against it
    /// has to know.
    pub not_recorded: Option<String>,
}

impl UndoReport {
    pub fn is_complete(&self) -> bool {
        self.skipped.is_empty() && self.not_recorded.is_none()
    }
}

/// Undoes the newest transaction in `journal_dir` that has not been undone yet.
///
/// Refused with [`ExecError::JournalInUse`] while a run in another window is
/// still writing a newer journal: that run is the newest batch, and undoing
/// the one beneath it would move files underneath it
/// ([`Journal::latest_undoable`]).
pub fn undo_last(platform: &dyn Platform, journal_dir: &Path) -> Result<UndoReport, ExecError> {
    let path = Journal::latest_undoable(journal_dir)?
        .ok_or_else(|| ExecError::NothingToUndo(journal_dir.to_path_buf()))?;
    undo_transaction(&path, platform)
}

/// Undoes one specific journal file.
///
/// **One reverse walk over every op the journal announced**, newest first,
/// deciding each by what the journal and the disk say about it:
///
/// * `Failed` — it never happened. Skipped outright: "`to` present, `from`
///   absent" would otherwise read as proof that a failed rename landed, and
///   undo moved a stranger's file that had appeared at `to`.
/// * `Completed` — it happened, and is put back, or skipped with a reason if
///   something has changed it since.
/// * Neither — announced and never confirmed. The op runs *before* its
///   `Completed` is written, and since D168 that record is not even synced on
///   its own, so a crash can leave an op that ran with nothing saying so.
///   The disk decides, as D84 decided for renames: a rename whose new name is
///   there and old name free happened; an action whose value is still the
///   one we wrote happened; a folder that exists was ours (the planner only
///   announces folders that did not exist); a created file that holds exactly
///   what we meant to write is ours. Otherwise it never ran, and there is
///   nothing to say about it.
///
/// Newest first is what makes the disk decisive: each unconfirmed rename's
/// `from` has been vacated by the later ones by the time it is asked for.
/// Actions come off before any rename (they ran after every rename, P46), and
/// a script's files before either, so a file written into a folder the run
/// renamed is removed while that folder still has the name it was written
/// under. Folders come off last, deepest first, once everything that moved
/// into them has moved back out.
pub fn undo_transaction(path: &Path, platform: &dyn Platform) -> Result<UndoReport, ExecError> {
    // Held for the whole undo: nothing else writes this journal meanwhile,
    // and a second window cannot roll the same batch back twice. A run still
    // writing it refuses here (`JournalInUse`).
    let mut file = Journal::open_exclusive(path)?;
    let lines = Journal::read_from(&mut file, path)?;
    Journal::check_understood(path, &lines)?;
    let txn = lines.first().map(|l| l.txn.clone()).unwrap_or_default();

    if lines
        .iter()
        .any(|l| matches!(l.record, Record::Undone { .. }))
    {
        return Err(ExecError::NothingToUndo(path.to_path_buf()));
    }

    let mut planned: HashMap<u64, (PathBuf, PathBuf)> = HashMap::new();
    let mut created: HashMap<u64, PathBuf> = HashMap::new();
    let mut written: HashMap<u64, (PathBuf, Option<Written>)> = HashMap::new();
    let mut acted: HashMap<u64, Act> = HashMap::new();
    let mut irreversible: HashMap<u64, (PathBuf, String)> = HashMap::new();
    let mut finished: HashSet<u64> = HashSet::new();
    let mut failed: HashSet<u64> = HashSet::new();
    // Exhaustive on purpose. This used to end in `_ => {}`, which meant a new
    // record kind would be *silently* skipped — undo quietly not restoring
    // anything, with no error, no skip and no log line. Every arm now either
    // does something or says why it does not.
    for line in &lines {
        match &line.record {
            Record::PlanRename { seq, from, to } => {
                planned.insert(*seq, (from.clone(), to.clone()));
            }
            Record::PlanCreateDir { seq, path } => {
                created.insert(*seq, path.clone());
            }
            Record::PlanWriteFile {
                seq,
                path,
                replaced,
                written: what,
            } => {
                if *replaced {
                    // Nothing was kept, so there is nothing to put back. Same
                    // shape as `PlanIrreversible`: reported, not skipped, so a
                    // batch whose renames came back perfectly still exits zero.
                    irreversible.insert(*seq, (path.clone(), "overwrote a file".to_owned()));
                } else {
                    written.insert(*seq, (path.clone(), *what));
                }
            }
            Record::PlanAct {
                seq,
                path,
                change,
                before,
                ..
            } => {
                acted.insert(
                    *seq,
                    Act {
                        path: path.clone(),
                        change: change.clone(),
                        before: *before,
                    },
                );
            }
            Record::PlanIrreversible { seq, path, op, .. } => {
                irreversible.insert(*seq, (path.clone(), op.clone()));
            }
            Record::Completed { seq } => {
                finished.insert(*seq);
            }
            Record::Failed { seq, .. } => {
                failed.insert(*seq);
            }
            // Bookkeeping, not work: nothing to take back.
            Record::Begin { .. } | Record::Commit { .. } => {}
            // `Undone` is rejected above, and `Unknown` by `check_understood`
            // — reaching either here would mean one of those checks was
            // removed.
            Record::Undone { .. } | Record::Unknown => {}
        }
    }

    let mut report = UndoReport {
        txn: txn.clone(),
        journal: path.to_path_buf(),
        ..Default::default()
    };

    let mut announced: Vec<u64> = planned
        .keys()
        .chain(created.keys())
        .chain(written.keys())
        .chain(acted.keys())
        .chain(irreversible.keys())
        .copied()
        .filter(|seq| !failed.contains(seq))
        .collect();
    announced.sort_unstable_by(|a, b| b.cmp(a));
    announced.dedup();

    let mut hops = Hops::new(&planned);
    let mut folders: Vec<PathBuf> = Vec::new();
    for seq in announced {
        let done = finished.contains(&seq);
        if let Some((path, op)) = irreversible.get(&seq) {
            // Reported, never attempted. Saying so is the whole point: a user
            // who undoes a batch that both renamed and rewrote tags has to be
            // told which half came back. One that never ran is the recovery
            // banner's to name, not this report's.
            if done {
                report
                    .irreversible
                    .push((path.clone(), format!("{op} cannot be undone")));
            }
        } else if let Some(act) = acted.get(&seq) {
            // Unconfirmed: put back only if our value is still the file's —
            // otherwise the change never ran, and there is nothing to say.
            if done || check_value_staleness(platform, act).is_ok() {
                match revert(platform, act) {
                    Ok(Some(what)) => report.reverted.push((act.path.clone(), what)),
                    Ok(None) => {}
                    Err(reason) => report.skipped.push((act.path.clone(), reason)),
                }
            }
        } else if let Some((path, what)) = written.get(&seq) {
            remove_written(&mut report, path, what.as_ref(), done);
        } else if let Some(dir) = created.get(&seq) {
            // A folder announced and never confirmed that exists now is ours:
            // the planner only announces folders that did not exist.
            if done || dir.symlink_metadata().is_ok() {
                folders.push(dir.clone());
            }
        } else if let Some((from, to)) = planned.get(&seq) {
            if let Err(reason) = check_staleness(platform, from, to) {
                // Silent when unconfirmed: a rename that never happened is not
                // a skip, because there is nothing about it to report.
                if done {
                    report.skipped.push((to.clone(), reason));
                }
                continue;
            }
            match platform.rename(to, from) {
                Ok(()) => {
                    platform.notify_shell_changed(from);
                    if let Some(pair) = hops.restored(from, to) {
                        report.restored.push(pair);
                    }
                }
                Err(e) => report.skipped.push((to.clone(), e.to_string())),
            }
        }
    }

    // `remove_dir` refuses a folder that is not empty, which is exactly the
    // check we want and one that no race can defeat.
    folders.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for dir in folders {
        match std::fs::remove_dir(&dir) {
            Ok(()) => report.removed_dirs.push(dir),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => report.kept_dirs.push(dir),
        }
    }

    let n = lines.last().map(|l| l.n + 1).unwrap_or(0);
    let stamped = if stamp_fails() {
        Err(ExecError::io(
            path,
            std::io::Error::new(std::io::ErrorKind::StorageFull, "test: the disk is full"),
        ))
    } else {
        Journal::stamp(
            &mut file,
            path,
            &txn,
            n,
            Record::Undone {
                restored: report.restored.len(),
                skipped: report.skipped.len(),
            },
        )
    };
    if let Err(error) = stamped {
        report.not_recorded = Some(error.to_string());
    }

    Ok(report)
}

// Test seam: makes the `Undone` stamp fail, the one failure that comes after
// the files have already moved.
#[cfg(test)]
thread_local! {
    static FAIL_STAMP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn stamp_fails() -> bool {
    FAIL_STAMP.with(std::cell::Cell::get)
}

#[cfg(not(test))]
fn stamp_fails() -> bool {
    false
}

/// Takes a script's file off again, if it is still the file the run wrote.
///
/// Confirmed: removed, unless the journal knows what was written and the file
/// no longer holds it — then it is the user's file now, and deleting it would
/// lose their edits, so it is a skip with the reason. Already gone is not
/// "removed": nothing was.
///
/// Unconfirmed: removed only when it holds exactly what the run meant to write,
/// which proves it is ours and complete. Anything else is a half-written file
/// or a stranger's, and the recovery banner has already named it.
fn remove_written(report: &mut UndoReport, path: &Path, what: Option<&Written>, done: bool) {
    let intact = what.map(|expected| holds(path, expected));
    let remove = match (done, intact) {
        (true, Some(false)) => {
            if path.symlink_metadata().is_ok() {
                report.skipped.push((
                    path.to_path_buf(),
                    format!(
                        "{} was changed after this run wrote it, so it was left in place",
                        path.display()
                    ),
                ));
            }
            false
        }
        (true, _) => true,
        (false, Some(true)) => true,
        (false, _) => false,
    };
    if !remove {
        return;
    }
    match std::fs::remove_file(path) {
        Ok(()) => report.removed_files.push(path.to_path_buf()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => report.skipped.push((path.to_path_buf(), e.to_string())),
    }
}

/// Whether the file at `path` holds exactly what `expected` describes.
fn holds(path: &Path, expected: &Written) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.len() == expected.len)
        && std::fs::read(path).is_ok_and(|bytes| Written::of(&bytes) == *expected)
}

/// Joins the hops of a broken cycle back into one pair per file.
///
/// A temp name is a path one rename in the journal moved a file *to* and a
/// later one moved it on *from* — which in a plan is only ever a cycle's
/// `__renameit-tmp-N` (P6). Walking back, the finish (`b → tmp`) comes first:
/// the file is parked, and nothing is reported. The stage (`tmp → a`) comes
/// next and reports `(b, a)`.
struct Hops {
    temps: HashSet<PathBuf>,
    /// Where each parked file was before this undo, by its temp name.
    parked: HashMap<PathBuf, PathBuf>,
}

impl Hops {
    fn new(planned: &HashMap<u64, (PathBuf, PathBuf)>) -> Self {
        let left_at: HashMap<&Path, u64> = planned
            .iter()
            .map(|(seq, (from, _))| (from.as_path(), *seq))
            .collect();
        let temps = planned
            .iter()
            .filter(|(seq, (_, to))| left_at.get(to.as_path()).is_some_and(|later| later > seq))
            .map(|(_, (_, to))| to.clone())
            .collect();
        Self {
            temps,
            parked: HashMap::new(),
        }
    }

    /// `to → from` has just been reversed: the pair to report, if any.
    fn restored(&mut self, from: &Path, to: &Path) -> Option<(PathBuf, PathBuf)> {
        if self.temps.contains(from) {
            self.parked.insert(from.to_path_buf(), to.to_path_buf());
            return None;
        }
        let current = if self.temps.contains(to) {
            self.parked.remove(to).unwrap_or_else(|| to.to_path_buf())
        } else {
            to.to_path_buf()
        };
        Some((current, from.to_path_buf()))
    }
}

/// One journalled metadata change, ready to be put back.
#[derive(Debug)]
struct Act {
    path: PathBuf,
    change: Effect,
    before: Before,
}

/// How far apart two timestamps may be and still count as "the same".
///
/// FAT rounds a written timestamp to two seconds and exFAT to ten
/// milliseconds; NTFS keeps 100 ns and ext4 1 ns, and a network redirector
/// keeps whatever it likes. A check demanding equality would report every file
/// on a FAT volume as "changed by something else" and then refuse to undo
/// anything on it — so the tolerance is set by the coarsest filesystem we
/// expect to meet, not by the finest.
const TIME_TOLERANCE: Duration = Duration::from_secs(2);

fn times_agree(a: TimeStamp, b: TimeStamp) -> bool {
    let (a, b) = match (a.to_system(), b.to_system()) {
        (Some(a), Some(b)) => (a, b),
        // Unrepresentable here: nothing useful to compare, so do not block.
        _ => return true,
    };
    a.duration_since(b)
        .or_else(|_| b.duration_since(a))
        .is_ok_and(|d| d <= TIME_TOLERANCE)
}

/// Puts one metadata change back, if the file still carries what we wrote.
///
/// `Ok(None)` means there was nothing to restore — the change touched only
/// components the filesystem never reported.
fn revert(platform: &dyn Platform, act: &Act) -> Result<Option<String>, String> {
    if act.path.symlink_metadata().is_err() {
        return Err(format!("{} no longer exists", act.path.display()));
    }
    check_value_staleness(platform, act)?;

    let restoring = act.before.restoring(&act.change);
    if restoring.is_empty() {
        return Ok(None);
    }
    super::apply::write_effect(&act.path, platform, &restoring).map(|()| {
        platform.notify_shell_changed(&act.path);
        Some(describe_restore(&restoring))
    })
}

/// Is our write still the file's value?
///
/// Deliberately compares against the *after*-image rather than the before: the
/// question is "has something else changed this since we wrote it", not "is it
/// already back where it started".
///
/// Two components are restored but never compared. `accessed`, because reading
/// a file updates it — including, on some systems, the very `get_times` call
/// doing the checking — and FAT stores only its date. `archive`, because the OS
/// sets it on any write and every backup tool clears it. A check on either is a
/// guaranteed false positive, and a false positive here means refusing to undo.
fn check_value_staleness(platform: &dyn Platform, act: &Act) -> Result<(), String> {
    match (&act.change, act.before) {
        // A tag write in a `PlanAct` record is not a state this executor can
        // produce: it journals those as `PlanIrreversible`, with no
        // before-image, and undo buckets them separately (D54). Reaching here
        // means the journal was edited, and the safe reading of an edited
        // journal is to refuse rather than to write.
        (Effect::WriteTags { .. } | Effect::RemoveTags { .. }, _) => {
            Err("the journal records a tag change as undoable, which it never is".to_owned())
        }
        (Effect::Unknown, _) => {
            Err("this line was written by a newer version and cannot be checked".to_owned())
        }
        (Effect::Times(change), Before::Times(before)) => {
            let now = TimeSet::of(platform.get_times(&act.path).map_err(|e| e.to_string())?);
            let expected = TimeSet {
                created: change.created.or(before.created),
                accessed: None,
                modified: change.modified.or(before.modified),
            };
            for (wrote, want, now, which) in [
                (change.created, expected.created, now.created, "created"),
                (change.modified, expected.modified, now.modified, "modified"),
            ] {
                if wrote.is_none() {
                    continue;
                }
                if let (Some(want), Some(now)) = (want, now)
                    && !times_agree(want, now)
                {
                    return Err(format!(
                        "the {which} date of {} was changed by something else",
                        act.path.display()
                    ));
                }
            }
            Ok(())
        }
        (Effect::Attributes(change), Before::Attributes(before)) => {
            let now = platform
                .get_attributes(&act.path)
                .map_err(|e| e.to_string())?;
            let after = change.apply_to(before);
            for (wrote, want, now, which) in [
                (
                    change.read_only,
                    after.read_only,
                    now.read_only,
                    "read-only",
                ),
                (change.hidden, after.hidden, now.hidden, "hidden"),
                (change.system, after.system, now.system, "system"),
            ] {
                if wrote.is_some() && want != now {
                    return Err(format!(
                        "the {which} attribute of {} was changed by something else",
                        act.path.display()
                    ));
                }
            }
            Ok(())
        }
        // Exhaustive on purpose. This used to end in a catch-all, which meant
        // the next `Effect` variant would fall into it and report *every* row
        // as "the journal's record and its before-image disagree" — an
        // accusation of a hand-edited journal, on a correct undo, with
        // `is_complete()` going false and the CLI exiting non-zero. The pairs
        // below are the only ones the executor writes; anything else really is
        // a hand-edited file.
        (Effect::Times(_), Before::Attributes(_)) | (Effect::Attributes(_), Before::Times(_)) => {
            Err("the journal's record and its before-image disagree".to_owned())
        }
    }
}

fn describe_restore(effect: &Effect) -> String {
    match effect {
        Effect::Attributes(_) => "attributes restored".to_owned(),
        Effect::Times(_) => "dates restored".to_owned(),
        // Unreachable through the executor, and named rather than left to a
        // catch-all so the next effect has to be thought about.
        Effect::WriteTags { .. } | Effect::RemoveTags { .. } => "nothing to restore".to_owned(),
        Effect::Unknown => "nothing to restore".to_owned(),
    }
}

/// The pre-flight: the file must still be where the journal left it, and the
/// name we are about to restore must be free.
fn check_staleness(platform: &dyn Platform, from: &Path, to: &Path) -> Result<(), String> {
    if to.symlink_metadata().is_err() {
        return Err(format!("{} no longer exists", to.display()));
    }
    let rules = platform.naming_rules(to);
    let case_only = rules.fold(&from.to_string_lossy()) == rules.fold(&to.to_string_lossy());
    if !case_only && from.symlink_metadata().is_ok() {
        return Err(format!(
            "{} was recreated by something else",
            from.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::TimeSet;
    use crate::exec::recover::{rollback, unfinished};
    use tempfile::TempDir;

    /// A journal that announces `records` and then stops, as a crash leaves it.
    fn crashed(journals: &Path, records: Vec<Record>) {
        let mut journal = Journal::create(journals).unwrap();
        journal
            .write(Record::Begin {
                platform: "test".into(),
                items: records.len(),
            })
            .unwrap();
        for record in records {
            journal.write(record).unwrap();
        }
    }

    fn roll_back(journals: &Path) -> UndoReport {
        let found = unfinished(journals).0;
        assert_eq!(found.len(), 1, "{found:?}");
        rollback(&found[0], ren_platform::host().as_ref()).unwrap()
    }

    /// A folder created and never confirmed. Since D168 a `Completed` is
    /// appended without a sync, so a power cut right after `create_dir` leaves
    /// exactly this, and rollback has to take the folder away again — the
    /// planner only announces folders that did not exist, and `remove_dir`
    /// refuses one that has anything in it.
    #[test]
    fn rollback_removes_a_folder_created_but_never_confirmed() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let folder = tree.path().join("A");
        crashed(
            journals.path(),
            vec![Record::PlanCreateDir {
                seq: 0,
                path: folder.clone(),
            }],
        );
        std::fs::create_dir(&folder).unwrap();

        let report = roll_back(journals.path());
        assert!(!folder.exists(), "{report:?}");
        assert_eq!(report.removed_dirs, [folder]);
    }

    /// Unconfirmed renames are replayed newest first, so each one's `from` has
    /// been vacated by the time it is asked for. A chain of six, all performed
    /// and none confirmed, comes back only in exactly that order — any other
    /// finds a `from` still occupied, reads it as "recreated by something
    /// else", and leaves the file where it is.
    #[test]
    fn unconfirmed_renames_are_replayed_newest_first() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let at = |i: usize| tree.path().join(format!("x{i}"));
        // seq 0: x5 → x6, seq 1: x4 → x5, … seq 5: x0 → x1: the order a chain
        // runs in, from its free end.
        let records = (0..6)
            .map(|seq| Record::PlanRename {
                seq: seq as u64,
                from: at(5 - seq),
                to: at(6 - seq),
            })
            .collect();
        crashed(journals.path(), records);
        for i in 1..=6 {
            std::fs::write(at(i), format!("was x{}", i - 1)).unwrap();
        }

        let report = roll_back(journals.path());
        assert_eq!(report.restored.len(), 6, "{report:?}");
        for i in 0..6 {
            assert_eq!(std::fs::read_to_string(at(i)).unwrap(), format!("was x{i}"));
        }
        assert!(!at(6).exists());
    }

    fn act(seq: u64, path: &Path, before: u64, after: u64) -> Record {
        Record::PlanAct {
            seq,
            path: path.to_path_buf(),
            op: "set_date".into(),
            change: Effect::Times(TimeSet {
                modified: Some(TimeStamp {
                    secs: after as i64,
                    nanos: 0,
                }),
                ..Default::default()
            }),
            before: Before::Times(TimeSet {
                modified: Some(TimeStamp {
                    secs: before as i64,
                    nanos: 0,
                }),
                ..Default::default()
            }),
        }
    }

    fn set_modified(path: &Path, secs: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    fn modified(path: &Path) -> u64 {
        std::fs::metadata(path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// A metadata change that ran and whose confirmation was lost is decided
    /// the way D84 decides a rename: from the disk. The file carries the value
    /// we wrote, so we wrote it, and it goes back.
    #[test]
    fn rollback_reverts_an_action_that_ran_but_was_never_confirmed() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let file = tree.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        crashed(journals.path(), vec![act(0, &file, 1_000_000, 2_000_000)]);
        set_modified(&file, 2_000_000);

        let report = roll_back(journals.path());
        assert_eq!(modified(&file), 1_000_000, "{report:?}");
        assert_eq!(report.reverted.len(), 1);
        assert!(report.is_complete());
    }

    /// And one that never ran is left alone, silently: the file still has its
    /// old value, so there is nothing to put back and nothing to report.
    #[test]
    fn rollback_leaves_an_action_that_never_ran_alone() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let file = tree.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        set_modified(&file, 1_000_000);
        crashed(journals.path(), vec![act(0, &file, 1_000_000, 2_000_000)]);

        let report = roll_back(journals.path());
        assert_eq!(modified(&file), 1_000_000);
        assert!(report.reverted.is_empty(), "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
    }

    /// The `Undone` stamp is the one step that comes after the files have
    /// moved. When it fails, the report of what moved has to survive it — as
    /// an error it was dropped, the CLI exited as if the command line had been
    /// wrong, and the next press replayed the batch and reported every file
    /// as skipped.
    #[test]
    fn an_undo_that_cannot_be_recorded_still_reports_what_it_restored() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let (from, to) = (tree.path().join("a"), tree.path().join("b"));
        std::fs::write(&to, b"x").unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        journal
            .write(Record::PlanRename {
                seq: 0,
                from: from.clone(),
                to: to.clone(),
            })
            .unwrap();
        journal.write(Record::Completed { seq: 0 }).unwrap();
        let path = journal.path().to_path_buf();
        drop(journal);

        FAIL_STAMP.with(|fail| fail.set(true));
        let report = undo_transaction(&path, ren_platform::host().as_ref());
        FAIL_STAMP.with(|fail| fail.set(false));

        let report = report.expect("the files moved, so the report comes back");
        assert_eq!(report.restored, [(to, from.clone())]);
        assert!(report.not_recorded.is_some(), "{report:?}");
        assert!(
            !report.is_complete(),
            "an unrecorded undo is not a complete one"
        );
        assert!(from.exists());
    }

    /// `kept_dirs` means "something else is in it now". A created folder that
    /// is already gone is not that, and reporting it there told the user a
    /// folder had been kept that no longer exists.
    #[test]
    fn a_created_folder_that_is_already_gone_is_not_reported_as_kept() {
        let journals = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        journal
            .write(Record::PlanCreateDir {
                seq: 0,
                path: tree.path().join("gone"),
            })
            .unwrap();
        journal.write(Record::Completed { seq: 0 }).unwrap();
        journal
            .write(Record::Commit {
                renamed: 0,
                failed: 0,
            })
            .unwrap();
        let path = journal.path().to_path_buf();
        drop(journal);

        let report = undo_transaction(&path, ren_platform::host().as_ref()).unwrap();
        assert!(report.kept_dirs.is_empty(), "{report:?}");
    }
}
