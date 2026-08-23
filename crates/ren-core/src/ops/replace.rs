//! Find & Replace, and Batch Replace.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};
use crate::cache::Cached;
use crate::matcher::MatchSpec;
use crate::regex_flavor::{Pattern, PatternOptions, RegexError};
use crate::template::{Template, TextTemplate};
use crate::wildcard;

/// *"The Replace function allows you to look for a certain string within
/// filenames and replace it with another string."*
///
/// Case Sensitive, Swap Mode and Regular Expression all start unchecked, and
/// Skip and Max both start at 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Replace {
    /// Compiled on first use. Without this the regex is rebuilt once per file
    /// per keystroke, which alone blew the M1 planning budget three times over.
    #[serde(skip)]
    compiled: Cached<Result<Pattern, RegexError>>,
    /// *"Enter the string you wish to search for."* Wildcards allowed.
    pub find: String,
    /// *"If the search string is found, it will be replaced with this string."*
    /// *"Leave this box empty to delete the search string."*
    ///
    /// A template: *"You can use tags in
    /// the replace box"*. Serialises transparently, so every preset and job
    /// file written before it became one still parses.
    pub replace: TextTemplate,
    /// *"If you want an exact match, check this one."*
    pub case_sensitive: bool,
    /// *"in addition to "Look" being replaced by "Replace", "Replace" is also
    /// replaced by "Look""*.
    pub swap: bool,
    /// *"Enable this to turn on regular expressions in the find box (instead of
    /// just simple wildcards)."*
    pub regex: bool,
    /// *"You can skip replacing the first # occurances the program finds."*
    pub skip: usize,
    /// The UI calls this **Max**, the documented name is Count:
    /// *"limit the number of replaces to perform within each filename […] Enter
    /// 0 for unlimited."*
    pub max: usize,
}

impl Replace {
    pub fn new(find: impl Into<String>, replace: impl Into<TextTemplate>) -> Self {
        Self {
            find: find.into(),
            replace: replace.into(),
            ..Default::default()
        }
    }

    pub fn regex(mut self, yes: bool) -> Self {
        self.regex = yes;
        self
    }

    pub fn case_sensitive(mut self, yes: bool) -> Self {
        self.case_sensitive = yes;
        self
    }

    pub fn swap(mut self, yes: bool) -> Self {
        self.swap = yes;
        self
    }

    pub fn skip(mut self, n: usize) -> Self {
        self.skip = n;
        self
    }

    pub fn max(mut self, n: usize) -> Self {
        self.max = n;
        self
    }

    /// The replacement as a fixed string, when it is one.
    ///
    /// `None` means it holds tags — or does not compile, in which case `apply`
    /// is the right place for the error and this is not.
    fn literal_replacement(&self) -> Option<&str> {
        self.replace
            .compiled()
            .ok()
            .and_then(Template::literal_text)
    }

    /// *"Swap mode will be ignored if wildcards are used in the find box, since
    /// it is not logically possible to conbine the both."*
    ///
    /// We extend the same reasoning to regex mode: a pattern cannot be turned
    /// back into the literal text it matched, so there is nothing to swap with.
    /// And to a replacement holding tags, for a mechanical reason rather than a
    /// logical one: swap compiles an alternation of *both* literals into one
    /// pattern cached for the whole operation, and a value that differs per
    /// file cannot live in that cache — while a per-file pattern would thrash
    /// the process-wide regex cache P40 exists to protect. The editor already
    /// draws "swap ignored here" whenever this returns false.
    /// Recorded as a deviation in `docs/DECISIONS.md`.
    pub fn swap_applies(&self) -> bool {
        self.swap_applies_with(self.literal_replacement())
    }

    /// The same, given an answer already in hand — `apply` has one and asking
    /// again per file is work with a known result.
    fn swap_applies_with(&self, literal: Option<&str>) -> bool {
        self.swap && !self.regex && !wildcard::has_wildcards(&self.find) && literal.is_some()
    }

    fn spec(&self) -> MatchSpec {
        if self.regex {
            MatchSpec::Regex(self.find.clone())
        } else {
            MatchSpec::auto(self.find.clone())
        }
    }

