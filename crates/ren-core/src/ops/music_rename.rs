//! Music Rename — a filename built from what the file says about itself.
//!
//! > *"The music renaming function allows you to extract information from music
//! > files and rename the file according to a user defined pattern."*
//!
//! The UI offers three predefined styles and a Custom box, but the operation
//! stores only the **pattern**. That is deliberate: a preset that said "style 2"
//! would mean something different on a machine whose styles had been edited,
//! and D35 wants a preset to be self-contained. The radios are a picker over a
//! string, not a fourth kind of state.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};
use crate::run::AskSpec;
use crate::template::{TagNeeds, TextTemplate};

/// The three styles the dialog offers, shipped as our own data (D6).
pub const SHIPPED_STYLES: [&str; 3] = [
    "<Artist> - <Title>",
    "<Track>. <Title>",
    "<Track>. <Artist> - <Title>",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MusicRename {
    /// The pattern, whether it came from a radio button or the Custom box.
    pub style: TextTemplate,
}

impl Default for MusicRename {
    fn default() -> Self {
        Self {
            style: TextTemplate::new(SHIPPED_STYLES[0]),
        }
    }
}

impl MusicRename {
    pub fn new(style: impl Into<TextTemplate>) -> Self {
        Self {
            style: style.into(),
        }
    }
}

impl NameTransform for MusicRename {
    fn id(&self) -> &'static str {
        "music_rename"
    }

    fn summary(&self) -> String {
        if self.style.is_empty() {
            return "Music Rename (no pattern yet)".to_owned();
        }
        format!("Name as {:?}", self.style.as_str())
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        // A card the user has only just added, or has emptied. P34's reading.
        if self.style.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }

        let rendered = self
            .style
            .render(cx)
            .map_err(|e| OpError::new("Music Rename", e))?;

        // P33: an unanswered `<Ask>` is not empty text.
        if rendered.awaiting_input {
            return Ok(Cow::Borrowed(subject));
        }
        // The run-wide *"only rename if all tags are available"* switch.
        if cx.run.require_all_tags && !rendered.available() {
            return Ok(Cow::Borrowed(subject));
        }

        // And the case that switch does not cover, which matters far more here
        // than anywhere else: a file with **no** tags at all. Every tag in the
        // style is missing, so the whole name collapses to its separators —
        // `<Artist> - <Title>` becomes `" - "` — and every untagged file in the
        // folder collapses to the *same* separators. P4 would then block the
        // run for duplicate targets, which is safe but tells the user nothing
        // about why.
        //
        // Leaving the file alone says the true thing instead: it has no tags to
        // rename from. A file with *some* tags still renames, because that is
        // what the user asked for and `require_all_tags` is how they say
        // otherwise.
        if !rendered.missing.is_empty() && rendered.missing.len() == self.style.tag_count() {
            return Ok(Cow::Borrowed(subject));
        }

        if rendered.text == subject {
            return Ok(Cow::Borrowed(subject));
        }
        Ok(Cow::Owned(rendered.text))
    }

    fn needs(&self) -> TagNeeds {
        self.style.needs()
    }

    fn asks(&self) -> Vec<AskSpec> {
        self.style.asks()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::testing::Mp3;
    use crate::model::{FileEntry, Scope};
    use crate::pipeline::{Pipeline, Step, StepConfig};
    use crate::run::{Answers, RunContext, RunSettings};
    use crate::{OpKind, evaluate_all};
    use tempfile::TempDir;

    /// Runs the op as a pipeline step, so the engine's scoping is part of the
    /// test rather than bypassed by it.
    fn rename(style: &str, files: &[(&str, Mp3)]) -> Vec<String> {
        rename_with(style, files, RunSettings::default())
    }

    fn rename_with(style: &str, files: &[(&str, Mp3)], settings: RunSettings) -> Vec<String> {
        let dir = TempDir::new().unwrap();
        let entries: Vec<FileEntry> = files
            .iter()
            .map(|(name, mp3)| FileEntry::synthetic(mp3.write(dir.path(), name)))
            .collect();
        crate::meta::audio::forget_all();

        let pipeline = Pipeline::new().with(
            Step::Name(Box::new(MusicRename::new(style))),
            StepConfig::for_op(&OpKind::MusicRename(MusicRename::default())),
        );
        let run = RunContext::build(&entries, &settings, Answers::default());
        crate::pipeline::evaluate_all_with(&entries, &pipeline, &run)
            .into_iter()
            .map(|r| r.unwrap().name)
            .collect()
    }

    /// The default style, over an ordinary tagged file.
    #[test]
    fn the_first_predefined_style_is_artist_dash_title() {
        assert_eq!(SHIPPED_STYLES[0], "<Artist> - <Title>");
        assert_eq!(
            rename(
                SHIPPED_STYLES[0],
                &[(
                    "track.mp3",
                    Mp3::tagged("Metallica", "Nothing Else Matters")
                )]
            ),
            ["Metallica - Nothing Else Matters.mp3"]
        );
    }

    /// The other two styles, and the zero-padding `<Track>` applies.
    #[test]
    fn the_other_predefined_styles_number_the_tracks() {
        let file = ("t.mp3", Mp3::tagged("Metallica", "One").frame("TRCK", "4"));
        assert_eq!(
            rename(SHIPPED_STYLES[1], std::slice::from_ref(&file)),
            ["04. One.mp3"]
        );
        assert_eq!(
            rename(SHIPPED_STYLES[2], std::slice::from_ref(&file)),
            ["04. Metallica - One.mp3"]
        );
    }

    /// The extension is **not** in scope, and that is the opposite of Free
    /// Format's default for a reason worth pinning.
    ///
    /// D36 gave Free Format `Scope::Both` because its documented example is
    /// `<Parent>_<FullName>`, which already carries the extension — scoping to
    /// the name would append it twice. A music style carries no extension at
    /// all, so `Both` would destroy it: Process Name ticked, Process
    /// Extension clear.
    #[test]
    fn the_extension_survives_because_it_is_not_in_scope() {
        assert_eq!(
            OpKind::MusicRename(MusicRename::default()).default_scope(),
            Scope::Name
        );
        assert_eq!(
            rename(SHIPPED_STYLES[0], &[("t.mp3", Mp3::tagged("A", "B"))]),
            ["A - B.mp3"]
        );
    }

    /// The worked example for organising a collection:
    ///
    /// > *"By using the SubFolder `<\>` tag you can move your files into
    /// > subfolders named after different properties of the file. […] enter
    /// > this format string: `<ARTIST><\><ALBUM><\><TITLE>`"*
    #[test]
    fn the_subfolder_example_sorts_a_collection() {
        let mp3 = Mp3::tagged("Metallica", "One").frame("TALB", "And Justice For All");
        assert_eq!(
            rename("<ARTIST><\\><ALBUM><\\><TITLE>", &[("t.mp3", mp3)]),
            ["Metallica/And Justice For All/One.mp3"]
        );
    }

    /// A file with **no** tags is left alone rather than renamed to its own
    /// separators. Without this every untagged file in a folder collapses to
    /// `" - "`, they all collide, and P4 blocks the run with a message about
    /// duplicate targets that says nothing about the actual problem.
    #[test]
    fn a_file_with_no_tags_at_all_is_left_alone() {
        let untagged = || Mp3 {
            audio: true,
            ..Default::default()
        };
        assert_eq!(
            rename(
                SHIPPED_STYLES[0],
                &[("a.mp3", untagged()), ("b.mp3", untagged())]
            ),
            ["a.mp3", "b.mp3"],
            "two untagged files must not both become ' - .mp3'"
        );
    }

    /// But a file with *some* tags still renames, because that is what was
    /// asked for — `require_all_tags` is how a user says otherwise.
    #[test]
    fn a_partly_tagged_file_renames_unless_the_run_says_not_to() {
        let partial = || Mp3 {
            id3v2: vec![("TPE1", "Metallica".into())],
            audio: true,
            ..Default::default()
        };
        assert_eq!(
            rename(SHIPPED_STYLES[0], &[("t.mp3", partial())]),
            ["Metallica - .mp3"]
        );

        let strict = RunSettings {
            require_all_tags: true,
            ..Default::default()
        };
        assert_eq!(
            rename_with(SHIPPED_STYLES[0], &[("t.mp3", partial())], strict),
            ["t.mp3"],
            "\"only rename if all tags are available\" leaves it alone"
        );
    }

    /// A freshly added card must not blank every name before the first
    /// keystroke (P34).
    #[test]
    fn an_empty_pattern_leaves_the_name_alone() {
        assert_eq!(rename("", &[("t.mp3", Mp3::tagged("A", "B"))]), ["t.mp3"]);
        assert_eq!(
            MusicRename::new("").summary(),
            "Music Rename (no pattern yet)"
        );
    }

    /// The style is an ordinary template, so everything else in the engine is
    /// available in it — and what it needs has to reach the pipeline, or an
    /// `<Ask>` in a custom style never prompts and the run drops other cards'
    /// answers.
    #[test]
    fn a_custom_style_declares_what_it_needs_and_asks() {
        let op = MusicRename::new("<Counter> <Artist> <Ask-2>");
        assert!(op.needs().contains(TagNeeds::COUNTER));
        assert!(op.needs().contains(TagNeeds::FILE_CONTENT));
        assert!(op.needs().contains(TagNeeds::ASK));
        assert_eq!(op.asks().len(), 1);
        assert_eq!(op.asks()[0].slot, 2);
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = MusicRename::new("<Track>. <Title>");
        let back: MusicRename = toml::from_str(&toml::to_string(&op).unwrap()).unwrap();
        assert_eq!(back, op);
    }

    /// D29: a typo is an error, never a silently empty name.
    #[test]
    fn a_mistyped_tag_is_an_error() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B").write(dir.path(), "t.mp3");
        let entry = FileEntry::synthetic(path);
        let cx = EvalCx::simple(&entry, 0, 1);
        assert!(MusicRename::new("<Artsit>").apply("t", &cx).is_err());
    }

    /// A folder speaks through the first music file inside it.
    #[test]
    fn a_folder_is_named_from_the_first_track_inside_it() {
        let dir = TempDir::new().unwrap();
        let album = dir.path().join("album");
        std::fs::create_dir(&album).unwrap();
        Mp3::tagged("Metallica", "One")
            .frame("TALB", "And Justice For All")
            .write(&album, "01.mp3");
        crate::meta::audio::forget_all();

        let mut entry = FileEntry::synthetic(&album);
        entry.is_dir = true;
        let pipeline = Pipeline::new().with(
            Step::Name(Box::new(MusicRename::new("<Artist> - <Album>"))),
            StepConfig::scoped(Scope::Name),
        );
        assert_eq!(
            evaluate_all(std::slice::from_ref(&entry), &pipeline)[0]
                .as_ref()
                .unwrap()
                .name,
            "Metallica - And Justice For All"
        );
    }
}
