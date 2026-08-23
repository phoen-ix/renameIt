//! Add & Remove.
//!
//! The panel is a radio pair — Add or
//! Remove — plus a checkbox nothing documents but the UI spells out:
//! *"Both at the same time (Remove first, then add)"*.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError, byte_of_position, byte_range, char_len};
use crate::template::TextTemplate;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddRemoveMode {
    #[default]
    Add,
    Remove,
    /// *"Both at the same time (Remove first, then add)"* — and the order in
    /// that label is load-bearing: the add position is resolved against the
    /// **already shortened** name.
    Both,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AddRemove {
    pub mode: AddRemoveMode,

    /// *"Enter the string you wish to add to the filenames."*
    ///
    /// Tags work here, which is not decoration: the shipped "Add suffix to end
    /// of filename" preset is literally `Add '<Ask>' at pos 0 from ending`.
    pub insert: TextTemplate,
    /// *"If you insert xxx into the filename abcdef at position 2 you end up
    /// with abxxxcdef but if you instead enable overwriting you end up with
    /// abxxxf."*
    pub overwrite: bool,
    /// *"The position is zero based, meaning that position 0 points to before
    /// the first character."*
    pub add_pos: usize,
    /// *"position" starts at the end of the filename and "moves" towards the
    /// beginning*
    pub add_backwards: bool,

    /// *"How many characters (letters) do you want to remove from the
    /// filename?"*
    pub delete: usize,
    pub remove_pos: usize,
    pub remove_backwards: bool,
}

impl AddRemove {
    pub fn add(insert: impl Into<TextTemplate>, pos: usize) -> Self {
        Self {
            mode: AddRemoveMode::Add,
            insert: insert.into(),
            add_pos: pos,
            ..Default::default()
        }
    }

    pub fn remove(delete: usize, pos: usize) -> Self {
        Self {
            mode: AddRemoveMode::Remove,
            delete,
            remove_pos: pos,
            ..Default::default()
        }
    }

    pub fn backwards(mut self, yes: bool) -> Self {
        self.add_backwards = yes;
        self.remove_backwards = yes;
        self
    }

    pub fn overwrite(mut self, yes: bool) -> Self {
        self.overwrite = yes;
        self
    }

    /// *"Set "delete" to 999 to remove all characters after the starting
    /// position!"* — which works because an over-long count truncates instead
    /// of erroring.
    fn apply_remove(&self, subject: &str) -> String {
        if self.delete == 0 {
            return subject.to_owned();
        }
        let start_byte = byte_of_position(subject, self.remove_pos, self.remove_backwards);
        let start_char = subject[..start_byte].chars().count();
        let range = byte_range(subject, start_char, self.delete);
        let mut out = String::with_capacity(subject.len());
        out.push_str(&subject[..range.start]);
        out.push_str(&subject[range.end..]);
        out
    }

    /// The text the **Add** half is handed.
    ///
    /// The same string in `Add` mode; in `Both` mode the subject already
    /// shortened by the Remove half, because `apply` resolves `add_pos` against
    /// `apply_remove`'s output — *"Removes first, then adds"*, and the order in
    /// that label is load-bearing.
    ///
    /// Exposed for Visual Assist, which must show the Add strip the string the
    /// caret will actually be measured on. Computing it in the GUI instead
    /// would need a three-way case split with one undefined branch (a caret
    /// inside the removed span), which is exactly the D19 violation the engine
    /// exists to prevent. Giving Add and Remove separate ⌖ buttons solves the
    /// same problem without the split.
    pub fn add_subject<'a>(&self, subject: &'a str) -> Cow<'a, str> {
        match self.mode {
            AddRemoveMode::Both => Cow::Owned(self.apply_remove(subject)),
            AddRemoveMode::Add | AddRemoveMode::Remove => Cow::Borrowed(subject),
        }
    }

    fn apply_add(&self, subject: &str, insert: &str) -> String {
        if insert.is_empty() {
            return subject.to_owned();
        }
        let at = byte_of_position(subject, self.add_pos, self.add_backwards);
        let mut out = String::with_capacity(subject.len() + insert.len());
        out.push_str(&subject[..at]);
        out.push_str(insert);
        if self.overwrite {
            // Overwriting consumes as many characters as were inserted.
            let start_char = subject[..at].chars().count();
            let consumed = char_len(insert);
            let range = byte_range(subject, start_char, consumed);
            out.push_str(&subject[range.end..]);
        } else {
            out.push_str(&subject[at..]);
        }
        out
    }
}