    fn options(&self) -> PatternOptions {
        PatternOptions {
            case_sensitive: self.case_sensitive,
            ..Default::default()
        }
    }

    /// The pattern this operation matches with, compiled once.
    ///
    /// Swap Mode needs a different pattern (an alternation of both strings), so
    /// it is folded in here rather than compiled separately.
    fn pattern(&self) -> Result<&Pattern, RegexError> {
        self.compiled
            .get_or_init(|| {
                let source = if self.swap_applies() {
                    format!(
                        "{}|{}",
                        regex_escape(&self.find),
                        regex_escape(self.literal_replacement().unwrap_or_default())
                    )
                } else {
                    match self.spec() {
                        MatchSpec::Substring(s) => regex_escape(&s),
                        MatchSpec::Wildcard(s) => wildcard::to_regex(&s),
                        MatchSpec::Regex(s) => s,
                    }
                };
                Pattern::compile(&source, self.options())
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Swap Mode: one simultaneous pass, so text written by one direction is
    /// never re-read by the other. A sequential two-pass replace would turn
    /// every `find` into `replace` and then straight back again.
    fn swap_both_ways(&self, subject: &str, replacement: &str) -> Result<String, OpError> {
        if self.find.is_empty() || replacement.is_empty() {
            return Ok(subject.to_owned());
        }
        let pattern = self.pattern().map_err(|e| OpError::new("replace", e))?;

        let mut out = String::with_capacity(subject.len());
        let mut copied_to = 0usize;
        let mut seen = 0usize;
        let mut replaced = 0usize;

        for hit in pattern
            .find_all(subject)
            .map_err(|e| OpError::new("replace", e))?
        {
            seen += 1;
            if seen <= self.skip {
                continue;
            }
            if self.max != 0 && replaced >= self.max {
                break;
            }
            out.push_str(&subject[copied_to..hit.start]);
            let matched = &subject[hit.clone()];
            out.push_str(if equals(matched, &self.find, self.case_sensitive) {
                replacement
            } else {
                &self.find
            });
            copied_to = hit.end;
            replaced += 1;
        }
        out.push_str(&subject[copied_to..]);
        Ok(out)
    }
}

fn equals(a: &str, b: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.to_lowercase() == b.to_lowercase()
    }
}

/// Escapes every regex metacharacter so a literal string matches itself.
///
/// Public because Visual Assist lifts a span of a real filename into the *Look
/// For* box, and that box is an interpreter when *Regular expression* is
/// ticked: `(1)` selected from `photo (1).jpg` would otherwise become a capture
/// group matching the bare `1`. **D115**'s rule decides it — what the data said
/// is sanitised, what the user wrote is obeyed — and `\(1\)` is precisely the
/// pattern that means the literal the user pointed at.
pub fn regex_escape(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len() + 8);
    for c in literal.chars() {
        if "\\.+*?()|[]{}^$#&~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl NameTransform for Replace {
    fn id(&self) -> &'static str {
        "replace"
    }

    fn summary(&self) -> String {
        // A card the user has only just added has an empty *find* box, and
        // `Delete ""` is a summary that describes a deletion of nothing rather
        // than a card waiting to be filled in. It is also the first thing a
        // new user reads in the pipeline panel.
        if self.find.is_empty() {
            return "Nothing to find yet".to_owned();
        }
        if self.replace.is_empty() {
            format!("Delete {:?}", self.find)
        } else {
            format!("Replace {:?} -> {:?}", self.find, self.replace.as_str())
        }
    }

    fn needs(&self) -> crate::template::TagNeeds {
        self.replace.needs()
    }

    fn asks(&self) -> Vec<crate::run::AskSpec> {
        self.replace.asks()
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        if self.find.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }

        // Asked once. This runs per file per keystroke, and the answer cannot
        // change between the swap check and the replacement — reading it twice
        // was doing the same cached lookups and the same `literal_text` match
        // twice for every file in the listing.
        let literal = self.literal_replacement();

        if self.swap_applies_with(literal) {
            let out = self.swap_both_ways(subject, literal.unwrap_or_default())?;
            return Ok(borrow_if_unchanged(subject, out));
        }

        // The common case by far: a replacement with no tags in it is the text
        // the user typed, and there is nothing to render or to escape.
        let replacement = match literal {
            Some(text) => Cow::Borrowed(text),
            // `$1`–`$9` in the replacement are capture references, so a `$`
            // that arrived *in a tag value* is escaped and a `$` the user typed
            // is not (D48, D61). `$$` is the regex crate's literal dollar.
            None => match cx.render_with(&self.replace, |value, out| {
                for c in value.chars() {
                    if c == '$' {
                        out.push('$');
                    }
                    out.push(c);
                }
            })? {
                Some(text) => text,
                // "Only rename if all tags are available".
                None => return Ok(Cow::Borrowed(subject)),
            },
        };

        let pattern = self.pattern().map_err(|e| OpError::new("replace", e))?;
        let out = pattern
            .replace_skipping(subject, &replacement, self.skip, self.max)
            .map_err(|e| OpError::new("replace", e))?;
        Ok(borrow_if_unchanged(subject, out))
    }
}

