//! What Visual Assist means, with no widget in sight.
//!
//! The four ⌖ buttons want three different things — a string, a caret, and a
//! span — and getting that wrong is invisible: the run simply
//! renames the wrong characters. So the semantics live here, as pure functions
//! over an operation and a selection, and are tested without a harness.
//!
//! The drawing is `crate::panels::visual_assist`; the state machine is on
//! `RenameItApp`. This file knows only what a selection *does*.

use ren_core::ops::{AddRemoveMode, OpKind, regex_escape};

/// Which field a ⌖ fills. One value per button.
///
/// Four, not "every position/length field" as `docs/DESIGN.md` §S4 proposed:
/// Replace's Skip and Max are match counts, Zero Padding's Digits is a width,
/// and the CSV columns are column numbers — a span picker on those means
/// nothing. Move Section's *paste at* is out for a sharper reason: its
/// position is measured **after** the cut, so there is no text on screen to
/// pick it against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistTarget {
    /// Find & Replace ▸ *Look For*. Lifts the selected **text**.
    ReplaceFind,
    /// Add ▸ *at pos*. A caret: only where the selection starts is used.
    AddPos,
    /// Remove ▸ *from pos* + *Delete*. A span.
    RemoveSection,
    /// Move Section ▸ *from pos* + *Cut*. A span.
    MoveCut,
}

impl AssistTarget {
    /// The line above the field.
    ///
    /// Three variants, one per shape of answer: text, a caret, or a span.
    pub fn prompt(self) -> &'static str {
        match self {
            Self::ReplaceFind => "Select text:",
            Self::AddPos => "Place cursor on desired position:",
            Self::RemoveSection | Self::MoveCut => "Select text to set position and length:",
        }
    }

    /// Whether an empty selection is a usable answer.
    ///
    /// Only for a caret. The other three all have an early-return no-op for the
    /// empty case — `apply_remove` on `delete == 0`, `MoveSection::apply` on
    /// `cut == 0`, `Replace` on an empty `find` — so Select would appear to work
    /// and change nothing, which reads as a broken feature rather than as a
    /// refused one.
    pub fn accepts_empty(self) -> bool {
        matches!(self, Self::AddPos)
    }

    /// The targets this operation offers **right now**.
    ///
    /// Computed from the operation rather than from its kind: an Add & Remove
    /// card in `Remove` mode has one ⌖, in `Both` mode it has two. Remove comes
    /// first because that is the order the editor draws the groups in, and F3
    /// cycles in the order the eye buttons appear.
    pub fn offered_by(op: &OpKind) -> Vec<Self> {
        match op {
            OpKind::Replace(_) => vec![Self::ReplaceFind],
            OpKind::MoveSection(_) => vec![Self::MoveCut],
            OpKind::AddRemove(op) => match op.mode {
                AddRemoveMode::Add => vec![Self::AddPos],
                AddRemoveMode::Remove => vec![Self::RemoveSection],
                AddRemoveMode::Both => vec![Self::RemoveSection, Self::AddPos],
            },
            _ => Vec::new(),
        }
    }
}

/// A selection inside the text an operation is handed.
///
/// **Characters, and from the start of the subject** — not bytes, and not from
/// the start of the file name. Those are the two units every position field in
/// this program already uses (P15), and egui hands back character offsets, so
/// nothing on the path converts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistSpan {
    pub start: usize,
    pub len: usize,
    /// The selected characters. Owned because the subject they were cut from is
    /// gone by the time the app applies this, and because Replace wants the
    /// text while the three position targets want only the numbers.
    pub text: String,
}

