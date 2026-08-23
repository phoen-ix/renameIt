# RenameIt

A batch file renamer written in Rust. Modern UI, cross-platform core, Windows
ships first. MIT licensed.

> **Status: 1.3.0.** Browse or drag in files, stack up as many operations as you
> like, watch the composed result preview as you type, save the stack as a
> preset and run it again later — or from the command line. One rename is one
> undo, however many operations produced it.
>
> It reads what is *inside* a file: name a track from its own tags, write tags
> back from the filename, strip tag blocks out, name a photograph from the
> moment it was taken. And when none of the eighteen operations fits, a
> **script** can do it — sandboxed, with nine worked examples shipped.
>
> Also here: the full hotkey set including list navigation and row dragging,
> nine Settings pages, the file-list menu, portable mode, **thumbnails** as both
> a grid and a column, **Visual Assist** on the four pattern fields, and the
> **Explorer preset menu** — a RenameIt submenu on files, folders, drives and
> folder backgrounds, with one item per preset. The Windows build carries its
> own icon and version metadata, links the C runtime statically, and a tag cuts
> a release.

## Workspace

| Crate | Role |
|---|---|
| `crates/ren-core` | The engine: operations, pipeline, planner, conflict detection, transactional executor, journal-backed undo. Zero GUI dependencies, no OS-specific code. |
| `crates/ren-platform` | `trait Platform` — renames, DOS attributes, file times, naming rules, case-sensitivity probing. The **only** place `cfg(windows)` may appear. |
| `crates/ren-cli` | Headless runner. The automation story and the test harness; ships in every release. |
| `crates/ren-gui` | The desktop app (egui/eframe). A stub until M2. |

## Building

