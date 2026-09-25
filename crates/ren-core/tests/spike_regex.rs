//! **Spike A** — M0's regex spike.
//!
//! Question: does `fancy-regex` reproduce the JScript/VBScript regex dialect?
//! That dialect's metacharacter table is the contract, and every row of it is
//! a case below.
//!
//! The verdict lives in `docs/spikes/regex.md`; every deviation is recorded
//! under P7 in `docs/DECISIONS.md`. These tests are the executable half: if a
//! future `fancy-regex` closes one of the gaps, the corresponding test fails
//! and forces the documentation to be updated.

use ren_core::regex_flavor::{Pattern, PatternOptions, RegexError};

fn case_sensitive() -> PatternOptions {
    PatternOptions {
        case_sensitive: true,
        ..Default::default()
    }
}

/// `(pattern, subject, replacement, expected)` — replace every match.
fn check(pattern: &str, subject: &str, replacement: &str, expected: &str) {
    let compiled = Pattern::compile(pattern, case_sensitive())
        .unwrap_or_else(|e| panic!("{pattern} should compile: {e}"));
    let actual = compiled
        .replace_all(subject, replacement)
        .unwrap_or_else(|e| panic!("{pattern} should run: {e}"));
    assert_eq!(actual, expected, "pattern {pattern} on {subject:?}");
}

// --- The metacharacter table, row by row -------------------------------------

#[test]
fn backslash_marks_the_next_character_as_a_literal() {
    // `\\` matches a backslash, and `\(` a parenthesis.
    check(r"\\", r"a\b", "/", "a/b");
    check(r"\(", "a(b", "[", "a[b");
}

#[test]
fn caret_matches_the_beginning_and_dollar_the_end_of_the_input_string() {
    check("^ab", "abab", "X", "Xab");
    check("ab$", "abab", "X", "abX");
}

#[test]
fn star_plus_and_question_repeat_the_preceding_subexpression() {
    // `zo*` matches "z" and "zoo".
    check("zo*", "z zo zoo", "X", "X X X");
    // `zo+` matches "zo" and "zoo", but not "z".
    check("zo+", "z zo zoo", "X", "z X X");
    // `do(es)?` matches "do" and "does".
    check("do(es)?", "do does", "X", "X X");
}

#[test]
fn braces_repeat_an_exact_or_bounded_number_of_times() {
    // `o{2}` matches the two o's in "food" but not the one in "Bob".
    check("o{2}", "Bob food", "X", "Bob fXd");
    // `o{2,}` matches every o in "foooood" but not the one in "Bob".
    check("o{2,}", "Bob foooood", "X", "Bob fXd");
    // `o{1,3}` takes at most three o's at a time.
    check("o{1,3}", "fooooood", "X", "fXXd");
}

#[test]
fn a_trailing_question_mark_makes_a_quantifier_non_greedy() {
    // Over "oooo", `o+?` matches one o at a time, and `o+` all four at once.
    check("o+?", "oooo", "X", "XXXX");
    check("o+", "oooo", "X", "X");
}

#[test]
fn dot_matches_any_single_character_except_newline() {
    check("a.c", "abc a\nc", "X", "X a\nc");
    // A class of `.` and `\n` is how a newline is matched too.
    check("[.\n]", "a.b\nc", "X", "aXbXc");
}

#[test]
fn parentheses_capture_and_question_colon_does_not() {
    check("(ab)c", "abc", "<$1>", "<ab>");
    // `(?:…)` groups without capturing.
    check("industr(?:y|ies)", "industries", "X", "X");
}

#[test]
fn lookahead_matches_without_consuming() {
    // A positive lookahead: "Windows" only where 95, 98, NT or 2000 follows.
    check(
        "Windows (?=95|98|NT|2000)",
        "Windows 2000 Windows 3.1",
        "X",
        "X2000 Windows 3.1",
    );
    // A negative lookahead: "Windows" only where none of them follows.
    check(
        "Windows (?!95|98|NT|2000)",
        "Windows 2000 Windows 3.1",
        "X",
        "Windows 2000 X3.1",
    );
}

#[test]
fn alternation_and_character_sets_behave_as_documented() {
    // `(z|f)ood` matches "zood" and "food".
    check("(z|f)ood", "zood food", "X", "X X");
    // `[abc]` matches the a in "plain".
    check("[abc]", "plain", "X", "plXin");
    // `[^abc]` matches every other letter of it.
    check("[^abc]", "plain", "X", "XXaXX");
    check("[a-z]", "aZb", "X", "XZX");
    check("[^a-z]", "aZb", "X", "aXb");
}

