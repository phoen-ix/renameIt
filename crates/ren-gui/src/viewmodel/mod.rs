//! State and logic, with no egui in it.
//!
//! Keeping these free of widgets is what lets them be tested as plain Rust —
//! the headless `egui_kittest` suite (D24) then only has to cover the wiring
//! rather than the behaviour.

pub mod apply;
pub mod columns;
pub mod history;
pub mod listing;
pub mod preview;
pub mod session;
pub mod stack;

pub use apply::{ApplyWorker, InFlight, JobKind, Outcome};
pub use columns::{Column, ColumnKind, Columns, TableStyle};
pub use history::{Batch, History, JournalProblem, LogLine, describe_counts};
pub use listing::{Listed, ListingWorker, Source};
pub use preview::{PreviewWorker, Ready};
pub use session::{
    RowFilter, Selection, Session, SessionSettings, Sort, SortColumn, SourceMode, THUMB_DEFAULT,
    THUMB_MAX, THUMB_MIN, ViewMode,
};
pub use stack::{Card, CardId, CardStack};
