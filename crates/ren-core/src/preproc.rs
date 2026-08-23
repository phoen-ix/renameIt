//! The Pre-Processor: narrowing a file name before an operation sees it.
//!
//! *"The pre-processor filters out a
//! section of the filename. Only this section is then processed by the actual
//! rename function."*
//!
//! Two rules from that page shape everything here:
//!
//! * Stages run top to bottom: if one alters a filename, the next sees that
//!   processed filename rather than the one it started from.
//! * Only the filename — not the extension — is processed.
//!
//! Several stages can decide a file is **not renamed at all** — that is a skip,
//! not an error, and it is why [`PreProcessor::narrow`] returns an `Option`.

use serde::{Deserialize, Serialize};

use crate::cache::Cached;
use crate::matcher::{MatchSpec, Matcher};
use crate::model::Subject;
use crate::regex_flavor::RegexError;

/// Narrows the active slice of a name, top to bottom.
///
/// Every field is optional and disabled fields are skipped, so the default is
/// the identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PreProcessor {
    /// *"Skips the first n number of letters in the filename. If n is larger
    /// than the total number of letters in the filename, it will not be renamed
    /// at all."*
    pub skip_first: Option<usize>,
    /// *"The program will search for the string you enter. If it is found,
    /// everything up until this string begins is skipped. If it is not found,
    /// the whole filename is skipped."*
    pub skip_until: Option<MatchSpec>,
    /// *"Only n number of characters will be renamed. […] If n is larger than
    /// the remaining number of characters, all are included."*
    pub limit_to: Option<usize>,
    /// *"If it is found, the part of the filename after that will be skipped.
    /// If it is not found, everything is included."*
    pub cut_at: Option<MatchSpec>,
    /// *"This allows you to match a specific section of the filename […] Only
    /// this section is kept."*
    pub section: Option<MatchSpec>,
    /// Whether the string searches above respect case.
    ///
    /// There is a case switch — the dialog puts
    /// one under the advanced filter — but scoped to that filter alone. Ours
    /// covers all three text searches, and defaults to insensitive.
    pub case_sensitive: bool,

    /// All three matchers, compiled together on first use — this runs once per
    /// file per keystroke.
    ///
    /// Private on purpose: struct-update syntax would carry an
    /// already-populated cache onto a changed configuration and keep matching
    /// by the old rules. Use the builder methods, which reset it.
    #[serde(skip)]
    compiled: Cached<Result<Compiled, RegexError>>,
}

/// The compiled form of the three searching stages.
#[derive(Debug)]
struct Compiled {
    skip_until: Option<Matcher>,
    cut_at: Option<Matcher>,
    section: Option<Matcher>,
}

