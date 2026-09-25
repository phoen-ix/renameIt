//! Headless runner for the RenameIt engine.
//!
//! This is the automation story *and* the test harness — it ships in every
//! release.
//!
//! The modern `--` flags are the real interface (D18, P38). Legacy
//! `/p /l /r /f /d /s /k /x` switches are accepted too, rewritten into those
//! flags by [`compat`] before clap ever sees them, so an existing `.bat` file
//! keeps working without this CLI carrying two grammars.
//!
//! **Every path is made absolute before anything is listed.** A journal
//! records paths exactly as the plan gives them, and `undo` resolves them
//! against *its* working directory — so `apply .` in one folder and `undo` in
//! another would have replayed the batch somewhere else. `ren_core::apply`
//! refuses a relative path outright; this is the front end keeping its side.

mod compat;
mod exit;

use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use exit::Exit;

use clap::{Args, Parser, Subcommand};
use ren_core::exec::{ExecError, UndoReport, default_journal_dir};
use ren_core::model::Scope;
use ren_core::preset::{Preset, PresetStore, default_preset_dir};
use ren_core::{
    AppendSuffix, ApplyOptions, AskSpec, FileEntry, Job, ListOptions, Pipeline, RowState, Step,
    StepConfig, apply, plan, undo_last,
};
use ren_platform::Platform;

type AnyError = Box<dyn std::error::Error>;

/// Shown by `--help` and `/?`: what a script needs to know that no single
/// flag says.
const AFTER_LONG_HELP: &str = "\
Exit codes:
  0  The command did what it was asked.
  1  The command line was wrong, or something failed before any work started.
  2  Refused before anything was touched: the plan has conflicts or errors, a
     folder is one the operating system needs left alone, a change cannot be
     undone and --allow-irreversible was not given, or another run is using the
     journal. Fix the cause and run it again.
  3  The run started and some items did not make it, or an undo could not put
     everything back. Run `ren-cli recover` to see what is unfinished.

Legacy switches, rewritten into the flags above (--verbose shows how):
  /p PATH   The folder to list. PATH\\*.ext lists only matching names; a file
            loads just that file.
  /l FILE   A text file of full paths, one per line. Wins over /p.
  /r NAME   Run this preset (apply). Without /r the line only previews.
  /f /d     Include files / folders. /d alone lists folders and not files.
  /s        Include subfolders.
  /k        Delete the /l file once the run has succeeded.
  /x        Accepted and ignored: there is no window to keep open.
  /?        This help.";

#[derive(Debug, Parser)]
#[command(
    name = "ren-cli",
    version,
    about,
    long_about = None,
    after_help = "Exit codes and the legacy /p /l /r switches: ren-cli --help",
    after_long_help = AFTER_LONG_HELP
)]
struct Cli {
    /// Where transaction journals live. Defaults to the per-user data directory.
    #[arg(long, global = true, value_name = "DIR")]
    journal_dir: Option<PathBuf>,

    /// Explain what a legacy `/p /l /r …` command line was translated into.
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show what would be renamed, without touching anything.
    Preview {
        #[command(flatten)]
        job: JobArgs,
    },
    /// Rename, recording a journal so the batch can be undone.
    Apply {
        #[command(flatten)]
        job: JobArgs,
        /// Run the whole plan but perform no filesystem calls.
        #[arg(long)]
        simulate: bool,
        /// Permit changes that cannot be undone.
        ///
        /// Writing or removing tags rewrites the file and keeps nothing, so
        /// there is no journal entry that could put it back. Without this the
        /// run is refused before anything is touched. The GUI asks instead; a
        /// script has to say so up front.
        //
        // P2.
        #[arg(long)]
        allow_irreversible: bool,
    },
    /// Revert the most recent transaction.
    Undo {
        /// Undo one specific journal file instead of the newest one.
        #[arg(long, value_name = "FILE")]
        txn: Option<PathBuf>,
    },
    /// Report transactions that never finished, and optionally roll them back.
    Recover {
        /// Actually revert what the unfinished transactions did.
        #[arg(long)]
        rollback: bool,
    },
    /// Inspect the saved pipelines.
    Presets {
        #[command(subcommand)]
        action: PresetAction,
    },
}

#[derive(Debug, Subcommand)]
enum PresetAction {
    /// List every preset in the preset folder.
    List {
        #[arg(long, value_name = "DIR")]
        preset_dir: Option<PathBuf>,
    },
    /// Print one preset as it is stored.
    Show {
        name: String,
        #[arg(long, value_name = "DIR")]
        preset_dir: Option<PathBuf>,
    },
    /// Copy a preset file into the preset folder.
    Import {
        file: PathBuf,
        #[arg(long, value_name = "DIR")]
        preset_dir: Option<PathBuf>,
    },
    /// Write a preset out to a file you can share.
    Export {
        name: String,
        to: PathBuf,
        #[arg(long, value_name = "DIR")]
        preset_dir: Option<PathBuf>,
        /// Replace TO if it already exists.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Args)]
struct JobArgs {
    /// Directory to list. Omit when using --job.
    dir: Option<PathBuf>,

    /// A TOML job file describing the source and the pipeline. This is the
    /// same schema presets use.
    ///
    /// A job brings its own folder and listing, so none of the source or
    /// listing flags go with it.
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with_all = [
            "preset", "dir", "list", "file", "pattern", "folders", "no_files",
            "subfolders", "suffix", "scope",
        ]
    )]
    job: Option<PathBuf>,

