# RenameIt

A batch file renamer written in Rust. Modern UI, cross-platform core, Windows
ships first. MIT-licensed.

## Start here (every session)

1. `docs/DECISIONS.md` is the **authority** on every policy/tech decision
   (stack, licensing, deferred features, default behaviours). Never contradict
   it silently; append to it when you make a new policy call.
2. `docs/DESIGN.md` is the architecture: engine model, GUI/UX screens, quality
   strategy. Parts 1–3 are the planning documents; **Part 4 is the app as
   built** — the worker threads and a module map per crate. Start there.
3. Behaviour that is not obvious from the code is explained in the doc comment
   beside it. Read the module doc before changing a module.

## Hard rules

- **Licensing (D2):** the project is MIT. Any new dependency must be
  MIT/Apache-2.0/BSD/Zlib/ISC-class — check before adding; `cargo-deny`
  enforces it in CI. No GPL/LGPL static linking, ever.
- **Cross-platform discipline (D3):** OS-specific code lives only in
  `crates/ren-platform` behind the `Platform` trait. The one exception is the
  journal's lossless path encoding in `ren-core` (std only, D240). `ren-core`
  must build and test green on Linux at all times, even though Windows ships
  first.
- **Data is ours (D6):** shipped defaults — presets, scripts, casing rules,
  the batch-replace list — are our own data in our own formats.
- **Session hygiene:** before ending a session, the quality gate below must be
  green, decisions recorded, work committed.

## Layout

- `docs/DECISIONS.md` — decision record (D* locked decisions, P* default policies)
- `docs/DESIGN.md` — consolidated architecture
- `docs/tags.md` — the `<tag>` reference
- `docs/MIGRATION-legacy-scripts.md` — porting a legacy `.frs` script to Koto
- `docs/manual-checks.md` — the few things CI cannot prove, and how to check them
- `docs/spikes/` — written verdicts on the risky assumptions
- `deny.toml` / `about.toml` — which licences are allowed (D2, D238) / the
  notices a release ships (D237)
- `crates/` — Cargo workspace: `ren-core` (engine), `ren-platform` (OS traits),
  `ren-cli`, `ren-gui` (egui/eframe)

## Verification

- Engine semantics: unit tests beside the code, plus proptest invariants (no
  plan collision, no file loss, exact undo, preview == disk result) in
  `crates/ren-core/tests/properties.rs`.
- Run the GUI: `cargo run -p ren-gui`. Headless: `cargo run -p ren-cli -- preview|apply|undo`
  on a tempdir.
- Windows behaviour (attributes, created-date, case-insensitive renames) is
  only truly tested on the Windows CI runner or the user's machine.

## The quality gate

Everything below must be green before a session ends:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --lib --bins --target x86_64-pc-windows-msvc -- -D warnings
cargo test --workspace
TZ=Asia/Kolkata cargo test --workspace   # P61: a UTC agent cannot tell Local from Utc
cargo deny check
cargo run --release -p ren-core --example plan_budget   # a 10k-file preset plan, p95 under 50 ms
cargo run --release -p ren-gui --example spike_preview_headless -- 10000 100        # frame budget (D165)
cargo run --release -p ren-gui --example spike_preview_headless -- 10000 100 grid   # grid tile count (D165)
cargo bench -p ren-core --bench plan     # the same numbers with criterion's statistics; keep them in the commit message
```
