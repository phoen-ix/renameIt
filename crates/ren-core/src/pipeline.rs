//! A pipeline is an ordered stack of steps — and, from M4 on, also the preset
//! format (D8: "presets are just saved pipelines").

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::effect::PlannedAction;
use crate::filter::IncludeFilter;
use crate::model::{FileEntry, Scope, Subject};
use crate::ops::{EvalCx, NameTransform, OpError, SideEffectAction};
use crate::preproc::PreProcessor;
use crate::run::{Answers, RunContext, RunSettings};

#[derive(Debug)]
pub enum Step {
    Name(Box<dyn NameTransform>),
    /// Changes the file without renaming it — Set Attributes, Set Date.
    Action(Box<dyn SideEffectAction>),
}

/// One file's result: the name the pipeline produced, and everything else it
/// decided to do to the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub name: String,
    /// In step order. Empty for every pipeline made only of name transforms,
    /// and an empty `Vec` does not allocate — so the common path is free.
    pub actions: Vec<PlannedAction>,
}

/// Per-step settings. Rather than global options stored per preset item, the
/// modernised UI (D8) puts them on the operation card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StepConfig {
    /// D19: engine-owned and uniform across every operation.
    pub scope: Scope,
    pub enabled: bool,
    /// Tested against the **current** name, so an earlier step can change
    /// whether a later one runs: a step can rename a file into or out of a
    /// later step's filter.
    pub filter: Option<IncludeFilter>,
    /// Narrows the scope's slice further before the operation sees it.
    pub preproc: Option<PreProcessor>,
}

/// A step is enabled unless it says otherwise, which `#[derive(Default)]`
/// cannot express. `#[serde(default)]` on the struct routes missing job-file
/// keys through here, so `enabled` is opt-out rather than opt-in.
impl Default for StepConfig {
    fn default() -> Self {
        Self {
            scope: Scope::default(),
            enabled: true,
            filter: None,
            preproc: None,
        }
    }
}

impl StepConfig {
    pub fn scoped(scope: Scope) -> Self {
        Self {
            scope,
            ..Default::default()
        }
    }

    /// The settings a freshly added operation starts with.
    ///
    /// Only the scope varies, and only for Free Format — see
    /// [`crate::ops::OpKind::default_scope`].
    pub fn for_op(op: &crate::ops::OpKind) -> Self {
        Self::scoped(op.default_scope())
    }
}

#[derive(Debug, Default)]
pub struct Pipeline {
    steps: Vec<(Step, StepConfig)>,
    /// Counter, parts and the tag policy: run-wide settings rather than
    /// per-step ones. They live here so a pipeline is self-contained and M4
    /// can save one as a preset.
    pub settings: RunSettings,
    /// Answers collected before the run (D28). Empty during a preview, which is
    /// what makes `<Ask>` show a placeholder until the user is asked.
    pub answers: Answers,
}

