//! Turning a pipeline plus a listing into an ordered, conflict-checked plan.
//!
//! The plan is a pure function of (entries, pipeline, platform rules) — the
//! preview the user sees and the thing the executor runs are the same object,
//! which is what makes "previewed name == on-disk name" testable.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

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
    /// it started putting folders in the tree. Compared through the volume's
    /// naming rules, so `Holiday` → `holiday/holiday` is caught where the two
    /// spellings are one folder.
    IntoItself,
    /// A `<\>` move into a folder that this same run renames.
    ///
    /// `img.jpg` → `2024/img.jpg` while `2024` → `2024 (sorted)` has no order
    /// that keeps the preview's promise: the folder renamed first leaves the
    /// file's destination missing and the run fails part-way; the file moved
    /// first rides along into `2024 (sorted)`, which the preview never
    /// showed. The ordering edges and the landing resolution both follow a
    /// row's *source* folders, and this row's trouble is in its target's.
    /// Refused in P94's conservative spirit rather than supported — making it
    /// work means resolving a target through a folder's landing, which is a
    /// feature rather than a fix.
    IntoRenamedFolder { path: PathBuf },
    /// Part of a rename cycle that could not be broken.
    ///
    /// Cycles are normally resolved with a temp name (P6), and each folder
    /// hands them out from its own counter, so a thousand swaps in one folder
    /// cost a thousand names. This is only reported when more than a thousand
    /// files named like the temp names already sit in the folder, which takes
    /// a deliberately hostile set of existing files.
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
    /// A tag write or tag removal on a row that is a symbolic link (D241).
    ///
    /// The executor never retags through a link (D217: the swap would put a
    /// plain file where the link was), and a refusal that waited for the run
    /// stopped it part-way — after the rows before the link had been
    /// retagged, which no undo can take back. Reported here instead, so the
    /// preview shows it and the run does not start (P4).
    TagsThroughLink,
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
            Self::IntoRenamedFolder { path } => write!(
                f,
                "{} is a folder this run renames, so nothing can move into it in the same run",
                path.display()
            ),
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
            Self::TagsThroughLink => write!(
                f,
                "this is a symbolic link: its tags are in the file it points to, \
                 which is not changed through a link"
            ),
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

    /// Where the file will be when its actions run, and where the preview
    /// promises it ends up.
    ///
    /// The target for every row the run can execute — a renamed row's, and an
    /// untouched row's too, because a row that keeps its name still moves when
    /// a folder above it is renamed (see
    /// `resolve_targets_under_renamed_ancestors`). A row in conflict or in
    /// error goes nowhere, so it answers its source.
    pub fn final_path(&self) -> &Path {
        match self.state {
            RowState::Changed | RowState::Unchanged => &self.target,
            RowState::Conflict(_) | RowState::Error(_) => &self.source,
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
    /// Reasons the run as a whole cannot go ahead, which belong to no row.
    ///
    /// Today these are script writes the planner refused (P60 as amended): a
    /// path outside the folders the run lists, or one that collides with
    /// something the run itself moves or creates. A blocker rather than a
    /// note because a note is read *after* the run, and the user has to see
    /// this before anything is touched. [`Self::is_executable`] consults it,
    /// and `apply` quotes it in [`crate::ExecError::Blocked`]. The row counts
    /// ([`Counts`]) deliberately do not include it: they count rows.
    pub blockers: Vec<String>,
}

/// Every row count a summary wants, from one pass over the items.
///
/// The per-question methods on [`Plan`] each walk every item, and the status
/// bar asked six of those questions two or three times per frame — twelve
/// passes over ten thousand rows, sixty times a second, to draw one line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub changed: usize,
    pub acted: usize,
    pub affected: usize,
    pub unchanged: usize,
    pub conflicts: usize,
    pub errors: usize,
}

impl Plan {
    /// Every count at once. Agrees with the per-question methods by
    /// construction — each is the same predicate — and a test holds them
    /// equal.
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for item in &self.items {
            let changed = item.state.is_changed();
            let acts = item.acts();
            counts.changed += usize::from(changed);
            counts.acted += usize::from(acts);
            counts.affected += usize::from(changed || acts);
            counts.unchanged += usize::from(item.state == RowState::Unchanged && !acts);
            counts.conflicts += usize::from(item.state.is_conflict());
            counts.errors += usize::from(matches!(item.state, RowState::Error(_)));
        }
        counts
    }

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

    /// P4: conflicts hard-block execution rather than being skipped silently,
    /// and so does anything in [`Self::blockers`].
    pub fn is_executable(&self) -> bool {
        self.conflicts() == 0 && self.errors() == 0 && self.blockers.is_empty()
    }
}

