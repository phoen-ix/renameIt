//! The regular-expression engine, wrapped so P7 can be honoured.
//!
//! Two things this wrapper exists for:
//!
//! 1. **A step budget.** `fancy-regex` backtracks, so a user pattern can be
//!    exponential — and in RenameIt it runs once per file per keystroke.
//!    `Regex::replace_all` *panics* when the backtrack limit trips (it is
//!    `try_replacen(..).unwrap()`), which would take the whole app down. We
//!    always go through the fallible API and turn the overflow into a per-row
//!    "pattern too slow" error.
//! 2. **A single place to record dialect deviations** from the JScript
//!    flavour — see `docs/spikes/regex.md`.
//! 3. **JScript replacement semantics.** Rust parses `$name` greedily, so the
//!    perfectly reasonable `$1x` asks for a group called `1x` and silently
//!    expands to nothing. [`translate_replacement`] rewrites `$1`–`$9` into
//!    Rust's braced form first.

use fancy_regex::{Error as FancyError, RegexBuilder, RuntimeError};

/// Backtracking steps a single evaluation may take before it is abandoned.
///
/// `fancy-regex`'s own default is 1_000_000. Ours is deliberately lower: this
/// runs per file per keystroke, so "give up and tell the user" beats "stall the
/// preview".
pub const DEFAULT_BACKTRACK_LIMIT: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternOptions {
    /// The "Case Sensitive" checkbox is unchecked by default, so matching is
    /// case-insensitive unless asked otherwise.
    pub case_sensitive: bool,
    /// `false` makes `\w \d \s \b` ASCII-only, matching JScript exactly.
    /// `true` (our default) widens them to Unicode — a deliberate superset of
    /// what JScript itself offers.
    pub unicode: bool,
    pub backtrack_limit: usize,
}

impl Default for PatternOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            unicode: true,
            backtrack_limit: DEFAULT_BACKTRACK_LIMIT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegexError {
    #[error("invalid pattern: {0}")]
    InvalidPattern(String),
    #[error("pattern too slow: gave up after {limit} backtracking steps")]
    TooSlow { limit: usize },
    #[error("regex engine error: {0}")]
    Engine(String),
}

/// A compiled pattern with a per-evaluation budget attached.
///
/// The regex itself is shared: see [`Pattern::compile`].
#[derive(Debug, Clone)]
pub struct Pattern {
    regex: std::sync::Arc<fancy_regex::Regex>,
    backtrack_limit: usize,
}

/// Compiled regexes, keyed by exactly what produced them.
///
/// **Why this exists.** An operation's `Cached` compiled artefact is reset
/// whenever the operation is cloned (D21), and the GUI clones the whole
/// pipeline on every keystroke — that is what makes editing a pattern take
/// effect at all. The cost is paying for the *compile* again each time, and a
/// Batch Replace card carries fifty-one regexes: a preset-shaped pipeline spent
/// **21 ms per keystroke** compiling patterns that had not changed, before the
/// files were even looked at.
///
/// Compiling the same text twice can only ever produce the same matcher, so the
/// result is shared here instead. `Cached` still does its job — it decides
/// *when* a pattern is looked up — and this decides how much that costs.
static COMPILED: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<CacheKey, std::sync::Arc<fancy_regex::Regex>>>,
> = std::sync::OnceLock::new();

/// Bounded, because a user typing into the Find box produces a new pattern per
/// keystroke and nothing would ever evict them. Clearing wholesale is fine:
/// this is a cost cache, never a correctness one.
const COMPILED_CAPACITY: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    pattern: String,
    case_sensitive: bool,
    unicode: bool,
    backtrack_limit: usize,
}

impl Pattern {
    pub fn compile(pattern: &str, options: PatternOptions) -> Result<Self, RegexError> {
        let key = CacheKey {
            pattern: pattern.to_owned(),
            case_sensitive: options.case_sensitive,
            unicode: options.unicode,
            backtrack_limit: options.backtrack_limit,
        };

        let cache = COMPILED.get_or_init(Default::default);
        if let Ok(map) = cache.lock()
            && let Some(regex) = map.get(&key)
        {
            return Ok(Self {
                regex: regex.clone(),
                backtrack_limit: options.backtrack_limit,
            });
        }

        let regex = std::sync::Arc::new(
            RegexBuilder::new(pattern)
                .case_insensitive(!options.case_sensitive)
                .unicode_mode(options.unicode)
                .backtrack_limit(options.backtrack_limit)
                .build()
                .map_err(|e| classify(e, options.backtrack_limit))?,
        );

        if let Ok(mut map) = cache.lock() {
            if map.len() >= COMPILED_CAPACITY {
                map.clear();
            }
            map.insert(key, regex.clone());
        }

        Ok(Self {
            regex,
            backtrack_limit: options.backtrack_limit,
        })
    }