impl Pipeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, step: Step, config: StepConfig) -> &mut Self {
        self.steps.push((step, config));
        self
    }

    pub fn with(mut self, step: Step, config: StepConfig) -> Self {
        self.steps.push((step, config));
        self
    }

    /// Adds a name transform with default settings.
    pub fn then(self, transform: impl NameTransform + 'static) -> Self {
        self.with(Step::Name(Box::new(transform)), StepConfig::default())
    }

    /// Adds a name transform scoped to a specific part of the name.
    pub fn then_scoped(self, transform: impl NameTransform + 'static, scope: Scope) -> Self {
        self.with(Step::Name(Box::new(transform)), StepConfig::scoped(scope))
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn steps(&self) -> &[(Step, StepConfig)] {
        &self.steps
    }

    /// Everything the enabled steps' tag fields will cost.
    pub fn needs(&self) -> crate::template::TagNeeds {
        let mut needs = crate::template::TagNeeds::NONE;
        for (step, config) in &self.steps {
            if config.enabled {
                needs.insert(match step {
                    Step::Name(transform) => transform.needs(),
                    Step::Action(action) => action.needs(),
                });
            }
        }
        needs
    }

    /// Every `<Ask>` slot in the pipeline, deduplicated and in slot order —
    /// one form, filled in once, before the run starts.
    pub fn asks(&self) -> Vec<crate::run::AskSpec> {
        let mut slots: Vec<u8> = self
            .steps
            .iter()
            .filter(|(_, config)| config.enabled)
            .flat_map(|(step, _)| match step {
                Step::Name(transform) => transform.asks(),
                Step::Action(action) => action.asks(),
            })
            .map(|spec| spec.slot)
            .collect();
        slots.sort_unstable();
        slots.dedup();
        slots
            .into_iter()
            .map(|slot| crate::run::AskSpec { slot })
            .collect()
    }

    /// The serial pre-pass (D28): resolves the counter sequence and the
    /// run-wide answers before anything is evaluated in parallel.
    pub fn run_context(&self, entries: &[FileEntry]) -> RunContext {
        RunContext::build(entries, &self.settings, self.answers.clone())
    }

    /// Whether any enabled step needs rows in list order.
    ///
    /// See [`NameTransform::is_order_sensitive`]. A disabled step does not
    /// count: it is not going to run, so it has no reason to cost the whole
    /// preview its parallelism.
    pub fn is_order_sensitive(&self) -> bool {
        self.steps.iter().any(|(step, config)| {
            config.enabled
                && match step {
                    Step::Name(transform) => transform.is_order_sensitive(),
                    Step::Action(_) => false,
                }
        })
    }

    /// Open whatever run-level state the enabled steps hold, before row zero.
    ///
    /// Only reached on an ordered run, which is the contract
    /// [`NameTransform::begin_run`] states.
    fn begin_run(&self, entries: &[FileEntry], run: &RunContext) {
        for (step, config) in &self.steps {
            if config.enabled
                && let Step::Name(transform) = step
            {
                transform.begin_run(entries, run);
            }
        }
    }

    /// Close the run and collect what the steps asked for beyond the names.
    ///
    /// Called by [`crate::plan::plan`] after the evaluation pass. Harmless on a
    /// pipeline with nothing order-sensitive in it: every other operation
    /// returns an empty outcome.
    pub fn end_run(&self) -> crate::ops::RunOutcome {
        let mut outcome = crate::ops::RunOutcome::default();
        for (step, config) in &self.steps {
            if config.enabled
                && let Step::Name(transform) = step
            {
                let step_outcome = transform.end_run();
                outcome.writes.extend(step_outcome.writes);
                outcome.notes.extend(step_outcome.notes);
            }
        }
        outcome
    }

    /// Runs every enabled step over one file, in one walk.
    ///
    /// Each step sees only its scope's slice, narrowed further by its
    /// pre-processor; the engine reassembles between steps, so a later step
    /// observes the earlier steps' output.
    ///
    /// Actions are folded into the *same* walk rather than gathered in a second
    /// pass, and that is a correctness point rather than an optimisation: a
    /// step's filter is tested against the name as earlier steps left it
    /// (`a_filter_sees_the_name_as_earlier_steps_left_it`). A second pass would
    /// test every action's filter against the *final* name and quietly act on
    /// the wrong files.
    pub fn evaluate(&self, cx: &EvalCx<'_>) -> Result<Evaluation, OpError> {
        let path = cx.entry.path.to_string_lossy();
        let mut name = cx.entry.file_name.clone();
        let mut actions: Vec<PlannedAction> = Vec::new();

        for (index, (step, config)) in self.steps.iter().enumerate() {
            if let Some(_no) = gate(config, &path, &name, cx.entry.is_dir)? {
                continue;
            }

            // An action has no scope and no pre-processor — both are about a
            // slice of a name, and it does not touch the name. The include
            // filter stays live, which is what the branch above preserves.
            let transform = match step {
                Step::Action(action) => {
                    let step_cx = cx.with_current(&name);
                    if let Some(effect) = action.effect(&step_cx)?
                        && !effect.is_empty()
                    {
                        actions.push(PlannedAction {
                            step: index,
                            op: action.id(),
                            describe: action.describe(&effect),
                            undoability: action.undoable(),
                            effect,
                        });
                    }
                    continue;
                }
                Step::Name(transform) => transform,
            };

            let Ok(subject) = narrowed(config, &name, cx.entry.is_dir)? else {
                continue; // Scope does not apply, or the pre-processor said no.
            };

            let step_cx = cx.with_current(&name);
            let transformed = transform.apply(subject.active(), &step_cx)?;
            if transformed == subject.active() {
                continue;
            }
            let next = subject.reassemble(&transformed);
            name = next;
        }
        Ok(Evaluation { name, actions })
    }

    /// The exact text step `index` is handed for this file.
    ///
    /// The same string [`Self::evaluate`] hands that step's operation — the name
    /// as the steps *before* it left it, sliced by its scope and narrowed by its
    /// pre-processor — or the reason it is handed none.
    ///
    /// Visual Assist needs this and nothing else does yet. A position field is
    /// measured on the operation's **input**, and for a card partway down a
    /// stack that is neither the name on disk nor the name in the New-name
    /// column: it is a string that exists nowhere until it is asked for. **D19**
    /// makes the slicing the engine's job, so this is the engine answering
    /// rather than the GUI guessing and drifting the first time a pre-processor
    /// rule changes.
    ///
    /// Actions are replayed as far as their *gate* and no further: `evaluate`
    /// never lets one touch the name, so running its effect here would change
    /// nothing and would re-read Exif from the disk to prove it.
    pub fn subject_at(
        &self,
        index: usize,
        cx: &EvalCx<'_>,
    ) -> Result<Result<StepSubject, NoSubject>, OpError> {
        let Some((step, config)) = self.steps.get(index) else {
            return Ok(Err(NoSubject::NoSuchStep));
        };
        let path = cx.entry.path.to_string_lossy();
        let mut name = cx.entry.file_name.clone();

        // Everything above `index`, exactly as `evaluate` would run it.
        for (earlier, earlier_config) in self.steps.iter().take(index) {
            if gate(earlier_config, &path, &name, cx.entry.is_dir)?.is_some() {
                continue;
            }
            let Step::Name(transform) = earlier else {
                continue; // An action leaves the name alone.
            };
            let Ok(subject) = narrowed(earlier_config, &name, cx.entry.is_dir)? else {
                continue;
            };
            let step_cx = cx.with_current(&name);
            let transformed = transform.apply(subject.active(), &step_cx)?;
            if transformed == subject.active() {
                continue;
            }
            name = subject.reassemble(&transformed);
        }

        if let Some(no) = gate(config, &path, &name, cx.entry.is_dir)? {
            return Ok(Err(no));
        }
        if matches!(step, Step::Action(_)) {
            return Ok(Err(NoSubject::NotANameTransform));
        }
        let range = match narrowed(config, &name, cx.entry.is_dir)? {
            Ok(subject) => subject.range(),
            Err(no) => return Ok(Err(no)),
        };
        Ok(Ok(StepSubject { name, range }))
    }

    /// Whether an enabled step above `index` is order-sensitive.
    ///
    /// A script's contribution is **not** in [`Self::subject_at`]'s answer: its
    /// session is `Idle` on a pipeline that has not had `begin_run` called, and
    /// `to_pipeline` clones a fresh one every time, so `Script::apply` returns
    /// its subject unchanged. Replaying it honestly would cost the whole preview
    /// and would need the rows before this one evaluated in order, because
    /// three of the shipped scripts count across rows.
    ///
    /// So it is reported rather than hidden — one sentence in the UI beats a
    /// string that is quietly missing a step.
    pub fn unreplayed_script_before(&self, index: usize) -> bool {
        self.steps
            .iter()
            .take(index)
            .any(|(step, config)| match (config.enabled, step) {
                (true, Step::Name(transform)) => transform.is_order_sensitive(),
                _ => false,
            })
    }
}

