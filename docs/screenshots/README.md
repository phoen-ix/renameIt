# Screenshots

Every screen of the app, one picture each. Each image is also a test:
`crates/ren-gui/tests/snapshots.rs` drives the real window to the screen and
fails when the render no longer matches the file here (D248). To regenerate
them, see [Screenshots](../../README.md#screenshots) in the README.

## The main window

A folder of holiday photos with two cards on the stack — Find & Replace turns
`_` into spaces, Set Casing makes the words Title Case — and the file list
showing each old name beside its new one, the changed part in green.

<img src="main-preview.png" alt="The main window: two operation cards on the left, the file list with old and new names on the right" width="800">

The same in the dark theme (Settings ▸ Appearance).

<img src="main-preview.dark.png" alt="The main window in the dark theme" width="800">

## An empty folder

A folder with nothing in it: the list reads *No files listed*.

<img src="main-empty.png" alt="The main window over an empty folder, with the message No files listed" width="800">

## A conflict

Two files would end up with the same name, so the whole run is blocked: the
status bar counts the conflicts and Rename is greyed out (hover it for the
reason).

<img src="main-conflict.png" alt="Two rows marked as conflicting and the Rename button disabled" width="800">

## After a run

Every file renamed; Undo is live and the log underneath lists what changed.

<img src="main-after-run.png" alt="The list after a run: the files carry their new names, Undo is enabled and the run log is open below" width="800">

## The row menu

Right-click a row: it becomes the selection, so the preview narrows to it, and
the menu renames it in place, shows it in the file manager, copies names or
paths, or moves the selection into Free Select.

<img src="main-row-menu.png" alt="The right-click menu open over a selected row of the file list, with Rename, Show in file manager, Copy to clipboard and Add to Free Select" width="800">

## Free Select

Files picked from two folders — what a drag from the file manager gives you.

<img src="main-free-select.png" alt="Free Select mode listing four files from two folders" width="800">

## The grid

**List | Grid** at the end of the source bar: a tile per file with its
picture, its name and the name the run will write. A long name is clipped, and
a file that is not a picture shows ⊘.

<img src="main-grid.png" alt="The grid view with picture tiles, their names and new names" width="800">

## Writing systems

The bundled fonts cover Japanese, Chinese, Korean, Thai, Devanagari, Arabic,
Hebrew, Greek and Cyrillic, plus monochrome emoji. Right-to-left names keep
their letters joined; their word order is the known gap (P95).

<img src="main-writing-systems.png" alt="File names in ten writing systems, each with a check mark added in front" width="800">

## A batch that did not finish

At start-up, after a crash or a power cut part-way through a run: the banner
names the file that was mid-change and offers **Roll back** or **Leave as is**.

<img src="main-recovery-banner.png" alt="The recovery banner above the source bar" width="800">

## Adding an operation

**+ Add operation** or **Ctrl+K**: every operation, in groups; typing narrows
the list — here *tags* leaves the Music operations and Free Format.

<img src="palette-add-operation.png" alt="The add-operation palette with tags typed in the search box, listing Music Rename, Music Tagger, Remove Tags and Free Format" width="800">

## Presets

**Presets ▾**: save the stack under a name, and load, append or run a saved
one. The six shipped presets are listed.

<img src="presets-drawer.png" alt="The preset drawer beside the file list" width="800">

## Music Rename

Names each track from its own tags — here *Artist - Title*.

<img src="card-music-rename.png" alt="A Music Rename card and four tracks renamed from their tags" width="800">

## A run that cannot be undone

Writing tags changes what is inside the files, so Rename asks first and names
every file.

<img src="dialog-irreversible.png" alt="The confirmation dialog: This run cannot be undone, with Leave everything alone and Change 2 item(s)" width="800">

## `<Ask>`

A pipeline with `<Ask>` in it asks for the text when you press Rename.

<img src="dialog-ask.png" alt="The Ask prompt with a text box" width="800">

## Visual Assist

The ⌖ button beside a position or find box (or **F3**) shows the text that
operation is handed; select part of it and press **Select** to fill the box.

<img src="visual-assist.png" alt="The Visual Assist strip on a Find &amp; Replace card: the name beach_at_sunset with _at_ selected" width="800">

## Settings

**F8** or ⚙. Each page:

### Batch Replace

The rules a new Batch Replace card starts from.

<img src="settings-batch-replace.png" alt="Settings, Batch Replace page" width="800">

### Music Styles

The patterns Music Rename offers.

<img src="settings-music-styles.png" alt="Settings, Music Styles page" width="800">

### Casing Exceptions

The words a new Set Casing card keeps in lower or upper case.

<img src="settings-casing-exceptions.png" alt="Settings, Casing Exceptions page" width="800">

### Display

Which columns the list shows, row shading and click target, thumbnail size.

<img src="settings-display.png" alt="Settings, Display page" width="800">

### File System

Hidden and system files, and the system-folder guard.

<img src="settings-file-system.png" alt="Settings, File System page" width="800">

### Startup

What a fresh start remembers.

<img src="settings-startup.png" alt="Settings, Startup page" width="800">

### Appearance

Theme and interface size.

<img src="settings-appearance.png" alt="Settings, Appearance page" width="800">

### Reset all settings

Settings ▸ Problem Solver ▸ **Reset all settings…** says everything it will
put back before it does.

<img src="dialog-reset.png" alt="The reset dialog listing what it restores" width="800">

Not pictured: **About** (it carries the version, so every release would change
it), **Shell Integration** (a Linux render shows the page's "nothing to
install" branch, not the Windows one) and **Problem Solver** (it prints the
machine's own home folder).
