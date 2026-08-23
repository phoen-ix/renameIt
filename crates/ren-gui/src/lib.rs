//! The RenameIt desktop application.
//!
//! Layered so the interesting half is testable without a window:
//!
//! * [`viewmodel`] — session, preview worker, execute/undo history. No egui.
//! * [`panels`] — the widgets that draw the viewmodel.
//! * [`app`] — layout, hotkeys, persistence.
//! * [`theme`] — the `Style` both themes are built from.
//!
//! `spike` is M0's throwaway performance harness, kept because it is still the
//! benchmark CI runs against the 50 ms budget.

pub mod app;
pub mod dialogs;
pub mod editors;
pub mod launch;
pub mod panels;
pub mod spike;
pub mod theme;
pub mod thumbs;
pub mod viewmodel;
pub mod widgets;

pub use app::RenameItApp;