    /// A saved pipeline: the name of a preset, or the path to a `.toml` file.
    ///
    /// A job file brings its own source; a preset borrows yours — so this one
    /// needs a directory, and the listing flags below apply.
    #[arg(
        long,
        value_name = "NAME|FILE",
        conflicts_with_all = ["job", "suffix", "scope"]
    )]
    preset: Option<String>,

    /// Where presets live. Defaults to the per-user data directory.
    #[arg(long, value_name = "DIR")]
    preset_dir: Option<PathBuf>,

    /// Answer an <Ask> slot without being prompted: `--answer 0=Holiday`.
    ///
    /// Slot 0 is `<Ask>`, 1-9 are `<Ask-1>`…`<Ask-9>`. A slot the pipeline
    /// does not ask for is refused. An unanswered slot is asked on stdin by
    /// `apply`, which fails if stdin closes first; `preview` never asks.
    #[arg(long, value_name = "SLOT=TEXT")]
    answer: Vec<String>,

    /// Quick single-operation path: append this text. Only used when neither
    /// --job nor --preset is given.
    #[arg(long, default_value = "_renamed")]
    suffix: String,

    /// Which part of the file name --suffix may touch. Only used with --suffix.
    #[arg(long, value_enum, default_value = "name")]
    scope: ScopeArg,

    /// Include folders in the listing.
    #[arg(long)]
    folders: bool,

    /// Do not include files in the listing.
    #[arg(long)]
    no_files: bool,

    /// Recurse into subfolders.
    #[arg(long)]
    subfolders: bool,

    /// Listing mask, e.g. `*.mp3`. Empty, `*` and `*.*` all mean everything.
    #[arg(long, default_value = "")]
    pattern: String,

    /// A text file of full paths, one per line, to rename instead of a folder.
    ///
    /// The legacy `/l`, and what a shell integration emits. Blank lines and
    /// `#` comments are skipped; everything else must exist. UTF-8, UTF-16
    /// with a byte-order mark, and the system's legacy code page are all read.
    #[arg(long, value_name = "FILE", conflicts_with = "dir")]
    list: Option<PathBuf>,

    /// One path to rename. Repeatable — the Send To case, where the caller
    /// already has the files and there is no folder to list.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["dir", "list"])]
    file: Vec<PathBuf>,

    /// Where Script cards look for their `.koto` files.
    ///
    /// Defaults to the per-user script folder. A preset names a script but
    /// deliberately does not say where it lives, so this is how a portable run
    /// points at its own folder.
    #[arg(long, value_name = "DIR")]
    script_dir: Option<PathBuf>,

    /// Seed for `<Rnd*>` tags and for scripts that ask for `fr.seed`.
    ///
    /// A fresh one is chosen per invocation, so two runs produce different
    /// random values. Pass one to make a run reproducible.
    //
    // The other half of P16.
    #[arg(long, value_name = "N")]
    seed: Option<u64>,

    /// Delete the --list file after the command succeeds; never under
    /// --simulate.
    ///
    /// The legacy `/k`. Deleting it is not a rename, so it is not journalled
    /// and cannot be undone — which is why a blocked or failed run keeps the
    /// list, so it can be run again.
    //
    // D101, and D111 for why a successful preview counts.
    #[arg(long, requires = "list")]
    delete_list: bool,

    /// Rename inside a folder the operating system needs left alone —
    /// `C:\Windows`, the program folders, `/usr`, `/etc` and the like.
    ///
    /// Refused without this, by `preview` as well as `apply`: a rename there
    /// is recorded faithfully and can stop the machine from starting before
    /// anybody runs `undo`.
    //
    // D127; the GUI's switch is Settings ▸ File System ▸ System folders.
    #[arg(long)]
    allow_system_folders: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ScopeArg {
    Name,
    Extension,
    Both,
}

impl From<ScopeArg> for Scope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Name => Scope::Name,
            ScopeArg::Extension => Scope::Extension,
            ScopeArg::Both => Scope::Both,
        }
    }
}

/// Where the files come from.
///
/// Free Select is not a mode the engine has — `ren_core`'s listing module says
/// it *"is just a `Vec<FileEntry>` the caller assembles"* — so this is the
/// caller doing exactly that. It is what `/l` loads, and what D7's shell
/// launcher hands over.
enum Source {
    Folder { dir: PathBuf, options: ListOptions },
    Free(Vec<PathBuf>),
}

/// Everything a run needs, from either source of truth — before any `<Ask>`
/// has been answered, so nothing is asked of a run that will be refused.
struct Resolved {
    source: Source,
    pipeline: Pipeline,
    /// Things worth saying on stderr before the plan.
    notes: Vec<String>,
}

impl Resolved {
    fn report_notes(&self) {
        for note in &self.notes {
            eprintln!("{note}");
        }
    }

    /// The folder being browsed, which the system-folder guard checks even
    /// when it lists nothing.
    fn browsed(&self) -> Option<&Path> {
        match &self.source {
            Source::Folder { dir, .. } => Some(dir),
            Source::Free(_) => None,
        }
    }

