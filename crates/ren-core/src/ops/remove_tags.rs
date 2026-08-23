//! Remove Tags — stripping tag blocks out of music files.
//!
//! Remove Tags, whose group box reads **Remove Mp3
//! Tags** and whose one instruction is *"Remove these tags, if present:"*
//! over three independent checkboxes.
//!
//! > *"You may want to remove it if it for example contains garbage data that
//! > messes up your mp3-player display. And if you are about to write new tags,
//! > it may be a good idea to remove the old ones first."*
//!
//! **This function has no undo** (P2). Unlike Music Tagger it does not even
//! keep the values it destroys, so there is nothing a future version could
//! restore from — which is why the engine refuses it outright unless the caller
//! has said it may run (D52).
//!
//! *"if present"* is the whole error model: a file without the block, and a
//! file whose format cannot carry it, are both rows left alone.

use serde::{Deserialize, Serialize};

use super::{EvalCx, OpError, SideEffectAction};
use crate::effect::{Effect, Undoability};
use crate::meta::write::TagKind;
use crate::run::AskSpec;

/// The three checkboxes.
///
/// **All three default off.** Absence of a key in a job file must never mean
/// "yes" in the one operation that cannot be taken back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoveTags {
    pub id3v1: bool,
    pub id3v2: bool,
    /// *"Lyrics v1 & v2"* — one box for both, as the dialog has it.
    pub lyrics: bool,
}

impl RemoveTags {
    /// Every box paired with its kind, in the dialog's own order.
    pub fn boxes(&mut self) -> [(TagKind, &mut bool); 3] {
        [
            (TagKind::Id3v1, &mut self.id3v1),
            (TagKind::Id3v2, &mut self.id3v2),
            (TagKind::Lyrics3, &mut self.lyrics),
        ]
    }

    fn kinds(&self) -> Vec<TagKind> {
        [
            (TagKind::Id3v1, self.id3v1),
            (TagKind::Id3v2, self.id3v2),
            (TagKind::Lyrics3, self.lyrics),
        ]
        .into_iter()
        .filter_map(|(kind, on)| on.then_some(kind))
        .collect()
    }
}

impl SideEffectAction for RemoveTags {
    fn id(&self) -> &'static str {
        "remove_tags"
    }

    fn summary(&self) -> String {
        let kinds = self.kinds();
        if kinds.is_empty() {
            return "Remove Tags (nothing selected)".to_owned();
        }
        let names: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
        format!("Remove {}", names.join(", "))
    }

    fn effect(&self, cx: &EvalCx<'_>) -> Result<Option<Effect>, OpError> {
        let kinds = self.kinds();
        if kinds.is_empty() {
            return Ok(None);
        }
        // On the name, not the contents — this runs once per file per keystroke
        // and must not open anything. Whether the *format* can carry each block
        // is settled by the executor, which is the only place that knows.
        if !crate::meta::folder::has_extension(
            &cx.entry.path,
            &crate::meta::audio::AUDIO_EXTENSIONS,
        ) {
            return Ok(None);
        }
        Ok(Some(Effect::RemoveTags { kinds }))
    }

    fn describe(&self, effect: &Effect) -> String {
        let Effect::RemoveTags { kinds } = effect else {
            return self.summary();
        };
        format!(
            "Remove {}, if present",
            kinds
                .iter()
                .map(|k| k.label())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn undoable(&self) -> Undoability {
        Undoability::None
    }

    fn asks(&self) -> Vec<AskSpec> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::testing::Mp3;
    use crate::model::FileEntry;
    use tempfile::TempDir;

    fn effect_for(op: &RemoveTags, name: &str) -> Option<Effect> {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("A", "B").write(dir.path(), name);
        let entry = FileEntry::synthetic(&path);
        let cx = EvalCx::simple(&entry, 0, 1);
        op.effect(&cx).unwrap()
    }

    /// The default has to be "remove nothing", and it has to be the *serde*
    /// default too: a job file that says `op = "remove_tags"` and nothing else
    /// must strip nothing at all.
    #[test]
    fn nothing_is_removed_by_default_including_from_a_bare_job_file() {
        let op = RemoveTags::default();
        assert_eq!(effect_for(&op, "a.mp3"), None);
        assert_eq!(op.summary(), "Remove Tags (nothing selected)");
        assert_eq!(toml::from_str::<RemoveTags>("").unwrap(), op);
        assert!(crate::OpKind::RemoveTags(op).problem().is_none());
    }

    #[test]
    fn each_box_reaches_the_effect_in_the_dialogs_order() {
        let op = RemoveTags {
            id3v1: true,
            id3v2: true,
            lyrics: true,
        };
        let effect = effect_for(&op, "a.mp3").expect("something to do");
        assert_eq!(
            effect,
            Effect::RemoveTags {
                kinds: vec![TagKind::Id3v1, TagKind::Id3v2, TagKind::Lyrics3]
            }
        );
        assert_eq!(
            op.describe(&effect),
            "Remove ID3v1, ID3v2, Lyrics v1 & v2, if present"
        );

        let one = RemoveTags {
            lyrics: true,
            ..Default::default()
        };
        assert_eq!(
            effect_for(&one, "a.mp3"),
            Some(Effect::RemoveTags {
                kinds: vec![TagKind::Lyrics3]
            })
        );
    }

    /// A folder of music with a cover and a playlist in it still runs.
    #[test]
    fn a_file_that_is_not_music_is_left_alone() {
        let op = RemoveTags {
            id3v2: true,
            ..Default::default()
        };
        assert_eq!(effect_for(&op, "cover.jpg"), None);
        assert_eq!(effect_for(&op, "album.m3u"), None);
        assert!(effect_for(&op, "track.mp3").is_some());
    }

    #[test]
    fn it_reports_itself_as_impossible_to_undo() {
        let op = RemoveTags {
            id3v2: true,
            ..Default::default()
        };
        assert_eq!(op.undoable(), Undoability::None);
        assert_eq!(
            effect_for(&op, "a.mp3").unwrap().undoability(),
            Undoability::None
        );
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = RemoveTags {
            id3v1: true,
            lyrics: true,
            ..Default::default()
        };
        let text = toml::to_string(&op).unwrap();
        assert_eq!(toml::from_str::<RemoveTags>(&text).unwrap(), op);
    }
}
