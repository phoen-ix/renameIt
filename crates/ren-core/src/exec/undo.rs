//! Reverse replay of a journalled transaction.
//!
//! P9: one undo step reverts one executed batch. Entries whose current state no
//! longer matches the journal are reported and skipped rather than forced: an
//! undo is only safe while no other program has changed the names in between.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ren_platform::Platform;

use super::ExecError;
use super::journal::{Journal, Record};
use crate::effect::{Before, Effect, TimeSet, TimeStamp};

#[derive(Debug, Clone, Default)]
pub struct UndoReport {
    pub txn: String,
    pub journal: PathBuf,
    /// `(current name, restored name)` pairs, in the order they were reverted.
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
    /// be far worse than leaving an empty-looking one behind.
    pub kept_dirs: Vec<PathBuf>,
}

impl UndoReport {
    pub fn is_complete(&self) -> bool {
        self.skipped.is_empty()
    }
}

/// Undoes the newest transaction in `journal_dir` that has not been undone yet.
pub fn undo_last(platform: &dyn Platform, journal_dir: &Path) -> Result<UndoReport, ExecError> {
    let path = Journal::latest_undoable(journal_dir)?
        .ok_or_else(|| ExecError::NothingToUndo(journal_dir.to_path_buf()))?;
    undo_transaction(&path, platform)
}

