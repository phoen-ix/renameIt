# RenameIt

A batch file renamer written in Rust. Modern UI, cross-platform core, Windows
ships first. MIT licensed.

> **Status: 1.4.0**, with the second audit's fixes on `main` (see
> [`CHANGELOG.md`](CHANGELOG.md), *Unreleased*). Browse or drag in files, stack
> up as many operations as you like, watch the composed result preview as you
> type, save the stack as a preset and run it again later — or from the command
> line. One rename is one undo, however many operations produced it.
>
> It reads what is *inside* a file: name a track from its own tags, write tags
> back from the filename, strip tag blocks out, name a photograph from the
> moment it was taken. And when none of the seventeen built-in operations fits,
> the eighteenth — a **script** — can do it: sandboxed, with nine worked
> examples shipped.
>
> Also here: the full hotkey set including list navigation and row dragging,
> nine Settings pages, the file-list menu, portable mode, **thumbnails** as both
> a grid and a column, **Visual Assist** on the four pattern fields, screen-reader
> support, and the **Explorer preset menu** — a RenameIt submenu on files,
> folders, drives and folder backgrounds, with one item per preset. The Windows
> build carries its own icon and version metadata, links the C runtime
> statically, and a tag cuts a release.

## Workspace

| Crate | Role |
|---|---|
| `crates/ren-core` | The engine: operations, pipeline, planner, conflict detection, transactional executor, journal-backed undo, metadata readers, scripting. No GUI dependencies and no OS behaviour; its only platform-conditional code is std's lossless path encoding in the journal (D240). |
| `crates/ren-platform` | `trait Platform` — renames, DOS attributes, file times, naming rules, the system-folder guard, shell integration. Everything OS-specific lives here (D3). |
| `crates/ren-cli` | Headless runner. The automation story and the test harness; ships in every release. |
| `crates/ren-gui` | The desktop app (egui/eframe). |

`docs/DESIGN.md` Part 4 has a module map of each crate and the app's worker
threads.

## Building

Requires the toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml) (rustup installs it
automatically). On Linux the GUI additionally needs:

```sh
sudo apt-get install -y pkg-config libx11-dev libxcursor-dev libxrandr-dev \
  libxi-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libgl1-mesa-dev
```

```sh
cargo build --workspace
cargo test  --workspace
```

## Trying it

Try it on a scratch folder, never on the repository — a pipeline renames
whatever folder it is pointed at. From the repository root:

```sh
mkdir -p /tmp/try && touch '/tmp/try/01 - my_first file.txt' /tmp/try/another_file_name.txt
cargo run -p ren-cli -- preview /tmp/try --preset examples/cleanup.toml   # never touches anything
cargo run -p ren-cli -- apply   /tmp/try --preset examples/cleanup.toml
cargo run -p ren-cli -- undo
```

`preview` prints each rename (`01 - my_first file.txt  ->  01. My First File.txt`)
and a count; `apply` does it and names the transaction; `undo` puts it back, from
any folder.

`examples/cleanup.toml` is a **job file** — a TOML listing plus an ordered
pipeline — used here as a preset: `--preset` runs its pipeline over the folder
you name and says it ignored the file's own `[source]`. Run as a job
(`--job examples/cleanup.toml`), it renames its own `dir = "."`, which is read
relative to the directory you run the command from — the repository, if you
run it there.

The schema is the one presets use, so anything you write here loads into the
GUI unchanged:

```toml
[[step]]
op = "replace"
scope = "name"
find = "_"
replace = " "

[[step]]
op = "casing"
mode = "title"
lowercase_exceptions = true
```

Operations: `replace`, `batch_replace`, `casing`, `add_remove`, `move_section`,
`space_trim`, `add_counter`, `renumber`, `zero_padding`, `free_format`,
`csv_list`, `filename_editor`, `script`, `set_attributes`, `set_date`,
`music_rename`, `music_tagger`, `remove_tags`. Each step also takes
`scope` (`name` / `extension` / `both`), `enabled`, a `[step.filter]`
include/exclude filter and a `[step.preproc]` pre-processor. A `[settings]`
table carries the counter, the parts pattern and the tag policy, which belong to
the run rather than to any one step. Unknown keys are an error, not a silent
default.

[`examples/numbered.toml`](examples/numbered.toml) is the worked example for the counter, the `<tag>`
engine and `<\>`; [`examples/photo-cleanup.toml`](examples/photo-cleanup.toml) is a saved preset;
[`examples/music-rename.toml`](examples/music-rename.toml) names tracks from their tags and files them into
`<Artist>/<Album>/` subfolders; [`examples/camera-import.toml`](examples/camera-import.toml) names photographs
from their Exif date and then puts that date back on the file.

