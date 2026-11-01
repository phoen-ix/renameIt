; RenameIt installer (D249). Built by release.yml and ci.yml from a staged folder:
;
;   makensis /DVERSION=<v> /DSTAGE=<abs path to dist\RenameIt> [/DOUTDIR=<abs dir>] renameit.nsi
;     -> RenameIt-Setup-<v>.exe
;
; packaging/stage-windows.ps1 fills the staged folder. Everything in it is installed except
; the portable marker: an installed copy keeps its files in the per-user folder (D131).
;
; Switches: /S  /CurrentUser  /AllUsers  /RESTART  /D=<dir> (last)
; Exit codes: 2 = silent /AllUsers without administrator rights (nothing installed)
;             3 = silent run, and RenameIt was still running after 30 seconds (nothing changed)
; packaging/README.md explains the design; ci.yml's installer smoke test runs it on Windows,
; and crates/ren-gui/tests/installer_contract.rs holds the app and this file to the same names.

Unicode true
SetCompressor /SOLID lzma

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef STAGE
  !error "Pass /DSTAGE=<the staged dist\RenameIt folder>; packaging/stage-windows.ps1 makes it."
!endif
!ifndef OUTDIR
  !define OUTDIR "."
!endif
!define APP "RenameIt"
!define PUBLISHER "phoen-ix"
!define EXE "renameit.exe"
!define CLI "ren-cli.exe"
!define ICON_SRC "..\crates\ren-gui\assets\renameit.ico"
; Read by the app (ren_platform::update::UNINSTALL_KEY): an entry whose InstallLocation is the
; running exe's folder is what makes a copy "installed" rather than portable.
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"
; The app's download folders (TEMP_PREFIX in crates/ren-gui/src/viewmodel/update.rs).
!define TEMP_PREFIX "RenameIt-update-"
!define PORTABLE_MARKER "renameit-portable.txt"

Name "${APP} ${VERSION}"
OutFile "${OUTDIR}\RenameIt-Setup-${VERSION}.exe"
BrandingText "${APP} ${VERSION}"

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"   ; ${GetSize}, ${GetParameters}, ${GetOptions}

; ---- per-user / per-machine ----
!define MULTIUSER_EXECUTIONLEVEL Highest
!define MULTIUSER_MUI
; No MULTIUSER_INSTALLMODE_COMMANDLINE: /CurrentUser and /AllUsers are parsed by hand in
; .onInit, after the privilege fib there. Stock MultiUser answers an unelevated /AllUsers with
; an MB_OK box that has no silent default, so "/S /AllUsers" from a normal shell would hang on
; a dialog nobody can see.
!define MULTIUSER_USE_PROGRAMFILES64
!define MULTIUSER_INSTALLMODE_INSTDIR "${APP}"
; Per-user unless asked otherwise. Without this MultiUser preselects per-machine for anyone
; holding an admin token, and RenameIt needs no privileges at all.
!define MULTIUSER_INSTALLMODE_DEFAULT_CURRENTUSER
!include "MultiUser.nsh"

; After the include on purpose, overriding the "highest" MULTIUSER_EXECUTIONLEVEL emits: that
; would show a UAC prompt just to open the installer. asInvoker means no prompt unless
; all-users is picked, and then ElevateForAllUsers asks for exactly that.
RequestExecutionLevel user

Var RealPrivileges   ; the account type as Windows reports it, before the fib to MultiUser
Var RestartApp       ; "1" when /RESTART was passed: a silent install relaunches RenameIt
Var Locked           ; "1" when renameit.exe or ren-cli.exe is in use, set by ${UN}CheckLocked

; ---- UI ----
!define MUI_ICON "${ICON_SRC}"
!define MUI_UNICON "${ICON_SRC}"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_SMALLDESC
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Start RenameIt now"
!define MUI_FINISHPAGE_RUN_FUNCTION "LaunchApp"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!define MULTIUSER_PAGE_CUSTOMFUNCTION_LEAVE ElevateForAllUsers
!insertmacro MULTIUSER_PAGE_INSTALLMODE
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function LaunchApp
  ; Through Explorer, so RenameIt starts as the signed-in user even after an elevated
  ; per-machine install, rather than inheriting the installer's administrator token.
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\${EXE}"'
FunctionEnd

