//! Set Casing.
//!
//! Set Casing, plus three word-splitting rules that live as data (D20) — see
//! `crates/ren-core/data/casing_default.toml`.
//!
//! **D19:** scope is engine-owned and uniform, so this operation carries a
//! *single* mode rather than one selector for the name and another for the
//! extension. Two selectors would be two
//! pipeline steps (`scope = "name"` and `scope = "extension"`). The extension
//! panel's `Capitalize` is the same transform as the name panel's `Sentence
//! case`, and is accepted as an alias.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};

/// *"UPPER CASE, lower case, Sentance case & Title Case, plus the less used
/// iNVERT and rANdOm"* — plus `No change`, which both panels also offer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseMode {
    Upper,
    Lower,
    /// First letter up, the rest down. The extension panel calls this
    /// `Capitalize`.
    #[serde(alias = "capitalize")]
    Sentence,
    #[default]
    Title,
    NoChange,
    /// `iNVERT` — swap the case of every letter.
    Invert,
    /// `rANdOm` — deterministic given [`Casing::seed`], so a preview and the
    /// execute that follows it can never disagree.
    Random,
}

/// Kept for callers that want to name the two panels explicitly.
pub type NameCase = CaseMode;
/// Kept for callers that want to name the two panels explicitly.
pub type ExtensionCase = CaseMode;

/// The data-driven half of casing: word lists and character sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CasingRules {
    pub title_case: TitleCaseRules,
    pub exceptions: ExceptionRules,
    pub space: SpaceRules,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TitleCaseRules {
    pub lowercase_exceptions: Vec<String>,
    pub capitalize_after: String,
    pub capitalize_after_quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionRules {
    pub words: Vec<String>,
}

/// Space Trimming's defaults live in the same file; see [`super::space_trim`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceRules {
    pub maintain_before: String,
    pub maintain_after: String,
}

#[derive(Debug, Deserialize)]
struct ShippedFile {
    #[allow(dead_code)]
    version: u32,
    title_case: TitleCaseRules,
    exceptions: ExceptionRules,
    space: SpaceRules,
}

/// The defaults we ship, parsed once.
pub fn shipped_rules() -> &'static CasingRules {
    static RULES: std::sync::OnceLock<CasingRules> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        let text = include_str!("../../data/casing_default.toml");
        let parsed: ShippedFile =
            toml::from_str(text).expect("the shipped casing defaults must parse");
        CasingRules {
            title_case: parsed.title_case,
            exceptions: parsed.exceptions,
            space: parsed.space,
        }
    })
}

impl Default for CasingRules {
    fn default() -> Self {
        shipped_rules().clone()
    }
}

/// *"This function allows you to change casing (upper or lower) of the letters
/// within filenames."*
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Casing {
    pub mode: CaseMode,
    /// *"In accorance with common English spelling rules, some words will
    /// always be set to lower case, in order to improve readability."*
    /// Title Case only, and never the first word.
    pub lowercase_exceptions: bool,
    /// *"If it is spelled in all upper case, then it will retain this no matter
    /// what."*
    pub preserve_all_upper: bool,
    /// *"all words with one or more capitalized letter will retain its current
    /// state"*
    pub preserve_mixed: bool,
    /// *"Exceptions are words that should always be spelled with a certain
    /// case."*
    pub use_exceptions: bool,
    pub rules: CasingRules,
    /// Only consulted by [`CaseMode::Random`].
    pub seed: u64,
}

impl Default for Casing {
    fn default() -> Self {
        Self {
            mode: CaseMode::Title,
            lowercase_exceptions: false,
            preserve_all_upper: false,
            preserve_mixed: false,
            // "Enable exceptions" ships checked.
            use_exceptions: true,
            rules: CasingRules::default(),
            seed: 0x5265_6e61_6d65_4974,
        }
    }
}

impl Casing {
    pub fn new(mode: CaseMode) -> Self {
        Self {
            mode,
            ..Default::default()
        }
    }