/// Builds a plan. Pure apart from the `exists` probe against the destination.
pub fn plan(entries: &[FileEntry], pipeline: &Pipeline, platform: &dyn Platform) -> Plan {
    let evaluated = evaluate_all(entries, pipeline);

    // One row per entry, built in parallel: each is a pure function of its
    // own entry and its own evaluation — a validation, two path allocations
    // and a capability lookup — and done serially it was a fifth of the whole
    // plan at ten thousand rows.
    let mut items: Vec<PlanItem> = entries
        .par_iter()
        .zip(evaluated.into_par_iter())
        .enumerate()
        .map(|(index, (entry, result))| plan_item(index, entry, result, platform))
        .collect();
    // P63: always said out loud, never a silent skip. Counted after the pass
    // rather than inside it, so the pass has nothing to share.
    let lossy_names = entries.iter().filter(|e| e.name_is_lossy).count();

    let keys = Keys::build(&items, platform);
    // The two `<\>` passes ask the disk about the same few folders for every
    // row that moves; one answer per folder does for both.
    let mut disk = DiskProbe::default();
    detect_blocked_subfolders(&mut items, platform, &mut disk);
    detect_duplicate_targets(&keys, &mut items);
    detect_existing_targets(&keys, platform, &mut items);
    // Whether a folder is in the listing at all — and the common run, Files
    // without Folders, never has one. The passes below that are only about
    // folders cost a hash per row when they run, so they are told rather than
    // left to find out.
    let any_dir = entries.iter().any(|e| e.is_dir);
    if any_dir {
        detect_moves_into_renamed_folders(&keys, platform, &mut items);
    }

    // Before the targets are resolved, not after: a `<\>` subfolder has to be
    // created under the name its parent still has when the rename op runs. And
    // before `order_renames`, which is the only pass left that changes a state
    // — it can add an `UnresolvedCycle`, never clear one, so computing the
    // folders here can only include one belonging to a row that is about to
    // become a conflict, in a plan nothing will run.
    let wanted = wanted_directories(&items, platform, &mut disk);
    let wanted_keys: HashMap<&str, &Path> = wanted
        .iter()
        .map(|(dir, key)| (key.as_str(), dir.as_path()))
        .collect();
    // Before `order_renames` for the other half of that reason too: a row this
    // turns into a conflict must not still have a rename op built for it. The
    // plan would be unexecutable either way, but a plan whose ops describe a
    // row it has already refused is a plan that reads wrong.
    detect_targets_needed_as_folders(&keys, &wanted_keys, &mut items);
    let renames = order_renames(&keys, platform, any_dir, &wanted_keys, &mut items);

    // Folders first: a rename into a subfolder needs it to exist (D31). The
    // overwhelmingly common case is that there are none, and then the renames
    // are the plan — no second vector, no copy.
    let mut creations = directory_creations(&wanted);
    if any_dir {
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
    let mut blockers = Vec::new();
    if !outcome.writes.is_empty() {
        let mut check = WriteCheck::new(&items, &keys, &wanted_keys, platform, any_dir);
        for write in outcome.writes {
            match check.plan(write) {
                Ok(op) => ops.push(op),
                Err(Refused::Note(note)) => notes.push(note),
                Err(Refused::Blocked(reason)) => blockers.push(reason),
            }
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

    Plan {
        items,
        ops,
        notes,
        blockers,
    }
}

/// One row of the plan: the produced name validated, the target it implies,
/// and the capability pre-flight for whatever it acts on.
fn plan_item(
    index: usize,
    entry: &FileEntry,
    result: Result<crate::pipeline::Evaluation, crate::ops::OpError>,
    platform: &dyn Platform,
) -> PlanItem {
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
        return PlanItem {
            index,
            source: entry.path.clone(),
            target: entry.path.clone(),
            new_name: entry.file_name.clone(),
            state: RowState::Unchanged,
            actions: Vec::new(),
        };
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
    if !state.is_conflict()
        && entry.is_symlink
        && let Some(conflict) = link_conflict(&actions, platform)
    {
        state = RowState::Conflict(conflict);
    }

    // A name that does not validate has no target: `..` would otherwise
    // produce a path pointing at the parent directory, which nothing may
    // execute but everything downstream would have to keep checking.
    // Found by the M3 property test, on the name " ..".
    let target = match &state {
        RowState::Error(_) | RowState::Conflict(_) => entry.path.clone(),
        _ => target_path(entry.parent(), &new_name),
    };
    PlanItem {
        index,
        source: entry.path.clone(),
        target,
        new_name,
        state,
        actions,
    }
}

/// What a row that is a symbolic link cannot have done to it (D241).
///
/// Both are refusals the executor makes as well — `meta::write` refuses a
/// link, and the Unix attribute setter refuses read-only on one — but a
/// refusal that waits for the run stops it part-way (P4). Asked only for a
/// link row, so an ordinary listing pays nothing for it.
///
/// Read-only is asked for only when *setting* it: a link's own mode always
/// carries the write bits, so clearing read-only on one is a change of
/// nothing, and the setter lets it through.
fn link_conflict(actions: &[PlannedAction], platform: &dyn Platform) -> Option<ConflictKind> {
    use ren_platform::Capability;
    if actions.iter().any(|a| {
        matches!(
            a.effect,
            Effect::WriteTags { .. } | Effect::RemoveTags { .. }
        )
    }) {
        return Some(ConflictKind::TagsThroughLink);
    }
    let sets_read_only = actions
        .iter()
        .any(|a| matches!(&a.effect, Effect::Attributes(change) if change.read_only == Some(true)));
    (sets_read_only && !platform.supports(Capability::LinkReadOnly)).then(|| {
        ConflictKind::Unsupported {
            capability: Capability::LinkReadOnly,
            platform: platform.name(),
        }
    })
}

/// Why a requested write did not become a planned one.
enum Refused {
    /// Nothing was written and nothing else is affected — a script mistake,
    /// said in the log the way P59 says a failing `done()`.
    Note(String),
    /// The whole run is refused (P60 as amended), with the reason.
    Blocked(String),
}

/// Turns a script's write requests into planned writes, or says why not.
///
/// **A script may write only into a folder the run lists** — the folder of a
/// listed entry, which includes the browsed folder whenever it holds anything
/// the run lists (D108's `browser_path` is that folder by construction). A
/// script's `done()` may be code somebody else wrote, and the sandbox removes
/// `io.create` for exactly that reason; a write request that could name any
/// absolute path gave it straight back. Compared through the volume's naming
/// rules, so a differently cased spelling of a listed folder is that folder.
///
/// **Like with like.** The listed folders are keyed as the listing named
/// them, and a front end may name one through `..` (`std::path::absolute`
/// keeps it on Unix, so `/w/other/../music` is what the rows and D108's
/// `browser_path` carry). A write whose own parent is one of those folders,
/// spelled the same way, and whose last component is a plain name, is in it:
/// the OS resolves that spelling exactly as it resolved the listing, so the
/// folder is kept as written. Anything else is compared after lexical
/// normalisation, so `..` cannot climb out. Normalising the folders as well
/// would not do: `/w/link/../music` is not `/w/music` when `link` is a
/// symbolic link.
///
/// Refused as well, because each would make the run fail part-way or undo
/// lie: a path that is the new name of a row (the write would truncate the
/// file just renamed there), a file the run renames away (undo could not put
/// it back over the written one), a folder the run creates, an existing
/// folder or link, and a path another write in the same run already names
/// (the second would find the first's file and fail as though another
/// program had put it there).
///
/// Undoability is decided here, against the disk as it is *before* the run:
/// a file already at the path is an overwrite (`Undoability::None`, P2's
/// consent); anything else is a create, and the executor holds it to that
/// with `create_new`, so a file that appears between the preview and the run
/// fails the write rather than being truncated under a promise that nothing
/// was there.
struct WriteCheck<'a> {
    platform: &'a dyn Platform,
    /// The folders listed entries sit in, as comparison keys.
    folders: HashSet<String>,
    /// Rows that move, by the key of the path they leave.
    moving: HashMap<&'a str, &'a PlanItem>,
    /// Rows that move, by the key of the path they claim (before any folder
    /// above them moves — the coordinates a script's paths are written in).
    claimed: HashMap<&'a str, &'a PlanItem>,
    wanted: &'a HashMap<&'a str, &'a Path>,
    /// Where the run leaves each listed path that it moves, by the key of the
    /// path — so a write aimed at a folder the run renames lands in it.
    landings: HashMap<String, &'a Path>,
    /// Every write planned so far, by the key of where it lands, lexically
    /// normalised so two spellings of one file are one key.
    planned: HashSet<String>,
}

impl<'a> WriteCheck<'a> {
    fn new(
        items: &'a [PlanItem],
        keys: &'a Keys,
        wanted: &'a HashMap<&'a str, &'a Path>,
        platform: &'a dyn Platform,
        any_dir: bool,
    ) -> Self {
        let mut folders = HashSet::new();
        // A listing is sorted by path, so consecutive rows share a folder and
        // one key per folder is computed, not one per row.
        let mut last: Option<&Path> = None;
        for item in items {
            let Some(parent) = item.source.parent() else {
                continue;
            };
            if last.is_some_and(|seen| same_path(seen, parent)) {
                continue;
            }
            last = Some(parent);
            folders.insert(path_key(parent, platform.naming_rules(&item.source)));
        }
        let changed = || items.iter().filter(|i| i.state.is_changed());
        // Only a listed *folder* can have a write beneath it, and without one
        // in the listing nothing a write names can move.
        let landings = if any_dir {
            items
                .iter()
                .filter(|i| matches!(i.state, RowState::Changed | RowState::Unchanged))
                .filter(|i| i.target != i.source)
                .map(|i| {
                    (
                        path_key(&i.source, platform.naming_rules(&i.source)),
                        i.target.as_path(),
                    )
                })
                .collect()
        } else {
            HashMap::new()
        };
        Self {
            platform,
            folders,
            moving: changed()
                .map(|i| (keys.source[i.index].as_str(), i))
                .collect(),
            claimed: changed()
                .map(|i| (keys.target[i.index].as_str(), i))
                .collect(),
            wanted,
            landings,
            planned: HashSet::new(),
        }
    }

    fn plan(&mut self, write: crate::ops::FileWrite) -> Result<PlannedOp, Refused> {
        // Absolute only (P60). A relative path would resolve against the
        // process's working directory, which is not a folder the user chose
        // and, for the GUI, is wherever the app happened to be launched from.
        if !write.path.is_absolute() {
            return Err(Refused::Note(format!(
                "script: '{}' is not a full path, so nothing was written",
                write.path.display()
            )));
        }
        let rules = self.platform.naming_rules(&write.path);
        let in_listed = |path: &Path| {
            matches!(path.components().next_back(), Some(Component::Normal(_)))
                && path
                    .parent()
                    .is_some_and(|folder| self.folders.contains(&path_key(folder, rules)))
        };
        // As named first — the folder spelled as written, then the file's own
        // name — so a listing named through `..` is compared with a write
        // named the same way; normalised only when that fails.
        let as_named = write
            .path
            .parent()
            .zip(write.path.file_name())
            .map(|(folder, name)| folder.join(name))
            .filter(|path| in_listed(path));
        let (path, in_listed_folder) = match as_named {
            Some(path) => (path, true),
            None => {
                let path = normalize_lexically(&write.path);
                let inside = in_listed(&path);
                (path, inside)
            }
        };
        let rules = self.platform.naming_rules(&path);
        let shown = path.display();
        let blocked = |why: &str| Err(Refused::Blocked(format!("script: {shown} {why}")));

        if !in_listed_folder {
            // The one way a well-meaning script ends up here: `fr.path` of a
            // folder whose name is not text carries U+FFFD (D159), and the
            // path built from it names a folder that does not exist.
            return if path.to_string_lossy().contains('\u{FFFD}') {
                blocked(
                    "is in a folder whose name is not valid Unicode, so a script cannot name it",
                )
            } else {
                blocked("is outside the folders this run lists")
            };
        }

        let key = path_key(&path, rules);
        if let Some(item) = self.claimed.get(key.as_str()) {
            return blocked(&format!(
                "is the new name this run gives {}",
                item.source.display()
            ));
        }
        if self.moving.contains_key(key.as_str()) {
            return blocked("is a file this run renames");
        }
        if self.wanted.contains_key(key.as_str()) {
            return blocked("is a folder this run creates");
        }
        let undoability = match path.symlink_metadata() {
            Ok(meta) if meta.is_dir() => return blocked("is a folder"),
            Ok(meta) if meta.file_type().is_symlink() => return blocked("is a link"),
            Ok(_) => Undoability::None,
            Err(_) => Undoability::Full,
        };

        let landing = self.landing(path.clone(), rules);
        if !self
            .planned
            .insert(path_key(&normalize_lexically(&landing), rules))
        {
            return blocked("is written twice by this run");
        }
        Ok(PlannedOp::WriteFile {
            path: landing,
            contents: write.contents,
            undoability,
        })
    }

    /// Where `path` is once every rename has run.
    ///
    /// A script builds its paths from the listing *before* the run, and the
    /// write runs after every rename — so a playlist aimed at `My_Album` has
    /// to go into the folder that is `My Album` by then. The deepest listed
    /// ancestor already carries every rename above it, so one hop is the
    /// whole answer.
    fn landing(&self, path: PathBuf, rules: &ren_platform::NamingRules) -> PathBuf {
        if self.landings.is_empty() {
            return path;
        }
        for ancestor in path.ancestors() {
            if let Some(landed) = self.landings.get(&path_key(ancestor, rules)) {
                let rest = path.strip_prefix(ancestor).unwrap_or(Path::new(""));
                return if rest.as_os_str().is_empty() {
                    landed.to_path_buf()
                } else {
                    landed.join(rest)
                };
            }
        }
        path
    }
}

/// `path` with `.` dropped and each `..` taking away the component before it,
/// without asking the disk. A `..` at the root stays at the root, as it does
/// on every filesystem.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other),
        }
    }
    out
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

