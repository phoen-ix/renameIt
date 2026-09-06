//! Free Format — building a name from nothing but tags and literal text.
//!
//! > *"Free Format is a function that allows you to create your filenames from
//! > scratch by using `<tags>`. A tag is a special command that is replaced by
//! > unique information for each file when renaming. […] In addition to tags
//! > you can also type any string you like in between the tags."*
//!
//! The thinnest operation in the tree — M3's template engine does all the work
//! — and the one with the sharpest edges, because it replaces the *whole* slice
//! rather than editing it. Three of them are handled below.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};
use crate::run::AskSpec;
use crate::template::{TagNeeds, TextTemplate};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FreeFormat {
    /// The whole name, as a template. `<Parent>_<FullName>` is the documented
    /// worked example.
    pub pattern: TextTemplate,
}

impl FreeFormat {
    pub fn new(pattern: impl Into<TextTemplate>) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }
}

impl NameTransform for FreeFormat {
    fn id(&self) -> &'static str {
        "free_format"
    }

    fn summary(&self) -> String {
        if self.pattern.is_empty() {
            "Free Format (no pattern yet)".to_owned()
        } else {
            format!("Name as {:?}", self.pattern.as_str())
        }
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        // A card the user has only just added has an empty pattern. Without
        // this it would rename `song.mp3` to `.mp3` before the first keystroke
        // landed — and with a single file in the list, nothing would stop it:
        // duplicate-target detection only catches the multi-file case. Replace
        // short-circuits an empty Find box for the same reason.
        if self.pattern.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }

        match cx.render(&self.pattern)? {
            // A tag this file could not fill in leaves the name alone (P32).
            // For an operation that rewrites everything, the alternative is a
            // name made of the gaps between the tags.
            None => Ok(Cow::Borrowed(subject)),
            Some(text) if text == subject => Ok(Cow::Borrowed(subject)),
            Some(text) => Ok(Cow::Owned(text.into_owned())),
        }
    }

    fn needs(&self) -> TagNeeds {
        self.pattern.needs()
    }

    fn asks(&self) -> Vec<AskSpec> {
        self.pattern.asks()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, Scope};
    use crate::pipeline::{Pipeline, StepConfig};
    use crate::run::{Answers, RunContext, RunSettings};
    use crate::{OpKind, evaluate_all};

    /// Runs the op the way the app does: as a pipeline step, so the engine's
    /// scoping is part of the test rather than bypassed by it.
    fn rename(pattern: &str, path: &str) -> String {
        let entry = FileEntry::synthetic(path);
        let pipeline = Pipeline::new().with(
            crate::pipeline::Step::Name(Box::new(FreeFormat::new(pattern))),
            StepConfig::for_op(&OpKind::FreeFormat(FreeFormat::default())),
        );
        evaluate_all(std::slice::from_ref(&entry), &pipeline)
            .pop()
            .unwrap()
            .unwrap()
            .name
    }

    /// The worked example, whole: a webcam writes one folder per day
    /// and starts numbering again inside each, so the folder name has to come
    /// into the filename before they can share a directory.
    ///
    /// > *"The format string could be `<PARENT>_<FULLNAME>`."*
    #[test]
    fn the_webcam_example_prefixes_each_file_with_its_folder() {
        assert_eq!(
            rename("<PARENT>_<FULLNAME>", "/photos/May09/0001.jpg"),
            "May09_0001.jpg"
        );
        assert_eq!(
            rename("<PARENT>_<FULLNAME>", "/photos/May10/0003.jpg"),
            "May10_0003.jpg"
        );
    }

    /// The same example is why Free Format processes the extension by default.
    /// Under the ambient `Scope::Name` the engine would put `.jpg` back after a
    /// pattern that already ended in it.
    #[test]
    fn free_format_does_not_double_the_extension() {
        let doubled = Pipeline::new().with(
            crate::pipeline::Step::Name(Box::new(FreeFormat::new("<PARENT>_<FULLNAME>"))),
            StepConfig::scoped(Scope::Name),
        );
        let entry = FileEntry::synthetic("/photos/May09/0001.jpg");
        assert_eq!(
            evaluate_all(std::slice::from_ref(&entry), &doubled)[0]
                .as_ref()
                .unwrap()
                .name,
            "May09_0001.jpg.jpg",
            "this is what the wrong default would produce"
        );

        // Which is why the palette and the job-file parser start it at Both.
        assert_eq!(
            OpKind::FreeFormat(FreeFormat::default()).default_scope(),
            Scope::Both
        );
    }

    /// *"Using the Free Format function, simply type in `Playlist` and rename!"*
    /// A pattern with no tags at all is a perfectly good pattern.
    #[test]
    fn a_pattern_with_no_tags_is_a_constant_name() {
        assert_eq!(rename("Playlist", "/music/rock/list.m3u"), "Playlist");
    }

    /// A freshly added card must not touch a single name.
    ///
    /// One file, deliberately: with two, duplicate-target detection would block
    /// the run and hide the bug.
    #[test]
    fn an_empty_pattern_leaves_the_name_alone() {
        assert_eq!(rename("", "/music/song.mp3"), "song.mp3");
        assert_eq!(
            FreeFormat::default().summary(),
            "Free Format (no pattern yet)"
        );
    }

    /// `<Ask>` is answered once per run, before evaluation (D28) — so during
    /// the preview it has no value yet. Rendering it as empty would replace
    /// every name with the gaps between the tags, and one shipped preset is
    /// exactly `Format: <Ask> <Counter>`.
    #[test]
    fn an_unanswered_ask_leaves_the_name_alone() {
        let entries: Vec<FileEntry> = ["/a/one.txt", "/a/two.txt"]
            .iter()
            .map(FileEntry::synthetic)
            .collect();
        let op = FreeFormat::new("<Ask> <Counter>");

        let unanswered = RunContext::build(&entries, &RunSettings::default(), Answers::default());
        for (index, entry) in entries.iter().enumerate() {
            let cx = EvalCx::new(entry, index, entries.len(), &unanswered);
            assert_eq!(
                op.apply(&entry.file_name, &cx).unwrap(),
                entry.file_name.as_str(),
                "an unanswered <Ask> must not rename anything"
            );
        }

        let mut answers = Answers::default();
        answers.asks.insert(0, "Holiday".into());
        let answered = RunContext::build(&entries, &RunSettings::default(), answers);
        let cx = EvalCx::new(&entries[0], 0, entries.len(), &answered);
        assert_eq!(op.apply(&entries[0].file_name, &cx).unwrap(), "Holiday 1");
    }

    /// D29: a typo is an error, never a silently empty name.
    #[test]
    fn a_mistyped_tag_is_an_error() {
        let entry = FileEntry::synthetic("/a/one.txt");
        let cx = EvalCx::simple(&entry, 0, 1);
        assert!(FreeFormat::new("<Nmae>").apply("one.txt", &cx).is_err());
    }

    #[test]
    fn a_pattern_that_reproduces_the_name_borrows_instead_of_allocating() {
        let entry = FileEntry::synthetic("/a/one.txt");
        let cx = EvalCx::simple(&entry, 0, 1);
        let out = FreeFormat::new("<FullName>").apply("one.txt", &cx).unwrap();
        assert!(matches!(out, Cow::Borrowed(_)), "{out:?}");
    }

    #[test]
    fn it_declares_what_its_pattern_needs() {
        let op = FreeFormat::new("<Counter>-<Crc32>");
        assert!(op.needs().contains(TagNeeds::COUNTER));
        assert!(op.needs().contains(TagNeeds::FILE_CONTENT));
        assert!(op.asks().is_empty());

        let asking = FreeFormat::new("<Ask-3>");
        assert_eq!(asking.asks().len(), 1);
        assert_eq!(asking.asks()[0].slot, 3);
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = FreeFormat::new("<%1> - <%2>");
        let back: FreeFormat = toml::from_str(&toml::to_string(&op).unwrap()).unwrap();
        assert_eq!(back, op);
    }
}