#[test]
fn word_boundaries_behave_as_documented() {
    // `er\b` matches the er in "never" but not the one in "verb".
    check(r"er\b", "never verb", "X", "nevX verb");
    // `er\B` is the other way round.
    check(r"er\B", "never verb", "X", "never vXb");
}

#[test]
fn the_character_class_shorthands_behave_as_documented() {
    check(r"\d", "a1b2", "X", "aXbX");
    check(r"\D", "a1b2", "X", "X1X2");
    check(r"\w", "a_1-b", "X", "XXX-X");
    check(r"\W", "a_1-b", "X", "a_1Xb");
    check(r"\s", "a b\tc", "X", "aXbXc");
    check(r"\S", "a b", "X", "X X");
}

#[test]
fn the_control_character_escapes_behave_as_documented() {
    check(r"\f", "a\u{0c}b", "X", "aXb");
    check(r"\n", "a\nb", "X", "aXb");
    check(r"\r", "a\rb", "X", "aXb");
    check(r"\t", "a\tb", "X", "aXb");
    check(r"\v", "a\u{0b}b", "X", "aXb");
}

#[test]
fn hex_escapes_are_exactly_two_digits_long() {
    // `\x41` is "A", and `\x041` is `\x04` followed by "1": exactly two
    // hex digits.
    check(r"\x41", "ABA", "X", "XBX");
    check(r"\x041", "A1 A", "X", "A1 A");
    check(r"\x041", "\u{04}1", "X", "X");
}

#[test]
fn numbered_backreferences_work() {
    // `(.)\1` matches two identical characters in a row.
    check(r"(.)\1", "aabc", "X", "Xbc");
}

#[test]
fn capture_groups_one_to_nine_are_available_in_the_replacement() {
    check("(a)(b)(c)(d)(e)(f)(g)(h)(i)", "abcdefghi", "$9$8$1", "iha");
    // The contraction repair the shipped batch-replace list is built from.
    check(r"( don)[ `´]?(t)", " dont ", "$1'$2", " don't ");
    check(r"( don)[ `´]?(t)", " don`t ", "$1'$2", " don't ");
    check(r"( don)[ `´]?(t)", " don´t ", "$1'$2", " don't ");
}

#[test]
fn a_capture_group_reference_may_be_followed_by_text() {
    // Not in the table, but implied by `$1`–`$9`: JScript reads exactly one
    // digit. Rust does not — see translate_replacement.
    check("(a)(b)", "ab", "$1x", "ax");
    check("(a)(b)", "ab", "$12", "a2");
}

#[test]
fn case_insensitive_replacement_preserves_the_captured_casing() {
    // A case-insensitive replacement keeps the casing of what it matched.
    let p = Pattern::compile(r"( don)[ `´]?(t)", PatternOptions::default()).unwrap();
    assert_eq!(p.replace_all(" DONT ", "$1'$2").unwrap(), " DON'T ");
    assert_eq!(p.replace_all(" Dont ", "$1'$2").unwrap(), " Don't ");
}

// --- Deviations. These tests record where we do NOT match JScript. ---------

/// `\cx` — in JScript, the control character x.
///
/// `fancy-regex` rejects the escape outright. Impact: nil for filenames, which
/// cannot contain control characters on any platform we support.
#[test]
fn deviation_control_character_escapes_are_not_supported() {
    let err = Pattern::compile(r"\cM", case_sensitive()).unwrap_err();
    assert!(
        matches!(err, RegexError::InvalidPattern(ref m) if m.contains("Invalid escape")),
        "expected a parse error for \\cM, got {err:?}"
    );
}

/// `\n`, `\nm`, `\nml` — in JScript, either an octal escape or a
/// backreference.
///
/// The backreference half works. The octal fallback does not: `\7` with fewer
/// than seven groups is a compile error rather than U+0007. Impact: nil for
/// filenames; a user who wants a byte value has `\xnn`.
#[test]
fn deviation_octal_escapes_are_not_supported() {
    for pattern in [r"\7", r"\52", r"\101"] {
        let err = Pattern::compile(pattern, case_sensitive()).unwrap_err();
        assert!(
            matches!(err, RegexError::InvalidPattern(_)),
            "expected {pattern} to be rejected, got {err:?}"
        );
    }
}