    fn entries(&self) -> Result<Vec<FileEntry>, AnyError> {
        match &self.source {
            Source::Folder { dir, options } => {
                // Named: a scheduled line whose folder has gone otherwise
                // said only "No such file or directory".
                let (entries, problems) = ren_core::listing::list_reporting(dir, options.clone())
                    .map_err(|e| format!("{}: {e}", shown_path(dir)))?;
                // Named, not counted: a run that quietly covered less than the
                // caller asked for is what P63 exists to make visible.
                for problem in &problems {
                    eprintln!("skipped: {}", shown(&problem.to_string()));
                }
                Ok(entries)
            }
            Source::Free(paths) => paths
                .iter()
                .map(|path| {
                    FileEntry::from_path(path).map_err(|e| {
                        // Named, because a list of a thousand paths with one
                        // bad line in it is otherwise a guessing game.
                        format!("{}: {e}", shown_path(path)).into()
                    })
                })
                .collect(),
        }
    }
}

/// `path` made absolute against the working directory, without touching the
/// disk — `..` and links are left for the OS to resolve, as it would have.
fn absolute(path: &Path) -> Result<PathBuf, AnyError> {
    std::path::absolute(path).map_err(|e| format!("{}: {e}", shown_path(path)).into())
}

/// A Free Select source: every path absolute, and each named once.
///
/// Twice in a list is one file renamed twice — the second rename's source is
/// already gone, so the run fails part-way with a `failed:` line for a file
/// that was in fact renamed. The GUI drops a second drop of the same file for
/// the same reason. The first mention is kept, so the order a counter numbers
/// in is the order the list gave.
fn free_source(paths: Vec<PathBuf>, notes: &mut Vec<String>) -> Result<Source, AnyError> {
    let mut seen = HashSet::new();
    let mut kept = Vec::with_capacity(paths.len());
    for path in paths {
        let path = absolute(&path)?;
        if seen.insert(path.clone()) {
            kept.push(path);
        } else {
            notes.push(format!(
                "note: {} is named more than once; it is renamed once",
                shown_path(&path)
            ));
        }
    }
    Ok(Source::Free(kept))
}

/// Read the `/l` file: full paths, one per line.
///
/// Blank lines are skipped, and so are `#` comments — an extension, since a
/// strict reading would treat both as filenames and fail. A list file is
/// usually generated, but it is also the thing a person hand-edits when
/// something went wrong, and a format with no way to leave a note is a worse
/// format for no gain.
///
/// Decoded by `ren_platform::list_file`, because the tools that write one on
/// Windows mostly do not write UTF-8: Windows PowerShell's `Out-File` and `>`
/// write UTF-16, its `Set-Content` the ANSI code page. A byte-order mark is
/// never part of the first path.
fn read_list(path: &Path) -> Result<Vec<PathBuf>, AnyError> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("could not read the list file {}: {e}", shown_path(path)))?;
    let paths: Vec<PathBuf> = ren_platform::list_file::lines(&bytes)
        .into_iter()
        .filter(|line| !line.is_empty() && line.as_encoded_bytes().first() != Some(&b'#'))
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        return Err(format!("the list file {} names no files", shown_path(path)).into());
    }
    Ok(paths)
}

/// How an `<Ask>` slot is written in a template.
fn ask_tag(slot: u8) -> String {
    if slot == 0 {
        "<Ask>".to_owned()
    } else {
        format!("<Ask-{slot}>")
    }
}

impl JobArgs {
    fn resolve(&self) -> Result<Resolved, AnyError> {
        let mut resolved = self.resolve_source()?;
        // Fresh per invocation unless the caller pinned one. A job file that
        // carries its own non-zero seed keeps it, which is how a run is made
        // reproducible.
        match self.seed {
            Some(seed) => resolved.pipeline.settings.seed = seed,
            None if resolved.pipeline.settings.seed == 0 => resolved.pipeline.settings.reseed(),
            None => {}
        }
        resolved.pipeline.settings.script_dir = self.script_dir.clone();
        Ok(resolved)
    }

    fn resolve_source(&self) -> Result<Resolved, AnyError> {
        if let Some(path) = &self.job {
            // clap has already refused every source and listing flag beside
            // `--job`: the job file names its own.
            let job = Job::from_file(path)?;
            return Ok(Resolved {
                source: Source::Folder {
                    dir: absolute(&job.source.dir())?,
                    options: job.source.list_options(),
                },
                pipeline: job.pipeline(),
                notes: Vec::new(),
            });
        }

        let mut notes = Vec::new();
        if let Some(wanted) = &self.preset {
            // A preset borrows the caller's source, so it needs one — a
            // folder, a list file, or an explicit set of paths.
            let source = self
                .source(&mut notes)?
                .ok_or("--preset renames the files you name: pass a folder, --list or --file")?;
            let (preset, import) = self.find_preset(wanted)?;
            if let Some(source) = import.dropped_source {
                notes.push(format!(
                    "note: ignoring the [source] in that file ({}) — a preset runs over the \
                     folder you name",
                    shown_path(&source.dir())
                ));
            }
            return Ok(Resolved {
                source,
                pipeline: preset.pipeline(),
                notes,
            });
        }

        let source = self.source(&mut notes)?.ok_or(
            "give a directory to rename, or --list <FILE>, or --job <FILE>, or --preset <NAME>",
        )?;
        Ok(Resolved {
            source,
            pipeline: Pipeline::new().with(
                Step::Name(Box::new(AppendSuffix::new(self.suffix.clone()))),
                StepConfig::scoped(self.scope.into()),
            ),
            notes,
        })
    }