    /// A character that ends a word. Space plus the recovered
    /// `capitalize_after` set — which is precisely why a letter after `(` or
    /// `-` gets capitalised in Title Case: it starts a new word.
    fn is_separator(&self, c: char) -> bool {
        c.is_whitespace() || self.rules.title_case.capitalize_after.contains(c)
    }

    fn exception_for(&self, word: &str) -> Option<&str> {
        if !self.use_exceptions {
            return None;
        }
        self.rules
            .exceptions
            .words
            .iter()
            .find(|w| w.eq_ignore_ascii_case(word))
            .map(String::as_str)
    }

    fn is_lowercase_exception(&self, word: &str) -> bool {
        self.rules
            .title_case
            .lowercase_exceptions
            .iter()
            .any(|w| w.eq_ignore_ascii_case(word))
    }

    /// *"the program will first inspect what casing each word is currently
    /// using"* — so both preserve rules look at the **original** word.
    fn preserved(&self, word: &str) -> bool {
        let has_letter = word.chars().any(char::is_alphabetic);
        if !has_letter {
            return false;
        }
        let has_lower = word.chars().any(char::is_lowercase);
        let has_upper = word.chars().any(char::is_uppercase);
        if self.preserve_all_upper && !has_lower {
            return true;
        }
        if self.preserve_mixed && has_upper {
            return true;
        }
        false
    }

    fn render_word(&self, word: &str, is_first: bool, offset: usize) -> Cow<'_, str> {
        if word.is_empty() {
            return Cow::Borrowed("");
        }
        if self.preserved(word) {
            return Cow::Owned(word.to_owned());
        }
        if let Some(fixed) = self.exception_for(word) {
            return Cow::Owned(fixed.to_owned());
        }
        if self.mode == CaseMode::Title
            && self.lowercase_exceptions
            && !is_first
            && self.is_lowercase_exception(word)
        {
            return Cow::Owned(word.to_lowercase());
        }

        Cow::Owned(match self.mode {
            CaseMode::Upper => word.to_uppercase(),
            CaseMode::Lower => word.to_lowercase(),
            CaseMode::NoChange => word.to_owned(),
            CaseMode::Invert => word
                .chars()
                .flat_map(|c| {
                    if c.is_uppercase() {
                        Either::Left(c.to_lowercase())
                    } else {
                        Either::Right(c.to_uppercase())
                    }
                })
                .collect(),
            CaseMode::Random => word
                .chars()
                .enumerate()
                .flat_map(|(i, c)| {
                    if coin_flip(self.seed, offset + i) {
                        Either::Left(c.to_uppercase())
                    } else {
                        Either::Right(c.to_lowercase())
                    }
                })
                .collect(),
            // Sentence case capitalises the first word only; Title Case
            // capitalises every word. Both lower-case the remainder.
            CaseMode::Sentence if !is_first => word.to_lowercase(),
            CaseMode::Sentence | CaseMode::Title => self.capitalize(word),
        })
    }

    /// Upper-cases the word's leading letter and lower-cases the rest.
    ///
    /// The leading letter is the first character when that is a letter. When it
    /// is not, we look past it only if everything before the first letter is a
    /// quote-like character — the recovered `CapitalizeAfterWhenPrevIsSpace`
    /// rule. That is what turns `'best'` into `'Best'` while leaving `1st`
    /// alone instead of producing `1St`.
    fn capitalize(&self, word: &str) -> String {
        let quotes = &self.rules.title_case.capitalize_after_quote;
        let mut out = String::with_capacity(word.len());
        let mut done = false;
        for c in word.chars() {
            if done {
                out.extend(c.to_lowercase());
            } else if c.is_alphabetic() {
                out.extend(c.to_uppercase());
                done = true;
            } else if quotes.contains(c) {
                out.push(c);
            } else {
                // A digit or other character that is not a quote: this word
                // does not get a capital at all.
                out.push(c);
                done = true;
            }
        }
        out
    }
}