/// Why a step is handed no text for a file.
///
/// Every one of these is a configuration the user can see and change, which is
/// why the reason travels rather than collapsing into an empty string: "the box
/// is blank" has five causes and four of them are something the user did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoSubject {
    /// There is no step at that index — the card was deleted this frame.
    NoSuchStep,
    /// The card is unticked.
    Disabled,
    /// This step's own include filter rejects this file. Tested against the
    /// name as earlier steps left it, so it can differ from step to step.
    FilteredOut,
    /// Set Attributes and Set Date change the file, not its name: no scope, no
    /// pre-processor, nothing to select in.
    NotANameTransform,
    /// `Scope::Extension` on a file that has none.
    ScopeDoesNotApply,
    /// The pre-processor matched nothing, so this step leaves the file alone.
    PreProcessorRejected,
}

/// The text one step is handed, in the name it was cut from.
///
/// The whole name travels with the range rather than just the slice, so a
/// caller can show the out-of-scope parts greyed instead of showing a fragment
/// with no context — the difference between a user understanding what
/// *Process: Name* did and wondering where their extension went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSubject {
    /// The name as the steps before this one left it.
    pub name: String,
    /// Byte range of [`Self::name`]. **Bytes**, because `Subject` is byte-ranged;
    /// a caller counting characters converts once, at its own edge.
    pub range: std::ops::Range<usize>,
}