    /// Delete the `--list` file, if `--delete-list` asked for it.
    ///
    /// The legacy `/k`. Not a rename: it is not journalled and there is no
    /// undo for it, which is exactly why it waits until the run it belonged to
    /// has actually happened rather than firing the moment the file is read.
    /// A failed batch leaves the list where it was, so it can be run again.
    ///
    /// **A warning, never a failure.** By the time this runs the command has
    /// succeeded — for `apply`, the renames are committed and journalled — and
    /// exit 1 would tell a script that nothing was touched.
    fn consume_list(&self, simulated: bool) {
        if !self.delete_list || simulated {
            return;
        }
        let Some(list) = &self.list else {
            return;
        };
        if let Err(e) = std::fs::remove_file(list) {
            eprintln!(
                "warning: could not delete the list file {}: {e}",
                shown_path(list)
            );
        }
    }

    /// The three ways a caller can name files, in order of precedence: given
    /// both a path (`/p`) and a file list (`/l`), the list wins. clap already
    /// refuses the combination, so this only has to pick.
    fn source(&self, notes: &mut Vec<String>) -> Result<Option<Source>, AnyError> {
        if let Some(list) = &self.list {
            return free_source(read_list(list)?, notes).map(Some);
        }
        if !self.file.is_empty() {
            return free_source(self.file.clone(), notes).map(Some);
        }
        self.dir
            .as_deref()
            .map(|dir| {
                Ok(Source::Folder {
                    dir: absolute(dir)?,
                    options: self.listing(),
                })
            })
            .transpose()
    }

    fn listing(&self) -> ListOptions {
        ListOptions {
            files: !self.no_files,
            folders: self.folders,
            subfolders: self.subfolders,
            ..Default::default()
        }
        .with_pattern(&self.pattern)
    }

    fn store(&self) -> PresetStore {
        PresetStore::new(self.preset_dir.clone().unwrap_or_else(default_preset_dir))
    }

    /// A string naming an existing file is a path; anything else is a name
    /// looked up in the preset folder, the way `/r "name"` works.
    fn find_preset(&self, wanted: &str) -> Result<(Preset, ren_core::ImportNotes), AnyError> {
        let store = self.store();
        let as_path = PathBuf::from(wanted);
        if as_path.is_file() {
            Ok(store.load(&as_path)?)
        } else {
            Ok((store.load_named(wanted)?, ren_core::ImportNotes::default()))
        }
    }

    /// `<Ask>` is collected once, before anything is evaluated (D28).
    ///
    /// **Every answer has to land somewhere, and every question has to be
    /// answered.** A mistyped slot — `--answer 10=Holiday` for slot 1 — used
    /// to be stored and ignored, the real slot then went unanswered, and an
    /// unanswered `<Ask>` leaves the name alone (P33): a scheduled run that
    /// renamed nothing and exited 0. So a slot the pipeline does not ask for
    /// is refused, and `apply` refuses to start when stdin closes before a
    /// question is answered. `preview` never asks, and previews an unanswered
    /// slot as unchanged.
    fn answers(&self, asks: &[AskSpec], ask: bool) -> Result<ren_core::Answers, AnyError> {
        let asked: BTreeSet<u8> = asks.iter().map(|spec| spec.slot).collect();
        let mut answers = ren_core::Answers::default();
        for pair in &self.answer {
            let (slot, text) = pair
                .split_once('=')
                .ok_or_else(|| format!("--answer wants SLOT=TEXT, got {pair:?}"))?;
            let slot: u8 = slot
                .trim()
                .parse()
                .ok()
                .filter(|slot| *slot <= 9)
                .ok_or_else(|| format!("--answer slot must be 0-9, got {slot:?}"))?;
            if !asked.contains(&slot) {
                let wanted = if asked.is_empty() {
                    "this pipeline has no <Ask> to answer".to_owned()
                } else {
                    let slots: Vec<String> = asked
                        .iter()
                        .map(|slot| format!("{slot} for {}", ask_tag(*slot)))
                        .collect();
                    format!("this pipeline asks for {}", slots.join(", "))
                };
                return Err(format!("--answer {slot}: {wanted}").into());
            }
            answers.asks.insert(slot, text.to_owned());
        }

        let unanswered: Vec<AskSpec> = asks
            .iter()
            .filter(|spec| !answers.asks.contains_key(&spec.slot))
            .cloned()
            .collect();
        if ask && !unanswered.is_empty() {
            let from_stdin =
                ren_core::run::collect(&unanswered, false, &ren_core::StdinInteraction);
            answers.asks.extend(from_stdin.asks);
            let missing: Vec<String> = unanswered
                .iter()
                .filter(|spec| !answers.asks.contains_key(&spec.slot))
                .map(|spec| ask_tag(spec.slot))
                .collect();
            if !missing.is_empty() {
                return Err(format!(
                    "no answer for {}: stdin closed before one was given; pass --answer SLOT=TEXT",
                    missing.join(", ")
                )
                .into());
            }
        }
        Ok(answers)
    }