/// `flat_map` needs one iterator type; `to_uppercase` and `to_lowercase` are
/// different ones.
enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L: Iterator<Item = char>, R: Iterator<Item = char>> Iterator for Either<L, R> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        match self {
            Either::Left(l) => l.next(),
            Either::Right(r) => r.next(),
        }
    }
}

/// SplitMix64, so `Random` is reproducible from the seed alone.
fn coin_flip(seed: u64, position: usize) -> bool {
    let mut z = seed.wrapping_add((position as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) & 1 == 1
}

impl NameTransform for Casing {
    fn id(&self) -> &'static str {
        "casing"
    }

    fn summary(&self) -> String {
        format!("Set casing: {:?}", self.mode)
    }

    fn apply<'a>(&self, subject: &'a str, _cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        if self.mode == CaseMode::NoChange || subject.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }

        let mut out = String::with_capacity(subject.len());
        let mut word_start: Option<usize> = None;
        let mut first_word_done = false;

        for (i, c) in subject.char_indices() {
            if self.is_separator(c) {
                if let Some(start) = word_start.take() {
                    let word = &subject[start..i];
                    out.push_str(&self.render_word(word, !first_word_done, start));
                    first_word_done = true;
                }
                out.push(c);
            } else if word_start.is_none() {
                word_start = Some(i);
            }
        }
        if let Some(start) = word_start {
            let word = &subject[start..];
            out.push_str(&self.render_word(word, !first_word_done, start));
        }

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

    fn cased(mode: CaseMode, input: &str) -> String {
        run(&Casing::new(mode), input)
    }

    #[test]
    fn the_shipped_rules_parse_and_carry_the_recovered_values() {
        let rules = shipped_rules();
        assert!(
            rules
                .title_case
                .lowercase_exceptions
                .contains(&"the".into())
        );
        assert!(rules.title_case.capitalize_after.contains('('));
        assert!(rules.title_case.capitalize_after.contains('-'));
        assert!(rules.title_case.capitalize_after_quote.contains('\''));
        assert!(rules.exceptions.words.contains(&"DJ".into()));
        assert_eq!(rules.space.maintain_before, "([{");
        assert_eq!(rules.space.maintain_after, ")]};,");
    }

    /// "UPPER CASE, lower case"
    #[test]
    fn upper_and_lower_case_convert_the_whole_slice() {
        assert_eq!(cased(CaseMode::Upper, "hello world"), "HELLO WORLD");
        assert_eq!(cased(CaseMode::Lower, "HELLO World"), "hello world");
    }

    /// "Sentance case"
    #[test]
    fn sentence_case_capitalises_only_the_first_word() {
        assert_eq!(
            cased(CaseMode::Sentence, "hello BIG world"),
            "Hello big world"
        );
    }

    /// "Title Case"
    #[test]
    fn title_case_capitalises_every_word() {
        assert_eq!(cased(CaseMode::Title, "hello big world"), "Hello Big World");
    }

    /// The recovered `CapitalizeAfter` list, which nothing documents.
    #[test]
    fn title_case_capitalises_after_brackets_and_punctuation() {
        assert_eq!(cased(CaseMode::Title, "rock (live)"), "Rock (Live)");
        assert_eq!(cased(CaseMode::Title, "hi-fi"), "Hi-Fi");
        assert_eq!(cased(CaseMode::Title, "a_b.c,d!e"), "A_B.C,D!E");
        assert_eq!(cased(CaseMode::Title, "[demo] track"), "[Demo] Track");
    }

    /// The recovered `CapitalizeAfterWhenPrevIsSpace` list — the reason
    /// apostrophes are handled separately from the punctuation above.
    #[test]
    fn title_case_does_not_capitalise_after_an_apostrophe_inside_a_word() {
        assert_eq!(cased(CaseMode::Title, "don't stop"), "Don't Stop");
        assert_eq!(cased(CaseMode::Title, "rock 'n' roll"), "Rock 'N' Roll");
    }

    #[test]
    fn title_case_leaves_a_word_starting_with_a_digit_alone() {
        assert_eq!(cased(CaseMode::Title, "1st place"), "1st Place");
    }

    /// "the less used iNVERT and rANdOm"
    #[test]
    fn invert_swaps_the_case_of_every_letter() {
        assert_eq!(cased(CaseMode::Invert, "Hello World"), "hELLO wORLD");
    }

    #[test]
    fn no_change_is_the_identity() {
        assert_eq!(cased(CaseMode::NoChange, "LeAvE mE"), "LeAvE mE");
    }

    /// Preview and execute must never disagree, so Random is seeded.
    #[test]
    fn random_is_deterministic_for_a_given_seed() {
        let op = Casing::new(CaseMode::Random);
        let once = run(&op, "abcdefghij");
        let twice = run(&op, "abcdefghij");
        assert_eq!(once, twice);
        assert_eq!(once.to_lowercase(), "abcdefghij");

        let other = Casing {
            seed: 12345,
            ..Casing::new(CaseMode::Random)
        };
        assert_ne!(run(&other, "abcdefghij"), once, "a new seed must reshuffle");
    }

    /// "If it is spelled in all upper case, then it will retain this no matter
    /// what. This is typically useful when dealing with abbreviations."
    #[test]
    fn preserve_all_upper_case_words_keeps_abbreviations() {
        let op = Casing {
            preserve_all_upper: true,
            use_exceptions: false,
            ..Casing::new(CaseMode::Title)
        };
        assert_eq!(run(&op, "the BBC news"), "The BBC News");

        let op = Casing {
            preserve_all_upper: false,
            ..op
        };
        assert_eq!(run(&op, "the BBC news"), "The Bbc News");
    }

    /// "all words with one or more capitalized letter will retain its current
    /// state"
    #[test]
    fn preserve_mixed_case_words_keeps_them_verbatim() {
        let op = Casing {
            preserve_mixed: true,
            use_exceptions: false,
            ..Casing::new(CaseMode::Lower)
        };
        assert_eq!(run(&op, "iPhone AND macBook"), "iPhone AND macBook");
        assert_eq!(run(&op, "plain words"), "plain words");
    }

    /// "Exceptions are words that should always be spelled with a certain case.
    /// This is typically used for abbreviations, such a DJ and CD. Also Roman
    /// numbers, eg III"
    #[test]
    fn the_exceptions_list_forces_a_fixed_spelling() {
        let op = Casing::new(CaseMode::Title);
        assert_eq!(run(&op, "best of dj mix"), "Best Of DJ Mix");
        assert_eq!(run(&op, "part iii"), "Part III");
        assert_eq!(run(&op, "cd1 tracks"), "CD1 Tracks");

        let op = Casing {
            use_exceptions: false,
            ..op
        };
        assert_eq!(run(&op, "best of dj mix"), "Best Of Dj Mix");
    }

    /// "some words will always be set to lower case, in order to improve
    /// readability"
    #[test]
    fn lowercase_exceptions_apply_to_title_case_but_never_the_first_word() {
        let op = Casing {
            lowercase_exceptions: true,
            ..Casing::new(CaseMode::Title)
        };
        assert_eq!(run(&op, "the lord of the rings"), "The Lord of the Rings");
        // "of" leading the name still gets a capital.
        assert_eq!(run(&op, "of mice and men"), "Of Mice and Men");
    }

    #[test]
    fn lowercase_exceptions_do_not_apply_outside_title_case() {
        let op = Casing {
            lowercase_exceptions: true,
            ..Casing::new(CaseMode::Upper)
        };
        assert_eq!(run(&op, "the lord of the rings"), "THE LORD OF THE RINGS");
    }

    #[test]
    fn separators_and_spacing_survive_untouched() {
        assert_eq!(cased(CaseMode::Title, "  a  b  "), "  A  B  ");
        assert_eq!(cased(CaseMode::Upper, "a---b"), "A---B");
    }

    #[test]
    fn non_ascii_letters_are_cased_too() {
        assert_eq!(cased(CaseMode::Upper, "über straße"), "ÜBER STRASSE");
        assert_eq!(cased(CaseMode::Title, "über straße"), "Über Straße");
    }
}