fn borrow_if_unchanged(subject: &str, produced: String) -> Cow<'_, str> {
    if produced == subject {
        Cow::Borrowed(subject)
    } else {
        Cow::Owned(produced)
    }
}

/// *"With Batch Replace you can run several Replace commands in one go!"*
///
/// Ships pre-loaded with 51 rules. An empty Batch Replace would be an
/// operation the user can select and that then does nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BatchReplace {
    /// Executed top-down, each seeing the previous rule's output.
    pub rules: Vec<Replace>,
}

impl Default for BatchReplace {
    fn default() -> Self {
        Self {
            rules: shipped_rules().clone(),
        }
    }
}

impl BatchReplace {
    pub fn new(rules: Vec<Replace>) -> Self {
        Self { rules }
    }

    pub fn empty() -> Self {
        Self { rules: Vec::new() }
    }
}

// --- The shipped default list ------------------------------------------------

#[derive(Debug, Deserialize)]
struct ShippedFile {
    #[allow(dead_code)]
    version: u32,
    rule: Vec<LiteralRule>,
    contractions: Contractions,
}

#[derive(Debug, Deserialize)]
struct LiteralRule {
    find: String,
    replace: String,
    regex: bool,
    #[allow(dead_code)]
    #[serde(default)]
    note: String,
}

#[derive(Debug, Deserialize)]
struct Contractions {
    stand_ins: Vec<String>,
    boundary: String,
    pairs: Vec<(String, String)>,
}

/// The defaults we ship (D6/D20), parsed once.
///
/// The contraction rules are *generated* from linguistic pairs rather than
/// stored as fifty near-identical regexes — see
/// `crates/ren-core/data/batch_replace_default.toml`.
pub fn shipped_rules() -> &'static Vec<Replace> {
    static RULES: std::sync::OnceLock<Vec<Replace>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        let text = include_str!("../../data/batch_replace_default.toml");
        let parsed: ShippedFile =
            toml::from_str(text).expect("the shipped batch-replace defaults must parse");

        let mut rules: Vec<Replace> = parsed
            .rule
            .iter()
            .map(|r| Replace::new(&r.find, r.replace.as_str()).regex(r.regex))
            .collect();

        let class: String = parsed.contractions.stand_ins.concat();
        let boundary = &parsed.contractions.boundary;
        for (head, tail) in &parsed.contractions.pairs {
            rules.push(
                Replace::new(
                    format!("( {head})[{class}]?({tail}){boundary}"),
                    format!("$1'$2{boundary}"),
                )
                .regex(true),
            );
        }
        rules
    })
}

