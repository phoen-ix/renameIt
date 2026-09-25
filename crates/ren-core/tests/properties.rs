//! The invariants that matter more than any individual test.
//!
//! `docs/DESIGN.md` Part 3 names four, and a renamer that breaks any of them
//! loses somebody's files:
//!
//! * **No collision** — `plan()` never emits two identical folded targets.
//! * **No loss** — `apply` on a real tempdir preserves the file count and every
//!   file's contents.
//! * **Exact undo** — `undo(apply(p))` restores names and times exactly.
//! * **Preview truth** — the previewed name is the on-disk name afterwards.
//!
//! These run over arbitrary file sets and arbitrary pipelines, including the
//! cases that historically eat data: chains, swaps, case-only renames, unicode
//! and emoji.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::time::SystemTime;

use proptest::prelude::*;
use ren_core::model::Scope;
use ren_core::ops::{
    AddCounter, AddRemove, BatchReplace, CaseMode, Casing, CounterPlacement, DateTargets,
    FreeFormat, MoveSection, NumberAction, NumberTarget, OpKind, ReNumber, Replace, SetAttributes,
    SetDate, SpaceTrim, WallClock, ZeroPadding,
};
use ren_core::{
    ApplyOptions, CounterSetup, IncludeFilter, ListOptions, MatchSpec, Pipeline, PreProcessor,
    RunSettings, StepConfig, apply, list, plan, undo_last,
};
use tempfile::TempDir;

/// Names chosen to include everything that has historically broken renamers.
fn file_name() -> impl Strategy<Value = String> {
    prop_oneof![
        // Ordinary names, with the separators the ops care about.
        "[a-zA-Z0-9 ._()\\-]{1,12}",
        // Case-only differences, to exercise folded-target detection.
        prop::sample::select(vec![
            "readme".to_string(),
            "README".into(),
            "ReadMe".into(),
            "a.TXT".into(),
            "a.txt".into(),
        ]),
        // Chains and swaps.
        prop::sample::select(vec![
            "1".to_string(),
            "2".into(),
            "3".into(),
            "a".into(),
            "b".into(),
        ]),
        // Unicode and emoji.
        prop::sample::select(vec![
            "Ünïcödé".to_string(),
            "日本語".into(),
            "track 🎵".into(),
            "straße".into(),
        ]),
    ]
}

fn file_names() -> impl Strategy<Value = Vec<String>> {
    prop::collection::hash_set(file_name(), 1..8).prop_map(|set| {
        let mut names: Vec<String> = set.into_iter().collect();
        names.sort();
        names
    })
}

/// Templates that compile, including the one that turns a rename into a move.
///
/// Not arbitrary text: an unrecognised tag is a compile error (D29), so a
/// generator that produced `<xyz>` would only ever be testing the error path.
fn template() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        String::new(),
        "x".to_string(),
        "<Name>".into(),
        "<Counter>".into(),
        "<Counter>-<Name>".into(),
        "<Parent>_<Name>".into(),
        "<Left-2>".into(),
        "<NumFiles>".into(),
        // D31: a produced separator moves the file into a subfolder.
        "<Left-1><\\><Name>".into(),
        "sub<\\><Name>".into(),
    ])
}

/// The operations the invariants run over, as data.
///
/// Deliberately `OpKind` rather than a boxed transform: it is what the GUI
/// edits, what a preset stores, and what a job file parses into. Covered: the
/// General name operations, templates (with `<\>`), counters, number edits,
/// and the two metadata actions whose changes undo takes back — Set Date and
/// Set Attributes — so every invariant also holds for a run that acts on
/// files as well as renaming them. Not covered here, each for a reason: Script
/// and the CSV list need files of their own beside the listing (a script has
/// a property of its own, `a_scripted_preview_is_what_executes`); the
/// Filename Editor needs a list that matches the listing; the Music and tag
/// operations read and write audio files, and their writes cannot be undone
/// (P2).
fn op_kind() -> impl Strategy<Value = OpKind> {
    prop_oneof![
        ("[a-z ._]{0,3}", "[A-Z0-9 ]{0,3}")
            .prop_map(|(find, replace)| OpKind::Replace(Replace::new(find, replace))),
        Just(OpKind::BatchReplace(BatchReplace::default())),
        prop::sample::select(vec![
            CaseMode::Upper,
            CaseMode::Lower,
            CaseMode::Sentence,
            CaseMode::Title,
            CaseMode::Invert,
            CaseMode::Random,
        ])
        .prop_map(|mode| OpKind::Casing(Casing::new(mode))),
        (0usize..6, 0usize..6, any::<bool>()).prop_map(|(delete, pos, backwards)| {
            OpKind::AddRemove(AddRemove::remove(delete, pos).backwards(backwards))
        }),
        (template(), 0usize..6, any::<bool>()).prop_map(|(insert, pos, backwards)| {
            OpKind::AddRemove(AddRemove::add(insert, pos).backwards(backwards))
        }),
        (0usize..4, 0usize..6, 0usize..6)
            .prop_map(|(cut, from, to)| OpKind::MoveSection(MoveSection::new(cut, from, to))),
        Just(OpKind::SpaceTrim(SpaceTrim::default())),
        ("[-_. ]{0,2}", any::<bool>()).prop_map(|(separator, last)| {
            let placement = if last {
                CounterPlacement::Last
            } else {
                CounterPlacement::First
            };
            OpKind::AddCounter(AddCounter::new(placement, separator))
        }),
        (0usize..4).prop_map(|digits| OpKind::ZeroPadding(ZeroPadding::new(digits))),
        (
            prop::sample::select(vec![
                NumberAction::Add,
                NumberAction::Subtract,
                NumberAction::ReplaceWithCounter,
                NumberAction::Remove,
                NumberAction::ZeroPadTo,
            ]),
            prop::sample::select(vec![
                NumberTarget::All,
                NumberTarget::Nth(1),
                NumberTarget::Last,
            ]),
            0i64..5,
        )
            .prop_map(|(action, target, operand)| {
                OpKind::ReNumber(ReNumber::new(target, action).with_operand(operand.to_string()))
            }),
        template().prop_map(|pattern| OpKind::FreeFormat(FreeFormat::new(pattern))),
        // The modified date — the one stamp every platform can write, so a
        // generated run is not simply refused as unsupported (P5) on Linux.
        (0u32..28).prop_map(|day| {
            let when = chrono::NaiveDate::from_ymd_opt(2001, 9, day + 1)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap();
            OpKind::SetDate(SetDate {
                date: WallClock::from_naive(when),
                targets: DateTargets::default(),
                ..Default::default()
            })
        }),
        // Read-only, the one attribute every platform can write.
        prop::option::of(any::<bool>()).prop_map(|read_only| {
            OpKind::SetAttributes(SetAttributes {
                read_only,
                ..Default::default()
            })
        }),
    ]
}

