# RenameIt — Consolidated Design

Three independent design documents produced during planning (2026-08-15), preserved verbatim below.
They converged on the same stack: **egui/eframe GUI, Koto scripting (D92; they said Rhai), lofty/kamadak-exif metadata, fancy-regex, transactional planner with journal-backed undo**.

**Authority order: `docs/DECISIONS.md` > this file.**
Where a section below contradicts a locked decision, the decision wins. Known deltas:

- **No legacy importers.** Sections mentioning importers for legacy settings files, or a COM scripting bridge, are superseded: the locked decision is *fresh start* — ship equivalent defaults in new native formats (recreated as data, not byte-copied), one-language scripting (Koto, D92).
- **Crate naming**: use `ren-core`, `ren-platform`, `ren-cli`, `ren-gui` (Part 1 layout).



---

# Part 1: ren-core: Platform-Neutral Rename Engine Architecture

# ren-core — Engine Design

## 1. Workspace layout

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

**Scoping is the engine's job, not each op's.** The engine slices the filename by `Scope` (name / ext / full) and then by the `PreProcessor` (skip-n, skip-until, limit-n, cut-at, advanced wildcard/regex section match — applied top-to-bottom, name-only per spec). The op sees only the active section; the engine reassembles. This reproduces original semantics exactly and keeps every op trivial to test.

A **Pipeline** is `Vec<(Step, StepConfig)>` — this is also the preset format (serde JSON) and directly gives the modernized "visible multi-operation pipeline" UI. Filters are evaluated against the *current* (already-transformed) name at each step, as in original presets, with a global "override per-step options" flag.

`NameTransform::apply` outputs may contain `/`-separated relative components (the `<\>` tag) — `VirtualName` is `Vec<Component>` + extension, so move-to-subfolder falls out naturally.

## 3. Template / tag system

Grammar: literal text + `<Ident(-arg)*>` tags, case-insensitive except metadata namespaces. Compiled once per pipeline edit:

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
- **Counter**: full original semantics (start, step, zero-pad/auto, reset-each-folder, reset-on-basename-change, reset-at, persistent "running counter"). Stateful — see §4.
- **Dates**: VB-style format strings (`yyyy-mm-dd`, `dddd m mmmm`, `Hh:Nn:Ss`, named formats) translated by a small mapper onto `chrono` format items; ISO 8601 default.

## 4. Preview pipeline

Target: full recompute of 10k files in <30ms for pure ops; metadata fills in asynchronously.