## Music and images

`<Artist>`, `<Title>`, `<Album>`, `<Year>`, `<Genre>`, `<Track>`, `<Comment>`
and about fifty `<ID3-*>` frame tags read from MP3, FLAC, Ogg Vorbis, Opus,
Speex, MP4/M4A, Musepack and WavPack — one set of names across every format.
`<Exif-Make>`, `<Exif-Model>` and the rest of the 150 field names the Exif
standard gives for the primary image, Exif and GPS blocks — a name outside them
is an error, not an empty value — plus `<ExifDate>` and `<ExifTime>`. A folder
takes its tags from the first music file inside it.

`<Width>`, `<Height>`, `<Depth>`, `<Depthb>` and `<JpgComment>` read the image's
own header rather than a camera's Exif block, so they answer for a PNG or a BMP
where every `<Exif-…>` returns nothing — BMP, GIF, JPEG, PNG, TIFF and TGA.

The same six formats get **thumbnails**, and you choose how: `List | Grid` at
the end of the source bar switches the whole view to tiles, and a **Thumb**
column in Settings ▸ Display puts a small one beside each row instead. A tile
carries the picture, the name on disk and the name the run is about to write,
so the grid is somewhere you can work rather than only somewhere you can look.
Photographs come back upright whatever your phone wrote in their Exif; only
what is on screen is ever decoded, so a folder of ten thousand costs a
screenful of work; and renaming them moves the pictures rather than reading
them again.

Two operations change what is *inside* a file rather than its name — **Music
Tagger** writes tags from the filename, **Remove Tags** strips ID3v1, ID3v2 and
Lyrics3 blocks — and neither can be undone. The GUI asks before running one and
names the files; the CLI refuses with exit 2 unless given
`--allow-irreversible`. A preset with one such step:

```toml
version = 1

[preset]
name = "Strip ID3v1"

[[step]]
op = "remove_tags"
id3v1 = true
```

```sh
ren-cli apply ~/music --preset strip-v1.toml                        # refused, exit 2
ren-cli apply ~/music --preset strip-v1.toml --allow-irreversible
```

Fields you have not ticked are left exactly as they are, cover art included.
That sounds obvious and is not: the tag library's save replaces the whole tag,
so writing only the artist wipes everything else unless the write reads first.
And the file itself is never written: the change goes into a copy beside it,
which replaces it only when complete, keeping its modified date — a full disk
or a pulled USB stick part-way leaves the file as it was.

WMA and TTA are **not** supported — lofty handles neither. See D55 in
[`docs/DECISIONS.md`](docs/DECISIONS.md).

## Picking positions by eye

Operations that work by position — Add, Remove, Move Section — and Find &
Replace's *Look For* box each carry a ⌖ button, and **F3** opens the same thing.
It shows the text that operation is handed for one of your files; select part of
it and the boxes fill in.

The text it shows is the operation's own input, which is not always the file
name: a card further down the stack sees what the cards above it produced, and a
card set to *Process: Name* sees the stem without its extension. That is the
whole point — a position measured against the wrong string is wrong by exactly
the difference.

## Presets

A preset is this same file with a name and no `[source]` — it runs over whatever
folder you point it at. The app saves them one file per preset in the per-user
data directory; the CLI can list, show, import and export them:

```sh
cargo run -p ren-cli -- presets import examples/photo-cleanup.toml
cargo run -p ren-cli -- presets list
cargo run -p ren-cli -- apply ~/photos --preset "Photo cleanup"
cargo run -p ren-cli -- apply ~/photos --preset "Add prefix to filename" --answer "0=Holiday - "
```

`--preset` takes a path to a `.toml` file, or the **whole** name of a preset in
the preset folder, ignoring case. The six default presets — *Add prefix to
filename* is one — are written the first time the app starts; a machine that
has only ever run `ren-cli` has none until you import one. `ren-cli presets
list` shows what is there.

`--job` and `--preset` differ in exactly one way: a job file brings its own
source, a preset borrows yours. So `--preset` takes a folder and honours
`--pattern` / `--folders` / `--subfolders`, and `--job` refuses them.

## Tags

Any field that builds text takes tags — `<Name>`, `<Counter>`, `<Date-yyyy>`,
`<Parent>`, `<%1>`, `<Ask>`, `<Crc32>` and the rest of the list in
[`docs/tags.md`](docs/tags.md). Two things to know:

* **A tag you mistype is an error, not an empty string.** The editor names it
  while you are typing. One silent typo across a thousand files is how a folder
  of music ends up named `.mp3`.
* **`<\>` moves a file into a subfolder** of the folder it is already in,
  creating it if needed — `<Date-yyyy><\><Name>` sorts photos into one folder
  per year. Undo puts the files back and removes the folders it created, but
  never one you have since put something else into. A produced name can go down;
  it can never go up or out.

## The app

```sh
cargo run -p ren-gui                                   # the app
xvfb-run -a cargo run -p ren-gui                       # on a headless box
```

The window: source bar on top, operation panel on the left, a virtualized file
table showing old and new names with the changed part highlighted, and counts
plus Simulate / Undo / Rename along the bottom. The Rename button disables
itself **with a reason** when the plan has conflicts, because a conflict blocks
the whole run rather than skipping a file.

### Launching it

```text
renameit [PATH]... [--start-in] [--preset FILE] [--copy-names]
```

| Argument | Does |
|---|---|
| `PATH`… | Files or folders to open. One folder browses there; anything else is Free Select, the same reading a drag gets. |
| `--start-in` | Browse the folder the first path is in (a folder browses itself). |
| `--preset FILE` | Load a saved pipeline before the preview. A **path**, never a name: two presets may share one. The preview is shown; nothing is renamed. |
| `--copy-names` | Put the names of the given files on the clipboard, one per line, sorted, and exit without a window. |
| `--from-shell` | Written by the Explorer menu; nobody needs to type it. |

The parser never fails: anything it does not understand is read as a path, and
the status line says so. A release build has no console, so `renameit --help`
prints to nowhere on Windows — this table and `ren-cli --help` are the
documented places.

### Keyboard

| Key | Does |
|---|---|
| **F2** | Rename the row under the keyboard in place. The stem is selected; **Enter** renames and moves to the next row, **Escape** cancels. Honours Simulate. Refused while a run is going or the list is updating; a rename that fails says so in the status line. |
| **F3** | Visual Assist: open, cycle through the card's ⌖ targets, close. |
| **F4** or **Ctrl+Z** | Undo the last batch. |
| **F5** | Rename (run the plan). |
| **F6** | Put the keyboard in the address box (Browser mode). |
| **F8** | Settings. |
| **F9** | Forget everything read from inside the files, then list the folder again — the same as the ⟳ button. |
| **F12** | Pick a folder (Browser mode). |
| **Ctrl+K** | The add-operation palette. |
| **↑ ↓ Home End** | Move through the list. **Shift** extends the selection; **Ctrl** moves the keyboard and leaves the selection alone. |
| **Ctrl+A** | Select every row on screen. |
| **Enter** / **Backspace** | Into the folder under the keyboard / up to the parent (Browser mode; Enter needs the Folders chip). |
| **Delete** | Free Select only: take the selected rows out of the list. The files are untouched; nothing in the app deletes from disk. |
| **Ctrl +** / **Ctrl −** / **Ctrl 0** | Interface size (also in Settings ▸ Appearance). |
| **Enter** in the `<Ask>` prompt | Answer and rename. |

Three rules decide when a key acts. **Exact modifiers:** Ctrl+Shift+Z is not
undo, and Alt+F4 is not F4. **Not while you type:** in a text box the keys are
the box's own — except F3, and except the F-keys in a number box (Ctrl+Z there
undoes the digit). **Not behind a dialog:** with Settings, About, the palette,
the `<Ask>` prompt or a confirmation open, no shortcut acts. F1 is not bound.

Closing the window while a run, an undo or a rollback is going waits for it to
finish; Cancel stops a run sooner, between two files.

## Command line

```text
ren-cli [--journal-dir DIR] [--verbose] <preview | apply | undo | recover | presets> …
```

`ren-cli --help` and `ren-cli <command> --help` are the full reference; this is
the map.

| Command | Does |
|---|---|
| `preview [DIR]` | Show what would change. Touches nothing, never asks. |
| `apply [DIR]` | Rename, recording a journal so the batch can be undone. |
| `undo [--txn FILE]` | Revert the most recent transaction, or the one in `FILE`. |
| `recover [--rollback]` | List transactions that never finished; `--rollback` reverts them. |
| `presets list \| show NAME \| import FILE \| export NAME TO [--force]` | The preset folder. `export` will not replace an existing `TO` without `--force`. |

`preview` and `apply` take the same options:

| Group | Flag | Means |
|---|---|---|
| Pipeline | `--job FILE` | A job file: its own source and pipeline. Takes no source or listing flag. |
| | `--preset NAME\|FILE` | A preset, by whole name or path, over `DIR` or a list. |
| | `--suffix TEXT`, `--scope name\|extension\|both` | A one-operation smoke test: append `TEXT`. Only without `--job`/`--preset`. |
| | `--answer SLOT=TEXT` | Answer `<Ask>` (slot 0) or `<Ask-1>`…`<Ask-9>` up front. A slot the pipeline does not ask for is refused. |
| Source | `DIR` | The folder to list. |
| | `--list FILE` | A text file of paths, one per line (`#` comments and blank lines skipped). UTF-8, UTF-16 and the Windows code page are all read. |
| | `--file PATH` | One path; repeatable. |
| | `--delete-list` | Delete the `--list` file after a successful command (not under `--simulate`). |
| Listing | `--pattern MASK` | `*.mp3`; empty, `*` and `*.*` mean everything. |
| | `--folders`, `--no-files`, `--subfolders` | What the listing includes. |
| Run | `--simulate` | (`apply`) Plan everything, touch nothing. |
| | `--allow-irreversible` | (`apply`) Allow tag writes and removal, which cannot be undone. |
| | `--allow-system-folders` | Rename inside `C:\Windows`, the program folders, `/usr`, `/etc` and the like, which is otherwise refused. |
| | `--seed N` | Fix `<Rnd*>` and `fr.seed`, to reproduce a run. |
| Folders | `--preset-dir DIR`, `--script-dir DIR`, `--journal-dir DIR` | Where presets, scripts and journals live, instead of the per-user folder. |

`apply` asks on stdin for any `<Ask>` not answered with `--answer`, and fails if
stdin closes first.

### Exit codes

| Code | Means | Next step |
|---|---|---|
| `0` | Done. An undo that could not put back a tag write still exits 0 and says so: nothing can put one back. | — |
| `1` | The command line was wrong, or something failed before any work started. Never used once files have moved. | Fix the command. |
| `2` | Refused before anything was touched: conflicts or errors in the plan, a system folder, an irreversible change without `--allow-irreversible`, a path that is not absolute, or a journal another run is writing. | Fix the cause and run it again. |
| `3` | The run started and some files did not make it, or an undo or rollback could not put everything back. | `ren-cli recover`. |

### Legacy switches

A command line written with slash switches is translated before it is parsed,
so an existing batch file keeps working. `--verbose` prints what it became.

| Switch | Becomes |
|---|---|
| `/p PATH` | The folder to list. `PATH\*.ext` lists matching names; a path to a file loads just that file; a path that does not exist fails and names it. |
| `/l FILE` | `--list FILE`. Wins over `/p`. |
| `/r NAME` | `--preset NAME`, and the line becomes `apply`. **Without `/r` it only previews.** |
| `/f`, `/d` | Include files / folders. `/d` alone lists folders and not files. |
| `/s` | `--subfolders`. |
| `/k` | `--delete-list`: the list is deleted after a successful run. |
| `/x` | Accepted and ignored — there is no window to keep open. |
| `/?` | The help. |

```bat
ren-cli /p "C:\Photos\*.jpg" /r "Photo cleanup"
```

## If a run was interrupted

Every rename is written to a journal before it happens, so a crash, a power cut
or a killed process leaves a record of exactly what was under way.

* **In the app**, a banner appears at the next start: *N batches did not
  finish*. It names each file that was mid-change. **Roll back** puts back what
  those batches did; **Leave as is** hides the banner and keeps the journals. A
  journal another RenameIt window is still writing is named as running and is
  never offered for rollback.
* **From the command line**, `ren-cli recover` lists the same, and
  `ren-cli recover --rollback` reverts them. An `apply` that exits 3 is the cue.
* **Cancel** during a run is not an interruption: it stops between two files,
  the run is recorded as finished, and Undo takes back exactly what happened.
* **What rollback and undo cannot restore:** a tag write or tag removal. Those
  go through a copy that is swapped in whole, so a file is either as it was or
  fully changed — never half-written — but the old tags are gone. A file whose
  state no longer matches the journal (someone renamed or edited it since) is
  **skipped** and listed, never forced. A date is compared with two seconds'
  tolerance, because FAT stores times to two seconds.

## Where RenameIt keeps its files