/// The per-step settings a card carries, including the two the GUI could not
/// reach before M4.
fn step_config() -> impl Strategy<Value = StepConfig> {
    let scope = prop_oneof![Just(Scope::Name), Just(Scope::Extension), Just(Scope::Both)];
    let filter = prop_oneof![
        4 => Just(None),
        1 => "[a-z]{1,3}".prop_map(|text| Some(
            IncludeFilter::new().including(MatchSpec::Substring(text))
        )),
    ];
    let preproc = prop_oneof![
        4 => Just(None),
        1 => (0usize..4).prop_map(|n| Some(PreProcessor::new().skipping_first(n))),
    ];
    // Weighted towards enabled: a pipeline of disabled steps proves nothing,
    // but a disabled step among live ones proves a lot.
    let enabled = prop_oneof![4 => Just(true), 1 => Just(false)];
    (scope, enabled, filter, preproc).prop_map(|(scope, enabled, filter, preproc)| StepConfig {
        scope,
        enabled,
        filter,
        preproc,
    })
}

fn pipeline_steps() -> impl Strategy<Value = Vec<(OpKind, StepConfig)>> {
    prop::collection::vec((op_kind(), step_config()), 0..4)
}

fn build(steps: Vec<(OpKind, StepConfig)>) -> Pipeline {
    build_with(steps, RunSettings::default())
}

fn build_with(steps: Vec<(OpKind, StepConfig)>, settings: RunSettings) -> Pipeline {
    let mut pipeline = Pipeline::new();
    for (op, config) in steps {
        pipeline.push(op.to_step(), config);
    }
    pipeline.settings = settings;
    pipeline
}

struct Tree {
    dir: TempDir,
    journal: TempDir,
}

impl Tree {
    fn new(names: &[String]) -> Option<Self> {
        let dir = TempDir::new().ok()?;
        for (i, name) in names.iter().enumerate() {
            // Some generated names are not creatable on every filesystem;
            // skipping the whole case is better than asserting on the OS.
            if std::fs::write(dir.path().join(name), format!("payload {i}")).is_err() {
                return None;
            }
        }
        Some(Self {
            dir,
            journal: TempDir::new().ok()?,
        })
    }

    /// A tree with a folder in it, which is what Folders + Subfolders lists.
    ///
    /// The first name goes at the top, the rest go inside `folder`, so every
    /// case has at least one row on each side of the folder rename.
    fn nested(names: &[String], folder: &str) -> Option<Self> {
        if names.len() < 2 || names.contains(&folder.to_owned()) {
            return None;
        }
        let dir = TempDir::new().ok()?;
        let inside = dir.path().join(folder);
        std::fs::create_dir(&inside).ok()?;
        for (i, name) in names.iter().enumerate() {
            let at = if i == 0 { dir.path() } else { inside.as_path() };
            if std::fs::write(at.join(name), format!("payload {i}")).is_err() {
                return None;
            }
        }
        Some(Self {
            dir,
            journal: TempDir::new().ok()?,
        })
    }

    fn options(&self) -> ApplyOptions {
        ApplyOptions {
            simulate: false,
            allow_irreversible: false,
            journal_dir: self.journal.path().to_path_buf(),
            ..Default::default()
        }
    }
}

/// A generated Set Attributes step can leave a file read-only, and a read-only
/// file is one Windows will not let `TempDir` delete.
impl Drop for Tree {
    fn drop(&mut self) {
        fn writable(dir: &Path) {
            let Ok(read) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in read.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    writable(&path);
                } else if let Ok(meta) = std::fs::metadata(&path) {
                    let mut permissions = meta.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    permissions.set_readonly(false);
                    let _ = std::fs::set_permissions(&path, permissions);
                }
            }
        }
        writable(self.dir.path());
    }
}

