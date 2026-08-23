//! The three ways a user can describe a piece of text.
//!
//! *"If you only enter a normal string
//! (eg "hello") then files that contain this string are matched. You can also
//! enter a wildcard string (eg "hello\*") for more advanced matches"* — plus a
//! Regular Expression checkbox that *"disables the ordinary wildcards"*.
//!
//! The include filter and the pre-processor's advanced filter both need this,
//! and Replace's find box is the same three modes again.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::regex_flavor::{Pattern, PatternOptions, RegexError};
use crate::wildcard;

/// How the text in a find/filter box should be interpreted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchSpec {
    /// Plain text, matched anywhere in the subject.
    Substring(String),
    /// The `* : ?` wildcard language.
    Wildcard(String),
    /// Full regular expressions (P7 dialect).
    Regex(String),
}

impl MatchSpec {
    /// Picks substring or wildcard: the presence of a metacharacter is what
    /// switches modes. Regex is never inferred — it is
    /// always an explicit checkbox.
    pub fn auto(text: impl Into<String>) -> Self {
        let text = text.into();
        if wildcard::has_wildcards(&text) {
            Self::Wildcard(text)
        } else {
            Self::Substring(text)
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Self::Substring(s) | Self::Wildcard(s) | Self::Regex(s) => s,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text().is_empty()
    }

    /// Compiles with **search** semantics: find this text anywhere.
    ///
    /// Used by Replace and by the pre-processor's advanced filter, which is a
    /// search — `is*` against
    /// `Batch Renamer is fantastic!` yields `is fantastic!`.
    pub fn compile(&self, case_sensitive: bool) -> Result<Matcher, RegexError> {
        self.compile_with(case_sensitive, false)
    }

    /// Compiles with **filter** semantics, where a wildcard string is a mask
    /// over the whole subject rather than a search.
    ///
    /// The include filter is the one place this differs. `hello*` is the
    /// example of a "more advanced match" than the plain string `hello` —
    /// which is only true if the wildcard form is anchored. A plain
    /// string still means *"files that contain this string"*, and a regex is
    /// still a search, since the user has `^` and `$` to hand.
    pub fn compile_for_filter(&self, case_sensitive: bool) -> Result<Matcher, RegexError> {
        self.compile_with(case_sensitive, true)
    }

    fn compile_with(
        &self,
        case_sensitive: bool,
        anchor_wildcards: bool,
    ) -> Result<Matcher, RegexError> {
        let options = PatternOptions {
            case_sensitive,
            ..Default::default()
        };
        Ok(match self {
            // Substring search stays out of the regex engine entirely: it is the
            // overwhelmingly common case and runs per file per keystroke.
            Self::Substring(s) => Matcher::Substring {
                needle: s.clone(),
                case_sensitive,
            },
            Self::Wildcard(s) => {
                let body = wildcard::to_regex(s);
                let source = if anchor_wildcards {
                    format!("^(?:{body})$")
                } else {
                    body
                };
                Matcher::Pattern(Pattern::compile(&source, options)?)
            }
            Self::Regex(s) => Matcher::Pattern(Pattern::compile(s, options)?),
        })
    }
}

/// A compiled [`MatchSpec`].
#[derive(Debug)]
pub enum Matcher {
    Substring {
        needle: String,
        case_sensitive: bool,
    },
    Pattern(Pattern),
}

impl Matcher {
    pub fn is_match(&self, subject: &str) -> Result<bool, RegexError> {
        Ok(self.find(subject)?.is_some())
    }

