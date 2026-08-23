//! Filename Editor — typing the new names directly, one per line.
//!
//! > *"The filename editor contains a simple text editor that allows you to
//! > manually type in your new filenames. The first line in the text editor
//! > corresponds to the first file in your list, and so on. Besides text you
//! > can also enter `<tags>` here."*
//!
//! The only operation whose validity depends on the *listing* rather than on
//! its own configuration, which is why the line-count check lives in `apply`
//! (where `cx.total` is) rather than anywhere a card can ask about itself.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};
use crate::cache::Cached;
use crate::model::{FileEntry, Scope};
use crate::run::AskSpec;
use crate::template::{TagNeeds, TextTemplate};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilenameEditor {
    /// Exactly what was typed, newlines and all. Stored in the preset itself
    /// (D39) rather than in a stray file beside the settings.
    pub text: String,
    /// One compiled template per line. D21: a clone drops it, which is what
    /// makes typing take effect.
    #[serde(skip)]
    lines: Cached<Vec<TextTemplate>>,
}

impl FilenameEditor {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            lines: Cached::new(),
        }
    }

    /// One template per line.
    ///
    /// `str::lines` is doing real work here: it splits on `\n`, drops a
    /// trailing `\r` so text pasted from Windows pairs correctly, and — the
    /// part that matters — does *not* invent a final empty line from a trailing
    /// newline. Every text box ends in one, and an extra phantom line would
    /// make the count wrong for every user who pressed Enter at the end.
    pub fn lines(&self) -> &[TextTemplate] {
        self.lines
            .get_or_init(|| self.text.lines().map(TextTemplate::new).collect())
    }

    pub fn line_count(&self) -> usize {
        self.lines().len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// What `(copy current filename list into editor)` writes.
    ///
    /// Deliberately the *scoped* slice at the *card's* scope: those are the
    /// names `apply` will be handed, so copying and then running changes
    /// nothing. Writing whole names into a card scoped to the stem would append
    /// every extension twice on the very first run.
    ///
    /// A file the scope does not reach at all — an extensionless file under
    /// `Scope::Extension` (P12) — gets a blank line, which is the same "leave
    /// this one alone" the operation already means by one.
    ///
    /// **One line per file, always.** A name may legally contain a line break —
    /// POSIX forbids only `/` and NUL — and emitting it raw would split one file
    /// across two lines, shifting the pairing for every file after it. The break
    /// is written as a space so the count stays true; [`Self::unrepresentable`]
    /// is what stops the lossy copy being run blindly afterwards.
    pub fn text_for(entries: &[FileEntry], scope: Scope) -> String {
        entries
            .iter()
            .map(|e| {
                scope
                    .slice(&e.file_name)
                    .map_or(String::new(), |s| one_line(s.active()))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The listed names a line-based editor cannot represent.
    ///
    /// Shown by the card beside the copy link, so the user is told *which* file
    /// before they press it rather than after the run is blocked.
    pub fn unrepresentable(entries: &[FileEntry], scope: Scope) -> Vec<String> {
        entries
            .iter()
            .filter(|e| {
                scope
                    .slice(&e.file_name)
                    .is_some_and(|s| has_line_break(s.active()))
            })
            .map(|e| e.file_name.clone())
            .collect()
    }
}

/// Whether a name carries something `str::lines` would split on.
fn has_line_break(name: &str) -> bool {
    name.contains('\n') || name.contains('\r')
}

/// The same name with its breaks flattened to spaces.
fn one_line(name: &str) -> String {
    name.replace(['\n', '\r'], " ")
}

impl NameTransform for FilenameEditor {
    fn id(&self) -> &'static str {
        "filename_editor"
    }

    fn summary(&self) -> String {
        match self.line_count() {
            0 => "Filename Editor (empty)".to_owned(),
            1 => "Name from 1 typed line".to_owned(),
            n => format!("Names from {n} typed lines"),
        }
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        // A freshly added card is a no-op, not a complaint about 0 lines for 5
        // files — the same reading P34 gives Free Format's empty pattern.
        if self.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }

        // M5 pre-decides this, and it is the safe call: lines pair with
        // the listing by position, so a count that does not match means every
        // pairing after the first difference is somebody else's name. Reported
        // on every row, which makes `plan.errors()` non-zero and blocks the run
        // (P4) rather than renaming half the folder wrongly.
        if self.line_count() != cx.total {
            return Err(OpError::new(
                "Filename Editor",
                format!(
                    "the editor has {} line(s) but {} file(s) are listed",
                    self.line_count(),
                    cx.total
                ),
            ));
        }

        // A name carrying a line break cannot round-trip through a line-based
        // editor: `text_for` had to flatten it, so running the copied text back
        // would rename the file to the flattened form — silently, and without
        // the user ever having typed that name. Refused rather than guessed at.
        if has_line_break(subject) {
            return Err(OpError::new(
                "Filename Editor",
                "this name contains a line break, which one line of the editor cannot hold",
            ));
        }

        let Some(line) = self.lines().get(cx.index) else {
            return Ok(Cow::Borrowed(subject));
        };
        // A blank line leaves that file alone. Pasting a list out of a document
        // routinely carries blank separators, and "erase this filename" is not
        // a thing a user can mean.
        if line.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }

        match cx.render(line)? {
            None => Ok(Cow::Borrowed(subject)),
            Some(text) if text == subject => Ok(Cow::Borrowed(subject)),
            Some(text) => Ok(Cow::Owned(text.into_owned())),
        }
    }

    fn needs(&self) -> TagNeeds {
        self.lines()
            .iter()
            .fold(TagNeeds::NONE, |acc, l| acc.union(l.needs()))
    }

    fn asks(&self) -> Vec<AskSpec> {
        let mut out: Vec<AskSpec> = self.lines().iter().flat_map(TextTemplate::asks).collect();
        out.sort_by_key(|a| a.slot);
        out.dedup_by_key(|a| a.slot);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{Evaluation, Pipeline, Step, StepConfig};
    use crate::{OpKind, evaluate_all};

    fn entries(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|n| FileEntry::synthetic(format!("/files/{n}")))
            .collect()
    }

    fn run(text: &str, scope: Scope, names: &[&str]) -> Vec<Result<Evaluation, OpError>> {
        let pipeline = Pipeline::new().with(
            Step::Name(Box::new(FilenameEditor::new(text))),
            StepConfig::scoped(scope),
        );
        evaluate_all(&entries(names), &pipeline)
    }

    fn names(text: &str, scope: Scope, names: &[&str]) -> Vec<String> {
        run(text, scope, names)
            .into_iter()
            .map(|r| r.unwrap().name)
            .collect()
    }

    /// *"The first line in the text editor corresponds to the first file in
    /// your list, and so on."*
    #[test]
    fn the_first_line_corresponds_to_the_first_file_in_your_list() {
        assert_eq!(
            names(
                "First\nSecond\nThird",
                Scope::Name,
                &["a.txt", "b.txt", "c.txt"]
            ),
            ["First.txt", "Second.txt", "Third.txt"]
        );
    }

    /// Process Name ticked and Process Extension clear, so typing a name keeps
    /// the extension.
    #[test]
    fn the_editor_replaces_the_name_and_keeps_the_extension() {
        assert_eq!(names("Song", Scope::Name, &["track01.mp3"]), ["Song.mp3"]);
        assert_eq!(
            OpKind::FilenameEditor(FilenameEditor::default()).default_scope(),
            Scope::Name
        );
    }

    /// M5's acceptance: *"mismatch in line count = validation error"*. The message
    /// has to name both numbers, because "which one is wrong" is the only
    /// question the user has.
    #[test]
    fn a_mismatch_in_line_count_is_a_validation_error() {
        for (text, listed) in [("one\ntwo", 3usize), ("one\ntwo\nthree\nfour", 3)] {
            let files = ["a.txt", "b.txt", "c.txt"];
            let results = run(text, Scope::Name, &files[..listed]);
            assert_eq!(results.len(), listed);
            for result in results {
                let err = result.expect_err("a mismatch must not rename anything");
                let message = err.to_string();
                assert!(
                    message.contains("line(s)") && message.contains("file(s)"),
                    "{message}"
                );
            }
        }
    }

    /// A card the user has only just added must not accuse them of anything.
    #[test]
    fn an_empty_editor_leaves_every_name_alone() {
        assert_eq!(
            names("", Scope::Name, &["a.txt", "b.txt"]),
            ["a.txt", "b.txt"]
        );
        assert_eq!(
            FilenameEditor::default().summary(),
            "Filename Editor (empty)"
        );
        assert_eq!(FilenameEditor::default().line_count(), 0);
    }

    #[test]
    fn a_blank_line_leaves_that_file_alone() {
        assert_eq!(
            names("First\n\nThird", Scope::Name, &["a.txt", "b.txt", "c.txt"]),
            ["First.txt", "b.txt", "Third.txt"]
        );
    }

    /// A text box almost always ends in a newline, and a phantom final line
    /// would make every count wrong by one.
    #[test]
    fn a_trailing_newline_does_not_add_a_line() {
        assert_eq!(FilenameEditor::new("one\ntwo\n").line_count(), 2);
        assert_eq!(FilenameEditor::new("one\ntwo").line_count(), 2);
        assert_eq!(FilenameEditor::new("\n").line_count(), 1, "one blank line");
    }

    #[test]
    fn crlf_line_endings_are_accepted() {
        assert_eq!(
            names("First\r\nSecond", Scope::Name, &["a.txt", "b.txt"]),
            ["First.txt", "Second.txt"],
            "text pasted from Windows must not carry a trailing carriage return"
        );
    }

    /// *"Besides text you can also enter `<tags>` here."*
    #[test]
    fn besides_text_you_can_also_enter_tags_here() {
        assert_eq!(
            names(
                "<Counter> First\n<Counter> Second",
                Scope::Name,
                &["a.txt", "b.txt"]
            ),
            ["1 First.txt", "2 Second.txt"]
        );
        let op = FilenameEditor::new("<Ask-1>\n<Ask-1>\n<Counter>");
        assert!(op.needs().contains(TagNeeds::COUNTER));
        assert_eq!(op.asks().len(), 1, "the same slot twice is one question");
    }

    /// The strongest property this operation has: `(copy current filename list
    /// into editor)` followed by a run must be the identity, at either scope.
    /// If it is not, the copy link is writing the wrong slice.
    #[test]
    fn copying_the_current_list_into_the_editor_and_running_changes_nothing() {
        let files = ["a.txt", "b.txt", "no-extension", "two.dots.txt"];
        for scope in [Scope::Name, Scope::Both] {
            let text = FilenameEditor::text_for(&entries(&files), scope);
            assert_eq!(
                names(&text, scope, &files),
                files,
                "copy-then-run changed something at {scope:?}"
            );
        }
    }

    /// A name may legally contain a line break — POSIX forbids only `/` and
    /// NUL — and a line-based editor cannot hold one.
    ///
    /// Two halves. The copy keeps **one line per file**, because emitting the
    /// break raw would split one file across two lines and shift the pairing
    /// for every file after it — turning a broken name into somebody else's
    /// rename. And running such a row is refused rather than renaming the file
    /// to the flattened form the user never typed.
    #[test]
    fn a_name_containing_a_line_break_is_refused_rather_than_flattened() {
        let files = ["a.txt", "two\nlines.txt", "c.txt"];
        let listed = entries(&files);

        let text = FilenameEditor::text_for(&listed, Scope::Both);
        assert_eq!(
            text.lines().count(),
            3,
            "one line per file, whatever the names contain: {text:?}"
        );
        assert_eq!(text.lines().nth(1), Some("two lines.txt"));

        // And the card can say which file before the user presses copy.
        assert_eq!(
            FilenameEditor::unrepresentable(&listed, Scope::Both),
            ["two\nlines.txt"]
        );

        // Running the copied text back errors on that row instead of renaming
        // it to the flattened form.
        let op = FilenameEditor::new(&text);
        let run = crate::run::RunContext::default();
        let cx = EvalCx::new(&listed[1], 1, 3, &run);
        let err = op.apply("two\nlines.txt", &cx).unwrap_err();
        assert!(format!("{err}").contains("line break"), "{err}");

        // The rows either side are untouched by their neighbour's problem.
        let cx = EvalCx::new(&listed[0], 0, 3, &run);
        assert_eq!(op.apply("a.txt", &cx).unwrap(), "a.txt");
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = FilenameEditor::new("one\ntwo");
        let back: FilenameEditor = toml::from_str(&toml::to_string(&op).unwrap()).unwrap();
        assert_eq!(back, op);
        assert_eq!(back.line_count(), 2);
    }
}
