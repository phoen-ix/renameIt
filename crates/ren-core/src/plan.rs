//! Turning a pipeline plus a listing into an ordered, conflict-checked plan.
//!
//! The plan is a pure function of (entries, pipeline, platform rules) — the
//! preview the user sees and the thing the executor runs are the same object,
//! which is what makes "previewed name == on-disk name" testable.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use ren_platform::{NameProblem, Platform};

use crate::effect::{Effect, PlannedAction, Undoability};
use crate::model::FileEntry;
use crate::pipeline::{Pipeline, evaluate_all};

/// **D31.** The one character a produced name may contain that is not part of
/// the name: it moves the file into a subfolder of the folder it is already in.
/// `<\>` renders it, and a user can type it directly.
///
/// One canonical separator, not two. `\` is a perfectly legal character in a
/// POSIX filename, so treating it as a separator would corrupt names that
/// contain it; on Windows it is illegal anyway and the naming rules reject it
/// with a clear message.
pub const SUBFOLDER_SEPARATOR: char = '/';

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    /// Two or more listed items would end up with the same name.
    DuplicateTarget { others: Vec<usize> },
    /// A file that is not part of this batch already occupies the target.
    TargetExists,
    /// The target is not a legal name on this filesystem.
    InvalidName(NameProblem),
    /// A subfolder the target needs is blocked by a file of the same name.
    ///
    /// `<\>` turns a rename into a move (D31), and the folder it wants may
    /// already exist as an *ordinary file* — renaming `(` to `(/(` cannot work.
    /// Reported here rather than left to fail mid-batch, because P4 is that a
    /// conflict blocks the run.
    BlockedByFile { path: PathBuf },
    /// This row's new name is also a folder the same run has to create.
    ///
    /// The mirror of [`Self::BlockedByFile`], and the half that cannot be seen
    /// by looking at the disk: there, an *existing* file blocks a folder a move
    /// needs; here, the file is one this run is about to produce. One row wants
    /// `.../sub` to be its new name and another needs `.../sub` to be the
    /// directory it moves into, and whichever runs second fails — the folder
    /// creation on `EEXIST`, or the rename onto a directory.
    ///
    /// Neither `DuplicateTarget` nor `TargetExists` can catch it. The first
    /// compares rows against each other and these two rows have *different*
    /// targets (`.../sub` and `.../sub/x`); the second reads the destination
    /// directory, and the directory does not exist yet. Found by the M8
    /// property generator, reported after 1.0.
    NeededAsFolder { path: PathBuf },
    /// A folder whose target is inside the folder itself.
    ///
    /// `<\>` moves a file into a subfolder of the folder it is already in
    /// (D31), and for a *folder* row that subfolder can be the folder itself:
    /// `1` in `/x` with `<Name><\><Name>` asks for `/x/1` → `/x/1/1`. No
    /// filesystem can do it — Linux answers `EINVAL`, Windows
    /// `ERROR_SHARING_VIOLATION` — and P4 is that a conflict blocks the run
    /// rather than failing mid-batch. Found by the M8 property generator, once
    /// it started putting folders in the tree.
    IntoItself,
    /// Part of a rename cycle that could not be broken.
    ///
    /// Cycles are normally resolved with a temp name (P6). This is only
    /// reported when no free temp name could be found in the directory, which
    /// takes a deliberately hostile set of existing files.
    UnresolvedCycle,
    /// This platform cannot do what a step asks (P5).
    ///
    /// Caught here rather than at write time so the run is blocked before
    /// anything happens (P4). The alternative is what a Windows-authored "set
    /// created date" preset would otherwise do on Linux: open a journal, fail
    /// on file 1, and leave 9 999 files untouched behind a half-open
    /// transaction.
    Unsupported {
        capability: ren_platform::Capability,
        platform: &'static str,
    },
}

impl fmt::Display for ConflictKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTarget { others } => {
                write!(
                    f,
                    "same target as {}",
                    crate::plural(others.len(), "other item")
                )
            }
            Self::TargetExists => write!(f, "a file with that name already exists"),
            Self::BlockedByFile { path } => write!(
                f,
                "{} is a file, so it cannot also be the folder this moves into",
                path.display()
            ),
            Self::NeededAsFolder { path } => write!(
                f,
                "{} is a folder this run has to create, so it cannot also be this file's new name",
                path.display()
            ),
            Self::InvalidName(p) => write!(f, "invalid name: {p}"),
            Self::IntoItself => write!(f, "a folder cannot be moved inside itself"),
            Self::UnresolvedCycle => write!(
                f,
                "part of a rename cycle, and no free temporary name was available"
            ),
            // Word for word `PlatformError::CapabilityUnsupported`, so the
            // message reads identically wherever the user meets it.
            Self::Unsupported {
                capability,
                platform,
            } => write!(f, "{capability} is not supported on {platform}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    Changed,
    Unchanged,
    Conflict(ConflictKind),
    /// The pipeline itself failed for this file.
    Error(String),
}