```rust
pub struct PreviewEngine {
    generation: AtomicU64,          // cancellation token
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

Planner: nodes = renames; edge A→B if A's target equals B's current source (folded). Topological order executes chain tails first; **cycles** (A↔B swaps) are broken by renaming one member to `name.__ren_<128-bit random>` first, restoring last. Case-only renames on case-insensitive volumes execute as a single direct rename (NTFS allows it); the folded-map logic treats them as non-conflicting self-edges. Full two-phase (everything→temp→final) is a fallback used only when the destination volume's case behavior can't be probed.

**Journal**: write-ahead JSONL in the app data dir; entry per op `{txn, seq, op, done}` with an fsync before each destructive batch. Records old **and** new names, plus prior dates/attributes for `SetDate`/`SetAttributes`. Undo = replay reverse-order with pre-flight staleness check (current name/mtime must match journal; mismatches reported, rest offered). Crash recovery: on startup an unclosed txn offers rollback of completed steps. Tag writes/removals are `Undoability::None` (as original) unless the "sidecar backup" option (question below) is adopted. Simulation mode = execute planner, skip syscalls, render the log.

`trait Platform` (in ren-platform): `set_times(created?, modified?, accessed?)` (created: `SetFileTime` on Windows, unsupported-error on Linux, `setattrlist` later on macOS), `get/set_attributes(RHSA)` (Unix: readonly bit only), `naming_rules(volume)`, `case_sensitivity(path)`, `notify_shell(path)`.

## 6. Metadata providers (license-audited)

| Domain | Crate | License |
|---|---|---|
| Audio tags+props (MP3/Ogg/Opus/FLAC/Speex/MPC/WavPack/MP4/AIFF/APE) | `lofty` | MIT OR Apache-2.0 |
| ~~ID3 fine control (write v1-only/v2.3/v2.4, update-mode)~~ | ~~`id3`~~ | — **dropped, D60**: `lofty` covers the dependency question. See **D90** for what was *not* delivered — we always write v2.4, and the v1-only/v2.3 choices became a fixed policy (D70). |
| EXIF read (JPEG/TIFF/HEIF/PNG/WebP) | `kamadak-exif` | BSD-2-Clause |
| EXIF/IPTC strip (untagger) | `img-parts` | MIT OR Apache-2.0 |
| PDF pages/info | `lopdf` | MIT |
| `<DetectedExt>` | `infer` | MIT |
| `<Crc32>` | `crc32fast` | MIT OR Apache-2.0 |
| Regex (needs lookaround + backrefs) | `fancy-regex` | MIT |
| CSV | `csv` | Unlicense OR MIT |
| Core plumbing | `rayon`, `serde`, `chrono`, `unicase`, `walkdir`, `filetime`, `thiserror`, `windows-sys` | all MIT and/or Apache-2.0/Unlicense |

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

Batch Replace = a `Replace` op vector. Filename Editor = `Editor` transform holding `Vec<String>` by row. Casing exceptions/preserve rules = data file consumed by `ops::casing` (Unicode-aware via `unicode-segmentation`). Renumber decimals/minus/zero-pad-keep-length = `ops::renumber` with `rust_decimal` (MIT). `<HtmlTitle>` = bounded read + regex, no HTML crate needed. Running-counter persistence + `<Ask>` memory = `preset.rs` sidecar state.


## Key risks

- fancy-regex uses backtracking: user patterns can be exponential and preview runs them per keystroke per file — must wrap evaluation in a time/step budget and fall back to 'pattern too slow' row errors
- Stateful tags (Counter with folder/basename resets, unique Rnd) force a serial finalize pass; validate the two-phase preview holds the <50ms feel at 50k-100k files before freezing the design
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

`renameit-gui` (bin, depends on `renameit-core`):

- `app.rs` — eframe `App`, top-level layout, theme, command routing
- `viewmodel/` — `session.rs` (file set, selection, sort), `preview.rs` (async preview cache + generation counter), `undo.rs`
- `panels/` — as built: `source_bar.rs`, `operation.rs`, `file_table.rs`, `grid.rs`, `tile.rs`, `rows.rs`, `status_bar.rs`, `visual_assist.rs`, plus the windows below. *(This bullet and the two under it were written before M2 and named modules that never existed — `pipeline.rs`, `thumb_grid.rs`, `inspector.rs`; D8 killed the inspector. Corrected in M8 rather than left as a map of a building nobody put up.)*
- `editors/` — one module per operation editor (replace, casing, add_remove, move_section, space_trim, music, counter, renumber, zero_pad, attributes, datetime, csv, list_editor, format, script)
- There is no `dialogs/` **directory**: `dialogs.rs` is the `FileDialogs` trait, and the windows are `panels/{settings,presets,confirm,ask,about,run_settings}.rs`. **Visual Assist is not a dialog at all** — it is an inline strip inside the card whose field it fills (**D143**).
- `widgets/` — `diff_text.rs` (before/after span highlighting), `tag_picker.rs` (the `<tag>` dropdown), `filter_editor.rs`, `virtual_table.rs` wrapper

Preview flow: any edit bumps a `u64` generation, sends `(generation, PipelineSpec)` over a channel to a core worker; core computes `Vec<PreviewRow>` (new name + per-row status: Changed/Unchanged/Conflict/Invalid/Error) using rayon; results arriving with a stale generation are dropped. UI paints the latest cache; visible-row diff spans are computed lazily per row. This keeps 10k files "instantaneous" and never blocks paint.

## 3. Modernized UI design

**Core idea: the pipeline is first-class.** Rather than hiding multi-operation stacking inside Presets, the left panel *is* an ordered stack of operation cards. One card = the classic single-function mode; presets become nothing more than saved pipelines. This unifies "run one function", "stack functions", and "presets" into a single mental model.

Five groups (General, Music, Numbers, Advanced, Presets) map to: four **categories in the Add-Operation palette** (General: Replace, Set Casing, Add & Remove, Move Section, Space Trimming; Music: Music Rename, Tag Writer, Remove Tags; Numbers: Add Counter, Re-Number, Zero Pad; Advanced: Set Attributes, Set Date & Time, CSV List, Filename Editor, Free Format, Scripting) — and **Presets becomes the Preset drawer** (saved pipelines).

### Screen-by-screen outline

**S1. Main window** — three zones, fully resizable splitters, light/dark/system theme.

- *Top: Source bar.* Segmented control **Browser | Free Select** (replaces the old tabs). Browser: editable breadcrumb path + folder picker + pattern box (`*.mp3`) + toggle chips **Files / Folders / Subfolders** + an **Include filter** chip that opens a popover (contains/wildcard/regex include + exclude, match name/path/extension). Free Select: same bar shows "N files from M folders", an *Add files…* button, and a *Clear* button; drag-and-drop from Explorer adds files in either mode (in Browser mode a drop offers "switch to Free Select with these files"). Right side: a **List | Grid** pair, matching Browser | Free Select — thumbnails ship as a view *and* as a column and the user chooses, because which one a single checkbox ought to mean is genuinely ambiguous (D133).
- *Left: Pipeline panel.* Vertical stack of **operation cards**: drag-handle to reorder, checkbox to enable/disable (instant preview reflects it), name, one-line summary ("Replace `_` → ` `"), overflow menu (duplicate, delete, per-op scope). Selecting a card opens its **editor** inline (card expands) — no separate inspector to keep spatial locality. Bottom: **+ Add operation** button → palette (S2). Header: pipeline name, *Save as preset*, *Presets ▾* drawer. Per-op overrides: scope **Name / Extension / Both** and an optional per-op include filter live in each card's "Scope" expander; the global options only set defaults. **Counter** and **Parts** setups appear as two pinned chips at the panel top — "Counter: 1, step 1, pad auto" and "Parts: `<%1> - <%2>`" — each opening its dialog (S5/S6).
- *Right: File table (S3) / thumbnail grid.*
- *Bottom: status + action bar.* Left: "1 284 files, 12 folders • 1 240 will change • 2 conflicts". Right: **Simulate** toggle (a persistent switch that turns the Rename button into "Simulate"), **Undo ▾** (history stack), primary **Rename** button (F5) — disabled with a reason tooltip when conflicts exist or the pipeline is empty.

**S2. Add-operation palette.** Modal popover with search box; operations grouped under General / Music / Numbers / Advanced, each with icon + one-line description. Enter adds and expands the card. Also reachable via Ctrl+K command palette.

**S3. File table.** Virtualized (`egui_table`), sortable, user-configurable columns: ☑ selection, icon, **Name**, **→ New name**, size, dates, attributes, tag columns (artist/title) — defaults: Name + New name. The New-name cell renders **inline diffs**: deleted spans struck/red-tinted, inserted spans green-tinted; unchanged rows dimmed. **Row states:** normal; *unchanged* (dim); *conflict* (red badge — duplicate target, name collides with existing file, invalid characters, path too long) with tooltip explaining and a right-click "resolve: auto-number" action; *warning* (amber — extension changed). A filter chip above the table: All / Changed / Conflicts. **F2 / middle-click inline rename** — built in M8: the **stem is selected** with the caret at the extension boundary (Explorer's behaviour), and Enter renames and advances to the next row **on screen**, found by path because a rename re-sorts. **Drag rows to reorder** (feeds counter order) — the whole row is the drag source, and the order survives the run that used it (**D157**). Arrow keys, Home, End, `Ctrl+A`, and `Enter`/`Backspace` to walk the folder tree (**P87**, **P90**). Right-click menu: rename, add to Free Select, exclude from this run, open, show in Explorer (platform trait), properties. "Only rename selected" behavior kept: selecting rows scopes the run, none selected = all (per settings).

**S4. Operation editors** (inline in cards). Each carries its operation's full option set — e.g. Replace: find/replace, wildcards `* : ?`, regex with `$1–$9`, case-sensitive, swap mode, skip/start/count; Casing: name & extension modes (UPPER/lower/Sentence/Title/iNVERT/rANdOm), lowercase-exception words, preserve-caps options, exceptions list editor; Add & Remove with position, backwards counting; etc. Every position/length field gets the modernized **Visual Assist**: instead of a separate window, clicking the ⌖ button enters a mode where you *select a text span directly in any New-name cell* and the fields populate. **Amended in M8, twice.** The span is selected in a read-only field **inside the card**, not in a New-name cell — that column is the pipeline's *output*, while a position field is measured on the operation's *input*, which for a card partway down a stack is neither column on screen (**D144**). And it is **four** call sites rather than every position/length field: Replace's Skip and Max are match counts, Zero Padding's Digits is a width, and the CSV columns are column numbers, so a span picker on those means nothing. Tag-accepting fields have a `<>` button opening the tag picker (all `<tag>`s: name parts, `<counter>`, `<%1>`–`<%9>`, dates, size, parent folder, music tags, EXIF).

**S5. Counter setup dialog.** Start, step, running counter, reset-per-folder / on-base-name-change / reset-at, zero-padding (fixed or auto) — with a live 3-line sample.

**S6. Parts setup dialog.** Pattern field + live preview against the selected file, magic-wand auto-detect from a selection.

**S7. Execute flow.** Rename runs the core's two-phase transactional plan (cycle-safe via temp names). A pre-flight sheet summarizes: N renames, M skipped (filters/unchanged), K conflicts and chosen policy (skip / auto-number, from settings). Progress bar + streaming log; on completion a toast with "Undo" and a collapsible log drawer. Simulate runs the identical flow, writes nothing, and marks the log "SIMULATION".

**S8. Preset drawer / manager.** Slide-over listing saved pipelines with descriptions; actions: Load (replaces stack), Append, Run directly, rename/delete/duplicate, import/export (**TOML** files per D18 — enables sharing; the legacy `presets.dat` importer is dropped by D6). "Save as preset" captures the current stack including per-op scopes/filters; an `<Ask>` token in any text field prompts at run time (original feature, kept).

**S9. Settings dialog.** Pages: ~~**Renaming**~~ — **there is no Renaming page (P88)**: P4 makes a conflict a hard block rather than a policy, the extension-change warning is an inline per-row badge rather than a setting, and P22 makes selection scope the plan, so all three of the settings this page was promised became behaviours with decisions behind them. **Appearance** (theme, columns, font size, date format), **File system** (Windows page: attribute handling, created-date setting), **Shell Integration** (the Explorer menu: one checkbox that installs the whole cascade, a state line read back from the registry rather than remembered, the preset count, Refresh, and the two facts the menu cannot tell anyone itself — Windows 11's *"Show more options"*, and the 2000-character selection cap; behind the platform trait, and the page says so where there is nothing to install), **Casing exceptions**, **Batch replace list**, **Scripting** (engine: Koto — MIT — replacing WSH/VBScript, cross-platform), **Advanced**.

**Accessibility & keyboard:** full AccessKit exposure; F2 rename, F3 visual assist, F5 rename, Ctrl+Z undo, Ctrl+K palette.

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
Scope: workspace, CI matrix, `cargo-deny` license gate, `PlatformFs` trait with Windows+Unix impls for rename/attributes/dates, plan/execute/undo skeleton with a single hardcoded op, `renamer-cli preview|apply|undo`. Two spikes: (a) `fancy-regex` evaluated against all 69 `replace.dat` rules; (b) egui virtual list rendering 10k rows with per-frame recompute benchmark.
Acceptance: CI green on Linux + Windows; CLI renames a tempdir folder and undoes it byte-exactly; both spikes have written verdicts.
Defer: everything user-visible.

**M1 — Core pipeline + General group (the product's spine).**
Scope: full plan/conflict/execute/undo engine (collision detection incl. case-insensitive targets, swap/cycle handling via temp names, "unchanged" suppression); Global options: process name/ext split, include/exclude filter (substring, `* : ?` wildcards, regex), simulation mode; ops: **Replace** (case-sensitivity, swap mode, skip/start/count, regex with `$1-$9`), **Set Casing** (upper/lower/sentence/title/invert/random, lowercase-exceptions list, preserve-uppercase/mixed, `exceptions.txt`), **Add & Remove** (insert/overwrite/remove at pos/from end), **Move Section**, **Space Trimming**. CLI exposes all of it.
Acceptance: documented semantics from `func_replace/casing/add_remove/move/space.html` encoded as unit tests; property tests pass (below); 10k-file preview < 50 ms per keystroke on reference hardware; undo restores an adversarial tempdir (swaps, chains, unicode, case-only renames) exactly.
Defer: GUI, tags, counters.

**M2 — GUI shell v1.**
Scope: egui app: folder browser + Free Select (drag-drop), virtualized file list with live preview column, sortable columns, global-options panel, one-op-at-a-time function panel for the M1 ops, Rename/Undo buttons, log window, status bar, F2 inline manual rename with smart extension-aware cursor + jump-to-next, dark/light mode, resizable everything.
Acceptance: a user can perform every M1 operation without the CLI; preview visibly updates while typing on 5k files; window state persists.
Defer: pipeline editor UI, thumbnails, preset manager.

**M3 — Numbers group + format tags core.**
Scope: `<counter>` engine (start, step, zero-pad incl. auto, running counter persistence, reset-each-folder / on-base-name-change / reset-at), **Add Counter**, **Re-Number**, **Zero Pad Numbers**; format-tag engine v1: `<Name> <Ext> <FullName> <Size*> <Parent[-#]> <Left/Right/Mid/MidRev-#> <FLetter*> <Date*/Time*>`, Parts `<%1>–<%9>` with separator-pattern setup + auto-detect, "only rename if all tags available".
Acceptance: tag engine has a table-driven test per documented tag; counter reset semantics match Help exactly; renumber round-trips a shuffled `01..NN` set.
Defer: EXIF/music tags, folder-content "peek".

**M4 — Pipeline UI + Presets (first modernization payoff).**
Scope: the visible multi-operation pipeline (ordered op list, enable/disable, drag-reorder, per-op include filters, recall-into-editor) — this *is* the preset editor; preset save/load in a TOML format; ~~legacy importers for binary preset and batch-replace files~~ — **dropped, D6**; Batch Replace op; Pre-Processor (section slicing before op, top-to-bottom rules); "override stored global options" toggle; CLI `--preset`.
Acceptance: all 4 shipped default presets import and produce documented outcomes on fixture trees; golden snapshots of importer output; a 3-op pipeline previews as one composed result and executes atomically with single-journal undo.
Defer: shell extension hookup of presets.

**M5 — Advanced group.**
Scope: **Set Attributes** (tri-state R/H/S/A via `PlatformFs`; Unix maps read-only/hidden-dotfile or no-ops with capability reporting), **Set Date & Time** (all sources: manual, created/accessed/modified, now, add/subtract interval, from filename via parts `<%4>–<%9>`), **CSV List Rename** (full CSV rules from `func_csv.html`, tags in new-name column), **Filename Editor** (line-per-file editor, `edit.txt` persistence), **Free Format** (pure tag-pattern rename).
Acceptance: attribute/date changes verified by integration tests on the Windows runner (created-date is Windows-only — assert capability error on Linux); CSV fixture matching the Help example passes verbatim.
Defer: EXIF date source until M6 if `kamadak-exif` integration slips.

**M6 — Music + EXIF.**
Scope: read tags via `lofty` (MP3/ID3v1+v2, Vorbis, FLAC, Speex, MPC, WavPack, MP4/M4A…); **Music Renaming** styles + custom patterns, "only if all tags available", folder "peek" (first audio/image file inside folder supplies tags); **Music Tagger** (write, tri-state fields, parts→tags quick setup, ID3 version choice where lofty allows) and **Untagger** — both clearly marked *no undo*, gated behind confirmation; EXIF tags incl. smart `<ExifDate>/<ExifTime>`; optional external-`exiftool` bridge (subprocess only — keeps GPL/Artistic code out of our binary).
Acceptance: golden tests over a checked-in corpus of tiny tagged fixture files per format; tagger write→re-read round-trip; WMA/TTA support gap either closed or documented in release notes.
Defer: thumbnails.

**M7 — Scripting + automation.**
Scope: `koto` scripting with a façade over the documented API (`filename, full_filename, path, args, preview, disk_name, num_items, item_order, browser_path, format_tags(), get_all_filenames()`); nine worked example scripts; migration doc (VBScript `.frs` cannot run — flagged loudly); the full legacy switch set + `--simulate`.
Acceptance: each ported script reproduces its original behavior on a fixture tree; scripted preview vs execute consistency property holds.
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

**Golden tests (insta).** Fixtures: the shipped presets, the batch-replace rules, the casing exception lists and the nine example scripts. Snapshots cover end-to-end rename results on synthetic trees.

**Integration tests.** `tempfile`/`assert_fs` trees; hostile cases: locked file mid-batch (Windows), read-only targets, >260-char paths, unicode/emoji, reserved names (`CON`, trailing dots/spaces), partial-failure rollback and journal recovery after a simulated crash.

**Benchmarks.** `criterion` on plan generation for 1k/10k/50k items; CI trend gate (fail on >2x regression). Preview path: debounced input → `rayon` parallel plan → diff-only UI update.

**CI matrix (GitHub Actions).**
- `ubuntu-latest`: fmt, clippy `-D warnings`, `cargo-deny` (licenses + advisories), core tests, `cargo xwin build --target x86_64-pc-windows-msvc` as a fast cross-compile smoke.
- `windows-latest`: full test suite (the only place attribute/created-date/case-insensitive tests really run), GUI build, packaging step producing `RenameIt.exe` (icon, `FileVersion`, static CRT) + portable zip.
- Tag push → release workflow attaching artifacts; `macos-latest` added when the Unix port starts.

## Sequencing rationale / de-risking

Riskiest items are pulled forward: regex flavor and preview performance (M0 spikes), transactional rename + undo + case-insensitive semantics (M1, everything else stands on it), legacy `presets.dat` reverse-engineering (M4, timeboxed — fallback is a guided import assistant), tag-library coverage (M6 fixture corpus built before UI work), shell extension deliberately last and tiered because a Rust COM extension is the least-certain component and has a cheap tier-1 substitute.


## Key risks

- Regex flavour mismatch: the batch-replace rules rely on capture groups and byte-range character classes, so fancy-regex must be validated against every shipped rule in M0
- Case-insensitive NTFS semantics (case-only renames, A<->B swaps, reserved names, long paths) are where renamers eat data; must be solved and property-tested in M1, and can only be truly exercised on the Windows CI runner
- Instant preview at 10k+ files with per-keystroke recompute is a hard perf budget; egui + rayon spike in M0 must confirm <50 ms or the UI architecture changes early
- Audio tag coverage gaps: lofty may not cover WMA/TTA; and tag writes have no undo — needs explicit UX gating
- A Koto façade plus worked examples is a compatibility break for anyone with VBScript `.frs` files, and requires user expectation-setting
- Windows Explorer context-menu extension (COM/sparse package) is high-effort, low-certainty; mitigated by tiered plan with a simple launcher-based tier 1 for 1.0
- Windows-only behaviour (attributes, created dates, case-insensitive renames) can only be confirmed on the Windows runner or by hand; a late surprise there could reopen a 'done' milestone
