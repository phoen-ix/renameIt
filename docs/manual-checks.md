# Manual checks

Things CI cannot prove, and what to do about them. Each entry says which
acceptance criterion it belongs to, so the record stays honest about what is
actually verified.

Everything else in this project is checked automatically: `cargo test` covers
the engine and, since M2, the GUI itself through `egui_kittest` (D24) — no
display, no GPU, both CI runners.

**Recording a result.** Each section ends with a **Results** line. When you
work through a section, replace *not yet recorded* with one line per run:
the date, the machine (Windows build, or distribution and filesystem), and
pass or fail with the row numbers that failed. A section with no result has
not been checked — say so rather than assume it.

---

## M2 — drag-and-drop from Explorer

**Why it is here.** winit delivers dropped files as ordinary egui `RawInput`,
so the plumbing behind the feature *is* tested — `dropped_files_populate_free_select`
injects a drop and asserts Free Select fills up. What cannot be tested from
Linux is Windows Explorer actually handing those paths over, and the edge cases
`docs/DESIGN.md` flags as risky.

**On a Windows machine, run `renameit.exe` and drag onto the window:**

| # | Drop this | Expect |
|---|---|---|
| 1 | A few files from one folder | Switches to Free Select, lists exactly those files, "N file(s) from 1 folder(s)" |
| 2 | Files from two different folders | All listed, "…from 2 folder(s)" |
| 3 | A **single folder**, while in Browser mode | Navigates into it rather than adding it as an item |
| 4 | A folder **and** files together | Switches to Free Select and takes all of them |
| 5 | A file whose full path is **longer than 260 characters** | Listed and renameable, no truncation and no error |
| 6 | A file from a **UNC path** (`\\server\share\…`) | Listed and renameable |
| 7 | The same file twice | Appears once |

**If anything fails**, note which row and open an issue — rows 5 and 6 are the
ones `docs/DESIGN.md` predicted would bite.

**Results:** *not yet recorded.*

## M8 — non-Latin filenames (writing systems)

`every_bundled_script_rasterises` proves a real face claims each character and
that epaint got an outline out of it. What it cannot prove is whether the result
*reads* — the harness never looks at pixels.

Make a folder with one file per row and open it:

| # | Name | Expect |
| --- | --- | --- |
| 1 | `日本語のファイル.txt`, `中文文件.txt`, `한국어.txt` | Legible, no boxes, and the row height unchanged from a Latin-only listing |
| 2 | `ไฟล์ภาษาไทย.txt` | Tone marks sit **above** their consonants, not beside them |
| 3 | `हिन्दी फ़ाइल.txt` | The `ि` matra appears *before* the consonant it follows in memory — that is the shaper reordering correctly |
| 4 | `مرحبا بالعالم.txt` | Letters **joined**, not isolated. The two words will be in the wrong order (**P95**) — that is the known bidi gap, not a font problem |
| 5 | `שלום עולם.txt` | Same: each word right, the words swapped |
| 6 | A name with an emoji | Monochrome, not colour (**P95**) |
| 7 | Click into the middle of the Hebrew name in an F2 rename box | Known-bad: epaint emits phantom zero-width glyphs for RTL runs, so the caret lands wrong. Worth confirming it does not *crash* |
| 8 | A file whose name is not valid UTF-8 — on Linux, `printf 'x' > $'caf\xe9.txt'` | Row shows an amber ⚠ and *name is not text*; the status line says one item was left alone; **F2 renames it** and Ctrl+Z puts the original bytes back |

Row 8 is the one that used to take the whole window down, so it is the one to
repeat after any change to the listing or the journal.

**Results:** *not yet recorded.*

## M2 — appearance at Windows DPI settings

> **Worth doing now, in a way it was not before.** The icon vocabulary used to
> be Unicode characters in labels, and thirteen of them had no glyph in any
> bundled font — so "does it clip at 150 %" could not be answered while the
> controls were drawing as `◻` at every scale. The ones that are controls are
> vector marks now (**P92**), scaled from a unit square, and the Settings
> window no longer resizes itself around whichever page is open. Both are the
> sort of thing only a real screen settles.