impl RowState {
    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Conflict(_))
    }
    pub fn is_changed(&self) -> bool {
        matches!(self, Self::Changed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// Index into the entry list the plan was built from.
    pub index: usize,
    pub source: PathBuf,
    pub new_name: String,
    pub target: PathBuf,
    /// What happens to this row's *name*. Deliberately still only about the
    /// name: a row can be renamed *and* acted on, so one enum cannot carry
    /// both, and every analysis pass that filters on `is_changed()` means
    /// "does this file move" — which an action never does.
    pub state: RowState,
    /// What the run will do to the file besides renaming it, in step order.
    /// Empty for every pipeline built before M5.
    pub actions: Vec<PlannedAction>,
}

impl PlanItem {
    pub fn acts(&self) -> bool {
        !self.actions.is_empty()
    }

    /// True if this run touches the file at all.
    pub fn affected(&self) -> bool {
        self.state.is_changed() || self.acts()
    }

    /// Where the file will be when its actions run: its target if it moves,
    /// otherwise where it already is.
    pub fn final_path(&self) -> &Path {
        if self.state.is_changed() {
            &self.target
        } else {
            &self.source
        }
    }
}

/// Why a rename is in the plan, which matters for reading an execution log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameKind {
    /// The ordinary case: straight from the source to its final name.
    Direct,
    /// Moves a file out of the way under a temp name so a cycle can unwind
    /// (P6). Always paired with a [`RenameKind::CycleFinish`].
    CycleStage,
    /// Moves a staged file from its temp name to its real target.
    CycleFinish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedOp {
    /// A subfolder a rename needs (D31). Emitted parents-first, before every
    /// rename, and journalled so undo can take it back.
    CreateDir { path: PathBuf },
    /// A file an operation asked for — today, a script's `done()` (M7).
    ///
    /// A run-level operation like [`Self::CreateDir`], not a row: it belongs to
    /// the batch rather than to any one file, and the same rule decides whether
    /// it can be taken back. Creating a file that was not there is reversible,
    /// because removing it restores exactly what was; **overwriting one is
    /// not**, because the previous contents are gone and nothing kept them.
    ///
    /// The distinction is settled here rather than in the executor, because P2
    /// requires it to be known *before* a journal is opened — the confirmation
    /// is what stands between the user and a change nobody can undo.
    WriteFile {
        path: PathBuf,
        contents: String,
        undoability: Undoability,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
        kind: RenameKind,
    },
    /// A metadata change, at the file's **final** path.
    Act {
        path: PathBuf,
        op: &'static str,
        effect: Effect,
        /// P2: whether this can be taken back. The executor reads it *before*
        /// opening a journal.
        undoability: Undoability,
        /// What the log and the report say happened.
        describe: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub items: Vec<PlanItem>,
    /// Renames in the order they must execute.
    pub ops: Vec<PlannedOp>,
    /// Lines for the log, from operations that produced something to say.
    ///
    /// Today that is scripting: a `done()` return value goes to the log, and a
    /// script that went wrong is reported the same way rather than stopping the
    /// batch. A note never blocks execution — see
    /// [`Self::is_executable`], which does not consult it.
    pub notes: Vec<String>,
}

impl Plan {
    /// Rows that get a new name.
    ///
    /// Unchanged meaning, deliberately: five analysis passes, the CLI summary
    /// and a good many tests all read it as "this file moves". The new question
    /// gets a new name rather than quietly redefining this one.
    pub fn changed(&self) -> usize {
        self.items.iter().filter(|i| i.state.is_changed()).count()
    }

    /// Rows with at least one side effect, renamed or not.
    pub fn acted(&self) -> usize {
        self.items.iter().filter(|i| i.acts()).count()
    }

    /// Rows this run touches at all. Never double-counts a row that is both
    /// renamed and acted on.
    pub fn affected(&self) -> usize {
        self.items.iter().filter(|i| i.affected()).count()
    }

    /// Rows the run leaves completely alone.
    pub fn unchanged(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.state == RowState::Unchanged && !i.acts())
            .count()
    }

    pub fn conflicts(&self) -> usize {
        self.items.iter().filter(|i| i.state.is_conflict()).count()
    }

    pub fn errors(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.state, RowState::Error(_)))
            .count()
    }

    /// The worst thing this run does, as far as taking it back goes (P2).
    ///
    /// `max()` over every action, because a batch that renames a thousand files
    /// and rewrites one tag block is, as a whole, a batch that cannot be fully
    /// undone — and that is the sentence the confirmation has to say.
    pub fn undoability(&self) -> Undoability {
        self.items
            .iter()
            .flat_map(|item| item.actions.iter())
            .map(|action| action.undoability)
            .chain(self.write_undoability())
            .max()
            .unwrap_or(Undoability::Full)
    }

    /// Changes this run cannot take back.
    ///
    /// Counted in the units the user reads, which is why the two halves are
    /// added rather than kept apart: a row whose tags are rewritten is one such
    /// change, and a file overwritten wholesale is another. The confirmation
    /// says "Change N item(s)" and N has to mean something.
    pub fn irreversible(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.actions.iter().any(|a| !a.undoability.is_reversible()))
            .count()
            + self
                .write_undoability()
                .filter(|u| !u.is_reversible())
                .count()
    }

    /// Run-level writes this plan carries (M7), by whether they can be undone.
    fn write_undoability(&self) -> impl Iterator<Item = Undoability> + '_ {
        self.ops.iter().filter_map(|op| match op {
            PlannedOp::WriteFile { undoability, .. } => Some(*undoability),
            _ => None,
        })
    }

    /// Files this run will write that already exist, with what replaces them.
    ///
    /// For the confirmation, which has to name what it is about to destroy
    /// rather than only count it.
    pub fn overwrites(&self) -> impl Iterator<Item = &Path> + '_ {
        self.ops.iter().filter_map(|op| match op {
            PlannedOp::WriteFile {
                path,
                undoability: Undoability::None,
                ..
            } => Some(path.as_path()),
            _ => None,
        })
    }

    /// P4: conflicts hard-block execution rather than being skipped silently.
    pub fn is_executable(&self) -> bool {
        self.conflicts() == 0 && self.errors() == 0
    }
}

