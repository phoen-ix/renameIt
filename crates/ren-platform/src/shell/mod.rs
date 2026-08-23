//! The Explorer menu — **D7**, narrowed by D132, widened again by the preset
//! menu.
//!
//! Split in two, and the split is what makes the feature testable at all.
//!
//! * [`plan`] computes the whole registry layout as **data**, with no
//!   `cfg(windows)` anywhere, so the part that can be silently wrong — an unset
//!   `(Default)`, a sort prefix, a doubled ampersand, a quoted `%1` — is
//!   ordinary Linux unit tests.
//! * `apply` writes it, and is a loop over two enum arms.
//!
//! Before this, `shell.rs` was `#![cfg(windows)]` end to end and **nothing in
//! it ran on CI's Linux runner**; the whole file was type-checked and never
//! executed.

pub mod plan;

pub use plan::{MenuPreset, ShellPlan, Step, Value};

#[cfg(windows)]
mod apply;

#[cfg(windows)]
pub use apply::{is_registered, register, unregister};