/// Whether a changed row's target is in a different folder from its source.
///
/// [`target_path`] joins the produced name onto the source's own folder, and
/// every component of that name has passed [`validate_target`] — no empty
/// component, no `.` or `..` — so the target leaves the folder exactly when
/// the name carries a separator (D31). One `memchr` over the name, where the
/// obvious test costs two `Path::parent` walks per row.
fn moves_folder(item: &PlanItem) -> bool {
    item.new_name.contains(SUBFOLDER_SEPARATOR)
}

/// Two paths that are the same folder.
///
/// The bytes first: a `Path` compares by component, which walks both paths,
/// and the parents this file compares are cut from one listing where equal
/// folders are byte-for-byte equal. The component comparison is kept as the
/// fallback so a differently spelled path is still equal.
fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str() == b.as_os_str() || a == b
}

/// Joins a produced name — subfolders and all — onto the folder it came from.
fn target_path(parent: &Path, name: &str) -> PathBuf {
    let mut path = parent.to_path_buf();
    for component in name.split(SUBFOLDER_SEPARATOR) {
        path.push(component);
    }
    path
}

/// A comparison key for a path under `rules`: folded when the path is text,
/// exact when it is not.
///
/// Folding goes through `&str`, and a path that is not valid Unicode has no
/// `&str`. `to_string_lossy` used to stand in, which turned two sibling folders
/// `Caf\xE9` and `Caf\xE8` into one `Caf\u{FFFD}` — and every file inside
/// them into a false collision, or a false "freed by another row" that let a
/// real one through to fail at run time. D159 keeps such folders in play, so
/// their paths are keyed on their exact bytes instead: a NUL first, which no
/// path on any platform contains, so an exact key can never equal a folded
/// one. Unfolded, so on a case-insensitive volume two spellings of such a
/// path are two keys; `Platform::rename` never replaces (P13), so what that
/// misses fails safely at run time.
fn path_key(path: &Path, rules: &ren_platform::NamingRules) -> String {
    match path.to_str() {
        Some(text) => rules.fold(text),
        None => {
            use std::fmt::Write as _;
            let bytes = path.as_os_str().as_encoded_bytes();
            let mut key = String::with_capacity(1 + 2 * bytes.len());
            key.push('\0');
            for byte in bytes {
                let _ = write!(key, "{byte:02x}");
            }
            key
        }
    }
}