    pub fn is_match(&self, subject: &str) -> Result<bool, RegexError> {
        self.regex
            .is_match(subject)
            .map_err(|e| classify(e, self.backtrack_limit))
    }

    /// The byte range of the leftmost match, if any.
    pub fn find(&self, subject: &str) -> Result<Option<std::ops::Range<usize>>, RegexError> {
        self.regex
            .find(subject)
            .map(|m| m.map(|m| m.start()..m.end()))
            .map_err(|e| classify(e, self.backtrack_limit))
    }

    /// Every non-overlapping match, left to right.
    pub fn find_all(&self, subject: &str) -> Result<Vec<std::ops::Range<usize>>, RegexError> {
        self.regex
            .find_iter(subject)
            .map(|m| {
                m.map(|m| m.start()..m.end())
                    .map_err(|e| classify(e, self.backtrack_limit))
            })
            .collect()
    }

    /// Replace, honouring the **Skip** and **Max** boxes.
    ///
    /// Skip skips replacing the first *n* occurrences found (0 = from the
    /// beginning), and Max — the
    /// manual calls it Count — *"limit the number of replaces to perform within
    /// each filename"* (0 = unlimited). Skipped occurrences still count as
    /// occurrences; the limit applies to replacements actually made.
    pub fn replace_skipping(
        &self,
        subject: &str,
        replacement: &str,
        skip: usize,
        max: usize,
    ) -> Result<String, RegexError> {
        let expansion = translate_replacement(replacement);
        let mut out = String::with_capacity(subject.len());
        let mut copied_to = 0usize;
        let mut seen = 0usize;
        let mut replaced = 0usize;

        for captures in self.regex.captures_iter(subject) {
            let captures = captures.map_err(|e| classify(e, self.backtrack_limit))?;
            let whole = captures.get(0).expect("group 0 always exists");

            seen += 1;
            if seen <= skip {
                continue;
            }
            if max != 0 && replaced >= max {
                break;
            }

            out.push_str(&subject[copied_to..whole.start()]);
            captures.expand(&expansion, &mut out);
            copied_to = whole.end();
            replaced += 1;
        }

        out.push_str(&subject[copied_to..]);
        Ok(out)
    }

    /// Replaces every match. `$1`–`$9` in `replacement` refer to capture
    /// groups.
    pub fn replace_all(&self, subject: &str, replacement: &str) -> Result<String, RegexError> {
        self.replacen(subject, replacement, 0)
    }

    /// `limit == 0` means "no limit", the same convention the Count box
    /// uses.
    pub fn replacen(
        &self,
        subject: &str,
        replacement: &str,
        limit: usize,
    ) -> Result<String, RegexError> {
        self.regex
            .try_replacen(subject, limit, translate_replacement(replacement).as_str())
            .map(|c| c.into_owned())
            .map_err(|e| classify(e, self.backtrack_limit))
    }
}

/// Rewrites a JScript-style replacement string into the one Rust expects.
///
/// Capture groups are referenced as `$1`–`$9`. Rust reads `$` followed by the
/// *longest* run of word characters as a group name, so `$1x` means "group
/// `1x`" — which does not exist, so the whole replacement silently vanishes.
/// Braced `${1}` is unambiguous, so that is what we emit. Every other `$` is
/// escaped into a literal dollar sign, which is also what JScript does.
pub fn translate_replacement(replacement: &str) -> String {
    let mut out = String::with_capacity(replacement.len());
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // `$$` is the escape hatch for a literal dollar.
            Some('$') => {
                chars.next();
                out.push_str("$$");
            }
            Some(&d @ '1'..='9') => {
                chars.next();
                out.push('$');
                out.push('{');
                out.push(d);
                out.push('}');
            }
            // A lone `$`, or `$0`, or `$name`: a literal dollar sign.
            _ => out.push_str("$$"),
        }
    }
    out
}

