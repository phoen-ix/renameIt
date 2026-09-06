//! The RenameIt desktop application.
//!
//! Layered so the interesting half is testable without a window:
//!
//! * [`viewmodel`] — session, preview worker, execute/undo history. No egui.
//! * [`panels`] — the widgets that draw the viewmodel.
//! * [`app`] — layout, hotkeys, persistence.
//! * [`theme`] — the `Style` both themes are built from.
//!
//! M0's preview-performance harness lives with the two examples that run it
//! (`examples/spike/mod.rs`), not in this library: it is still CI's frame
//! gate, but it is not production code and the shipped binary has no use
//! for it.

pub mod app;
pub mod dialogs;
pub mod editors;
pub mod launch;
pub mod panels;
pub mod theme;
pub mod thumbs;
pub mod viewmodel;
pub mod widgets;

pub use app::RenameItApp;