    /// The listing, the system-folder guard and the answers — everything a
    /// plan needs — in the order that asks nothing of a run that will be
    /// refused. `None` when the guard refused it; the reason is already said.
    fn prepare(
        &self,
        platform: &dyn Platform,
        ask: bool,
    ) -> Result<Option<(Vec<FileEntry>, Pipeline)>, AnyError> {
        let mut resolved = self.resolve()?;
        resolved.report_notes();
        // D127, the same check the GUI's Rename button makes: before the
        // plan, so a preview says it too, and ahead of every other reason,
        // because it is not a fact about the plan. The browsed folder is
        // checked before it is walked — `/usr --subfolders` would otherwise
        // list the whole tree only to refuse it — and every entry's folder
        // after.
        let none: [&Path; 0] = [];
        if self.refuse_guarded(platform, resolved.browsed(), none) {
            return Ok(None);
        }
        let entries = resolved.entries()?;
        if self.refuse_guarded(
            platform,
            None,
            entries.iter().map(|entry| entry.path.as_path()),
        ) {
            return Ok(None);
        }
        resolved.pipeline.answers = self.answers(&resolved.pipeline.asks(), ask)?;
        Ok(Some((entries, resolved.pipeline)))
    }

    /// True, having said why, when the run would touch a folder the OS needs
    /// left alone and `--allow-system-folders` was not given.
    fn refuse_guarded<'a>(
        &self,
        platform: &dyn Platform,
        dir: Option<&Path>,
        paths: impl IntoIterator<Item = &'a Path>,
    ) -> bool {
        if self.allow_system_folders {
            return false;
        }
        let Some(folder) = ren_platform::guarded::first_guarded(platform, dir, paths) else {
            return false;
        };
        eprintln!(
            "error: {} is a folder the operating system needs left alone, so nothing in it \
             is renamed",
            shown_path(&folder)
        );
        eprintln!("pass --allow-system-folders to go ahead anyway");
        true
    }
}

