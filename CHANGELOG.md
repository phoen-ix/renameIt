# Changelog

Notable changes, newest first. Versions follow [semantic versioning](https://semver.org).

Two things this file does **not** track. Engine and UI decisions live in
`docs/DECISIONS.md`, which is the authority on why anything behaves as it does;
and the reasoning behind each default lives beside the code it governs.

## [Unreleased] — the audit

A read of the whole workspace for defects, dead code and cost, with the fixes.
Every finding is recorded with its decision number in `docs/DECISIONS.md`
(**D165**–**D170**, **P99**).

### Changed

- **A folder is listed on a worker thread** (**D167**). The walk used to run
  inside the frame, and the window was frozen until it came back — a stutter
  on a warm disk, seconds on a share or a deep Subfolders tree. The old
  listing stays on screen with *listing…* beside it until the new one lands.
- **A run, and an undo, happen on a worker thread** with *Renaming N of M…*
  in the status bar and a **Cancel** that stops a run between two files
  (**D169**). What has been renamed stays renamed and Undo takes it back.
- **A run costs one `fsync` per file, not two** (**D168**): the confirmation
  line is carried to the disk by the next file's intent. Over 1 000 files
  `strace` counts 1 002 syncs where it counted 2 002. A wider write-ahead
  window was tried and refused by a new property test that crashes a run at
  every point and recovers it (**P99** says what widening would need).
- **A keystroke over ten thousand files is inside its budget.** The planner
  went from 68 ms to 22 ms on the reference box (**D165**): Batch Replace asks
  one `RegexSet` which of its fifty-one rules can match a name and runs only
  those (**D166**); plan rows are built in parallel; the conflict passes stop
  walking paths per row. CI now runs the budget on the real engine
  (`ren-core/examples/plan_budget.rs`) — the old gate measured a harness that
  skipped all of the planning and reported 4 ms.
- **Listing a folder does one `stat` per entry**, not two; on Windows the
  second was the expensive open-handle call and the whole cost of the walk.
- **The metadata readers no longer `stat` every file per keystroke**, and no
  longer queue on one lock inside the parallel pass: one shared cache, keyed
  on the listing's own size and date.
- The preset drawer and the Settings window no longer re-read and re-parse
  every preset file on every frame.
- `presets list` exits 1, not 3, on an unreadable preset file: nothing ran.

### Fixed

- **`<HtmlTitle>` could panic** on a page whose `&` was followed by non-ASCII
  text at the wrong byte — and a panic in the preview worker used to wedge the
  app for the session with *updating…* that never finished. The panic is
  fixed, and any panic in the engine is now reported in the status bar with
  Rename disabled, not swallowed.
- **A journal write that failed mid-run exited 1** ("your command line was
  wrong") after files had already moved. It is exit 3 now (**D103**), with the
  count, the transaction and the cause; the GUI relists after it.
- **The plan on screen could describe another file** for a moment after a
  selection change, a sort or a relist — the index into it was kept apart
  from the plan and could disagree with it. The plan now carries its own
  scope, and every row checks the file before showing a preview for it.
- **A symlink is listed as the link it is**: a dangling one is a real,
  renameable row rather than an unreadable one, and a link to a folder is a
  file row, as the Files chip already treated it.
- `ren-cli dir --journal-dir /j` filed `/j` as a file to rename; a `/l` list
  file with a byte-order mark named a first path that did not exist; a switch
  with no value read as `""`; `/k` without `/l` produced a usage dump.
- Renaming a preset whose old file could not be removed reported success and
  left two presets. Recovery's completed count could underflow on a malformed
  journal. The *Save as preset* box could not be cleared. Deleting a Batch
  Replace row handed its cursor to the next one. A ⌖ click could be
  overwritten by the strip in the same frame. Arrowing over an empty,
  filtered list could panic.
- The release workflow's gate could not read CI runs on a private repository
  (`actions: read`), and no build step honoured `Cargo.lock` (`--locked`).
- Windows: the reserved-name table gains `COM0`/`LPT0` and the superscript
  aliases; *Show in file manager* quotes only the path, not the whole
  `/select,` argument; `SetFileAttributesW` is handed only the bits it can
  set; the system-folder guard keeps `C:\Windows` even with a stripped
  environment. Unix: a Linux volume mounted inside a FAT or NTFS tree keeps
  its own naming rules.

### Removed

- Dead code across all four crates, Spike B's harness out of the shipped
  library and into the examples that run it, two phantom workspace
  dependencies, and three doc comments describing tests and callers that did
  not exist.

## [1.3.0] - 2026-08-22

### Changed

- **Explanatory text is no longer the smallest text in the app** (**D163**).
  egui's `Small` is meant for incidental annotation — a unit beside a number —
  and this app used it for actual prose in **106 places**, so on the Settings
  pages almost every word carrying meaning was set at 9 pt while the four-word
  headings got 13. The scale was inverted, not merely small. Prose is now 22 %
  larger and sits at 0.79× body rather than 0.69×.
- **Settings ▸ Appearance can scale the whole window**, 90 % to 200 %
  (**D164**) — text, spacing, icons and rows together, so nothing is left
  behind at its old size. Ctrl+`+`, Ctrl+`-` and Ctrl+`0` have always done this;
  they were simply never mentioned anywhere. This is where
  `docs/DESIGN.md` specified a font-size control belonged before the page
  existed.

### Fixed

- **`Reset all settings` now puts the interface size back.** It lives in egui's
  own storage rather than in ours, so a reset that walks our settings field by
  field could not reach it.
- Four heights that had to fit text were bare numbers with no metric behind
  them — the file table's row and header, the grid tile's caption, and the
  Settings rail (**P97**). They ask the text now, which is what let the type
  scale move at all.

## [1.2.0] - 2026-08-22

The crash on unusual filenames, and the fonts to show them.

### Fixed

- **RenameIt crashed on a filename that is not valid Unicode** (**D159**,
  **D160**) — a Latin-1 `é` from an old drive, a name written under another
  locale, an unpaired surrogate on NTFS. Two failures in order: the listing
  replaced the offending byte with U+FFFD *silently*, so any new name would
  have been written over what was really there; and then the run **panicked**,
  because `serde` refuses to serialise a non-UTF-8 path and the journal said
  `.expect("journal records are always serialisable")`. There is no
  `catch_unwind` on that path, so in the GUI the whole window went down
  mid-batch, after the journal was already open. **It shipped in 1.0.0.**

  Such a file is now listed, marked *name is not text*, left alone by every
  pipeline, and repairable: press F2 and give it a name directly. That is safe
  because the *path* was always byte-exact, and it can be undone — the journal
  learned a lossless path encoding to make sure of it. One odd file costs its
  own row and no others (**P63**), so the rest of the folder still renames.

### Added

- **Fonts for CJK, Hebrew, Arabic, Thai and Devanagari** (**D161**). A rename
  tool shows filenames, and a filename is whatever the person who made it
  typed; anything outside Latin, Greek and Cyrillic used to draw as `◻`. Five
  OFL-1.1 Noto faces are compiled in. The binary grows **20.7 MB → 36.0 MB**,
  which is a deliberate trade recorded against D14's budget; startup is
  unchanged at 1.6–2.2 ms.
- A fifth property invariant: **a name that is not Unicode survives any
  pipeline untouched**, compared as bytes rather than text. The existing four
  could not reach this — the generator builds names as Rust `String`s, so an
  invalid name was unreachable by construction, and the "no loss" check
  compared `to_string_lossy` to `to_string_lossy`.

- **A volume that enforces DOS naming now gets the Windows rules on Linux**
  (**D162**). A file on a FAT stick or a mounted NTFS drive was validated with
  POSIX rules, which allow `:`, `?` and `*` — so a rename either failed at the
  syscall or produced a name the drive's other operating system cannot open.

### Known

- **Right-to-left word order** (**P95**). Letterforms and Arabic joining are
  correct — egui 0.36 shapes with HarfBuzz — but egui has no bidi pass, so a
  Hebrew or Arabic name containing a space renders its *words* in logical
  rather than visual order. A single word is right. Upstream gap, with the
  author's own TODO against it.
- **Colour emoji.** epaint's rasteriser is outline-only, so emoji are
  monochrome.
- Scripts outside the bundled five — Armenian, Georgian, Ethiopic and the rest
  — still draw as boxes.

Neither display limit stops a file being renamed: the engine works on bytes.

## [1.1.0] - 2026-08-22

A pass over how the window *reads*, prompted by a set of screenshots — plus
one engine fix, because running the suite for it turned up a planner defect
that had nothing to do with pixels. Four of these were defects rather than
preferences. Nothing was ever overwritten by the planner one — a rename onto a
directory fails rather than replacing it — but the run stopped part-way with a
journal open, which is the state P4 exists to keep the app out of.

### Fixed

- **Thirteen glyphs shipped as `◻`.** egui's `Proportional` family resolves
  Ubuntu-Light → NotoEmoji → emoji-icon-font, and none of the three had the
  code points the UI was typing: the operation card's drag handle and overflow
  menu, the About button, the file table's sort indicator, the tag field's
  history dropdown, four of the eight controls on every Batch Replace row, the
  preset row menu, the "no preview" tile, and — worse, because it is prose
  rather than an icon — the `Settings ▸ File System` inside the blocked-Rename
  explanation and the `→` inside the confirmation dialog's summary.
  Seven of the thirteen are in `Hack`, which epaint already bundles as the whole
  `Monospace` family and is now named as a `Proportional` fallback — that alone
  fixes every one that appears inside a sentence. The other six are in no
  bundled font at all: four became painted marks, and the two that are prose in
  the undo log took marks Ubuntu-Light already has. Four more that *did* render
  were painted anyway, because they are icon-only controls and a fallback glyph
  at emoji scale beside a drawn one looks like a mistake. **It was an
  accessibility defect too**: for an icon-only button the glyph is also its
  accessible name, so four tests were asserting tofu (**P92**).
- **Unchecked checkboxes were invisible inside an operation card.**
  `Ui::dnd_drop_zone` fills a card with `widgets.inactive.bg_fill`, which is the
  colour egui paints an unchecked checkbox with, and stock `inactive.bg_stroke`
  is `Stroke::NONE` — card and box were both `gray(230)` in light mode and
  `gray(60)` in dark, a contrast ratio of **1.00:1**. *Case sensitive*, *Swap
  mode* and *Regular expression* had no visible control at all (**P93**).
- **The file table used 298 pt of a 1 528 pt pane.** `egui_table` defaults to
  `AutoSizeMode::Never` and content-sizes every column on the first frame, so
  Name came out at 85 pt and New name at 64 while two thirds of the window sat
  empty to their right — and `renameit.exe` still ran into `unchanged` into
  `18.9 MB`, with no gutter anywhere and no ellipsis to say text had been cut.
- **The Settings window resized itself around whichever page was open**, from
  142 pt on Appearance to 562 on Casing Exceptions, moving the Close button
  420 pt up the screen when you changed tab.
- **Two runs of fourteen literal spaces** in the middle of the Problem Solver's
  "Start again" paragraph, left over from a flattened multi-line literal.
- **A run could plan a folder and a file onto the same path** (**P94**), and
  say it was executable. `<\>` moves a file into a subfolder, and the folder
  one row moves into could be the name another row was producing — a plan
  holding both `CreateDir .../sub` and `Rename .../1 -> .../sub`, which failed
  on whichever ran second — leaving the batch half-applied behind an open
  journal. Nothing was overwritten (a rename onto a directory fails rather than
  replacing it), but a half-applied run is precisely what a preview is supposed
  to rule out. That is the exact
  failure **P4** exists to prevent, and the check it needed was a third
  occupancy test: rows are compared against each other and against the disk,
  but never against the directories the same run was about to create. Found by
  the property suite during this pass and not caused by it.
- **A fresh Replace card summarised itself as `Delete ""`** — `{:?}` on an empty
  `String` — which describes deleting nothing and is the first thing a new user
  reads in the pipeline panel.

### Changed

- **The GUI has a `Style` of its own** (**P93**). It had none: the only styling
  call in the crate was `ctx.set_theme`, so contrast, spacing, checkbox, button
  and accent were all egui's demo-window defaults. Every colour is solved
  against WCAG AA and re-derived from the `Style` in a test, so a later tweak
  that drops one below the floor fails the build rather than the review.
  Secondary text was 2.7:1 and the *unchanged* marker 1.7:1; the accent was
  1.55:1 against the light panel, which is a "selected" segment you cannot see.
- **One accent, one meaning.** An expanded operation card wore the same blue
  slab as the source mode, the Files/Folders chips, the List/Grid toggle, the
  row filter and the settings tabs, so `Delete ""` read as a badge rather than a
  control. It is a disclosure caret and plain text now.
- **`Rename` is a primary button**, which `docs/DESIGN.md` §S3 has specified all
  along. Its whole emphasis was `RichText::strong()`, leaving it dimmer than the
  *Simulate* label beside it. In Simulate mode it reads "Run simulation", so the
  toggle and the button it controls no longer share a word.
- **Settings and About moved out of the Pipeline panel's header** to the
  window's top-right; `List | Grid` moved down to the toolbar's second row,
  joining the other switches that decide what the list shows.
- **The Settings pages are a left-hand rail**, which fixes both the height and
  the nine-tab strip that already touched the right edge of a 620 pt window at
  100 % scaling.
- **Batch Replace has column headings** — fifty-one rows of eight controls with
  nothing saying which was which.
- **Row shading ships on** (**P91**): P70
  waived "Show Guidelines" because shading is what makes a wide row readable,
  and the rows are only now wide. **New installs only** — an existing settings
  file already carries the old value, and "you never chose this" is not
  something the app can tell apart from "you chose it", so an upgrade leaves it
  alone. Settings ▸ Display switches it either way.
- **Counts read as English** — `1 item`, `2 items`, where the status bar, the
  confirmation, the undo log, the About box and the CLI all said `item(s)`.
- The add-operation palette's list follows the window height instead of a
  hard-coded 360 pt, which was chosen when the catalogue was shorter.

### Known

- **CJK, Hebrew and Arabic filenames draw as boxes.** The four bundled fonts
  cover Latin, Greek, Cyrillic and the common currency marks and nothing else,
  which for a tool that shows filenames from disk is a real gap. Closing it
  means shipping a CJK face against the binary budget **D14** watches, so it is
  a decision rather than a polish item. Recorded by the same test that keeps the
  UI's own glyphs honest.

## [1.0.0] - 2026-08-16

The first release. Eighteen operations that compose into a stack, a preview
that keeps up as you type, presets, a headless CLI, sandboxed scripting, and one
undo per run however many operations produced it.

The version is a claim rather than packaging (**P66**), and this is the commit
where it became true.

### Fixed, in code that had already shipped

Three defects the last pass turned up, none of which any test had caught:

- **Sorting the list re-scoped the run.** Selecting three files and then
  clicking a column header renamed whatever landed in those three rows —
  silently, because the highlight moves with the rows.
- **`Enter` never committed an inline rename.** F2's box re-requested focus
  every frame, which cancelled the focus surrender egui performs on Enter, so
  the one documented way to finish a manual rename did nothing.
- **A folder renamed alongside a swap inside it stopped a run part-way.**
  Inverting the casing of a folder holding `README` and `readme` renamed the
  folder first, and every path underneath then named a folder that was gone.

### Added

- **A RenameIt menu in Explorer.** Right-click files, folders, a drive, or the
  empty space inside a folder: *Start from this folder*, *Start and load
  selected files*, *Copy filenames to clipboard*, and then **one item per
  preset**. Install and remove it from Settings ▸ Shell Integration — it is
  written under `HKEY_CURRENT_USER`, so it needs no administrator either way.

  Choosing a preset **loads it and shows you the preview**. It does not rename.
  A right-click has nowhere to warn you that two files would end up with the
  same name, or to ask before something that cannot be undone, and the preview
  is one keystroke from the rename with both of those in place.

  The menu is a snapshot of your preset folder, because nothing in the registry
  can read a folder at the moment you right-click. RenameIt writes it again
  whenever you add, rename or delete a preset, and once at startup — so a preset
  dropped in by hand, one deleted while the program was closed, and a portable
  copy that moved all sort themselves out without you ticking anything again.

  Two things Windows imposes, said on the Settings page rather than left to be
  discovered: on **Windows 11** this lives under *"Show more options"*, and a
  selection reaches a menu entry as a command line the shell caps at about 2000
  characters — roughly thirty files. Past that Windows hides the entry rather
  than shortening the list. If a selection arrives near the limit, a dismissible
  banner says how many characters it was and offers to list the whole folder
  instead.

- **Six presets on a first run** — *Add prefix to filename*, *Add suffix to end
  of filename*, *Basic filename cleanup*, *Create numbered sequence*, *Rename
  Mp3s as Artist - Title* and *Sync file date with image Exif date*. Six presets,
  shipped as our own data. They are seeded only into a
  preset folder that does not exist yet: a preset you deleted is an item you
  took out of your own right-click menu, and putting it back every start would
  be the app arguing with you about your shell.

- `renameit --copy-names <files>` puts their names on the clipboard and exits,
  with no window. It is what the menu item runs, and it works from a script too.
  The app also still takes plain paths on its command line, so dropping files on
  the executable works as it always did — including one folder, which opens
  there rather than listing it as a single row.

- **Visual Assist.** Instead of typing "remove 5 characters from position 3"
  and checking the preview, click the ⌖ beside the box and *select the text*.
  Four places have one: Find & Replace's **Look For**, Add's position,
  Remove's section, and Move Section's cut. **F3**
  opens it, and on an Add & Remove card set to *Both* it cycles between the two.
  Selecting text for a regular-expression Find box escapes it, so picking
  `(1)` out of `photo (1).jpg` finds `(1)` rather than matching a bare `1`.
  When your selection runs to the end of a name, a button offers to anchor it
  there — so it lands correctly on every file rather than only on ones of that
  length.

  It shows you **the text that operation actually receives**, which is not
  always the file name: a card further down the stack sees what the cards above
  it produced, and a card set to *Name* sees the stem without its extension. And
  when there is nothing to select in, it says which of the reasons it is —
  the card is switched off, its filter skips this file, the file has no
  extension — rather than showing an empty box.

- **Thumbnails, as a view *and* as a column.** A `List | Grid` pair at the end
  of the source bar, and a **Thumb** column you can turn on in Settings ▸
  Display. Which of the two a single checkbox ought to mean is genuinely
  ambiguous, so both ship and you choose (D133).
  A grid tile carries the picture, the name on disk **and** the name the run is
  about to write, with the same conflict badge and the same fading the table
  uses — so the grid is somewhere you can work, not only somewhere you can look.
  Six formats (BMP, GIF, JPEG, PNG, TIFF, TGA); anything else says so on the
  tile rather than leaving a blank square. Photographs come back the right way
  up, whatever your phone wrote in their Exif.
- **A size slider** for thumbnails, and a border checkbox, both in Settings ▸
  Display. Continuous, rather than three fixed sizes.

Two things about how it behaves, because they are the difference between a
thumbnail view that is pleasant and one that is not:

- **Only what is on screen is ever decoded**, in either view. A folder of ten
  thousand photographs costs a screenful of work, not ten thousand, so a large
  folder cannot run the machine out of memory. The pictures are held under a
  memory budget
  that drops the least recently seen one at a time, rather than clearing
  everything at once and re-reading it.
- **Renaming does not re-read your photographs.** A rename, and an undo, move
  the pictures to their new names. Renaming four hundred files looks like
  renaming four hundred files, not like opening the folder for the first time.
- **Packaging.** The Windows executable carries its own icon and version
  metadata, links the C runtime statically so it starts on a machine without
  the Visual C++ Redistributable, and no longer opens a console behind itself
  in a release build. A tag-triggered workflow builds, tests, packages and
  drafts a release.
- **The icon**, drawn by `assets/icon.py` and committed with its generator.
- **Portable mode.** A marker file beside the executable moves presets, scripts,
  the undo journal and the window layout into a folder next to the program. The
  release zip ships it; deleting it restores the per-user behaviour. A folder
  that is not writable declines rather than failing on every write.
- **Six more Settings pages** — Casing Exceptions, Display, File System,
  Startup, Shell Integration and Problem Solver — bringing it to nine.
- **A right-click menu on the file list**: rename, show in file manager, copy
  names / new names / paths / both to the clipboard, and add to Free Select.
- **Configurable columns.** Seven of them — Name, New name, Size, Modified,
  Created, Ext, Folder — with order, width and visibility remembered. Plus row
  shading and click-anywhere-to-select.
- **A system-folder guard.** Renaming inside `C:\Windows`, Program Files, `/usr`
  or `/etc` is refused with a reason, and the guard can be switched off. It is
  the one mistake this program can make that undo does not fix.
- **Hidden, system and write-protected files are shown by default**, with
  switches to narrow the listing. Deliberately: a renamer that quietly leaves
  rows out is the more dangerous of the two.
- **Image tags** — `<Width> <Height> <Depth> <Depthb> <JpgComment>` — from the
  file's own header, so they answer for a PNG where every `<Exif-…>` is empty.
- **Folder tags** — `<DirSize> <DirFiles> <DirDirs> <FirstFileInFolder>` and the
  rest, at one level or through subfolders.
- **`<HtmlTitle>`**, so a folder of `Untitled-1.html` can be named from inside.
- **An About window**, with the version, the licence, and what this session
  renamed.
- **The rest of the hotkeys**: F4 undo, F6 focus the address box, F8
  settings, F12 folder browser, alongside the F2 / F5 / F9 already bound. F1 and
  Delete are deliberately not bound — see P64 and P65.
- **Tags in the Replace box**. A `$`
  arriving from a tag value is escaped and a `$1` the user typed is not (D115).
- **The New name column header** reorders the list by the names the run is about
  to write — once, as a command. As a sort mode it provably oscillates against
  an order-dependent counter (D119).

### Changed

- **Every keyboard shortcut works while a number box has focus.** Clicking or
  tabbing into *Delete:*, *from pos:*, the counter's Start value or any other
  numeric box silently disabled F2, F4, F5, F6, F8, F9, F12, Ctrl+Z and Ctrl+K
  until you clicked somewhere else — which is to say, they stopped working
  exactly when you had just finished setting up a card and reached for F5.
- **F9 (Refresh) now re-reads what is inside your files**, not only the folder.
  Names, sizes and dates always came from the directory and were always
  refreshed; the Exif date, the MP3 artist, an image's dimensions and its
  thumbnail are cached, and Refresh never touched them. A file edited by another
  program in a way that kept its size and timestamp went on showing its old
  values, and pressing Refresh confirmed the stale one rather than correcting it.
- **The file list offers eight columns** rather than seven — **Thumb** joins
  Name, New name, Size, Modified, Created, Ext and Folder. Turning it on makes
  every row taller, which is the honest cost and is why it is a switch.

### Fixed

- A folder and the files inside it can be renamed in **one run**. The folder
  sorted first, so it was renamed first and every rename underneath it failed on
  a path that no longer existed.
- A folder whose `<\>` target lands inside itself is now a blocked run with a
  reason, rather than an `EINVAL` half way through a batch (D118).
- **One unreadable folder no longer empties the listing.** It costs its own rows
  and no others, and both the app and the CLI say which (P63).
- Windows paths past 260 characters. Our own Win32 calls never took the `\\?\`
  prefix, so a deep file previewed clean and failed at the rename with a message
  about the folder being missing.
- `<ExifDate>` on a folder returned nothing on Windows: the size check meant to
  skip a file too small to hold Exif was being applied to a directory, which
  reports zero bytes there.
- *Copy current filename list into editor* is the identity again for a name
  containing a line break — such a row is refused rather than silently
  flattened.
- Sixteen further defects found by the M8 review sweep.

### Not built, on purpose

Each of these is a decision with its reasoning in `docs/DECISIONS.md`, not an
oversight:

- **Delete-to-recycle-bin** (P65) — the one destructive key whose worst case
  beats every rename bug this program can make.
- **F1 help** (P64) — every card already explains itself inline.
- **Movie and PDF tags** (P68, P69) — ffmpeg is LGPL and forbidden by our
  licence policy, and the PDF reader brings a crypto stack to read six fields.
- **A Send To shortcut** (D132) — needs COM plumbing nothing else here uses,
  for an entry the context menu already reaches from the same click.
- **An Advanced tab** (P71) — an INI file with a scrollbar. Every tweak on it
  that survives is a real setting on the page it belongs to.

## Earlier

Development ran as milestones M0–M7 rather than releases; there is no published
version before this one.
