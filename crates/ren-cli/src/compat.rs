//! Legacy slash switches, translated before clap ever sees them.
//!
//! A legacy command line loads files and then runs one of your presets on that
//! path. Eight switches, `/p /l /r /f /d /s /k /x`, plus `/?`. Accepting them
//! is what lets an existing batch file keep working.
//!
//! # Why a front layer rather than clap aliases
//!
//! Two of the switches do not survive contact with an argument parser. `/p`
//! carries a **path and a mask fused into one argument** — `/p c:\download\*.*`
//! is a directory plus `*.*` — and clap has nowhere to put the second half.
//! And `/f`/`/d` are additive include flags whose *combination* decides the
//! listing, where ours are `--folders` and `--no-files`: `/d` alone means
//! folders-and-not-files, which is two modern flags from one legacy one.
//!
//! So the translation happens here, in front, where it can be one function with
//! its own tests — and `--verbose` prints what a command line became, so
//! somebody porting a batch file can see it rather than guess.
//!
//! # Arguments are `OsString`, not `String`
//!
//! A name that is not valid Unicode is exactly what a renamer gets pointed at,
//! and `std::env::args` panics on one. A switch is always ASCII, so it is
//! recognised through `to_str`; every other argument travels through
//! untouched.
//!
//! # What is not translated, and why
//!
//! * **`/x`** — legacy "do not close the window when the rename finishes". A
//!   headless CLI has no window to keep open. Accepted and ignored, with a
//!   note, because failing on it would break a batch file that has carried it
//!   for a decade.
//! * **Neither `/f` nor `/d`** lists files and not folders. Ours is stateless
//!   by design: a command line that renamed differently depending on what
//!   somebody last clicked in a GUI is not one you can put in a scheduler, so
//!   there is no "whatever was used last" to fall back to. Files-not-folders is
//!   what `/f` alone says, and the commonest thing to want.

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// What a legacy command line became.
pub struct Translated {
    /// Arguments to hand to clap, `argv[0]` included.
    pub argv: Vec<OsString>,
    /// Lines for `--verbose`, and for the switches we accept but ignore.
    pub notes: Vec<String>,
}

/// Whether an argument looks like a legacy switch.
///
/// `-?` and `\?` are accepted alongside `/?`: all three spellings are in
/// common use for "help" on Windows.
fn switch_of(arg: &OsStr) -> Option<char> {
    // A switch is ASCII, so an argument that is not Unicode is never one.
    let arg = arg.to_str()?;
    let mut chars = arg.chars();
    let lead = chars.next()?;
    if !matches!(lead, '/' | '\\' | '-') {
        return None;
    }
    let rest: String = chars.collect();
    // A single character only. `--preset` starts with '-' and is ours.
    if rest.chars().count() != 1 {
        return None;
    }
    let c = rest.chars().next()?.to_ascii_lowercase();
    match c {
        // `-p`, `-l` and so on would collide with nothing today, but a
        // *modern* single-dash flag is not something this layer should claim.
        // Only `?` is accepted with a leading dash.
        '?' => Some('?'),
        'p' | 'l' | 'r' | 'f' | 'd' | 's' | 'k' | 'x' if lead == '/' => Some(c),
        _ => None,
    }
}

/// Our own subcommands, so a bare `preview` is never mistaken for a file that
/// happens to sit in the working directory.
const SUBCOMMANDS: &[&str] = &["preview", "apply", "undo", "recover", "presets", "help"];

fn is_subcommand(arg: &OsStr) -> bool {
    arg.to_str().is_some_and(|arg| SUBCOMMANDS.contains(&arg))
}

/// True if this command line is a legacy one rather than ours.
///
/// Two shapes count. A legacy **switch** is the obvious one — but only before
/// any of our subcommands: a line that names `preview` or `apply` is ours, and
/// on Linux `/d` or `/s` after it is an ordinary folder at the root, or the
/// value of a flag like `--journal-dir`. The other is the switchless case:
/// with no switches at all, the command line is a list of files to load into
/// free select mode, or a single folder to start in — which is how a Send To
/// shortcut invokes it. That form is claimed only when the **first** argument
/// is a path that exists and is not one of our subcommands, so a mistyped
/// `ren-cli prevew /tmp` still reaches clap and gets a real error rather than
/// being silently reinterpreted.
pub fn looks_legacy(args: &[OsString]) -> bool {
    let rest = &args[1.min(args.len())..];
    for arg in rest {
        if is_subcommand(arg) {
            return false;
        }
        if switch_of(arg).is_some() {
            return true;
        }
    }
    // The **first** argument decides, not all of them. Requiring every path to
    // exist meant that one file deleted since the shortcut was made threw the
    // whole line at clap, which then complained about an unrecognised
    // subcommand and named the *first* path — a message about the wrong file,
    // for the wrong reason. Claimed here, the missing one is reported by name
    // when the listing is built.
    //
    // Flags are allowed alongside, so `ren-cli file.txt --verbose` still works;
    // the first argument still has to be a real path, so a mistyped subcommand
    // reaches clap and gets a proper error.
    rest.first().is_some_and(|first| is_path(first))
}

