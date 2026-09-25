//! Headless runner for the RenameIt engine.
//!
//! This is the automation story *and* the test harness — it ships in every
//! release.
//!
//! The modern `--` flags are the real interface (D18, P38). Legacy
//! `/p /l /r /f /d /s /k /x` switches are accepted too, rewritten into those
//! flags by [`compat`] before clap ever sees them, so an existing `.bat` file
//! keeps working without this CLI carrying two grammars.

mod compat;
mod exit;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use exit::Exit;

use clap::{Args, Parser, Subcommand};
use ren_core::exec::{ExecError, default_journal_dir};
use ren_core::model::Scope;
use ren_core::preset::{Preset, PresetStore, default_preset_dir};
use ren_core::{
    AppendSuffix, ApplyOptions, Job, ListOptions, Pipeline, RowState, Step, StepConfig, apply,
    plan, undo_last,
};

#[derive(Debug, Parser)]
#[command(name = "ren-cli", version, about, long_about = None)]
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
        /// Permit changes that cannot be undone (P2).
        ///
        /// Writing or removing tags rewrites the file and keeps nothing, so
        /// there is no journal entry that could put it back. Without this the
        /// run is refused before anything is touched. The GUI asks instead; a
        /// script has to say so up front.
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
    },
}

#[derive(Debug, Args)]
struct JobArgs {
    /// Directory to list. Omit when using --job.
    dir: Option<PathBuf>,

    /// A TOML job file describing the source and the pipeline. This is the
    /// same schema presets use.
    #[arg(long, value_name = "FILE", conflicts_with = "preset")]
    job: Option<PathBuf>,

    /// A saved pipeline: the name of a preset, or the path to a `.toml` file.
    ///
    /// A job file brings its own source; a preset borrows yours — so this one
    /// needs a directory, and the listing flags below apply.
    #[arg(long, value_name = "NAME|FILE", conflicts_with = "job")]
    preset: Option<String>,

    /// Where presets live. Defaults to the per-user data directory.
    #[arg(long, value_name = "DIR")]
    preset_dir: Option<PathBuf>,

    /// Answer an <Ask> slot without being prompted: `--answer 0=Holiday`.
    ///
    /// Slot 0 is `<Ask>`, 1-9 are `<Ask-1>`…`<Ask-9>`. Prompting on stdin is
    /// fine for a person and hostile to a script.
    #[arg(long, value_name = "SLOT=TEXT")]
    answer: Vec<String>,

    /// Quick single-operation path: append this text. Ignored with --job.
    #[arg(long, default_value = "_renamed")]
    suffix: String,

    /// Which part of the file name --suffix may touch.
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
    /// `#` comments are skipped; everything else must exist.
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
    /// random values. Pass one to make a run reproducible — the other half of
    /// P16.
    #[arg(long, value_name = "N")]
    seed: Option<u64>,

    /// Delete the --list file once the command has succeeded.
    ///
    /// The legacy `/k`: delete the `/l` file once it has been read.
    /// Not a rename, so it is not journalled and cannot be undone — which is
    /// why it waits for success rather than firing the moment the file is read
    /// (D101). A blocked or failed run leaves the list where it was.
    #[arg(long, requires = "list")]
    delete_list: bool,
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

/// Everything a run needs, from either source of truth.
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

    fn entries(&self) -> Result<Vec<ren_core::FileEntry>, Box<dyn std::error::Error>> {
        match &self.source {
            Source::Folder { dir, options } => {
                let (entries, problems) = ren_core::listing::list_reporting(dir, options.clone())?;
                // Named, not counted: a run that quietly covered less than the
                // caller asked for is what P63 exists to make visible.
                for problem in &problems {
                    eprintln!("skipped: {problem}");
                }
                Ok(entries)
            }
            Source::Free(paths) => paths
                .iter()
                .map(|path| {
                    ren_core::FileEntry::from_path(path).map_err(|e| {
                        // Named, because a list of a thousand paths with one
                        // bad line in it is otherwise a guessing game.
                        format!("{}: {e}", path.display()).into()
                    })
                })
                .collect(),
        }
    }
}

