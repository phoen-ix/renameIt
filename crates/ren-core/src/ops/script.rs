//! Scripting — the operation card.
//!
//! Scripting extends the app with functions it does not ship.
//!
//! The card is deliberately thin: two controls and two persisted settings —
//! **which script**, and **the arguments**.
//! Everything else — the sandbox, the façade, the session — lives in
//! [`crate::script`].
//!
//! # Why this operation needs rows in order
//!
//! Every other name transform is a pure function of one string, which is what
//! lets [`crate::pipeline::evaluate_all_with`] run the whole listing through
//! rayon. A script is not: its globals stay static for the whole rename
//! session, and three of the nine shipped scripts depend on that.
//!
//! So a Script step reports [`NameTransform::is_order_sensitive`], and the
//! pipeline evaluates that whole run serially in list order instead. That is a
//! deliberate trade — a scripted preview gives up parallelism — and it is the
//! honest one: the alternative is to keep `par_iter` and let a stateful script
//! produce a different answer every time it is previewed.
//!
//! It also buys a guarantee: rows reach a script in list order, so a script
//! that counts across rows can rely on the sequence.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};
use crate::model::FileEntry;
use crate::run::RunContext;
use crate::script::{Compiled, SCRIPT_DEADLINE, ScriptError, Session, store};

/// Where a session is between runs.
#[derive(Debug, Default)]
enum State {
    /// Not inside a run. `apply` in this state is a probe, not an evaluation —
    /// `OpKind::problem` reaches it once per frame — so it does not run
    /// anything.
    #[default]
    Idle,
    /// The script could not be started. Every row reports it, rather than the
    /// run silently doing nothing.
    Failed(ScriptError),
    Live(Box<Session>),
}

/// The live session. Per-*run* state, not configuration — so it is invisible to
/// `Clone`, equality and serde, exactly as [`crate::cache::Cached`] is.
#[derive(Default)]
struct SessionSlot(Mutex<State>);

impl Clone for SessionSlot {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for SessionSlot {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for SessionSlot {}

impl std::fmt::Debug for SessionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.lock() {
            Ok(state) => write!(f, "SessionSlot({state:?})"),
            Err(_) => f.write_str("SessionSlot(poisoned)"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Script {
    /// *Choose script* — the file stem.
    ///
    /// Empty on a fresh card, and an empty card is a no-op rather than an
    /// error: P34 says an operation must not shout before the user has picked
    /// anything.
    pub script: String,
    /// Arguments passed on to the script, which uses them however it likes —
    /// one untyped string.
    pub args: String,

    /// Which folder to look in. `None` means the user's script folder; tests
    /// and the CLI point it somewhere else.
    ///
    /// Not serialised: a preset that hard-coded a machine's script folder would
    /// be a preset that only works on that machine.
    #[serde(skip)]
    dir: Option<PathBuf>,

    #[serde(skip)]
    session: SessionSlot,
}

impl Script {
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn with_args(mut self, args: impl Into<String>) -> Self {
        self.args = args.into();
        self
    }

    /// Look for scripts in `dir` rather than the user's folder.
    #[must_use]
    pub fn in_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = Some(dir.into());
        self
    }

    /// The folder this card reads from — what the picker enumerates.
    pub fn store(&self) -> store::ScriptStore {
        self.store_in(None)
    }

    /// The same, with the run's folder as a fallback.
    ///
    /// The card's own `dir` wins when it has one (tests set it); otherwise the
    /// run may name a folder (`ren-cli --script-dir`), and failing both it is
    /// the user's. `problem()` has no run to consult, which is why the two are
    /// separate: a card can only ever check against the folder it knows about.
    fn store_in(&self, run: Option<&RunContext>) -> store::ScriptStore {
        match self
            .dir
            .as_ref()
            .or(run.and_then(|r| r.script_dir.as_ref()))
        {
            Some(dir) => store::ScriptStore::new(dir),
            None => store::ScriptStore::user(),
        }
    }

    /// The chosen script's header and compile state, for the card to display.
    ///
    /// `None` when nothing is chosen. Reads through the same cache the preview
    /// uses, so drawing the card costs a `stat`.
    pub fn chosen(&self) -> Option<Arc<Result<Compiled, ScriptError>>> {
        self.compiled()
    }

    /// The compiled script, or `None` for a card nobody has configured yet.
    ///
    /// Reads through the process-wide cache in [`store::load`], so the cost of
    /// the GUI cloning this operation every keystroke is a `stat` rather than a
    /// read-and-compile — the same arrangement `CsvList` uses for its CSV.
    fn compiled(&self) -> Option<Arc<Result<Compiled, ScriptError>>> {
        self.compiled_in(None)
    }

    fn compiled_in(&self, run: Option<&RunContext>) -> Option<Arc<Result<Compiled, ScriptError>>> {
        if self.script.is_empty() {
            return None;
        }
        Some(store::load(&self.store_in(run).path_of(&self.script)))
    }
}

impl NameTransform for Script {
    fn id(&self) -> &'static str {
        "script"
    }