/// Builds a plan. Pure apart from the `exists` probe against the destination.
pub fn plan(entries: &[FileEntry], pipeline: &Pipeline, platform: &dyn Platform) -> Plan {
    let evaluated = evaluate_all(entries, pipeline);
    let mut items = Vec::with_capacity(entries.len());

    let mut lossy_names = 0usize;
    for (index, (entry, result)) in entries.iter().zip(evaluated).enumerate() {
        let rules = platform.naming_rules(&entry.path);
        // A name the listing could not read as text is left completely alone.
        //
        // `file_name` holds U+FFFD where the disk holds something else, so any
        // name derived from it would write the replacement character over what
        // was really there — D50's argument about `from_utf8_lossy`, one layer
        // earlier than D50 made it. The row is still listed, still counted and
        // still renameable by hand, because `path` is byte-exact; what it is
        // not is *transformable*.
        //
        // Left alone rather than blocked: **P63** — one unreadable entry costs
        // its own rows and no others. A single odd file in a folder of ten
        // thousand must not refuse the other 9 999, and it is said out loud in
        // `notes` instead.
        if entry.name_is_lossy {
            lossy_names += 1;
            items.push(PlanItem {
                index,
                source: entry.path.clone(),
                target: entry.path.clone(),
                new_name: entry.file_name.clone(),
                state: RowState::Unchanged,
                actions: Vec::new(),
            });
            continue;
        }
        let (new_name, mut state, actions) = match result {
            Err(e) => (
                entry.file_name.clone(),
                RowState::Error(e.to_string()),
                Vec::new(),
            ),
            Ok(ev) => {
                let state = match validate_target(&ev.name, rules) {
                    Err(problem) => RowState::Conflict(ConflictKind::InvalidName(problem)),
                    Ok(()) if ev.name == entry.file_name => RowState::Unchanged,
                    Ok(()) => RowState::Changed,
                };
                (ev.name, state, ev.actions)
            }
        };

        // The capability pre-flight (P4/P5). Costs nothing per file:
        // `supports` is a lookup in a constant table, and this only runs for a
        // row that has actions at all.
        if !state.is_conflict()
            && let Some(capability) = actions
                .iter()
                .flat_map(|a| a.effect.required_capabilities())
                .find(|c| !platform.supports(*c))
        {
            state = RowState::Conflict(ConflictKind::Unsupported {
                capability,
                platform: platform.name(),
            });
        }

        // A name that does not validate has no target: `..` would otherwise
        // produce a path pointing at the parent directory, which nothing may
        // execute but everything downstream would have to keep checking.
        // Found by the M3 property test, on the name " ..".
        let target = match &state {
            RowState::Error(_) | RowState::Conflict(_) => entry.path.clone(),
            _ => target_path(entry.parent(), &new_name),
        };
        items.push(PlanItem {
            index,
            source: entry.path.clone(),
            target,
            new_name,
            state,
            actions,
        });
    }

    let keys = Keys::build(&items, platform);
    detect_blocked_subfolders(&mut items);
    detect_duplicate_targets(&keys, &mut items);
    detect_existing_targets(&keys, platform, &mut items);

    // Before the targets are resolved, not after: a `<\>` subfolder has to be
    // created under the name its parent still has when the rename op runs. And
    // before `order_renames`, which is the only pass left that changes a state
    // — it can add an `UnresolvedCycle`, never clear one, so computing the
    // folders here can only include one belonging to a row that is about to
    // become a conflict, in a plan nothing will run.
    let wanted = wanted_directories(&items);
    // Before `order_renames` for the other half of that reason too: a row this
    // turns into a conflict must not still have a rename op built for it. The
    // plan would be unexecutable either way, but a plan whose ops describe a
    // row it has already refused is a plan that reads wrong.
    detect_targets_needed_as_folders(&keys, platform, &wanted, &mut items);
    let renames = order_renames(&keys, platform, &mut items);

    // Folders first: a rename into a subfolder needs it to exist (D31). The
    // overwhelmingly common case is that there are none, and then the renames
    // are the plan — no second vector, no copy.
    let mut creations = directory_creations(wanted);
    // Nothing to resolve unless a folder is in the listing at all, and the
    // common run — Files, no Folders — never has one. Worth the check: the
    // resolution walks and hashes a path per row, and this is a bool per row.
    if entries.iter().any(|e| e.is_dir) {
        resolve_targets_under_renamed_ancestors(&mut items);
    }
    let mut ops = if creations.is_empty() {
        renames
    } else {
        creations.extend(renames);
        creations
    };
    // Then every action, after every rename.
    //
    // Not interleaved per file: a file caught in a cycle only reaches its final
    // name at the very end (`order_renames` appends the finishes after its main
    // loop), so "right after that file's rename" would act on the temp name.
    // "After everything" is the only rule correct for all three `RenameKind`s,
    // and it hands undo the property its reverse walk already needs — every
    // action is reverted before any rename is taken back, so the path a record
    // names still resolves.
    //
    // Built here rather than in the loop above because `order_renames` can
    // retroactively turn a row into an `UnresolvedCycle` conflict.
    ops.extend(action_ops(&items));

    // Last of all, whatever the run itself asked for. `Create Mp3 Playlist`
    // wants the *renamed* names in its playlist, so this has to come after
    // every rename — and it comes after the actions too, because a file
    // written here is not one any of them should touch.
    let outcome = pipeline.end_run();
    let mut notes = outcome.notes;
    for write in outcome.writes {
        match write_op(write) {
            Ok(op) => ops.push(op),
            Err(note) => notes.push(note),
        }
    }

    // P63: always said out loud, never a silent skip.
    if lossy_names > 0 {
        notes.push(format!(
            "{} left alone: the name on disk is not valid Unicode, so no new name can be \
             built from it. Rename it directly to give it one.",
            crate::plural(lossy_names, "item")
        ));
    }

    Plan { items, ops, notes }
}

