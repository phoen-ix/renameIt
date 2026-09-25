//! What a command line asked the app to do.
//!
//! Until now there was no grammar at all: every argument was a path, because
//! the only thing that ever passed one was a drag onto the executable or
//! D132's single *"Open with RenameIt"* verb. The Explorer preset menu needs
//! more — which preset, and which of four things to do with the selection — so
//! this is where that vocabulary lives.
//!
//! # The parser cannot be allowed to fail
//!
//! A release build is `windows_subsystem = "windows"`. clap's reflex on an
//! unrecognised argument is to print to stderr and `exit(2)`; in a process with
//! no console that is **a program that visibly does nothing**, and the user has
//! no way to find out why. So [`parse`] never fails: anything clap rejects
//! falls back to the reading every build before this one used — every argument
//! is a path — and the reason is carried into the status line, where there is
//! somewhere to say it.
//!
//! That is the same instinct `start_at` already had about a path that no longer
//! exists: an error dialog before the window has been seen is not a greeting.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::Parser;

/// The Explorer menu writes these; this module reads them. The pairing is
/// checked by a test that runs every command string the menu will hold back
/// through [`parse`].
#[derive(Parser, Debug)]
#[command(name = "renameit", version, about = "RenameIt — a batch file renamer")]
struct Cli {
    /// Files or folders to open. One folder browses there; anything else is
    /// Free Select — the same reading a drag from the file manager gets.
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,

    /// Browse the folder the first path is in. A folder browses itself.
    #[arg(long)]
    start_in: bool,

    /// Load a saved pipeline before showing the preview.
    ///
    /// A **path**, never a name: two presets may share a display name, which is
    /// exactly why `PresetStore::load_named` returns `Ambiguous` rather than
    /// guessing. The menu must not guess either.
    #[arg(long, value_name = "FILE")]
    preset: Option<PathBuf>,

    /// Put the names of these files on the clipboard and exit, with no window.
    #[arg(long, conflicts_with_all = ["preset", "start_in"])]
    copy_names: bool,

    /// Written by the Explorer menu. Nobody needs to type it.
    #[arg(long)]
    from_shell: bool,
}

/// What the app should do with the command line it was given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launch {
    pub paths: Vec<PathBuf>,
    /// Browse the folder the first path is in, rather than listing the paths.
    pub start_in: bool,
    pub preset: Option<PathBuf>,
    pub copy_names: bool,
    /// Set by the Explorer menu, and the only thing that unlocks the selection
    /// caveat — a drag of two hundred files must not be told about a limit that
    /// did not apply to it.
    pub from_shell: bool,
    /// What could not be made sense of, for the status line.
    pub complaint: Option<String>,
    /// How many characters the OS handed this process, before anything split
    /// or unquoted them.
    ///
    /// Here rather than read at the point of use, so `Launch` is the whole
    /// description of what a command line asked — and so the one thing about
    /// it that only Windows can answer is testable everywhere.
    pub command_line_chars: Option<usize>,
}

/// The placeholders Windows substitutes. One arriving *unsubstituted* means the
/// registry entry is malformed, which is otherwise undiagnosable: it looks like
/// a path that does not exist, and `start_at` drops those silently.
const PLACEHOLDERS: [&str; 4] = ["%1", "%V", "%W", "%*"];

/// Reads a command line, and never fails.
///
/// See the module docs for why "never" is load-bearing rather than tidy.
pub fn parse(argv: impl IntoIterator<Item = OsString>) -> Launch {
    let argv: Vec<OsString> = argv.into_iter().collect();

    let chars = ren_platform::command_line_chars();
    let mut launch = match Cli::try_parse_from(&argv) {
        Ok(cli) => Launch {
            paths: cli.paths,
            start_in: cli.start_in,
            preset: cli.preset,
            copy_names: cli.copy_names,
            from_shell: cli.from_shell,
            complaint: None,
            command_line_chars: chars,
        },
        Err(error) => {
            // `--help` and `--version` are "errors" that print to stdout and
            // mean success. A windows-subsystem build has no stdout either, so
            // they reach nobody — `ren-cli --help` is where the help lives, and
            // this at least does not open a window on top of it.
            if !error.use_stderr() {
                let _ = error.print();
                std::process::exit(0);
            }
            Launch {
                paths: argv.iter().skip(1).map(PathBuf::from).collect(),
                complaint: Some(format!(
                    "Did not understand part of the command line, so every argument was read \
                     as a path. ({})",
                    first_line(&error.to_string())
                )),
                command_line_chars: chars,
                ..Default::default()
            }
        }
    };

    // An unsubstituted placeholder is a broken registry entry, not a file.
    let before = launch.paths.len();
    launch
        .paths
        .retain(|path| !PLACEHOLDERS.iter().any(|p| path.as_os_str() == *p));
    if launch.paths.len() != before {
        launch.complaint = Some(
            "Windows did not fill in the file names for that menu entry. Re-install the menu \
             in Settings ▸ Shell Integration."
                .to_owned(),
        );
    }

    launch
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text).trim()
}

