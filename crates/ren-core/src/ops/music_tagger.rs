//! Music Tagger — writing tags *from* the filename, the inverse of Music
//! Rename.
//!
//! The typical case: your mp3s are named `artist - album - title`. Setup Parts
//! takes `<%1> - <%2> - <%3>`, which loads each `<%n>` tag with one part of the
//! filename, and those tags are then the input to each field here.
//!
//! Each field is an enable checkbox plus a tag-accepting box, and the enable is
//! exactly what `Option` means here: a field whose box is clear is not written,
//! and whatever the file already carries is left intact.
//!
//! **This function has no undo** (P2), so it is refused by the engine unless
//! the caller has said in as many words that it may run (D52).
//!
//! The split that makes it safe: `effect` resolves the templates and nothing
//! else — it runs in the parallel preview pass, and `OpKind::problem()` calls
//! it every frame for every card against a synthetic path — while the
//! read-modify-write that keeps the *unenabled* fields intact belongs to the
//! executor, in [`crate::meta::write`].

use serde::{Deserialize, Serialize};

use super::{EvalCx, OpError, SideEffectAction};
use crate::effect::{Effect, Skipped, Undoability};
use crate::meta::write::{FieldWrite, MusicField};
use crate::run::AskSpec;
use crate::template::{TagNeeds, TextTemplate};

/// One field per box, `None` when its checkbox is clear.
///
/// Seven `Option`s rather than a map, because this is what a preset reads as:
/// a job file says `artist = "<%1>"` and says nothing at all about the fields
/// it is not writing. All seven default to `None` — every box clear, which is
/// what P34 wants from a freshly added card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MusicTagger {
    pub track: Option<TextTemplate>,
    pub title: Option<TextTemplate>,
    pub artist: Option<TextTemplate>,
    pub album: Option<TextTemplate>,
    pub year: Option<TextTemplate>,
    pub genre: Option<TextTemplate>,
    pub comment: Option<TextTemplate>,
}

impl MusicTagger {
    /// The style the Quick Setup menu offers, as a set of field mappings.
    ///
    /// Quick Setup is the *same* three styles Music Rename lists, and on this
    /// tab they run the other way: picking `<Artist> - <Title>` means "my
    /// filenames look like that", so it sets Parts to `<%1> - <%2>` and points
    /// Artist at `<%1>` and Title at `<%2>`.
    pub fn from_style(style: &str) -> Option<(String, Self)> {
        let mut tagger = Self::default();
        let mut parts = String::new();
        let mut slot = 0usize;
        let mut rest = style;

        while let Some(open) = rest.find('<') {
            let close = rest[open..].find('>')? + open;
            parts.push_str(&rest[..open]);
            let tag = &rest[open + 1..close];
            slot += 1;
            if slot > 9 {
                return None;
            }
            parts.push_str(&format!("<%{slot}>"));
            let field = TextTemplate::new(format!("<%{slot}>"));
            match tag.to_ascii_lowercase().as_str() {
                "artist" => tagger.artist = Some(field),
                "title" => tagger.title = Some(field),
                "album" => tagger.album = Some(field),
                "track" | "trackn" => tagger.track = Some(field),
                "year" => tagger.year = Some(field),
                "genre" => tagger.genre = Some(field),
                "comment" => tagger.comment = Some(field),
                _ => return None,
            }
            rest = &rest[close + 1..];
        }
        parts.push_str(rest);
        Some((parts, tagger))
    }

    /// Every box paired with its field, in the card's top-to-bottom order, so
    /// the UI and the engine cannot disagree about it.
    pub fn boxes(&mut self) -> [(MusicField, &mut Option<TextTemplate>); 7] {
        [
            (MusicField::Track, &mut self.track),
            (MusicField::Title, &mut self.title),
            (MusicField::Artist, &mut self.artist),
            (MusicField::Album, &mut self.album),
            (MusicField::Year, &mut self.year),
            (MusicField::Genre, &mut self.genre),
            (MusicField::Comment, &mut self.comment),
        ]
    }

    fn enabled(&self) -> Vec<(MusicField, &TextTemplate)> {
        [
            (MusicField::Track, &self.track),
            (MusicField::Title, &self.title),
            (MusicField::Artist, &self.artist),
            (MusicField::Album, &self.album),
            (MusicField::Year, &self.year),
            (MusicField::Genre, &self.genre),
            (MusicField::Comment, &self.comment),
        ]
        .into_iter()
        .filter_map(|(field, slot)| slot.as_ref().map(|t| (field, t)))
        .collect()
    }
}

