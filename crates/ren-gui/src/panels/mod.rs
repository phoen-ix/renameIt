//! The four zones of the main window (screen S1).

pub mod about;
pub mod ask;
pub mod confirm;
pub mod file_table;
pub mod grid;
pub mod operation;
pub mod palette;
pub mod presets;
pub mod rows;
pub mod run_settings;
pub mod settings;
pub mod source_bar;
pub mod status_bar;
pub mod tile;
pub mod visual_assist;

pub use ask::{AskForm, AskOutcome};
pub use file_table::FileTable;
pub use source_bar::SourceBarOutput;
pub use status_bar::StatusBarOutput;