fn classify(error: FancyError, limit: usize) -> RegexError {
    match error {
        FancyError::RuntimeError(RuntimeError::BacktrackLimitExceeded) => {
            RegexError::TooSlow { limit }
        }
        FancyError::ParseError(pos, e) => {
            RegexError::InvalidPattern(format!("{e} (at offset {pos})"))
        }
        FancyError::CompileError(e) => RegexError::InvalidPattern(e.to_string()),
        other => RegexError::Engine(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_groups_are_referenced_as_dollar_one_to_nine() {
        // The worked example from the metacharacter reference.
        let p = Pattern::compile(r"( don)[ `´]?(t)", PatternOptions::default()).unwrap();
        assert_eq!(p.replace_all(" dont ", "$1'$2").unwrap(), " don't ");
        assert_eq!(p.replace_all(" don`t ", "$1'$2").unwrap(), " don't ");
    }

    #[test]
    fn matching_is_case_insensitive_by_default() {
        let p = Pattern::compile("abc", PatternOptions::default()).unwrap();
        assert!(p.is_match("xxABCxx").unwrap());

        let p = Pattern::compile(
            "abc",
            PatternOptions {
                case_sensitive: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!p.is_match("xxABCxx").unwrap());
    }

    #[test]
    fn an_invalid_pattern_is_an_error_not_a_panic() {
        let err = Pattern::compile("(unclosed", PatternOptions::default()).unwrap_err();
        assert!(matches!(err, RegexError::InvalidPattern(_)), "{err:?}");
    }

    /// P7's actual promise: a pathological pattern degrades to a row error.
    /// Note the fallible API — `Regex::replace_all` would *panic* here.
    ///
    /// The pattern needs a backreference to a repeated group: `fancy-regex`
    /// delegates anything the linear-time `regex` engine can handle, so the
    /// textbook `(a+)+$` bomb is harmless here. See `docs/spikes/regex.md`.
    #[test]
    fn a_catastrophic_pattern_reports_too_slow_instead_of_hanging() {
        let options = PatternOptions {
            backtrack_limit: 1_000,
            ..Default::default()
        };
        let p = Pattern::compile(r"(\w+\s?)*\1$", options).unwrap();
        assert_eq!(
            p.replace_all("an input string that takes a long time!", "x")
                .unwrap_err(),
            RegexError::TooSlow { limit: 1_000 }
        );
    }

    /// The reassuring half of the same finding.
    #[test]
    fn the_textbook_exponential_pattern_is_delegated_and_stays_fast() {
        let p = Pattern::compile(r"(a+)+$", PatternOptions::default()).unwrap();
        let subject = "a".repeat(60) + "!";
        assert!(p.replace_all(&subject, "x").is_ok());
    }

    #[test]
    fn a_group_reference_followed_by_text_still_means_that_group() {
        let p = Pattern::compile(r"(a)(b)", PatternOptions::default()).unwrap();
        // Rust alone would read `1x` as a group name and produce "".
        assert_eq!(p.replace_all("ab", "$1x").unwrap(), "ax");
        // `$12` is group 1 followed by a literal 2, as in JScript.
        assert_eq!(p.replace_all("ab", "$12").unwrap(), "a2");
        assert_eq!(p.replace_all("ab", "$1'$2").unwrap(), "a'b");
    }

    #[test]
    fn a_dollar_sign_that_is_not_a_group_reference_stays_literal() {
        let p = Pattern::compile(r"(a)", PatternOptions::default()).unwrap();
        assert_eq!(p.replace_all("a", "cost: $").unwrap(), "cost: $");
        assert_eq!(p.replace_all("a", "$$1").unwrap(), "$1");
        assert_eq!(p.replace_all("a", "$0").unwrap(), "$0");
    }

    #[test]
    fn skip_passes_over_the_first_n_occurrences() {
        let p = Pattern::compile("a", PatternOptions::default()).unwrap();
        assert_eq!(p.replace_skipping("aaaa", "X", 0, 0).unwrap(), "XXXX");
        assert_eq!(p.replace_skipping("aaaa", "X", 2, 0).unwrap(), "aaXX");
        // Skipping past the end leaves the subject alone.
        assert_eq!(p.replace_skipping("aaaa", "X", 9, 0).unwrap(), "aaaa");
    }

    #[test]
    fn max_limits_the_number_of_replacements_and_zero_means_unlimited() {
        let p = Pattern::compile("a", PatternOptions::default()).unwrap();
        assert_eq!(p.replace_skipping("aaaa", "X", 0, 1).unwrap(), "Xaaa");
        assert_eq!(p.replace_skipping("aaaa", "X", 0, 3).unwrap(), "XXXa");
        assert_eq!(p.replace_skipping("aaaa", "X", 0, 0).unwrap(), "XXXX");
    }

    #[test]
    fn skip_and_max_compose_left_to_right() {
        let p = Pattern::compile("a", PatternOptions::default()).unwrap();
        // Skip the first, then replace at most two of what remains.
        assert_eq!(p.replace_skipping("aaaaa", "X", 1, 2).unwrap(), "aXXaa");
    }

    #[test]
    fn find_all_reports_every_non_overlapping_match() {
        let p = Pattern::compile("ab", PatternOptions::default()).unwrap();
        assert_eq!(p.find_all("xabyabz").unwrap(), vec![1..3, 4..6]);
        assert_eq!(p.find("xabyabz").unwrap(), Some(1..3));
    }

    #[test]
    fn replacement_translation_is_exact() {
        assert_eq!(translate_replacement("$1'$2"), "${1}'${2}");
        assert_eq!(translate_replacement("$1x"), "${1}x");
        assert_eq!(translate_replacement("$$"), "$$");
        assert_eq!(translate_replacement("plain"), "plain");
        assert_eq!(translate_replacement("50$"), "50$$");
        assert_eq!(translate_replacement("$9"), "${9}");
    }
}
