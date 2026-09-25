//! Set Casing, plus the word-splitting rules that live as data (D20) — see
//! `crates/ren-core/data/casing_default.toml`.
//!
//! **D19:** scope is engine-owned and uniform, so this operation carries a
//! *single* mode rather than one selector for the name and another for the
//! extension. Casing both is two pipeline steps (`scope = "name"` and
//! `scope = "extension"`). `capitalize` is accepted as another spelling of
//! `sentence`: first letter up, the rest down is the same transform whichever
//! part of the name it is applied to.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};

/// UPPER CASE, lower case, Sentence case, Title Case, iNVERT and rANdOm — plus
/// `No change`, so a card can be left in place and switched off by mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseMode {
    Upper,
    Lower,
    /// First letter up, the rest down. Also read as `capitalize`, the word
    /// for the same transform on an extension.
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

/// The data-driven half of casing: word lists and character sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CasingRules {
    pub title_case: TitleCaseRules,
    pub exceptions: ExceptionRules,
    /// Space Trimming's defaults used to ride along here, copied into every
    /// Casing step, every preset and the saved settings — and nothing read
    /// them: Space Trimming takes its defaults from [`shipped_space`].
    ///
    /// Still *accepted*, because `deny_unknown_fields` would otherwise refuse
    /// every preset and job file with a Casing step written before, and the
    /// GUI's saved state would fail to load and silently reset. Never written.
    #[serde(default, rename = "space", skip_serializing)]
    retired_space: Retired,
}

/// A value that is read and thrown away — what a retired field deserialises
/// into.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Retired;

impl<'de> Deserialize<'de> for Retired {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer).map(|_| Self)
    }
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

/// Space Trimming's defaults, which live in the same data file; see
/// [`super::space_trim`] and [`shipped_space`].
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

/// The data file, parsed once.
fn shipped() -> &'static (CasingRules, SpaceRules) {
    static SHIPPED: std::sync::OnceLock<(CasingRules, SpaceRules)> = std::sync::OnceLock::new();
    SHIPPED.get_or_init(|| {
        let text = include_str!("../../data/casing_default.toml");
        let parsed: ShippedFile =
            toml::from_str(text).expect("the shipped casing defaults must parse");
        let rules = CasingRules {
            title_case: parsed.title_case,
            exceptions: parsed.exceptions,
            retired_space: Retired,
        };
        (rules, parsed.space)
    })
}

/// The casing defaults we ship.
pub fn shipped_rules() -> &'static CasingRules {
    &shipped().0
}

/// Space Trimming's defaults, from the same file.
pub fn shipped_space() -> &'static SpaceRules {
    &shipped().1
}

impl Default for CasingRules {
    fn default() -> Self {
        shipped_rules().clone()
    }
}

/// Changes the case of the letters in a name, word by word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Casing {
    pub mode: CaseMode,
    /// Keep the short words English titles leave in lower case — "of",
    /// "the", "and" — in lower case. Title Case only, and never the first
    /// word.
    pub lowercase_exceptions: bool,
    /// A word already in all capitals keeps them — an abbreviation such as
    /// `BBC` survives Title Case.
    pub preserve_all_upper: bool,
    /// A word with any capital letter in it keeps its casing exactly —
    /// `iPhone`, `macBook`.
    pub preserve_mixed: bool,
    /// Words that always take a fixed spelling, whatever the mode:
    /// abbreviations (`DJ`, `CD`), Roman numerals (`III`).
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

    /// A character that ends a word. Space plus the `capitalize_after` set
    /// (D20) — which is precisely why a letter after `(` or `-` gets
    /// capitalised in Title Case: it starts a new word.
    fn is_separator(&self, c: char) -> bool {
        c.is_whitespace() || self.rules.title_case.capitalize_after.contains(c)
    }

    fn exception_for(&self, word: &str) -> Option<&str> {
        if !self.use_exceptions {
            return None;
        }
        let ascii = word.is_ascii();
        self.rules
            .exceptions
            .words
            .iter()
            .find(|entry| same_word(entry, word, ascii))
            .map(String::as_str)
    }

    fn is_lowercase_exception(&self, word: &str) -> bool {
        let ascii = word.is_ascii();
        self.rules
            .title_case
            .lowercase_exceptions
            .iter()
            .any(|entry| same_word(entry, word, ascii))
    }

    /// Both preserve rules look at the word as it **was**, before this
    /// operation changed anything — preserving is about the casing a word
    /// arrived with.
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
    /// quote-like character — the `capitalize_after_quote` list (D20). That is
    /// what turns `'best'` into `'Best'` while leaving `1st` alone instead of
    /// producing `1St`.
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