/// Turn a requested write into a planned one, or say why it cannot be.
///
/// The `stat` here is what decides undoability, and it is deliberately taken
/// at *plan* time: P2's confirmation has to know whether anything is about to
/// be destroyed before the executor opens a journal. It is racy in the sense
/// that anything filesystem-shaped is — a file appearing between the plan and
/// the run would be overwritten under a promise that it would not — which is
/// the same window every "does the target exist" check in this file lives with.
fn write_op(write: crate::ops::FileWrite) -> Result<PlannedOp, String> {
    // Absolute only. A relative path would resolve against the process's
    // working directory, which is not a folder the user chose and, for the GUI,
    // is wherever the app happened to be launched from.
    if !write.path.is_absolute() {
        return Err(format!(
            "script: '{}' is not a full path, so nothing was written",
            write.path.display()
        ));
    }
    let undoability = if write.path.exists() {
        Undoability::None
    } else {
        Undoability::Full
    };
    Ok(PlannedOp::WriteFile {
        path: write.path,
        contents: write.contents,
        undoability,
    })
}

fn action_ops(items: &[PlanItem]) -> Vec<PlannedOp> {
    items
        .iter()
        .filter(|item| item.affected())
        .flat_map(|item| {
            item.actions.iter().map(|action| PlannedOp::Act {
                path: item.final_path().to_path_buf(),
                op: action.op,
                effect: action.effect.clone(),
                undoability: action.undoability,
                describe: action.describe.clone(),
            })
        })
        .collect()
}

/// Checks a produced name, which may carry the file into a subfolder (D31).
///
/// Every component goes through the same [`ren_platform::NamingRules`] a plain
/// name does, which is what rejects a name that would escape: an absolute path
/// or a trailing separator leaves an empty component, `.` and `..` are refused
/// everywhere, and a Windows drive letter trips over the illegal `:`.
fn validate_target(name: &str, rules: &ren_platform::NamingRules) -> Result<(), NameProblem> {
    for component in name.split(SUBFOLDER_SEPARATOR) {
        rules.validate_component(component)?;
    }
    Ok(())
}

/// Joins a produced name — subfolders and all — onto the folder it came from.
fn target_path(parent: &Path, name: &str) -> PathBuf {
    let mut path = parent.to_path_buf();
    for component in name.split(SUBFOLDER_SEPARATOR) {
        path.push(component);
    }
    path
}