/// The names of these files, one per line, sorted.
///
/// **Sorted here and nowhere else.** Every other path into the app builds a
/// `Session`, and a listing is sorted as it is installed, before anything is
/// numbered — which is what makes Explorer's habit of passing the
/// right-clicked file *first* harmless everywhere else. This function has no
/// session, so it is the one place that order could reach the user.
///
/// **Names, not paths.** The menu item says "filenames", and Windows already
/// offers "Copy as path" behind Shift+right-click; copying paths here would
/// duplicate the shell rather than add to it.
pub fn names_text(paths: &[PathBuf]) -> String {
    let mut names: Vec<String> = paths
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    // A trailing newline on every line, not a separator between them — which
    // is what `RenameItApp::rows_as_text` already produces, and pasting into a
    // text editor wants the last line terminated like the rest. The two are
    // separate functions because a context-menu click never builds a
    // `Session`, so `the_menu_and_the_row_menu_copy_the_same_list` is the only
    // thing stopping them drifting; it caught this on its first run.
    names.iter().map(|name| format!("{name}\n")).collect()
}

/// Puts these files' names on the clipboard.
///
/// **No window opens, and on Windows none is needed.** `SetClipboardData`
/// transfers ownership of the memory to the system, so the text outlives this
/// process exiting a moment later — which is what lets a context-menu item be a
/// program that starts, copies and stops.
///
/// **Linux is the opposite and M9 must not assume otherwise**: X11 and Wayland
/// serve the selection from the owning process, so a future file-manager action
/// there has to keep one alive rather than copy and quit.
///
/// The failure is handed back rather than swallowed: a release build has no
/// stderr to complain to, so the caller opens the window it was avoiding and
/// says so on screen.
pub fn copy_names(paths: &[PathBuf]) -> Result<(), arboard::Error> {
    arboard::Clipboard::new()?.set_text(names_text(paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<OsString> {
        std::iter::once("renameit")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect()
    }

    /// The reading every build before this one used, unchanged. A drag onto the
    /// executable and a stale D132 verb both still arrive this way.
    #[test]
    fn bare_paths_still_mean_what_they_always_did() {
        let launch = parse(argv(&["/tmp/a.txt", "/tmp/b.txt"]));
        assert_eq!(
            launch.paths,
            [PathBuf::from("/tmp/a.txt"), "/tmp/b.txt".into()]
        );
        assert!(!launch.start_in);
        assert!(!launch.from_shell);
        assert!(launch.preset.is_none());
        assert!(launch.complaint.is_none());
    }

    #[test]
    fn the_menu_entries_are_understood() {
        let start = parse(argv(&["--from-shell", "--start-in", "/tmp/pics"]));
        assert!(start.from_shell && start.start_in);
        assert_eq!(start.paths, [PathBuf::from("/tmp/pics")]);

        let select = parse(argv(&["--from-shell", "/tmp/a.txt", "/tmp/b.txt"]));
        assert!(select.from_shell && !select.start_in);
        assert_eq!(select.paths.len(), 2);

        let copy = parse(argv(&["--from-shell", "--copy-names", "/tmp/a.txt"]));
        assert!(copy.copy_names);

        let preset = parse(argv(&[
            "--from-shell",
            "--preset",
            "/tmp/presets/Tidy up.toml",
            "/tmp/a.txt",
        ]));
        assert_eq!(
            preset.preset.as_deref(),
            Some(std::path::Path::new("/tmp/presets/Tidy up.toml"))
        );
        assert_eq!(preset.paths, [PathBuf::from("/tmp/a.txt")]);
    }

    /// The one that matters: a release build has no stderr, so a parser that
    /// exits is a program that silently fails to appear.
    #[test]
    fn a_command_line_we_cannot_parse_still_opens_the_app() {
        let launch = parse(argv(&["--presett", "/tmp/a.txt"]));
        assert_eq!(
            launch.paths,
            [PathBuf::from("--presett"), "/tmp/a.txt".into()],
            "every argument read as a path, exactly as before this module existed"
        );
        assert!(launch.complaint.is_some(), "and it says so");
        assert!(launch.preset.is_none());
    }

    /// A `%1` that reached us is a registry entry Windows did not fill in. It
    /// would otherwise be a path that does not exist, which `start_at` drops
    /// without a word.
    #[test]
    fn an_unsubstituted_placeholder_is_named_rather_than_dropped() {
        for placeholder in PLACEHOLDERS {
            let launch = parse(argv(&["--from-shell", placeholder]));
            assert!(launch.paths.is_empty(), "{placeholder}");
            let complaint = launch.complaint.expect(placeholder);
            assert!(complaint.contains("Shell Integration"), "{complaint}");
        }
    }

    /// Explorer sends the file you right-clicked **first**, whatever the order
    /// on screen. Everywhere else the listing is sorted before anything is
    /// numbered; this function has no session, so it sorts for itself.
    #[test]
    fn the_clipboard_list_is_sorted_not_the_order_explorer_sent() {
        let text = names_text(&[
            PathBuf::from("/tmp/photos/c.jpg"),
            PathBuf::from("/tmp/photos/a.jpg"),
            PathBuf::from("/tmp/photos/b.jpg"),
        ]);
        assert_eq!(text, "a.jpg\nb.jpg\nc.jpg\n");
    }

    /// Names, because the item says filenames — and because Windows already has
    /// "Copy as path" one modifier away.
    #[test]
    fn the_clipboard_gets_names_and_not_paths() {
        let text = names_text(&[PathBuf::from("/tmp/deep/nested/holiday.jpg")]);
        assert_eq!(text, "holiday.jpg\n");
    }

    #[test]
    fn nothing_selected_is_an_empty_clipboard_rather_than_a_panic() {
        assert_eq!(names_text(&[]), "");
    }

    /// Split a command line the way `CommandLineToArgvW` does.
    ///
    /// Written out rather than approximated with `split_whitespace`, because
    /// the thing under test is a command line full of quoted paths with spaces
    /// in them — a splitter that got quoting wrong would fail the test for a
    /// reason that has nothing to do with the menu.
    ///
    /// The rule Microsoft documents and nobody remembers: `2n` backslashes
    /// before a `"` are `n` backslashes and the quote toggles; `2n+1` are `n`
    /// backslashes and a **literal** quote. Backslashes not before a quote are
    /// themselves — which is the only reason `C:\Users\mk` survives at all.
    fn split_windows(line: &str) -> Vec<OsString> {
        let mut args = Vec::new();
        let mut current = String::new();
        let mut started = false;
        let mut quoted = false;
        let mut slashes = 0usize;

        for ch in line.chars() {
            match ch {
                '\\' => slashes += 1,
                '"' => {
                    current.push_str(&"\\".repeat(slashes / 2));
                    if slashes % 2 == 1 {
                        current.push('"');
                    } else {
                        quoted = !quoted;
                    }
                    slashes = 0;
                    // `""` is an empty argument, so a quote alone starts one.
                    started = true;
                }
                ch if ch.is_whitespace() && !quoted => {
                    current.push_str(&"\\".repeat(slashes));
                    slashes = 0;
                    if started {
                        args.push(OsString::from(std::mem::take(&mut current)));
                    }
                    started = false;
                }
                ch => {
                    current.push_str(&"\\".repeat(slashes));
                    slashes = 0;
                    current.push(ch);
                    started = true;
                }
            }
        }
        current.push_str(&"\\".repeat(slashes));
        if started {
            args.push(OsString::from(current));
        }
        args
    }

    #[test]
    fn the_splitter_follows_the_rule_it_claims_to() {
        assert_eq!(
            split_windows(r#""C:\a b\x.exe" --preset "C:\p\Rock & Roll.toml" plain"#),
            [
                OsString::from(r"C:\a b\x.exe"),
                "--preset".into(),
                r"C:\p\Rock & Roll.toml".into(),
                "plain".into(),
            ]
        );
        // 2n before a quote: the quote toggles. 2n+1: the quote is literal.
        assert_eq!(
            split_windows(r#"a "b\\" c"#),
            [OsString::from("a"), r"b\".into(), "c".into()]
        );
        assert_eq!(
            split_windows(r#"a "b\" c""#),
            [OsString::from("a"), r#"b" c"#.into()]
        );
    }

    /// **The test this feature rests on.**
    ///
    /// Every command string the registry will hold, substituted the way
    /// Explorer substitutes it, split the way Windows splits it, and fed back
    /// through the parser that has to understand it. It runs on Linux, where
    /// none of those three things happen — and it still kills the bug it is
    /// aimed at, because that bug is not about Windows: it is a flag the menu
    /// writes and the parser does not accept, which shows up on a user's
    /// machine as a menu item that opens a window showing nothing.
    ///
    /// The two halves live in different crates and were written a day apart.
    /// Nothing but this test connects them.
    #[test]
    fn every_menu_item_parses_back_to_what_it_promised() {
        use ren_platform::shell::{MenuPreset, ShellPlan, Step, Value};

        let presets = [
            MenuPreset {
                name: "Rock & Roll",
                file: std::path::Path::new(r"C:\Users\mk\presets\Rock & Roll.toml"),
            },
            MenuPreset {
                name: "50% off",
                file: std::path::Path::new(r"C:\Users\mk\presets\50% off.toml"),
            },
        ];
        let plan = ShellPlan::build(
            std::path::Path::new(r"C:\Apps\Ren It\renameit.exe"),
            &presets,
        );

        // A label and the command underneath it. The `\command` subkey sorts
        // after its parent's values in the step list, so one pass with a
        // pending label is enough.
        let mut label = String::new();
        let mut checked = 0;
        for step in &plan.steps {
            let Step::Set { key, name, value } = step else {
                continue;
            };
            let Value::Sz(text) = value else { continue };
            if name == "MUIVerb" && key.contains(r"\shell\") {
                label = text.clone();
                continue;
            }
            if !name.is_empty() || !key.ends_with(r"\command") {
                continue;
            }

            // What Explorer puts there: under `Player` the whole selection,
            // already quoted per item; for a background click, one folder.
            let line = text
                .replace("%1", r#""C:\pics\a b.jpg" "C:\pics\c.jpg""#)
                .replace("%V", r"C:\pics");
            let launch = parse(split_windows(&line));

            assert!(launch.from_shell, "{label}: {line}");
            assert_eq!(launch.complaint, None, "{label}: {line}");
            assert!(!launch.paths.is_empty(), "{label}: {line}");
            assert!(
                // A string prefix, not `Path::starts_with`: this test runs on
                // Linux, where `C:\pics\a b.jpg` is one component and the
                // component-wise version is false for every path here.
                launch
                    .paths
                    .iter()
                    .all(|p| p.to_string_lossy().starts_with(r"C:\pics")),
                "{label}: {:?} from {line}",
                launch.paths
            );

            match label.as_str() {
                "Start from this folder" => {
                    assert!(launch.start_in, "{line}");
                    assert!(!launch.copy_names && launch.preset.is_none(), "{line}");
                }
                "Start and load selected files" => {
                    assert!(!launch.start_in && !launch.copy_names, "{line}");
                    assert!(launch.preset.is_none(), "{line}");
                    assert_eq!(launch.paths.len(), 2, "the whole selection, {line}");
                }
                "Copy filenames to clipboard" => {
                    assert!(launch.copy_names, "{line}");
                    assert!(!launch.start_in && launch.preset.is_none(), "{line}");
                    assert_eq!(launch.paths.len(), 2, "{line}");
                }
                menu_label => {
                    // The caption is escaped for the menu; the file it points
                    // at is not — and it is the file that has to arrive.
                    let name = menu_label.replace("&&", "&");
                    assert_eq!(
                        launch.preset,
                        Some(PathBuf::from(format!(r"C:\Users\mk\presets\{name}.toml"))),
                        "{line}"
                    );
                    assert!(!launch.start_in && !launch.copy_names, "{line}");
                }
            }
            checked += 1;
        }

        // Three fixed items and two presets on the item side, one and two on
        // the background side. Without this, an extractor that matched nothing
        // would pass in silence — which is the usual way a test like this
        // stops testing anything.
        assert_eq!(checked, 8, "every command in the plan, and no fewer");
    }
    /// A drive root is the one path a menu line cannot carry through the C
    /// runtime's splitter: `"E:\"` reads as `E:"` with the quote still open,
    /// and every argument after it is swallowed. `main` re-splits a
    /// `--from-shell` line with `ren_platform::split_verbatim`, so that is the
    /// splitter this checks every menu command against — on a selection of
    /// two drives, and on one drive's background.
    #[test]
    fn a_drive_root_from_the_menu_arrives_whole() {
        use ren_platform::shell::{MenuPreset, ShellPlan, Step, Value};

        let presets = [MenuPreset {
            name: "Tidy up",
            file: std::path::Path::new(r"C:\Users\mk\presets\Tidy up.toml"),
        }];
        let plan = ShellPlan::build(
            std::path::Path::new(r"C:\Apps\Ren It\renameit.exe"),
            &presets,
        );

        let mut checked = 0;
        for step in &plan.steps {
            let Step::Set { key, name, value } = step else {
                continue;
            };
            let Value::Sz(text) = value else { continue };
            if !name.is_empty() || !key.ends_with(r"\command") {
                continue;
            }
            let line = text.replace("%1", r#""E:\" "F:\""#).replace("%V", r"E:\");
            let launch = parse(ren_platform::split_verbatim(std::ffi::OsStr::new(&line)));

            assert!(launch.from_shell, "{line}");
            assert_eq!(launch.complaint, None, "{line}");
            let one = [PathBuf::from(r"E:\")];
            let two = [PathBuf::from(r"E:\"), PathBuf::from(r"F:\")];
            assert!(
                launch.paths == one || launch.paths == two,
                "{:?} from {line}",
                launch.paths
            );
            checked += 1;
        }
        // Three fixed items and the preset on the item side; one fixed item
        // and the preset on the background side.
        assert_eq!(checked, 6, "every command in the plan, and no fewer");
    }
}