    /// The byte range of the leftmost match, if any.
    pub fn find(&self, subject: &str) -> Result<Option<Range<usize>>, RegexError> {
        match self {
            Self::Substring {
                needle,
                case_sensitive,
            } => {
                if needle.is_empty() {
                    return Ok(Some(0..0));
                }
                if *case_sensitive {
                    Ok(subject.find(needle.as_str()).map(|i| i..i + needle.len()))
                } else {
                    Ok(find_case_insensitive(subject, needle))
                }
            }
            Self::Pattern(pattern) => pattern.find(subject),
        }
    }
}

/// Case-insensitive substring search returning a range in the **original**
/// string.
///
/// Folding the haystack with `to_lowercase` and reusing the offset is wrong:
/// case folding changes byte lengths (`İ` is 2 bytes and folds to 3, `ß` folds
/// to `ss`), so the hit would land at the wrong place — and slicing there
/// panics or silently corrupts the name. Comparing folded characters as we walk
/// the original keeps every offset anchored to the source.
fn find_case_insensitive(haystack: &str, needle: &str) -> Option<Range<usize>> {
    let folded_needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if folded_needle.is_empty() {
        return Some(0..0);
    }

    for (start, _) in haystack.char_indices() {
        let mut wanted = folded_needle.iter();
        let mut matched = true;
        for produced in haystack[start..].chars().flat_map(char::to_lowercase) {
            match wanted.next() {
                Some(&expected) if expected == produced => {}
                Some(_) => {
                    matched = false;
                    break;
                }
                None => break,
            }
        }
        if !matched || wanted.next().is_some() {
            continue;
        }

        // Consume whole source characters until they have produced at least as
        // many folded characters as the needle has.
        let mut produced = 0usize;
        let mut end = start;
        for c in haystack[start..].chars() {
            if produced >= folded_needle.len() {
                break;
            }
            produced += c.to_lowercase().count();
            end += c.len_utf8();
        }
        return Some(start..end);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(spec: &MatchSpec, subject: &str, case_sensitive: bool) -> Option<Range<usize>> {
        spec.compile(case_sensitive).unwrap().find(subject).unwrap()
    }

    #[test]
    fn a_plain_string_matches_files_that_contain_it() {
        let spec = MatchSpec::auto("hello");
        assert_eq!(spec, MatchSpec::Substring("hello".into()));
        assert_eq!(find(&spec, "say hello world", true), Some(4..9));
        assert_eq!(find(&spec, "nothing here", true), None);
    }

    #[test]
    fn a_wildcard_string_switches_the_mode_automatically() {
        let spec = MatchSpec::auto("hello*");
        assert_eq!(spec, MatchSpec::Wildcard("hello*".into()));
        assert!(spec.compile(true).unwrap().is_match("hello world").unwrap());
    }

    #[test]
    fn substring_matching_is_case_insensitive_unless_asked() {
        let spec = MatchSpec::Substring("BATCH".into());
        assert_eq!(find(&spec, "batch renamer", false), Some(0..5));
        assert_eq!(find(&spec, "batch renamer", true), None);
    }

    /// Case folding changes byte lengths, so a naive fold-and-reuse-the-offset
    /// implementation lands in the wrong place — or panics mid-character.
    #[test]
    fn case_insensitive_offsets_survive_multi_byte_characters() {
        for (subject, needle, expected) in [
            ("İÜbertWERT", "wert", "WERT"),
            ("straße WERT", "wert", "WERT"),
            ("ÜBER", "über", "ÜBER"),
            ("aÄb", "ä", "Ä"),
        ] {
            let spec = MatchSpec::Substring(needle.into());
            let range = find(&spec, subject, false)
                .unwrap_or_else(|| panic!("{needle} should be found in {subject}"));
            assert_eq!(&subject[range], expected, "in {subject:?}");
        }
    }

    #[test]
    fn case_insensitive_search_finds_the_leftmost_match() {
        let spec = MatchSpec::Substring("ab".into());
        assert_eq!(find(&spec, "xxABxxab", false), Some(2..4));
        assert_eq!(find(&spec, "nope", false), None);
    }

    #[test]
    fn an_empty_substring_matches_everything() {
        let spec = MatchSpec::Substring(String::new());
        assert_eq!(find(&spec, "anything", true), Some(0..0));
        assert_eq!(find(&spec, "anything", false), Some(0..0));
    }

    #[test]
    fn regex_mode_is_never_inferred() {
        // No wildcard metacharacters, so this stays a plain substring — a user
        // typing a regex without ticking the box gets literal matching.
        let spec = MatchSpec::auto("^a.c$");
        assert!(matches!(spec, MatchSpec::Substring(_)), "{spec:?}");
        assert!(!spec.compile(true).unwrap().is_match("abc").unwrap());

        let spec = MatchSpec::Regex("^a.c$".into());
        assert!(spec.compile(true).unwrap().is_match("abc").unwrap());
    }

    /// Search semantics for Replace and the pre-processor; mask semantics for
    /// the include filter.
    #[test]
    fn wildcards_are_a_search_normally_and_a_mask_in_a_filter() {
        let spec = MatchSpec::Wildcard("track ?".into());

        let search = spec.compile(true).unwrap();
        assert!(
            search.is_match("track 10").unwrap(),
            "search finds 'track 1'"
        );

        let filter = spec.compile_for_filter(true).unwrap();
        assert!(filter.is_match("track 1").unwrap());
        assert!(
            !filter.is_match("track 10").unwrap(),
            "mask must match wholly"
        );
        assert!(!filter.is_match("my track 1").unwrap());
    }

    #[test]
    fn a_filter_mask_still_understands_star() {
        let spec = MatchSpec::Wildcard("*.mp3".into());
        let filter = spec.compile_for_filter(false).unwrap();
        assert!(filter.is_match("song.mp3").unwrap());
        assert!(!filter.is_match("song.flac").unwrap());
    }
}
