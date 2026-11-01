# Installs, upgrades and uninstalls RenameIt for real, per-user and per-machine (D249).
#
#   pwsh packaging/installer-smoke.ps1 -Installer dist/RenameIt-Setup-<v>.exe
#
# Run by ci.yml on windows-latest, whose account is an administrator with UAC off, so
# /AllUsers needs no prompt there. Not for a developer's machine: it installs and removes
# RenameIt in the real per-user and per-machine places.
param([Parameter(Mandatory)] [string] $Installer)
$ErrorActionPreference = "Stop"
$Installer = (Resolve-Path $Installer).Path

$key = "Software\Microsoft\Windows\CurrentVersion\Uninstall\RenameIt"
$data = Join-Path $env:APPDATA "RenameIt"

function Run([string] $exe, [string] $arguments) {
    (Start-Process -FilePath $exe -ArgumentList $arguments -Wait -PassThru).ExitCode
}
function Check([bool] $condition, [string] $what) {
    if (-not $condition) { throw "FAILED: $what" }
    Write-Host "ok: $what"
}

# The uninstaller in place (_?=), so this waits for it rather than for the copy NSIS would
# otherwise start from %TEMP% and return straight away. Its last act is a detached shell that
# removes uninstall.exe and the folder a moment later.
function Uninstall([string] $dir) {
    Check ((Run "$dir\uninstall.exe" "/S _?=$dir") -eq 0) "silent uninstall of $dir exits 0"
    Start-Sleep -Seconds 5
    Check (-not (Test-Path $dir)) "nothing is left in $dir (every staged file is deleted by name)"
}

# ---- per user ----
$dir = Join-Path $env:LOCALAPPDATA "Programs\RenameIt"
New-Item -ItemType Directory -Force $data | Out-Null
Set-Content "$data\keep.txt" "the user's own work"

Check ((Run $Installer "/S") -eq 0) "silent per-user install exits 0"
Check (Test-Path "$dir\renameit.exe") "renameit.exe is in $dir"
Check (Test-Path "$dir\docs\cli.md") "the docs are installed"
Check (-not (Test-Path "$dir\renameit-portable.txt")) "an installed copy is not portable"
Check ((Get-ItemProperty "HKCU:\$key").InstallLocation -eq $dir) "HKCU InstallLocation names the folder"
Check (Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\RenameIt.lnk") "a Start Menu shortcut"

# A running RenameIt, as far as the installer can tell: its exe cannot be opened for writing.
# Held for a few seconds, the way the updater's window takes a moment to close: a silent
# upgrade waits, then goes ahead.
$held = [System.IO.File]::Open("$dir\renameit.exe", "Open", "Read", "Read")
$upgrade = Start-Process -FilePath $Installer -ArgumentList "/S" -PassThru
Start-Sleep -Seconds 3
$held.Dispose()
$upgrade.WaitForExit()
Check ($upgrade.ExitCode -eq 0) "a silent upgrade waits for RenameIt to close, then installs"

# Held throughout: after 30 seconds it gives up, with its own exit code, having changed nothing.
$held = [System.IO.File]::Open("$dir\renameit.exe", "Open", "Read", "Read")
try {
    Check ((Run $Installer "/S") -eq 3) "a silent upgrade gives up with exit code 3 if RenameIt stays open"
} finally {
    $held.Dispose()
}

Uninstall $dir
Check (-not (Test-Path "HKCU:\$key")) "the HKCU uninstall entry is gone"
Check (-not (Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\RenameIt.lnk")) "the shortcut is gone"
Check (Test-Path "$data\keep.txt") "a silent uninstall keeps the user's presets, scripts and journal"

# ---- per machine ----
$dir = Join-Path $env:ProgramFiles "RenameIt"
Check ((Run $Installer "/S /AllUsers") -eq 0) "silent per-machine install exits 0"
Check (Test-Path "$dir\renameit.exe") "renameit.exe is in $dir"
Check ((Get-ItemProperty "HKLM:\$key").InstallLocation -eq $dir) "HKLM InstallLocation names the folder, in the 64-bit view"
Uninstall $dir
Check (-not (Test-Path "HKLM:\$key")) "the HKLM uninstall entry is gone"

Remove-Item -Recurse -Force $data
