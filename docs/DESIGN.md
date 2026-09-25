# RenameIt — Consolidated Design

Three independent design documents produced during planning (2026-08-15), kept below as Parts 1–3 and amended inline where a decision superseded them, plus **Part 4, the app as built**.
They converged on the same stack: **egui/eframe GUI, Koto scripting (D92; they said Rhai), lofty/kamadak-exif metadata, fancy-regex, transactional planner with journal-backed undo**.

**Authority order: `docs/DECISIONS.md` > this file.**
Where a section below contradicts a locked decision, the decision wins.

> **Reading Parts 1–3.** They are the 2026-08-15 planning documents. A paragraph that is struck or carries an amendment has been checked against the code; one that does not may still describe the plan rather than the program, so check `docs/DECISIONS.md` before relying on it. **The module maps in Part 1 §1 and Part 2 §2 are the plan's; [Part 4](#part-4--as-built-14--unreleased) has the real ones**, and the worker threads.

Known deltas — where the plan below and the program differ, with the decision that settled each:

- **No legacy importers.** Sections mentioning importers for legacy settings files, or a COM scripting bridge, are superseded: the locked decision is *fresh start* — ship equivalent defaults in new native formats (recreated as data, not byte-copied), one-language scripting (Koto, D92).
- **Crate naming**: use `ren-core`, `ren-platform`, `ren-cli`, `ren-gui` (Part 1 layout); Part 2 says `renameit-*` and Part 3 `renamer-*` for the same four.
- **Module maps** (Part 1 §1, Part 2 §2): there is no `scope.rs`, `tags/`, `preview/`, `ren-platform/windows/` or `unix/` directory, no `viewmodel/undo.rs`, `widgets/tag_picker.rs` or `widgets/virtual_table.rs`, and the editors are one per operation. See Part 4.
- **Presets are TOML** (D18, D37), not serde JSON, and there is **no "override per-step options" flag** (D32).
- **Every tag is case-insensitive** (D62), metadata namespaces included.
- **The template** is `Node::{Literal, Tag}` behind a private `Resolver`; there is no `TagResolver` trait, no `PathSep` node (`<\>` is a tag), and `TagNeeds` is a declaration rather than a gate (D184). Dates go through the app's own format tokenizer and renderer (`template/dates.rs`, D30, D180, D181), not chrono format items.
- **No serial finalize pass** (D28): a serial pre-pass resolves counters, answers and the clock, and evaluation stays parallel. **No `Pending` placeholders or metadata prefetch pool**: metadata is read synchronously behind one mtime-keyed cache (P44).
- **The pre-processor narrows whatever the scope gave it** (P20), not the name only. **A folder has no extension** (P100). **An action card has no scope and no pre-processor** (P108).
- **Temp names** are `__renameit-tmp-<n>` from a per-folder cursor (D176), not `name.__ren_<random>`. There is no probe-driven two-phase fallback (P96); a case-only rename is one direct rename except on a case-insensitive Linux volume, where it goes through a temporary sibling (D202).
- **Tag writes** change a copy that `Platform::replace_file` swaps in (D203, D217); they remain `Undoability::None` (P2).
- **Dependencies** that §6 lists and the workspace does not use: `img-parts`, `lopdf` (P69), `unicase`, `unicode-segmentation`, `id3` (D60), `insta`.
- **GUI**: a drop in Browser mode switches to Free Select at once rather than offering to; the status bar has one **Undo** button, not an *Undo ▾* stack; F2 and double-click open the inline rename (no middle-click); the row menu is D125's (plus P111's *Remove from Free Select*), with no *exclude from this run*, *open* or *properties*; there are no per-row resolution actions and no conflict policy (P4, P114); Settings has Batch Replace, Music Styles, Casing Exceptions, Display, File System, Startup, Shell Integration, Appearance and Problem Solver, and no Renaming, Scripting or Advanced page (P71, P88).
- **Quality strategy**: no golden snapshot tests (`insta` is not a dependency — behaviour is pinned by named tests and proptest invariants); the performance gate is `plan_budget`'s absolute budget plus Spike B's frame and tile checks (D165), not a criterion trend gate; and CI type-checks Windows with `clippy --target` rather than `cargo xwin build` (D17).



---

# Part 1: ren-core: Platform-Neutral Rename Engine Architecture

# ren-core — Engine Design

## 1. Workspace layout