impl StepSubject {
    /// The part the operation sees, and the only part it may change.
    pub fn active(&self) -> &str {
        &self.name[self.range.clone()]
    }

    /// The active slice in **characters** — the unit every position field in
    /// this program is measured in (P15).
    pub fn char_len(&self) -> usize {
        self.active().chars().count()
    }

    pub fn prefix(&self) -> &str {
        &self.name[..self.range.start]
    }

    pub fn suffix(&self) -> &str {
        &self.name[self.range.end..]
    }
}

/// Enabled, and past the include filter. The gate an action passes too.
///
/// Returns the reason it did *not* pass, so one caller can `continue` and the
/// other can say why. Shared with [`Pipeline::subject_at`] rather than written
/// twice, because a change to what stops a step running that reached only one
/// of them would be invisible until someone noticed the strip disagreeing with
/// the preview.
fn gate(
    config: &StepConfig,
    path: &str,
    name: &str,
    is_dir: bool,
) -> Result<Option<NoSubject>, OpError> {
    if !config.enabled {
        return Ok(Some(NoSubject::Disabled));
    }
    if let Some(filter) = &config.filter
        && !filter
            .accepts_entry(path, name, is_dir)
            .map_err(|e| OpError::new("include filter", e))?
    {
        return Ok(Some(NoSubject::FilteredOut));
    }
    Ok(None)
}

/// Scope, then pre-processor. Name steps only.
///
/// The other half of the shared gate — see [`gate`]. `is_dir` because a
/// folder's name has no extension to scope away ([`crate::model::split_name`]).
fn narrowed<'n>(
    config: &StepConfig,
    name: &'n str,
    is_dir: bool,
) -> Result<Result<Subject<'n>, NoSubject>, OpError> {
    let Some(subject) = config.scope.slice(name, is_dir) else {
        return Ok(Err(NoSubject::ScopeDoesNotApply));
    };
    let Some(preproc) = &config.preproc else {
        return Ok(Ok(subject));
    };
    match preproc
        .narrow(&subject)
        .map_err(|e| OpError::new("pre-processor", e))?
    {
        Some(narrowed) => Ok(Ok(narrowed)),
        // Nothing to narrow to: this step leaves the file alone.
        None => Ok(Err(NoSubject::PreProcessorRejected)),
    }
}

/// The pure pass of the preview engine (`docs/DESIGN.md` Part 1 §4).
///
/// Order-independent by construction, which is what lets it use rayon — with
/// one exception, added in M7 and stated rather than smuggled: a pipeline
/// containing a **script** step is evaluated serially in list order, because a
/// script's globals are session-scoped and three of the shipped scripts count
/// and accumulate across rows. See
/// [`NameTransform::is_order_sensitive`](crate::ops::NameTransform::is_order_sensitive).
pub fn evaluate_all(
    entries: &[FileEntry],
    pipeline: &Pipeline,
) -> Vec<Result<Evaluation, OpError>> {
    let run = pipeline.run_context(entries);
    evaluate_all_with(entries, pipeline, &run)
}