; ---- a running RenameIt ----
; RenameIt is a window, not a tray app: it has no single-instance mutex and no quit event to
; signal. What matters is whether its files can be replaced, so that is what is asked: a
; running exe cannot be opened for writing. The in-app updater closes the window as it starts
; this installer, so a silent run waits for that; a run by hand asks the user to close it.
!macro TRY_OPEN FILE
  ${If} ${FileExists} "${FILE}"
    ClearErrors
    FileOpen $0 "${FILE}" a
    ${If} ${Errors}
      StrCpy $Locked "1"
    ${Else}
      FileClose $0
    ${EndIf}
  ${EndIf}
!macroend

!macro CLOSE_FUNCS UN ACTION
Function ${UN}CheckLocked
  Push $0
  StrCpy $Locked "0"
  !insertmacro TRY_OPEN "$INSTDIR\${EXE}"
  !insertmacro TRY_OPEN "$INSTDIR\${CLI}"
  Pop $0
FunctionEnd

Function ${UN}EnsureClosed
  Push $1
  StrCpy $1 0
 ec_check:
  Call ${UN}CheckLocked
  ${If} $Locked == "0"
    Pop $1
    Return
  ${EndIf}
  ${If} ${Silent}
    ${If} $1 < 60
      Sleep 500
      IntOp $1 $1 + 1
      Goto ec_check
    ${EndIf}
    SetErrorLevel 3
    Quit
  ${EndIf}
  MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION|MB_TOPMOST|MB_SETFOREGROUND "RenameIt is running, and ${ACTION} cannot replace it while it is.$\n$\nClose every RenameIt window, and wait for any ren-cli run to finish, then click Retry." /SD IDCANCEL IDRETRY ec_check
  Abort "Cancelled: RenameIt is still running."
FunctionEnd
!macroend
!insertmacro CLOSE_FUNCS ""    "Setup"
!insertmacro CLOSE_FUNCS "un." "the uninstaller"

; ---- sections ----
Section "-Core" SEC_CORE
  SectionIn RO
  Call EnsureClosed
  SetOutPath "$INSTDIR"
  File /r /x "${PORTABLE_MARKER}" "${STAGE}\*"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr   SHCTX "${UNINST_KEY}" "DisplayName"     "${APP}"
  WriteRegStr   SHCTX "${UNINST_KEY}" "DisplayVersion"  "${VERSION}"
  WriteRegStr   SHCTX "${UNINST_KEY}" "Publisher"       "${PUBLISHER}"
  WriteRegStr   SHCTX "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\${EXE},0"
  WriteRegStr   SHCTX "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   SHCTX "${UNINST_KEY}" "URLInfoAbout"    "https://github.com/phoen-ix/renameIt"
  WriteRegStr   SHCTX "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr   SHCTX "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD SHCTX "${UNINST_KEY}" "EstimatedSize" $0
  WriteRegDWORD SHCTX "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD SHCTX "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Start Menu shortcut" SEC_SM
  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\${EXE}"
SectionEnd

Section /o "Desktop shortcut" SEC_DESK
  CreateShortcut "$DESKTOP\${APP}.lnk" "$INSTDIR\${EXE}"
SectionEnd

; No Explorer-menu section: the menu is per user and records the exe's path, so the app
; writes it (Settings > Shell Integration) as the signed-in user. An elevated install would
; write the administrator's.