/// What sits at a path, as far as the `<\>` passes care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Occupant {
    Nothing,
    Folder,
    /// A file, a link, anything a folder cannot be created over or moved into.
    Other,
}

/// The disk, asked once per path.
///
/// [`detect_blocked_subfolders`] and [`wanted_directories`] both walk every
/// ancestor of every row that moves, and a run sorting ten thousand files into
/// a handful of folders asks about the same handful twenty thousand times — a
/// handle opened and closed per question on Windows. One answer per folder
/// does for both passes. `symlink_metadata`, so a link is never followed: to
/// the first pass it is something a folder cannot be made over, and the
/// second only sees rows the first let through.
#[derive(Debug, Default)]
struct DiskProbe(HashMap<PathBuf, Occupant>);

impl DiskProbe {
    fn at(&mut self, path: &Path) -> Occupant {
        if let Some(&occupant) = self.0.get(path) {
            return occupant;
        }
        let occupant = match path.symlink_metadata() {
            Ok(meta) if meta.is_dir() => Occupant::Folder,
            Ok(_) => Occupant::Other,
            Err(_) => Occupant::Nothing,
        };
        self.0.insert(path.to_path_buf(), occupant);
        occupant
    }
}

/// A subfolder a move needs, blocked by an existing file of that name (D31).
///
/// Found by the M4 property tests: a file called `(` renamed to `(/(` asks the
/// executor to create a directory where a file already is, which fails halfway
/// through the batch. Checking here turns it into a blocked run with a reason.
fn detect_blocked_subfolders(
    items: &mut [PlanItem],
    platform: &dyn Platform,
    disk: &mut DiskProbe,
) {
    for item in items.iter_mut().filter(|i| i.state.is_changed()) {
        // A rename that stays in its folder — every row without `<\>`, which
        // is nearly every row — has no subfolder to check and cannot be moving
        // into itself. Decided by the produced name rather than by the two
        // parents: `target_path` joins the name onto the source's folder, so
        // the target is in another folder exactly when the name has a
        // separator in it, and `Path::parent` twice per row was the whole
        // cost of this pass.
        if !moves_folder(item) {
            continue;
        }
        let source_dir = item.source.parent().unwrap_or(Path::new(""));
        let Some(target_dir) = item.target.parent() else {
            continue;
        };
        let mut at = target_dir;
        let mut blocked = false;
        while at != source_dir {
            if disk.at(at) == Occupant::Other {
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
        //
        // The target is the source's folder joined with the produced name's
        // components, so it is inside the source exactly when the *first*
        // component is the source's own name — compared through the volume's
        // rules, because on NTFS `holiday/holiday` from `Holiday` is inside
        // itself too, and a byte comparison let it through to fail at
        // `MoveFileExW`.
        if !blocked && first_component_is_own_name(item, platform.naming_rules(&item.source)) {
            item.state = RowState::Conflict(ConflictKind::IntoItself);
        }
    }
}

/// Whether a moving row's produced name starts with the row's own name.
fn first_component_is_own_name(item: &PlanItem, rules: &ren_platform::NamingRules) -> bool {
    let first = item
        .new_name
        .split(SUBFOLDER_SEPARATOR)
        .next()
        .unwrap_or_default();
    match item.source.file_name().and_then(|name| name.to_str()) {
        Some(own) => rules.fold(first) == rules.fold(own),
        None => false,
    }
}

/// A `<\>` move into a folder this same run renames (P94, extended).
///
/// Walks each moving row's *target* folders down from where it starts and
/// refuses the row if one of them is the source of another row that moves —
/// see [`ConflictKind::IntoRenamedFolder`] for why no order can work. Only
/// asked when a folder is listed, and only of rows whose name carries a
/// separator, so the common run pays nothing.
fn detect_moves_into_renamed_folders(keys: &Keys, platform: &dyn Platform, items: &mut [PlanItem]) {
    let moving: HashSet<&str> = items
        .iter()
        .filter(|i| i.state.is_changed())
        .map(|i| keys.source[i.index].as_str())
        .collect();
    let mut refused: Vec<(usize, PathBuf)> = Vec::new();
    for item in items.iter().filter(|i| i.state.is_changed()) {
        if !moves_folder(item) {
            continue;
        }
        let source_dir = item.source.parent().unwrap_or(Path::new(""));
        let rules = platform.naming_rules(&item.source);
        let mut at = item.target.parent();
        while let Some(dir) = at {
            if dir == source_dir {
                break;
            }
            if moving.contains(path_key(dir, rules).as_str()) {
                refused.push((item.index, dir.to_path_buf()));
                break;
            }
            at = dir.parent();
        }
    }
    for (index, path) in refused {
        items[index].state = RowState::Conflict(ConflictKind::IntoRenamedFolder { path });
    }
}

/// The folders the plan has to create, parents first and each one only once.
///
/// Only folders that do not already exist are listed, so undo knows that every
/// one of them is its own to remove.
fn directory_creations(wanted: &[(PathBuf, String)]) -> Vec<PlannedOp> {
    wanted
        .iter()
        .map(|(path, _)| PlannedOp::CreateDir { path: path.clone() })
        .collect()
}

/// The walk that finds them, hoisted so it happens once, with each folder's
/// comparison key.
///
/// Two passes need it — [`directory_creations`] turns it into ops, and
/// [`detect_targets_needed_as_folders`] has to know whether a row's new name is
/// one of these — and the walk is the expensive half: a disk probe per
/// ancestor per row that moves, answered once per folder by `disk`. A run with
/// no `<\>` in it never gets past the first `continue`, which is why the
/// common case costs a `parent()` compare.
///
/// **One folder per key, not per spelling.** On a volume that folds case, `a/`
/// and `A/` asked for by two rows are one folder: the second `CreateDir` would
/// fail with "already exists" and stop the run before a file was sorted. The
/// first spelling is created, and the row that asked for the other lands in
/// it — which is what that volume does with the name anyway.
fn wanted_directories(
    items: &[PlanItem],
    platform: &dyn Platform,
    disk: &mut DiskProbe,
) -> Vec<(PathBuf, String)> {
    let mut wanted: Vec<(PathBuf, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for item in items.iter().filter(|i| i.state.is_changed()) {
        if !moves_folder(item) {
            continue;
        }
        let source_dir = item.source.parent().unwrap_or(Path::new(""));
        let Some(target_dir) = item.target.parent() else {
            continue;
        };
        // Walk up to the folder the file started in, then create downwards.
        let mut missing: Vec<&Path> = Vec::new();
        let mut at = target_dir;
        while at != source_dir {
            if disk.at(at) == Occupant::Nothing {
                missing.push(at);
            }
            match at.parent() {
                Some(parent) => at = parent,
                None => break,
            }
        }
        let rules = platform.naming_rules(&item.source);
        for dir in missing.into_iter().rev() {
            let key = path_key(dir, rules);
            if seen.insert(key.clone()) {
                wanted.push((dir.to_path_buf(), key));
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
    wanted: &HashMap<&str, &Path>,
    items: &mut [PlanItem],
) {
    // No `<\>` anywhere in the run means nothing to create and nothing to
    // check, which is the overwhelmingly common case.
    if wanted.is_empty() {
        return;
    }

    for item in items.iter_mut().filter(|i| i.state.is_changed()) {
        // A row's own wanted folders are all strict ancestors of its target, so
        // it can never be its own conflict here.
        if let Some(path) = wanted.get(keys.target[item.index].as_str()) {
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
                (path_key(&item.source, rules), path_key(&item.target, rules))
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

    // One read per destination folder, and — since a listing is sorted by
    // path, so consecutive rows share one — one *lookup* per folder too: the
    // last folder's set is kept to hand, and the map is only asked when the
    // folder changes. Asking it per row meant an owned `PathBuf` and a
    // component-wise hash of it per row, which was most of this pass.
    let mut occupied: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    let mut last: Option<(PathBuf, HashSet<String>)> = None;
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

        let dir = item.target.parent().unwrap_or(Path::new(""));
        if !last.as_ref().is_some_and(|(seen, _)| same_path(seen, dir)) {
            if let Some((seen, names)) = last.take() {
                occupied.insert(seen, names);
            }
            let names = match occupied.remove(dir) {
                Some(names) => names,
                None => read_dir_folded(dir, platform.naming_rules(&item.source)),
            };
            last = Some((dir.to_path_buf(), names));
        }
        let (_, names) = last.as_ref().expect("set above");
        if names.contains(target_key) {
            item.state = RowState::Conflict(ConflictKind::TargetExists);
        }
    }
}

/// The keys of the full paths of everything currently in `dir`.
///
/// Keyed with the *same* rules the target keys use — lower-casing
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
        .map(|e| path_key(&e.path(), rules))
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
fn order_renames(
    keys: &Keys,
    platform: &dyn Platform,
    any_dir: bool,
    wanted: &HashMap<&str, &Path>,
    items: &mut [PlanItem],
) -> Vec<PlannedOp> {
    let movers: Vec<usize> = items
        .iter()
        .filter(|i| i.state.is_changed())
        .map(|i| i.index)
        .collect();
    if movers.is_empty() {
        return Vec::new();
    }

    // Indexed by item, not hashed by it. Every structure below used to be a
    // `HashMap<usize, _>` or `HashSet<usize>` keyed by the item index, and the
    // hashing was a third of this function's time at ten thousand rows; a
    // `Vec` the size of the listing answers the same questions by offset.
    let n = items.len();
    let source_of: HashMap<&str, usize> = movers
        .iter()
        .map(|&i| (keys.source[i].as_str(), i))
        .collect();

    // successors[y] = renames that may only run once y has moved away.
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut in_degree: Vec<usize> = vec![0; n];
    for &x in &movers {
        if let Some(&y) = source_of.get(keys.target[x].as_str())
            && y != x
        {
            successors[y].push(x);
            in_degree[x] += 1;
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
    // O(rows²) that would show up in the 10 000-file budget. And only when a
    // folder is listed at all: the walk hashes every ancestor of every mover,
    // and with no folder in the listing there is nothing for it to find.
    if any_dir {
        let mover_at: HashMap<&Path, usize> = movers
            .iter()
            .map(|&i| (items[i].source.as_path(), i))
            .collect();
        for &x in &movers {
            let mut above = items[x].source.parent();
            while let Some(dir) = above {
                if let Some(&folder) = mover_at.get(dir) {
                    successors[x].push(folder);
                    in_degree[folder] += 1;
                }
                above = dir.parent();
            }
        }
    }

    // Every name this batch touches, so a temp name can avoid all of them —
    // the folders it creates included: they do not exist yet when a temp name
    // is chosen, so the disk probe cannot see them, and `CreateDir` runs
    // before every rename. Borrowed from `keys` rather than cloned: two owned
    // strings per mover was twenty thousand allocations per keystroke for a
    // set consulted only when a cycle turns up.
    let reserved: HashSet<&str> = movers
        .iter()
        .flat_map(|&i| [keys.source[i].as_str(), keys.target[i].as_str()])
        .chain(wanted.keys().copied())
        .collect();
    // Where each folder's next temp name is looked for. Temp names are never
    // released — every finish runs at the end — so a counter that only moves
    // forward is all the bookkeeping they need: nothing it has passed can be
    // handed out again, and a thousand swaps in one folder cost a thousand
    // probes rather than half a million.
    let mut cursors: HashMap<String, u32> = HashMap::new();

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
    // 1000-file plan a fifth of its time. Counted for a files-only listing
    // too: a recursive one has files at several depths, and the plan's op
    // order is part of what two runs over one tree have to agree on.
    let depths: Vec<usize> = items.iter().map(|i| depth(&i.source)).collect();

    let mut ready: Vec<usize> = movers
        .iter()
        .copied()
        .filter(|&i| in_degree[i] == 0)
        .collect();
    ready.sort_unstable_by_key(|&i| (depths[i], std::cmp::Reverse(i)));
    // Pop deepest first, and the lowest index among equals, so plans are stable.

    // The order victims are chosen in when a cycle has to be broken: deepest
    // first, lowest index among equals — the same order `ready` pops in, for
    // the same reasons. A cursor over it rather than a scan per cycle, so a
    // listing of thousands of pairwise swaps costs thousands of steps, not
    // thousands of passes over thousands of movers.
    let mut by_victim_order: Vec<usize> = movers.clone();
    by_victim_order.sort_unstable_by_key(|&i| (std::cmp::Reverse(depths[i]), i));
    let mut next_victim = 0usize;

    let mut ops: Vec<PlannedOp> = Vec::with_capacity(movers.len());
    let mut finishes: Vec<PlannedOp> = Vec::new();
    let mut done: Vec<bool> = vec![false; n];
    let mut stuck: Vec<usize> = Vec::new();

    loop {
        while let Some(y) = ready.pop() {
            ops.push(PlannedOp::Rename {
                from: items[y].source.clone(),
                to: items[y].target.clone(),
                kind: RenameKind::Direct,
            });
            done[y] = true;
            release(&successors, &mut in_degree, &mut ready, &done, y);
        }

        // Deepest first here too, for the same reason: staging a folder under a
        // temp name moves everything inside it, so a pending rename within that
        // folder has to have run already. Lowest index breaks a depth tie, which
        // keeps the choice of victim — and so the whole plan — deterministic.
        while next_victim < by_victim_order.len() && done[by_victim_order[next_victim]] {
            next_victim += 1;
        }
        let Some(&victim) = by_victim_order.get(next_victim) else {
            break;
        };
        // Whether stuck or staged, this row is settled either way.
        next_victim += 1;

        // Break the cycle: move the victim out of the way first.
        let Some(temp) = temp_path(&items[victim].source, platform, &reserved, &mut cursors) else {
            stuck.push(victim);
            continue;
        };

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
        done[victim] = true;
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
    //
    // Only a *folder* rename can be an ancestor of a temp name, so with no
    // folder listed every finish goes on the end — without a scan of `ops`
    // per finish, which for a listing of pairwise swaps was a scan of
    // thousands per thousands.
    if !any_dir {
        ops.extend(finishes);
        return ops;
    }
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
/// **Every row the run can execute gets its landing**, not only the renamed
/// ones. A row that keeps its name still moves when a folder above it does,
/// and an action on it runs at [`PlanItem::final_path`] like any other — so
/// keeping the answer only for renamed rows addressed that action at the
/// folder's old path, in a plan that said it was executable.
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

        if matches!(items[i].state, RowState::Changed | RowState::Unchanged) {
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
    successors: &[Vec<usize>],
    in_degree: &mut [usize],
    ready: &mut Vec<usize>,
    done: &[bool],
    y: usize,
) {
    for &x in &successors[y] {
        if done[x] {
            continue;
        }
        in_degree[x] -= 1;
        if in_degree[x] == 0 {
            ready.push(x);
        }
    }
}

/// How many foreign files named like our temp names a folder may hold before
/// a cycle in it is refused as [`ConflictKind::UnresolvedCycle`].
///
/// A bound rather than a search to exhaustion, so a folder full of
/// `__renameit-tmp-*` files costs a preview a known number of probes.
const TEMP_NAMES_ON_DISK: u32 = 1_000;

/// A free name in `source`'s directory to park a file under.
///
/// Deterministic, so `plan()` stays a pure function of its inputs and two runs
/// over the same tree produce byte-identical plans. Checked against both the
/// names this batch touches and the directory itself, starting where the last
/// temp name handed out in this folder left off.
fn temp_path(
    source: &Path,
    platform: &dyn Platform,
    reserved: &HashSet<&str>,
    cursors: &mut HashMap<String, u32>,
) -> Option<PathBuf> {
    let dir = source.parent().unwrap_or(Path::new(""));
    let rules = platform.naming_rules(source);
    let cursor = cursors.entry(path_key(dir, rules)).or_insert(0);
    // Every candidate turned away is one of the reserved names or a file on
    // disk, and the cursor never revisits one, so this is enough for every
    // reserved name plus the foreign files the bound allows.
    let reserved_len = u32::try_from(reserved.len()).unwrap_or(u32::MAX);
    let limit = cursor
        .saturating_add(reserved_len)
        .saturating_add(TEMP_NAMES_ON_DISK);
    while *cursor < limit {
        let candidate = dir.join(format!("__renameit-tmp-{cursor}"));
        *cursor += 1;
        if reserved.contains(path_key(&candidate, rules).as_str()) {
            continue;
        }
        if candidate.symlink_metadata().is_ok() {
            continue;
        }
        return Some(candidate);
    }
    None
}