impl PreProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    /// *"Skip the first n characters"*.
    pub fn skipping_first(mut self, n: usize) -> Self {
        self.skip_first = Some(n);
        self
    }

    /// *"Skip until string is found"*.
    pub fn skipping_until(mut self, spec: MatchSpec) -> Self {
        self.skip_until = Some(spec);
        self.compiled = Cached::new();
        self
    }

    /// *"Include up to n characters"*.
    pub fn limited_to(mut self, n: usize) -> Self {
        self.limit_to = Some(n);
        self
    }

    /// *"Cut if string is found"*.
    pub fn cutting_at(mut self, spec: MatchSpec) -> Self {
        self.cut_at = Some(spec);
        self.compiled = Cached::new();
        self
    }

    /// The advanced filter: keep only the matched section.
    pub fn keeping_section(mut self, spec: MatchSpec) -> Self {
        self.section = Some(spec);
        self.compiled = Cached::new();
        self
    }

    pub fn matching_case(mut self, yes: bool) -> Self {
        self.case_sensitive = yes;
        self.compiled = Cached::new();
        self
    }

    /// Forget the compiled matchers — the `&mut` editor's answer to what the
    /// builders above do by hand.
    ///
    /// The GUI edits these fields in place. Today that is safe because the plan
    /// path clones (`Card::stored_config`) and a clone starts with an empty
    /// cache — but that is a property of a *caller*, not of this type, and the
    /// day something stops cloning the bug would appear only in a live preview
    /// and nowhere in this crate's tests.
    pub fn invalidate(&mut self) {
        self.compiled = Cached::new();
    }

    pub fn is_identity(&self) -> bool {
        self.skip_first.is_none()
            && self.skip_until.is_none()
            && self.limit_to.is_none()
            && self.cut_at.is_none()
            && self.section.is_none()
    }

    fn compiled(&self) -> Result<&Compiled, RegexError> {
        self.compiled
            .get_or_init(|| {
                // **An armed but empty box is a stage that does nothing.**
                //
                // `cut_at` and `section` both match at offset 0 on an empty
                // search, and both then narrow to `0..0` — so ticking *Cut if
                // string is found* and typing nothing blanks every name in the
                // folder. With two files **P4** would catch the collision;
                // with one, nothing would.
                //
                // That is **P34** verbatim, applied where it belongs: an empty
                // Find box is already a no-op in `Replace::apply`, and an empty
                // Free Format pattern is already a no-op by P34's own text. The
                // editor makes this state reachable in two clicks, because
                // ticking a checkbox before typing into it is the normal order
                // of operations.
                let compile = |spec: &Option<MatchSpec>| -> Result<Option<Matcher>, RegexError> {
                    spec.as_ref()
                        .filter(|spec| !spec.is_empty())
                        .map(|s| s.compile(self.case_sensitive))
                        .transpose()
                };
                Ok(Compiled {
                    skip_until: compile(&self.skip_until)?,
                    cut_at: compile(&self.cut_at)?,
                    section: compile(&self.section)?,
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Narrows `subject`, or returns `None` if the file must not be renamed.
    pub fn narrow<'a>(&self, subject: &Subject<'a>) -> Result<Option<Subject<'a>>, RegexError> {
        let compiled = self.compiled()?;
        let mut current = subject.clone();

        // 1. Offset beginning — skip the first n characters.
        if let Some(n) = self.skip_first {
            let active = current.active();
            let Some(offset) = char_offset(active, n) else {
                // "If n is larger than the total number of letters in the
                // filename, it will not be renamed at all."
                return Ok(None);
            };
            current = current.narrow(offset..active.len());
        }

        // 2. Offset beginning — skip until a string is found.
        if let Some(matcher) = &compiled.skip_until {
            let active = current.active();
            let Some(hit) = matcher.find(active)? else {
                // "If it is not found, the whole filename is skipped."
                return Ok(None);
            };
            current = current.narrow(hit.start..active.len());
        }

        // 3. Limit length — include up to n characters.
        if let Some(n) = self.limit_to {
            let active = current.active();
            let end = char_offset(active, n).unwrap_or(active.len());
            current = current.narrow(0..end);
        }

        // 4. Limit length — cut if a string is found.
        if let Some(matcher) = &compiled.cut_at {
            let active = current.active();
            if let Some(hit) = matcher.find(active)? {
                current = current.narrow(0..hit.start);
            }
            // "If it is not found, everything is included."
        }

        // 5. Advanced filter — keep only the matched section.
        if let Some(matcher) = &compiled.section {
            let active = current.active();
            let Some(hit) = matcher.find(active)? else {
                return Ok(None);
            };
            current = current.narrow(hit);
        }

        Ok(Some(current))
    }
}

/// Byte offset of the `n`th character, or `None` if the string is shorter.
fn char_offset(s: &str, n: usize) -> Option<usize> {
    if n == 0 {
        return Some(0);
    }
    s.char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .or(if s.chars().count() == n {
            Some(s.len())
        } else {
            None
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn narrow(pp: &PreProcessor, name: &str) -> Option<String> {
        let subject = Subject::whole(name);
        pp.narrow(&subject).unwrap().map(|s| s.active().to_owned())
    }

    #[test]
    fn the_identity_pre_processor_changes_nothing() {
        let pp = PreProcessor::default();
        assert!(pp.is_identity());
        assert_eq!(narrow(&pp, "abc"), Some("abc".into()));
    }

    /// "tell the pre-processor to skip the first 14 characters in the filename
    /// so that only `is fantastic!` is sent to the rename function"
    #[test]
    fn skipping_the_first_n_characters_reproduces_the_manuals_example() {
        let pp = PreProcessor::new().skipping_first(14);
        assert_eq!(
            narrow(&pp, "Batch Renamer is fantastic!"),
            Some("is fantastic!".into())
        );
    }

    /// "If n is larger than the total number of letters in the filename, it
    /// will not be renamed at all."
    #[test]
    fn skipping_past_the_end_means_the_file_is_not_renamed_at_all() {
        let pp = PreProcessor::new().skipping_first(10);
        assert_eq!(narrow(&pp, "short"), None);
        // Skipping exactly the whole name leaves an empty section, which is
        // still a valid insertion point.
        let pp = PreProcessor::new().skipping_first(5);
        assert_eq!(narrow(&pp, "short"), Some(String::new()));
    }

    /// "If it is found, everything up until this string begins is skipped. If
    /// it is not found, the whole filename is skipped."
    #[test]
    fn skip_until_starts_at_the_match_and_skips_the_file_when_absent() {
        let pp = PreProcessor::new().skipping_until(MatchSpec::Substring("is".into()));
        assert_eq!(
            narrow(&pp, "Batch Renamer is fantastic!"),
            Some("is fantastic!".into())
        );
        // Leftmost match wins, so an earlier occurrence is the one that counts.
        assert_eq!(
            narrow(&pp, "This is it"),
            Some("is is it".into()),
            "the 'is' inside 'This' comes first"
        );
        assert_eq!(narrow(&pp, "nothing to see"), None);
    }

    /// "Only n number of characters will be renamed. […] If n is larger than
    /// the remaining number of characters, all are included."
    #[test]
    fn limit_to_truncates_and_tolerates_short_names() {
        let pp = PreProcessor::new().limited_to(5);
        assert_eq!(narrow(&pp, "abcdefghij"), Some("abcde".into()));
        assert_eq!(narrow(&pp, "abc"), Some("abc".into()));
    }

    /// "If it is found, the part of the filename after that will be skipped. If
    /// it is not found, everything is included."
    #[test]
    fn cut_at_drops_the_tail_and_is_a_no_op_when_absent() {
        let pp = PreProcessor::new().cutting_at(MatchSpec::Substring(" - ".into()));
        assert_eq!(narrow(&pp, "Artist - Title"), Some("Artist".into()));
        assert_eq!(narrow(&pp, "NoSeparator"), Some("NoSeparator".into()));
    }

    /// An armed but empty box is a stage that does nothing — **P34**'s rule,
    /// which `Replace::apply` already applies to an empty Find box.
    ///
    /// Without it, `cut_at` and `section` both match at offset 0, both narrow
    /// to `0..0`, and ticking *Cut if string is found* before typing into it
    /// blanks **every name in the folder**. That is two clicks away in the
    /// editor, because ticking a box before filling it is the normal order of
    /// operations.
    #[test]
    fn a_stage_whose_search_string_is_empty_does_nothing() {
        for pp in [
            PreProcessor::new().cutting_at(MatchSpec::Substring(String::new())),
            PreProcessor::new().skipping_until(MatchSpec::Substring(String::new())),
            PreProcessor::new().keeping_section(MatchSpec::Substring(String::new())),
            PreProcessor::new().cutting_at(MatchSpec::Wildcard(String::new())),
            PreProcessor::new().keeping_section(MatchSpec::Regex(String::new())),
        ] {
            assert_eq!(
                narrow(&pp, "Batch Renamer"),
                Some("Batch Renamer".into()),
                "{pp:?}"
            );
        }
    }

    /// The GUI edits these fields in place and the matchers are compiled once.
    /// Two edits in a row must both take effect.
    #[test]
    fn a_matcher_edited_in_place_takes_effect_once_the_cache_is_invalidated() {
        let mut pp = PreProcessor::new().cutting_at(MatchSpec::Substring("-".into()));
        assert_eq!(narrow(&pp, "a-b"), Some("a".into()), "populates the cache");

        pp.cut_at = Some(MatchSpec::Substring("b".into()));
        pp.invalidate();
        assert_eq!(narrow(&pp, "a-b"), Some("a-".into()), "the second setting");
    }

    /// "searching for `is*` in `Batch Renamer is fantastic!` will find
    /// `is fantastic!`"
    #[test]
    fn the_advanced_filter_keeps_only_the_matched_section() {
        let pp = PreProcessor::new().keeping_section(MatchSpec::Wildcard("is*".into()));
        assert_eq!(
            narrow(&pp, "Batch Renamer is fantastic!"),
            Some("is fantastic!".into())
        );
    }

    #[test]
    fn the_advanced_filter_can_pick_out_a_bracketed_section() {
        let pp = PreProcessor::new().keeping_section(MatchSpec::Wildcard("(*)".into()));
        assert_eq!(narrow(&pp, "song (live) take 2"), Some("(live)".into()));
        assert_eq!(narrow(&pp, "no brackets here"), None);
    }

    /// "All options you see below are processed from top to bottom."
    #[test]
    fn stages_run_top_to_bottom_each_seeing_the_previous_output() {
        let pp = PreProcessor::new().skipping_first(6).limited_to(7);
        assert_eq!(
            narrow(&pp, "Batch Renamer is fantastic!"),
            Some("Renamer".into())
        );
    }

    #[test]
    fn narrowing_keeps_the_untouched_text_for_reassembly() {
        let pp = PreProcessor::new().skipping_first(6).limited_to(7);
        let subject = Subject::whole("Batch Renamer is fantastic!");
        let narrowed = pp.narrow(&subject).unwrap().unwrap();
        assert_eq!(narrowed.active(), "Renamer");
        assert_eq!(
            narrowed.reassemble("RENAMER"),
            "Batch RENAMER is fantastic!"
        );
    }

    #[test]
    fn character_counting_is_unicode_aware() {
        let pp = PreProcessor::new().skipping_first(3);
        // Three characters, six bytes.
        assert_eq!(narrow(&pp, "ÜÖÄxyz"), Some("xyz".into()));
    }
}
