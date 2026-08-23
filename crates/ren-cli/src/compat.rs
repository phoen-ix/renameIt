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
//! # What is not translated, and why
//!
//! * **`/x`** — legacy "do not close the window when the rename finishes". A
//!   headless CLI has no window to keep open. Accepted and ignored, with a
//!   note, because failing on it would break a batch file that has carried it
//!   for a decade.
//! * Neither `/f` nor `/d` given once meant "use the last used settings".
//!   Ours is stateless by design: a
//!   command line that renames differently depending on what somebody last
//!   clicked in a GUI is not one you can put in a scheduler. Omitting both
//!   means files-not-folders here, which is what `/f` alone would have said.

/// What a legacy command line became.
pub struct Translated {
    /// Arguments to hand to clap, `argv[0]` included.
    pub argv: Vec<String>,
    /// Lines for `--verbose`, and for the switches we accept but ignore.
    pub notes: Vec<String>,
}

/// Whether an argument looks like a legacy switch.
///
/// `-?` and `\?` are accepted alongside `/?`: all three spellings are in the
/// wild.
fn switch_of(arg: &str) -> Option<char> {
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

/// True if this command line is a legacy one rather than ours.
///
/// Two shapes count. A legacy **switch** is the obvious one. The other is the
/// switchless case: with no switches at all, the command line is a list of
/// files to load into free select mode, or a single folder to start in —
/// which is how a Send To shortcut invokes it. That form is claimed only when
/// **every** argument is a path that exists and the first is not one of our
/// subcommands — so a mistyped `ren-cli prevew /tmp` still reaches clap and
/// gets a real error rather than being silently reinterpreted.
pub fn looks_legacy(args: &[String]) -> bool {
    let rest = &args[1.min(args.len())..];
    if rest.iter().any(|a| switch_of(a).is_some()) {
        return true;
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
    !rest.is_empty() && !SUBCOMMANDS.contains(&rest[0].as_str()) && is_path(&rest[0])
}

/// A real path, and not something that only looks like one.
fn is_path(arg: &str) -> bool {
    !arg.starts_with('-') && std::path::Path::new(arg).exists()
}

/// The switchless form: a list of files to load into free select mode, or a
/// single folder to start in.
///
/// Always a `preview`. There is no preset to run, so listing what is there is
/// the only reading that cannot surprise somebody whose Send To shortcut
/// suddenly renames a selection.
fn bare_paths(program: String, rest: &[String]) -> Translated {
    let mut argv = vec![program, "preview".to_owned()];
    let mut notes = Vec::new();

    // Anything beginning with `-` is one of ours and travels through untouched.
    let (paths, flags): (Vec<&String>, Vec<&String>) =
        rest.iter().partition(|a| !a.starts_with('-'));

    if paths.len() == 1 && std::path::Path::new(paths[0]).is_dir() {
        notes.push(format!(
            "a single folder, {}, is the folder to list",
            paths[0]
        ));
        argv.push(paths[0].clone());
    } else {
        notes.push(format!(
            "{} with no switches load into free select, as a Send To shortcut sends them",
            ren_core::plural(paths.len(), "path")
        ));
        for path in paths {
            argv.push("--file".to_owned());
            argv.push(path.clone());
        }
    }
    argv.extend(flags.into_iter().cloned());
    Translated { argv, notes }
}

/// Split `/p c:\download\*.*` into a directory and a mask.
///
/// `/p` sets the startup path. Anything on the end that is not a valid folder
/// (e.g. `*.mp3`) is the file pattern instead.
///
/// The rule is "a valid folder", so the test is whether the whole
/// thing is a directory *on this machine* — which means the same command line
/// can split differently on a machine where the folder is missing. That is the
/// legacy behaviour, and the alternative (guessing from the presence of a `*`)
/// would disagree with it on a folder legitimately named `*`.
fn split_path(raw: &str) -> (String, Option<String>) {
    let path = std::path::Path::new(raw);
    if path.is_dir() {
        return (raw.to_owned(), None);
    }
    match path.parent().zip(path.file_name()) {
        Some((parent, mask)) if !parent.as_os_str().is_empty() => (
            parent.to_string_lossy().into_owned(),
            Some(mask.to_string_lossy().into_owned()),
        ),
        // No separator at all: a bare mask, against the current directory.
        _ => (".".to_owned(), Some(raw.to_owned())),
    }
}

/// Translate a legacy command line into a modern one.
///
/// The subcommand follows the legacy behaviour: without a preset (`/r`) the
/// command line only loads the specified path (`/p`) or file list (`/l`).
///
/// So `/r` means *do it* — `apply` — and its absence means *show me* —
/// `preview`. A batch file that renamed things still renames things, and one
/// that only loaded a folder still only lists it.
pub fn translate(args: &[String]) -> Translated {
    let program = args.first().cloned().unwrap_or_default();
    let mut notes = Vec::new();

    // The switchless form: bare paths, which is what a Send To shortcut sends.
    if !args.iter().skip(1).any(|a| switch_of(a).is_some()) {
        return bare_paths(program, &args[1.min(args.len())..]);
    }

    let mut dir: Option<String> = None;
    let mut pattern: Option<String> = None;
    let mut preset: Option<String> = None;
    let mut list: Option<String> = None;
    let mut delete_list = false;
    let mut include_files = false;
    let mut include_folders = false;
    let mut subfolders = false;
    let mut help = false;
    // Anything that is not a legacy switch or its value is passed through, so
    // `--journal-dir` and friends still work alongside the old flags.
    let mut passthrough: Vec<String> = Vec::new();

    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        let Some(switch) = switch_of(arg) else {
            passthrough.push(arg.clone());
            continue;
        };
        let mut value = || rest.next().cloned().unwrap_or_default();
        match switch {
            'p' => {
                let (d, m) = split_path(&value());
                if let Some(m) = &m {
                    notes.push(format!("/p split into {d} with pattern {m}"));
                }
                dir = Some(d);
                pattern = m;
            }
            'l' => list = Some(value()),
            'r' => preset = Some(value()),
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
        argv.push("--help".to_owned());
        return Translated { argv, notes };
    }

    // Given both a path (`/p`) and a file list (`/l`), the list wins.
    if list.is_some() && dir.is_some() {
        notes.push("note: /l takes precedence over /p".to_owned());
        dir = None;
        pattern = None;
    }

    argv.push(if preset.is_some() { "apply" } else { "preview" }.to_owned());
    if let Some(dir) = dir {
        argv.push(dir);
    }
    if let Some(pattern) = pattern {
        argv.push("--pattern".to_owned());
        argv.push(pattern);
    }
    if let Some(preset) = preset {
        argv.push("--preset".to_owned());
        argv.push(preset);
    }
    if let Some(list) = list {
        argv.push("--list".to_owned());
        argv.push(list);
    }
    if delete_list {
        argv.push("--delete-list".to_owned());
    }
    if subfolders {
        argv.push("--subfolders".to_owned());
    }
    // `/f` and `/d` are additive; ours are "include folders" and "exclude
    // files", so the *combination* is what translates.
    if include_folders {
        argv.push("--folders".to_owned());
        if !include_files {
            argv.push("--no-files".to_owned());
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

    fn argv(line: &[&str]) -> Vec<String> {
        let mut all = vec!["ren-cli".to_owned()];
        all.extend(line.iter().map(|s| (*s).to_owned()));
        translate(&all).argv[1..].to_vec()
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

    /// A real folder stays whole; anything else splits at the last separator.
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
    /// on Linux and, on Windows, a root plus the mask `tmp` — so hard-coding
    /// one tested the split rather than the switches, and only on one OS.
    #[test]
    fn the_include_switches_combine_the_way_the_original_means_them() {
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

    /// > *"If you specify both a path (/p) and a file list (/l), the file list
    /// > will take precedence."*
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
        let all = vec!["ren-cli".into(), "/p".into(), real.clone(), "/x".into()];
        let out = translate(&all);
        assert_eq!(out.argv[1..], ["preview", &real]);
        assert!(
            out.notes.iter().any(|n| n.contains("/x")),
            "{:?}",
            out.notes
        );
    }

    /// Undocumented, but the binary answers all three spellings.
    #[test]
    fn the_undocumented_help_switches_all_work() {
        for spelling in ["/?", "\\?", "-?"] {
            assert_eq!(argv(&[spelling]), ["--help"], "{spelling}");
        }
    }

    /// The layer only engages for a genuinely legacy command line, so a modern
    /// one is never rewritten.
    #[test]
    fn a_modern_command_line_is_left_alone() {
        let modern: Vec<String> = ["ren-cli", "preview", "/tmp", "--pattern", "*.mp3"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert!(!looks_legacy(&modern));

        let legacy: Vec<String> = ["ren-cli", "/p", "/tmp"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert!(looks_legacy(&legacy));
    }

    /// A long flag is ours, not theirs — `--preset` must not be read as `/p`.
    #[test]
    fn modern_long_flags_are_not_mistaken_for_switches() {
        assert_eq!(switch_of("--preset"), None);
        assert_eq!(switch_of("--p"), None);
        assert_eq!(
            switch_of("-p"),
            None,
            "a single dash is not a legacy switch"
        );
        assert_eq!(switch_of("/p"), Some('p'));
        assert_eq!(switch_of("/P"), Some('p'), "switches are case-insensitive");
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

        let line = vec!["ren-cli".to_owned(), a.clone(), b.clone()];
        assert!(looks_legacy(&line));
        assert_eq!(
            translate(&line).argv[1..],
            ["preview", "--file", &a, "--file", &b]
        );
    }

    /// A single bare folder is the folder to start in.
    #[test]
    fn a_single_bare_folder_is_the_folder_to_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let line = vec!["ren-cli".to_owned(), path.clone()];
        assert!(looks_legacy(&line));
        assert_eq!(translate(&line).argv[1..], ["preview", &path]);
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

        let line = vec!["ren-cli".to_owned(), real.clone(), gone.clone()];
        assert!(
            looks_legacy(&line),
            "the first path is real, so this is the old form"
        );
        assert_eq!(
            translate(&line).argv[1..],
            ["preview", "--file", &real, "--file", &gone]
        );
    }

    /// And our own flags survive alongside bare paths.
    #[test]
    fn modern_flags_pass_through_the_switchless_form() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let line = vec!["ren-cli".to_owned(), path.clone(), "--verbose".to_owned()];
        assert!(looks_legacy(&line));
        assert_eq!(translate(&line).argv[1..], ["preview", &path, "--verbose"]);
    }

    /// The guard on that: a mistyped modern command must reach clap and get a
    /// real error, not be silently reread as a list of files.
    #[test]
    fn a_mistyped_subcommand_is_not_mistaken_for_a_path() {
        let line = vec!["ren-cli".to_owned(), "prevew".to_owned(), "/tmp".to_owned()];
        assert!(
            !looks_legacy(&line),
            "'prevew' does not exist, so this is not a path list"
        );

        // And a real subcommand is never claimed even when a file of that name
        // happens to exist in the working directory.
        let line = vec![
            "ren-cli".to_owned(),
            "preview".to_owned(),
            "/tmp".to_owned(),
        ];
        assert!(!looks_legacy(&line));
    }

    /// Arguments the layer does not recognise travel through untouched, so
    /// `--journal-dir` still works beside the old switches.
    #[test]
    fn unrecognised_arguments_pass_through() {
        let all: Vec<String> = ["ren-cli", "/p", "/tmp", "--journal-dir", "/j"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let out = translate(&all);
        assert!(out.argv.contains(&"--journal-dir".to_owned()));
        assert!(out.argv.contains(&"/j".to_owned()));
    }
}