/// `text` as a terminal can show it without being told to do something.
///
/// A file name on Linux may hold any byte but `/` and NUL, and a name built
/// from a tag carries whatever the tag held — so a carriage return, an escape
/// sequence or a right-to-left override in a name could overwrite or
/// disguise the preview line a user reads before running `apply`. Each is
/// shown escaped (`\r`, `\u{1b}`, `\u{202e}`), which is also a name the user
/// can recognise as odd.
fn shown(text: &str) -> Cow<'_, str> {
    let hostile =
        |c: char| c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
    if !text.chars().any(hostile) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if hostile(c) {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

fn shown_path(path: &Path) -> String {
    shown(&path.to_string_lossy()).into_owned()
}

fn main() -> ExitCode {
    // Before anything reads a path. A portable folder holds both binaries, and
    // the CLI writing its journal to `%APPDATA%` while the app wrote one beside
    // the exe would make `undo` find nothing (D131).
    if let Some(root) = ren_platform::portable_root() {
        let _ = ren_platform::use_portable_root(root);
    }
    match run() {
        Ok(exit) => exit.code(),
        Err(e) => {
            eprintln!("error: {}", shown(&e.to_string()));
            Exit::Usage.code()
        }
    }
}

fn run() -> Result<Exit, AnyError> {
    // `args_os`, never `args`: the latter panics on an argument that is not
    // valid Unicode — exit 101, outside every code this CLI promises — and
    // such a name is exactly what a renamer is pointed at.
    let args: Vec<OsString> = std::env::args_os().collect();
    // Translated before clap sees it, and only when the line really is the old
    // shape — `looks_legacy` needs an actual `/x` switch before any of our
    // subcommands, or a bare path, so a modern command is never rewritten.
    let legacy = compat::looks_legacy(&args).then(|| {
        // On Windows, split again the way a batch file means it: its
        // `"%~dp0"` ends in `\"`, which the C runtime reads as an escaped
        // quote that swallows every switch after it.
        let verbatim = ren_platform::raw_command_line()
            .map(|line| ren_platform::split_verbatim(&line))
            .filter(|verbatim| compat::looks_legacy(verbatim));
        compat::translate(verbatim.as_deref().unwrap_or(&args))
    });
    // `try_parse_from` rather than `parse`, so a bad command line exits
    // `Usage`. clap's own default for a parse error is **2**, which is
    // `Exit::Blocked` — "refused, nothing touched". A caller cannot be left
    // unable to tell a typo from a refused rename.
    let parsed = match &legacy {
        Some(translated) => Cli::try_parse_from(&translated.argv),
        None => Cli::try_parse_from(&args),
    };
    let cli = match parsed {
        Ok(cli) => cli,
        Err(error) => {
            // `use_stderr` is false for `--help` and `--version`, which are
            // requests rather than failures and exit zero.
            let failed = error.use_stderr();
            // Say what the line became before clap prints usage for a grammar
            // the caller did not type. Without this the message is baffling:
            // it names flags that appear nowhere in what was typed.
            if failed && let Some(translated) = &legacy {
                eprintln!(
                    "ren-cli: legacy switches translated to `{}`, which did not parse:",
                    shown_argv(&translated.argv[1..])
                );
            }
            let _ = error.print();
            return Ok(if failed { Exit::Usage } else { Exit::Success });
        }
    };

    if let Some(translated) = &legacy {
        // Always said, not only under --verbose: the translation decides
        // whether the line renames or only previews, and a caller who does not
        // know which would have to work it out from what happened afterwards.
        eprintln!(
            "ren-cli: legacy switches translated to `{}`",
            shown_argv(&translated.argv[1..])
        );
        if cli.verbose {
            for note in &translated.notes {
                eprintln!("  {}", shown(note));
            }
        } else if !translated.notes.is_empty() {
            eprintln!("  (--verbose explains each switch)");
        }
    }

    let platform = ren_platform::host();
    let journal_dir = cli.journal_dir.clone().unwrap_or_else(default_journal_dir);

    match &cli.command {
        Command::Preview { job } => {
            let Some((entries, pipeline)) = job.prepare(platform.as_ref(), false)? else {
                return Ok(Exit::Blocked);
            };
            let plan = plan(&entries, &pipeline, platform.as_ref());
            print_plan(&plan);
            // `done()` runs while planning, so a preview is where a script's
            // own message first exists. Silence here is D77's failure mode.
            for note in &plan.notes {
                println!("{}", shown(note));
            }
            for op in &plan.ops {
                if let ren_core::PlannedOp::WriteFile { path, .. } = op {
                    println!("would write {}", shown_path(path));
                }
            }
            // P4: a plan that cannot run is not a successful preview. A script
            // that pipes this into a deploy has to be able to tell "nothing
            // needed renaming" from "the rename would be refused".
            if !plan.is_executable() {
                return Ok(Exit::Blocked);
            }
            // `/k` on a line with no `/r` translates to `preview --delete-list`,
            // and that is the shape a shell integration emits (`/l "%l" /k`).
            // Deleting only on `apply` would leak its temp file on every
            // invocation, forever — so a successful preview consumes the list
            // too: it has been read.
            job.consume_list(false);
            Ok(Exit::Success)
        }

        Command::Apply {
            job,
            simulate,
            allow_irreversible,
        } => {
            let Some((entries, pipeline)) = job.prepare(platform.as_ref(), true)? else {
                return Ok(Exit::Blocked);
            };
            let plan = plan(&entries, &pipeline, platform.as_ref());
            print_plan(&plan);

            let options = ApplyOptions {
                simulate: *simulate,
                journal_dir,
                allow_irreversible: *allow_irreversible,
                ..Default::default()
            };
            match apply(&plan, platform.as_ref(), &options) {
                Ok(report) => {
                    let (renamed, modified) = if report.simulated {
                        ("would rename", "would modify")
                    } else {
                        ("renamed", "modified")
                    };
                    println!(
                        "\n{renamed} {}",
                        ren_core::plural(report.renamed.len(), "item")
                    );
                    if !report.acted.is_empty() {
                        println!("{modified} {}", ren_core::plural(report.modified(), "item"));
                    }
                    // A script asking for a file is a change the user has to
                    // be told about, and "replaced" is the half that matters:
                    // creating one can be undone, replacing one cannot (D99).
                    for (path, replaced) in &report.wrote {
                        if *replaced {
                            println!("replaced {} (cannot be undone)", shown_path(path));
                        } else {
                            println!("wrote {}", shown_path(path));
                        }
                    }
                    // Whatever a script's done() returned, which belongs in
                    // the log (P59).
                    for note in &report.notes {
                        println!("{}", shown(note));
                    }
                    if let Some(txn) = &report.txn {
                        println!("transaction {txn}");
                    }
                    for (path, error) in &report.failed {
                        eprintln!(
                            "failed: {} — {}",
                            shown_path(path),
                            shown(&error.to_string())
                        );
                    }
                    if !report.is_success() {
                        return Ok(Exit::Failed);
                    }
                    job.consume_list(report.simulated);
                    Ok(Exit::Success)
                }
                // Refused before anything was touched, which is a different
                // thing for a caller to handle than a partial run: P4's
                // conflicts, P2's irreversible changes, and a path that could
                // not be undone from another folder.
                Err(e @ (ExecError::Blocked { .. } | ExecError::RelativePath { .. })) => {
                    eprintln!("error: {}", shown(&e.to_string()));
                    Ok(Exit::Blocked)
                }
                Err(e @ ExecError::Irreversible { .. }) => {
                    eprintln!("error: {e}");
                    eprintln!("pass --allow-irreversible to go ahead anyway");
                    Ok(Exit::Blocked)
                }
                // D103: the run started and stopped part-way, so this is 3 —
                // "the next step is `recover`, not a retry" — never the 1 a
                // wrong command line gets, which would tell a script that
                // nothing was touched.
                Err(e @ ExecError::Interrupted { .. }) => {
                    eprintln!("error: {}", shown(&e.to_string()));
                    Ok(Exit::Failed)
                }
                Err(e) => Err(e.into()),
            }
        }

        Command::Undo { txn } => {
            let undone = match txn {
                Some(path) => ren_core::exec::undo_transaction(path, platform.as_ref()),
                None => undo_last(platform.as_ref(), &journal_dir),
            };
            let report = match undone {
                Ok(report) => report,
                // Nothing was touched, and it will work once the other run
                // has finished: refused, not a bad command line.
                Err(e @ ExecError::JournalInUse { .. }) => {
                    eprintln!("error: {}", shown(&e.to_string()));
                    return Ok(Exit::Blocked);
                }
                Err(e) => return Err(e.into()),
            };
            println!(
                "restored {} from transaction {}",
                ren_core::plural(report.restored.len(), "item"),
                report.txn
            );
            if !report.reverted.is_empty() {
                println!(
                    "put back the metadata of {}",
                    ren_core::plural(report.reverted.len(), "item")
                );
            }
            print_undo_details(&report, "");
            // D54 and D77: an undo that could not put a tag write back has
            // still done its whole job, so `irreversible` does not reach this.
            // A genuine skip does, and so does an undo the journal could not
            // record.
            Ok(if report.is_complete() {
                Exit::Success
            } else {
                Exit::Failed
            })
        }

        Command::Presets { action } => run_presets(action),

        Command::Recover { rollback } => {
            let (pending, problems) = ren_core::exec::unfinished(&journal_dir);
            // One journal that cannot be judged no longer hides the others:
            // it is named here, and the run is not reported as complete. A
            // journal another window is still writing is not a problem at
            // all — it is a run in progress, and is left alone.
            let mut complete = true;
            for (path, error) in &problems {
                if matches!(error, ExecError::JournalInUse { .. }) {
                    eprintln!(
                        "{}: still being written by a run in another window; left alone",
                        shown_path(path)
                    );
                } else {
                    eprintln!(
                        "could not check {}: {}",
                        shown_path(path),
                        shown(&error.to_string())
                    );
                    complete = false;
                }
            }
            if pending.is_empty() {
                if problems.is_empty() {
                    println!("no unfinished transactions in {}", shown_path(&journal_dir));
                }
                return Ok(if complete {
                    Exit::Success
                } else {
                    Exit::Failed
                });
            }

            for item in &pending {
                println!(
                    "transaction {}: {} completed, {} in flight",
                    item.txn,
                    ren_core::plural(item.completed, "change"),
                    item.in_flight.len()
                );
                // Named, not counted. The one file whose *contents* may be
                // half-written is the one the user has to go and look at, and
                // until now it was the only thing the report did not say.
                for flight in &item.in_flight {
                    if flight.rewrote_contents {
                        println!(
                            "  {} — {} was interrupted, so the file may be half-written",
                            shown_path(&flight.path),
                            flight.op
                        );
                    } else {
                        println!(
                            "  {} — {} was interrupted, so it may be under either name",
                            shown_path(&flight.path),
                            flight.op
                        );
                    }
                }
                if !rollback {
                    continue;
                }
                // One transaction that cannot be rolled back is reported and
                // the rest still are: stopping at the first error left every
                // later one unrecovered, and exited 1 — "nothing ran" — after
                // the earlier ones had been put back.
                let report = match ren_core::exec::rollback(item, platform.as_ref()) {
                    Ok(report) => report,
                    Err(e) => {
                        eprintln!("  could not roll back: {}", shown(&e.to_string()));
                        complete = false;
                        continue;
                    }
                };
                println!(
                    "  rolled back {}",
                    ren_core::plural(report.restored.len(), "change")
                );
                print_undo_details(&report, "  ");
                if !report.is_complete() {
                    complete = false;
                }
            }
            if !rollback {
                println!("\nre-run with --rollback to revert them");
            }
            Ok(if complete {
                Exit::Success
            } else {
                Exit::Failed
            })
        }
    }
}

/// Everything an undo or a rollback did besides putting names back, each
/// named — D77's rule, that a channel nothing reads is a change the user is
/// never told about.
fn print_undo_details(report: &UndoReport, indent: &str) {
    // D54's bucket, said out loud. Not an error and not a skip — the renames
    // came back, these never could — but silence here means a user believes
    // a tag write was reverted when it was not.
    for (path, what) in &report.irreversible {
        println!("{indent}still applied: {} — {what}", shown_path(path));
    }
    for path in &report.removed_files {
        println!(
            "{indent}removed {}, which the run had written",
            shown_path(path)
        );
    }
    for path in &report.removed_dirs {
        println!("{indent}removed folder {}", shown_path(path));
    }
    // Why a folder the run created is still there: something else is in it
    // now, and deleting a folder the user has since filled is far worse than
    // leaving an empty-looking one.
    for path in &report.kept_dirs {
        println!(
            "{indent}kept folder {}: it is not empty, so it was left in place",
            shown_path(path)
        );
    }
    for (path, reason) in &report.skipped {
        eprintln!("{indent}skipped: {} — {}", shown_path(path), shown(reason));
    }
    // The files did move; the journal does not say so. Undo would offer the
    // same transaction again.
    if let Some(reason) = &report.not_recorded {
        eprintln!(
            "{indent}error: the undo happened but could not be recorded in the journal: {}",
            shown(reason)
        );
    }
}

/// A translated command line, as a person would read it.
fn shown_argv(argv: &[OsString]) -> String {
    argv.iter()
        .map(|arg| shown(&arg.to_string_lossy()).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_plan(plan: &ren_core::Plan) {
    for item in &plan.items {
        let name = item
            .source
            .file_name()
            .map(|n| shown(&n.to_string_lossy()).into_owned())
            .unwrap_or_default();
        let new_name = shown(&item.new_name);
        // What the run does to this row, the same way the GUI's New name
        // column reads it (D38): a Set Date row is not "unchanged".
        let actions: Vec<&str> = item.actions.iter().map(|a| a.describe.as_str()).collect();
        let acted = if actions.is_empty() {
            String::new()
        } else {
            format!("  [{}]", shown(&actions.join(", ")))
        };
        match &item.state {
            RowState::Unchanged if actions.is_empty() => println!("  =  {name}"),
            RowState::Unchanged => println!("  ~  {name}{acted}"),
            RowState::Changed => println!("  →  {name}  ->  {new_name}{acted}"),
            RowState::Conflict(kind) => {
                println!(
                    "  !  {name}  ->  {new_name}  [{}]",
                    shown(&kind.to_string())
                );
            }
            RowState::Error(message) => println!("  x  {name}  [{}]", shown(message)),
        }
    }
    let mut parts = vec![format!("{} to rename", plan.changed())];
    if plan.acted() > 0 {
        parts.push(format!("{} to modify", plan.acted()));
    }
    parts.push(format!("{} unchanged", plan.unchanged()));
    parts.push(ren_core::plural(plan.conflicts(), "conflict"));
    parts.push(ren_core::plural(plan.errors(), "error"));
    println!(
        "\n{}: {}",
        ren_core::plural(plan.items.len(), "item"),
        parts.join(", ")
    );
    // What blocks the run besides its rows — a script's write outside the
    // listed folders — or the exit code 2 would come with no reason.
    for reason in &plan.blockers {
        println!("blocked: {}", shown(reason));
    }
}

fn store_at(dir: &Option<PathBuf>) -> PresetStore {
    PresetStore::new(dir.clone().unwrap_or_else(default_preset_dir))
}

fn run_presets(action: &PresetAction) -> Result<Exit, AnyError> {
    match action {
        PresetAction::List { preset_dir } => {
            let store = store_at(preset_dir);
            let (entries, problems) = store.list();
            if entries.is_empty() && problems.is_empty() {
                println!("no presets in {}", shown_path(store.dir()));
            }
            for entry in &entries {
                let description = if entry.description.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", shown(&entry.description))
                };
                println!(
                    "{}  ({}){description}",
                    shown(&entry.name),
                    ren_core::plural(entry.steps, "operation")
                );
            }
            for problem in &problems {
                eprintln!("unreadable: {}", shown(&problem.error.to_string()));
            }
            // 1, not 3: nothing ran and there is no journal, so D103's "the
            // next step is `recover`" does not apply. 1 is the code for a
            // filesystem that refused something before any work started.
            Ok(if problems.is_empty() {
                Exit::Success
            } else {
                Exit::Usage
            })
        }

        PresetAction::Show { name, preset_dir } => {
            let preset = store_at(preset_dir).load_named(name)?;
            print!("{}", preset.to_toml()?);
            Ok(Exit::Success)
        }

        PresetAction::Import { file, preset_dir } => {
            let store = store_at(preset_dir);
            let (path, notes) = store.import(file)?;
            if let Some(source) = notes.dropped_source {
                eprintln!(
                    "note: dropped the [source] it named ({}) — a preset borrows yours",
                    shown_path(&source.dir())
                );
            }
            println!("imported to {}", shown_path(&path));
            Ok(Exit::Success)
        }

        PresetAction::Export {
            name,
            to,
            preset_dir,
            force,
        } => {
            // TO is any path the user typed, not our own data file, and there
            // is no save dialog here to ask before replacing it — `notes.txt`
            // typed for `notes.toml` would become TOML with no undo.
            if !force && to.symlink_metadata().is_ok() {
                return Err(format!(
                    "{} already exists; pass --force to replace it",
                    shown_path(to)
                )
                .into());
            }
            let store = store_at(preset_dir);
            let preset = store.load_named(name)?;
            store.export(&preset, to)?;
            println!("wrote {}", shown_path(to));
            Ok(Exit::Success)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hostile_name_is_shown_escaped_and_an_ordinary_one_untouched() {
        assert!(matches!(shown("Ünïcödé 🎵.mp3"), Cow::Borrowed(_)));
        assert_eq!(shown("a\rb"), "a\\rb");
        assert_eq!(shown("x\u{1b}[2Ky"), "x\\u{1b}[2Ky");
        assert_eq!(shown("evil\u{202e}gpj.exe"), "evil\\u{202e}gpj.exe");
        assert_eq!(shown("tab\there"), "tab\\there");
    }

    #[test]
    fn an_ask_slot_is_named_the_way_a_template_writes_it() {
        assert_eq!(ask_tag(0), "<Ask>");
        assert_eq!(ask_tag(3), "<Ask-3>");
    }
}
