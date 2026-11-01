# Packaging (Windows)

`renameit.nsi` builds `RenameIt-Setup-<version>.exe`, the installer (**D249**). The
release workflow builds it on every tag, and RenameIt's updater downloads it (**D250**).

```powershell
pwsh packaging/stage-windows.ps1 -Notices <THIRD-PARTY-LICENSES.html>   # -> dist/RenameIt
makensis /WX /V2 /DVERSION=1.7.0 "/DSTAGE=$PWD\dist\RenameIt" "/DOUTDIR=$PWD\dist" packaging\renameit.nsi
```

`stage-windows.ps1` is the one list of what a Windows release carries. The zip is that
folder. The installer installs it, minus `renameit-portable.txt`.

## Switches

| Switch | Meaning |
|---|---|
| `/S` | Silent. Every message box has a silent answer, so a silent run never waits on a dialog. |
| `/CurrentUser` | Install for the current user, under `%LOCALAPPDATA%\Programs\RenameIt`. This is the default for a new install. |
| `/AllUsers` | Install for everyone, under `%ProgramFiles%\RenameIt`. Needs administrator rights. |
| `/RESTART` | With `/S`, start RenameIt when the install finishes. The updater passes it for an update the user asked for. |
| `/D=<dir>` | Install folder. Must come last; NSIS's own rule. |
| `/RESUME` | Internal. The elevated copy that choosing *anyone who uses this computer* starts gets it, and opens on Components instead of on Welcome: the first copy already showed Welcome, the licence and the scope page. |

With no scope switch, an upgrade follows the existing install: per-user first, then
per-machine, and the folder it was in. The uninstaller is `uninstall.exe` in the install
folder. `/S` keeps your presets, scripts, journal and settings.

| Exit code | Meaning |
|---|---|
| 0 | Installed. |
| 2 | `/S /AllUsers` without administrator rights. Nothing was installed. |
| 3 | `/S`, and RenameIt was still running 30 seconds later. Nothing was changed. |

## Why it is built this way

- **No administrator prompt to open it.** The script uses `RequestExecutionLevel user`. Choosing *anyone who uses this computer* relaunches the installer elevated, and only then. The elevated copy continues at Components, with Back disabled, rather than starting over.
- **No quit event.** The sibling apps signal their tray process to quit; RenameIt is a window that may be partway through a batch. The installer asks whether `renameit.exe` and `ren-cli.exe` can be opened for writing. The updater closes the window itself, and the close waits for a running batch.
- **No Explorer-menu section.** The menu is per user and records the exe's path, so the app writes it (Settings ▸ Shell Integration) as the signed-in user. The uninstaller runs `renameit --uninstall-cleanup`, which removes it only if it points at this copy.
- **Files are deleted by name, never `RMDir /r $INSTDIR`.** The install folder may be one the user picked, holding more than RenameIt. `installer-smoke.ps1` fails if a staged file is left behind.
- **HKLM in the 64-bit view** (`SetRegView 64`), where the 64-bit app reads the uninstall entry.
- **Unsigned.** The updater checks the download against `SHA256SUMS.txt`. That catches a corrupted download, not a compromised repository: the list comes from the same release. Authenticode signing would close that gap.

`crates/ren-gui/tests/installer_contract.rs` holds the app, this script and `release.yml` to the same names and switches.