impl NameTransform for BatchReplace {
    fn id(&self) -> &'static str {
        "batch_replace"
    }

    fn summary(&self) -> String {
        format!("Batch replace ({} rules)", self.rules.len())
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        let mut current = Cow::Borrowed(subject);
        for rule in &self.rules {
            match rule.apply(&current, cx)? {
                Cow::Borrowed(_) => {}
                Cow::Owned(next) => current = Cow::Owned(next),
            }
        }
        Ok(current)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::run;
    use super::*;

    /// "Use the Replace function and enter "_" in the search box and " " (space)
    /// in the replace box to tidy up your filenames!"
    #[test]
    fn replacing_underscores_with_spaces_tidies_up_a_filename() {
        assert_eq!(
            run(&Replace::new("_", " "), "my_holiday_photo"),
            "my holiday photo"
        );
    }

    // --- tags in the replace box (M8) --------------------------------------

    /// > *"You can use tags in the replace box"*.
    ///
    /// It could not: `replace` was a plain `String` and `apply` took an
    /// `EvalCx` it ignored, so `<Parent>` was written into the filename
    /// verbatim, angle brackets and all.
    #[test]
    fn the_replace_box_takes_tags() {
        // `run` builds the entry under /tmp, so `<Parent>` is "tmp".
        assert_eq!(run(&Replace::new("_", "<Parent>"), "a_b"), "atmpb");
    }

    /// D48 and D61's rule, in the one field where a rendered value re-enters an
    /// interpreter: what the data said is sanitised, what the user wrote is
    /// obeyed. A file whose own name holds `$1` keeps it.
    #[test]
    fn a_dollar_from_a_tag_is_text_and_a_dollar_the_user_typed_is_a_capture() {
        assert_eq!(run(&Replace::new("Q", "<Name>"), "a$1bQ"), "a$1ba$1bQ");
        assert_eq!(
            run(&Replace::new("(a)(b)", "$2$1").regex(true), "ab"),
            "ba",
            "a capture reference in the box the user typed in still means capture"
        );
    }

    /// Swap compiles an alternation of *both* literals into one pattern cached
    /// for the whole operation, so a replacement that differs per file cannot
    /// take part. Same shape as the wildcard and regex exclusions, and the
    /// editor draws "swap ignored here" for all three.
    #[test]
    fn swap_mode_is_ignored_when_the_replace_box_holds_tags() {
        assert!(Replace::new("a", "b").swap(true).swap_applies());
        assert!(!Replace::new("a", "<Name>").swap(true).swap_applies());
        // And the behaviour, not just the predicate: with a tag it replaces one
        // way only.
        assert_eq!(run(&Replace::new("a", "b").swap(true), "ab"), "ba");
        assert_eq!(run(&Replace::new("a", "<Parent>").swap(true), "ab"), "tmpb");
    }

    /// D29 makes an unrecognised closed tag a hard error rather than literal
    /// text. That now reaches this box too — the compatibility break recorded
    /// with the change.
    #[test]
    fn an_unrecognised_tag_in_the_replace_box_is_an_error() {
        let e = super::super::testing::entry("x");
        let cx = EvalCx::simple(&e, 0, 1);
        assert!(Replace::new("x", "<Nmae>").apply("x", &cx).is_err());
        // An unclosed `<` stays literal, which is what keeps the blast radius
        // narrow: only a *closed* unrecognised tag changes meaning.
        assert_eq!(run(&Replace::new("x", "<Nmae"), "x"), "<Nmae");
    }

    /// `TextTemplate` is `#[serde(transparent)]`, so a preset or job file
    /// written when this was a plain string still parses unchanged.
    #[test]
    fn a_preset_written_before_the_box_took_tags_still_parses() {
        let op: Replace = toml::from_str("find = \"_\"\nreplace = \" \"\n").unwrap();
        assert_eq!(op.replace.as_str(), " ");
        assert_eq!(run(&op, "a_b"), "a b");
    }

    /// "Leave this box empty to delete the search string!"
    #[test]
    fn an_empty_replacement_deletes_the_search_string() {
        assert_eq!(run(&Replace::new("XXX", ""), "aXXXbXXXc"), "abc");
    }

    /// Unchecked, `Photo` and `photo` are the same word to the matcher.
    #[test]
    fn matching_is_case_insensitive_unless_case_sensitive_is_checked() {
        assert_eq!(run(&Replace::new("photo", "P"), "Photo photo"), "P P");
        assert_eq!(
            run(
                &Replace::new("photo", "P").case_sensitive(true),
                "Photo photo"
            ),
            "Photo P"
        );
    }

    #[test]
    fn wildcards_work_in_the_find_box() {
        // "*" matches zero or more characters.
        assert_eq!(
            run(&Replace::new("(*)", ""), "song (live).mp3"),
            "song .mp3"
        );
        // "?" matches exactly one.
        assert_eq!(run(&Replace::new("track ?", "#"), "track 7"), "#");
    }

    #[test]
    fn regex_mode_supports_capture_groups_in_the_replacement() {
        // The worked example.
        let op = Replace::new(r"( don)[ `´]?(t)", "$1'$2").regex(true);
        assert_eq!(run(&op, "I dont care"), "I don't care");
        assert_eq!(run(&op, "I don`t care"), "I don't care");
    }

    #[test]
    fn a_literal_find_string_never_acts_as_a_regex() {
        // Without the regex box ticked, "." is just a period.
        assert_eq!(run(&Replace::new(".", "-"), "a.b.c"), "a-b-c");
        assert_eq!(run(&Replace::new("a+b", "x"), "a+b ab"), "x ab");
    }

    /// "You can skip replacing the first # occurances the program finds. Enter 0
    /// to incklude all occurances from the beginning."
    #[test]
    fn skip_passes_over_the_first_occurrences() {
        assert_eq!(run(&Replace::new("a", "X").skip(0), "aaaa"), "XXXX");
        assert_eq!(run(&Replace::new("a", "X").skip(2), "aaaa"), "aaXX");
    }

    /// "You can limit the number of replaces to perform within each filename by
    /// entering a value here. Enter 0 for unlimited."
    #[test]
    fn max_limits_the_replacements_and_zero_means_unlimited() {
        assert_eq!(run(&Replace::new("a", "X").max(1), "aaaa"), "Xaaa");
        assert_eq!(run(&Replace::new("a", "X").max(0), "aaaa"), "XXXX");
    }

    /// "in addition to "Look" being replaced by "Replace", "Replace" is also
    /// replaced by "Look". In other words, the two search strings will be
    /// swapped."
    #[test]
    fn swap_mode_exchanges_the_two_strings_in_one_pass() {
        let op = Replace::new("Artist", "Title").swap(true);
        assert_eq!(run(&op, "Artist - Title"), "Title - Artist");
        // A sequential two-pass implementation would produce "Artist - Artist".
        assert_eq!(run(&op, "Artist Artist Title"), "Title Title Artist");
    }

    /// "Swap mode will be ignored if wildcards are used in the find box"
    #[test]
    fn swap_mode_is_ignored_when_the_find_box_holds_wildcards() {
        let op = Replace::new("a*b", "Z").swap(true);
        assert!(!op.swap_applies());
        assert_eq!(run(&op, "axxb Z"), "Z Z");
    }

    /// Our extension of the same rule — recorded as a deviation.
    #[test]
    fn swap_mode_is_ignored_in_regex_mode_too() {
        let op = Replace::new("a.b", "Z").regex(true).swap(true);
        assert!(!op.swap_applies());
    }

    #[test]
    fn an_empty_find_box_is_a_no_op() {
        assert_eq!(run(&Replace::new("", "X"), "unchanged"), "unchanged");
    }

    #[test]
    fn batch_replace_runs_its_rules_top_down() {
        let batch = BatchReplace::new(vec![
            Replace::new("_", " "),
            Replace::new("  ", " "),
            Replace::new("dont", "don't"),
        ]);
        assert_eq!(run(&batch, "i_dont__care"), "i don't care");
    }

    #[test]
    fn a_batch_with_no_rules_changes_nothing() {
        assert_eq!(run(&BatchReplace::empty(), "untouched"), "untouched");
    }

    /// The panel reads "51 items in batch replace list".
    #[test]
    fn the_shipped_list_has_the_documented_number_of_rules() {
        assert_eq!(shipped_rules().len(), 51);
        assert_eq!(BatchReplace::default().rules.len(), 51);
    }

    /// The whole point of shipping it pre-loaded: selecting Batch Replace and
    /// doing nothing else already tidies a name up.
    #[test]
    fn the_shipped_list_fixes_underscores_and_contractions_out_of_the_box() {
        let batch = BatchReplace::default();
        assert_eq!(run(&batch, "i_dont_care"), "i don't care");
        assert_eq!(run(&batch, "it_isnt_here "), "it isn't here ");
        assert_eq!(run(&batch, "nothing to fix"), "nothing to fix");
    }
}