    fn summary(&self) -> String {
        if self.script.is_empty() {
            return "No script chosen".into();
        }
        if self.args.is_empty() {
            format!("Run '{}'", self.script)
        } else {
            format!("Run '{}' with '{}'", self.script, self.args)
        }
    }

    /// A script step is evaluated in list order, not in parallel. See the
    /// module docs for why.
    fn is_order_sensitive(&self) -> bool {
        !self.script.is_empty()
    }

    /// Start the session: compile, run the script's top level, and hold it open
    /// for the rows that follow.
    fn begin_run(&self, entries: &[FileEntry], run: &RunContext) {
        let Ok(mut state) = self.session.0.lock() else {
            return;
        };
        *state = State::Idle;

        let Some(compiled) = self.compiled_in(Some(run)) else {
            return;
        };
        *state = match &*compiled {
            Err(error) => State::Failed(error.clone()),
            Ok(compiled) => {
                match Session::start(compiled, &self.args, entries, run, SCRIPT_DEADLINE) {
                    Ok(session) => State::Live(Box::new(session)),
                    Err(error) => State::Failed(error),
                }
            }
        };
    }

    /// Close the session by calling `done`, and hand on whatever it asked for.
    ///
    /// Taking the session out rather than borrowing it: `done` runs once per
    /// run, and leaving a spent session behind would let a second `plan` over
    /// the same operation run it twice.
    fn end_run(&self) -> super::RunOutcome {
        let Ok(mut state) = self.session.0.lock() else {
            return super::RunOutcome::default();
        };
        match std::mem::take(&mut *state) {
            State::Live(mut session) => session.finish(),
            // A session that never started has nothing to close. Its failure
            // was already reported on every row.
            State::Idle | State::Failed(_) => super::RunOutcome::default(),
        }
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        let Ok(mut state) = self.session.0.lock() else {
            return Err(OpError::new("script", "script state was poisoned"));
        };
        match &mut *state {
            // No run in progress. `OpKind::problem` lands here every frame, so
            // this must neither run the script nor pretend something is wrong;
            // a genuinely broken script is reported by `problem` below.
            State::Idle => Ok(Cow::Borrowed(subject)),
            State::Failed(error) => Err(OpError::new("script", error)),
            State::Live(session) => match session.rename(cx.index, subject) {
                Err(error) => Err(OpError::new("script", error)),
                // An empty string from the script leaves this file alone.
                Ok(None) => Ok(Cow::Borrowed(subject)),
                Ok(Some(name)) => Ok(Cow::Owned(name)),
            },
        }
    }
}

impl Script {
    /// The card's own complaint, if it has one.
    ///
    /// Separate from [`NameTransform::apply`] because `OpKind::problem` runs
    /// once per card per frame: it may check that the script *compiles*, which
    /// is cached, but it must never *run* one.
    pub fn problem(&self) -> Option<String> {
        match &*self.compiled()? {
            Err(error) => Some(error.to_string()),
            Ok(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::Pipeline;
    use crate::run::{Answers, RunSettings};
    use std::path::Path;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, source: &str) {
        store::forget_all();
        std::fs::write(dir.join(format!("{name}.koto")), source).unwrap();
    }

    fn entries(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|n| FileEntry::synthetic(PathBuf::from("/music").join(n)))
            .collect()
    }

    /// Run a listing through a one-step pipeline, the way `plan` would.
    fn run(op: Script, names: &[&str]) -> Vec<String> {
        let entries = entries(names);
        let pipeline = Pipeline::new().then(op);
        crate::pipeline::evaluate_all(&entries, &pipeline)
            .into_iter()
            .map(|r| match r {
                Ok(ev) => ev.name,
                Err(e) => format!("<{e}>"),
            })
            .collect()
    }

    #[test]
    fn a_script_renames_every_row() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "Upper",
            "rename = || fr.filename.to_uppercase()",
        );
        assert_eq!(
            run(Script::new("Upper").in_dir(dir.path()), &["a.mp3", "b.mp3"]),
            ["A.mp3", "B.mp3"],
            "the extension is outside Scope::Name, so only the stem changes"
        );
    }

    /// A fresh card must not complain, and must not change anything (P34).
    #[test]
    fn an_unconfigured_card_is_a_no_op() {
        let op = Script::default();
        assert_eq!(op.problem(), None);
        assert!(!op.is_order_sensitive());
        assert_eq!(run(op, &["a.txt"]), ["a.txt"]);
    }

    #[test]
    fn a_missing_script_is_named_in_the_complaint() {
        let dir = TempDir::new().unwrap();
        let op = Script::new("Nowhere").in_dir(dir.path());
        let problem = op.problem().expect("a missing script is a problem");
        assert!(problem.contains("Nowhere"), "{problem}");
    }

