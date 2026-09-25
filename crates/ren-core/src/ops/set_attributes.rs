//! Set Attributes — changing a file's DOS attribute bits.
//!
//! Check the box to set the attribute, uncheck it to clear it, and leave it in
//! the greyed middle state to leave the attribute alone.
//!
//! The first operation that changes a file without renaming it — so it is a
//! [`SideEffectAction`], and everything about *how* the change reaches the disk
//! lives in the executor.

use ren_platform::AttributeChange;
use serde::{Deserialize, Serialize};

use super::{EvalCx, OpError, SideEffectAction};
use crate::effect::{Effect, Undoability};

/// Tri-state per bit: `None` is the grey box.
///
/// All four default to grey, which is what a freshly added card must be — an
/// action card that does nothing at all until the user says otherwise (P34).
///
/// The wire keys are `read_only`/`hidden`/`system`/`archive`, mapping one to one
/// onto [`AttributeChange`] and [`ren_platform::Capability`]. The *label* is
/// `Write Protect`, the word a user knows it by; a preset key that disagreed
/// with the enum it feeds would be a bug waiting to be written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetAttributes {
    pub read_only: Option<bool>,
    pub hidden: Option<bool>,
    pub system: Option<bool>,
    pub archive: Option<bool>,
}

impl SetAttributes {
    /// The card's captions, in its order: Write Protect and System on the first
    /// row, Hidden and Archive on the second.
    pub const LABELS: [&'static str; 4] = ["Write Protect", "Hidden", "System", "Archive"];

    pub fn to_change(self) -> AttributeChange {
        AttributeChange {
            read_only: self.read_only,
            hidden: self.hidden,
            system: self.system,
            archive: self.archive,
        }
    }

    /// Every bit paired with its label, for a UI that draws them uniformly.
    pub fn bits(&mut self) -> [(&'static str, &mut Option<bool>); 4] {
        [
            (Self::LABELS[0], &mut self.read_only),
            (Self::LABELS[1], &mut self.hidden),
            (Self::LABELS[2], &mut self.system),
            (Self::LABELS[3], &mut self.archive),
        ]
    }

    pub fn is_empty(self) -> bool {
        self.to_change().is_empty()
    }
}

/// "Write Protect off", "Hidden on, Archive off" — what the row shows and the
/// log records.
fn describe_change(change: AttributeChange) -> String {
    let parts: Vec<String> = [
        (SetAttributes::LABELS[0], change.read_only),
        (SetAttributes::LABELS[1], change.hidden),
        (SetAttributes::LABELS[2], change.system),
        (SetAttributes::LABELS[3], change.archive),
    ]
    .into_iter()
    .filter_map(|(label, bit)| bit.map(|on| format!("{label} {}", if on { "on" } else { "off" })))
    .collect();
    parts.join(", ")
}

impl SideEffectAction for SetAttributes {
    fn id(&self) -> &'static str {
        "set_attributes"
    }

    fn summary(&self) -> String {
        if self.is_empty() {
            // Every box grey: a card that does nothing, and says so.
            return "Set Attributes (all left unchanged)".to_owned();
        }
        describe_change(self.to_change())
    }

    fn effect(&self, _cx: &EvalCx<'_>) -> Result<Option<Effect>, OpError> {
        // The same edit for every file — the operation does not look at one.
        // An all-grey card produces an empty effect, which the pipeline drops.
        Ok(Some(Effect::Attributes(self.to_change())))
    }

    fn describe(&self, effect: &Effect) -> String {
        match effect {
            Effect::Attributes(change) => describe_change(*change),
            // The pipeline only ever hands back what `effect` produced.
            _ => self.summary(),
        }
    }

    /// The executor records the four bits it is about to replace, so undo puts
    /// them back exactly.
    fn undoable(&self) -> Undoability {
        Undoability::Journaled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileEntry;
    use crate::ops::OpKind;

    fn effect_of(op: &SetAttributes) -> Effect {
        let entry = FileEntry::synthetic("/files/a.txt");
        let cx = EvalCx::simple(&entry, 0, 1);
        op.effect(&cx).unwrap().expect("always produces one")
    }

    /// A freshly added card must change nothing at all.
    #[test]
    fn all_four_boxes_start_grey_so_nothing_is_changed() {
        let op = SetAttributes::default();
        assert!(op.is_empty());
        assert!(effect_of(&op).is_empty(), "an empty effect is dropped");
        assert_eq!(op.summary(), "Set Attributes (all left unchanged)");
        assert!(effect_of(&op).required_capabilities().is_empty());
    }

    /// Files copied from a CD arrive write-protected: untick Write Protect and
    /// leave every other box grey.
    #[test]
    fn the_cd_example_clears_write_protect_and_touches_nothing_else() {
        let op = SetAttributes {
            read_only: Some(false),
            ..Default::default()
        };
        assert_eq!(
            effect_of(&op),
            Effect::Attributes(AttributeChange {
                read_only: Some(false),
                ..Default::default()
            })
        );
        assert_eq!(op.summary(), "Write Protect off");
        assert_eq!(
            effect_of(&op).required_capabilities(),
            vec![ren_platform::Capability::ReadOnlyAttribute],
            "and asks the platform for nothing else"
        );
    }

    #[test]
    fn the_labels_are_the_ones_on_the_card() {
        assert_eq!(
            SetAttributes::LABELS,
            ["Write Protect", "Hidden", "System", "Archive"]
        );
        assert_eq!(
            OpKind::SetAttributes(SetAttributes::default()).label(),
            "Set Attributes"
        );
    }

    #[test]
    fn several_bits_are_listed_in_card_order() {
        let op = SetAttributes {
            hidden: Some(true),
            archive: Some(false),
            ..Default::default()
        };
        assert_eq!(op.summary(), "Hidden on, Archive off");
    }

    /// D35's writer strips defaults, so the CD example is one line in a preset.
    #[test]
    fn a_preset_omits_every_grey_box() {
        let op = SetAttributes {
            read_only: Some(false),
            ..Default::default()
        };
        let toml = toml::to_string(&op).unwrap();
        assert!(toml.contains("read_only = false"), "{toml}");
        assert!(!toml.contains("hidden"), "{toml}");

        let back: SetAttributes = toml::from_str(&toml).unwrap();
        assert_eq!(back, op);
    }

    /// It changes a file rather than its name, and the engine has to know.
    #[test]
    fn it_is_an_action_not_a_name_transform() {
        let op = OpKind::SetAttributes(SetAttributes::default());
        assert_eq!(op.produces(), crate::ops::Produces::Action);
        assert!(matches!(op.to_step(), crate::pipeline::Step::Action(_)));
    }
}