/// Everything on disk under a root, as the invariants compare it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    /// `relative path -> (contents, mtime)` for every file.
    files: BTreeMap<String, (Vec<u8>, SystemTime)>,
    /// Every folder, by relative path. Without these, a folder an undo left
    /// behind or a simulation created was invisible to every "exact"
    /// assertion. No mtime: a folder's changes whenever anything inside it is
    /// renamed, and undo cannot put that back.
    dirs: BTreeSet<String>,
}

/// The whole tree, recursively, because `<\>` moves files into subfolders
/// (D31): a non-recursive snapshot would read a moved file as a lost one.
fn snapshot(dir: &Path) -> Snapshot {
    let mut out = Snapshot {
        files: BTreeMap::new(),
        dirs: BTreeSet::new(),
    };
    collect(dir, dir, &mut out);
    out
}

/// A path relative to `root`, spelled with `/` whatever the platform uses.
///
/// `PlanItem::new_name` is the name the *preview* showed, and D31 makes `/`
/// the one canonical separator in a produced name — so on Windows a file
/// moved into a subfolder is `sub\file` on disk and `sub/file` in the plan.
/// Comparing those without normalising fails for a difference in spelling
/// rather than in behaviour, which is what the Windows runner caught.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join(&ren_core::plan::SUBFOLDER_SEPARATOR.to_string())
}

fn collect(root: &Path, dir: &Path, out: &mut Snapshot) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            out.dirs.insert(relative(root, &path));
            collect(root, &path, out);
        } else {
            out.files.insert(
                relative(root, &path),
                (
                    std::fs::read(&path).unwrap_or_default(),
                    meta.modified().expect("mtime"),
                ),
            );
        }
    }
}

