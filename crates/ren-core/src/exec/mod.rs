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
    #[error("refusing to run: {conflicts} conflict(s) and {errors} error(s) in the plan")]
    Blocked { conflicts: usize, errors: usize },
    #[error("no transaction to undo in {0}")]
    NothingToUndo(PathBuf),
}

impl ExecError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
