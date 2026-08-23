//! Space Trimming.
//!
//! Space Trimming, plus the panel's shipped defaults —
//! everything on, Maintain Space Before `([{` and After `)]};,`.
//!
//! The options say nothing about what order they run in, and the order decides
//! the answer. Ours is fixed and documented below.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::casing::shipped_rules;
use super::{EvalCx, NameTransform, OpError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpaceTrim {
    /// *"Removes spaces before the first character in the filename."*
    pub leading: bool,
    /// *"Removes spaces after the last character in the filename."*
    pub trailing: bool,
    /// *"If several spaces are found next to each other they will be replaced
    /// by a single one."*
    pub shrink: bool,
    /// *"If needed, insert a space character right before the specified
    /// characters."* Empty disables it.
    pub maintain_before: String,
    /// *"If needed, insert a space character right after the specified
    /// characters."* Empty disables it.
    pub maintain_after: String,
    /// *"Replace underscore by space. Self explanatory."*
    pub underscores_to_spaces: bool,
}

impl Default for SpaceTrim {
    /// The panel ships with every box ticked.
    fn default() -> Self {
        let space = &shipped_rules().space;
        Self {
            leading: true,
            trailing: true,
            shrink: true,
            maintain_before: space.maintain_before.clone(),
            maintain_after: space.maintain_after.clone(),
            underscores_to_spaces: true,
        }
    }
}

impl SpaceTrim {
    /// Only the three trim options, with nothing inserted.
    pub fn trim_only() -> Self {
        Self {
            maintain_before: String::new(),
            maintain_after: String::new(),
            underscores_to_spaces: false,
            ..Default::default()
        }
    }
}

impl NameTransform for SpaceTrim {
    fn id(&self) -> &'static str {
        "space_trim"
    }

    fn summary(&self) -> String {
        "Trim spaces".to_owned()
    }

    fn apply<'a>(&self, subject: &'a str, _cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        // The stage order is fixed, and it is the only one that composes:
        //
        //   1. underscores -> spaces, so the spaces they become are visible to
        //      every later stage;
        //   2. insert maintained spaces, which is what *adds* spaces;
        //   3. shrink runs, so a maintained space next to an existing one does
        //      not leave a double;
        //   4. trim ends last, so a maintained space inserted at the very edge
        //      is still removed.
        //
        // Any other order leaves observable double spaces.
        let mut out = String::with_capacity(subject.len() + 8);

        for c in subject.chars() {
            if self.underscores_to_spaces && c == '_' {
                out.push(' ');
            } else {
                out.push(c);
            }
        }

        if !self.maintain_before.is_empty() || !self.maintain_after.is_empty() {
            let mut spaced = String::with_capacity(out.len() + 8);
            let mut chars = out.chars().peekable();
            while let Some(c) = chars.next() {
                // Decided from what has actually been **written**, not from the
                // source character before it. Tracking the source could not see
                // a space the after-rule had just inserted, so `a)(b` with both
                // rules armed came out `a)  (b` — two spaces — and only the
                // shrink stage hid it. With shrink unticked, which is a
                // user-facing checkbox, the doubles were visible.
                if self.maintain_before.contains(c) && !spaced.is_empty() && !spaced.ends_with(' ')
                {
                    spaced.push(' ');
                }
                spaced.push(c);
                // And look *ahead* for the mirror case: a character already
                // followed by a space must not gain a second one. At the end of
                // the name there is nothing to separate from, so nothing is
                // added — the trim stage used to clean that up, but only when
                // trimming was on.
                if self.maintain_after.contains(c) && chars.peek().is_some_and(|next| *next != ' ')
                {
                    spaced.push(' ');
                }
            }
            out = spaced;
        }

        if self.shrink {
            let mut shrunk = String::with_capacity(out.len());
            let mut last_was_space = false;
            for c in out.chars() {
                if c == ' ' {
                    if !last_was_space {
                        shrunk.push(c);
                    }
                    last_was_space = true;
                } else {
                    shrunk.push(c);
                    last_was_space = false;
                }
            }
            out = shrunk;
        }

        let trimmed = match (self.leading, self.trailing) {
            (true, true) => out.trim_matches(' '),
            (true, false) => out.trim_start_matches(' '),
            (false, true) => out.trim_end_matches(' '),
            (false, false) => out.as_str(),
        };

        if trimmed == subject {
            Ok(Cow::Borrowed(subject))
        } else {
            Ok(Cow::Owned(trimmed.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::run;
    use super::*;

    /// "Removes spaces before the first character in the filename."
    #[test]
    fn leading_spaces_are_removed() {
        let op = SpaceTrim {
            trailing: false,
            shrink: false,
            ..SpaceTrim::trim_only()
        };
        assert_eq!(run(&op, "   name  "), "name  ");
    }

    /// "Removes spaces after the last character in the filename."
    #[test]
    fn trailing_spaces_are_removed() {
        let op = SpaceTrim {
            leading: false,
            shrink: false,
            ..SpaceTrim::trim_only()
        };
        assert_eq!(run(&op, "   name  "), "   name");
    }

    /// "If several spaces are found next to each other they will be replaced by
    /// a single one."
    #[test]
    fn multiple_spaces_shrink_into_one() {
        let op = SpaceTrim {
            leading: false,
            trailing: false,
            ..SpaceTrim::trim_only()
        };
        assert_eq!(run(&op, "a    b   c"), "a b c");
    }

    /// "Replace underscore by space. Self explanatory."
    #[test]
    fn underscores_become_spaces() {
        let op = SpaceTrim {
            maintain_before: String::new(),
            maintain_after: String::new(),
            ..Default::default()
        };
        assert_eq!(run(&op, "my_holiday_photo"), "my holiday photo");
    }

    /// "Maintain space asserts that certain characters always are surrounded by
    /// spaces."
    #[test]
    fn maintain_space_inserts_before_the_listed_characters() {
        let op = SpaceTrim {
            maintain_after: String::new(),
            underscores_to_spaces: false,
            ..Default::default()
        };
        assert_eq!(run(&op, "song(live)"), "song (live)");
        assert_eq!(run(&op, "song [demo]"), "song [demo]", "already spaced");
    }

    #[test]
    fn maintain_space_inserts_after_the_listed_characters() {
        let op = SpaceTrim {
            maintain_before: String::new(),
            underscores_to_spaces: false,
            ..Default::default()
        };
        assert_eq!(run(&op, "a,b;c"), "a, b; c");
        assert_eq!(run(&op, "(live)song"), "(live) song");
    }

    /// The shipped defaults, applied to a genuinely messy name.
    #[test]
    fn the_shipped_defaults_clean_up_a_messy_name() {
        let op = SpaceTrim::default();
        assert_eq!(op.maintain_before, "([{");
        assert_eq!(op.maintain_after, ")]};,");
        assert_eq!(run(&op, "  my_song(live)  "), "my song (live)");
    }

    /// The stage order is observable: a maintained space at the very end must
    /// still be trimmed, and one next to an existing space must not double up.
    #[test]
    fn inserted_spaces_are_shrunk_and_trimmed_afterwards() {
        let op = SpaceTrim::default();
        // ";" is in the *after* list, so this inserts a trailing space — which
        // the trim stage, running later, removes again.
        assert_eq!(run(&op, "name;"), "name;");
        // A maintained space next to an existing one must not double up.
        assert_eq!(run(&op, "a ,  b"), "a , b");
        assert_eq!(run(&op, "a(b"), "a (b");
    }

    /// The same guarantee **without shrink**, which is the case that was
    /// broken and the case nothing tested.
    ///
    /// Shrink is a checkbox, not a law: untick it and every maintained space
    /// used to double up wherever the before- and after-rules met, because the
    /// pass decided from the source character rather than from what it had
    /// written. `a)(b` came out `a)  (b`. Asserted here with the rules armed and
    /// every other stage off, so nothing can hide it again.
    #[test]
    fn maintained_spaces_never_double_up_even_with_shrink_off() {
        let op = SpaceTrim {
            shrink: false,
            leading: false,
            trailing: false,
            underscores_to_spaces: false,
            ..SpaceTrim::default()
        };
        // The two rules meeting: `)` wants a space after, `(` one before.
        assert_eq!(run(&op, "a)(b"), "a) (b");
        assert_eq!(run(&op, "song,(live)"), "song, (live)");
        // The mirror: a character already followed by a space gains nothing.
        assert_eq!(run(&op, "a) b"), "a) b");
        assert_eq!(run(&op, "a, b"), "a, b");
        // And one that genuinely needs the space still gets exactly one.
        assert_eq!(run(&op, "a,b"), "a, b");
    }

    #[test]
    fn a_period_is_deliberately_not_in_the_maintain_after_default() {
        // Otherwise every version number and processed extension would gain a
        // space.
        assert_eq!(run(&SpaceTrim::default(), "v1.2.3"), "v1.2.3");
    }

    #[test]
    fn a_name_that_needs_nothing_is_left_alone() {
        assert_eq!(run(&SpaceTrim::default(), "already clean"), "already clean");
    }

    #[test]
    fn everything_disabled_is_the_identity() {
        let op = SpaceTrim {
            leading: false,
            trailing: false,
            shrink: false,
            maintain_before: String::new(),
            maintain_after: String::new(),
            underscores_to_spaces: false,
        };
        assert_eq!(run(&op, "  a_b  ,c  "), "  a_b  ,c  ");
    }
}
