//! The app, the installer script and the release workflow name the same things
//! (D249, D250).
//!
//! Each pair below is written in two files that know nothing of each other, and
//! renaming one side alone breaks nothing that a test would otherwise notice. The
//! updater would find no installer to download, or start it with a switch it
//! ignores, or call an installed copy portable. So the files are read as text and
//! held together here, on Linux, where the installer itself cannot run.

use ren_gui::viewmodel::update::{
    self, PER_MACHINE, PER_USER, RESTART, SUMS, TEMP_PREFIX, Version,
};

const NSI: &str = include_str!("../../../packaging/renameit.nsi");
const RELEASE: &str = include_str!("../../../.github/workflows/release.yml");

/// The value of `!define NAME "value"`, with any `${OTHER}` in it expanded.
fn define(name: &str) -> String {
    let prefix = format!("!define {name} \"");
    let line = NSI
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("renameit.nsi defines no {name}"));
    let mut value = line[prefix.len()..].trim_end_matches('"').to_owned();
    while let Some(start) = value.find("${") {
        let end = start + value[start..].find('}').expect("a closed ${…}");
        let inner = define(&value[start + 2..end]);
        value.replace_range(start..=end, &inner);
    }
    value
}

#[test]
fn the_uninstall_entry_is_where_the_app_looks_for_it() {
    assert_eq!(define("UNINST_KEY"), ren_platform::update::UNINSTALL_KEY);
    assert!(
        NSI.contains(r#""${UNINST_KEY}" "InstallLocation" "$INSTDIR""#),
        "InstallLocation is what tells an installed copy from a portable one"
    );
}

#[test]
fn the_installer_leaves_out_the_portable_marker_the_app_looks_for() {
    assert_eq!(define("PORTABLE_MARKER"), ren_platform::PORTABLE_MARKER);
    assert!(NSI.contains(r#"File /r /x "${PORTABLE_MARKER}""#));
}

#[test]
fn the_uninstaller_sweeps_the_folders_the_updater_downloads_into() {
    assert_eq!(define("TEMP_PREFIX"), TEMP_PREFIX);
    assert!(NSI.contains(r#""$TEMP\${TEMP_PREFIX}*""#));
}

/// `/S` is NSIS's own; the rest are parsed by hand in `.onInit`.
#[test]
fn every_switch_the_updater_passes_is_one_the_installer_reads() {
    for switch in [PER_USER, PER_MACHINE, RESTART] {
        assert!(
            NSI.contains(&format!(r#"${{GetOptions}} $R0 "{switch}""#)),
            "{switch} is passed by the updater and never read by the installer"
        );
    }
}

#[test]
fn the_uninstaller_runs_a_flag_the_app_understands() {
    assert!(NSI.contains(r#"'"$INSTDIR\${EXE}" --uninstall-cleanup'"#));
    assert_eq!(define("EXE"), "renameit.exe");
    let launch = ren_gui::launch::parse(["renameit", "--uninstall-cleanup"].map(Into::into));
    assert!(launch.uninstall_cleanup);
}

/// Choosing "anyone who uses this computer" starts an elevated copy of the
/// installer. That copy continues at the step after the scope page instead of
/// greeting the user again: the relaunch says so with `/RESUME`, `.onInit`
/// reads it, and the three pages the first copy showed are skipped.
#[test]
fn the_elevated_copy_continues_where_the_first_one_stopped() {
    assert!(NSI.contains(r#"ExecShell "runas" "$EXEPATH" "/AllUsers /RESUME""#));
    assert!(NSI.contains(r#"${GetOptions} $R0 "/RESUME""#));
    for page in [
        "!insertmacro MUI_PAGE_WELCOME",
        "!insertmacro MUI_PAGE_LICENSE",
        "!insertmacro MULTIUSER_PAGE_INSTALLMODE",
    ] {
        let at = NSI.find(page).unwrap_or_else(|| panic!("{page}"));
        let before: Vec<&str> = NSI[..at].lines().rev().take(3).collect();
        assert!(
            before.iter().any(|line| line.contains("PRE SkipIfResumed")),
            "{page} is not skipped in the elevated copy"
        );
    }
    assert!(
        NSI.contains("MUI_PAGE_CUSTOMFUNCTION_SHOW FirstPageAfterResume"),
        "the first page the elevated copy shows has no Back into the skipped ones"
    );
}

/// D252: the installer's right-click menu box is ticked by default, has a
/// switch each way for silent installs, records its answer where the app reads
/// it, and on a fresh per-user install runs a flag the app understands.
#[test]
fn the_menu_box_is_on_by_default_and_recorded_where_the_app_reads_it() {
    assert_eq!(
        define("CONTEXT_MENU_VALUE"),
        ren_platform::update::CONTEXT_MENU_VALUE
    );
    assert!(
        NSI.contains(r#"Section "Right-click menu in Explorer" SEC_MENU"#),
        "ticked by default: no /o"
    );
    for switch in ["/MENU", "/NOMENU"] {
        assert!(
            NSI.contains(&format!(r#"${{GetOptions}} $R0 "{switch}""#)),
            "{switch}"
        );
    }
    assert!(NSI.contains(r#"'"$INSTDIR\${EXE}" --install-menu'"#));
    let launch = ren_gui::launch::parse(["renameit", "--install-menu"].map(Into::into));
    assert!(launch.install_menu);
}

/// NSIS shows a message box under `/S` unless it says what to answer. A silent
/// install or update that stops on a dialog nobody can see is a hang.
#[test]
fn every_message_box_answers_itself_when_silent() {
    let boxes: Vec<&str> = NSI
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("MessageBox"))
        .collect();
    assert!(!boxes.is_empty());
    for line in boxes {
        assert!(line.contains(" /SD "), "no silent default: {line}");
    }
}

/// The updater builds the installer's URL from the tag; the release has to
/// carry a file of exactly that name, and a checksum list naming it.
#[test]
fn the_release_carries_the_files_the_updater_asks_for() {
    let out_file = NSI
        .lines()
        .find_map(|line| line.strip_prefix("OutFile \""))
        .expect("an OutFile")
        .trim_end_matches('"');
    assert_eq!(
        out_file.replace("${VERSION}", "1.2.3"),
        format!("${{OUTDIR}}\\{}", update::installer_asset(Version(1, 2, 3)))
    );

    assert!(RELEASE.contains("packaging\\renameit.nsi"));
    assert!(RELEASE.contains(&format!("> {SUMS}")));
    assert!(RELEASE.contains(&format!("dist/{SUMS}")));
    assert!(
        RELEASE.contains("sha256sum RenameIt-Setup-*.exe"),
        "the checksum list covers the installer"
    );
    assert!(RELEASE.contains("dist/RenameIt-Setup-*.exe"));
}