Function .onInit
  ; The 64-bit view, so the HKLM entry lands where the 64-bit app reads it.
  SetRegView 64
  !insertmacro MULTIUSER_INIT
  ; Stock MultiUser skips the scope page entirely for a non-admin, so a standard user who
  ; knows an administrator password could never install for everyone. Remember what we really
  ; are, then claim Admin so the choice renders; ElevateForAllUsers does the real elevating.
  ; asInvoker means even an administrator arrives here unelevated, so this matters for them too.
  StrCpy $RealPrivileges $MultiUser.Privileges
  ${If} $RealPrivileges != "Admin"
  ${AndIf} $RealPrivileges != "Power"
    StrCpy $MultiUser.Privileges "Admin"
  ${EndIf}

  Push $R0
  Push $R1
  Push $R2
  ${GetParameters} $R0
  ; /CurrentUser and /AllUsers, by hand and after the fib. /AllUsers wins if both are given.
  StrCpy $R2 "0"
  ClearErrors
  ${GetOptions} $R0 "/CurrentUser" $R1
  ${IfNot} ${Errors}
    Call MultiUser.InstallMode.CurrentUser
    StrCpy $R2 "1"
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/AllUsers" $R1
  ${IfNot} ${Errors}
    Call MultiUser.InstallMode.AllUsers
    StrCpy $R2 "1"
  ${EndIf}
  ; Upgrade in place. With no scope switch, follow an existing install, per-user first as the
  ; app reads them, rather than putting a second copy beside it. A switch is obeyed: the
  ; updater always passes the scope of the copy it is updating.
  ${If} $R2 == "0"
    ReadRegStr $0 HKCU "${UNINST_KEY}" "UninstallString"
    ${If} $0 == ""
      ReadRegStr $0 HKLM "${UNINST_KEY}" "UninstallString"
      ${If} $0 != ""
        Call MultiUser.InstallMode.AllUsers
      ${EndIf}
    ${EndIf}
  ${EndIf}
  ; A silent run never sees the scope page, so nothing would elevate it. Fail with a code of
  ; its own rather than half-install into Program Files with a token that cannot write there.
  ${If} ${Silent}
  ${AndIf} $MultiUser.InstallMode == "AllUsers"
  ${AndIf} $RealPrivileges != "Admin"
  ${AndIf} $RealPrivileges != "Power"
    SetErrorLevel 2
    Quit
  ${EndIf}
  ; Reuse the previous install's folder, so a custom directory is upgraded rather than
  ; orphaned: a silent upgrade has no directory page to pick it again.
  ReadRegStr $0 SHCTX "${UNINST_KEY}" "InstallLocation"
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$0\${EXE}"
    StrCpy $INSTDIR $0
  ${EndIf}
  ; /RESTART: start RenameIt again when a silent install finishes. The updater passes it for
  ; an update the user asked for; it closed the window, and nothing else would reopen it.
  StrCpy $RestartApp "0"
  ClearErrors
  ${GetOptions} $R0 "/RESTART" $R1
  ${IfNot} ${Errors}
    StrCpy $RestartApp "1"
  ${EndIf}
  ClearErrors
  Pop $R2
  Pop $R1
  Pop $R0
FunctionEnd

; After a successful install, silent ones included: /S never reaches the finish page, so its
; "Start RenameIt now" never fires.
Function .onInstSuccess
  ${If} ${Silent}
  ${AndIf} $RestartApp == "1"
    Call LaunchApp
  ${EndIf}
FunctionEnd

; The scope page's leave handler, after MultiUser has applied the choice. A non-admin who
; picked all users hands the install to an elevated copy of this installer.
Function ElevateForAllUsers
  ${If} $MultiUser.InstallMode != "AllUsers"
    Return
  ${EndIf}
  ${If} $RealPrivileges == "Admin"
  ${OrIf} $RealPrivileges == "Power"
    Return
  ${EndIf}
  ClearErrors
  ExecShell "runas" "$EXEPATH" "/AllUsers"
  ${IfNot} ${Errors}
    Quit
  ${EndIf}
  Call MultiUser.InstallMode.CurrentUser
  MessageBox MB_OK|MB_ICONINFORMATION|MB_TOPMOST|MB_SETFOREGROUND "Administrator rights were not granted, so RenameIt will be installed for you only." /SD IDOK
FunctionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_SM}   "Add RenameIt to the Start Menu."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESK} "Add RenameIt to the Desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---- uninstall ----
Function un.onInit
  SetRegView 64
  !insertmacro MULTIUSER_UNINIT
  ; Stock MultiUser picks CurrentUser for every token here, since the registered uninstall
  ; strings carry no /AllUsers. Work out which install this uninstaller belongs to instead: an
  ; HKLM entry naming our own folder means per-machine, and SHCTX and the shortcut folders
  ; have to follow.
  ReadRegStr $0 HKCU "${UNINST_KEY}" "InstallLocation"
  ${If} $0 != "$INSTDIR"
    ReadRegStr $0 HKLM "${UNINST_KEY}" "InstallLocation"
    ${If} $0 == "$INSTDIR"
      StrCpy $MultiUser.InstallMode AllUsers
      SetShellVarContext all
    ${EndIf}
  ${EndIf}
  ${If} $MultiUser.InstallMode == "AllUsers"
  ${AndIf} $MultiUser.Privileges != "Admin"
  ${AndIf} $MultiUser.Privileges != "Power"
    ; Hand over to an elevated copy. $INSTDIR\uninstall.exe, not $EXEPATH: NSIS runs an
    ; uninstaller from a copy in $TEMP. /S is forwarded so a quiet uninstall stays quiet.
    ClearErrors
    ${If} ${Silent}
      ExecShell "runas" "$INSTDIR\uninstall.exe" "/S"
    ${Else}
      ExecShell "runas" "$INSTDIR\uninstall.exe"
    ${EndIf}
    ${IfNot} ${Errors}
      Quit
    ${EndIf}
    MessageBox MB_OK|MB_ICONSTOP|MB_TOPMOST|MB_SETFOREGROUND "RenameIt was installed for everyone on this computer, so removing it needs administrator rights.$\n$\nRight-click uninstall.exe in the RenameIt folder and choose 'Run as administrator'." /SD IDOK
    Quit
  ${EndIf}