The headless tests assert behaviour, not pixels. Worth eyeballing once at
**125 %** and **150 %** scaling: the file table's columns, the status bar's
right-hand buttons, and the operation panel's radio groups should all still fit
without clipping.

**Results:** *not yet recorded.*

## M3 — tag behaviours worth eyeballing

Four readings recorded as policies. All four are implemented and tested, using
Free Format:

| # | Type this | Our answer (P#) |
|---|---|---|
| 1 | `<FLetter>` on a file named `7abc.txt` | nothing — a digit is not a letter (P28) |
| 2 | `<FLetterN2>` on `7abc.txt` | `#` (P28) |
| 3 | `<Mid-2>` on `abcdef.txt` | `cdef` — zero-based (P26) |
| 4 | `<Size>` on a 1536-byte file | `1.5KB` (P29) |
| 5 | Zero Padding to 2 digits on `Track 007` | `Track 07` — cropped from the left (P30) |
| 6 | Re-Number, all numbers, `[/] Divide by: 3`, on `File 10` | `File 3.333…` unrounded (P31) |

**Results:** *not yet recorded.*

## M3 — `<\>` Move To SubFolder

`<\>` is D31. Worth walking once:

| # | Try | Our answer |
|---|---|---|
| 1 | `<Date-yyyy><\><Name>` over a folder of photos | Creates one folder per year, moves each file into it |
| 2 | Then Undo | Files come back; the year folders are removed, because this run created them |
| 3 | Put an unrelated file into one of those folders, then Undo | That folder is kept, not deleted, and is listed as kept |
| 4 | `..<\><Name>` | Refused before anything is written — a target may never leave its folder |

**Results:** *not yet recorded.*

## M4 — the file pickers

The only thing in the pipeline UI that CI cannot reach. `rfd` opens a native
dialog, so it is injected behind a trait ([`ren_gui::dialogs::FileDialogs`]) and
the headless app is given a stub that answers nothing. The paths either side of
the dialog *are* tested — `import_preset_from` and `export_preset_to` take a
path and are driven directly — so what is left is the dialog itself.

**On a Windows machine:**

| # | Do this | Expect |
|---|---|---|
| 1 | Presets ▾ → Import… | A file dialog filtered to `*.toml` |
| 2 | Pick a preset exported from another machine | It appears in the list and loads |
| 3 | A preset's ⋮ → Export… | A save dialog, pre-filled with the preset's name plus `.toml` |
| 4 | The source bar's 📂 | A folder picker, starting in the folder you are browsing |

**Results:** *not yet recorded.*

## M4 — the card stack at Windows scaling

The cards are the first vertically-stacked resizable content in the app, and
the most likely thing to clip. Worth eyeballing once at **125 %** and **150 %**:
eight cards in the panel, one expanded, the ⋮ menu open, and the preset drawer
open beside the file table.

**Results:** *not yet recorded.*

## M8 — the Windows-only work, and who can confirm it

CI's `windows-latest` runner compiles and runs everything below, so "green"
means the code does what its tests say. What a runner **cannot** tell us is how
it behaves against a real user's machine: a drive that is not `C:`, a mapped
network share, a folder with `LongPathsEnabled` off, an install rather than a
`cargo run`. This is the list to work through on a real Windows box before each release.

| # | Do this | Expect |
|---|---|---|
| 1 | Rename a file whose full path is over 260 characters, on a folder deep in `Documents` | It renames. Before `wide_verbatim` this failed with *"the system cannot find the path specified"* — a message about the folder, for a problem with the length |
| 2 | The same on a **mapped network drive** and on a raw `\\server\share\…` path | The same. The UNC form is the branch a runner never exercises |
| 3 | The same with the machine's `LongPathsEnabled` registry value **off** (the default) | Still renames. That value is what a `longPathAware` manifest would have depended on, and the reason it was not the fix chosen |
| 4 | Set Attributes and Set Date on a file at that depth | Both succeed — they go through the same four Win32 calls |
| 5 | Reveal in Explorer on that file | Explorer opens and selects it. This is the one path deliberately **not** made verbatim, so it is the one to check has not regressed |