impl SideEffectAction for MusicTagger {
    fn id(&self) -> &'static str {
        "music_tagger"
    }

    fn summary(&self) -> String {
        let on = self.enabled();
        if on.is_empty() {
            return "Music Tagger (no fields enabled)".to_owned();
        }
        let names: Vec<&str> = on.iter().map(|(field, _)| field.label()).collect();
        format!("Write {}", names.join(", "))
    }

    fn effect(&self, cx: &EvalCx<'_>) -> Result<Option<Effect>, OpError> {
        let on = self.enabled();
        if on.is_empty() {
            return Ok(None);
        }

        // Before the extension gate, not after. `OpKind::problem()` — which
        // is what puts D29's "that is not a tag" under the editor as you type
        // — evaluates against a synthetic `/example/name.txt`, and a gate that
        // ran first would swallow the error for every operation whose files
        // are music. Compiling is cached, so this costs a branch.
        for (_, template) in &on {
            template
                .compiled()
                .map_err(|e| OpError::new("Music Tagger", e.to_string()))?;
        }

        // A cheap gate, on the name rather than the contents: this runs once
        // per file per keystroke and must not open anything. A file the list
        // includes but this operation cannot touch is a row left alone (P45),
        // not an error — a folder of 500 tracks with three text files in it
        // has to run.
        if !crate::meta::folder::has_extension(
            &cx.entry.path,
            &crate::meta::audio::AUDIO_EXTENSIONS,
        ) {
            return Ok(None);
        }

        let mut fields = Vec::new();
        let mut skipped = Vec::new();
        for (field, template) in on {
            // `None` is P33's unanswered `<Ask>` or the run-wide *Only rename
            // if all tags are available* switch. Either way this file is left
            // alone entirely rather than written with a gap in it.
            let Some(value) = cx.render(template)? else {
                return Ok(None);
            };
            let value = value.trim();
            // An enabled box whose template rendered to nothing writes nothing.
            // lofty's own behaviour for an empty value is to *drop* the field,
            // so "enabled but empty" would silently delete what it was meant to
            // leave alone. Not reported: an empty box is a card the user is
            // still filling in, not a value that was rejected (P34).
            if value.is_empty() {
                continue;
            }
            // The one value that is checked rather than taken. lofty writes a
            // literal `0` over a real track number when handed something it
            // cannot parse, so a vinyl rip's `A1` would zero a whole album.
            if field == MusicField::Track && !field.writable(value) {
                skipped.push(Skipped {
                    field,
                    why: format!("{value:?} is not a track number"),
                });
                continue;
            }
            fields.push(FieldWrite {
                field,
                value: value.to_owned(),
            });
        }

        if fields.is_empty() && skipped.is_empty() {
            return Ok(None);
        }
        Ok(Some(Effect::WriteTags { fields, skipped }))
    }

    fn describe(&self, effect: &Effect) -> String {
        let Effect::WriteTags { fields, skipped } = effect else {
            return self.summary();
        };
        let mut parts: Vec<String> = fields
            .iter()
            .map(|f| format!("{} → {}", f.field.label(), f.value))
            .collect();
        // After what will happen, so the row leads with the change and still
        // says what it declined to do.
        parts.extend(
            skipped
                .iter()
                .map(|s| format!("{} skipped, {}", s.field.label(), s.why)),
        );
        parts.join(", ")
    }

    fn undoable(&self) -> Undoability {
        Undoability::None
    }

    fn needs(&self) -> TagNeeds {
        self.enabled()
            .iter()
            .fold(TagNeeds::NONE, |acc, (_, t)| acc.union(t.needs()))
    }

    fn asks(&self) -> Vec<AskSpec> {
        self.enabled().iter().flat_map(|(_, t)| t.asks()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::testing::Mp3;
    use crate::model::FileEntry;
    use crate::run::{Answers, RunContext, RunSettings};
    use tempfile::TempDir;

    fn tagger() -> MusicTagger {
        MusicTagger {
            artist: Some(TextTemplate::new("<%1>")),
            title: Some(TextTemplate::new("<%2>")),
            ..Default::default()
        }
    }

    fn effect_for(op: &MusicTagger, name: &str, parts: &str) -> Option<Effect> {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("Old", "Old").write(dir.path(), name);
        let entry = FileEntry::synthetic(&path);
        let settings = RunSettings {
            parts: crate::PartsSpec::new(parts),
            ..Default::default()
        };
        let run = RunContext::build(std::slice::from_ref(&entry), &settings, Answers::default());
        let cx = EvalCx::new(&entry, 0, 1, &run);
        op.effect(&cx).unwrap()
    }

    /// Setup Parts mapping a name onto fields, as a preview.
    #[test]
    fn the_parts_example_maps_a_filename_onto_fields() {
        let effect = effect_for(&tagger(), "Metallica - One.mp3", "<%1> - <%2>")
            .expect("something to write");
        let Effect::WriteTags { fields, .. } = &effect else {
            panic!("wrong effect: {effect:?}");
        };
        // In the card's order, which puts Title above Artist.
        assert_eq!(
            fields,
            &[
                FieldWrite {
                    field: MusicField::Title,
                    value: "One".into()
                },
                FieldWrite {
                    field: MusicField::Artist,
                    value: "Metallica".into()
                },
            ]
        );
        assert_eq!(
            tagger().describe(&effect),
            "Title → One, Artist → Metallica"
        );
    }

    /// P34: a freshly added card does nothing at all.
    #[test]
    fn a_card_with_no_boxes_ticked_does_nothing() {
        let op = MusicTagger::default();
        assert_eq!(effect_for(&op, "a.mp3", ""), None);
        assert_eq!(op.summary(), "Music Tagger (no fields enabled)");
        assert!(crate::OpKind::MusicTagger(Box::new(op)).problem().is_none());
    }

    /// It writes what is inside a file, so it has to be a file it can open.
    /// A row left alone rather than an error, or a folder with a `cover.jpg`
    /// in it stops the run (P45).
    #[test]
    fn a_file_that_is_not_music_is_left_alone_rather_than_failing_the_run() {
        assert_eq!(effect_for(&tagger(), "cover.jpg", "<%1> - <%2>"), None);
        assert_eq!(effect_for(&tagger(), "notes.txt", "<%1> - <%2>"), None);
    }

    /// An enabled box whose pattern produced nothing must not clear the field
    /// — lofty drops a field written with an empty value, so this is the
    /// difference between "left intact" and "deleted".
    #[test]
    fn an_enabled_field_that_renders_to_nothing_is_skipped_not_cleared() {
        // The separator is there, so `<%1>` fills and `<%2>` is empty — as
        // opposed to a name the pattern misses entirely, which fills neither.
        let effect = effect_for(&tagger(), "Metallica - .mp3", "<%1> - <%2>");
        let Some(Effect::WriteTags { fields, .. }) = effect else {
            panic!("expected a partial write, got {effect:?}");
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field, MusicField::Artist);
    }

    /// And the run-wide switch turns that into "leave the file entirely alone".
    #[test]
    fn only_if_all_tags_are_available_leaves_the_whole_file_alone() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("Old", "Old").write(dir.path(), "Metallica.mp3");
        let entry = FileEntry::synthetic(&path);
        let settings = RunSettings {
            parts: crate::PartsSpec::new("<%1> - <%2>"),
            require_all_tags: true,
            ..Default::default()
        };
        let run = RunContext::build(std::slice::from_ref(&entry), &settings, Answers::default());
        let cx = EvalCx::new(&entry, 0, 1, &run);
        assert_eq!(tagger().effect(&cx).unwrap(), None);
    }

    /// P2, as the engine carries it.
    #[test]
    fn it_reports_itself_as_impossible_to_undo() {
        assert_eq!(tagger().undoable(), Undoability::None);
        let effect = effect_for(&tagger(), "A - B.mp3", "<%1> - <%2>").unwrap();
        assert_eq!(effect.undoability(), Undoability::None);
    }

    /// Quick Setup: the same three styles Music Rename offers, read backwards.
    #[test]
    fn quick_setup_turns_a_style_into_parts_and_field_mappings() {
        let (parts, op) = MusicTagger::from_style("<Artist> - <Title>").unwrap();
        assert_eq!(parts, "<%1> - <%2>");
        assert_eq!(op.artist.as_ref().unwrap().as_str(), "<%1>");
        assert_eq!(op.title.as_ref().unwrap().as_str(), "<%2>");
        assert!(op.album.is_none());

        let (parts, op) = MusicTagger::from_style("<Track>. <Artist> - <Title>").unwrap();
        assert_eq!(parts, "<%1>. <%2> - <%3>");
        assert_eq!(op.track.as_ref().unwrap().as_str(), "<%1>");
        assert_eq!(op.artist.as_ref().unwrap().as_str(), "<%2>");
        assert_eq!(op.title.as_ref().unwrap().as_str(), "<%3>");

        // Every shipped style has to work, or the menu offers a dead entry.
        for style in crate::ops::music_rename::SHIPPED_STYLES {
            assert!(MusicTagger::from_style(style).is_some(), "{style}");
        }
        // And a style naming something that is not a writable field does not.
        assert_eq!(MusicTagger::from_style("<Bitrate> - <Title>"), None);
    }

    /// Tag fields are ordinary templates, so what they need has to reach the
    /// run — or an `<Ask>` in a tagger field never prompts.
    #[test]
    fn it_declares_what_its_fields_need_and_ask() {
        let op = MusicTagger {
            artist: Some(TextTemplate::new("<Ask-3>")),
            title: Some(TextTemplate::new("<Counter>")),
            ..Default::default()
        };
        assert!(op.needs().contains(TagNeeds::ASK));
        assert!(op.needs().contains(TagNeeds::COUNTER));
        assert_eq!(op.asks().len(), 1);
        assert_eq!(op.asks()[0].slot, 3);
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = tagger();
        let text = toml::to_string(&op).unwrap();
        assert!(text.contains("artist"), "{text}");
        assert!(
            !text.contains("album"),
            "an unticked box should say nothing: {text}"
        );
        assert_eq!(toml::from_str::<MusicTagger>(&text).unwrap(), op);
    }

    /// D29: a typo is an error, not a silently empty field.
    #[test]
    fn a_mistyped_tag_is_an_error() {
        let op = MusicTagger {
            artist: Some(TextTemplate::new("<Artsit>")),
            ..Default::default()
        };
        assert!(crate::OpKind::MusicTagger(Box::new(op)).problem().is_some());
    }
}