/// Undoes one specific journal file.
pub fn undo_transaction(path: &Path, platform: &dyn Platform) -> Result<UndoReport, ExecError> {
    let lines = Journal::read(path)?;
    Journal::check_understood(path, &lines)?;
    let txn = lines.first().map(|l| l.txn.clone()).unwrap_or_default();

    if lines
        .iter()
        .any(|l| matches!(l.record, Record::Undone { .. }))
    {
        return Err(ExecError::NothingToUndo(path.to_path_buf()));
    }

    // Only operations the journal says actually happened.
    let mut planned: HashMap<u64, (PathBuf, PathBuf)> = HashMap::new();
    let mut created: HashMap<u64, PathBuf> = HashMap::new();
    let mut written: HashMap<u64, PathBuf> = HashMap::new();
    let mut acted: HashMap<u64, Act> = HashMap::new();
    let mut irreversible: HashMap<u64, (PathBuf, String)> = HashMap::new();
    let mut completed: Vec<u64> = Vec::new();
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
            } => {
                if *replaced {
                    // Nothing was kept, so there is nothing to put back. Same
                    // shape as `PlanIrreversible`: reported, not skipped, so a
                    // batch whose renames came back perfectly still exits zero.
                    irreversible.insert(*seq, (path.clone(), "overwrote a file".to_owned()));
                } else {
                    written.insert(*seq, path.clone());
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
            Record::Completed { seq } => completed.push(*seq),
            // Bookkeeping, not work: nothing to take back.
            Record::Begin { .. } | Record::Failed { .. } | Record::Commit { .. } => {}
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

    // Renames that were written ahead and never confirmed.
    //
    // The rename syscall happens *before* its `Completed` record is written, so
    // a crash in that window leaves the file under its **new** name with
    // nothing in the journal saying it landed. Replaying only `completed` — as
    // this did until M6 — left those files renamed for good, while
    // `Unfinished.in_flight`'s own doc comment promised *"a file that may be
    // under either name — recovery checks before touching it"*. Nothing
    // checked.
    //
    // `check_staleness` is that check, and it is decisive in both directions:
    // if the new name is absent the rename never happened and there is nothing
    // to do; if it is there and the old name is free, the rename landed and
    // comes back. First, so it is unwound before anything earlier in the batch.
    // A set for the membership test: `completed` is a `Vec` because its order
    // is the replay order, and `contains` on it made undoing a 10 000-file
    // batch a hundred million comparisons.
    let finished: std::collections::HashSet<u64> = completed.iter().copied().collect();
    let in_flight: Vec<u64> = planned
        .keys()
        .copied()
        .filter(|seq| !finished.contains(seq))
        .collect();
    let mut folders_in_flight: Vec<PathBuf> = Vec::new();
    for seq in in_flight.into_iter().rev() {
        // A folder announced and never confirmed: the planner only announces
        // folders that did not exist, so one that exists now is ours — and it
        // is removed after the renames below, once the files that may have
        // moved into it have moved back out. `remove_dir` refusing a folder
        // that is not empty is the guard against being wrong about that.
        if let Some(dir) = created.get(&seq) {
            folders_in_flight.push(dir.clone());
            continue;
        }
        let Some((from, to)) = planned.get(&seq) else {
            continue;
        };
        // Silent when the rename never happened: an announced-but-not-performed
        // rename is not a skip, because there is nothing about it to report.
        if check_staleness(platform, from, to).is_err() {
            continue;
        }
        match platform.rename(to, from) {
            Ok(()) => {
                platform.notify_shell_changed(from);
                report.restored.push((to.clone(), from.clone()));
            }
            Err(e) => report.skipped.push((to.clone(), e.to_string())),
        }
    }

    // Reverse order, which is already right for actions: a file that was
    // renamed *and* acted on has its action reverted first, while the recorded
    // path still resolves, and only then is the rename taken back.
    for seq in completed.iter().copied().rev() {
        if let Some((path, op)) = irreversible.get(&seq) {
            // Reported, never attempted. Saying so is the whole point: a user
            // who undoes a batch that both renamed and rewrote tags has to be
            // told which half came back.
            report
                .irreversible
                .push((path.clone(), format!("{op} cannot be undone")));
            continue;
        }
        if let Some(act) = acted.get(&seq) {
            match revert(platform, act) {
                Ok(Some(what)) => report.reverted.push((act.path.clone(), what)),
                Ok(None) => {}
                Err(reason) => report.skipped.push((act.path.clone(), reason)),
            }
            continue;
        }
        let Some((from, to)) = planned.get(&seq) else {
            continue;
        };
        if let Err(reason) = check_staleness(platform, from, to) {
            report.skipped.push((to.clone(), reason));
            continue;
        }
        match platform.rename(to, from) {
            Ok(()) => {
                platform.notify_shell_changed(from);
                report.restored.push((to.clone(), from.clone()));
            }
            Err(e) => report.skipped.push((to.clone(), e.to_string())),
        }
    }

    // Files this transaction created come off before the folders, so a script
    // that wrote a playlist into a folder the same run created leaves that
    // folder empty and removable rather than stubbornly occupied.
    for path in completed.iter().rev().filter_map(|seq| written.get(seq)) {
        match std::fs::remove_file(path) {
            Ok(()) => report.removed_files.push(path.clone()),
            // Already gone is the outcome we wanted; anything else is a skip
            // the user should see.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.removed_files.push(path.clone());
            }
            Err(e) => report.skipped.push((path.clone(), e.to_string())),
        }
    }

    // Folders the transaction created come off last and deepest first, once
    // the files it put in them have moved back out. The announced-and-never-
    // confirmed ones go with them: silent when never created, because a
    // folder that is not there is not a skip.
    let mut folders: Vec<PathBuf> = completed
        .iter()
        .rev()
        .filter_map(|seq| created.get(seq).cloned())
        .chain(
            folders_in_flight
                .into_iter()
                .filter(|dir| dir.symlink_metadata().is_ok()),
        )
        .collect();
    folders.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for dir in folders {
        // `remove_dir` refuses a folder that is not empty, which is exactly the
        // check we want and one that no race can defeat.
        match std::fs::remove_dir(&dir) {
            Ok(()) => report.removed_dirs.push(dir),
            Err(_) => report.kept_dirs.push(dir),
        }
    }

    let n = lines.last().map(|l| l.n + 1).unwrap_or(0);
    Journal::append_to(
        path,
        &txn,
        n,
        Record::Undone {
            restored: report.restored.len(),
            skipped: report.skipped.len(),
        },
    )?;

    Ok(report)
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