/// The same pass with a run context the caller already built (D28).
pub fn evaluate_all_with(
    entries: &[FileEntry],
    pipeline: &Pipeline,
    run: &RunContext,
) -> Vec<Result<Evaluation, OpError>> {
    let total = entries.len();
    let one = |(index, entry): (usize, &FileEntry)| {
        pipeline.evaluate(&EvalCx::new(entry, index, total, run))
    };

    if pipeline.is_order_sensitive() {
        // Serial, in list order, and *only* for the pipelines that need it. The
        // cost is real — a scripted preview of a large folder gives up every
        // core but one — and it buys the thing that makes such a preview worth
        // showing at all: run it twice, get the same answer.
        pipeline.begin_run(entries, run);
        entries.iter().enumerate().map(one).collect()
    } else {
        entries.par_iter().enumerate().map(one).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::MatchSpec;
    use crate::ops::{AddRemove, AppendSuffix, CaseMode, Casing, Replace, Script};
    use std::path::PathBuf;

    fn entry(name: &str) -> FileEntry {
        FileEntry::synthetic(PathBuf::from("/tmp/music").join(name))
    }

    fn eval(pipeline: &Pipeline, name: &str) -> String {
        let e = entry(name);
        pipeline.evaluate(&EvalCx::simple(&e, 0, 1)).unwrap().name
    }

    #[test]
    fn name_scope_leaves_the_extension_intact() {
        let p = Pipeline::new().then(AppendSuffix::new("_v2"));
        assert_eq!(eval(&p, "song.mp3"), "song_v2.mp3");
    }

    #[test]
    fn extension_scope_only_touches_the_extension() {
        let p = Pipeline::new().then_scoped(AppendSuffix::new("x"), Scope::Extension);
        assert_eq!(eval(&p, "song.mp3"), "song.mp3x");
    }

    #[test]
    fn a_disabled_step_changes_nothing() {
        let p = Pipeline::new().with(
            Step::Name(Box::new(AppendSuffix::new("_v2"))),
            StepConfig {
                enabled: false,
                ..Default::default()
            },
        );
        assert_eq!(eval(&p, "song.mp3"), "song.mp3");
    }

    #[test]
    fn later_steps_see_earlier_output() {
        let p = Pipeline::new()
            .then(AppendSuffix::new("_a"))
            .then(AppendSuffix::new("_b"));
        assert_eq!(eval(&p, "song.mp3"), "song_a_b.mp3");
    }

    #[test]
    fn an_empty_pipeline_is_the_identity() {
        let p = Pipeline::new();
        assert_eq!(eval(&p, "song.mp3"), "song.mp3");
        assert_eq!(eval(&p, "READ ME"), "READ ME");
    }

    /// The two-selector Casing panel is two steps under D19.
    #[test]
    fn casing_the_name_and_the_extension_is_two_steps() {
        let p = Pipeline::new()
            .then_scoped(Casing::new(CaseMode::Title), Scope::Name)
            .then_scoped(Casing::new(CaseMode::Lower), Scope::Extension);
        assert_eq!(eval(&p, "my holiday photo.JPG"), "My Holiday Photo.jpg");
    }

    #[test]
    fn a_step_filter_can_skip_a_file_without_stopping_the_pipeline() {
        let p = Pipeline::new()
            .with(
                Step::Name(Box::new(AppendSuffix::new("_live"))),
                StepConfig {
                    filter: Some(
                        IncludeFilter::new().including(MatchSpec::Substring("concert".into())),
                    ),
                    ..Default::default()
                },
            )
            .then(AppendSuffix::new("_done"));

        assert_eq!(eval(&p, "concert.mp3"), "concert_live_done.mp3");
        assert_eq!(eval(&p, "studio.mp3"), "studio_done.mp3");
    }

    /// An earlier step can rename a file into a later step's filter.
    #[test]
    fn a_filter_sees_the_name_as_earlier_steps_left_it() {
        let p = Pipeline::new().then(Replace::new("demo", "final")).with(
            Step::Name(Box::new(AppendSuffix::new("!"))),
            StepConfig {
                filter: Some(IncludeFilter::new().including(MatchSpec::Substring("final".into()))),
                ..Default::default()
            },
        );
        assert_eq!(eval(&p, "track demo.mp3"), "track final!.mp3");
    }

    /// The pre-processor narrowing what a step can reach, as a pipeline.
    #[test]
    fn a_pre_processor_narrows_what_the_operation_can_reach() {
        let p = Pipeline::new().with(
            Step::Name(Box::new(Casing::new(CaseMode::Upper))),
            StepConfig {
                preproc: Some(PreProcessor::new().skipping_first(14)),
                ..Default::default()
            },
        );
        assert_eq!(
            eval(&p, "Batch Renamer is fantastic!.txt"),
            "Batch Renamer IS FANTASTIC!.txt"
        );
    }

    /// Positions are measured on the narrowed slice: skip the start of the
    /// name with the pre-processor and "position 0" is the start of what is
    /// left, which sits in the middle of the name.
    #[test]
    fn adding_at_position_zero_lands_inside_the_pre_processed_section() {
        let p = Pipeline::new().with(
            Step::Name(Box::new(AddRemove::add(">", 0))),
            StepConfig {
                preproc: Some(PreProcessor::new().skipping_first(6)),
                ..Default::default()
            },
        );
        assert_eq!(eval(&p, "Batch Renamer.txt"), "Batch >Renamer.txt");
    }

    #[test]
    fn a_pre_processor_that_rejects_a_file_skips_only_that_step() {
        let p = Pipeline::new()
            .with(
                Step::Name(Box::new(AppendSuffix::new("_x"))),
                StepConfig {
                    preproc: Some(
                        PreProcessor::new().skipping_until(MatchSpec::Substring("zzz".into())),
                    ),
                    ..Default::default()
                },
            )
            .then(AppendSuffix::new("_y"));
        assert_eq!(eval(&p, "song.mp3"), "song_y.mp3");
    }

    #[test]
    fn evaluate_all_runs_the_whole_listing() {
        let entries = [entry("a_b.txt"), entry("c_d.txt")];
        let p = Pipeline::new().then(Replace::new("_", "-"));
        let out = evaluate_all(&entries, &p);
        assert_eq!(out[0].as_ref().unwrap().name, "a-b.txt");
        assert_eq!(out[1].as_ref().unwrap().name, "c-d.txt");
    }

    // --- `subject_at`: the text a step is handed --------------------------

    /// Records what the engine actually handed it, and changes nothing.
    ///
    /// The only honest way to test `subject_at`: assert against what `evaluate`
    /// *did*, not against what this file thinks it should have done. A
    /// hand-written expected string would agree with a wrong `subject_at` the
    /// moment both were wrong in the same way.
    #[derive(Debug, Default)]
    struct Probe(std::sync::Mutex<Vec<String>>);

    impl NameTransform for Probe {
        fn id(&self) -> &'static str {
            "probe"
        }
        fn summary(&self) -> String {
            "probe".into()
        }
        fn apply<'a>(
            &self,
            subject: &'a str,
            _cx: &EvalCx<'_>,
        ) -> Result<std::borrow::Cow<'a, str>, OpError> {
            self.0.lock().unwrap().push(subject.to_owned());
            Ok(std::borrow::Cow::Borrowed(subject))
        }
    }

    /// Builds a pipeline whose last step is a `Probe`, runs it, and returns
    /// what the probe saw beside what `subject_at` claims it would see.
    fn what_the_last_step_sees(
        build: impl FnOnce(Pipeline) -> Pipeline,
        config: StepConfig,
        name: &str,
    ) -> (Option<String>, Result<StepSubject, NoSubject>) {
        let probe = std::sync::Arc::new(Probe::default());
        let watcher = std::sync::Arc::clone(&probe);

        #[derive(Debug)]
        struct Shared(std::sync::Arc<Probe>);
        impl NameTransform for Shared {
            fn id(&self) -> &'static str {
                "probe"
            }
            fn summary(&self) -> String {
                "probe".into()
            }
            fn apply<'a>(
                &self,
                subject: &'a str,
                cx: &EvalCx<'_>,
            ) -> Result<std::borrow::Cow<'a, str>, OpError> {
                self.0.apply(subject, cx)
            }
        }

        let pipeline = build(Pipeline::new()).with(Step::Name(Box::new(Shared(probe))), config);
        let last = pipeline.len() - 1;
        let e = entry(name);
        let cx = EvalCx::simple(&e, 0, 1);
        pipeline.evaluate(&cx).unwrap();
        let claimed = pipeline.subject_at(last, &cx).unwrap();
        let seen = watcher.0.lock().unwrap().first().cloned();
        (seen, claimed)
    }

    /// The whole point: what the strip shows must be what the operation gets.
    ///
    /// Four shapes in one table, because the ways these can disagree are all
    /// different — an earlier step's output, a scope, a pre-processor, and a
    /// per-step filter that only some files pass.
    #[test]
    fn the_text_a_step_is_handed_is_the_text_subject_at_reports() {
        // Composition: the probe must see the *first* step's output.
        let (seen, claimed) = what_the_last_step_sees(
            |p| p.then(Replace::new("_", "-")),
            StepConfig::default(),
            "my_holiday.jpg",
        );
        assert_eq!(seen.as_deref(), Some("my-holiday"));
        assert_eq!(claimed.unwrap().active(), "my-holiday");

        // Scope: the extension only.
        let (seen, claimed) = what_the_last_step_sees(
            |p| p,
            StepConfig::scoped(Scope::Extension),
            "my_holiday.jpg",
        );
        assert_eq!(seen.as_deref(), Some("jpg"));
        let subject = claimed.unwrap();
        assert_eq!(subject.active(), "jpg");
        assert_eq!(subject.prefix(), "my_holiday.");
        assert_eq!(subject.suffix(), "");

        // Pre-processor: narrowed further still.
        let (seen, claimed) = what_the_last_step_sees(
            |p| p,
            StepConfig {
                preproc: Some(PreProcessor::new().skipping_first(3)),
                ..Default::default()
            },
            "my_holiday.jpg",
        );
        assert_eq!(seen.as_deref(), Some("holiday"));
        assert_eq!(claimed.unwrap().active(), "holiday");
    }

    /// Every reason a step gets nothing is a reason the user can act on, so it
    /// travels rather than collapsing into an empty string.
    #[test]
    fn a_step_that_is_handed_nothing_says_which_of_the_five_reasons_it_is() {
        let e = entry("song.mp3");
        let cx = EvalCx::simple(&e, 0, 1);
        let at = |p: &Pipeline| p.subject_at(0, &cx).unwrap();

        let disabled = Pipeline::new().with(
            Step::Name(Box::new(AppendSuffix::new("x"))),
            StepConfig {
                enabled: false,
                ..Default::default()
            },
        );
        assert_eq!(at(&disabled), Err(NoSubject::Disabled));

        let filtered = Pipeline::new().with(
            Step::Name(Box::new(AppendSuffix::new("x"))),
            StepConfig {
                filter: Some(
                    IncludeFilter::new().including(MatchSpec::Substring("nothing here".into())),
                ),
                ..Default::default()
            },
        );
        assert_eq!(at(&filtered), Err(NoSubject::FilteredOut));

        let no_extension = Pipeline::new().with(
            Step::Name(Box::new(AppendSuffix::new("x"))),
            StepConfig::scoped(Scope::Extension),
        );
        let readme = entry("README");
        let readme_cx = EvalCx::simple(&readme, 0, 1);
        assert_eq!(
            no_extension.subject_at(0, &readme_cx).unwrap(),
            Err(NoSubject::ScopeDoesNotApply)
        );

        let rejected = Pipeline::new().with(
            Step::Name(Box::new(AppendSuffix::new("x"))),
            StepConfig {
                preproc: Some(
                    PreProcessor::new().skipping_until(MatchSpec::Substring("zzz".into())),
                ),
                ..Default::default()
            },
        );
        assert_eq!(at(&rejected), Err(NoSubject::PreProcessorRejected));

        assert_eq!(
            Pipeline::new().subject_at(0, &cx).unwrap(),
            Err(NoSubject::NoSuchStep)
        );
    }

    /// An empty active slice is **not** the same as no subject. Both of these
    /// are legitimate insertion points, and calling them "empty" is how someone
    /// conflates them with `None` a year from now.
    #[test]
    fn an_empty_slice_is_a_subject_and_not_a_refusal() {
        // A trailing period yields an extension that is present and empty.
        let dotted = entry("weird.");
        let cx = EvalCx::simple(&dotted, 0, 1);
        let p = Pipeline::new().with(
            Step::Name(Box::new(AppendSuffix::new("x"))),
            StepConfig::scoped(Scope::Extension),
        );
        let subject = p.subject_at(0, &cx).unwrap().expect("present, and empty");
        assert_eq!(subject.active(), "");
        assert_eq!(subject.char_len(), 0);
    }

    /// Positions are characters, so the slice has to be measured in them —
    /// `song.mp3` at `Scope::Name` is four, whatever the bytes say.
    #[test]
    fn the_slice_is_measured_in_characters_not_bytes() {
        let accented = entry("Ünïcödé.txt");
        let cx = EvalCx::simple(&accented, 0, 1);
        let p = Pipeline::new().then(AppendSuffix::new(""));
        let subject = p.subject_at(0, &cx).unwrap().unwrap();
        assert_eq!(subject.active(), "Ünïcödé");
        assert_eq!(subject.char_len(), 7);
        assert!(subject.range.len() > 7, "and the bytes are more than that");
    }

    /// A script above the card is not replayed, and the caller has to be able to
    /// say so rather than show text that is quietly missing a step.
    #[test]
    fn an_order_sensitive_step_above_is_reported() {
        let plain = Pipeline::new()
            .then(Replace::new("a", "b"))
            .then(AppendSuffix::new("x"));
        assert!(!plain.unreplayed_script_before(1));

        // A script with something chosen is order-sensitive; above the card it
        // is reported, below it is not.
        let scripted = Pipeline::new()
            .then(Script::new("x"))
            .then(AppendSuffix::new("y"));
        assert!(scripted.unreplayed_script_before(1));
        assert!(!scripted.unreplayed_script_before(0));

        // A disabled script is not going to run, so it hides nothing.
        let disabled = Pipeline::new()
            .with(
                Step::Name(Box::new(Script::new("x"))),
                StepConfig {
                    enabled: false,
                    ..Default::default()
                },
            )
            .then(AppendSuffix::new("y"));
        assert!(!disabled.unreplayed_script_before(1));
    }

    fn folder(name: &str) -> FileEntry {
        let mut e = entry(name);
        e.is_dir = true;
        e
    }

    fn eval_entry(pipeline: &Pipeline, e: &FileEntry) -> String {
        pipeline.evaluate(&EvalCx::simple(e, 0, 1)).unwrap().name
    }

    /// A folder has no extension: `Vol. 2` is one name, not the stem `Vol`
    /// and the extension ` 2`. Sliced like a file, every name-scoped card
    /// reached only the text before the last period.
    #[test]
    fn a_folder_name_is_all_name_and_no_extension() {
        let title = Pipeline::new().then(Casing::new(CaseMode::Title));
        assert_eq!(
            eval_entry(&title, &folder("dr. who season 1")),
            "Dr. Who Season 1"
        );
        assert_eq!(
            eval_entry(&title, &entry("dr. who season 1")),
            "Dr. who season 1",
            "a file's text after the last period is its extension"
        );

        let dots = Pipeline::new().then(Replace::new(".", "_"));
        assert_eq!(eval_entry(&dots, &folder("regex-1.13.1")), "regex-1_13_1");

        let extension = Pipeline::new().then_scoped(AppendSuffix::new("x"), Scope::Extension);
        assert_eq!(
            eval_entry(&extension, &folder("Vol. 2")),
            "Vol. 2",
            "Scope::Extension leaves a folder alone, as it does an extensionless file"
        );

        // The templates agree with the slicing.
        let tags =
            Pipeline::new().then_scoped(crate::ops::FreeFormat::new("<Name>|<Ext>"), Scope::Both);
        assert_eq!(eval_entry(&tags, &folder("Vol. 2")), "Vol. 2|");
        assert_eq!(eval_entry(&tags, &entry("Vol. 2")), "Vol| 2");

        // And so does the include filter's stem.
        let filtered = Pipeline::new().with(
            Step::Name(Box::new(AppendSuffix::new("!"))),
            StepConfig {
                filter: Some(IncludeFilter::new().including(MatchSpec::Wildcard("*2".into()))),
                ..Default::default()
            },
        );
        assert_eq!(eval_entry(&filtered, &folder("Vol. 2")), "Vol. 2!");
    }
}
