# Stages what the Windows release ships into one folder: the zip is that folder, and the
# installer installs it minus the portable marker (packaging/renameit.nsi).
#
#   pwsh packaging/stage-windows.ps1 -Notices dist/notices/THIRD-PARTY-LICENSES.html
#
# One script for release.yml and for ci.yml's installer smoke test, so the smoke test installs
# and uninstalls exactly what a release carries. The uninstaller deletes these files by name,
# and the smoke test fails if one is left behind.
#
# Built from an explicit list rather than by sweeping a directory, so nothing reaches a
# release by accident.
param(
    [Parameter(Mandatory)] [string] $Notices,
    [string] $Bin = "target/release",
    [string] $Out = "dist/RenameIt"
)
$ErrorActionPreference = "Stop"

foreach ($dir in @($Out, "$Out/docs", "$Out/examples", "$Out/font-licences")) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}
Copy-Item "$Bin/renameit.exe" $Out
Copy-Item "$Bin/ren-cli.exe"  $Out
Copy-Item LICENSE             $Out
Copy-Item README.md           $Out
Copy-Item CHANGELOG.md        $Out
# D237: every crate the binaries link, with its licence text.
Copy-Item $Notices "$Out/THIRD-PARTY-LICENSES.html"
# The font licences require their notice to travel with the fonts, and the fonts are compiled
# into the executable. cargo-deny cannot see an asset that is not a crate, so this is the only
# thing carrying the obligation for the five Noto faces (OFL.txt) and egui's own four
# (font-licences/).
Copy-Item THIRD-PARTY-FONTS.md $Out
Copy-Item crates/ren-gui/assets/fonts/OFL.txt "$Out/OFL.txt"
Copy-Item crates/ren-gui/assets/fonts/egui/*.txt "$Out/font-licences"
# The README and the Script card point at these.
Copy-Item docs/tags.md                     "$Out/docs"
Copy-Item docs/MIGRATION-legacy-scripts.md "$Out/docs"
Copy-Item docs/cli.md                      "$Out/docs"
Copy-Item docs/manual.md                   "$Out/docs"
foreach ($example in @("camera-import", "cleanup", "music-rename", "numbered", "photo-cleanup")) {
    Copy-Item "examples/$example.toml" "$Out/examples"
}
# A downloaded, extracted folder is exactly the portable case, so the marker ships in the zip.
# Deleting it makes the copy behave like an install and keep its files in the per-user folder
# instead (D131). If the folder turns out not to be writable — Program Files, a read-only
# share — the app declines the portable path on its own. The installer leaves it out (D249).
Set-Content -Path "$Out/renameit-portable.txt" -Value @(
    "The presence of this file makes this a portable copy of RenameIt:",
    "presets, scripts, the undo journal and the window layout are kept in",
    "the RenameIt-data folder beside the program, so copying this folder",
    "takes them with it.",
    "",
    "Delete this file to keep them in your user profile instead."
)
# Checked rather than trusted: a licence obligation that depends on somebody remembering a
# copy step is one that will eventually be forgotten.
$needed = @(
    "OFL.txt", "THIRD-PARTY-FONTS.md", "THIRD-PARTY-LICENSES.html",
    "font-licences/UFL.txt", "font-licences/Hack-Regular.txt",
    "font-licences/emoji-icon-font-mit-license.txt",
    "docs/tags.md", "docs/MIGRATION-legacy-scripts.md", "docs/cli.md", "docs/manual.md"
)
foreach ($file in $needed) {
    if (-not (Test-Path "$Out/$file")) { throw "missing from the archive: $file" }
}
Get-ChildItem $Out -Recurse -File | Select-Object FullName, Length