/// Read the `/l` file: full paths, one per line.
///
/// Blank lines are skipped, and so are `#` comments — an extension, since a
/// strict reading would treat both as filenames and fail. A list file is
/// usually generated, but it is also the thing a person hand-edits when
/// something went wrong, and a format with no way to leave a note is a worse
/// format for no gain.
///
/// A UTF-8 byte-order mark is stripped, for the reason D50 strips it from a
/// CSV: PowerShell's `Out-File` and `Set-Content` write one by default, and
/// `/l` is exactly the switch a script feeds. Left in, the first path is
/// `\u{feff}C:\…`, which fails as missing with an error that prints
/// identically to the real path.
fn read_list(path: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read the list file {}: {e}", path.display()))?;
    let paths: Vec<PathBuf> = text
        .strip_prefix('\u{feff}')
        .unwrap_or(&text)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        return Err(format!("the list file {} names no files", path.display()).into());
    }
    Ok(paths)
}

impl JobArgs {
    /// `ask` is false for `preview`, which must stay non-interactive: an
    /// unanswered `<Ask>` simply previews as unchanged.
    fn resolve(&self, ask: bool) -> Result<Resolved, Box<dyn std::error::Error>> {
        let mut resolved = self.resolve_source(ask)?;
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

    fn resolve_source(&self, ask: bool) -> Result<Resolved, Box<dyn std::error::Error>> {
        if let Some(path) = &self.job {
            if self.dir.is_some() {
                return Err("pass either a directory or --job, not both".into());
            }
            let job = Job::from_file(path)?;
            let answers = self.answers(job.pipeline().asks(), ask)?;
            return Ok(Resolved {
                source: Source::Folder {
                    dir: job.source.dir(),
                    options: job.source.list_options(),
                },
                pipeline: job.pipeline_with(answers),
                notes: Vec::new(),
            });
        }

        if let Some(wanted) = &self.preset {
            // A preset borrows the caller's source, so it needs one — a
            // folder, a list file, or an explicit set of paths.
            let source = self
                .source()?
                .ok_or("--preset renames the files you name: pass a folder, --list or --file")?;
            let (preset, import) = self.find_preset(wanted)?;
            let mut notes = Vec::new();
            if let Some(source) = import.dropped_source {
                notes.push(format!(
                    "note: ignoring the [source] in that file ({}) — a preset runs over the \
                     folder you name",
                    source.dir().display()
                ));
            }
            let answers = self.answers(preset.pipeline().asks(), ask)?;
            return Ok(Resolved {
                source,
                pipeline: preset.pipeline_with(answers),
                notes,
            });
        }

        let source = self.source()?.ok_or(
            "give a directory to rename, or --list <FILE>, or --job <FILE>, or --preset <NAME>",
        )?;
        Ok(Resolved {
            source,
            pipeline: Pipeline::new().with(
                Step::Name(Box::new(AppendSuffix::new(self.suffix.clone()))),
                StepConfig::scoped(self.scope.into()),
            ),
            notes: Vec::new(),
        })
    }

    /// Delete the `--list` file, if `--delete-list` asked for it.
    ///
    /// The legacy `/k`. Not a rename: it is not journalled and there is no
    /// undo for it, which is exactly why it waits until the run it belonged to
    /// has actually happened rather than firing the moment the file is read.
    /// A failed batch leaves the list where it was, so it can be run again.
    fn consume_list(&self, simulated: bool) -> Result<(), Box<dyn std::error::Error>> {
        if !self.delete_list || simulated {
            return Ok(());
        }
        let Some(list) = &self.list else {
            return Ok(());
        };
        std::fs::remove_file(list)
            .map_err(|e| format!("could not delete the list file {}: {e}", list.display()))?;
        Ok(())
    }

    /// The three ways a caller can name files, in order of precedence: given
    /// both a path (`/p`) and a file list (`/l`), the list wins. clap already
    /// refuses the combination, so this only has to pick.
    fn source(&self) -> Result<Option<Source>, Box<dyn std::error::Error>> {
        if let Some(list) = &self.list {
            return Ok(Some(Source::Free(read_list(list)?)));
        }
        if !self.file.is_empty() {
            return Ok(Some(Source::Free(self.file.clone())));
        }
        Ok(self.dir.clone().map(|dir| Source::Folder {
            dir,
            options: self.listing(),
        }))
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
    fn find_preset(
        &self,
        wanted: &str,
    ) -> Result<(Preset, ren_core::ImportNotes), Box<dyn std::error::Error>> {
        let store = self.store();
        let as_path = PathBuf::from(wanted);
        if as_path.is_file() {
            Ok(store.load(&as_path)?)
        } else {
            Ok((store.load_named(wanted)?, ren_core::ImportNotes::default()))
        }
    }

    /// `<Ask>` is collected once, before anything is evaluated (D28).
    fn answers(
        &self,
        asks: Vec<ren_core::AskSpec>,
        ask: bool,
    ) -> Result<ren_core::Answers, Box<dyn std::error::Error>> {
        let mut answers = ren_core::Answers::default();
        for pair in &self.answer {
            let (slot, text) = pair
                .split_once('=')
                .ok_or_else(|| format!("--answer wants SLOT=TEXT, got {pair:?}"))?;
            let slot: u8 = slot
                .trim()
                .parse()
                .map_err(|_| format!("--answer slot must be 0-9, got {slot:?}"))?;
            answers.asks.insert(slot, text.to_owned());
        }

        let unanswered: Vec<_> = asks
            .into_iter()
            .filter(|spec| !answers.asks.contains_key(&spec.slot))
            .collect();
        if ask && !unanswered.is_empty() {
            let from_stdin =
                ren_core::run::collect(&unanswered, false, &ren_core::StdinInteraction);
            answers.asks.extend(from_stdin.asks);
        }
        Ok(answers)
    }
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
            eprintln!("error: {e}");
            Exit::Usage.code()
        }
    }
}