/// **Each file's contents are where the preview said that file would be.**
///
/// The name-level invariants compare *sets* — of names, of contents — and a
/// swap executed as a no-op leaves both sets exactly as they were. This is the
/// check that tells `a` becoming `b` apart from `a` staying `a`: every file
/// row's landing place (`final_path`, the preview's promise) holds the bytes
/// its source held before the run.
fn contents_followed_their_files(
    root: &Path,
    plan: &ren_core::Plan,
    before: &Snapshot,
) -> Result<(), TestCaseError> {
    for item in &plan.items {
        // Folder rows have no contents of their own; their files are rows too.
        let Some((contents, _)) = before.files.get(&relative(root, &item.source)) else {
            continue;
        };
        let landed = item.final_path();
        let now = std::fs::read(landed).ok();
        prop_assert_eq!(
            now.as_ref(),
            Some(contents),
            "{} should hold what {} held",
            landed.display(),
            item.source.display()
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// A plan that sends two files to the same name is a plan that deletes one
    /// of them. The folded comparison is what catches `README` vs `readme` on a
    /// case-insensitive volume.
    #[test]
    fn a_plan_never_sends_two_files_to_one_name(
        names in file_names(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::new(&names) else { return Ok(()); };
        let platform = ren_platform::host();
        let entries = list(tree.dir.path(), ListOptions::default()).unwrap();
        let plan = plan(&entries, &build(steps), platform.as_ref());

        let rules = platform.naming_rules(tree.dir.path());
        let mut seen = HashSet::new();
        for op in &plan.ops {
            let ren_core::PlannedOp::Rename { to, .. } = op else { continue };
            let folded = rules.fold(&to.to_string_lossy());
            prop_assert!(seen.insert(folded), "two ops target {}", to.display());
        }
    }

    /// Whatever the pipeline does, every file must still be there afterwards
    /// with its contents intact.
    #[test]
    fn applying_a_plan_never_loses_a_file(
        names in file_names(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::new(&names) else { return Ok(()); };
        let platform = ren_platform::host();
        let before = snapshot(tree.dir.path());

        let entries = list(tree.dir.path(), ListOptions::default()).unwrap();
        let plan = plan(&entries, &build(steps), platform.as_ref());
        if !plan.is_executable() {
            return Ok(());
        }
        let report = apply(&plan, platform.as_ref(), &tree.options()).unwrap();
        prop_assert!(report.is_success(), "{:?}", report.failed);

        let after = snapshot(tree.dir.path());
        prop_assert_eq!(before.files.len(), after.files.len(), "file count changed");

        let mut before_contents: Vec<_> = before.files.values().map(|(c, _)| c.clone()).collect();
        let mut after_contents: Vec<_> = after.files.values().map(|(c, _)| c.clone()).collect();
        before_contents.sort();
        after_contents.sort();
        prop_assert_eq!(before_contents, after_contents, "contents changed");
        contents_followed_their_files(tree.dir.path(), &plan, &before)?;
    }

    /// The name shown in the preview is the name written to disk. This is the
    /// promise the whole UI rests on.
    #[test]
    fn the_previewed_name_is_the_name_on_disk(
        names in file_names(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::new(&names) else { return Ok(()); };
        let platform = ren_platform::host();
        let before = snapshot(tree.dir.path());
        let entries = list(tree.dir.path(), ListOptions::default()).unwrap();
        let plan = plan(&entries, &build(steps), platform.as_ref());
        if !plan.is_executable() {
            return Ok(());
        }

        let mut previewed: Vec<String> = plan.items.iter().map(|i| i.new_name.clone()).collect();
        let report = apply(&plan, platform.as_ref(), &tree.options()).unwrap();
        prop_assert!(report.is_success(), "{:?}", report.failed);

        let mut on_disk: Vec<String> = snapshot(tree.dir.path()).files.into_keys().collect();
        previewed.sort();
        on_disk.sort();
        prop_assert_eq!(previewed, on_disk);
        // The names agreeing as a set is not enough: each has to be the right
        // file's name.
        contents_followed_their_files(tree.dir.path(), &plan, &before)?;
    }

    /// The same two promises — the preview is the truth, and undo is exact —
    /// over a tree with a folder in it, listed the way Folders + Subfolders
    /// lists it: the folder and the files inside it in one run.
    ///
    /// The flat generator could not reach this. It renamed the folder first and
    /// every rename underneath it failed on a path that no longer existed, and
    /// nothing in the suite had a folder to notice with.
    #[test]
    fn a_run_over_a_folder_and_its_contents_previews_truly_and_undoes_exactly(
        names in file_names(),
        folder in file_name(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::nested(&names, &folder) else { return Ok(()); };
        let platform = ren_platform::host();
        let before = snapshot(tree.dir.path());

        let entries = list(tree.dir.path(), ListOptions {
            folders: true,
            subfolders: true,
            ..Default::default()
        }).unwrap();
        let plan = plan(&entries, &build(steps), platform.as_ref());
        if !plan.is_executable() {
            return Ok(());
        }

        let previewed: Vec<(std::path::PathBuf, String)> = plan
            .items
            .iter()
            .filter(|i| i.state.is_changed())
            .map(|i| (i.target.clone(), i.new_name.clone()))
            .collect();

        let report = apply(&plan, platform.as_ref(), &tree.options()).unwrap();
        prop_assert!(report.is_success(), "{:?}", report.failed);

        for (target, name) in &previewed {
            prop_assert!(
                target.exists(),
                "the preview promised {} at {}",
                name,
                target.display()
            );
        }
        contents_followed_their_files(tree.dir.path(), &plan, &before)?;

        // A pipeline that changed nothing wrote no transaction, so there is
        // nothing to take back and nothing to check.
        if report.reversible == 0 {
            return Ok(());
        }
        let undo = undo_last(platform.as_ref(), tree.journal.path()).unwrap();
        prop_assert!(undo.is_complete(), "{:?}", undo.skipped);
        prop_assert_eq!(before, snapshot(tree.dir.path()), "undo was not exact");
    }

    /// Undo has to be exact, or it is worse than no undo at all.
    #[test]
    fn undo_restores_the_tree_exactly(
        names in file_names(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::new(&names) else { return Ok(()); };
        let platform = ren_platform::host();
        let before = snapshot(tree.dir.path());

        let entries = list(tree.dir.path(), ListOptions::default()).unwrap();
        let plan = plan(&entries, &build(steps), platform.as_ref());
        if !plan.is_executable() || plan.ops.is_empty() {
            return Ok(());
        }
        apply(&plan, platform.as_ref(), &tree.options()).unwrap();

        let undo = undo_last(platform.as_ref(), tree.journal.path()).unwrap();
        prop_assert!(undo.is_complete(), "{:?}", undo.skipped);
        prop_assert_eq!(snapshot(tree.dir.path()), before);
    }

    /// A simulation that touches the disk is a bug with teeth.
    #[test]
    fn simulating_never_touches_the_disk(
        names in file_names(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::new(&names) else { return Ok(()); };
        let platform = ren_platform::host();
        let before = snapshot(tree.dir.path());

        let entries = list(tree.dir.path(), ListOptions::default()).unwrap();
        let plan = plan(&entries, &build(steps), platform.as_ref());
        if !plan.is_executable() {
            return Ok(());
        }
        let options = ApplyOptions {
            simulate: true,
            ..tree.options()
        };
        apply(&plan, platform.as_ref(), &options).unwrap();

        prop_assert_eq!(snapshot(tree.dir.path()), before);
    }

    /// Evaluating a pipeline twice must give the same answer, or the preview
    /// would drift from the execute that follows it. This is what pins down
    /// `Random` casing.
    #[test]
    fn evaluating_a_pipeline_is_deterministic(
        names in file_names(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::new(&names) else { return Ok(()); };
        let platform = ren_platform::host();
        let entries = list(tree.dir.path(), ListOptions::default()).unwrap();
        let pipeline = build(steps);

        let first = plan(&entries, &pipeline, platform.as_ref());
        let second = plan(&entries, &pipeline, platform.as_ref());
        prop_assert_eq!(first.items, second.items);
        prop_assert_eq!(first.ops, second.ops);
    }

    /// Since D31 a produced name may carry a file *down* into a subfolder, so
    /// the old "never a separator" rule is now the stronger one it was standing
    /// in for: a target may never leave the folder its file came from.
    #[test]
    fn a_target_never_escapes_the_folder_its_file_came_from(
        names in file_names(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::new(&names) else { return Ok(()); };
        let platform = ren_platform::host();
        let entries = list(tree.dir.path(), ListOptions::default()).unwrap();
        let plan = plan(&entries, &build(steps), platform.as_ref());

        let root = tree.dir.path();
        for item in &plan.items {
            prop_assert!(
                item.target.starts_with(root),
                "{} escaped {}",
                item.target.display(),
                root.display()
            );
            // Nothing may reach the parent, whatever the components spell.
            prop_assert!(
                !item.target.components().any(|c| c.as_os_str() == ".."),
                "{} walks upwards",
                item.target.display()
            );
        }

        for op in &plan.ops {
            let path = match op {
                ren_core::PlannedOp::Rename { to, .. } => to,
                ren_core::PlannedOp::CreateDir { path } => path,
                ren_core::PlannedOp::Act { path, .. } => path,
                // Never generated here — `op_kind` has no Script — so this arm
                // only states the rule. The planner enforces it (P60 as
                // amended): a script may write only into a folder the run
                // lists, and every listed folder is under the root. The
                // engine tests hold the refusals.
                ren_core::PlannedOp::WriteFile { path, .. } => path,
            };
            prop_assert!(
                path.starts_with(root),
                "{} escaped {}",
                path.display(),
                root.display()
            );
        }
    }

    /// M7's acceptance criterion, as a property: **a scripted preview is what
    /// executes.**
    ///
    /// Two halves, and the second is the one that needs a property test. First,
    /// planning the same listing twice gives the same answer — which is not
    /// free for a script, because session semantics let one count and
    /// accumulate across rows, and under the parallel pass those
    /// would come out in a different order each time (D94). Second, what lands
    /// on disk is exactly what the plan said, for a pipeline whose output the
    /// planner cannot predict from the names alone.
    ///
    /// The script is deliberately stateful *and* order-dependent: it numbers
    /// the rows as it sees them, so any reordering shows up immediately.
    #[test]
    fn a_scripted_preview_is_what_executes(names in file_names()) {
        let scripts = TempDir::new().unwrap();
        std::fs::write(
            scripts.path().join("Numbered.koto"),
            "s = {n: 0}\nrename = ||\n  s.n += 1\n  '{s.n}-{fr.filename}'",
        ).unwrap();

        let build = || {
            Pipeline::new().then(
                ren_core::ops::Script::new("Numbered").in_dir(scripts.path()),
            )
        };

        let Some(tree) = Tree::new(&names) else { return Ok(()); };
        let entries = list(tree.dir.path(), ListOptions::default()).unwrap();
        let platform = ren_platform::host();

        let first = plan(&entries, &build(), platform.as_ref());
        let second = plan(&entries, &build(), platform.as_ref());
        let names_of = |p: &ren_core::Plan| -> Vec<String> {
            p.items.iter().map(|i| i.new_name.clone()).collect()
        };
        prop_assert_eq!(
            names_of(&first),
            names_of(&second),
            "two previews of the same folder disagreed"
        );

        // Only a plan with nothing to trip over can be executed at all; a
        // collision is the planner doing its job, not a counterexample.
        prop_assume!(first.is_executable());

        let previewed: Vec<(std::path::PathBuf, String)> = first
            .items
            .iter()
            .filter(|i| i.state.is_changed())
            .map(|i| (i.target.clone(), i.new_name.clone()))
            .collect();

        let journal = TempDir::new().unwrap();
        let report = apply(
            &first,
            platform.as_ref(),
            &ApplyOptions {
                journal_dir: journal.path().to_path_buf(),
                ..Default::default()
            },
        ).expect("an executable plan applies");
        prop_assert!(report.failed.is_empty(), "{:?}", report.failed);

        for (target, name) in &previewed {
            prop_assert!(
                target.exists(),
                "the preview promised {name} and it is not on disk"
            );
        }
    }

    /// The writer is the parser's exact inverse.
    ///
    /// Everything a preset stores goes through this: if a field survives as
    /// data but not as text, a preset saved and reloaded renames differently
    /// from the one the user built, which is the worst kind of quiet wrong.
    #[test]
    fn a_document_survives_a_round_trip_through_toml(
        steps in pipeline_steps(),
        start in -5i64..5,
        parts in "[a-z<>%1-9 -]{0,12}",
        require_all_tags in any::<bool>(),
    ) {
        let doc = ren_core::Document {
            meta: ren_core::PresetMeta {
                name: "Round trip".to_owned(),
                description: "written by a property test".to_owned(),
            },
            source: None,
            steps,
            settings: RunSettings {
                counter: CounterSetup { start, ..Default::default() },
                parts: ren_core::PartsSpec::new(parts),
                require_all_tags,
                seed: 0,
                ..Default::default()
            },
        };

        let text = doc.to_toml().expect("every document must be writable");
        let back = ren_core::Document::parse(&text)
            .unwrap_or_else(|e| panic!("wrote unreadable TOML: {e}\n{text}"));

        prop_assert_eq!(&back.steps, &doc.steps, "steps drifted:\n{}", text);
        prop_assert_eq!(&back.settings, &doc.settings, "settings drifted:\n{}", text);
        prop_assert_eq!(&back.meta, &doc.meta);
        // And writing what was written changes nothing.
        prop_assert_eq!(back.to_toml().unwrap(), text);
    }

    /// The stronger half: a preset that round-trips as *data* must also plan
    /// the same names. This is what catches a field that survives serde but is
    /// dropped on the way into the pipeline.
    #[test]
    fn a_preset_plans_the_same_names_after_a_round_trip(
        names in file_names(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::new(&names) else { return Ok(()); };
        let platform = ren_platform::host();
        let entries = list(tree.dir.path(), ListOptions::default()).unwrap();

        let preset = ren_core::Preset {
            name: "Round trip".to_owned(),
            description: String::new(),
            steps,
            settings: RunSettings::default(),
        };
        let (reloaded, _) = ren_core::Preset::parse(&preset.to_toml().unwrap()).unwrap();

        let before = plan(&entries, &preset.pipeline(), platform.as_ref());
        let after = plan(&entries, &reloaded.pipeline(), platform.as_ref());
        prop_assert_eq!(before.items, after.items);
    }
}

/// A name with a byte that is not valid UTF-8 in it.
///
/// The four invariants above cannot reach this: their generator produces Rust
/// `String`s, so an invalid name is unreachable by construction — which is
/// exactly why the panic on such a file survived to 1.0.0. This is the arm
/// that closes it.
#[cfg(unix)]
fn broken_names() -> impl Strategy<Value = Vec<Vec<u8>>> {
    // A lone continuation byte, a truncated two-byte sequence, a bare 0xFF,
    // and a valid name for company — the run has to keep working around it.
    prop::collection::hash_set(
        prop::sample::select(vec![
            b"caf\xE9.txt".to_vec(),
            b"\x80start.txt".to_vec(),
            b"end\xFF".to_vec(),
            b"mid\xC3dle.jpg".to_vec(),
            b"ordinary.txt".to_vec(),
        ]),
        1..5,
    )
    .prop_map(|set| {
        let mut names: Vec<Vec<u8>> = set.into_iter().collect();
        names.sort();
        names
    })
}

#[cfg(unix)]
fn raw_names_in(dir: &std::path::Path) -> Vec<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt as _;
    let mut names: Vec<Vec<u8>> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().as_bytes().to_vec())
        .collect();
    names.sort();
    names
}

#[cfg(unix)]
proptest! {
    /// **A run never mangles a name it could not read, and never dies on one.**
    ///
    /// The fifth invariant, and the one that was missing. Whatever the pipeline
    /// is, a file whose name is not valid UTF-8 comes out of the run with the
    /// same bytes it went in with — and the files beside it still rename (P63:
    /// one unreadable entry costs its own rows and no others).
    #[test]
    fn a_name_that_is_not_unicode_survives_any_pipeline_untouched(
        names in broken_names(),
        steps in pipeline_steps(),
    ) {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let dir = TempDir::new().unwrap();
        let journal = TempDir::new().unwrap();
        for (i, name) in names.iter().enumerate() {
            let at = dir.path().join(OsStr::from_bytes(name));
            if std::fs::write(&at, format!("payload {i}")).is_err() {
                return Ok(());
            }
        }

        let unreadable: Vec<Vec<u8>> = names
            .iter()
            .filter(|n| std::str::from_utf8(n).is_err())
            .cloned()
            .collect();

        let platform = ren_platform::host();
        let entries = list(dir.path(), ListOptions::default()).unwrap();
        prop_assert_eq!(entries.len(), names.len(), "every file is listed");

        let plan = plan(&entries, &build(steps), platform.as_ref());
        if !plan.is_executable() {
            return Ok(());
        }
        // The run completes. Before this suite existed it panicked here.
        let report = apply(&plan, platform.as_ref(), &ApplyOptions {
            journal_dir: journal.path().to_path_buf(),
            ..Default::default()
        }).unwrap();
        prop_assert!(report.is_success(), "{:?}", report.failed);

        // Every unreadable name is still there, byte for byte. Compared as
        // bytes on purpose: `to_string_lossy` would compare U+FFFD to U+FFFD
        // and pass even if the byte underneath had been replaced.
        let after = raw_names_in(dir.path());
        for name in &unreadable {
            prop_assert!(after.contains(name), "{name:?} was changed: {after:?}");
        }

        // And undo puts everything back exactly, including those bytes — when
        // there was anything to undo. A pipeline that changed no name (or a
        // listing that was all unreadable) opens no transaction.
        if report.renamed.is_empty() {
            prop_assert_eq!(raw_names_in(dir.path()), {
                let mut original = names.clone();
                original.sort();
                original
            });
            return Ok(());
        }
        let undo = ren_core::undo_last(platform.as_ref(), journal.path()).unwrap();
        prop_assert!(undo.is_complete(), "skipped: {:?}", undo.skipped);
        let mut original = names.clone();
        original.sort();
        prop_assert_eq!(raw_names_in(dir.path()), original);
    }
}

/// Names shaped to exercise the shipped Batch Replace rules: contractions
/// with and without their apostrophe, each stand-in the rules know, mixed
/// case, underscores, and text the rules must leave alone — in random order
/// and combination, so a rule that fires on a name an earlier rule produced
/// is generated as often as one that fires on the original.
fn contraction_soup() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("I ll".to_owned()),
        Just("dont".to_owned()),
        Just("DON T".to_owned()),
        Just("can`t".to_owned()),
        Just("Isn´t".to_owned()),
        Just("they re".to_owned()),
        Just("pedant".to_owned()),
        Just("wont".to_owned()),
        Just("here_s".to_owned()),
        Just("_".to_owned()),
        Just("  ".to_owned()),
        Just("Ünïcödé".to_owned()),
        Just("(final)".to_owned()),
        "[a-z]{1,4}",
    ];
    proptest::collection::vec(piece, 0..8).prop_map(|pieces| pieces.join(" "))
}

/// The naive reading of a Batch Replace: every rule, top to bottom, each
/// seeing the last one's output.
fn batch_naively(rules: &[Replace], name: &str) -> String {
    use ren_core::ops::NameTransform;
    let entry = ren_core::model::FileEntry::synthetic("/x/a.txt");
    let cx = ren_core::ops::EvalCx::simple(&entry, 0, 1);
    let mut current = name.to_owned();
    for rule in rules {
        current = rule.apply(&current, &cx).unwrap().into_owned();
    }
    current
}

proptest! {
    /// **Batch Replace's prefilter never changes an answer.**
    ///
    /// The card asks one `RegexSet` which of its rules can match a name and
    /// runs only those, re-asking after any rule that changed the name. That
    /// is exactly "every rule, top to bottom" only if the set and the rules
    /// agree on what matches — which they do by construction, since the
    /// engine hands every non-fancy pattern to the same crate the set is
    /// built from — and this holds the two readings equal over names built
    /// to make the rules fire, chain, and stay quiet.
    #[test]
    fn the_batch_replace_prefilter_agrees_with_running_every_rule(
        name in contraction_soup(),
    ) {
        use ren_core::ops::NameTransform;
        let batch = BatchReplace::default();
        let entry = ren_core::model::FileEntry::synthetic("/x/a.txt");
        let cx = ren_core::ops::EvalCx::simple(&entry, 0, 1);
        let filtered = batch.apply(&name, &cx).unwrap().into_owned();
        let naive = batch_naively(&batch.rules, &name);
        prop_assert_eq!(filtered, naive, "over {:?}", name);
    }
}

/// A platform that performs `survives` changes faithfully and then dies —
/// either instead of the next one (the crash came before the syscall) or
/// right after it (the syscall landed, its `Completed` line never did). The
/// two windows a crash can fall into, and recovery has to be right in both.
///
/// A "change" is every call the executor makes to alter a file: a rename, and
/// the date and attribute writes of the metadata actions. Folder creations
/// are not routed through the platform, so a crash cannot land *on* one —
/// the power-loss variants in [`crash_and_recover`] reach the state that
/// matters there instead, a folder created and never confirmed.
struct CrashingPlatform {
    inner: std::sync::Arc<dyn ren_platform::Platform>,
    survives: std::sync::atomic::AtomicUsize,
    after: bool,
}

impl CrashingPlatform {
    /// Runs `change` unless this is the call the crash lands on.
    fn change<T>(
        &self,
        change: impl FnOnce() -> ren_platform::Result<T>,
    ) -> ren_platform::Result<T> {
        use std::sync::atomic::Ordering;
        if self.survives.load(Ordering::SeqCst) == 0 {
            if self.after {
                change()?;
            }
            std::panic::panic_any(CRASH);
        }
        self.survives.fetch_sub(1, Ordering::SeqCst);
        change()
    }
}

impl std::fmt::Debug for CrashingPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrashingPlatform").finish_non_exhaustive()
    }
}

impl ren_platform::Platform for CrashingPlatform {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn capabilities(&self) -> &'static [ren_platform::Capability] {
        self.inner.capabilities()
    }
    fn rename(&self, from: &Path, to: &Path) -> ren_platform::Result<()> {
        self.change(|| self.inner.rename(from, to))
    }
    fn get_attributes(&self, path: &Path) -> ren_platform::Result<ren_platform::FileAttributes> {
        self.inner.get_attributes(path)
    }
    fn set_attributes(
        &self,
        path: &Path,
        change: ren_platform::AttributeChange,
    ) -> ren_platform::Result<()> {
        self.change(|| self.inner.set_attributes(path, change))
    }
    fn get_times(&self, path: &Path) -> ren_platform::Result<ren_platform::FileTimes> {
        self.inner.get_times(path)
    }
    fn set_times(&self, path: &Path, change: ren_platform::TimeChange) -> ren_platform::Result<()> {
        self.change(|| self.inner.set_times(path, change))
    }
    fn naming_rules(&self, path: &Path) -> &'static ren_platform::NamingRules {
        self.inner.naming_rules(path)
    }
    fn case_sensitivity(&self, dir: &Path) -> ren_platform::CaseSensitivity {
        self.inner.case_sensitivity(dir)
    }
    fn reveal_in_file_manager(&self, path: &Path) -> ren_platform::Result<()> {
        self.inner.reveal_in_file_manager(path)
    }
    fn notify_shell_changed(&self, path: &Path) {
        self.inner.notify_shell_changed(path);
    }
}

/// The payload the crash carries, so a real panic is not mistaken for it.
const CRASH: &str = "simulated crash";

/// What reached the disk before the crash, beyond what the executor synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Loss {
    /// A process death: everything the executor wrote is in the page cache,
    /// and the next reader sees it.
    Nothing,
    /// A power cut before the current intent's sync: the previous op's
    /// `Completed` (appended unsynced, D168) and the intent after it are both
    /// gone. The previous op *ran*; nothing on disk says so.
    CompletedAndIntent,
    /// The same, with the intent's page written back and the confirmation's
    /// not — nothing orders the two, so recovery has to be right either way.
    CompletedOnly,
}

/// Drops what `loss` says a power cut would have taken from the only journal
/// in `dir`. `false` when the journal does not end in the shape that loss
/// needs — an intent with the confirmation of the op before it just above —
/// so there is no such state to test.
fn lose_unsynced_tail(dir: &Path, loss: Loss) -> bool {
    if loss == Loss::Nothing {
        return true;
    }
    let path = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .expect("a journal");
    let lines = ren_core::exec::Journal::read(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let raw: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(raw.len(), lines.len());
    let n = lines.len();
    let intent = |r: &ren_core::exec::Record| {
        use ren_core::exec::Record::*;
        matches!(
            r,
            PlanRename { .. } | PlanCreateDir { .. } | PlanAct { .. } | PlanIrreversible { .. }
        )
    };
    if n < 2
        || !intent(&lines[n - 1].record)
        || !matches!(
            lines[n - 2].record,
            ren_core::exec::Record::Completed { .. }
        )
    {
        return false;
    }
    let keep = match loss {
        Loss::CompletedAndIntent => raw[..n - 2].to_vec(),
        Loss::CompletedOnly => [&raw[..n - 2], &raw[n - 1..]].concat(),
        Loss::Nothing => unreachable!(),
    };
    let mut out = keep.join("\n");
    out.push('\n');
    std::fs::write(&path, out).unwrap();
    true
}

/// Runs `plan` until the `survives`th change, "crashes" there, loses what
/// `loss` says, recovers, and checks the tree is exactly what it was.
fn crash_and_recover(
    tree: &Tree,
    plan: &ren_core::Plan,
    before: &Snapshot,
    survives: usize,
    after: bool,
    loss: Loss,
) -> Result<(), TestCaseError> {
    let platform = CrashingPlatform {
        inner: ren_platform::host(),
        survives: std::sync::atomic::AtomicUsize::new(survives),
        after,
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        apply(plan, &platform, &tree.options())
    }));
    match outcome {
        Err(payload) => {
            let ours = payload.downcast_ref::<&str>().is_some_and(|p| *p == CRASH);
            prop_assert!(ours, "a real panic, not the simulated crash");
        }
        // Fewer changes than `survives`: the run finished. Undo instead, which
        // exercises the same replay over a committed journal.
        Ok(report) => {
            let report = report.unwrap();
            prop_assert!(report.is_success(), "{:?}", report.failed);
            if report.reversible > 0 {
                undo_last(ren_platform::host().as_ref(), tree.journal.path()).unwrap();
            }
            prop_assert_eq!(before, &snapshot(tree.dir.path()));
            return Ok(());
        }
    }
    if !lose_unsynced_tail(tree.journal.path(), loss) {
        // No such state at this crash point. Put the tree back through the
        // ordinary path so the next case starts from `before`.
        let unfinished = ren_core::exec::unfinished(tree.journal.path()).0;
        ren_core::exec::rollback(&unfinished[0], ren_platform::host().as_ref()).unwrap();
        return Ok(());
    }

    let unfinished = ren_core::exec::unfinished(tree.journal.path()).0;
    prop_assert_eq!(unfinished.len(), 1, "one open transaction");
    let report = ren_core::exec::rollback(&unfinished[0], ren_platform::host().as_ref()).unwrap();
    prop_assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    prop_assert_eq!(
        before,
        &snapshot(tree.dir.path()),
        "after a crash at change {} ({} the syscall, {:?} lost)",
        survives,
        if after { "after" } else { "before" },
        loss
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// **A crash at any point of a run is recovered exactly** — with chains,
    /// swaps, subfolder moves, a folder row and metadata actions in the tree.
    ///
    /// The gate for the write-ahead window, which is **one** (P99): the
    /// executor announces one rename or folder creation, syncs, performs it,
    /// and appends its `Completed` unsynced for the next sync to carry (D168).
    /// Any future widening has to pass this first. Every change of the plan
    /// is a crash point, in both windows a crash can fall into — before the
    /// syscall, and after it but before its journal line — and a crash before
    /// the syscall is also tried as a power cut, which takes the unsynced
    /// confirmation of the op before it (and maybe the new intent) with it.
    /// Recovery decides each announced-but-unconfirmed op from the disk (D84).
    #[test]
    fn a_crash_at_any_change_is_recovered_exactly(
        names in file_names(),
        folder in file_name(),
        steps in pipeline_steps(),
    ) {
        let Some(tree) = Tree::nested(&names, &folder) else { return Ok(()); };
        let platform = ren_platform::host();
        let before = snapshot(tree.dir.path());

        let entries = list(tree.dir.path(), ListOptions {
            folders: true,
            subfolders: true,
            ..Default::default()
        }).unwrap();
        let plan = plan(&entries, &build(steps), platform.as_ref());
        if !plan.is_executable() || plan.ops.is_empty() {
            return Ok(());
        }
        let changes = plan
            .ops
            .iter()
            .filter(|op| matches!(op, ren_core::PlannedOp::Rename { .. } | ren_core::PlannedOp::Act { .. }))
            .count();

        for survives in 0..changes {
            for (after, loss) in [
                (false, Loss::Nothing),
                (true, Loss::Nothing),
                (false, Loss::CompletedAndIntent),
                (false, Loss::CompletedOnly),
            ] {
                crash_and_recover(&tree, &plan, &before, survives, after, loss)?;
                // Each crash leaves a rolled-back journal behind; start the
                // next one clean so `unfinished` sees exactly one.
                for entry in std::fs::read_dir(tree.journal.path()).unwrap() {
                    let _ = std::fs::remove_file(entry.unwrap().path());
                }
            }
        }
    }
}