    /// The card checks that its script compiles, and says so — but a script
    /// that compiles is never *run* by the per-frame check.
    #[test]
    fn a_script_that_does_not_compile_is_reported_by_the_card() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "Broken", "rename = |||( <<");
        let problem = Script::new("Broken")
            .in_dir(dir.path())
            .problem()
            .expect("a broken script is a problem");
        assert!(problem.contains("compile"), "{problem}");
    }

    /// The reason this operation exists in its own right: state carried across
    /// rows, in list order.
    ///
    /// **Not sufficient on its own**, and worth knowing before deleting the one
    /// below as a duplicate. Restoring `par_iter` while leaving `begin_run` in
    /// place leaves *this* test passing — three rows is a small enough workload
    /// that rayon usually happens to run them in order.
    /// `two_evaluations_of_a_stateful_script_agree` is the one that catches it.
    #[test]
    fn state_carries_across_rows_in_list_order() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "Number",
            "s = {n: 0}\nrename = ||\n  s.n += 1\n  '{s.n}-{fr.filename}'",
        );
        assert_eq!(
            run(Script::new("Number").in_dir(dir.path()), &["a", "b", "c"]),
            ["1-a", "2-b", "3-c"]
        );
    }

    /// The property that makes a scripted preview usable at all: run it twice,
    /// get the same answer.
    ///
    /// This is the test that pins the *serial* half of the design, verified by
    /// mutation: put `par_iter` back in `evaluate_all_with` and this is the
    /// only test in the file that fails. Eight rows rather than three, and two
    /// runs rather than one, for exactly that reason.
    #[test]
    fn two_evaluations_of_a_stateful_script_agree() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "Number",
            "s = {n: 0}\nrename = ||\n  s.n += 1\n  '{s.n}'",
        );
        let op = Script::new("Number").in_dir(dir.path());
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let first = run(op.clone(), &names);
        let second = run(op, &names);
        assert_eq!(first, second);
        assert_eq!(first, ["1", "2", "3", "4", "5", "6", "7", "8"]);
    }

    /// A run is a fresh session. Otherwise the counter above would keep
    /// climbing across previews and the second one would disagree.
    #[test]
    fn each_run_starts_a_new_session() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "Once",
            "s = {n: 0}\nrename = ||\n  s.n += 1\n  '{s.n}'",
        );
        let op = Script::new("Once").in_dir(dir.path());
        assert_eq!(run(op.clone(), &["a"]), ["1"]);
        assert_eq!(run(op, &["a"]), ["1"], "the session did not reset");
    }

    /// An error while starting the session belongs on every row, not nowhere.
    #[test]
    fn a_script_that_fails_to_start_reports_on_every_row() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "NoRename", "x = 1");
        let out = run(Script::new("NoRename").in_dir(dir.path()), &["a", "b"]);
        assert!(out.iter().all(|o| o.contains("rename")), "{out:?}");
    }

    #[test]
    fn the_arguments_box_reaches_the_script() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "Prefix", "rename = || fr.args + fr.filename");
        assert_eq!(
            run(
                Script::new("Prefix").in_dir(dir.path()).with_args("x-"),
                &["a"]
            ),
            ["x-a"]
        );
    }

    #[test]
    fn the_summary_says_what_will_run() {
        assert_eq!(Script::default().summary(), "No script chosen");
        assert_eq!(Script::new("Swap").summary(), "Run 'Swap'");
        assert_eq!(
            Script::new("Swap").with_args(" - ").summary(),
            "Run 'Swap' with ' - '"
        );
    }

    /// Editing the script file takes effect, because the compile cache is keyed
    /// on the file's stamp rather than its path alone.
    #[test]
    fn editing_the_script_takes_effect() {
        let dir = TempDir::new().unwrap();
        let op = Script::new("Edit").in_dir(dir.path());
        write(dir.path(), "Edit", "rename = || 'first'");
        assert_eq!(run(op.clone(), &["a"]), ["first"]);
        write(dir.path(), "Edit", "rename = || 'second'");
        assert_eq!(run(op, &["a"]), ["second"]);
    }

    /// The path is deliberately absent from what a preset stores.
    #[test]
    fn a_preset_stores_the_script_name_and_args_and_nothing_else() {
        let op = Script::new("Swap").with_args(" - ").in_dir("/somewhere");
        let text = toml::to_string(&op).unwrap();
        assert!(text.contains("script = \"Swap\""), "{text}");
        assert!(text.contains("args = \" - \""), "{text}");
        assert!(
            !text.contains("somewhere"),
            "a machine's script folder must not travel in a preset:\n{text}"
        );

        let back: Script = toml::from_str(&text).unwrap();
        assert_eq!(back.script, "Swap");
        assert_eq!(back.args, " - ");
    }

    /// The run context reaches the script through the tag engine, so a script
    /// can use `<Counter>` and the rest without the operation knowing about
    /// them.
    #[test]
    fn a_script_can_render_run_level_tags() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "Counted",
            "rename = || fr.format_tags '<Counter>-<Name>'",
        );

        let entries = entries(&["a.txt", "b.txt"]);
        let settings = RunSettings::default();
        let run_cx = RunContext::build(&entries, &settings, Answers::default());
        let pipeline = Pipeline::new().then(Script::new("Counted").in_dir(dir.path()));
        let out: Vec<String> = crate::pipeline::evaluate_all_with(&entries, &pipeline, &run_cx)
            .into_iter()
            .map(|r| r.unwrap().name)
            .collect();
        assert_eq!(out, ["1-a.txt", "2-b.txt"]);
    }
}