fn is_flag(arg: &OsStr) -> bool {
    arg.as_encoded_bytes().first() == Some(&b'-')
}

/// A real path, and not something that only looks like one.
fn is_path(arg: &OsStr) -> bool {
    !is_flag(arg) && Path::new(arg).exists()
}

/// The switchless form: a list of files to load into free select mode, or a
/// single folder to start in.
///
/// Always a `preview`. There is no preset to run, so listing what is there is
/// the only reading that cannot surprise somebody whose Send To shortcut
/// suddenly renames a selection.
fn bare_paths(program: OsString, rest: &[OsString]) -> Translated {
    let mut argv = vec![program, "preview".into()];
    let mut notes = Vec::new();

    // Anything from the first `-` onwards is ours and travels through
    // untouched — *including* what follows a flag. Partitioning every argument
    // by its first character put a flag's value in with the paths, so
    // `dir --journal-dir /j` turned `/j` into a file to rename and left the
    // flag dangling for clap to reject.
    let first_flag = rest.iter().position(|a| is_flag(a)).unwrap_or(rest.len());
    let (paths, flags) = rest.split_at(first_flag);

    if paths.len() == 1 && Path::new(&paths[0]).is_dir() {
        notes.push(format!(
            "a single folder, {}, is the folder to list",
            Path::new(&paths[0]).display()
        ));
        argv.push(paths[0].clone());
    } else {
        notes.push(format!(
            "{} with no switches load into free select, as a Send To shortcut sends them",
            ren_core::plural(paths.len(), "path")
        ));
        for path in paths {
            argv.push("--file".into());
            argv.push(path.clone());
        }
    }
    argv.extend(flags.iter().cloned());
    Translated { argv, notes }
}

/// What a `/p` value turned out to name.
#[derive(Debug, PartialEq, Eq)]
enum PathArg {
    /// A folder to list — or a path that is not there, kept whole so the
    /// listing fails and names it.
    Folder(OsString),
    /// An existing file: that file and nothing else.
    File(OsString),
    /// A folder and a mask fused into one argument: `C:\download\*.mp3`.
    Masked { dir: OsString, mask: OsString },
}

/// Reads `/p c:\download\*.*` as a directory and a mask.
///
/// Four readings, in order:
///
/// 1. **An existing folder** is the folder to list — including one whose
///    name contains `*`, which Linux allows.
/// 2. **An existing file** is that one file, loaded as Free Select would
///    load it. Split into its folder plus its name as a mask, it used to
///    select every name in the folder *containing* the file's name, and a
///    `/r` renamed all of them.
/// 3. **A last component with `*` or `?`** is a mask over the folder before
///    it. A value with no folder part is a mask over the current directory.
/// 4. **Anything else** — a folder that has been moved or deleted since the
///    batch file was written — stays whole, so the listing fails and says
///    which folder is missing. Read as a mask it became "every name in the
///    parent containing this word", and a scheduled `/r` renamed those
///    instead, night after night, with exit code 0.
fn split_path(raw: &OsStr) -> PathArg {
    let path = Path::new(raw);
    if path.is_dir() {
        return PathArg::Folder(raw.to_owned());
    }
    if path.is_file() {
        return PathArg::File(raw.to_owned());
    }
    let wild = path.file_name().is_some_and(|name| {
        name.as_encoded_bytes()
            .iter()
            .any(|b| matches!(b, b'*' | b'?'))
    });
    if !wild {
        return PathArg::Folder(raw.to_owned());
    }
    match path.parent().zip(path.file_name()) {
        Some((parent, mask)) if !parent.as_os_str().is_empty() => PathArg::Masked {
            dir: parent.as_os_str().to_owned(),
            mask: mask.to_owned(),
        },
        // No separator at all: a bare mask, against the current directory.
        _ => PathArg::Masked {
            dir: ".".into(),
            mask: raw.to_owned(),
        },
    }
}