/// A subfolder a move needs, blocked by an existing file of that name (D31).
///
/// Found by the M4 property tests: a file called `(` renamed to `(/(` asks the
/// executor to create a directory where a file already is, which fails halfway
/// through the batch. Checking here turns it into a blocked run with a reason.
fn detect_blocked_subfolders(items: &mut [PlanItem]) {
    for item in items.iter_mut().filter(|i| i.state.is_changed()) {
        let source_dir = item.source.parent().unwrap_or(Path::new(""));
        let Some(target_dir) = item.target.parent() else {
            continue;
        };
        let mut at = target_dir;
        let mut blocked = false;
        while at != source_dir {
            if at.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
                item.state = RowState::Conflict(ConflictKind::BlockedByFile {
                    path: at.to_path_buf(),
                });
                blocked = true;
                break;
            }
            match at.parent() {
                Some(parent) => at = parent,
                None => break,
            }
        }

        // After the walk, not before it. A **file** whose `<\>` target lands
        // under its own name — `(` to `(/(` — is under its own path too, and
        // "a folder cannot be moved inside itself" is a wrong answer about a
        // file. The walk above names it correctly first; what is left here is
        // a real folder moving into its own subtree, which the walk cannot
        // see because it climbs straight past `source_dir`.
        if !blocked && target_dir.starts_with(&item.source) {
            item.state = RowState::Conflict(ConflictKind::IntoItself);
        }
    }
}

/// The folders the plan has to create, parents first and each one only once.
///
/// Only folders that do not already exist are listed, so undo knows that every
/// one of them is its own to remove.
fn directory_creations(wanted: Vec<PathBuf>) -> Vec<PlannedOp> {
    wanted
        .into_iter()
        .map(|path| PlannedOp::CreateDir { path })
        .collect()
}

/// The walk that finds them, hoisted so it happens once.
///
/// Two passes need it — [`directory_creations`] turns it into ops, and
/// [`detect_targets_needed_as_folders`] has to know whether a row's new name is
/// one of these — and the walk is the expensive half: an `exists` per ancestor
/// per row that moves. A run with no `<\>` in it never gets past the first
/// `continue`, which is why the common case costs a `parent()` compare.
fn wanted_directories(items: &[PlanItem]) -> Vec<PathBuf> {
    let mut wanted: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for item in items.iter().filter(|i| i.state.is_changed()) {
        let source_dir = item.source.parent().unwrap_or(Path::new(""));
        let Some(target_dir) = item.target.parent() else {
            continue;
        };
        if target_dir == source_dir {
            continue;
        }
        // Walk up to the folder the file started in, then create downwards.
        let mut missing: Vec<PathBuf> = Vec::new();
        let mut at = target_dir;
        while at != source_dir {
            if !at.exists() {
                missing.push(at.to_path_buf());
            }
            match at.parent() {
                Some(parent) => at = parent,
                None => break,
            }
        }
        for dir in missing.into_iter().rev() {
            if seen.insert(dir.clone()) {
                wanted.push(dir);
            }
        }
    }

    wanted
}

/// A row whose new name is a folder this same run has to create (P4).
///
/// The check the planner did not have. `detect_duplicate_targets` compares
/// rows against each other, `detect_existing_targets` compares them against
/// what is on disk, and neither sees the third occupant of a name: a directory
/// this run is about to make. A plan could therefore contain both
/// `CreateDir .../sub` and `Rename .../1 -> .../sub`, report
/// `is_executable() == true`, and fail on whichever of the two ran second —
/// after opening a journal.
///
/// Folded with the same rules the target keys use, so `SUB` and `sub` collide
/// on Windows and do not on ext4.
fn detect_targets_needed_as_folders(
    keys: &Keys,
    platform: &dyn Platform,
    wanted: &[PathBuf],
    items: &mut [PlanItem],
) {
    // No `<\>` anywhere in the run means nothing to create and nothing to
    // check, which is the overwhelmingly common case.
    if wanted.is_empty() {
        return;
    }

    let folded: HashMap<String, &Path> = wanted
        .iter()
        .map(|dir| {
            let rules = platform.naming_rules(dir);
            (rules.fold(&dir.to_string_lossy()), dir.as_path())
        })
        .collect();

    for item in items.iter_mut().filter(|i| i.state.is_changed()) {
        // A row's own wanted folders are all strict ancestors of its target, so
        // it can never be its own conflict here.
        if let Some(path) = folded.get(keys.target[item.index].as_str()) {
            item.state = RowState::Conflict(ConflictKind::NeededAsFolder {
                path: path.to_path_buf(),
            });
        }
    }
}

/// The folded source and target path of every item, computed once.
///
/// Three passes need these, and folding a path allocates. At 10 000 files that
/// is the difference between a plan the preview can afford and one it cannot.
struct Keys {
    source: Vec<String>,
    target: Vec<String>,
}

impl Keys {
    fn build(items: &[PlanItem], platform: &dyn Platform) -> Self {
        // Pure and per-item, so it parallelises for free.
        let (source, target) = items
            .par_iter()
            .map(|item| {
                let rules = platform.naming_rules(&item.source);
                (
                    rules.fold(&item.source.to_string_lossy()),
                    rules.fold(&item.target.to_string_lossy()),
                )
            })
            .unzip();
        Self { source, target }
    }

