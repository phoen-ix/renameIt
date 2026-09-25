//! The three-metacharacter wildcard language.
//!
//! ```text
//! *   matches zero or more characters
//! :   matches one or more characters
//! ?   matches exactly one character
//! ```
//!
//! The same three metacharacters wherever a search string is accepted.
//! Wildcards appear in three places — the Replace find box, the include filter
//! and the pre-processor's advanced filter — so there is exactly one
//! implementation, and it compiles down to [`crate::regex_flavor::Pattern`] so
//! the P7 step budget applies to wildcards too.

/// Do any of the three metacharacters appear in `pattern`?
///
/// Two things hang off this: Swap Mode does nothing when the find box holds a
/// wildcard (a pattern has no fixed text to swap with), and a filter string
/// containing wildcards switches from substring matching to wildcard matching.
pub fn has_wildcards(pattern: &str) -> bool {
    pattern.contains(['*', ':', '?'])
}

/// Translates a wildcard pattern into an equivalent regular expression.
///
/// The result is **unanchored**: a wildcard pattern is searched for, not
/// matched against the whole string (P19) — `is*` against
/// `Batch Renamer is fantastic!` finds `is fantastic!`, not the whole name.
/// The include filter anchors it where a mask is wanted.
pub fn to_regex(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() * 2);
    for c in pattern.chars() {
        match c {
            '*' => out.push_str(".*"),
            ':' => out.push_str(".+"),
            '?' => out.push('.'),
            // Everything else is a literal, including regex metacharacters.
            c => escape_into(c, &mut out),
        }
    }
    out
}

fn escape_into(c: char, out: &mut String) {
    const NEEDS_ESCAPE: [char; 15] = [
        '\\', '.', '+', '(', ')', '|', '[', ']', '{', '}', '^', '$', '#', '&', '~',
    ];
    if NEEDS_ESCAPE.contains(&c) {
        out.push('\\');
    }
    out.push(c);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regex_flavor::{Pattern, PatternOptions};

    fn matches(wildcard: &str, subject: &str) -> bool {
        let pattern = Pattern::compile(&to_regex(wildcard), PatternOptions::default()).unwrap();
        pattern.is_match(subject).unwrap()
    }

    #[test]
    fn star_matches_zero_or_more_characters() {
        assert!(matches("a*b", "ab"));
        assert!(matches("a*b", "axxxb"));
        assert!(!matches("a*b", "a"));
    }

    #[test]
    fn colon_matches_one_or_more_characters() {
        assert!(!matches("a:b", "ab"));
        assert!(matches("a:b", "axb"));
        assert!(matches("a:b", "axxxb"));
    }

    #[test]
    fn question_mark_matches_exactly_one_character() {
        assert!(!matches("a?b", "ab"));
        assert!(matches("a?b", "axb"));
        assert!(!matches("a?b", "axxb"));
    }

    #[test]
    fn regex_metacharacters_stay_literal() {
        assert!(matches("a.b", "a.b"));
        assert!(!matches("a.b", "axb"), "'.' must not act as a wildcard");
        assert!(matches("(x)", "a(x)b"));
        assert!(matches("1+1", "1+1"));
        assert!(matches("[a]", "[a]"));
        assert!(matches("100$", "cost 100$"));
    }

    #[test]
    fn matching_is_a_search_not_a_whole_string_match() {
        // The pre-processor's search (P19).
        assert!(matches("is*", "Batch Renamer is fantastic!"));
        assert!(matches("hello*", "say hello world"));
    }

    #[test]
    fn wildcard_detection_drives_swap_mode_and_filter_mode() {
        assert!(has_wildcards("a*b"));
        assert!(has_wildcards("a:b"));
        assert!(has_wildcards("a?b"));
        assert!(!has_wildcards("plain text"));
        assert!(!has_wildcards("a.b+c"));
    }
}