/// JScript's `\w \d \s` are ASCII-only. Ours are Unicode-aware by default —
/// a deliberate widening.
///
/// `PatternOptions::unicode = false` restores exact JScript behaviour, which
/// is what makes this a *choice* rather than a limitation.
#[test]
fn deviation_shorthand_classes_are_unicode_aware_by_default() {
    let ascii = PatternOptions {
        case_sensitive: true,
        unicode: false,
        ..Default::default()
    };

    let unicode = Pattern::compile(r"\w+", case_sensitive()).unwrap();
    assert_eq!(unicode.replace_all("café über", "X").unwrap(), "X X");

    let jscript = Pattern::compile(r"\w+", ascii).unwrap();
    assert_eq!(jscript.replace_all("café über", "X").unwrap(), "Xé üX");

    let unicode = Pattern::compile(r"\d+", case_sensitive()).unwrap();
    assert_eq!(unicode.replace_all("12٣٤", "X").unwrap(), "X");

    let jscript = Pattern::compile(r"\d+", ascii).unwrap();
    assert_eq!(jscript.replace_all("12٣٤", "X").unwrap(), "X٣٤");
}

/// JScript has lookahead but no lookbehind. `fancy-regex` has both. A superset
/// costs nobody anything, but it means a pattern written for RenameIt may not
/// run under a strict JScript engine.
#[test]
fn deviation_lookbehind_is_available_although_jscript_has_none() {
    check("(?<=a)b", "ab cb", "X", "aX cb");
}

// --- The default Batch Replace list -----------------------------------------

#[derive(Debug, serde::Deserialize)]
struct BatchReplaceDefaults {
    version: u32,
    rule: Vec<LiteralRule>,
    contractions: Contractions,
}

#[derive(Debug, serde::Deserialize)]
struct LiteralRule {
    find: String,
    replace: String,
    regex: bool,
}

#[derive(Debug, serde::Deserialize)]
struct Contractions {
    stand_ins: Vec<String>,
    boundary: String,
    pairs: Vec<(String, String)>,
}

impl Contractions {
    /// `( <head>)[<stand_ins>]?(<tail>)<boundary>` — the shape documented in
    /// the data file.
    fn pattern_for(&self, head: &str, tail: &str) -> String {
        let class: String = self.stand_ins.concat();
        format!("( {head})[{class}]?({tail}){}", self.boundary)
    }
}

fn defaults() -> BatchReplaceDefaults {
    let text = include_str!("../data/batch_replace_default.toml");
    toml::from_str(text).expect("the shipped defaults must parse")
}

#[test]
fn the_shipped_batch_replace_defaults_parse_and_compile() {
    let defaults = defaults();
    assert_eq!(defaults.version, 1);

    for rule in &defaults.rule {
        if rule.regex {
            Pattern::compile(&rule.find, PatternOptions::default())
                .unwrap_or_else(|e| panic!("literal rule {:?} failed: {e}", rule.find));
        }
    }
    // The underscore rule comes first: `_` to a space, literally.
    let underscore = &defaults.rule[0];
    assert_eq!(
        (underscore.find.as_str(), underscore.replace.as_str()),
        ("_", " ")
    );
    assert!(!underscore.regex);

    assert_eq!(
        defaults.contractions.pairs.len(),
        50,
        "50 contraction rules ship"
    );
    for (head, tail) in &defaults.contractions.pairs {
        let pattern = defaults.contractions.pattern_for(head, tail);
        Pattern::compile(&pattern, PatternOptions::default())
            .unwrap_or_else(|e| panic!("contraction rule {pattern:?} failed: {e}"));
    }
}

#[test]
fn every_contraction_rule_repairs_its_own_contraction() {
    let defaults = defaults();
    for (head, tail) in &defaults.contractions.pairs {
        let pattern = Pattern::compile(
            &defaults.contractions.pattern_for(head, tail),
            PatternOptions::default(),
        )
        .unwrap();

        for stand_in in ["", " ", "`", "´"] {
            let subject = format!("track {head}{stand_in}{tail} here");
            let expected = format!("track {head}'{tail} here");
            assert_eq!(
                pattern.replace_all(&subject, "$1'$2 ").unwrap(),
                expected,
                "rule {head}'{tail} with stand-in {stand_in:?}"
            );
        }
    }
}

#[test]
fn contraction_rules_do_not_fire_inside_longer_words() {
    let defaults = defaults();
    let pattern = Pattern::compile(
        &defaults.contractions.pattern_for("don", "t"),
        PatternOptions::default(),
    )
    .unwrap();
    // "pedant" must survive; only a standalone " dont " is repaired.
    for subject in ["a pedant speaks", "abandont x", "London tower"] {
        assert_eq!(
            pattern.replace_all(subject, "$1'$2 ").unwrap(),
            subject,
            "{subject:?} should be left alone"
        );
    }
}