    /// Which name this item ends up occupying.
    fn final_key(&self, item: &PlanItem) -> &str {
        match item.state {
            RowState::Changed => &self.target[item.index],
            _ => &self.source[item.index],
        }
    }
}

/// Two listed items landing on the same name. Unchanged items count as
/// occupants — renaming `a` onto an untouched `b` is just as much a collision.
fn detect_duplicate_targets(keys: &Keys, items: &mut [PlanItem]) {
    // Count first, then collect only the keys that actually clash. Building a
    // Vec per bucket up front would allocate once per file, and in a healthy
    // plan every single one of those buckets holds exactly one item.
    let mut counts: HashMap<&str, u32> = HashMap::with_capacity(items.len());
    for item in items.iter() {
        if matches!(item.state, RowState::Error(_) | RowState::Conflict(_)) {
            continue;
        }
        *counts.entry(keys.final_key(item)).or_insert(0) += 1;
    }
    if counts.len() == items.len() {
        return; // Every target is distinct — the overwhelmingly common case.
    }

    let mut buckets: HashMap<&str, Vec<usize>> = HashMap::new();
    for item in items.iter() {
        if matches!(item.state, RowState::Error(_) | RowState::Conflict(_)) {
            continue;
        }
        let key = keys.final_key(item);
        if counts[key] > 1 {
            buckets.entry(key).or_default().push(item.index);
        }
    }

    let groups: Vec<Vec<usize>> = buckets.into_values().collect();
    for group in groups {
        for &index in &group {
            if !items[index].state.is_changed() {
                continue; // An untouched file is not itself in error.
            }
            let others = group.iter().copied().filter(|&o| o != index).collect();
            items[index].state = RowState::Conflict(ConflictKind::DuplicateTarget { others });
        }
    }
}

/// A file on disk that this batch will not move out of the way.
///
/// Reads each destination directory **once** rather than stat-ing every target.
/// A per-target `symlink_metadata` is one syscall per file, which at 10 000
/// files dominated the whole plan.
fn detect_existing_targets(keys: &Keys, platform: &dyn Platform, items: &mut [PlanItem]) {
    let freed: HashSet<&str> = items
        .iter()
        .filter(|i| i.state.is_changed())
        .map(|i| keys.source[i.index].as_str())
        .collect();

    let mut occupied: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    for (index, item) in items.iter_mut().enumerate() {
        if !item.state.is_changed() {
            continue;
        }
        let target_key = &keys.target[index];
        // A case-only rename "collides" with itself — that is not a conflict.
        if *target_key == keys.source[index] {
            continue;
        }
        if freed.contains(target_key.as_str()) {
            continue; // Another item in this batch vacates the name first.
        }

        let dir = item.target.parent().unwrap_or(Path::new("")).to_path_buf();
        let rules = platform.naming_rules(&item.source);
        let names = occupied
            .entry(dir.clone())
            .or_insert_with(|| read_dir_folded(&dir, rules));
        if names.contains(target_key) {
            item.state = RowState::Conflict(ConflictKind::TargetExists);
        }
    }
}

/// Folded full paths of everything currently in `dir`.
///
/// Folded with the *same* rules the target keys use — lower-casing
/// unconditionally would make every comparison miss on a case-sensitive
/// filesystem, and silently stop reporting `TargetExists` there.
///
/// An unreadable directory yields an empty set: the executor still refuses to
/// overwrite (`Platform::rename` never replaces), so the worst case is a
/// conflict reported at execute time instead of at preview time.
fn read_dir_folded(dir: &Path, rules: &ren_platform::NamingRules) -> HashSet<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return HashSet::new();
    };
    read.filter_map(Result::ok)
        .map(|e| rules.fold(&e.path().to_string_lossy()))
        .collect()
}

