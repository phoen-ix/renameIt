//! Move Section.
//!
//! *"The move function cuts out a section of the
//! filenames and moves it to another position."*
//!
//! The panel has **two independent** "counting backwards" checkboxes — one for
//! `from pos`, one for `paste at` — which is why this operation carries two
//! flags rather than one.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError, byte_of_position, byte_range, char_len};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoveSection {
    /// *"Enter the number of characters you want to cut out of the filenames."*
    pub cut: usize,
    /// *"Enter the position from where to start cutting the characters."*
    pub from_pos: usize,
    pub from_backwards: bool,
    /// *"This is the position where the cut out section is to be inserted into
    /// the filename again. […] Note that the position is calculated after the
    /// cut has taken place."*
    pub to_pos: usize,
    pub to_backwards: bool,
    /// *"Instead of moving to an absolute position, move x steps away from the
    /// position where the original section was cut from."*
    pub relative: bool,
}

impl MoveSection {
    pub fn new(cut: usize, from_pos: usize, to_pos: usize) -> Self {
        Self {
            cut,
            from_pos,
            to_pos,
            ..Default::default()
        }
    }

    pub fn relative(mut self, yes: bool) -> Self {
        self.relative = yes;
        self
    }

    pub fn from_backwards(mut self, yes: bool) -> Self {
        self.from_backwards = yes;
        self
    }

    pub fn to_backwards(mut self, yes: bool) -> Self {
        self.to_backwards = yes;
        self
    }
}

impl NameTransform for MoveSection {
    fn id(&self) -> &'static str {
        "move_section"
    }

    fn summary(&self) -> String {
        format!(
            "Move {} chars from {} to {}{}",
            self.cut,
            self.from_pos,
            self.to_pos,
            if self.relative { " (relative)" } else { "" }
        )
    }

    fn apply<'a>(&self, subject: &'a str, _cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        if self.cut == 0 {
            return Ok(Cow::Borrowed(subject));
        }

        // Cut.
        let cut_byte = byte_of_position(subject, self.from_pos, self.from_backwards);
        let cut_char = subject[..cut_byte].chars().count();
        let range = byte_range(subject, cut_char, self.cut);
        let section = &subject[range.clone()];
        if section.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }
        let mut remainder = String::with_capacity(subject.len() - section.len());
        remainder.push_str(&subject[..range.start]);
        remainder.push_str(&subject[range.end..]);

        // Paste. Both positions are resolved against the post-cut string.
        let remainder_len = char_len(&remainder);
        let insert_char = if self.relative {
            if self.to_backwards {
                cut_char.saturating_sub(self.to_pos)
            } else {
                cut_char.saturating_add(self.to_pos).min(remainder_len)
            }
        } else if self.to_backwards {
            remainder_len.saturating_sub(self.to_pos)
        } else {
            self.to_pos.min(remainder_len)
        };

        let at = super::byte_of_char(&remainder, insert_char);
        let mut out = String::with_capacity(subject.len());
        out.push_str(&remainder[..at]);
        out.push_str(section);
        out.push_str(&remainder[at..]);

        if out == subject {
            Ok(Cow::Borrowed(subject))
        } else {
            Ok(Cow::Owned(out))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::run;
    use super::*;

    /// "cuts out a section of the filenames and moves it to another position"
    #[test]
    fn a_section_is_cut_and_pasted_elsewhere() {
        // "abcdef" -> cut "ab" -> "cdef" -> paste at 2 -> "cdabef"
        assert_eq!(run(&MoveSection::new(2, 0, 2), "abcdef"), "cdabef");
    }

    /// "Note that the position is calculated after the cut has taken place."
    #[test]
    fn the_paste_position_is_calculated_after_the_cut() {
        // Cutting "cd" leaves "abef" (4 chars); pasting at 4 appends.
        assert_eq!(run(&MoveSection::new(2, 2, 4), "abcdef"), "abefcd");
        // Position 6 would be out of range on the shortened string, and
        // saturates to the same place rather than erroring.
        assert_eq!(run(&MoveSection::new(2, 2, 6), "abcdef"), "abefcd");
    }

    #[test]
    fn moving_a_section_to_the_front_is_the_common_case() {
        // "Artist - Title" -> move "Title" to the front.
        assert_eq!(
            run(&MoveSection::new(5, 9, 0), "Artist - Title"),
            "TitleArtist - "
        );
    }

    /// "move x steps away from the position where the original section was cut
    /// from. Again, position is calculated after the cut has taken place."
    #[test]
    fn relative_mode_moves_a_number_of_steps_from_the_original_position() {
        // Cut "ab" at 0, then move 2 steps right of where it was.
        assert_eq!(
            run(&MoveSection::new(2, 0, 2).relative(true), "abcdef"),
            "cdabef"
        );
        // Zero steps puts it straight back.
        assert_eq!(
            run(&MoveSection::new(2, 2, 0).relative(true), "abcdef"),
            "abcdef"
        );
    }

    #[test]
    fn relative_mode_can_move_left_by_counting_backwards() {
        // Cut "ef" at 4, move 2 steps towards the beginning.
        assert_eq!(
            run(
                &MoveSection::new(2, 4, 2).relative(true).to_backwards(true),
                "abcdef"
            ),
            "abefcd"
        );
    }

    /// "the position starts at the end of the filename and "moves" towards the
    /// beginning"
    #[test]
    fn the_two_backwards_flags_are_independent() {
        // Cut 2 chars starting 2 from the end ("ef"), paste at the front.
        assert_eq!(
            run(&MoveSection::new(2, 2, 0).from_backwards(true), "abcdef"),
            "efabcd"
        );
        // Cut from the front, paste 0 from the end.
        assert_eq!(
            run(&MoveSection::new(2, 0, 0).to_backwards(true), "abcdef"),
            "cdefab"
        );
    }

    #[test]
    fn cutting_nothing_changes_nothing() {
        assert_eq!(run(&MoveSection::new(0, 2, 0), "abcdef"), "abcdef");
        // A cut that starts past the end has nothing to move.
        assert_eq!(run(&MoveSection::new(3, 99, 0), "abcdef"), "abcdef");
    }

    #[test]
    fn an_over_long_cut_takes_what_is_there() {
        assert_eq!(run(&MoveSection::new(999, 3, 0), "abcdef"), "defabc");
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        // Four characters, eight bytes: cut "ÜÖ", leaving "ÄÑ", paste at 2.
        assert_eq!(run(&MoveSection::new(2, 0, 2), "ÜÖÄÑ"), "ÄÑÜÖ");
        assert_eq!(run(&MoveSection::new(1, 0, 1), "ÜÖÄÑ"), "ÖÜÄÑ");
    }
}