**Results:** *not yet recorded.*

## M8 — shell integration and portable mode

CI proves the registry keys round-trip, that every command string the menu
holds parses back into the flags it meant, and that the app opens on a path it
is handed. What it cannot show is Explorer actually drawing the cascade, how
many files it will carry, or a folder copied to a stick behaving like one.

**Check 3 is load-bearing.** Whether `MultiSelectModel = Player` is honoured for
a verb *inside* an `ExtendedSubCommandsKey` store is undocumented, and no CI can
settle it. If three files give three windows, **P82** withdraws the file tier —
forty photographs would be forty windows, which is the failure **D132** already
refused. Folders, drives and backgrounds are unaffected and stay either way.

| # | Do this | Expect |
|---|---|---|
| 1 | Settings ▸ Shell Integration, tick the box, then right-click a file, a folder, and a **mixed** selection of one of each | A **RenameIt** submenu with the app's icon: *Start from this folder*, *Start and load selected files*, *Copy filenames to clipboard*, a separator, then one item per preset in the order the drawer lists them. The mixed selection shows the same menu — that is what `AllFilesystemObjects` is for |
| 2 | Right-click the **empty space** inside a folder, and a **drive** in This PC | The background menu has *Start from this folder* and the presets, and **not** the two selection items. The drive has the full menu |
| 3 | **Select three files, choose *Start and load selected files*** | **One** window, listing all three. If it is three windows, stop and apply **P82**: the file tier is withdrawn |
| 4 | Select two files whose names contain spaces and use a preset item | Both arrive, unsplit — this is the bare `%1` under `Player`, the one formulation that cannot be safe under both readings of an undocumented behaviour |
| 5 | Save a preset called `Rock & Roll`, then open the menu | It reads *Rock & Roll*, with no underline and no missing character. Delete it and open the menu again without re-ticking anything: the item is gone |
| 6 | Move the program to another folder and start it once, then use the entry | It launches the *new* path. The startup rewrite is what retires the old "tick the box again" instruction — confirm it needed nothing from the user |
| 7 | Untick the box, and check `HKCU\Software\Classes` in `regedit` | `AllFilesystemObjects\shell\RenameIt`, `Drive\shell\RenameIt`, `Directory\Background\shell\RenameIt`, `RenameIt.ContextMenu` and `RenameIt.ContextMenu.Background` are all gone. Nothing needed an administrator either way |
| 8 | On **Windows 11**: right-click a file, then Shift+F10 | The entry is under *"Show more options"* on the short menu, and directly on the long one. This is what the Settings page warns about |
| 9 | Select ~30, then ~90, then ~150 files and open the menu each time | Note where the entry stops appearing. Windows hides it rather than shortening the selection — record the number in this file, because it is the only measurement of the 2000-character cap we will ever have |
| 10 | Select enough files to get near that number and use *Start and load selected files* | The banner appears, gives a character count, and *List the whole folder* relists the folder. Then select three files and confirm **no** banner |
| 11 | Use *Copy filenames to clipboard* on three files and paste into a text editor | Three names, one per line, sorted — no paths, and no window ever opened |
| 12 | Extract the release zip to a stick, run it, save a preset, then run it from another machine | The preset is there. `RenameIt-data` sits beside the executable and nothing was written to `%APPDATA%` |
| 13 | Extract the same zip into `C:\Program Files\` and run it | It starts, and Settings ▸ Problem Solver reports a **normal** install — the portable path is declined when the folder is not writable, rather than taken and then failing |
| 14 | Delete `renameit-portable.txt` from a portable copy and start it | A normal install: files in `%APPDATA%\RenameIt` |
| 15 | Right-click a file whose **path contains a space** and choose *Show in file manager*; then the same for a folder with a space in its path | Explorer opens the containing folder with the file selected, and the folder itself. The command line is `explorer.exe /select,"C:\My Folder\a.txt"` — switch bare, path quoted (`raw_arg`). Before the audit the whole argument was quoted as one string, which Explorer is known to mis-parse; this is the check no CI can run |

**Results:** *not yet recorded.* Check 3 decides **P82** and check 9 is the only
measurement of the selection cap: record both outcomes here, with the number of
files check 9 reached, and append a row to `docs/DECISIONS.md` closing P82
either way.

## M8 — Full Row Select

`egui_kittest` drives widgets through the accessibility tree, and this setting
is about clicking a cell that is deliberately **not** a widget — a plain size or
date label, with the row's click target behind it. There is no node to aim at,
so the test can only show that the setting reaches the table. Same class of gap
as the file dialogs and the clipboard.

```sh
xvfb-run -a cargo run -p ren-gui     # or just `cargo run -p ren-gui`
```

| # | Do this | Expect |
|---|---|---|
| 1 | Settings ▸ Display, tick *Click anywhere on a row to select it*, close | — |
| 2 | Click a row's **Size** or **Modified** cell | The row is selected, and the count at the bottom follows |
| 3 | Ctrl-click the same cell again | It is deselected — the same rule the name cell follows |
| 4 | Double-click the **name** of a row | The inline editor opens. The row target behind it must not have swallowed this |
| 5 | Untick it, click a Size cell again | Nothing is selected: only the name senses a click, which is the default |
| 6 | Tick *Shade every other row* | Alternate rows are faintly shaded, and a selected row still reads as selected |

**Results:** *not yet recorded.*

## M8 — the Batch Replace box beside *Add rule*

The colon-separated add is `rule_table::added_by`'s own unit test; what a
headless run cannot do is type into the box. A hint is a placeholder rather
than an accessible name, so there is no node for the harness to aim at.

| # | Do this | Expect |
|---|---|---|
| 1 | Settings ▸ Batch Replace, press **Add rule** with the box empty | One blank rule appears at the bottom, as it always did |
| 2 | Type `aa:bb:cc` in the box and press **Add rule** | Three rules, finding `aa`, `bb` and `cc`, and the box is cleared |
| 3 | Type `_` and press **Add rule** | One rule finding `_`, the box cleared |
| 4 | On a Find & Replace card, fill the find box and press **Add to Batch Replace** | The rule appears at the bottom of the Settings list. A Batch Replace card you already added is unchanged (D35) |

**Results:** *not yet recorded.*

## M8 — the thumbnail view

**Why it is here.** Everything about a thumbnail that a headless harness can
reach is already tested: which files are asked for, at which size, how many,
whether a refusal is remembered, whether a rename moves a tile rather than
re-decoding it. What it cannot reach is *the picture*. There is no GPU (D24),
there is no snapshot testing, and the only thing about a texture that appears in
the accessibility tree is its `alt_text`. So a tile that drew the wrong file's
picture, drew it upside down, or drew nothing at all would pass every test in
the suite.

A tempdir of about thirty photographs is the fixture: some portrait, some
landscape, at least one taken on a phone (for the Exif orientation), one very
large, and one text file renamed to `.jpg`.

| # | Do this | Expect |
|---|---|---|
| 1 | Open the folder, press **Grid** on the source bar | Tiles, each with its picture, its name, and what the run will write |
| 2 | Look at the phone photograph taken in portrait | Upright. If Exif orientation were dropped it would be on its side, and this is the single most visible thing this feature can get wrong |
| 3 | Compare each tile's picture with its name | They match. Nothing here proves that automatically — the cache is keyed by path, length and mtime, and a wrong key would show a plausible picture under the wrong name |
| 4 | Settings ▸ Display, drag the **Size** slider from end to end | Tiles resize smoothly. Between the three decode buckets the picture is scaled, never re-read — it must not flicker or blank while you drag |
| 5 | Tick **Draw a border around thumbnails** | A one-pixel border, tight to the picture rather than to the tile |
| 6 | Put a Find & Replace card on the stack | The second line under each tile is the new name, diffed, with the ⚠ if the extension changes |
| 7 | Narrow the row filter to **Changed** | Unchanged tiles disappear entirely, and the ones left still show their own pictures |
| 8 | Scroll fast to the bottom of a folder of several hundred | Tiles fill in behind you; the window stays responsive and the memory in Task Manager settles rather than climbing |
| 9 | While that is happening, type in a Find box | Keystrokes keep up. This is what D134's private thread pool is for — on rayon the preview would be behind the decodes |
| 10 | Look at the text file renamed `.jpg` | A ⊘ placeholder with "cannot be read" on hover, not a blank square and not a spinner |
| 11 | Rename all thirty (F5), then Undo (F4) | The pictures stay put through both. No blanking, no re-decode — that is D139's rekey, and a flicker here means it missed |
| 12 | Press **F9** | The folder is read again. Nothing visibly changes, which is the point; edit a photograph in another program first and it should be the edited one afterwards |
| 13 | Switch to **List**, tick the **Thumb** column in Settings ▸ Display | Every row grows to the tile size — that is the cost, and it should be obvious rather than surprising |
| 14 | Windows at 125% and 150% scaling | Tiles are sharp, not soft. The bucket above the drawn size is what makes this work, so a soft tile means the scale is not reaching `key_for` |
| 15 | Drop a 64 000 × 64 000 PNG in the folder | "too large to preview" on that one tile, promptly. The app stays responsive and memory does not spike |

**Results:** *not yet recorded.*

## M8 — the file list's pointer and caret

**Why it is here.** Everything the accessibility tree can reach is tested —
which row the keyboard is on, what a drag *does* once its indices are known,
what Enter and Backspace change. What it cannot reach is **the pointer**: a
synthetic click lands at a node's rect, and a drag is a press, a move and a
release across two rows. So the model half is CI's and the gesture half is
yours.

One line in particular has **no test at all and the suite stays green without
it**: `FileTable::row_ui` translates the dropped row to an *entry* index, and
with a row filter off the two numbers are equal. Check 4 is the only thing
standing between that and a drag that moves the wrong file.

| # | Do this | Expect |
|---|---|---|
| 1 | Press **F2** on `song.mp3` | The stem `song` is selected and the caret sits before the dot. Type `track` and press Enter: `track.mp3`, and the editor opens on the **next** row |
| 2 | Rename `a.txt` to `zz.txt` with F2 in a folder of `a b c` | The list re-sorts, and the editor opens on `b.txt` — the file that *was* next, wherever it landed |
| 3 | Drag a row by its middle, over another row's top half, then its bottom half | A line above and below respectively; dropping puts the file there, and an Add Counter card renumbers it in place |
| 4 | **Narrow the chip to Changed, then drag the last visible row onto the first** | The file you dragged moves. If a *different* one moves, the drop index is being taken from the visible row rather than the entry |
| 5 | Drag one of three selected rows | All three move, together and in order. Drag an **un**selected row: it moves alone and the selection is untouched |
| 6 | After a drag, look at the column headers | **No** header shows an arrow — the listing is in nobody's column order. Click *New name* instead and that header alone gets one |
| 7 | Drag a row, then press Rename | The order you set is the order that runs, and the list is still in it afterwards |
| 8 | Click a row, then Shift-click three rows down | Four rows selected, not two. Ctrl-click one of them: it drops out and the rest stay |
| 9 | Arrow up and down the list | A ring follows the keyboard, one row at a time, and scrolls into view at the ends. Hold Ctrl: the ring moves and the highlight does not |
| 10 | Click into a position spinner on a card, then press Down | The **number** changes and the list does not move. Then `Ctrl+A`: it selects the spinner's text, not every file |
| 11 | With **Folders** on, put the keyboard on a folder and press Enter, then Backspace | Down into it, then back out — landing on the folder you left |
| 12 | Press Backspace with **Folders off**, at a drive root, and in Free Select | Up one level; nothing at the root; nothing in Free Select |

**Results:** *not yet recorded.*

## M8 — Visual Assist

**Why it is here.** Everything about the strip that the accessibility tree can
reach is tested: which text it shows, what Select writes, what F3 cycles, what
survives a re-sort and what a run closes. What it cannot reach is **the mouse**.
Turning "characters 3 to 8" into pixel coordinates needs the galley's glyph
advances, which the tree does not usefully expose, so CI drives the selection
through the model and a human has to drive it with a pointer.

The other gap is sharper: the harness never lays out a glyph, so it cannot
notice that egui moves the caret one *character* at a time while a grapheme
cluster may be several. Check 3 is the only thing standing between that and a
user's filenames.

| # | Do this | Expect |
|---|---|---|
| 1 | Open a Find & Replace card, click **⌖ find**, drag across part of the name | The selection highlights, and `Position` / `Selection length` track the drag character by character |
| 2 | Click once with no drag | No caret is painted — a read-only field has none — so **Select** must be greyed out and say why on hover. If that reads as broken rather than as refused, the wording is wrong |
| 3 | A name with a **decomposed accent** (any file copied from a Mac), a **CJK character** and an **emoji outside the BMP**. Select across all three, press Select, run | The run cuts exactly where the strip said. This is the byte/character/grapheme check, and no headless test can make it: a decomposed `é` has two cursor positions at nearly the same x, so a selection can land between a letter and its accent |
| 4 | Double-click a word; triple-click the line | Both select, and the readout agrees with an equivalent drag |
| 5 | Type into the strip's field | Nothing happens, and the card's own fields are untouched |
| 6 | Click into the strip's field, press **F3** | The strip cycles or closes |
| 7 | Then click into a real **Find** box and press **F5** and **Ctrl+Z** | Neither fires — the guard still holds where it should (P80) |
| 8 | **Tab** to a ⌖ | It is reachable. Image buttons are exactly the kind of control a toolkit skips in the tab order, so this is the check that makes "the toolkit handles it" true rather than assumed |
| 9 | A very long filename in a narrow window | The field scrolls or wraps and the far end is reachable. It is a `multiline` field precisely so it does not clip |
| 10 | A folder of several thousand files | The picker opens promptly and its last line reads "the first 200 of N" |
| 11 | An Add & Remove card in **Both** mode | Two markers, and the Add one shows a *shorter* name than the Remove one. That difference is the feature working, not a glitch |
| 12 | Select to the end of a name, take **Anchor to the end**, then look at a file of a different length | The insertion lands at the end of both |
| 13 | Select `a*b` in a name on a Find card **without** *Regular expression* | Before Select, the strip names the `*` and says to tick *Regular expression* to match it literally (D236) |

**Results:** *not yet recorded.*

## Second audit (2026-09) — Windows

What the second audit changed on Windows that only a real machine can confirm.
CI compiles and unit-tests each of these under `cfg(windows)`; the rows are
about the machine around the code.

| # | Do this | Expect |
|---|---|---|
| 1 | Tag an MP3 that has a created date, the **Hidden** attribute and an alternate data stream (download one, so it carries `Zone.Identifier`), with Music Tagger | All three survive, the modified date is unchanged, and no `__renameit-tags-*` file is left beside it (D203, D217). CI's `replace_file_keeps_the_created_date_on_windows` covers the created date only |
| 2 | The same on a **read-only** MP3 | Refused with *this file cannot be written*; the file is untouched and no copy is left |
| 3 | `attrib +P` a file (OneDrive *Always keep on this device*), then Set Attributes → Hidden, then Undo | `attrib` still shows `P` after both (D215) |
| 4 | Explorer menu on a **drive root** (`E:\`): *Start from this folder* on the drive's background, and *Start and load selected files* on a selection spanning two drives | The app opens on `E:\` and lists the selection — not on nothing (D211) |
| 5 | A `.bat` beside some files containing `ren-cli /p "%~dp0" /r "Photo cleanup"` | It renames them: the switches after the quoted folder are kept (D211) |
| 6 | In Windows PowerShell 5.1: `Get-ChildItem *.jpg \| % FullName > list.txt`, then `ren-cli /l list.txt` | The files are listed. The file is UTF-16 with a byte-order mark; `Set-Content` (ANSI) must work as well (D212) |
| 7 | With **Narrator** and then **NVDA** running, Tab through a card, a file row and a ⌖ button | Each is read with its name, not as *button* or silence (D218) |
| 8 | About ▸ the repository link | The browser opens it (D218) |
| 9 | Start an undo of a large batch on a network share, then close the window — and try **Alt+F4**. Repeat with the window minimised, closing it from the taskbar | The window stays until the undo finishes, then closes, minimised or not; Alt+F4 never starts an undo (D220, D222, D245) |
| 10 | Start a long run in one RenameIt window, then open a second | The second window's banner says *a run is still going on in another RenameIt window* and offers no Roll back for it (D175, D228) |
| 11 | `ren-cli apply C:\Windows\Temp --suffix _x` from an elevated prompt | Exit 2, naming the folder; nothing renamed (D200) |

**Results:** *not yet recorded.*

## Second audit (2026-09) — Linux

Linux ships as an alpha archive, and these are the things only a real mount, a
real desktop and a real screen reader show.

| # | Do this | Expect |
|---|---|---|
| 1 | Mount a FAT image (`mkfs.vfat` a file, `mount -o loop`), open it in the app, and preview a Free Format of `a:b` | The row is refused as an invalid name (D162) |
| 2 | Start the app **first**, then mount the image, and within two seconds preview the same | The same refusal — the mount table is re-read (D204) |
| 3 | On a vfat or exFAT stick, rename `IMG.JPG` → `IMG.jpg` (F2, or Set Casing lower on the extension) | `ls` shows `IMG.jpg`. Before D202 the run reported success and the name stayed |
| 4 | Right-click a row, *Show in file manager*, with a path containing a space | The desktop's file manager opens the folder (xdg-open) |
| 5 | With **Orca** running, Tab through a card, a file row and a ⌖ button | Each is read with its name (D218) |
| 6 | Music Tagger on a group-writable file owned by another user | It is tagged; the owner becomes you — the documented cost of the copy-and-swap (D217) |
| 7 | `sudo ren-cli preview /etc` and `ren-cli preview /home/you/../../usr` | Both exit 2, naming the folder (D200) |

**Results:** *not yet recorded.*

## Second audit (2026-09) — the GUI by eye

| # | Do this | Expect |
|---|---|---|
| 1 | Light theme: select a few rows, with *Shade every other row* on | A navy bar at the left of each selected row and tile, clearly distinct from a stripe (P113) |
| 2 | **Tab** onto a card header | A focus outline in the selection colour |
| 3 | The grid over a folder of long Free Format names, scrolling | Columns stay even; a long name is clipped, not widening its tile |
| 4 | The Filename Editor over a folder of CJK names | The glyphs, not ◻ (D233) |
| 5 | Type a date into Set Date digit by digit, e.g. `2024-02-30` then correct it | What you type stays while you type; nothing snaps back or turns into another date |
| 6 | Run a preset with `<Ask>` and type at once, then press Enter | The prompt had the keyboard, and Enter renames |
| 7 | In Free Select, select two rows and press **Delete**; then right-click a row | They leave the list, the files stay on disk, and the menu offers *Remove from Free Select* (P111) |

**Results:** *not yet recorded.*