/// Translate a legacy command line into a modern one.
///
/// Without a preset (`/r`) a legacy line only loads the path (`/p`) or the
/// file list (`/l`); with one, it runs it. So `/r` means *do it* — `apply` —
/// and its absence means *show me* — `preview`. A batch file that renamed
/// things still renames things, and one that only loaded a folder still only
/// lists it.
pub fn translate(args: &[OsString]) -> Translated {
    let program = args.first().cloned().unwrap_or_default();
    let mut notes = Vec::new();

    // The switchless form: bare paths, which is what a Send To shortcut sends.
    if !args.iter().skip(1).any(|a| switch_of(a).is_some()) {
        return bare_paths(program, &args[1.min(args.len())..]);
    }

    let mut dir: Option<OsString> = None;
    let mut file: Option<OsString> = None;
    let mut pattern: Option<OsString> = None;
    let mut preset: Option<OsString> = None;
    let mut list: Option<OsString> = None;
    let mut delete_list = false;
    let mut include_files = false;
    let mut include_folders = false;
    let mut subfolders = false;
    let mut help = false;
    // Anything that is not a legacy switch or its value is passed through, so
    // `--journal-dir` and friends still work alongside the old flags.
    let mut passthrough: Vec<OsString> = Vec::new();

    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        let Some(switch) = switch_of(arg) else {
            passthrough.push(arg.clone());
            continue;
        };
        // A switch's value is the next argument — unless there is none, or
        // the next argument is itself a switch. Defaulting to `""` there used
        // to turn `/p` at the end of a line into "list the current folder"
        // and `/p /s` into a pattern called `/s` with Subfolders lost; both
        // are the line the user typed missing something, and are said.
        let mut value = |name: &str| -> Option<OsString> {
            match rest.clone().next() {
                Some(next) if switch_of(next).is_none() => rest.next().cloned(),
                _ => {
                    notes.push(format!("note: {name} needs a value and had none; ignored"));
                    None
                }
            }
        };
        match switch {
            'p' => {
                if let Some(raw) = value("/p") {
                    dir = None;
                    file = None;
                    pattern = None;
                    match split_path(&raw) {
                        PathArg::Folder(d) => dir = Some(d),
                        PathArg::File(f) => {
                            notes.push(format!(
                                "/p names a file, {}, so only that file is loaded",
                                Path::new(&f).display()
                            ));
                            file = Some(f);
                        }
                        PathArg::Masked { dir: d, mask } => {
                            notes.push(format!(
                                "/p split into {} with pattern {}",
                                Path::new(&d).display(),
                                Path::new(&mask).display()
                            ));
                            dir = Some(d);
                            pattern = Some(mask);
                        }
                    }
                }
            }
            'l' => list = value("/l"),
            'r' => preset = value("/r"),
            'f' => include_files = true,
            'd' => include_folders = true,
            's' => subfolders = true,
            'k' => delete_list = true,
            'x' => notes.push(
                "note: /x asks the window to stay open after renaming, and a command-line \
                 run has no window. Ignored."
                    .to_owned(),
            ),
            '?' => help = true,
            _ => passthrough.push(arg.clone()),
        }
    }

    let mut argv = vec![program];
    if help {
        argv.push("--help".into());
        return Translated { argv, notes };
    }

    // Given both a path (`/p`) and a file list (`/l`), the list wins: it is
    // the more specific of the two, and a shell integration that passes a
    // selection as a list must not have it widened to a whole folder.
    if list.is_some() && (dir.is_some() || file.is_some()) {
        notes.push("note: /l takes precedence over /p".to_owned());
        dir = None;
        file = None;
        pattern = None;
    }

    argv.push(if preset.is_some() { "apply" } else { "preview" }.into());
    if let Some(dir) = dir {
        argv.push(dir);
    }
    if let Some(pattern) = pattern {
        argv.push("--pattern".into());
        argv.push(pattern);
    }
    if let Some(file) = file {
        argv.push("--file".into());
        argv.push(file);
    }
    if let Some(preset) = preset {
        argv.push("--preset".into());
        argv.push(preset);
    }
    let has_list = list.is_some();
    if let Some(list) = list {
        argv.push("--list".into());
        argv.push(list);
    }
    // `/k` without `/l` has nothing to delete. Dropped with a note rather
    // than passed on: clap would reject `--delete-list` without `--list`
    // with a usage dump for a flag the user never typed.
    if delete_list && has_list {
        argv.push("--delete-list".into());
    } else if delete_list {
        notes.push("note: /k has no /l list to delete; ignored".to_owned());
    }
    if subfolders {
        argv.push("--subfolders".into());
    }
    // `/f` and `/d` are additive; ours are "include folders" and "exclude
    // files", so the *combination* is what translates.
    if include_folders {
        argv.push("--folders".into());
        if !include_files {
            argv.push("--no-files".into());
        }
    }
    // Anything this layer did not recognise goes on the end untouched, so a
    // modern global like `--journal-dir` still works beside the old switches.
    argv.extend(passthrough);

    Translated { argv, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(args: &[&str]) -> Vec<OsString> {
        std::iter::once("ren-cli")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect()
    }

    fn strings(argv: &[OsString]) -> Vec<String> {
        argv.iter()
            .map(|a| a.clone().into_string().unwrap())
            .collect()
    }

    fn argv(args: &[&str]) -> Vec<String> {
        strings(&translate(&line(args)).argv[1..])
    }

    /// The canonical scheduled-task line, end to end:
    /// `/p c:\download\*.* /r "my preset" /f`
    #[test]
    fn the_canonical_example_translates() {
        let out = argv(&["/p", "/tmp/download/*.*", "/r", "my preset", "/f"]);
        assert_eq!(
            out,
            [
                "apply",
                "/tmp/download",
                "--pattern",
                "*.*",
                "--preset",
                "my preset"
            ],
            "/f alone is the default, so it adds nothing"
        );
    }

    /// `/r` means run it; without one the folder is only listed.
    #[test]
    fn a_preset_applies_and_its_absence_only_previews() {
        assert_eq!(argv(&["/p", "/tmp"])[0], "preview");
        assert_eq!(argv(&["/p", "/tmp", "/r", "p"])[0], "apply");
    }

    /// A real folder stays whole; a mask splits at the last separator.
    #[test]
    fn a_path_splits_into_a_folder_and_a_mask_only_when_it_has_to() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().to_string_lossy().into_owned();
        assert_eq!(argv(&["/p", &real]), ["preview", &real]);

        let masked = format!("{real}/*.mp3");
        assert_eq!(
            argv(&["/p", &masked]),
            ["preview", &real, "--pattern", "*.mp3"]
        );
    }

    /// An existing file is that file — never its name as a substring filter
    /// over the folder, which selected `hosts`, `hosts.allow` and
    /// `hosts.deny` for `/p /etc/hosts`.
    #[test]
    fn a_path_naming_a_file_loads_only_that_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("hosts");
        std::fs::write(&file, "").unwrap();
        std::fs::write(dir.path().join("hosts.allow"), "").unwrap();
        let file = file.to_string_lossy().into_owned();
        assert_eq!(
            argv(&["/p", &file, "/r", "tidy"]),
            ["apply", "--file", &file, "--preset", "tidy"]
        );
    }

    /// A folder that is not there any more stays whole, so the run fails
    /// naming it — rather than renaming every name in its parent that
    /// contains its last word.
    #[test]
    fn a_missing_folder_is_kept_whole_rather_than_read_as_a_mask() {
        let dir = tempfile::TempDir::new().unwrap();
        let gone = dir.path().join("raw").to_string_lossy().into_owned();
        assert_eq!(
            argv(&["/p", &gone, "/r", "tidy"]),
            ["apply", &gone, "--preset", "tidy"]
        );
    }

    /// A bare mask with no folder in it means the current directory.
    #[test]
    fn a_bare_mask_lists_the_current_directory() {
        assert_eq!(
            argv(&["/p", "*.mp3"]),
            ["preview", ".", "--pattern", "*.mp3"]
        );
    }

    /// `/f` and `/d` are additive there; here they are two different flags.
    ///
    /// The path has to be a folder that really exists, because `split_path`
    /// asks the machine (see its doc comment). A literal `/tmp` is a directory
    /// on Linux and, on Windows, a root plus the name `tmp` — so hard-coding
    /// one tested the split rather than the switches, and only on one OS.
    #[test]
    fn the_include_switches_combine_into_two_modern_flags() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().to_string_lossy().into_owned();
        assert_eq!(argv(&["/p", &real, "/f"]), ["preview", &real]);
        assert_eq!(
            argv(&["/p", &real, "/d"]),
            ["preview", &real, "--folders", "--no-files"],
            "/d alone is folders and not files"
        );
        assert_eq!(
            argv(&["/p", &real, "/f", "/d"]),
            ["preview", &real, "--folders"],
            "both means both"
        );
        assert_eq!(
            argv(&["/p", &real, "/s"]),
            ["preview", &real, "--subfolders"]
        );
    }

    #[test]
    fn a_file_list_becomes_free_select() {
        assert_eq!(
            argv(&["/l", "files.txt", "/k"]),
            ["preview", "--list", "files.txt", "--delete-list"]
        );
    }

    /// `/l` wins over `/p` (see `translate`).
    #[test]
    fn a_file_list_wins_over_a_path() {
        let out = argv(&["/p", "/tmp/*.mp3", "/l", "files.txt"]);
        assert_eq!(out, ["preview", "--list", "files.txt"]);
        assert!(!out.contains(&"--pattern".to_owned()));
    }

    /// The switch that has nothing to translate to: accepted, ignored, and
    /// said out loud rather than silently dropped.
    #[test]
    fn the_keep_window_open_switch_is_accepted_and_explained() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().to_string_lossy().into_owned();
        let out = translate(&line(&["/p", &real, "/x"]));
        assert_eq!(strings(&out.argv[1..]), ["preview", &real]);
        assert!(
            out.notes.iter().any(|n| n.contains("/x")),
            "{:?}",
            out.notes
        );
    }

    /// All three spellings of "help" are accepted.
    #[test]
    fn every_spelling_of_the_help_switch_works() {
        for spelling in ["/?", "\\?", "-?"] {
            assert_eq!(argv(&[spelling]), ["--help"], "{spelling}");
            assert!(looks_legacy(&line(&[spelling])), "{spelling}");
        }
    }

    /// The layer only engages for a genuinely legacy command line, so a modern
    /// one is never rewritten.
    #[test]
    fn a_modern_command_line_is_left_alone() {
        assert!(!looks_legacy(&line(&[
            "preview",
            "/tmp",
            "--pattern",
            "*.mp3"
        ])));
        assert!(looks_legacy(&line(&["/p", "/tmp"])));
    }

    /// On Linux `/d` and `/s` are ordinary folders at the root, and a flag's
    /// value can be spelled like a switch. After one of our subcommands none
    /// of them is legacy: `ren-cli preview /d` became `preview --folders
    /// --no-files preview`.
    #[test]
    fn a_switch_after_a_subcommand_is_a_path_not_a_switch() {
        assert!(!looks_legacy(&line(&["preview", "/d"])));
        assert!(!looks_legacy(&line(&["apply", "x", "--journal-dir", "/s"])));
        assert!(!looks_legacy(&line(&["--verbose", "undo"])));
        // A modern flag *before* the legacy switches is still a legacy line.
        assert!(looks_legacy(&line(&["--verbose", "/p", "/tmp"])));
    }

    /// A long flag is ours, not theirs — `--preset` must not be read as `/p`.
    #[test]
    fn modern_long_flags_are_not_mistaken_for_switches() {
        let switch = |s: &str| switch_of(OsStr::new(s));
        assert_eq!(switch("--preset"), None);
        assert_eq!(switch("--p"), None);
        assert_eq!(switch("-p"), None, "a single dash is not a legacy switch");
        assert_eq!(switch("/p"), Some('p'));
        assert_eq!(switch("/P"), Some('p'), "switches are case-insensitive");
    }

    /// With no switches, the command line is a list of files to load into free
    /// select mode — which is how a Send To shortcut invokes it.
    #[test]
    fn bare_paths_load_into_free_select() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        let (a, b) = (
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        );

        let args = line(&[&a, &b]);
        assert!(looks_legacy(&args));
        assert_eq!(
            strings(&translate(&args).argv[1..]),
            ["preview", "--file", &a, "--file", &b]
        );
    }

    /// A single bare folder is the folder to start in.
    #[test]
    fn a_single_bare_folder_is_the_folder_to_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let args = line(&[&path]);
        assert!(looks_legacy(&args));
        assert_eq!(strings(&translate(&args).argv[1..]), ["preview", &path]);
    }

    /// A path that has since been deleted must not throw the whole line at
    /// clap: the run is claimed and the missing file named when the listing is
    /// built, rather than clap complaining about the *first* path instead.
    #[test]
    fn a_missing_path_among_the_arguments_still_claims_the_line() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().join("here.txt");
        std::fs::write(&real, "").unwrap();
        let real = real.to_string_lossy().into_owned();
        let gone = dir.path().join("gone.txt").to_string_lossy().into_owned();

        let args = line(&[&real, &gone]);
        assert!(
            looks_legacy(&args),
            "the first path is real, so this is the old form"
        );
        assert_eq!(
            strings(&translate(&args).argv[1..]),
            ["preview", "--file", &real, "--file", &gone]
        );
    }

    /// And our own flags survive alongside bare paths.
    #[test]
    fn modern_flags_pass_through_the_switchless_form() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let args = line(&[&path, "--verbose"]);
        assert!(looks_legacy(&args));
        assert_eq!(
            strings(&translate(&args).argv[1..]),
            ["preview", &path, "--verbose"]
        );
    }

    /// A switch at the end of the line, or followed by another switch, has
    /// no value — and says so, rather than reading `""` and listing the
    /// current folder or swallowing the next switch.
    #[test]
    fn a_switch_with_no_value_is_noted_and_ignored() {
        assert_eq!(argv(&["/p"]), ["preview"]);
        assert_eq!(argv(&["/p", "/s"]), ["preview", "--subfolders"]);
        let translated = translate(&line(&["/r"]));
        assert_eq!(strings(&translated.argv[1..]), ["preview"]);
        assert!(
            translated
                .notes
                .iter()
                .any(|n| n.contains("/r needs a value"))
        );
    }

    /// `/k` with no `/l` has nothing to delete; it is dropped with a note
    /// rather than handed to clap as a flag that requires `--list`.
    #[test]
    fn delete_list_without_a_list_is_dropped() {
        assert_eq!(argv(&["/k", "/f"]), ["preview"]);
    }

    /// A flag's *value* is not a path. Partitioning by first character used
    /// to file `/j` under `--file` and leave `--journal-dir` with nothing.
    #[test]
    fn a_flag_keeps_its_value_in_the_switchless_form() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let args = line(&[&path, "--journal-dir", "/j", "--verbose"]);
        assert!(looks_legacy(&args));
        assert_eq!(
            strings(&translate(&args).argv[1..]),
            ["preview", &path, "--journal-dir", "/j", "--verbose"]
        );
    }

    /// The guard on that: a mistyped modern command must reach clap and get a
    /// real error, not be silently reread as a list of files.
    #[test]
    fn a_mistyped_subcommand_is_not_mistaken_for_a_path() {
        assert!(
            !looks_legacy(&line(&["prevew", "/tmp"])),
            "'prevew' does not exist, so this is not a path list"
        );
        // And a real subcommand is never claimed even when a file of that name
        // happens to exist in the working directory.
        assert!(!looks_legacy(&line(&["preview", "/tmp"])));
    }

    /// Arguments the layer does not recognise travel through untouched, so
    /// `--journal-dir` still works beside the old switches.
    #[test]
    fn unrecognised_arguments_pass_through() {
        let out = translate(&line(&["/p", "/tmp", "--journal-dir", "/j"]));
        assert!(out.argv.contains(&"--journal-dir".into()));
        assert!(out.argv.contains(&"/j".into()));
    }

    /// A name that is not Unicode is never a switch and never a panic: it
    /// travels through as the bytes it is.
    #[test]
    #[cfg(unix)]
    fn an_argument_that_is_not_unicode_travels_through_untouched() {
        use std::os::unix::ffi::OsStringExt;

        let odd = OsString::from_vec(b"/nowhere/caf\xe9".to_vec());
        let args = vec![
            "ren-cli".into(),
            "/p".into(),
            odd.clone(),
            "/r".into(),
            "x".into(),
        ];
        assert!(looks_legacy(&args));
        let out = translate(&args);
        assert_eq!(out.argv[2], odd, "a missing folder is kept whole");
        assert!(!looks_legacy(&[
            "ren-cli".into(),
            "preview".into(),
            OsString::from_vec(b"caf\xe9".to_vec())
        ]));
    }

    /// A batch file's `/p "%~dp0" /r tidy` ends its folder in `\"`, which the
    /// C runtime reads as an escaped quote. Split verbatim — what `main` does
    /// for a legacy line on Windows — the switches after it survive.
    #[test]
    fn a_verbatim_split_line_keeps_the_switches_after_a_trailing_backslash() {
        let args = ren_platform::split_verbatim(OsStr::new(
            r#"ren-cli.exe /p "C:\Scripts\" /r "my preset""#,
        ));
        let out = strings(&translate(&args).argv[1..]);
        assert_eq!(out[0], "apply", "{out:?}");
        assert!(
            out.windows(2).any(|w| w == ["--preset", "my preset"]),
            "{out:?}"
        );
    }
}