/// Orders renames so a name is always vacated before it is claimed, breaking
/// cycles with temp names (P6).
///
/// Kahn's algorithm over the edge "Y must run before X" whenever X's target is
/// Y's source. When the queue empties with work left, everything remaining is
/// in — or downstream of — a cycle. Staging one member of that cycle under a
/// temp name frees its source, which unblocks its dependents and lets Kahn
/// continue; the staged file moves to its real target at the very end, by which
/// point the file that occupied it has moved away.
///
/// This is what lets `a`→`b`, `b`→`a` and `File 1`→`2`, `2`→`3`, `3`→`1`
/// execute in a single run, without a second pass and without leaving undo in
/// a state it cannot reverse.
fn order_renames(keys: &Keys, platform: &dyn Platform, items: &mut [PlanItem]) -> Vec<PlannedOp> {
    let movers: Vec<usize> = items
        .iter()
        .filter(|i| i.state.is_changed())
        .map(|i| i.index)
        .collect();
    if movers.is_empty() {
        return Vec::new();
    }

    let source_of: HashMap<&str, usize> = movers
        .iter()
        .map(|&i| (keys.source[i].as_str(), i))
        .collect();

    // successors[y] = renames that may only run once y has moved away.
    let mut successors: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut in_degree: HashMap<usize, usize> = movers.iter().map(|&i| (i, 0)).collect();
    for &x in &movers {
        if let Some(&y) = source_of.get(keys.target[x].as_str())
            && y != x
        {
            successors.entry(y).or_default().push(x);
            *in_degree.get_mut(&x).unwrap() += 1;
        }
    }

    // **A folder waits for everything inside it.** This is a real edge, not a
    // tiebreak, and the comment below about depth being "a safe tiebreak"
    // missed it: it reasons about rows that *block* each other, and a folder
    // with no blocker is ready on the first pass — so a folder whose children
    // are tangled in a cycle was renamed **first**, and every path underneath
    // then named a folder that no longer existed. Reproduced by inverting the
    // casing of a folder holding `README` and `readme`.
    //
    // Walking up each row's own path rather than comparing every pair: folders
    // are few and paths are shallow, so this is O(rows × depth) rather than the
    // O(rows²) that would show up in the 10 000-file budget.
    let mover_at: HashMap<&Path, usize> = movers
        .iter()
        .map(|&i| (items[i].source.as_path(), i))
        .collect();
    for &x in &movers {
        let mut above = items[x].source.parent();
        while let Some(dir) = above {
            if let Some(&folder) = mover_at.get(dir) {
                successors.entry(x).or_default().push(folder);
                *in_degree.get_mut(&folder).unwrap() += 1;
            }
            above = dir.parent();
        }
    }

    // Every name this batch touches, so a temp name can avoid all of them.
    let mut reserved: HashSet<String> = HashSet::new();
    for &i in &movers {
        reserved.insert(keys.source[i].clone());
        reserved.insert(keys.target[i].clone());
    }

    // Deepest first, then by index. Folders + Subfolders puts a folder and the
    // files inside it in one run, and the folder sorts *before* its own
    // children — so plain index order renamed the folder first and every rename
    // inside it then failed on a path that no longer existed.
    //
    // Depth is a safe tiebreak rather than a second constraint fighting the
    // first: an edge exists only when one row's target is another row's source,
    // a target never leaves the folder its source is in (`<\>` goes down, never
    // up or out), so everything that blocks a descendant of a folder is itself a
    // descendant of that folder. No edge can ever demand a parent before its own
    // child, which is the only thing depth would have to overrule.
    // Computed once. `sort_unstable_by_key` calls its key function per
    // comparison, and counting a path's components per comparison cost the
    // 1000-file plan a fifth of its time.
    let depths: Vec<usize> = items.iter().map(|i| depth(&i.source)).collect();

    let mut ready: Vec<usize> = movers
        .iter()
        .copied()
        .filter(|i| in_degree[i] == 0)
        .collect();
    ready.sort_unstable_by_key(|&i| (depths[i], std::cmp::Reverse(i)));
    // Pop deepest first, and the lowest index among equals, so plans are stable.

    let mut ops: Vec<PlannedOp> = Vec::with_capacity(movers.len());
    let mut finishes: Vec<PlannedOp> = Vec::new();
    let mut done: HashSet<usize> = HashSet::new();
    let mut stuck: Vec<usize> = Vec::new();

    loop {
        while let Some(y) = ready.pop() {
            ops.push(PlannedOp::Rename {
                from: items[y].source.clone(),
                to: items[y].target.clone(),
                kind: RenameKind::Direct,
            });
            done.insert(y);
            release(&successors, &mut in_degree, &mut ready, &done, y);
        }

        // Deepest first here too, for the same reason: staging a folder under a
        // temp name moves everything inside it, so a pending rename within that
        // folder has to have run already. Lowest index breaks a depth tie, which
        // keeps the choice of victim — and so the whole plan — deterministic.
        let Some(&victim) = movers
            .iter()
            .filter(|i| !done.contains(i) && !stuck.contains(i))
            .max_by_key(|&&i| (depths[i], std::cmp::Reverse(i)))
        else {
            break;
        };

        // Break the cycle: move the victim out of the way first.
        let Some(temp) = temp_path(&items[victim].source, platform, &reserved) else {
            stuck.push(victim);
            continue;
        };
        reserved.insert(
            platform
                .naming_rules(&items[victim].source)
                .fold(&temp.to_string_lossy()),
        );

        ops.push(PlannedOp::Rename {
            from: items[victim].source.clone(),
            to: temp.clone(),
            kind: RenameKind::CycleStage,
        });
        finishes.push(PlannedOp::Rename {
            from: temp,
            to: items[victim].target.clone(),
            kind: RenameKind::CycleFinish,
        });
        done.insert(victim);
        release(&successors, &mut in_degree, &mut ready, &done, victim);
    }

    if !stuck.is_empty() {
        for i in stuck {
            items[i].state = RowState::Conflict(ConflictKind::UnresolvedCycle);
        }
        return Vec::new();
    }

    // **Each finish goes in before the first folder above it moves**, not at
    // the end of the batch.
    //
    // A cycle's second half addresses a temp name that lives *inside* a folder,
    // and `ops` may already carry that folder's own rename — Folders +
    // Subfolders puts both in one run. Appending every finish at the end put
    // `a/tmp → a/readme` after `a → A`, so it named a path that had stopped
    // existing and the run stopped there, part-way. Found by
    // `a_run_over_a_folder_and_its_contents_previews_truly_and_undoes_exactly`
    // once a swap and a renamed parent turned up in the same case: invert the
    // casing of a folder holding `README` and `readme`.
    //
    // Inserting before the *first* ancestor rename is enough, and lands after
    // the direct rename that freed the target: `order_renames` is deepest-first,
    // so every rename inside a folder is already ahead of the folder's own.
    for finish in finishes {
        let PlannedOp::Rename { from, .. } = &finish else {
            continue;
        };
        let at = ops
            .iter()
            .position(|op| match op {
                PlannedOp::Rename { from: ancestor, .. } => {
                    from != ancestor && from.starts_with(ancestor)
                }
                _ => false,
            })
            .unwrap_or(ops.len());
        ops.insert(at, finish);
    }
    ops
}

