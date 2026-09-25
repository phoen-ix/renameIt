//! Space Trimming, with the panel's shipped defaults — everything on, Maintain
//! Space Before `([{` and After `)]};,`.
//!
//! The options say nothing about what order they run in, and the order decides
//! the answer. Ours is fixed (P18) and explained in `apply`.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::casing::shipped_space;
use super::{EvalCx, NameTransform, OpError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpaceTrim {
    /// Remove spaces before the first character.
    pub leading: bool,
    /// Remove spaces after the last character.
    pub trailing: bool,
    /// Collapse a run of spaces into one.
    pub shrink: bool,
    /// Make sure a space comes before each of these characters — unless the
    /// name starts with it, or it follows another of them (`((`). Empty
    /// disables it.
    pub maintain_before: String,
    /// Make sure a space comes after each of these characters — unless the
    /// name ends with it, or what follows is another of them or punctuation
    /// (`),`). Empty disables it.
    pub maintain_after: String,
    /// Turn every underscore into a space.
    pub underscores_to_spaces: bool,
}

impl Default for SpaceTrim {
    /// The panel ships with every box ticked.
    fn default() -> Self {
        let space = shipped_space();
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
        //
        // A name none of the stages would touch is answered without building
        // it: this operation is in the shipped cleanup preset with every
        // stage on, and over a folder that is already clean it allocated four
        // strings per file per keystroke to hand back `Borrowed`.
        if self.cannot_change(subject) {
            return Ok(Cow::Borrowed(subject));
        }
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
                //
                // Two openers in a row are one unit, `((`, not `( (`.
                if self.maintain_before.contains(c)
                    && !spaced.is_empty()
                    && !spaced.ends_with(' ')
                    && !spaced.ends_with(|previous| self.maintain_before.contains(previous))
                {
                    spaced.push(' ');
                }
                spaced.push(c);
                // And look *ahead* for the mirror case: a character already
                // followed by a space must not gain a second one. At the end of
                // the name there is nothing to separate from, so nothing is
                // added — the trim stage used to clean that up, but only when
                // trimming was on.
                //
                // Nor does a space go between a closer and what closes the
                // phrase after it — another closer, or punctuation: the rule
                // is there to separate a bracket from the next *word*, and
                // `Song (Live) , 2020` is not what anyone writes.
                if self.maintain_after.contains(c)
                    && chars.peek().is_some_and(|&next| {
                        next != ' '
                            && !self.maintain_after.contains(next)
                            && !CLOSING_PUNCTUATION.contains(next)
                    })
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

/// Punctuation that ends a phrase. The maintain-after rule never puts a space
/// in front of one, whether or not it is in the user's list.
const CLOSING_PUNCTUATION: &str = ",;.:!?";

impl SpaceTrim {
    /// True when no stage has anything to do on `subject`, decided in one
    /// pass over its characters and without allocating.
    ///
    /// Conservative on purpose: a `false` costs the full pass, which then
    /// answers exactly; a `true` has to be right. So the maintain rules are
    /// checked by presence of the character alone, not by whether the space
    /// is already there.
    fn cannot_change(&self, subject: &str) -> bool {
        let mut previous_space = false;
        for c in subject.chars() {
            if c == '_' && self.underscores_to_spaces {
                return false;
            }
            if c == ' ' {
                if previous_space && self.shrink {
                    return false;
                }
                previous_space = true;
            } else {
                previous_space = false;
            }
            if self.maintain_before.contains(c) || self.maintain_after.contains(c) {
                return false;
            }
        }
        !(self.leading && subject.starts_with(' ')) && !(self.trailing && subject.ends_with(' '))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::run;
    use super::*;

    /// Only the three trim options, with nothing inserted.
    fn trim_only() -> SpaceTrim {
        SpaceTrim {
            maintain_before: String::new(),
            maintain_after: String::new(),
            underscores_to_spaces: false,
            ..Default::default()
        }
    }

    #[test]
    fn leading_spaces_are_removed() {
        let op = SpaceTrim {
            trailing: false,
            shrink: false,
            ..trim_only()
        };
        assert_eq!(run(&op, "   name  "), "name  ");
    }

    #[test]
    fn trailing_spaces_are_removed() {
        let op = SpaceTrim {
            leading: false,
            shrink: false,
            ..trim_only()
        };
        assert_eq!(run(&op, "   name  "), "   name");
    }

    #[test]
    fn multiple_spaces_shrink_into_one() {
        let op = SpaceTrim {
            leading: false,
            trailing: false,
            ..trim_only()
        };
        assert_eq!(run(&op, "a    b   c"), "a b c");
    }

    #[test]
    fn underscores_become_spaces() {
        let op = SpaceTrim {
            maintain_before: String::new(),
            maintain_after: String::new(),
            ..Default::default()
        };
        assert_eq!(run(&op, "my_holiday_photo"), "my holiday photo");
    }

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
        // A closer followed by punctuation or another closer, and an opener
        // after another opener, are one unit: no space inside them.
        assert_eq!(run(&op, "Song (Live), 2020"), "Song (Live), 2020");
        assert_eq!(run(&op, "((x))"), "((x))");
        assert_eq!(
            run(&op, "Album [Deluxe (Remastered)]"),
            "Album [Deluxe (Remastered)]"
        );
        assert_eq!(run(&op, "(live).mp3"), "(live).mp3");
        assert_eq!(run(&op, "[x]; y"), "[x]; y");
    }

    /// The shipped cleanup preset runs these defaults over music names, and a
    /// closing bracket before a comma is the commonest shape there is.
    #[test]
    fn the_defaults_leave_punctuation_after_a_bracket_alone() {
        let op = SpaceTrim::default();
        assert_eq!(
            run(&op, "Song (Live), 2020 [Remix]; Edit"),
            "Song (Live), 2020 [Remix]; Edit"
        );
        assert_eq!(
            run(&op, "a(b)c"),
            "a (b) c",
            "a word either side is still spaced"
        );
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

    /// The fast path may only say "nothing to do" when the full pass agrees.
    /// Every combination of stages over a spread of names, so a stage the
    /// pre-scan forgot shows up as a disagreement rather than a wrong name.
    #[test]
    fn the_no_change_fast_path_never_disagrees_with_the_full_pass() {
        let names = [
            "already clean",
            "a_b",
            " lead",
            "trail ",
            "two  spaces",
            "a(b)c",
            "x,y",
            "(a) [b]",
            "(a), b",
            "((a))",
            "",
            " ",
            "_",
        ];
        for bits in 0..64u8 {
            let op = SpaceTrim {
                leading: bits & 1 != 0,
                trailing: bits & 2 != 0,
                shrink: bits & 4 != 0,
                maintain_before: if bits & 8 != 0 {
                    "([".into()
                } else {
                    String::new()
                },
                maintain_after: if bits & 16 != 0 {
                    ")],".into()
                } else {
                    String::new()
                },
                underscores_to_spaces: bits & 32 != 0,
            };
            for name in names {
                if op.cannot_change(name) {
                    assert_eq!(run(&op, name), name, "{op:?} over {name:?}");
                }
            }
        }
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