/// Whether a list entry and a word are the same word in any case.
///
/// Unicode-aware, because both lists are editable in Settings and hold words
/// in whatever language the user renames in: an ASCII-only comparison left
/// `À` unmatched by an `à` entry and `été` by an `ÉTÉ` one. Compared character
/// by character without allocating, since this runs for every word of every
/// name against every entry — and plain ASCII, the common case by far, takes
/// the cheap path, with the word's own check (`word_is_ascii`) made once per
/// word rather than once per entry.
fn same_word(entry: &str, word: &str, word_is_ascii: bool) -> bool {
    if word_is_ascii && entry.is_ascii() {
        return entry.eq_ignore_ascii_case(word);
    }
    entry
        .chars()
        .flat_map(char::to_lowercase)
        .eq(word.chars().flat_map(char::to_lowercase))
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
    fn the_shipped_rules_parse_and_carry_the_shipped_values() {
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
        assert_eq!(shipped_space().maintain_before, "([{");
        assert_eq!(shipped_space().maintain_after, ")]};,");
    }

    /// A Casing step written while the space defaults still rode along in its
    /// rules parses, and is written back without them.
    #[test]
    fn a_step_written_with_the_retired_space_table_still_parses() {
        let mut table = toml::Table::try_from(Casing::default()).unwrap();
        let rules = table["rules"].as_table_mut().unwrap();
        assert!(!rules.contains_key("space"), "never written");
        rules.insert(
            "space".into(),
            toml::toml! { maintain_before = "([{"
            maintain_after = ")]};," }
            .into(),
        );
        let back: Casing = table.try_into().unwrap();
        assert_eq!(back, Casing::default());

        // A self-describing format, like the one the GUI saves its state in.
        let mut json = serde_json::to_value(Casing::default()).unwrap();
        json["rules"]["space"] =
            serde_json::json!({ "maintain_before": "(", "maintain_after": ")" });
        let back: Casing = serde_json::from_value(json).unwrap();
        assert_eq!(back, Casing::default());
    }

    #[test]
    fn upper_and_lower_case_convert_the_whole_slice() {
        assert_eq!(cased(CaseMode::Upper, "hello world"), "HELLO WORLD");
        assert_eq!(cased(CaseMode::Lower, "HELLO World"), "hello world");
    }

    #[test]
    fn sentence_case_capitalises_only_the_first_word() {
        assert_eq!(
            cased(CaseMode::Sentence, "hello BIG world"),
            "Hello big world"
        );
    }

    #[test]
    fn title_case_capitalises_every_word() {
        assert_eq!(cased(CaseMode::Title, "hello big world"), "Hello Big World");
    }

    /// The `capitalize_after` list (D20): a letter after one of these starts a
    /// word.
    #[test]
    fn title_case_capitalises_after_brackets_and_punctuation() {
        assert_eq!(cased(CaseMode::Title, "rock (live)"), "Rock (Live)");
        assert_eq!(cased(CaseMode::Title, "hi-fi"), "Hi-Fi");
        assert_eq!(cased(CaseMode::Title, "a_b.c,d!e"), "A_B.C,D!E");
        assert_eq!(cased(CaseMode::Title, "[demo] track"), "[Demo] Track");
    }

    /// The `capitalize_after_quote` list (D20) — the reason
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

    /// Abbreviations are the reason this option exists.
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

    /// Abbreviations and Roman numerals, spelled one way whatever the mode.
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

    /// Both lists are editable in Settings, so they hold whatever language the
    /// user writes in, and a word matches its entry in any case.
    #[test]
    fn exception_lists_match_non_ascii_words_in_any_case() {
        let mut op = Casing {
            lowercase_exceptions: true,
            ..Casing::new(CaseMode::Title)
        };
        op.rules.title_case.lowercase_exceptions = vec!["à".into()];
        op.rules.exceptions.words = vec!["ÉTÉ".into()];
        assert_eq!(run(&op, "voyage À paris été"), "Voyage à Paris ÉTÉ");
    }

    #[test]
    fn non_ascii_letters_are_cased_too() {
        assert_eq!(cased(CaseMode::Upper, "über straße"), "ÜBER STRASSE");
        assert_eq!(cased(CaseMode::Title, "über straße"), "Über Straße");
    }
}