/// How far down a path sits, for ordering a folder against its own contents.
fn depth(path: &Path) -> usize {
    path.components().count()
}

/// Where each row actually comes to rest, once the folders above it have moved.
///
/// A run over Folders + Subfolders renames a folder and the files inside it.
/// The files go first (`order_renames` is deepest-first), each inside the parent
/// it was listed under, and that parent's own rename then carries them along:
/// the row's own op lands `a/x.txt` at `a/y.txt`, and the run finishes with it
/// at `b/y.txt`.
///
/// `target` is the second one. It is what the preview promises, what "no file is
/// lost" is checked against, and what the actions have to address — they run
/// after every rename (see `plan`), so a Set Date on a file inside a renamed
/// folder would otherwise stat a path that had already moved. The rename ops
/// keep the first: they are the journey, this is the destination.
///
/// Shallowest first, so a chain composes — `a`→`A` is resolved before `a/b` is
/// asked where it lands, and `a/b/c.txt` then sees `A/B` already.
fn resolve_targets_under_renamed_ancestors(items: &mut [PlanItem]) {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_cached_key(|&i| depth(&items[i].source));

    // Every listed path, mapped to where the run leaves it. Unmoved rows are in
    // here too: a folder that is not itself renamed still has to pass its own
    // renamed parent down to its children.
    let mut lands: HashMap<PathBuf, PathBuf> = HashMap::with_capacity(items.len());

    for i in order {
        let source = items[i].source.clone();
        let mut resolved = if items[i].state.is_changed() {
            items[i].target.clone()
        } else {
            source.clone()
        };

        // The nearest listed ancestor already holds the whole chain above it,
        // so one hop is the whole answer.
        for ancestor in source.ancestors().skip(1) {
            if let Some(landed) = lands.get(ancestor) {
                if let Ok(rest) = resolved.strip_prefix(ancestor) {
                    resolved = landed.join(rest);
                }
                break;
            }
        }

        if items[i].state.is_changed() {
            items[i].target = resolved.clone();
        }
        lands.insert(source, resolved);
    }
}

/// `y` has moved away, so anything that was waiting on its name may now run.
///
/// Skipping successors that are already done matters once cycles are in play:
/// in `a`↔`b`, staging `a` releases `b`, and then `b` moving would try to
/// release `a` a second time.
fn release(
    successors: &HashMap<usize, Vec<usize>>,
    in_degree: &mut HashMap<usize, usize>,
    ready: &mut Vec<usize>,
    done: &HashSet<usize>,
    y: usize,
) {
    for &x in successors.get(&y).map(Vec::as_slice).unwrap_or_default() {
        if done.contains(&x) {
            continue;
        }
        let degree = in_degree.get_mut(&x).expect("every mover has a degree");
        *degree -= 1;
        if *degree == 0 {
            ready.push(x);
        }
    }
}

/// A free name in `source`'s directory to park a file under.
///
/// Deterministic, so `plan()` stays a pure function of its inputs and two runs
/// over the same tree produce byte-identical plans. Checked against both the
/// names this batch touches and the directory itself.
fn temp_path(
    source: &Path,
    platform: &dyn Platform,
    reserved: &HashSet<String>,
) -> Option<PathBuf> {
    let dir = source.parent().unwrap_or(Path::new(""));
    let rules = platform.naming_rules(source);
    for n in 0..1_000u32 {
        let candidate = dir.join(format!("__renameit-tmp-{n}"));
        if reserved.contains(&rules.fold(&candidate.to_string_lossy())) {
            continue;
        }
        if candidate.symlink_metadata().is_ok() {
            continue;
        }
        return Some(candidate);
    }
    None
}