FunctionEnd

; Removes the program, and asks before removing what the user made with it.
Section "Uninstall"
  Call un.EnsureClosed
  ; The Explorer menu records an exe's path. The app removes it only if it is this copy's;
  ; a portable copy that wrote it last keeps it (D249).
  ${If} ${FileExists} "$INSTDIR\${EXE}"
    ExecWait '"$INSTDIR\${EXE}" --uninstall-cleanup'
  ${EndIf}

  SetOutPath "$TEMP"   ; move the working directory out of $INSTDIR so it can be removed
  ; By name, never RMDir /r "$INSTDIR": that folder may be one the user chose, holding more
  ; than RenameIt. The installer smoke test fails if a staged file is missing here.
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\${CLI}"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\CHANGELOG.md"
  Delete "$INSTDIR\THIRD-PARTY-LICENSES.html"
  Delete "$INSTDIR\THIRD-PARTY-FONTS.md"
  Delete "$INSTDIR\OFL.txt"
  Delete "$INSTDIR\docs\*.md"
  RMDir  "$INSTDIR\docs"
  Delete "$INSTDIR\examples\*.toml"
  RMDir  "$INSTDIR\examples"
  Delete "$INSTDIR\font-licences\*.txt"
  RMDir  "$INSTDIR\font-licences"
  Delete "$SMPROGRAMS\${APP}.lnk"
  Delete "$DESKTOP\${APP}.lnk"

  FindFirst $0 $1 "$TEMP\${TEMP_PREFIX}*"
  ${DoWhile} $1 != ""
    RMDir /r "$TEMP\$1"
    FindNext $0 $1
  ${Loop}
  FindClose $0

  ; Presets, scripts, the undo journal and the settings are the user's work, so they stay
  ; unless the user says otherwise; a silent uninstall keeps them (D105's rule: losing
  ; somebody's edits is a defect). The current account's folder, even from a per-machine
  ; uninstall, whose "all" context would make $APPDATA ProgramData.
  SetShellVarContext current
  ${If} ${FileExists} "$APPDATA\${APP}\*.*"
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2|MB_TOPMOST|MB_SETFOREGROUND "Also delete your presets, scripts, undo history and settings?$\n$\nThey are in $APPDATA\${APP}. Keep them if you might install RenameIt again." /SD IDNO IDNO keep_data
    RMDir /r "$APPDATA\${APP}"
   keep_data:
  ${EndIf}

  ; This install's Add/Remove Programs entry, and only this one: both hives use the same key
  ; name, and a per-user copy may sit beside a per-machine one.
  ReadRegStr $0 HKCU "${UNINST_KEY}" "InstallLocation"
  ${If} $0 == "$INSTDIR"
    DeleteRegKey HKCU "${UNINST_KEY}"
  ${EndIf}
  ReadRegStr $0 HKLM "${UNINST_KEY}" "InstallLocation"
  ${If} $0 == "$INSTDIR"
    DeleteRegKey HKLM "${UNINST_KEY}"
  ${EndIf}

  Delete "$INSTDIR\uninstall.exe"
  RMDir  "$INSTDIR"
  ; NSIS runs an uninstaller from a copy in $TEMP, so the folder may still be held. Sweep it up
  ; from a detached shell a moment later.
  IfFileExists "$INSTDIR\uninstall.exe" 0 +2
    Exec '"$SYSDIR\cmd.exe" /c ping 127.0.0.1 -n 3 >nul & del /f /q "$INSTDIR\uninstall.exe" & rmdir "$INSTDIR"'
SectionEnd