| What | Windows | macOS | Linux | Portable copy |
|---|---|---|---|---|
| Undo journal | `%APPDATA%\RenameIt\journal` | `~/Library/Application Support/RenameIt/journal` | `$XDG_DATA_HOME/RenameIt/journal`, else `~/.local/share/RenameIt/journal` | `RenameIt-data\journal` beside the program |
| Presets | `…\RenameIt\presets` | `…/RenameIt/presets` | `…/RenameIt/presets` | `RenameIt-data\presets` |
| Scripts | `…\RenameIt\scripts` | `…/RenameIt/scripts` | `…/RenameIt/scripts` | `RenameIt-data\scripts` |
| Settings and window layout | `%APPDATA%\RenameIt\data\app.ron` | `~/Library/Application Support/RenameIt/app.ron` | `~/.local/share/renameit/app.ron` (lower case) | `RenameIt-data\app-state.ron` |
| Settings a newer version wrote, kept aside | `%APPDATA%\RenameIt\app-state.unreadable-<seconds>.ron` | `…/RenameIt/app-state.unreadable-<seconds>.ron` | `…/RenameIt/app-state.unreadable-<seconds>.ron` | `RenameIt-data\app-state.unreadable-<seconds>.ron` |

Settings ▸ Problem Solver shows which of these this copy uses, with buttons to
open them. `ren-cli` uses the same folders, so `ren-cli undo` can undo a batch
the app ran.

## Portable mode

A copy that keeps everything beside itself. Put a file called
`renameit-portable.txt` next to the executable and presets, scripts, the undo
journal and the window layout all move into `RenameIt-data/` in that folder —
so copying the folder to a stick takes them with it. The release zip ships the
marker already; delete it to get the ordinary per-user behaviour.

Two conditions, both required: the marker has to be there, and its folder has to
be **writable**. A copy in `Program Files` or on a read-only share declines the
portable path rather than taking it and failing on every write. Settings ▸
Problem Solver says which mode you are in and where the files actually are.

## The quality gate

Everything below must be green before a session ends — the same list as
[`CLAUDE.md`](CLAUDE.md):

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

`plan_budget` is the number of record and fails above its budget; `cargo bench`
asserts nothing and is there for its statistics (D165).

The Windows cross-check prints one build-script warning — the icon needs a
Windows host to embed. That is expected off Windows and is not a failure; the
release build runs on `windows-latest`, where it does not appear.

## Documentation

- [`CLAUDE.md`](CLAUDE.md) — how to work in this repo
- [`docs/DECISIONS.md`](docs/DECISIONS.md) — **the authority** on every policy and technology call
- [`docs/DESIGN.md`](docs/DESIGN.md) — architecture: engine model, GUI/UX screens, quality strategy, and Part 4, the app as built
- [`docs/tags.md`](docs/tags.md) — the `<tag>` reference
- [`docs/MIGRATION-legacy-scripts.md`](docs/MIGRATION-legacy-scripts.md) — porting a legacy `.frs` script to Koto
- [`docs/spikes/`](docs/spikes/) — written verdicts on the risky assumptions
- [`docs/manual-checks.md`](docs/manual-checks.md) — the few things CI cannot prove, and how to check them

## Releases

Push `main` first and **let CI go green**, then tag:
`git tag -a v1.4.0 -m "1.4.0" && git push origin v1.4.0` runs
[`.github/workflows/release.yml`](.github/workflows/release.yml), which builds, packages
`RenameIt-windows-x64.zip` **and** `RenameIt-linux-x64.tar.gz`, and drafts a
GitHub release whose notes are the newest section of [`CHANGELOG.md`](CHANGELOG.md). Draft, not
published: the tag is the act of building, and deciding it is fit to hand out
is a separate one. Each archive is built from an explicit list rather than by
sweeping a directory, and carries the licence notices of everything the
programs link (`THIRD-PARTY-LICENSES.html`, generated by
[`cargo-about`](about.toml)), the font notices, and the two docs the app points
at.

The order matters because the release **does not re-run the tests** — it checks
that CI already passed on the exact commit the tag points at, and refuses
otherwise. A tag does not trigger CI, so tagging before CI finishes is a
refusal rather than a wait: sleeping on a paid runner would cost the minutes
that check exists to save.

The application icon is ours, drawn by [`assets/icon.py`](assets/icon.py) and committed with its
generator so that stays checkable. It needs Pillow:

```sh
python3 assets/icon.py    # rewrites crates/ren-gui/assets/renameit.ico, renameit-256.png and renameit-64.rgba
```

## Licence

MIT — see [`LICENSE`](LICENSE). Bundled fonts are OFL-1.1, and egui's own under
their licences; see [`THIRD-PARTY-FONTS.md`](THIRD-PARTY-FONTS.md). A release
archive lists every linked crate's licence in `THIRD-PARTY-LICENSES.html`.