impl AssistSpan {
    /// The selection a character range makes in `subject`.
    pub fn cut(subject: &str, start: usize, len: usize) -> Self {
        let text: String = subject.chars().skip(start).take(len).collect();
        Self { start, len, text }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether this selection runs to the end of `subject`.
    ///
    /// The one case where *counting backwards* is the more general answer: a
    /// caret at the end is `pos 0` from the end of **every** name, whatever its
    /// length, which is exactly the form the shipped "Add suffix" preset uses.
    pub fn touches_the_end(&self, subject_chars: usize) -> bool {
        self.start + self.len == subject_chars
    }
}

/// Writes a selection into the field it was raised for.
///
/// Here rather than in the four editors because four copies of "which field
/// does ⌖ fill" is four places for Add and Remove to disagree, and because
/// `super::ui`'s `match op` beside it is the only exhaustive match over
/// `OpKind` in the GUI — so a nineteenth operation that wants Visual Assist is
/// a compile error rather than an omission nobody notices.
///
/// Called by the app's drain rather than during the draw: a value in flight
/// across a frame boundary can land in a card that was deleted, duplicated or
/// moved in between. Returns false when the operation is not the one the target
/// belongs to, which is the app's cue to say so rather than write into whatever
/// is there now.
///
/// **Select unticks *counting backwards*.** Converting the forward span instead
/// would encode *this* file's length into a rule that runs over all of them —
/// right for the file in the strip and wrong for every other. Anchoring to the
/// end is offered separately, as a deliberate click, by [`anchor_to_end`].
pub fn apply_assist(op: &mut OpKind, target: AssistTarget, span: &AssistSpan) -> bool {
    match (op, target) {
        (OpKind::Replace(replace), AssistTarget::ReplaceFind) => {
            replace.find = if replace.regex {
                regex_escape(&span.text)
            } else {
                span.text.clone()
            };
            true
        }
        (OpKind::AddRemove(add_remove), AssistTarget::AddPos) => {
            add_remove.add_pos = span.start;
            add_remove.add_backwards = false;
            true
        }
        (OpKind::AddRemove(add_remove), AssistTarget::RemoveSection) => {
            add_remove.remove_pos = span.start;
            add_remove.delete = span.len;
            add_remove.remove_backwards = false;
            true
        }
        (OpKind::MoveSection(move_section), AssistTarget::MoveCut) => {
            move_section.from_pos = span.start;
            move_section.cut = span.len;
            move_section.from_backwards = false;
            true
        }
        _ => false,
    }
}

/// Re-expresses a position as a distance from the **end** of the name.
///
/// Offered when the selection runs to the end of the subject, because that is
/// the one shape where backwards is not a different way of saying the same
/// thing but a strictly better one: `at pos 0, counting backwards` appends to
/// every name in the listing, while `at pos 17` appends only to the ones that
/// happen to be seventeen characters long.
///
/// A click rather than an automatic rewrite: it changes a checkbox the user can
/// see, and silently reversing someone's own setting is worse than offering it.
pub fn anchor_to_end(op: &mut OpKind, target: AssistTarget, subject_chars: usize) -> bool {
    match (op, target) {
        (OpKind::AddRemove(add_remove), AssistTarget::AddPos) => {
            add_remove.add_pos = subject_chars.saturating_sub(add_remove.add_pos);
            add_remove.add_backwards = true;
            true
        }
        (OpKind::AddRemove(add_remove), AssistTarget::RemoveSection) => {
            add_remove.remove_pos = subject_chars.saturating_sub(add_remove.remove_pos);
            add_remove.remove_backwards = true;
            true
        }
        (OpKind::MoveSection(move_section), AssistTarget::MoveCut) => {
            move_section.from_pos = subject_chars.saturating_sub(move_section.from_pos);
            move_section.from_backwards = true;
            true
        }
        _ => false,
    }
}

/// Wildcard characters in a lifted selection, if any.
///
/// The wildcard language has **no escape**: `wildcard::to_regex` maps `*`, `:`
/// and `?` unconditionally, and `MatchSpec::auto` switches the *Look For* box
/// into that language the moment one appears. So unlike the regex case there is
/// nothing to sanitise — the honest move is to say what will happen and offer
/// the one box that can express the literal, which the strip does before
/// Select (`visual_assist::wildcard_note`).
///
/// All three are Windows-reserved characters, so this is reachable only on
/// Linux and macOS.
pub fn wildcards_in(text: &str) -> Vec<char> {
    let mut found: Vec<char> = "*:?".chars().filter(|c| text.contains(*c)).collect();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use ren_core::ops::{AddRemove, MoveSection, Replace};

    fn add_remove(mode: AddRemoveMode) -> OpKind {
        OpKind::AddRemove(AddRemove {
            mode,
            ..Default::default()
        })
    }

    /// `IMG_2024_holiday`, with the `2024` selected.
    fn span() -> AssistSpan {
        AssistSpan::cut("IMG_2024_holiday", 4, 4)
    }

    #[test]
    fn a_span_carries_the_characters_it_covers() {
        let span = span();
        assert_eq!(span.start, 4);
        assert_eq!(span.len, 4);
        assert_eq!(span.text, "2024");
    }

    /// Characters, not bytes — the whole feature is wrong by however many
    /// multi-byte characters precede the selection otherwise.
    #[test]
    fn a_span_is_cut_in_characters_not_bytes() {
        let span = AssistSpan::cut("Ünïcödé Trâck", 0, 7);
        assert_eq!(span.text, "Ünïcödé");
        assert_eq!(span.len, 7);
    }

    #[test]
    fn remove_writes_the_length_as_well_as_the_position() {
        let mut op = add_remove(AddRemoveMode::Remove);
        assert!(apply_assist(&mut op, AssistTarget::RemoveSection, &span()));
        let OpKind::AddRemove(op) = &op else {
            panic!("{op:?}")
        };
        assert_eq!(op.remove_pos, 4);
        assert_eq!(op.delete, 4);
    }

    /// A caret uses where the selection *starts* and nothing else: the prompt
    /// says "place cursor on desired position", not "mark a section".
    #[test]
    fn add_takes_the_position_and_not_the_length() {
        let mut op = add_remove(AddRemoveMode::Add);
        assert!(apply_assist(&mut op, AssistTarget::AddPos, &span()));
        let OpKind::AddRemove(op) = &op else {
            panic!("{op:?}")
        };
        assert_eq!(op.add_pos, 4);
        assert_eq!(op.delete, 0, "a caret removes nothing");
    }

    #[test]
    fn move_writes_the_cut_and_where_it_starts() {
        let mut op = OpKind::MoveSection(MoveSection::default());
        assert!(apply_assist(&mut op, AssistTarget::MoveCut, &span()));
        let OpKind::MoveSection(op) = &op else {
            panic!("{op:?}")
        };
        assert_eq!(op.from_pos, 4);
        assert_eq!(op.cut, 4);
        assert_eq!(op.to_pos, 0, "the paste position is not assisted");
    }

    /// The position the user picked reads forward, so the box that would read
    /// it backwards has to come off. Converting instead would bake this one
    /// file's length into a rule that runs over the whole listing.
    #[test]
    fn select_unticks_counting_backwards() {
        for (mut op, target) in [
            (
                add_remove(AddRemoveMode::Remove),
                AssistTarget::RemoveSection,
            ),
            (add_remove(AddRemoveMode::Add), AssistTarget::AddPos),
            (
                OpKind::MoveSection(MoveSection::default()),
                AssistTarget::MoveCut,
            ),
        ] {
            match &mut op {
                OpKind::AddRemove(a) => {
                    a.add_backwards = true;
                    a.remove_backwards = true;
                }
                OpKind::MoveSection(m) => m.from_backwards = true,
                _ => unreachable!(),
            }
            assert!(apply_assist(&mut op, target, &span()));
            match &op {
                OpKind::AddRemove(a) if target == AssistTarget::AddPos => {
                    assert!(!a.add_backwards);
                }
                OpKind::AddRemove(a) => assert!(!a.remove_backwards),
                OpKind::MoveSection(m) => assert!(!m.from_backwards),
                _ => unreachable!(),
            }
        }
    }

    /// The one shape where backwards is the *more* general answer, and it is
    /// literally the form the shipped "Add suffix to end of filename" preset
    /// uses: `Add '<Ask>' at pos 0 from ending`.
    #[test]
    fn a_selection_at_the_end_can_be_anchored_to_it() {
        let subject = "IMG_2024_holiday";
        let chars = subject.chars().count();
        let caret = AssistSpan::cut(subject, chars, 0);
        assert!(caret.touches_the_end(chars));

        let mut op = add_remove(AddRemoveMode::Add);
        apply_assist(&mut op, AssistTarget::AddPos, &caret);
        let OpKind::AddRemove(before) = &op else {
            panic!()
        };
        assert_eq!((before.add_pos, before.add_backwards), (16, false));

        assert!(anchor_to_end(&mut op, AssistTarget::AddPos, chars));
        let OpKind::AddRemove(after) = &op else {
            panic!()
        };
        assert_eq!(
            (after.add_pos, after.add_backwards),
            (0, true),
            "position 0 from the end appends to every name, not just this one"
        );
    }

    /// A selection that does not reach the end is not an anchor candidate.
    #[test]
    fn a_selection_in_the_middle_is_not_at_the_end() {
        assert!(!span().touches_the_end("IMG_2024_holiday".chars().count()));
    }

    /// D115: what the data said is sanitised. The box is a *pattern* when the
    /// checkbox is ticked, and `\(1\)` is the pattern that means the literal
    /// `(1)` the user pointed at.
    #[test]
    fn a_regex_find_box_gets_the_escaped_literal() {
        let picked = AssistSpan::cut("photo (1).jpg", 6, 3);
        assert_eq!(picked.text, "(1)");

        let mut plain = OpKind::Replace(Replace::default());
        apply_assist(&mut plain, AssistTarget::ReplaceFind, &picked);
        let OpKind::Replace(op) = &plain else {
            panic!()
        };
        assert_eq!(op.find, "(1)", "a literal box takes the literal");

        let mut regex = OpKind::Replace(Replace::new("", "").regex(true));
        apply_assist(&mut regex, AssistTarget::ReplaceFind, &picked);
        let OpKind::Replace(op) = &regex else {
            panic!()
        };
        assert_eq!(op.find, r"\(1\)", "a pattern box takes the pattern");
    }

    /// The wildcard language has no escape at all, so the only honest answer is
    /// to name the characters and let the user decide.
    #[test]
    fn wildcards_in_a_lifted_name_are_named_because_they_cannot_be_escaped() {
        assert_eq!(wildcards_in("track 1?.mp3"), vec!['?']);
        assert_eq!(wildcards_in("a*b:c?d"), vec!['*', ':', '?']);
        assert!(wildcards_in("photo (1).jpg").is_empty());
    }

    /// A mismatched pair must change nothing at all, or a card that was
    /// duplicated or deleted between the click and the drain gets written into.
    #[test]
    fn a_target_that_does_not_belong_to_the_operation_writes_nothing() {
        let mut op = OpKind::Replace(Replace::new("keep", "me"));
        assert!(!apply_assist(&mut op, AssistTarget::RemoveSection, &span()));
        let OpKind::Replace(op) = &op else { panic!() };
        assert_eq!(op.find, "keep", "untouched");
    }

    /// One ⌖ in Add or Remove mode, two in Both — and Remove first, because
    /// that is the order the card draws them and the order F3 cycles them.
    #[test]
    fn which_targets_a_card_offers_follows_its_mode() {
        assert_eq!(
            AssistTarget::offered_by(&add_remove(AddRemoveMode::Add)),
            [AssistTarget::AddPos]
        );
        assert_eq!(
            AssistTarget::offered_by(&add_remove(AddRemoveMode::Remove)),
            [AssistTarget::RemoveSection]
        );
        assert_eq!(
            AssistTarget::offered_by(&add_remove(AddRemoveMode::Both)),
            [AssistTarget::RemoveSection, AssistTarget::AddPos],
            "the Remove group is drawn first"
        );
        assert_eq!(
            AssistTarget::offered_by(&OpKind::Replace(Replace::default())),
            [AssistTarget::ReplaceFind]
        );
        assert!(
            AssistTarget::offered_by(&OpKind::Casing(Default::default())).is_empty(),
            "Casing offers no target"
        );
    }

    /// Three of the four do nothing at all with an empty selection, so Select
    /// must refuse rather than appear to work.
    #[test]
    fn only_a_caret_accepts_an_empty_selection() {
        assert!(AssistTarget::AddPos.accepts_empty());
        for target in [
            AssistTarget::ReplaceFind,
            AssistTarget::RemoveSection,
            AssistTarget::MoveCut,
        ] {
            assert!(!target.accepts_empty(), "{target:?}");
        }
    }

    /// Every target says something, and the three strings are distinct.
    #[test]
    fn every_target_has_a_prompt() {
        assert_eq!(AssistTarget::ReplaceFind.prompt(), "Select text:");
        assert_eq!(
            AssistTarget::AddPos.prompt(),
            "Place cursor on desired position:"
        );
        assert_eq!(
            AssistTarget::RemoveSection.prompt(),
            "Select text to set position and length:"
        );
        assert_eq!(
            AssistTarget::MoveCut.prompt(),
            AssistTarget::RemoveSection.prompt()
        );
    }
}