fn run() -> Result<Exit, Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    // Translated before clap sees it, and only when the line really is the old
    // shape — `looks_legacy` needs an actual `/x` switch, so a modern command
    // is never rewritten.
    let legacy = compat::looks_legacy(&args).then(|| compat::translate(&args));
    // `try_parse` rather than `parse`, so a bad command line exits `Usage`.
    // clap's own default for a parse error is **2**, which is `Exit::Blocked`
    // — "the plan has conflicts and nothing ran". A caller cannot be left
    // unable to tell a typo from a refused rename.
    let parsed = match &legacy {
        Some(translated) => Cli::try_parse_from(&translated.argv),
        None => Cli::try_parse(),
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
                    translated.argv[1..].join(" ")
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
            translated.argv[1..].join(" ")
        );
        if cli.verbose {
            for note in &translated.notes {
                eprintln!("  {note}");
            }
        } else if !translated.notes.is_empty() {
            eprintln!("  (--verbose explains each switch)");
        }
    }

    let platform = ren_platform::host();
    let journal_dir = cli.journal_dir.clone().unwrap_or_else(default_journal_dir);

    match &cli.command {
        Command::Preview { job } => {
            let resolved = job.resolve(false)?;
            resolved.report_notes();
            let entries = resolved.entries()?;
            let plan = plan(&entries, &resolved.pipeline, platform.as_ref());
            print_plan(&plan);
            // `done()` runs while planning, so a preview is where a script's
            // own message first exists. Silence here is D77's failure mode.
            for note in &plan.notes {
                println!("{note}");
            }
            for op in &plan.ops {
                if let ren_core::PlannedOp::WriteFile { path, .. } = op {
                    println!("would write {}", path.display());
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
            // Deleting only on `apply` would leak its temp file
            // on every invocation, forever — so a successful preview consumes
            // the list too, which is also what "after it has been read" says.
            job.consume_list(false)?;
            Ok(Exit::Success)
        }

        Command::Apply {
            job,
            simulate,
            allow_irreversible,
        } => {
            let resolved = job.resolve(true)?;
            resolved.report_notes();
            let entries = resolved.entries()?;
            let plan = plan(&entries, &resolved.pipeline, platform.as_ref());
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
                            println!("replaced {} (cannot be undone)", path.display());
                        } else {
                            println!("wrote {}", path.display());
                        }
                    }
                    // Whatever a script's done() returned, which belongs in
                    // the log (P59).
                    for note in &report.notes {
                        println!("{note}");
                    }
                    if let Some(txn) = &report.txn {
                        println!("transaction {txn}");
                    }
                    for (path, error) in &report.failed {
                        eprintln!("failed: {} — {error}", path.display());
                    }
                    if !report.is_success() {
                        return Ok(Exit::Failed);
                    }
                    job.consume_list(report.simulated)?;
                    Ok(Exit::Success)
                }
                // P4 again: refused before anything was touched, which is a
                // different thing for a caller to handle than a partial run.
                Err(e @ ExecError::Blocked { .. }) => {
                    eprintln!("error: {e}");
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
                    eprintln!("error: {e}");
                    Ok(Exit::Failed)
                }
                Err(e) => Err(e.into()),
            }
        }

        Command::Undo { txn } => {
            let report = match txn {
                Some(path) => ren_core::exec::undo_transaction(path, platform.as_ref())?,
                None => undo_last(platform.as_ref(), &journal_dir)?,
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
            // D54's bucket, said out loud. Not an error and not a skip — the
            // renames came back, these never could — but silence here means a
            // user believes a tag write was reverted when it was not.
            for (path, what) in &report.irreversible {
                println!("still applied: {} — {what}", path.display());
            }
            for (path, reason) in &report.skipped {
                eprintln!("skipped: {} — {reason}", path.display());
            }
            // D54 and D77: an undo that could not put a tag write back has
            // still done its whole job, so `irreversible` above does not reach
            // this. Only a genuine skip does.
            Ok(if report.is_complete() {
                Exit::Success
            } else {
                Exit::Failed
            })
        }

        Command::Presets { action } => run_presets(action),

        Command::Recover { rollback } => {
            let (pending, unreadable) = ren_core::exec::unfinished(&journal_dir);
            // One journal that cannot be judged no longer hides the others:
            // it is named here, and the run is not reported as complete.
            for (path, error) in &unreadable {
                eprintln!("could not check {}: {error}", path.display());
            }
            if pending.is_empty() && unreadable.is_empty() {
                println!("no unfinished transactions in {}", journal_dir.display());
                return Ok(Exit::Success);
            }

            let mut complete = unreadable.is_empty();
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
                            flight.path.display(),
                            flight.op
                        );
                    } else {
                        println!(
                            "  {} — {} was interrupted, so it may be under either name",
                            flight.path.display(),
                            flight.op
                        );
                    }
                }
                if !rollback {
                    continue;
                }
                let report = ren_core::exec::rollback(item, platform.as_ref())?;
                println!(
                    "  rolled back {}",
                    ren_core::plural(report.restored.len(), "change")
                );
                // Same rule as `undo` (D77): a change that could never be taken
                // back is reported, not silently dropped.
                for (path, what) in &report.irreversible {
                    println!("  still applied: {} — {what}", path.display());
                }
                for (path, reason) in &report.skipped {
                    eprintln!("  skipped: {} — {reason}", path.display());
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

fn print_plan(plan: &ren_core::Plan) {
    for item in &plan.items {
        let name = item
            .source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // What the run does to this row, the same way the GUI's New name
        // column reads it (D38): a Set Date row is not "unchanged".
        let actions: Vec<&str> = item.actions.iter().map(|a| a.describe.as_str()).collect();
        let acted = if actions.is_empty() {
            String::new()
        } else {
            format!("  [{}]", actions.join(", "))
        };
        match &item.state {
            RowState::Unchanged if actions.is_empty() => println!("  =  {name}"),
            RowState::Unchanged => println!("  ~  {name}{acted}"),
            RowState::Changed => println!("  →  {name}  ->  {}{acted}", item.new_name),
            RowState::Conflict(kind) => println!("  !  {name}  ->  {}  [{kind}]", item.new_name),
            RowState::Error(message) => println!("  x  {name}  [{message}]"),
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
        println!("blocked: {reason}");
    }
}

fn store_at(dir: &Option<PathBuf>) -> PresetStore {
    PresetStore::new(dir.clone().unwrap_or_else(default_preset_dir))
}

fn run_presets(action: &PresetAction) -> Result<Exit, Box<dyn std::error::Error>> {
    match action {
        PresetAction::List { preset_dir } => {
            let store = store_at(preset_dir);
            let (entries, problems) = store.list();
            if entries.is_empty() && problems.is_empty() {
                println!("no presets in {}", store.dir().display());
            }
            for entry in &entries {
                let description = if entry.description.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", entry.description)
                };
                println!(
                    "{}  ({}){description}",
                    entry.name,
                    ren_core::plural(entry.steps, "operation")
                );
            }
            for problem in &problems {
                eprintln!("unreadable: {}", problem.error);
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
                    source.dir().display()
                );
            }
            println!("imported to {}", path.display());
            Ok(Exit::Success)
        }

        PresetAction::Export {
            name,
            to,
            preset_dir,
        } => {
            let store = store_at(preset_dir);
            let preset = store.load_named(name)?;
            store.export(&preset, to)?;
            println!("wrote {}", to.display());
            Ok(Exit::Success)
        }
    }
}