impl NameTransform for AddRemove {
    fn id(&self) -> &'static str {
        "add_remove"
    }

    fn summary(&self) -> String {
        match self.mode {
            AddRemoveMode::Add => format!("Add {:?} at {}", self.insert.as_str(), self.add_pos),
            AddRemoveMode::Remove => {
                format!("Remove {} chars at {}", self.delete, self.remove_pos)
            }
            AddRemoveMode::Both => format!(
                "Remove {} at {} then add {:?} at {}",
                self.delete,
                self.remove_pos,
                self.insert.as_str(),
                self.add_pos
            ),
        }
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        let insert = match self.mode {
            AddRemoveMode::Remove => Cow::Borrowed(""),
            _ => match cx.render(&self.insert)? {
                Some(text) => text,
                // "Only rename if all tags are available".
                None => return Ok(Cow::Borrowed(subject)),
            },
        };
        let out = match self.mode {
            AddRemoveMode::Add => self.apply_add(subject, &insert),
            AddRemoveMode::Remove => self.apply_remove(subject),
            AddRemoveMode::Both => self.apply_add(&self.apply_remove(subject), &insert),
        };
        if out == subject {
            Ok(Cow::Borrowed(subject))
        } else {
            Ok(Cow::Owned(out))
        }
    }

    fn needs(&self) -> crate::template::TagNeeds {
        self.insert.needs()
    }

    fn asks(&self) -> Vec<crate::run::AskSpec> {
        self.insert.asks()
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::run;
    use super::*;

    /// "If you insert xxx into the filename abcdef at position 2 you end up
    /// with abxxxcdef"
    #[test]
    fn inserting_at_a_position_pushes_the_rest_along() {
        assert_eq!(run(&AddRemove::add("xxx", 2), "abcdef"), "abxxxcdef");
    }

    /// "but if you instead enable overwriting you end up with abxxxf"
    #[test]
    fn overwriting_consumes_as_many_characters_as_it_writes() {
        assert_eq!(
            run(&AddRemove::add("xxx", 2).overwrite(true), "abcdef"),
            "abxxxf"
        );
    }

    /// "position 0 points to before the first character"
    #[test]
    fn position_zero_is_before_the_first_character() {
        assert_eq!(run(&AddRemove::add("pre-", 0), "name"), "pre-name");
    }

    /// The shipped "Add suffix to end of filename" preset is
    /// `Add '<Ask>' at pos 0 from ending`.
    #[test]
    fn backwards_position_zero_appends_to_the_end() {
        assert_eq!(
            run(&AddRemove::add("-suffix", 0).backwards(true), "name"),
            "name-suffix"
        );
        assert_eq!(
            run(&AddRemove::add("X", 2).backwards(true), "abcdef"),
            "abcdXef"
        );
    }

    #[test]
    fn an_add_position_past_the_end_appends() {
        assert_eq!(run(&AddRemove::add("!", 99), "abc"), "abc!");
    }

    /// "The Remove function allows you to delete a certain number of characters
    /// from the filenames."
    #[test]
    fn removing_deletes_the_requested_run() {
        assert_eq!(run(&AddRemove::remove(3, 2), "abcdefgh"), "abfgh");
    }

    /// "Set "delete" to 999 to remove all characters after the starting
    /// position!"
    #[test]
    fn a_delete_count_of_999_removes_everything_after_the_position() {
        assert_eq!(run(&AddRemove::remove(999, 3), "abcdefgh"), "abc");
        assert_eq!(run(&AddRemove::remove(999, 0), "abcdefgh"), "");
    }

    #[test]
    fn removing_from_the_end_counts_backwards() {
        assert_eq!(
            run(&AddRemove::remove(2, 3).backwards(true), "abcdef"),
            "abcf"
        );
    }

    #[test]
    fn a_delete_of_zero_changes_nothing() {
        assert_eq!(run(&AddRemove::remove(0, 2), "abcdef"), "abcdef");
    }

    /// "Both at the same time (Remove first, then add)"
    #[test]
    fn both_removes_first_then_adds() {
        let op = AddRemove {
            mode: AddRemoveMode::Both,
            insert: "NEW".into(),
            add_pos: 0,
            delete: 3,
            remove_pos: 0,
            ..Default::default()
        };
        // "abcdef" -> remove 3 at 0 -> "def" -> add "NEW" at 0 -> "NEWdef"
        assert_eq!(run(&op, "abcdef"), "NEWdef");
    }

    #[test]
    fn both_resolves_the_add_position_against_the_shortened_name() {
        let op = AddRemove {
            mode: AddRemoveMode::Both,
            insert: "-".into(),
            add_pos: 3,
            delete: 2,
            remove_pos: 0,
            ..Default::default()
        };
        // "abcdefgh" -> "cdefgh" -> insert at 3 -> "cde-fgh"
        assert_eq!(run(&op, "abcdefgh"), "cde-fgh");
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        // Four characters, eight bytes.
        assert_eq!(run(&AddRemove::add("|", 2), "ÜÖÄÑ"), "ÜÖ|ÄÑ");
        assert_eq!(run(&AddRemove::remove(2, 1), "ÜÖÄÑ"), "ÜÑ");
    }

    /// *"Removes first, then adds"*, so in `Both` mode `add_pos` is measured on
    /// a name that is already shorter than the one on screen.
    ///
    /// Visual Assist has to show the Add half **that** string, or a caret placed
    /// at the visible position 5 lands somewhere else entirely. Asserted against
    /// what `apply` actually does rather than against a hand-written expectation,
    /// so the two cannot drift.
    #[test]
    fn the_add_half_is_handed_the_already_shortened_name() {
        let both = AddRemove {
            mode: AddRemoveMode::Both,
            delete: 2,
            remove_pos: 0,
            insert: "-".into(),
            add_pos: 3,
            ..Default::default()
        };
        assert_eq!(both.add_subject("abcdefgh"), "cdefgh");
        // And that is the string the caret is measured on: position 3 of
        // "cdefgh", not of "abcdefgh".
        assert_eq!(run(&both, "abcdefgh"), "cde-fgh");

        // In the other two modes the Add half sees the subject untouched.
        let add_only = AddRemove::add("-", 3);
        assert_eq!(add_only.add_subject("abcdefgh"), "abcdefgh");
        let remove_only = AddRemove::remove(2, 0);
        assert_eq!(remove_only.add_subject("abcdefgh"), "abcdefgh");
    }
}