*The plan's map. The code's is in [Part 4](#part-4--as-built-14--unreleased).*

```
crates/
├─ ren-core/            # platform-neutral engine, zero GUI deps
│  ├─ model.rs          # FileEntry, VirtualName, RunSet, RenameKind
│  ├─ scope.rs          # Process Name/Ext + pre-processor masking
│  ├─ filter.rs         # include/exclude (wildcard * : ? + regex)
│  ├─ ops/              # one module per operation (see §2)
│  ├─ template/         # tag lexer → AST → compiled Template
│  ├─ tags/             # TagRegistry + per-domain resolvers
│  ├─ counter.rs        # counter state machine (start/step/resets/pad)
│  ├─ preview/          # incremental engine, conflicts, validity
│  ├─ exec/             # planner, renamer, journal, undo
│  ├─ meta/             # metadata providers + cache (see §6)
│  ├─ preset.rs         # serde model: Pipeline as saved preset
│  └─ script/           # Koto embedding + sandbox (§7)
├─ ren-platform/        # traits + naming-rule data
│  ├─ windows/          # attrs, created-date, shell notify (cfg(windows))
│  └─ unix/             # xattr-lite fallback impls
├─ ren-cli/             # headless preset runner (also accepts legacy /p /r /l /f /d /s)
└─ ren-gui/             # separate design doc
```

`ren-core` compiles on all targets; every OS-specific behavior goes through `ren-platform` traits injected at startup.

## 2. Operation model

Two step kinds, because the spec mixes name transforms with file mutations:

```rust
pub enum Step {
    Name(Box<dyn NameTransform>),   // Add, Remove, Replace, Casing, Format,
                                    // Counter, Renumber, ZeroPad, SpaceTrim,
                                    // MoveSection, CsvRename, Editor,
                                    // MusicRename, Script
    Action(Box<dyn SideEffectAction>), // SetDate, SetAttributes,
                                       // MusicTagWrite, TagRemove
}

pub struct StepConfig {            // per-step, matching preset semantics
    pub scope: Scope,              // process name / extension / both
    pub filter: Option<IncludeFilter>,
    pub preproc: Option<PreProcessor>,
}

pub trait NameTransform: Send + Sync {
    /// Pure: subject string in, subject string out. No IO here.
    fn apply(&self, subject: &str, cx: &EvalCx) -> Result<Cow<str>, OpError>;
    fn wants(&self) -> TagNeeds;   // bitflags: AUDIO|EXIF|PDF|MEDIA|HASH|ASK|COUNTER…
}

pub trait SideEffectAction: Send + Sync {
    fn describe(&self, entry: &FileEntry) -> ActionPreview; // badge in preview
    fn execute(&self, entry: &FileEntry, p: &dyn Platform, j: &mut Journal) -> Result<()>;
    fn undoable(&self) -> Undoability; // Full | Journaled | None (tag writes)
}
```

**Scoping is the engine's job, not each op's.** The engine slices the filename by `Scope` (name / ext / full) and then by the `PreProcessor` (skip-n, skip-until, limit-n, cut-at, advanced wildcard/regex section match — applied top-to-bottom, name-only per spec). The op sees only the active section; the engine reassembles. That keeps every op a plain string transform, trivial to test. *(As built: the pre-processor narrows whatever slice the scope gave it, extension included under `both` (P20); a folder row is all stem (P100); an action gets neither (P108).)*

A **Pipeline** is `Vec<(Step, StepConfig)>` — this is also the preset format ~~(serde JSON)~~ **(TOML, D18/D37)** and directly gives the modernized "visible multi-operation pipeline" UI. Filters are evaluated against the *current* (already-transformed) name at each step. ~~with a global "override per-step options" flag~~ — **rejected, D32**: a preset stores exactly what the cards show.

`NameTransform::apply` outputs may contain `/`-separated relative components (the `<\>` tag) — `VirtualName` is `Vec<Component>` + extension, so move-to-subfolder falls out naturally.

## 3. Template / tag system

Grammar: literal text + `<Ident(-arg)*>` tags, case-insensitive ~~except metadata namespaces~~ **everywhere (D62)**. Compiled once per pipeline edit. *(As built: `Node::{Literal, Tag}` behind a private `Resolver` in `template/mod.rs` — no `TagResolver` trait, and `<\>` is a tag rather than a node. `TagNeeds` is a declaration, not a gate: files stay closed because resolution is lazy (D184).)*

```rust
pub enum Node { Lit(String), Tag(TagId, SmallVec<Arg>), PathSep }
pub struct Template { nodes: Vec<Node>, needs: TagNeeds }

pub trait TagResolver: Send + Sync {
    fn domains(&self) -> TagNeeds;
    fn resolve(&self, tag: TagId, args: &[Arg], cx: &EvalCx) -> TagValue;
    // TagValue::Missing drives "only rename if all tags available"
}
```

Registry domains: **Name** (`<Name> <Ext> <FullName> <Left-#> <Mid-#-#> <FLetter*> <Parent-#>`), **Fs** (`<Size*> <Dir*> <Date/CDate/ADate[-fmt]> <NowDate>`), **Audio** (`<Artist>…<ID3-*>`, props `<Bitrate> <Length>`…), **Image** (`<ExifDate> <Exif-*> <Iptc-*> <Width>`…), **Pdf**, **Media** (AVI/MPEG), **Numbers** (`<Counter> <Rnd*> <NumFiles>`), **Misc** (`<Ask-#> <Clipboard> <Crc32> <DetectedExt> <FileMax-#> <PathMax-#> <HtmlTitle> <\>`), **Parts** (`<%1>–<%9>`).

- **Parts**: a mini pattern (`<%1>. <%2> (<%3>) <%4>`) compiled to a greedy left-to-right splitter with literal separators; resolved lazily per file, cached in `EvalCx`. Doubles as the date-from-filename source (`<%4>`=year…`<%9>`=second).
- **`<Ask>`**: core never blocks on UI. `trait Interaction { fn ask(&self, prompt:&AskSpec) -> Option<String>; fn clipboard(&self) -> Option<String>; }` — collected **once per run** before evaluation; preview shows placeholder chips until answered. CLI implements via stdin.
- **Counter**: start, step, zero-pad/auto, reset-each-folder, reset-on-basename-change, reset-at (a ceiling, D183), persistent "running counter". Stateful — see §4, and D28 for how it stayed parallel.
- **Dates**: VB-style format strings (`yyyy-mm-dd`, `dddd m mmmm`, `Hh:Nn:Ss`, named formats) ~~translated by a small mapper onto `chrono` format items~~ — **as built, the app's own tokenizer and renderer in `template/dates.rs`**, with fixed, sortable output (D30, D180, D181); ISO 8601 default.

## 4. Preview pipeline

> **Superseded in two places.** The serial finalize pass below was replaced by a serial *pre*-pass (`RunContext::build`, **D28**), so evaluation is one parallel pass. And there are no `Pending` placeholders or prefetch pool: metadata is read synchronously behind one mtime-keyed cache (**P44**). What survives is the generation counter, the parallel pass and the conflict detection. The worker that runs it is in Part 4.

Target: full recompute of 10k files in <30ms for pure ops; metadata fills in asynchronously.

```rust
pub struct PreviewEngine {
    generation: AtomicU64,          // newest request; an older answer is dropped
    files: Arc<Vec<FileEntry>>,     // immutable snapshot per listing
    cache: MetaCache,               // survives pipeline edits
}
pub struct PreviewRow { new_name: VirtualName, state: RowState /* Ok|Unchanged|Pending|Error(OpError)|Conflict(ConflictKind) */ }
```

Two-phase evaluation per generation:

1. **Parallel pure pass** (rayon): every file runs the compiled pipeline; stateful tags (`Counter`, `Rnd`-unique, `<Ask>` unanswered) emit typed placeholders. Metadata tags hit `MetaCache`; on miss they emit `Pending` and enqueue a prefetch — `Template::needs` ensures we never touch audio/EXIF readers unless the pipeline uses them.
2. **Serial finalize pass** (visible list order — order is user-draggable and drives numbering): counters advance/reset, uniqueness pools draw, placeholders substitute. This pass is O(n) string patching, cheap.

Metadata prefetch completes on a worker pool; each completion bumps only affected rows for the *current* generation (stale generations dropped). Cache key `(path, mtime, size)`; edits to the pipeline never re-read files.

**Conflict detection** (every generation): build `HashMap<FoldedPath, SmallVec<idx>>` of targets, folded per destination-volume rules → `DuplicateTarget` (two sources → one name), `TargetExists` (disk collision with a non-renamed file — checked against the listing snapshot plus a `dir_exists` probe), `Unchanged`, `SwapChain` (informational: planner will resolve). **Validity** per component via `ren-platform::NamingRules`: illegal chars, reserved device names (CON/PRN/AUX/NUL/COM1-9/LPT1-9), trailing dot/space, component/path length (`<FileMax>`/`<PathMax>` tags reuse the same table), empty name. Rules are data, selected by target OS/volume — Linux/macOS later means adding a table, not code.

No two-pass renumber workaround is needed: the planner handles chains and cycles, so `File 1→2, 2→3, 3→4` previews and executes cleanly in one run.

## 5. Execution engine + undo

```rust
pub struct Plan { ops: Vec<PlannedOp> }        // ordered, conflict-free
pub enum PlannedOp {
    MkDir(PathBuf),
    Rename { from: PathBuf, to: PathBuf, temp: Option<PathBuf> },
    Action { file_idx: usize, step_idx: usize },
}
```

Planner: nodes = renames; edge A→B if A's target equals B's current source (folded). Topological order executes chain tails first; **cycles** (A↔B swaps) are broken by renaming one member to ~~`name.__ren_<128-bit random>`~~ **`__renameit-tmp-<n>`, from a per-folder cursor (D176)** first, restoring last. Case-only renames on case-insensitive volumes execute as a single direct rename (NTFS and APFS allow it; **on Linux the platform goes through a temporary sibling, D202**); the folded-map logic treats them as non-conflicting self-edges. ~~Full two-phase (everything→temp→final) is a fallback used only when the destination volume's case behavior can't be probed.~~ **Not built: case is folded from the platform's static naming rules, and the probe is not called (P96).**

**Journal**: write-ahead JSONL in the app data dir; entry per op `{txn, seq, op, done}` with an fsync before each destructive op (one per op, D168; the wider window is P99). The run itself, and undo, happen on a worker thread with a progress line and a Cancel (D169). Records old **and** new names, plus prior dates/attributes for `SetDate`/`SetAttributes`. Undo = replay reverse-order with pre-flight staleness check (current name/mtime must match journal; mismatches reported, rest offered). Crash recovery: on startup an unclosed txn offers rollback of completed steps. Tag writes/removals are `Undoability::None` (P2) unless the "sidecar backup" option (question below) is adopted; as built they change a copy of the file that `Platform::replace_file` swaps in whole, so an interrupted write never leaves a half-written file (D203, D217). Undo and rollback are one reverse walk over every announced op (D174). Simulation mode = execute planner, skip syscalls, render the log.

`trait Platform` (in ren-platform): `set_times(created?, modified?, accessed?)` (created: `SetFileTime` on Windows, unsupported-error on Linux, `setattrlist` later on macOS), `get/set_attributes(RHSA)` (Unix: readonly bit only), `naming_rules(volume)`, `case_sensitivity(path)`, `notify_shell(path)`.

## 6. Metadata providers (license-audited)

| Domain | Crate | License |
|---|---|---|
| Audio tags+props (MP3/Ogg/Opus/FLAC/Speex/MPC/WavPack/MP4/AIFF/APE) | `lofty` | MIT OR Apache-2.0 |
| ~~ID3 fine control (write v1-only/v2.3/v2.4, update-mode)~~ | ~~`id3`~~ | — **dropped, D60**: `lofty` covers the dependency question. See **D90** for what was *not* delivered — we always write v2.4, and the v1-only/v2.3 choices became a fixed policy (D70). |
| EXIF read (JPEG/TIFF/HEIF/PNG/WebP) | `kamadak-exif` | BSD-2-Clause |
| ~~EXIF/IPTC strip (tag stripping)~~ | ~~`img-parts`~~ | — **not a dependency**: nothing strips Exif. |
| ~~PDF pages/info~~ | ~~`lopdf`~~ | — **not a dependency**: the PDF tags are waived (P69). |
| `<DetectedExt>` | `infer` | MIT |
| `<Crc32>` | `crc32fast` | MIT OR Apache-2.0 |
| Regex (needs lookaround + backrefs) | `fancy-regex` | MIT |
| CSV | `csv` | Unlicense OR MIT |
| Core plumbing | `rayon`, `serde`, `chrono`, ~~`unicase`~~, `walkdir`, `filetime`, `thiserror`, `windows-sys`, `libc` (Unix, D201) | all MIT and/or Apache-2.0/Unlicense |

Explicitly **avoided**: TagLib (LGPL), FreeImage (FIPL/GPL), symphonia (MPL-2.0, unnecessary — lofty supplies bitrate/length/channels). ~~WMA/ASF and AVI/MPEG headers have no good permissive crate: ship tiny in-house read-only parsers in `meta/riff.rs` / `meta/asf.rs` (both are simple chunked formats; ~300 lines each) for `<Width>/<Height>/<Fps>/<Codec>` and WMA tag *reads*.~~ — **half superseded (M6): `meta/asf.rs` was not written and will not be. D55 drops WMA/ASF and TTA, reversing P3.** The AVI/RIFF half stands and belongs to M8. ExifTool stays an optional user-installed external process (`Exif-*` passthrough) — process invocation creates no license coupling.

## 7. Scripting: Koto — and the truth about .frs

Legacy `.frs` files are **VBScript executed by a Windows-only COM scripting host**. Supporting them therefore means implementing VBScript — months of work, and inherently Windows-flavored. Rejected.

**This section recommended Rhai, and M7 reversed it (D92).** The recommendation is kept below with what was wrong marked, because the *reasoning* still applies to the engine we chose — and one of its premises turned out to be false in a way worth remembering.

~~**Recommendation: Rhai** (MIT OR Apache-2.0)~~ — **wrong on licensing.** `rhai` itself is MIT OR Apache-2.0, but it depends *unconditionally* on `smartstring`, which is `MPL-2.0+` and outside D2's allowlist, so `cargo deny check` fails the moment it is added. The choice was between an exception on the exact line D2 draws and a different engine; the user chose the engine.

**Koto** (MIT) instead, with its whole dependency tree already inside the allowlist. It is pure Rust, so the no-C-toolchain argument above survives intact. Two of the other premises did not:

- *"its engine is `Send+Sync` and cheaply cloneable so scripts run inside the parallel preview pass"* — Koto is `Send + Sync` under its `arc` feature, but **scripts do not run in the parallel pass at all**, and would not have under Rhai either. A script's globals stay static for the whole rename session, and three of the nine shipped scripts count or accumulate across files. That is inherently sequential, and D94 makes a scripted pipeline evaluate serially in list order. The engine was never the constraint.
- *"first-class sandboxing … no FS access unless registered"* — true of Rhai, **false of Koto**, and it inverts the work. Koto's prelude ships `io` (open, create, remove_file) and `os` (`command`, clocks) by default, so hardening is *removal* rather than absence. D93 has the list; the point is that removal fails silently, so each removal is pinned by its own test.

The scripting surface: an `fr` object (`fr.filename`, `fr.full_filename`, `fr.path`, `fr.args`, `fr.preview`, `fr.disk_name`, `fr.num_items`, `fr.item_order`, `fr.browser_path`, `fr.format_tags("<size>")`, `fr.all_filenames()`), an `init()/rename()/done()` lifecycle — with `init` being the file's top level, since Koto runs it — and a `# description:` header. Three members stand in for what unrestricted filesystem access would otherwise give (`fr.contents()`, `fr.args_file()`, `fr.seed`, D100). Nine worked examples ship as `.koto`; `docs/MIGRATION-legacy-scripts.md` is the VBScript→Koto page. No COM bridge, then or now.

## 8. Remaining spec items → homes

Batch Replace = a `Replace` op vector. Filename Editor = `Editor` transform holding `Vec<String>` by row. Casing exceptions/preserve rules = data file consumed by `ops::casing` (Unicode-aware ~~via `unicode-segmentation`~~ through std's `char` case mapping; no segmentation crate is used). Renumber decimals/minus/zero-pad-keep-length = `ops::renumber` with `rust_decimal` (MIT). `<HtmlTitle>` = bounded read + regex, no HTML crate needed. Running-counter persistence + `<Ask>` memory = `preset.rs` sidecar state.


## Key risks

- fancy-regex uses backtracking: user patterns can be exponential and preview runs them per keystroke per file — must wrap evaluation in a time/step budget and fall back to 'pattern too slow' row errors
- Stateful tags (Counter with folder/basename resets, unique Rnd) force a serial finalize pass; validate the two-phase preview holds the <50ms feel at 50k-100k files before freezing the design *(retired: D28's pre-pass keeps evaluation parallel)*
- lofty gaps: no WMA/ASF write, TTA support unverified, ID3v2.2 write absent — the in-house ASF/RIFF parsers and the id3-crate fallback need early spikes
- Volume-behavior detection (case sensitivity, exFAT vs NTFS rules, 260-char vs long-path policy) is heuristic; a wrong probe can produce false conflict reports or, worse, planner-approved renames that fail mid-transaction — the journal must make every partial failure cleanly rollback-able
- Running `.frs` files verbatim would need a Windows-only scripting host; everyone gets Koto instead — this has to be communicated as a scoped compatibility promise
- Setting created-date is impossible on Linux (no btime write API) — the SetDate op must degrade explicitly per platform rather than silently no-op
- Undo cannot protect tag-write/tag-remove operations without sidecar backups; if backups are added, disk usage and cleanup policy need design


---

# Part 2: GUI framework selection (egui/eframe) and UX design

# GUI framework & UX proposal

## 1. Framework evaluation

**Slint — disqualified, confirmed.** Slint is triple-licensed: GPLv3, a proprietary "Royalty-Free Desktop/Mobile/Web" license (attribution-required, not OSI, not sublicensable), or paid commercial. None is compatible with releasing *our* code under MIT such that downstream users inherit MIT freedoms. GPLv3 would infect the binary; the royalty-free license forbids the recipients of our source from relicensing. Out.

**Tauri v2 (MIT/Apache-2.0) — capable but rejected.** The UI would be HTML/TS in WebView2. Pros: best styling/accessibility, mature JS table virtualization. Cons that kill it here: (a) **WebView2 runtime dependency** — preinstalled on Win 11, but Win 10 LTSC/N and locked-down machines may lack it; the fix is either an online bootstrapper (violates "standalone binary") or bundling the ~180 MB Fixed Version runtime; (b) two-language codebase and an IPC boundary through which 10k-row preview updates must be serialized — workable, but it complicates the "instant preview" path and the platform-neutral-core architecture; (c) on Linux it drags in webkit2gtk, historically the worst part of Tauri's cross-platform story. A file renamer is a data-dense desktop tool, not a web app.

**Dioxus (MIT/Apache-2.0) — rejected.** Desktop renderer is also a system-webview (same WebView2 problem); the native renderer (Blitz) is not production-ready. Less mature packaging/ecosystem than Tauri with the same core drawback.

**iced (MIT) — credible runner-up, rejected.** Elm architecture is pleasant and it produces attractive UIs (Halloy, Sniffnet, COSMIC's fork). But for our specific needs: no first-class virtualized, resizable-column table widget (community widgets are immature); accessibility (AccessKit) integration still partial; significant breaking API churn between releases with thin docs; no docking story. Building the table + inline-diff + inline-rename widget ourselves in iced is more work than in egui.

**egui/eframe (MIT OR Apache-2.0) — RECOMMENDED.** Reasons, against the stated criteria:

- **Table virtualization:** `egui_extras::TableBuilder` and `egui_table` (both MIT/Apache, the latter built by rerun.io) render only visible rows; rerun proves egui handles million-row virtualized tables. 10k files with a live preview column is comfortably in budget.
- **Instant preview:** immediate mode fits this perfectly — the visible rows re-render every frame from a preview cache; there is no retained-widget invalidation dance. Recompute happens in the core crate off-thread (see §3), so typing in "Find" re-paints previews within one frame.
- **Drag & drop from Explorer:** winit delivers `HoveredFile`/`DroppedFile` (WM_DROPFILES) events; eframe exposes them via `raw_input.dropped_files`. Proven.
- **Native file dialogs:** `rfd` (MIT) — native Win32/GTK/NSPanel dialogs.
- **Thumbnails grid:** decode via `image` (MIT/Apache) on worker threads, upload as textures, LRU-evict. Built in M8, and closer to this line than most of these predictions turned out to be — though "trivial" was optimistic: see D133–D142 for the ten decisions it actually took, of which the sharpest is that `ImageReader::limits` protects PNG and nothing else (D137). **The Windows Shell source is waived — P76.** It would need `IShellItemImageFactory`, which means hand-written COM or the much larger `windows` crate (the wall D132 hit for `IShellLinkW`), a code path that exists on one platform and so is tested on one platform, and an app showing different pictures on two machines with the same files. Decoding is `image` alone — six formats (D122), and anything else gets a named placeholder rather than a blank square. A FIPL/GPL decoder such as FreeImage is barred by D2.
- **Docking/tabs:** `egui_dock` (MIT) if we want detachable panels; our default layout doesn't require docking.
- **Dark mode:** built-in light/dark visuals + `dark-light` (MIT) for OS preference detection.
- **Accessibility:** egui has the most mature AccessKit (MIT/Apache) integration of the native-Rust options — real UIA tree on Windows.
- **Binary size:** single self-contained exe, ~8–15 MB with the `glow` backend, no runtime deps. Best-in-class for the "standalone binary" constraint.
- **Maturity/cross-platform:** most-used pure-Rust GUI; identical rendering on Windows/Linux/macOS, which de-risks the later ports completely.
- **Windows polish:** per-monitor DPI via winit works; IME works and has had sustained fixes, though CJK polish trails native apps (accepted risk, see risks).

Honest weaknesses: non-native look (acceptable — the brief says modernized, not native clone); programmatic styling; egui's minor-version breaking changes (pin versions, upgrade deliberately).

**Dependency license audit (GUI side):** eframe/egui, egui_extras, egui_table, accesskit, wgpu — MIT OR Apache-2.0; winit — Apache-2.0; egui_dock, rfd, dark-light — MIT; image — MIT/Apache-2.0; serde/ron, directories — MIT/Apache-2.0; windows/windows-sys — MIT OR Apache-2.0; egui-phosphor icons — MIT; UI font Inter — SIL OFL 1.1 (embedding a font is fine in an MIT app; the OFL covers the font file, not our code). Add `cargo-deny` with `[licenses] allow = ["MIT","Apache-2.0","BSD-2-Clause","BSD-3-Clause","Zlib","ISC","OFL-1.1","Unicode-3.0"]` to CI so this stays enforced.

## 2. Crate & module structure (GUI side)

*The plan's map, corrected in places; the full as-built map is in [Part 4](#part-4--as-built-14--unreleased).*

`ren-gui` (lib + bin, depends on `ren-core`):

- `app.rs` — eframe `App`, top-level layout, theme, command routing
- `viewmodel/` — `session.rs` (file set, selection, sort), `preview.rs` (the preview worker), `listing.rs` (the listing worker, D167), `apply.rs` (the run worker, D169), `history.rs` (run, undo, recovery), `stack.rs` (the card stack), `columns.rs` (the column model)
- `panels/` — as built: `source_bar.rs`, `operation.rs`, `file_table.rs`, `grid.rs`, `tile.rs`, `rows.rs`, `status_bar.rs`, `visual_assist.rs`, plus the windows below. *(This bullet and the two under it were written before M2 and named modules that never existed — `pipeline.rs`, `thumb_grid.rs`, `inspector.rs`; D8 killed the inspector. Corrected in M8 rather than left as a map of a building nobody put up.)*
- `editors/` — one module per operation, named for it (`replace`, `casing`, `add_remove`, …, `set_date`, `script`), plus `assist.rs`, Visual Assist's widget-free half
- There is no `dialogs/` **directory**: `dialogs.rs` is the `FileDialogs` trait, and the windows are `panels/{settings,presets,confirm,ask,about,run_settings}.rs`. **Visual Assist is not a dialog at all** — it is an inline strip inside the card whose field it fills (**D143**).
- `widgets/` — `diff_text.rs` (before/after span highlighting), `tag_field.rs` (a text field with the `<tag>` menu beside it — there is no `tag_picker.rs`), `filter_editor.rs`, `preproc_editor.rs`, `rule_table.rs`, `string_list.rs`, `number.rs`, `row_keys.rs`, `tri_checkbox.rs`, `form.rs`, `icons.rs`. There is no `virtual_table.rs`: `egui_table` virtualizes the list and `panels/grid.rs` the tiles.

Preview flow *(as built: Part 4's worker table)*: any edit bumps a `u64` generation, sends `(generation, PipelineSpec)` over a channel to a core worker; core computes `Vec<PreviewRow>` (new name + per-row status: Changed/Unchanged/Conflict/Invalid/Error) using rayon; results arriving with a stale generation are dropped. UI paints the latest cache; visible-row diff spans are computed lazily per row. This keeps 10k files "instantaneous" and never blocks paint.

## 3. Modernized UI design

**Core idea: the pipeline is first-class.** Rather than hiding multi-operation stacking inside Presets, the left panel *is* an ordered stack of operation cards. One card = the classic single-function mode; presets become nothing more than saved pipelines. This unifies "run one function", "stack functions", and "presets" into a single mental model.

Five groups (General, Music, Numbers, Advanced, Presets) map to: four **categories in the Add-Operation palette** (General: Replace, Set Casing, Add & Remove, Move Section, Space Trimming; Music: Music Rename, Tag Writer, Remove Tags; Numbers: Add Counter, Re-Number, Zero Pad; Advanced: Set Attributes, Set Date & Time, CSV List, Filename Editor, Free Format, Scripting) — and **Presets becomes the Preset drawer** (saved pipelines).

### Screen-by-screen outline

**S1. Main window** — three zones, fully resizable splitters, light/dark/system theme.

- *Top: Source bar.* Segmented control **Browser | Free Select** (replaces the old tabs). Browser: editable breadcrumb path + folder picker + pattern box (`*.mp3`) + toggle chips **Files / Folders / Subfolders** + an **Include filter** chip that opens a popover (contains/wildcard/regex include + exclude, match name/path/extension). Free Select: same bar shows "N files from M folders", an *Add files…* button, and a *Clear* button; drag-and-drop from Explorer adds files in either mode (in Browser mode a drop ~~offers "switch to Free Select with these files"~~ **switches to Free Select at once**; a single folder browses into it). Right side: a **List | Grid** pair, matching Browser | Free Select — thumbnails ship as a view *and* as a column and the user chooses, because which one a single checkbox ought to mean is genuinely ambiguous (D133).
- *Left: Pipeline panel.* Vertical stack of **operation cards**: drag-handle to reorder, checkbox to enable/disable (instant preview reflects it), name, one-line summary ("Replace `_` → ` `"), overflow menu (duplicate, delete, per-op scope). Selecting a card opens its **editor** inline (card expands) — no separate inspector to keep spatial locality. Bottom: **+ Add operation** button → palette (S2). Header: pipeline name, *Save as preset*, *Presets ▾* drawer. Per-op overrides: scope **Name / Extension / Both** and an optional per-op include filter live in each card's "Scope" expander; the global options only set defaults. **Counter** and **Parts** setups appear as two pinned chips at the panel top — "Counter: 1, step 1, pad auto" and "Parts: `<%1> - <%2>`" — each opening its dialog (S5/S6).
- *Right: File table (S3) / thumbnail grid.*
- *Bottom: status + action bar.* Left: "1 284 files, 12 folders • 1 240 will change • 2 conflicts". Right: **Simulate** toggle (a persistent switch that turns the Rename button into "Simulate"), ~~**Undo ▾** (history stack)~~ **Undo** (one button, newest batch first; F4/Ctrl+Z), primary **Rename** button (F5) — disabled with a reason tooltip when conflicts exist or the pipeline is empty.

**S2. Add-operation palette.** Modal popover with search box; operations grouped under General / Music / Numbers / Advanced, each with icon + one-line description. Enter adds and expands the card. Also reachable via Ctrl+K command palette.

**S3. File table.** Virtualized (`egui_table`), sortable, user-configurable columns: ☑ selection, icon, **Name**, **→ New name**, size, dates, attributes, tag columns (artist/title) — defaults: Name + New name. The New-name cell renders **inline diffs**: inserted spans green-tinted; a deleted span struck and red-tinted **only when nothing was inserted in its place (D235)**; unchanged rows dimmed. **Row states:** normal; *unchanged* (dim); *conflict* (red badge — duplicate target, name collides with existing file, invalid characters, path too long) with tooltip explaining ~~and a right-click "resolve: auto-number" action~~ (**no resolution actions, P114**); *warning* (amber — extension changed). A filter chip above the table: All / Changed / Conflicts. **F2 / double-click inline rename** (no middle-click) — built in M8: the **stem is selected** with the caret at the extension boundary (Explorer's behaviour), and Enter renames and advances to the next row **on screen**, found by path because a rename re-sorts. **Drag rows to reorder** (feeds counter order) — the whole row is the drag source, and the order survives the run that used it (**D157**). Arrow keys, Home, End, `Ctrl+A`, and `Enter`/`Backspace` to walk the folder tree (**P87**, **P90**). Right-click menu ~~: rename, add to Free Select, exclude from this run, open, show in Explorer (platform trait), properties~~ — **as built (D125, P111):** Rename…, Show in file manager, Copy to clipboard ▸, and Add to Free Select (in Free Select, Remove from Free Select). "Only rename selected" behavior kept: selecting rows scopes the run, none selected = all (per settings).

**S4. Operation editors** (inline in cards). Each carries its operation's full option set — e.g. Replace: find/replace, wildcards `* : ?`, regex with `$1–$9`, case-sensitive, swap mode, skip/start/count; Casing: name & extension modes (UPPER/lower/Sentence/Title/iNVERT/rANdOm), lowercase-exception words, preserve-caps options, exceptions list editor; Add & Remove with position, backwards counting; etc. Every position/length field gets the modernized **Visual Assist**: instead of a separate window, clicking the ⌖ button enters a mode where you *select a text span directly in any New-name cell* and the fields populate. **Amended in M8, twice.** The span is selected in a read-only field **inside the card**, not in a New-name cell — that column is the pipeline's *output*, while a position field is measured on the operation's *input*, which for a card partway down a stack is neither column on screen (**D144**). And it is **four** call sites rather than every position/length field: Replace's Skip and Max are match counts, Zero Padding's Digits is a width, and the CSV columns are column numbers, so a span picker on those means nothing. Tag-accepting fields have a `<>` button opening the tag picker (all `<tag>`s: name parts, `<counter>`, `<%1>`–`<%9>`, dates, size, parent folder, music tags, EXIF).

**S5. Counter setup dialog.** Start, step, running counter, reset-per-folder / on-base-name-change / reset-at, zero-padding (fixed or auto) — with a live 3-line sample.

**S6. Parts setup dialog.** Pattern field + live preview against the selected file, magic-wand auto-detect from a selection.

**S7. Execute flow.** Rename runs the core's two-phase transactional plan (cycle-safe via temp names). A pre-flight sheet summarizes: N renames, M skipped (filters/unchanged), K conflicts ~~and chosen policy (skip / auto-number, from settings)~~ **— a conflict blocks the run; there is no policy to choose (P4, P114)**. *(As built, the sheet appears only for a run that cannot be undone or that writes a file, D78 and D219.)* Progress bar + streaming log; on completion a toast with "Undo" and a collapsible log drawer. Simulate runs the identical flow, writes nothing, and marks the log "SIMULATION".

**S8. Preset drawer / manager.** Slide-over listing saved pipelines with descriptions; actions: Load (replaces stack), Append, Run directly, rename/delete/duplicate, import/export (**TOML** files per D18 — enables sharing; a legacy preset importer is dropped by D6). "Save as preset" captures the current stack including per-op scopes/filters; an `<Ask>` token in any text field prompts at run time.

**S9. Settings dialog.** Pages: ~~**Renaming**~~ — **there is no Renaming page (P88)**: P4 makes a conflict a hard block rather than a policy, the extension-change warning is an inline per-row badge rather than a setting, and P22 makes selection scope the plan, so all three of the settings this page was promised became behaviours with decisions behind them. **Appearance** (theme, columns, font size, date format), **File system** (Windows page: attribute handling, created-date setting), **Shell Integration** (the Explorer menu: one checkbox that installs the whole cascade, a state line read back from the registry rather than remembered, the preset count, Refresh, and the two facts the menu cannot tell anyone itself — Windows 11's *"Show more options"*, and the 2000-character selection cap; behind the platform trait, and the page says so where there is nothing to install), **Casing exceptions**, **Batch replace list**, ~~**Scripting** (engine: Koto — MIT — replacing WSH/VBScript, cross-platform), **Advanced**~~. **As built there are nine pages** — Batch Replace, Music Styles, Casing Exceptions, Display, File System, Startup, Shell Integration, Appearance, Problem Solver — and no Scripting or Advanced page (P71): the script folder is on the Problem Solver page, and each tweak lives on the page that owns it.

**Accessibility & keyboard:** full AccessKit exposure (in the shipped app since D218); F2 rename, F3 visual assist, F5 rename, Ctrl+Z undo, Ctrl+K palette. The full table is in the README; the rules (exact modifiers, nothing behind a modal) are D220.

## Key risks

- egui's IME/CJK text-input polish on Windows trails native Win32 edit controls; must be validated early with Japanese/Chinese input in the Find/Replace fields, since renaming non-ASCII filenames is a core use case.
- egui look-and-feel is non-native; acceptable under the 'modernized, not clone' mandate, but stakeholders expecting native Windows chrome should sign off on a mockup before build-out.
- egui/eframe makes breaking API changes across minor versions; pin exact versions and budget for deliberate upgrades.
- Screen-reader support via AccessKit is good but not level with browser/UIA-native; if accessibility certification is a hard requirement, re-validate against NVDA early.
- Instant preview at 10k+ files depends on the off-thread generation-counter preview cache described in §2; a naive on-UI-thread recompute would visibly stutter — this is an architectural commitment, not an optimization to defer.
- Windows Explorer drag-and-drop via winit (WM_DROPFILES) must be smoke-tested with long paths (>260 chars), UNC paths, and mixed file/folder drops.
- A FIPL/GPL image decoder and a Windows-only VBScript host are both barred for licensing/portability reasons; the replacements (the `image` crate — the Windows shell half was waived, P76; Koto scripting) mean existing `.frs` scripts do not run.
- Importing a legacy binary preset format is a reverse-engineering task; out of scope (D6).


---

# Part 3: RenameIt delivery roadmap & quality strategy

# Delivery Roadmap & Quality Strategy

## Architecture baseline (assumed by every milestone)

Cargo workspace:

- `renamer-core` — platform-neutral: ops, pipeline, tag/format engine, plan builder, conflict detector, transactional executor, undo journal, preset model + legacy importers. No GUI, no `windows` deps except behind the `platform` trait.
- `renamer-platform` — `trait PlatformFs { set_attributes, set_created, case_insensitive_eq, reveal_in_manager, … }` with `cfg(windows)` impl via `windows` crate (MIT/Apache-2.0) + `filetime` (MIT/Apache-2.0); Unix impl stubbed from day one.
- `renamer-cli` — headless driver over core. Exists primarily as the test harness and automation story (also accepts legacy `/p /l /r /f /d /s /k /x`), ships in every release.
- `renamer-gui` — `eframe`/`egui` (MIT/Apache-2.0): immediate-mode is ideal for "recompute preview every frame" and has built-in dark mode + virtualized lists. (Slint is explicitly rejected: GPL/commercial dual license.)

Key dependency/license audit (enforced in CI by `cargo-deny`): `regex` + `fancy-regex` (MIT/Apache-2.0 — fancy-regex needed because the shipped batch-replace rules use look-around-style patterns and `$1..$9` captures), `walkdir` (MIT/Unlicense), `rayon` (MIT/Apache-2.0), `serde`/`toml`/`serde_json` (MIT/Apache-2.0), `csv` (MIT/Unlicense), `lofty` (MIT/Apache-2.0) for audio tags, `kamadak-exif` (BSD-2) for EXIF, `image` (MIT/Apache-2.0) for thumbnails, `koto` (MIT) for scripting — `rhai` was rejected at M7 over its MPL-2.0 `smartstring` dependency (D92), `tempfile`/`assert_fs` (MIT/Apache-2.0), `proptest` (MIT/Apache-2.0), `insta` (Apache-2.0), `criterion` (MIT/Apache-2.0), `winresource` (MIT). No FreeImage, no taglib, no GPL anywhere.

Core data model sketch (drives all testing):

```rust
struct RenameJob { items: Vec<Item>, pipeline: Vec<OpInstance>, scope: Scope /* name/ext, filters, preprocessor */ }
struct RenamePlan { steps: Vec<PlanStep>, conflicts: Vec<Conflict> } // pure fn of (JobSnapshot)
enum Conflict { DuplicateTarget, TargetExists, Unchanged, InvalidName, CaseOnlyCycle, … }
struct UndoJournal { session: Uuid, entries: Vec<{from, to, pre_meta}> } // persisted per execute
```

Preview = `plan()` rendered in the list; Execute = `apply(plan)` with cycle-breaking via temp names, per-item results, journal write **before** each rename. Undo replays the journal in reverse, refusing entries whose current state no longer matches `to`.

---

## Milestones (each independently shippable & testable)

**M0 — Scaffolding & risk spikes (1–2 weeks).**
Scope: workspace, CI matrix, `cargo-deny` license gate, `PlatformFs` trait with Windows+Unix impls for rename/attributes/dates, plan/execute/undo skeleton with a single hardcoded op, `renamer-cli preview|apply|undo`. Two spikes: (a) `fancy-regex` evaluated against the shipped batch-replace rules and the JScript regex constructs; (b) egui virtual list rendering 10k rows with per-frame recompute benchmark.
Acceptance: CI green on Linux + Windows; CLI renames a tempdir folder and undoes it byte-exactly; both spikes have written verdicts.
Defer: everything user-visible.

**M1 — Core pipeline + General group (the product's spine).**
Scope: full plan/conflict/execute/undo engine (collision detection incl. case-insensitive targets, swap/cycle handling via temp names, "unchanged" suppression); Global options: process name/ext split, include/exclude filter (substring, `* : ?` wildcards, regex), simulation mode; ops: **Replace** (case-sensitivity, swap mode, skip/start/count, regex with `$1-$9`), **Set Casing** (upper/lower/sentence/title/invert/random, lowercase-exceptions list, preserve-uppercase/mixed, a fixed-case exceptions list), **Add & Remove** (insert/overwrite/remove at pos/from end), **Move Section**, **Space Trimming**. CLI exposes all of it.
Acceptance: each operation's behaviour encoded as unit tests named for the sentence they pin; property tests pass (below); 10k-file preview < 50 ms per keystroke on reference hardware; undo restores an adversarial tempdir (swaps, chains, unicode, case-only renames) exactly.
Defer: GUI, tags, counters.

**M2 — GUI shell v1.**
Scope: egui app: folder browser + Free Select (drag-drop), virtualized file list with live preview column, sortable columns, global-options panel, one-op-at-a-time function panel for the M1 ops, Rename/Undo buttons, log window, status bar, F2 inline manual rename with smart extension-aware cursor + jump-to-next, dark/light mode, resizable everything.
Acceptance: a user can perform every M1 operation without the CLI; preview visibly updates while typing on 5k files; window state persists.
Defer: pipeline editor UI, thumbnails, preset manager.

**M3 — Numbers group + format tags core.**
Scope: `<counter>` engine (start, step, zero-pad incl. auto, running counter persistence, reset-each-folder / on-base-name-change / reset-at), **Add Counter**, **Re-Number**, **Zero Pad Numbers**; format-tag engine v1: `<Name> <Ext> <FullName> <Size*> <Parent[-#]> <Left/Right/Mid/MidRev-#> <FLetter*> <Date*/Time*>`, Parts `<%1>–<%9>` with separator-pattern setup + auto-detect, "only rename if all tags available".
Acceptance: tag engine has a table-driven test per tag in `docs/tags.md`; counter reset semantics pinned by tests; renumber round-trips a shuffled `01..NN` set.
Defer: EXIF/music tags, folder-content "peek".

**M4 — Pipeline UI + Presets (first modernization payoff).**
Scope: the visible multi-operation pipeline (ordered op list, enable/disable, drag-reorder, per-op include filters, recall-into-editor) — this *is* the preset editor; preset save/load in a TOML format; ~~legacy importers for binary preset and batch-replace files~~ — **dropped, D6**; Batch Replace op; Pre-Processor (section slicing before op, top-to-bottom rules); ~~"override stored global options" toggle~~ (**rejected, D32**); CLI `--preset`.
Acceptance: the shipped default presets load and produce their described outcomes on fixture trees; ~~golden snapshots of importer output~~ (no importer, D6); a 3-op pipeline previews as one composed result and executes atomically with single-journal undo.
Defer: shell extension hookup of presets.

**M5 — Advanced group.**
Scope: **Set Attributes** (tri-state R/H/S/A via `PlatformFs`; Unix maps read-only/hidden-dotfile or no-ops with capability reporting), **Set Date & Time** (all sources: manual, created/accessed/modified, now, add/subtract interval, from filename via parts `<%4>–<%9>`), **CSV List Rename** (standard CSV rules except newlines within data, which reject the file (D49); UTF-8 or CP1252 (D50); tags in the new-name column, with any path stripped (D48)), **Filename Editor** (line-per-file editor; its lines are stored in the preset (D39) rather than in a side file), **Free Format** (pure tag-pattern rename).
Acceptance: attribute/date changes verified by integration tests on the Windows runner (created-date is Windows-only — assert capability error on Linux); a CSV fixture renames exactly as its rows say.
Defer: EXIF date source until M6 if `kamadak-exif` integration slips.

**M6 — Music + EXIF.**
Scope: read tags via `lofty` (MP3/ID3v1+v2, Vorbis, FLAC, Speex, MPC, WavPack, MP4/M4A…); **Music Renaming** styles + custom patterns, "only if all tags available", folder "peek" (first audio/image file inside folder supplies tags); **Music Tagger** (write, tri-state fields, parts→tags quick setup, ID3 version choice where lofty allows) and **Remove Tags** — both clearly marked *no undo*, gated behind confirmation; EXIF tags incl. smart `<ExifDate>/<ExifTime>`; optional external-`exiftool` bridge (subprocess only — keeps GPL/Artistic code out of our binary).
Acceptance: golden tests over a checked-in corpus of tiny tagged fixture files per format; tagger write→re-read round-trip; WMA/TTA support gap either closed or documented in release notes.
Defer: thumbnails.

**M7 — Scripting + automation.**
Scope: `koto` scripting with a façade over the legacy scripting API (`filename, full_filename, path, args, preview, disk_name, num_items, item_order, browser_path, format_tags(), get_all_filenames()`); nine worked example scripts; migration doc (VBScript `.frs` cannot run — flagged loudly); the full legacy switch set + `--simulate`.
Acceptance: each ported script reproduces its legacy behaviour on a fixture tree; scripted preview vs execute consistency property holds.
Defer: embedding Windows Script Host.

**M8 — 1.0: polish, packaging, integration.**
Scope: thumbnails (`image`), high-DPI, the full keyboard shortcut set, portable mode (settings beside exe), single-exe packaging with icon + version info (`winresource`, `+crt-static`), Explorer integration as a **static HKCU cascade** — `ExtendedSubCommandsKey` on `AllFilesystemObjects`, `Drive` and `Directory\Background`, three fixed items plus one per preset, all launching `ren-gui` with a command line (**D150**). An in-process COM handler is the alternative, and the GPL-3.0 options are barred by **D2** — but such a handler's config is plain command lines, so there is no COM tier worth deferring to. What that costs, and what the Settings page says out loud, is Windows 11's *"Show more options"* and a 2000-character cap on the selection (**P83**); the Send To half is waived by **D132**. The layout is data in `shell/plan.rs` with no `cfg`, so the half that can be silently wrong is ordinary Linux unit tests; Linux build promoted from "CI artifact" to alpha.
Acceptance: the behaviour sweep closed; zip + exe artifacts published by CI tag build.

---

## Quality strategy

**Behaviour-as-tests.** Each operation's documented behaviour becomes a test whose *name* is the sentence it pins, so a failure reads as a broken promise rather than a broken assertion.

**Property tests (proptest), run on core from M1:**
- *No-collision invariant*: for arbitrary file sets + arbitrary pipelines, `plan()` never emits two steps with the same normalized target, and `apply` on a real tempdir never loses a file (count + content-hash preserved).
- *Undo exactness*: `undo(apply(plan))` restores names, and (for attribute/date ops) metadata, exactly.
- *Preview truth*: the previewed name equals the on-disk name after execute, for every item, including multi-op pipelines and counters.
- *Case-insensitivity safety*: plans containing case-only renames and A↔B swaps succeed via temp-name two-phase on a case-insensitive FS (exercised for real on the Windows runner).

~~**Golden tests (insta).**~~ **As built: named behaviour tests, not snapshots** — `insta` is not a dependency. Fixtures: the shipped presets, the batch-replace rules, the casing exception lists and the nine example scripts, each exercised end to end on synthetic trees (`tests/engine.rs`, `tests/ported_scripts.rs`, and `ren-cli`'s test that loads every shipped example).

**Integration tests.** `tempfile`/`assert_fs` trees; hostile cases: locked file mid-batch (Windows), read-only targets, >260-char paths, unicode/emoji, reserved names (`CON`, trailing dots/spaces), partial-failure rollback and journal recovery after a simulated crash.

**Benchmarks.** `criterion` on plan generation for 1k/10k/50k items; ~~CI trend gate (fail on >2x regression)~~ **as built, the gate is `examples/plan_budget.rs`'s absolute p95 budget, plus Spike B's frame and tile-count checks (D165); criterion reports and asserts nothing**. Preview path: debounced input → `rayon` parallel plan → diff-only UI update.

**CI matrix (GitHub Actions).**
- `ubuntu-latest`: fmt, clippy `-D warnings`, `cargo-deny` (licenses + advisories), core tests, ~~`cargo xwin build --target x86_64-pc-windows-msvc` as a fast cross-compile smoke~~ **`cargo clippy --target x86_64-pc-windows-msvc` (D17), run before the tests**.
- `windows-latest`: full test suite (the only place attribute/created-date/case-insensitive tests really run), GUI build, packaging step producing `RenameIt.exe` (icon, `FileVersion`, static CRT) + portable zip.
- Tag push → release workflow attaching artifacts; `macos-latest` added when the Unix port starts.

## Sequencing rationale / de-risking

Riskiest items are pulled forward: regex flavor and preview performance (M0 spikes), transactional rename + undo + case-insensitive semantics (M1, everything else stands on it), ~~legacy preset import (M4)~~ (dropped, D6), tag-library coverage (M6 fixture corpus built before UI work), shell extension deliberately last and tiered because a Rust COM extension is the least-certain component and has a cheap tier-1 substitute.


## Key risks

- Regex flavour mismatch: the batch-replace rules rely on capture groups and byte-range character classes, so fancy-regex must be validated against every shipped rule in M0
- Case-insensitive NTFS semantics (case-only renames, A<->B swaps, reserved names, long paths) are where renamers eat data; must be solved and property-tested in M1, and can only be truly exercised on the Windows CI runner
- Instant preview at 10k+ files with per-keystroke recompute is a hard perf budget; egui + rayon spike in M0 must confirm <50 ms or the UI architecture changes early
- Audio tag coverage gaps: lofty may not cover WMA/TTA; and tag writes have no undo — needs explicit UX gating
- A Koto façade plus worked examples is a compatibility break for anyone with VBScript `.frs` files, and requires user expectation-setting
- Windows Explorer context-menu extension (COM/sparse package) is high-effort, low-certainty; mitigated by tiered plan with a simple launcher-based tier 1 for 1.0
- Windows-only behaviour (attributes, created dates, case-insensitive renames) can only be confirmed on the Windows runner or by hand; a late surprise there could reopen a 'done' milestone

---

# Part 4 — as built (1.4 / unreleased)

*Written 2026-09-25 from the code at the second audit, and checked against it. Parts 1–3 are the plan; this is the program. Where they disagree, this part describes the code and `docs/DECISIONS.md` says why.*

## 4.1 Threads

The GUI is one UI thread and a handful of workers. Every worker has the same shape: the UI sends a request over an `mpsc` channel and never blocks; the worker answers over a second channel and calls an injected `repaint` closure once per answer (**D135** — the views never ask for a repaint themselves, which is what lets a headless test settle, **D26**); the UI polls every frame and drops any answer older than the newest it asked for.

| Thread | Owns | Request → answer | Coalescing | Cancellation | Panic | Decisions |
|---|---|---|---|---|---|---|
| UI (eframe's main thread) | egui, `RenameItApp`, `Session`, `History`, the card stack | — | — | — | Unguarded: a panic here is a crash, so nothing fallible runs here | D26, D135 |
| `renameit-preview` (`viewmodel/preview.rs`) | `ren_core::plan` over the scoped rows | `(generation, entries, pipeline, scoped rows)` → `(generation, Plan or panic message, scoped rows, elapsed)` | Drains the queue to the newest request before planning | None mid-plan; a superseded plan is finished and dropped | `catch_unwind`: a *failed generation* — answered, no plan, the message in the status bar, Rename disabled | P35, D79, D165 |
| `renameit-listing` (`viewmodel/listing.rs`) | The folder walk or the Free Select stat | `(generation, Browser{dir, options} \| FreeSelect{paths})` → `(generation, entries + problems, or an error)` | Drains the queue to the newest request | A shared atomic generation is polled between entries; a superseded walk abandons itself and sends nothing. Only the answer to the newest request is installed | `catch_unwind`: an ordinary failed listing | D167, D224, P63 |
| `renameit-apply` (`viewmodel/apply.rs`) | `exec::apply`, `exec::undo_transaction`, `exec::rollback` | `Job::{Run, Undo, Rollback}` → `Outcome::{Run, Undo, Rollback, Panicked}` | None: **one job at a time**, and a second is refused while one is out | A run polls a cancel flag between ops (a cancelled run is a shorter, committed run); undo and rollback cannot be cancelled, because they must finish to be exact. Progress is an atomic counter the status line reads | `catch_unwind`: `Outcome::Panicked` naming the job kind; the app relists and re-scans the journals so the banner offers Roll back | D168, D169, D222, D223 |
| `renameit-thumbs-0` … `-3` (`thumbs/worker.rs`) | Thumbnail decodes | One job per visible key → `(generation, key, pixels or a named refusal)` | None — a *set*: the view sends everything visible and not held | A job older than the newest generation is discarded unread, for the price of an atomic load | `catch_unwind` around each decode (`meta::thumb`); a panic is a cached refusal | D134, D136, P74, P75 |
| `renameit-mounts` (`ren-platform/src/unix.rs`, Linux only) | Re-reads `/proc/self/mounts` every two seconds | — (lookups are one atomic load of the current table) | — | Never stops; started on the first lookup, only where the file exists | A failed read keeps the last table | D162, D204 |
| rayon's global pool | The parallel passes inside `plan()`: evaluating every row, building plan rows | Called from `renameit-preview` (and from `ren-cli`'s main thread) | — | — | Inside the preview worker's `catch_unwind` | D94, D165 |
| `script compile` (`script/engine.rs`) | Compiling one Koto script on a stack sized to it, then joined | — | — | — | A compiler panic is a compile error | D189 |

The thumbnail pool is `(available_parallelism / 2).clamp(1, 4)` threads and deliberately **not** rayon: a decode is one long task, and on the global pool it would starve the preview's short ones (**D134**). A pipeline with a script evaluates serially in list order on the preview worker (**D94**).

### How the pieces agree

- **A run never uses a stale plan.** Rename, F5 and a preset's Run wait for the preview to catch up (**P35**); consent to an irreversible run or a script write is bound to the generation it was given for (**D79**, **D219**); and a run is refused, not deferred, while a relist is pending, because the plan in hand describes names the last run replaced (**D223**).
- **A run's report is followed, not re-derived.** After a run, F2, an undo or a rollback, `Session::refresh_after_run` remaps the Free Select set and a hand-set order through the report's `(from, to)` pairs (**D224**), and the thumbnail cache is rekeyed rather than cleared (**D139**). The relist that follows is asked for in the same frame that landed the job, before any older walk can be installed.
- **The order within a frame** (`RenameItApp::ui`): the job outcome, then the relist request, then the listing, then the preview, then drops and keys — so F2 and F5 act on the rows and the plan the frame draws — then thumbnails, then drawing.
- **The guard sits where a run starts.** `run()` asks `ren_platform::guarded::first_guarded` before anything else, with the same `Platform` the run will use (**D200**).
- **Closing waits.** A close request while a job is out is cancelled and retried when the job lands (**D222**).
- **`ren-cli` has none of this**: it lists, plans and applies on its main thread, with rayon inside `plan()` as above.

## 4.2 Module map

### `ren-core` — the engine (no GUI, no OS behaviour; D3, D240)

| Module | What it is |
|---|---|
| `lib.rs` | The crate's surface: `plan`, `apply`, `Pipeline`, `Plan`, `PresetStore` and the types around them. |
| `model.rs` | `FileEntry`, and how a name splits into stem and extension (P12; a folder is all stem, P100). |
| `listing.rs` | Building the list: the Browser walk with its chips and pattern, Free Select paths, the visibility switches and their pruning (P63, D126, D191). |
| `pipeline.rs` | The ordered steps and `evaluate_all` — rayon over rows, serially when a script is present (D94); what a card is handed (`subject_at`, D144). |
| `run.rs` | `RunContext`, the serial pre-pass: counters, `<Ask>` answers, the clipboard, the run's clock (D28). |
| `plan.rs` | `plan()`: new names → validated, conflict-checked, ordered `PlannedOp`s, with cycle breaking, folder creation, script-write confinement (`blockers`) and one path key for every comparison (P4, P6, D31, D171, D176). |
| `job.rs` | Job files and the shared document schema (D18, D33). |
| `preset.rs` | `PresetStore`: one TOML file per preset, seeding, rename and import without overwriting (D37, P84, D192). |
| `effect.rs` | What an action decided to do to one file, and `Undoability` (P2, D52). |
| `counter.rs`, `parts.rs` | Counter Setup and Setup Parts, the two run-wide settings. |
| `filter.rs`, `matcher.rs`, `wildcard.rs`, `regex_flavor.rs`, `preproc.rs` | The include filter (P19, D178); plain / wildcard / regex text; the wildcard language; the `fancy-regex` wrapper with its step budget (P7); the pre-processor (P20). |
| `csv_table.rs` | Reading a CSV into an old → new lookup (D48–D50, D195). |
| `datetime.rs` | Set Date's arithmetic, and the checked wall-clock conversion (D180). |
| `cache.rs` | `Cached`, an operation's compiled artefacts (D21, D185). |
| `text.rs` | Plurals. |
| `test_platform.rs` | Test-only: a platform that counts calls and can be told to fail. |
| `ops/` | `kind.rs` — `OpKind`, the one enum over the eighteen operations (D23), with `refresh` (D185); one module per operation; `numbers.rs`, the number finder Re-Number and Zero Padding share. |
| `template/` | `mod.rs` compiles and renders (`Node::{Literal, Tag}`, a private `Resolver`); `lexer.rs`, `tag.rs` (every tag, its spelling and its argument shape, D29, D182); `dates.rs`, the date format language (D30, D181); `content.rs`, `<Crc32>`/`<DetectedExt>` (D179); `exif_names.rs`, the Exif names `<Exif-…>` accepts. |
| `meta/` | Reading what is inside files, behind one cache (`cache.rs`, P44, D193): `audio.rs`, `exif.rs`, `image.rs`, `thumb.rs`, `folder.rs` (the peek and the `<Dir…>` tags), `html.rs`, `lyrics3.rs`, `names.rs` (the ID3 names, D57); `write.rs`, the one writer — through a copy and `replace_file` (D188, D217); `testing/`, fixture writers behind the `testing` feature (D197). |
| `script/` | `engine.rs`, the hardened Koto engine and its gates (D93, D106, D189); `facade.rs`, `fr` and the session (D100, D190); `header.rs`; `store.rs`, the script folder (D105, D198). |
| `exec/` | `apply.rs`, the executor (write-ahead, D168; per-folder file-manager notices, P110); `journal.rs`, the JSONL journal (D44, D160, D175); `undo.rs`, the reverse walk (D174); `recover.rs`, unfinished transactions and rollback; `mod.rs`, options and errors. |
| `examples/plan_budget.rs`, `benches/plan.rs` | The gated 10k-row planning budget, and the same with criterion's statistics (D165). |
| `tests/` | `engine.rs` (end to end), `properties.rs` (the proptest invariants, including a crash at every change), `tags.rs`, `formats.rs`, `script_writes.rs`, `ported_scripts.rs`, `non_utf8_names.rs`, `spike_regex.rs`. |

### `ren-platform` — everything OS-specific (D3)

| Module | What it is |
|---|---|
| `lib.rs` | `trait Platform` (rename, `replace_file`, times, attributes, naming rules, case probing, reveal, shell notify), `host()`, `app_data_dir` and portable mode (D16, D131), the raw command line and `split_verbatim` (D211). |
| `windows.rs` | `MoveFileExW`, `ReplaceFileW`, `SetFileTime`, attributes (D215), long paths. |
| `unix.rs` | `renameat2` with its fallbacks (D201), the case-only temp step (D202), `lutimes` (D205), the mount table and its watcher (D162, D204). |
| `naming.rs` | The naming rules as data: the Windows and POSIX tables (D213). |
| `guarded.rs` | The system folders and `first_guarded`, the one guard both front ends call (D127, D200). |
| `visibility.rs` | Hidden, system and write-protected, as each OS means them (D126). |
| `list_file.rs` | Decoding a `--list` file by its bytes (D212). |
| `shell/` | The Explorer menu: `plan.rs` as data with no `cfg`, `apply.rs` writing it to HKCU (D132, D150, D154). |

### `ren-cli` — the headless runner

| Module | What it is |
|---|---|
| `main.rs` | clap commands (`preview`, `apply`, `undo`, `recover`, `presets`), sources made absolute (D207), the guard (D200), `<Ask>` handling (D209), escaped output (D214). |
| `compat.rs` | The legacy slash switches, translated before clap sees them (D102, D210). |
| `exit.rs` | The exit codes, which are the interface (D103, D206). |
| `tests/` | `roundtrip.rs`, `preset.rs`, `legacy_cli.rs` — the binary run as a process. |

### `ren-gui` — the app

| Module | What it is |
|---|---|
| `main.rs` | The portable decision before eframe starts (D131), the argument line (`launch`, D152, D211), `run_native`. |
| `app.rs` | `RenameItApp`: the frame (§4.1), hotkeys (D220), persistence (D226), wiring the workers to the panels. |
| `launch.rs`, `dialogs.rs`, `theme.rs` | The command-line grammar; native file dialogs behind a trait so tests never open one; the `Style`, contrast-tested (P93, P113). |
| `viewmodel/` | No egui in it: `session.rs` (source, selection, sort, Free Select, the guard), `preview.rs`, `listing.rs`, `apply.rs` (the three workers above), `history.rs` (runs, undo, the recovery banner's data), `stack.rs` (the card stack), `columns.rs` (the column model). |
| `panels/` | The zones and windows: `source_bar`, `operation` (the card stack), `file_table` and `grid` with `rows` and `tile` shared between them, `status_bar`, `visual_assist`, `palette`, `presets`, `settings` (nine pages), `confirm`, `ask`, `about`, `run_settings`. |
| `editors/` | One editor per operation, named for it, a dispatcher in `mod.rs`, and `assist.rs` — Visual Assist's widget-free half (D147). |
| `widgets/` | `diff_text`, `filter_editor`, `form`, `icons`, `number`, `preproc_editor`, `row_keys`, `rule_table`, `string_list`, `tag_field`, `tri_checkbox`. |
| `thumbs/` | The decode pool (`worker.rs`) and the byte-bounded texture cache (`cache.rs`, D138). |
| `examples/spike*` | Spike B's harness: the frame budget and the grid's tile count (D165). |
| `tests/gui.rs` | The app driven headlessly through its accessibility tree (D24). |