Requires the toolchain pinned in `rust-toolchain.toml` (rustup installs it
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

Real work goes through a **job file** — a TOML listing plus an ordered pipeline.
This is the same schema presets will use, so anything you write here will load
into the GUI unchanged.

```sh
cargo run -p ren-cli -- preview --job examples/cleanup.toml   # never touches anything
cargo run -p ren-cli -- apply   --job examples/cleanup.toml
cargo run -p ren-cli -- undo
```

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
`csv_list`, `filename_editor`, `set_attributes`, `set_date`, `music_rename`,
`music_tagger`, `remove_tags`. Each step also takes
`scope` (`name` / `extension` / `both`), `enabled`, a `[step.filter]`
include/exclude filter and a `[step.preproc]` pre-processor. A `[settings]`
table carries the counter, the parts pattern and the tag policy, which belong to
the run rather than to any one step. Unknown keys are an error, not a silent
default.

`examples/numbered.toml` is the worked example for the counter, the `<tag>`
engine and `<\>`; `examples/photo-cleanup.toml` is a saved preset;
`examples/music-rename.toml` names tracks from their tags and files them into
`<Artist>/<Album>/` subfolders; `examples/camera-import.toml` names photographs
from their Exif date and then puts that date back on the file.

## Music and images

`<Artist>`, `<Title>`, `<Album>`, `<Year>`, `<Genre>`, `<Track>`, `<Comment>`
and about fifty `<ID3-*>` frame tags read from MP3, FLAC, Ogg Vorbis, Opus,
Speex, MP4/M4A, Musepack and WavPack — one set of names across every format.
`<Exif-Make>`, `<Exif-Model>` and any of the
161 fields the Exif reader knows, plus `<ExifDate>` and `<ExifTime>`. A folder
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
names the files; the CLI refuses without `--allow-irreversible`:

```bash
ren-cli apply ~/music --preset examples/music-rename.toml --allow-irreversible
```

Fields you have not ticked are left exactly as they are, cover art included.
That sounds obvious and is not: the tag library's save replaces the whole tag,
so writing only the artist wipes everything else unless the write reads first.

WMA and TTA are **not** supported — lofty handles neither. See D55 in
`docs/DECISIONS.md`.

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
cargo run -p ren-cli -- apply ~/photos --preset "Add prefix" --answer 0=Holiday
```

`--job` and `--preset` differ in exactly one way: a job file brings its own
source, a preset borrows yours. So `--preset` takes a folder and honours
`--pattern` / `--folders` / `--subfolders`, and `--job` takes neither.

## Tags

Any field that builds text takes tags — `<Name>`, `<Counter>`, `<Date-yyyy>`,
`<Parent>`, `<%1>`, `<Ask>`, `<Crc32>` and the rest of the list in
`docs/tags.md`. Two things to know:

* **A tag you mistype is an error, not an empty string.** The editor names it
  while you are typing. One silent typo across a thousand files is how a folder
  of music ends up named `.mp3`.
* **`<\>` moves a file into a subfolder** of the folder it is already in,
  creating it if needed — `<Date-yyyy><\><Name>` sorts photos into one folder
  per year. Undo puts the files back and removes the folders it created, but
  never one you have since put something else into. A produced name can go down;
  it can never go up or out.

There is also a one-operation shortcut for smoke tests:

```sh
cargo run -p ren-cli -- apply ~/some/dir --suffix _v2 --scope name
```

`--simulate` runs the whole plan and writes nothing. `--journal-dir` moves the
transaction log. `recover` reports transactions that never finished — after a
crash or a power cut — and `recover --rollback` reverts them.

```sh
cargo run -p ren-gui                                   # the app
xvfb-run -a cargo run -p ren-gui                       # on a headless box
cargo run --release -p ren-gui --example spike_preview_headless   # Spike B
```

The window: source bar on top, operation
panel on the left, a virtualized file table showing old and new names with the
changed part highlighted, and counts plus Simulate / Undo / Rename along the
bottom. The Rename button disables itself **with a reason** when the plan has
conflicts, because a conflict blocks the whole run rather than skipping a file.

`F2` renames one row, `F5` runs, `F9` re-lists, `Ctrl+Z` undoes.

## The quality gate

Everything below must be green before a session ends:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --lib --bins --target x86_64-pc-windows-msvc -- -D warnings
cargo test --workspace
TZ=Asia/Kolkata cargo test --workspace   # P61: a UTC agent cannot tell Local from Utc
cargo deny check
cargo bench -p ren-core --bench plan     # a 10k-file plan must stay under 50 ms
```

The Windows cross-check prints one build-script warning — the icon needs a
Windows host to embed. That is expected off Windows and is not a failure; the
release build runs on `windows-latest`, where it does not appear.

## Documentation

- `CLAUDE.md` — how to work in this repo
- `docs/DECISIONS.md` — **the authority** on every policy and technology call
- `docs/DESIGN.md` — architecture: engine model, GUI/UX screens, quality strategy
- `docs/tags.md` — the `<tag>` reference
- `docs/MIGRATION-legacy-scripts.md` — porting a legacy `.frs` script to Koto
- `docs/spikes/` — written verdicts on the risky assumptions
- `docs/manual-checks.md` — the few things CI cannot prove, and how to check them

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

## Releases

Push `main` first and **let CI go green**, then tag:
`git tag -a v1.3.0 -m "1.3.0" && git push origin v1.3.0` runs
`.github/workflows/release.yml`, which builds, packages
`RenameIt-windows-x64.zip` **and** `RenameIt-linux-x64.tar.gz`, and drafts a
GitHub release from `CHANGELOG.md`. Draft, not published: the tag is the act of
building, and deciding it is fit to hand out is a separate one. Each archive is
built from an explicit list rather than by sweeping a directory.

The order matters because the release **does not re-run the tests** — it checks
that CI already passed on the exact commit the tag points at, and refuses
otherwise. A tag does not trigger CI, so tagging before CI finishes is a
refusal rather than a wait: sleeping on a paid runner would cost the minutes
that check exists to save.

The application icon is ours, drawn by `assets/icon.py` and committed with its
generator so that stays checkable:

```sh
python3 assets/icon.py    # rewrites crates/ren-gui/assets/renameit.{ico,png,rgba}
```

## Licence

MIT — see `LICENSE`. Bundled fonts are OFL-1.1; see `THIRD-PARTY-FONTS.md`.
