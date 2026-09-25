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
    /// Set from another thread to stop the run before its next op.
    ///
    /// Every stopping point is already a consistent one — the journal is
    /// write-ahead per op — so a cancelled run is a shorter run: the ops that
    /// happened are confirmed and committed, and undo reverts exactly them.
    /// The report says it was cancelled, so a front end does not mistake a
    /// shorter run for a finished one.
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Bumped once per op performed, for a front end drawing a progress line
    /// while the run happens on its thread.
    pub progress: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            simulate: false,
            journal_dir: default_journal_dir(),
            allow_irreversible: false,
            cancel: None,
            progress: None,
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
    /// The run was stopped by [`ApplyOptions::cancel`] before it finished.
    /// What happened before the stop is confirmed and committed, and undo
    /// reverts it; what did not happen is simply not in the report.
    pub cancelled: bool,
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
            blockers: plan.blockers.clone(),
        });
    }
    if let Some(path) = plan
        .ops
        .iter()
        .flat_map(op_paths)
        .find(|p| !p.is_absolute())
    {
        return Err(ExecError::RelativePath {
            path: path.to_path_buf(),
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
        let mut parked = Parked::default();
        for op in &plan.ops {
            match op {
                // A simulation reports what the user asked for, not the
                // temp-name bookkeeping that makes it possible.
                PlannedOp::Rename { from, to, kind } => {
                    if let Some(pair) = parked.renamed(from, to, *kind) {
                        report.renamed.push(pair);
                    }
                }
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
    apply_journalled(plan, platform, options, journal, report)
}

/// Every path an op names, for the absolute-path check.
fn op_paths(op: &PlannedOp) -> impl Iterator<Item = &std::path::Path> {
    let (first, second) = match op {
        PlannedOp::Rename { from, to, .. } => (from.as_path(), Some(to.as_path())),
        PlannedOp::CreateDir { path }
        | PlannedOp::WriteFile { path, .. }
        | PlannedOp::Act { path, .. } => (path.as_path(), None),
    };
    std::iter::once(first).chain(second)
}

/// The files a cycle has parked under a temp name, by that name.
///
/// A broken cycle moves its victim twice — `a → tmp`, then `tmp → b` — and
/// only the second hop is a rename the user asked for. Reported as
/// `tmp → b`, it was a pair nobody could use: a front end reads `renamed` as
/// an old → new map, to carry a hand-set order across the relist (D157), to
/// rekey a picture, to write the log. Remembering where each temp name came
/// from reports it as `a → b`.
#[derive(Debug, Default)]
struct Parked(std::collections::HashMap<PathBuf, PathBuf>);

impl Parked {
    /// The pair to report for this hop, if any.
    fn renamed(
        &mut self,
        from: &std::path::Path,
        to: &std::path::Path,
        kind: RenameKind,
    ) -> Option<(PathBuf, PathBuf)> {
        match kind {
            RenameKind::CycleStage => {
                self.0.insert(to.to_path_buf(), from.to_path_buf());
                None
            }
            RenameKind::CycleFinish => Some((
                self.0.remove(from).unwrap_or_else(|| from.to_path_buf()),
                to.to_path_buf(),
            )),
            RenameKind::Direct => Some((from.to_path_buf(), to.to_path_buf())),
        }
    }
}

fn options_cancelled(options: &ApplyOptions) -> bool {
    options
        .cancel
        .as_ref()
        .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
}

/// How many renames are written ahead before any of them runs.
///
/// **One, and the property test is why.** The executor is shaped so that a
/// window of sixty-four is a one-constant change — announce the window,
/// sync once, perform, confirm — and
/// `a_crash_at_any_change_is_recovered_exactly` in `tests/properties.rs`
/// was written to gate exactly that widening. It fails at anything above
/// one, with the same shape every time: a swap through a temp name whose
/// three renames are announced together and *none* performed. Recovery
/// walks the announced-and-unconfirmed ops newest first and decides each
/// from the disk — `to` present and `from` absent means it happened — and
/// for `tmp → readme` with nothing done, `readme` *is* present (it is the
/// original file) and `tmp` *is* absent (it was never made). Recovery then
/// "puts back" a rename that never ran and scrambles the pair.
///
/// A window of one cannot produce that state: at most the op being
/// performed and the one before it are ever unconfirmed at once, and the
/// one before it has always run. Every case the test generates — chains,
/// cycles, subfolder moves, a folder row, metadata actions, a crash before
/// and after every change, and a power cut that loses the unsynced tail —
/// recovers exactly at one.
///
/// Widening it needs recovery to know *which file* is where rather than
/// only which names exist — a file identity in the intent, or a replay of
/// the announced prefix against the disk — which is a journal-format
/// change with its own decision (P99). The halving of syncs D168 delivered
/// comes from `Completed` being appended unsynced and carried by the next
/// intent's sync, not from the window.
pub const WRITE_AHEAD_WINDOW: usize = 1;

/// The run itself, once a journal is open.
///
/// Split from [`apply`] so a test can hand in a journal that fails part-way
/// through — the one failure that turns into [`ExecError::Interrupted`] and
/// that no amount of `chmod` can produce on an already-open file.
///
/// The journal is write-ahead: an op's intent is durable before the
/// filesystem is touched. Renames and folder creations are announced in
/// windows of [`WRITE_AHEAD_WINDOW`] and synced once per window; each op's
/// `Completed` is appended without a sync and made durable by the next
/// window's sync, or by `Commit`. That is one `fdatasync` per op where
/// there used to be two — and it is the whole of what the disk is asked
/// for, so a ten-thousand-file run pays ten thousand syncs rather than
/// twenty thousand. What a crash can lose is unchanged in kind: a change
/// that happened and was not confirmed, which recovery reads from the disk
/// (D84); what is new is that the *previous* op's confirmation can be lost
/// with it, and `an_unconfirmed_rename_before_the_crash_point_is_recovered`
/// holds that recovery is still exact then.
fn apply_journalled(
    plan: &Plan,
    platform: &dyn Platform,
    options: &ApplyOptions,
    mut journal: Journal,
    mut report: ApplyReport,
) -> Result<ApplyReport, ExecError> {
    report.txn = Some(journal.txn().to_owned());
    report.journal = Some(journal.path().to_path_buf());
    if let Err(error) = journal.write(Record::Begin {
        platform: platform.name().to_owned(),
        items: plan.ops.len(),
    }) {
        // Nothing announced, so nothing touched — and a journal left behind
        // here would be "a batch that did not finish" at the next start, with
        // no files and nothing to roll back. Removed; the disk being full is
        // the likeliest reason, and the caller still hears it.
        let path = journal.path().to_path_buf();
        drop(journal);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }

    // From here on a journal write that fails is `ExecError::Interrupted`: the
    // run has started, files may have moved, and the caller has to be told
    // which — see the variant. `completed` is what the message says.
    let mut completed = 0usize;
    let mut parked = Parked::default();

    // Every physical change is journalled, temp-name hops included, so undo
    // unwinds a broken cycle by replaying them in reverse (P6).
    let mut at = 0usize;
    'run: while at < plan.ops.len() {
        // Between ops, never inside one: the op under way finishes and is
        // confirmed, so the journal closes on a consistent state.
        if options_cancelled(options) {
            report.cancelled = true;
            break;
        }
        // The window: a run of windowable ops, or exactly one of anything
        // else.
        let end = if windowable(&plan.ops[at]) {
            plan.ops[at..]
                .iter()
                .take(WRITE_AHEAD_WINDOW)
                .take_while(|op| windowable(op))
                .count()
                + at
        } else {
            at + 1
        };

        // Write-ahead for the whole window, then one sync. An action's
        // before-image is read here, with its intent: "set modified to X"
        // does not say what X replaced, so the intent alone cannot be
        // inverted.
        let mut announced: Vec<(u64, &PlannedOp, Option<String>)> = Vec::with_capacity(end - at);
        for (seq, op) in plan.ops[at..end].iter().enumerate() {
            let seq = (at + seq) as u64;
            let refused = announce(&mut journal, seq, op, platform)
                .map_err(|e| interrupted(&journal, completed, e))?;
            announced.push((seq, op, refused));
        }
        journal
            .sync()
            .map_err(|e| interrupted(&journal, completed, e))?;

        for (seq, op, refused) in announced {
            let subject = subject_of(op);
            let outcome = match refused {
                Some(reason) => Err(reason),
                None => perform(op, platform),
            };
            match outcome {
                Ok(()) => {
                    // The change has happened whether or not this line lands,
                    // so it counts before the append, not after.
                    completed += 1;
                    journal
                        .append(Record::Completed { seq })
                        .map_err(|e| interrupted(&journal, completed, e))?;
                    record_success(&mut report, &mut parked, op, platform);
                    if let Some(progress) = &options.progress {
                        progress.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                    // Stop at the first failure: whatever already happened
                    // stays undoable, and the user decides what to do next.
                    // The rest of this window was announced and never
                    // performed, which recovery and undo read as exactly
                    // that: a target that is not there is a rename that never
                    // happened.
                    break 'run;
                }
            }
        }
        at = end;
    }

    journal
        .write(Record::Commit {
            renamed: report.renamed.len(),
            failed: report.failed.len(),
        })
        .map_err(|e| interrupted(&journal, completed, e))?;

    Ok(report)
}

/// Whether an op may share a write-ahead window with its neighbours.
///
/// A rename or a folder creation announces everything undo needs in its
/// intent — the two names, the path — so any number of them can be
/// announced together. An action carries a before-image and a written file
/// may be an overwrite; those stay one to a window, journalled and synced
/// on their own as they always were.
fn windowable(op: &PlannedOp) -> bool {
    matches!(op, PlannedOp::Rename { .. } | PlannedOp::CreateDir { .. })
}

/// Journals an op's intent. Returns the reason it must not run, if there is
/// one — an action whose before-image could not be read has nothing
/// announced, and a failure recorded in its place.
fn announce(
    journal: &mut Journal,
    seq: u64,
    op: &PlannedOp,
    platform: &dyn Platform,
) -> Result<Option<String>, ExecError> {
    match op {
        PlannedOp::CreateDir { path } => {
            journal.append(Record::PlanCreateDir {
                seq,
                path: path.clone(),
            })?;
        }
        PlannedOp::WriteFile {
            path,
            undoability,
            contents,
        } => {
            // `replaced` comes from the plan's decision rather than a fresh
            // `stat`, so the file the user consented to overwrite is the
            // file the journal says was overwritten. Re-checking here would
            // let the two disagree — and for a create it need not: `perform`
            // refuses to replace anything.
            journal.append(Record::PlanWriteFile {
                seq,
                path: path.clone(),
                replaced: !undoability.is_reversible(),
                written: Some(super::journal::Written::of(contents.as_bytes())),
            })?;
        }
        PlannedOp::Rename { from, to, .. } => {
            journal.append(Record::PlanRename {
                seq,
                from: from.clone(),
                to: to.clone(),
            })?;
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
            journal.append(Record::PlanIrreversible {
                seq,
                path: path.clone(),
                op: (*op).to_owned(),
                change: effect.clone(),
            })?;
        }
        PlannedOp::Act {
            path, op, effect, ..
        } => match read_before(platform, path, effect) {
            // Every journalled action has a before-image, because the one
            // kind that has none took the branch above.
            Ok(Some(before)) => {
                journal.append(Record::PlanAct {
                    seq,
                    path: path.clone(),
                    op: (*op).to_owned(),
                    change: effect.clone(),
                    before,
                })?;
            }
            // No before-image, no attempt. A change that cannot be undone is
            // worse than a change that did not happen, and nothing was
            // written — so the `Failed` record stands alone without a
            // `PlanAct`, which `recover::unfinished` counts with saturating
            // arithmetic for exactly this reason.
            Err(e) => return Ok(Some(e.to_string())),
            // An effect with no before-image reaching the reversible branch
            // means its `undoability()` and the action's `undoable()`
            // disagree — a bug, refused rather than written.
            Ok(None) => {
                return Ok(Some(
                    "this change reports itself as undoable but keeps no before-image".to_owned(),
                ));
            }
        },
    }
    Ok(None)
}

/// The filesystem call an op stands for.
fn perform(op: &PlannedOp, platform: &dyn Platform) -> Result<(), String> {
    match op {
        PlannedOp::CreateDir { path } => std::fs::create_dir(path).map_err(|e| e.to_string()),
        PlannedOp::WriteFile {
            path,
            contents,
            undoability,
        } => write_file(path, contents, *undoability),
        PlannedOp::Rename { from, to, .. } => platform.rename(from, to).map_err(|e| e.to_string()),
        PlannedOp::Act { path, effect, .. } => write_effect(path, platform, effect),
    }
}

/// Writes a script's file, holding a create to being one.
///
/// A write the plan decided creates a file (`Undoability::Full`) was never
/// shown to anyone as destroying one, so it is opened `create_new`: a file
/// that appeared between the preview and the run fails the op rather than
/// being truncated under a promise that nothing was there. An overwrite had
/// P2's consent for exactly that file, and replaces it.
fn write_file(
    path: &std::path::Path,
    contents: &str,
    undoability: Undoability,
) -> Result<(), String> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if undoability.is_reversible() {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    let mut file = options.open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!(
            "{} appeared after the preview, and this run was not asked to replace it",
            path.display()
        ),
        _ => e.to_string(),
    })?;
    file.write_all(contents.as_bytes())
        .map_err(|e| e.to_string())
}

/// The path a failure is reported against.
fn subject_of(op: &PlannedOp) -> PathBuf {
    match op {
        PlannedOp::CreateDir { path }
        | PlannedOp::WriteFile { path, .. }
        | PlannedOp::Act { path, .. } => path.clone(),
        PlannedOp::Rename { from, .. } => from.clone(),
    }
}

/// What the report says about an op that succeeded.
fn record_success(
    report: &mut ApplyReport,
    parked: &mut Parked,
    op: &PlannedOp,
    platform: &dyn Platform,
) {
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
        // The same bookkeeping the simulate branch does, and for the same
        // reason — without it two files swapping names reported **three**
        // renames and the status bar could say "Renamed 3 of 2", and the one
        // that went through a temp name was reported from it. The journal
        // still records every hop, because undo needs them; the *report* is
        // what the user reads.
        PlannedOp::Rename { from, to, kind } => {
            platform.notify_shell_changed(to);
            report.reversible += 1;
            if let Some(pair) = parked.renamed(from, to, *kind) {
                report.renamed.push(pair);
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
    use std::path::Path;

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

        let error = apply_journalled(
            &plan,
            platform.as_ref(),
            &ApplyOptions::default(),
            journal,
            report,
        )
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
        let unfinished = crate::exec::unfinished(journals.path()).0;
        assert_eq!(unfinished.len(), 1);
        assert_eq!(unfinished[0].completed, 2);
        let report = crate::exec::rollback(&unfinished[0], platform.as_ref()).unwrap();
        assert_eq!(report.restored.len(), 2, "{report:?}");
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a_1.txt", "a_2.txt", "a_3.txt"]);
    }

    /// A journal that cannot even record its `Begin` has announced nothing and
    /// touched nothing, so it must not stay behind: at the next start it would
    /// be "a batch that did not finish", with no files and nothing to roll
    /// back. Removed, and the error still reaches the caller.
    #[test]
    fn a_journal_that_cannot_record_its_start_is_removed() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a_1.txt"), b"x").unwrap();
        let entries = crate::list(dir.path(), Default::default()).unwrap();
        let pipeline = Pipeline::new().then(Replace::new("_", "-"));
        let platform = ren_platform::host();
        let plan = plan(&entries, &pipeline, platform.as_ref());

        let journals = tempfile::TempDir::new().unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        journal.fail_writes_from(0);
        let error = apply_journalled(
            &plan,
            platform.as_ref(),
            &ApplyOptions::default(),
            journal,
            ApplyReport::default(),
        )
        .expect_err("the Begin write fails");
        assert!(matches!(error, ExecError::Io { .. }), "{error:?}");
        assert_eq!(
            std::fs::read_dir(journals.path()).unwrap().count(),
            0,
            "the empty journal was left behind"
        );
        assert!(dir.path().join("a_1.txt").exists());
    }

    /// A journal records paths exactly as the plan gives them, and a relative
    /// one resolves against the working directory *at undo time*. Refused
    /// before a journal exists, whatever the op.
    #[test]
    fn a_plan_that_names_a_relative_path_is_refused() {
        let journals = tempfile::TempDir::new().unwrap();
        let options = ApplyOptions {
            journal_dir: journals.path().to_path_buf(),
            ..Default::default()
        };
        let plan = Plan {
            ops: vec![PlannedOp::Rename {
                from: PathBuf::from("a.txt"),
                to: PathBuf::from("b.txt"),
                kind: RenameKind::Direct,
            }],
            ..Default::default()
        };
        let error = apply(&plan, ren_platform::host().as_ref(), &options).unwrap_err();
        assert!(
            matches!(&error, ExecError::RelativePath { path } if path == Path::new("a.txt")),
            "{error:?}"
        );
        assert_eq!(std::fs::read_dir(journals.path()).unwrap().count(), 0);
    }

    /// A cancelled run is a shorter run: what happened before the stop is
    /// confirmed and committed, the report says it stopped, and undo reverts
    /// exactly what happened. Cancelled through the platform, because that is
    /// the one hook a test has between two ops.
    #[test]
    fn a_cancelled_run_stops_between_ops_and_is_undoable() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let dir = tempfile::TempDir::new().unwrap();
        for name in ["a_1.txt", "a_2.txt", "a_3.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let entries = crate::list(dir.path(), Default::default()).unwrap();
        let pipeline = Pipeline::new().then(Replace::new("_", "-"));
        let platform = ren_platform::host();
        let plan = plan(&entries, &pipeline, platform.as_ref());

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let progress = std::sync::Arc::new(AtomicUsize::new(0));
        let journals = tempfile::TempDir::new().unwrap();
        let options = ApplyOptions {
            journal_dir: journals.path().to_path_buf(),
            cancel: Some(cancel.clone()),
            progress: Some(progress.clone()),
            ..Default::default()
        };

        // A platform that pulls the flag after the first rename.
        struct CancelAfterOne {
            inner: std::sync::Arc<dyn Platform>,
            cancel: std::sync::Arc<AtomicBool>,
        }
        impl std::fmt::Debug for CancelAfterOne {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("CancelAfterOne")
            }
        }
        impl Platform for CancelAfterOne {
            fn name(&self) -> &'static str {
                self.inner.name()
            }
            fn capabilities(&self) -> &'static [ren_platform::Capability] {
                self.inner.capabilities()
            }
            fn rename(
                &self,
                from: &std::path::Path,
                to: &std::path::Path,
            ) -> ren_platform::Result<()> {
                self.cancel.store(true, Ordering::Relaxed);
                self.inner.rename(from, to)
            }
            fn replace_file(
                &self,
                temp: &std::path::Path,
                target: &std::path::Path,
            ) -> ren_platform::Result<()> {
                self.inner.replace_file(temp, target)
            }
            fn get_attributes(
                &self,
                path: &std::path::Path,
            ) -> ren_platform::Result<ren_platform::FileAttributes> {
                self.inner.get_attributes(path)
            }
            fn set_attributes(
                &self,
                path: &std::path::Path,
                change: ren_platform::AttributeChange,
            ) -> ren_platform::Result<()> {
                self.inner.set_attributes(path, change)
            }
            fn get_times(
                &self,
                path: &std::path::Path,
            ) -> ren_platform::Result<ren_platform::FileTimes> {
                self.inner.get_times(path)
            }
            fn set_times(
                &self,
                path: &std::path::Path,
                change: ren_platform::TimeChange,
            ) -> ren_platform::Result<()> {
                self.inner.set_times(path, change)
            }
            fn naming_rules(&self, path: &std::path::Path) -> &'static ren_platform::NamingRules {
                self.inner.naming_rules(path)
            }
            fn case_sensitivity(&self, dir: &std::path::Path) -> ren_platform::CaseSensitivity {
                self.inner.case_sensitivity(dir)
            }
            fn reveal_in_file_manager(&self, path: &std::path::Path) -> ren_platform::Result<()> {
                self.inner.reveal_in_file_manager(path)
            }
            fn notify_shell_changed(&self, path: &std::path::Path) {
                self.inner.notify_shell_changed(path);
            }
        }
        let cancelling = CancelAfterOne {
            inner: platform.clone(),
            cancel: cancel.clone(),
        };

        let report = apply(&plan, &cancelling, &options).unwrap();
        assert!(report.cancelled);
        assert_eq!(report.renamed.len(), 1);
        assert_eq!(progress.load(Ordering::Relaxed), 1);
        assert!(report.is_success(), "a stop is not a failure");

        // Committed, so it is offered for undo rather than for recovery, and
        // undo puts back exactly the one.
        assert!(crate::exec::unfinished(journals.path()).0.is_empty());
        let undo = crate::exec::undo_last(platform.as_ref(), journals.path()).unwrap();
        assert_eq!(undo.restored.len(), 1);
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a_1.txt", "a_2.txt", "a_3.txt"]);
    }

    /// The state the unsynced `Completed` line makes possible: op *k* ran
    /// and its confirmation was lost with the crash, and op *k+1* was
    /// announced (its intent's sync is what should have carried the
    /// confirmation, and did not reach the disk in time). Both are in flight;
    /// recovery has to put back exactly the one that ran. A swap through a
    /// temp name, because that is the shape whose announced-but-unrun op has
    /// a target that exists.
    #[test]
    fn an_unconfirmed_rename_before_the_crash_point_is_recovered() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"payload a").unwrap();
        std::fs::write(&b, b"payload b").unwrap();
        let tmp = dir.path().join("__renameit-tmp-0");

        // The journal a crash would leave: the stage announced, performed,
        // and its `Completed` lost; the middle rename announced and never
        // performed.
        let journals = tempfile::TempDir::new().unwrap();
        let mut journal = Journal::create(journals.path()).unwrap();
        journal
            .write(Record::Begin {
                platform: "test".into(),
                items: 3,
            })
            .unwrap();
        journal
            .write(Record::PlanRename {
                seq: 0,
                from: a.clone(),
                to: tmp.clone(),
            })
            .unwrap();
        std::fs::rename(&a, &tmp).unwrap();
        journal
            .write(Record::PlanRename {
                seq: 1,
                from: b.clone(),
                to: a.clone(),
            })
            .unwrap();
        drop(journal);

        let platform = ren_platform::host();
        let unfinished = crate::exec::unfinished(journals.path()).0;
        assert_eq!(unfinished[0].in_flight.len(), 2);
        let report = crate::exec::rollback(&unfinished[0], platform.as_ref()).unwrap();
        assert_eq!(report.restored.len(), 1, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(std::fs::read(&a).unwrap(), b"payload a");
        assert_eq!(std::fs::read(&b).unwrap(), b"payload b");
        assert!(!tmp.exists());
    }
}
