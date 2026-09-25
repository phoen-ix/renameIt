//! Scripting: the escape hatch for renames nobody anticipated.
//!
//! Legacy `.frs` scripts are VBScript hosted by a Windows-only COM component,
//! which is impractical to support cross-platform. Koto replaces it.
//!
//! So we keep the **shape** — the `init`/`rename`/`done` lifecycle, the eleven
//! façade members (plus four new ones, fifteen in all), the description
//! header, the Script folder — and change the
//! language to [Koto](https://koto.dev) (D92, which reverses D5's "Rhai only":
//! rhai depends unconditionally on `smartstring`, which is MPL-2.0+ and outside
//! D2's allowlist).
//!
//! # The sandbox is *removal*, not absence (D93)
//!
//! This is the thing to understand before touching anything here, because it
//! inverts the posture the Rhai design assumed. Rhai's `Engine::new()` has no IO
//! at all and you add back what you want. **Koto's prelude ships the filesystem
//! and process execution by default**, so an unhardened `Koto::default()` hands
//! a downloaded script `io.remove_file` and `os.command`.
//!
//! Hardening therefore means taking things away, and taking things away is
//! fragile in a way that adding them is not: a koto point release that adds one
//! function to `io` would silently widen the sandbox and nothing would fail. So
//! every removal below has its own test, and those tests are the real security
//! boundary — not this comment.
//!
//! See [`engine`] for what is removed and why each one matters.

pub mod engine;
pub mod facade;
pub mod header;
pub mod store;

pub use engine::{Compiled, SCRIPT_DEADLINE, ScriptError, compile};
pub use facade::Session;
pub use header::Header;
pub use store::{Legacy, ScriptEntry, ScriptStore, default_script_dir};