#[cfg(test)]
mod skipped_tests {
    use super::*;
    use crate::PartsSpec;
    use crate::meta::testing::Mp3;
    use crate::model::FileEntry;
    use crate::run::{Answers, RunContext, RunSettings};
    use tempfile::TempDir;

    /// P5: never silently fail to make a change the user asked for.
    ///
    /// A Track mapped to a part that turns out to be `A1` cannot be written —
    /// lofty would put a literal `0` in its place. Skipping it is right;
    /// skipping it *quietly* is not, because if Track is the only enabled field
    /// the row would show nothing at all and the user would conclude the run
    /// worked.
    #[test]
    fn a_field_that_cannot_be_written_says_so_in_the_preview() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B").write(dir.path(), "A1 - One.mp3");
        let entry = FileEntry::synthetic(&path);
        let settings = RunSettings {
            parts: PartsSpec::new("<%1> - <%2>"),
            ..Default::default()
        };
        let run = RunContext::build(std::slice::from_ref(&entry), &settings, Answers::default());
        let cx = EvalCx::new(&entry, 0, 1, &run);

        let op = MusicTagger {
            track: Some(TextTemplate::new("<%1>")),
            ..Default::default()
        };
        let effect = op
            .effect(&cx)
            .unwrap()
            .expect("a row with something to say");
        let Effect::WriteTags { fields, skipped } = &effect else {
            panic!("wrong effect: {effect:?}");
        };
        assert!(fields.is_empty(), "nothing should be written");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].field, MusicField::Track);
        assert_eq!(
            op.describe(&effect),
            "Track skipped, \"A1\" is not a track number"
        );

        // And it survives into the plan, rather than being dropped as an empty
        // effect the way a card with nothing enabled is.
        assert!(!effect.is_empty());
    }

    /// The mixed case: what can be written is, what cannot is reported, and the
    /// row says both.
    #[test]
    fn a_partly_writable_row_reports_both_halves() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B").write(dir.path(), "A1 - One.mp3");
        let entry = FileEntry::synthetic(&path);
        let settings = RunSettings {
            parts: PartsSpec::new("<%1> - <%2>"),
            ..Default::default()
        };
        let run = RunContext::build(std::slice::from_ref(&entry), &settings, Answers::default());
        let cx = EvalCx::new(&entry, 0, 1, &run);

        let op = MusicTagger {
            track: Some(TextTemplate::new("<%1>")),
            title: Some(TextTemplate::new("<%2>")),
            ..Default::default()
        };
        let effect = op.effect(&cx).unwrap().unwrap();
        assert_eq!(
            op.describe(&effect),
            "Title → One, Track skipped, \"A1\" is not a track number"
        );
    }
}
