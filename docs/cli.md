# Command line and file locations

The reference for `ren-cli`, its exit codes and legacy switches, and the
folders RenameIt keeps its files in. The [README](../README.md) has the
overview; `ren-cli --help` prints the exit codes and legacy switches too.

## ren-cli

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

## Exit codes

| Code | Means | Next step |
|---|---|---|
| `0` | Done. An undo that could not put back a tag write still exits 0 and says so: nothing can put one back. | — |
| `1` | The command line was wrong, or something failed before any work started. Never used once files have moved. | Fix the command. |
| `2` | Refused before anything was touched: conflicts or errors in the plan, a system folder, an irreversible change without `--allow-irreversible`, a path that is not absolute, or a journal another run is writing. | Fix the cause and run it again. |
| `3` | The run started and some files did not make it, or an undo or rollback could not put everything back. | `ren-cli recover`. |

## Legacy switches

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

## Where RenameIt keeps its files

| What | Windows | macOS | Linux | Portable copy |
|---|---|---|---|---|
| Undo journal | `%APPDATA%\RenameIt\journal` | `~/Library/Application Support/RenameIt/journal` | `$XDG_DATA_HOME/RenameIt/journal`, else `~/.local/share/RenameIt/journal` | `RenameIt-data\journal` beside the program |
| Presets | `…\RenameIt\presets` | `…/RenameIt/presets` | `…/RenameIt/presets` | `RenameIt-data\presets` |
| Scripts | `…\RenameIt\scripts` | `…/RenameIt/scripts` | `…/RenameIt/scripts` | `RenameIt-data\scripts` |
| Settings and window layout | `%APPDATA%\RenameIt\data\app.ron` | `~/Library/Application Support/RenameIt/app.ron` | `~/.local/share/renameit/app.ron` (lower case) | `RenameIt-data\app-state.ron` |
| Settings a newer version wrote, kept aside | `%APPDATA%\RenameIt\app-state.unreadable-<seconds>.ron` | `…/RenameIt/app-state.unreadable-<seconds>.ron` | `…/RenameIt/app-state.unreadable-<seconds>.ron` | `RenameIt-data\app-state.unreadable-<seconds>.ron` |

An installed copy uses the per-user folders. The program itself is in
`%LOCALAPPDATA%\Programs\RenameIt`, or `%ProgramFiles%\RenameIt` when installed
for everyone, and the installer leaves the portable marker out (D249). The
updater downloads into `%TEMP%\RenameIt-update-*`; folders older than a day are
swept on the next download, and the uninstaller removes the rest.

Settings ▸ Where files are kept shows which of these this copy uses, with buttons to
open them. `ren-cli` uses the same folders, so `ren-cli undo` can undo a batch
the app ran.
