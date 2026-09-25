//! Executing a plan, and undoing it.
//!
//! Every destructive step is written to a write-ahead JSONL journal *before* it
//! happens, so a crash mid-batch leaves enough on disk to roll back (P9).

use std::path::PathBuf;

pub mod apply;
pub mod journal;
pub mod recover;
pub mod undo;

pub use apply::{ApplyOptions, ApplyReport, apply};
pub use journal::{JOURNAL_VERSION, Journal, Line, Record, default_journal_dir};
pub use recover::{Unfinished, rollback, unfinished};
pub use undo::{UndoReport, undo_last, undo_transaction};

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Platform(#[from] ren_platform::PlatformError),
    #[error("journal {path} is corrupt at line {line}: {source}")]
    CorruptJournal {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    /// Written by a newer build. Modelled on `JobError::FromTheFuture`, which
    /// M4 introduced for presets and the journal never got.
    #[error(
        "journal {path} was written by a newer RenameIt (version {found}, this build reads {known})"
    )]
    JournalFromTheFuture {
        path: PathBuf,
        found: u32,
        known: u32,
    },
    #[error("journal {path} contains a record this build does not understand")]
    JournalNotUnderstood { path: PathBuf },
    /// Another run holds this journal: it is being written right now.
    ///
    /// A `Journal` keeps an exclusive lock on its file for as long as it
    /// lives, so a transaction with no `Commit` whose lock is taken is a batch
    /// still under way in another window — not one that crashed. Offering it
    /// for rollback would reverse renames while that run keeps going; undoing
    /// it would stamp `Undone` in the middle of a journal still being
    /// appended to. Both refuse with this instead. A crashed process holds no
    /// lock, so a journal it left behind is never mistaken for this.
    #[error("journal {path} belongs to a run still going on in another window")]
    JournalInUse { path: PathBuf },
    /// A record could not be turned into a line.
    ///
    /// This used to be an `.expect("journal records are always serialisable")`,
    /// and the premise was false: `serde`'s `impl Serialize for Path` refuses a
    /// path that is not valid UTF-8, so one oddly-named file aborted the whole
    /// batch — after the journal was already open. `raw_path` now encodes those
    /// losslessly, which leaves this variant with no known trigger; it exists
    /// because a write path that can fail should say so rather than abort.
    #[error("journal {path}: a record could not be written: {source}")]
    JournalNotWritable {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// P2: an irreversible action needs explicit permission, and the caller
    /// did not give it.
    #[error("refusing to run: {items} item(s) would be changed in a way that cannot be undone")]
    Irreversible { items: usize },
    /// P4: conflicts block the run instead of being skipped one by one.
    ///
    /// `blockers` carries [`crate::Plan::blockers`] word for word: a run
    /// refused for a reason that belongs to no row — a script write outside
    /// the listed folders — would otherwise say "0 conflict(s) and 0 error(s)"
    /// and nothing else.
    #[error("refusing to run: {}", blocked_reason(*conflicts, *errors, blockers))]
    Blocked {
        conflicts: usize,
        errors: usize,
        blockers: Vec<String>,
    },
    /// A plan names a path that is not absolute.
    ///
    /// A journal records paths exactly as the plan gives them, and a relative
    /// one resolves against whatever the working directory is *at undo time*
    /// — so `ren-cli apply .` followed by an undo from another folder would
    /// replay the batch somewhere else. Refused before a journal is opened;
    /// front ends make their paths absolute when they list.
    #[error("refusing to run: {path} is not a full path, so it could not be undone reliably")]
    RelativePath { path: PathBuf },
    #[error("no transaction to undo in {0}")]
    NothingToUndo(PathBuf),
    /// The run stopped part-way because the *journal* could not be written.
    ///
    /// Distinct from every other error because of *when* it happens: after
    /// `Begin` is durable, some of the plan may already have been performed,
    /// and a caller that treats this like a refused command line — exit code
    /// 1, "nothing was touched" — is lying to whoever automates against it
    /// (D103 reserves 3 for a run that started). The journal carries what did
    /// happen, so the next step is `recover`, not a retry.
    #[error(
        "the run stopped after {completed} change(s) because the journal could not be \
         written: {source}. The journal {journal} records what happened; recover or undo \
         transaction {txn} before running again"
    )]
    Interrupted {
        journal: PathBuf,
        txn: String,
        completed: usize,
        #[source]
        source: Box<ExecError>,
    },
}

/// The sentence after "refusing to run:".
fn blocked_reason(conflicts: usize, errors: usize, blockers: &[String]) -> String {
    let rows = (conflicts > 0 || errors > 0 || blockers.is_empty())
        .then(|| format!("{conflicts} conflict(s) and {errors} error(s) in the plan"));
    rows.into_iter()
        .chain(blockers.iter().cloned())
        .collect::<Vec<_>>()
        .join("; ")
}

impl ExecError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
