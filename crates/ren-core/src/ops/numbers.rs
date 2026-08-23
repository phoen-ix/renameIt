//! Finding the numbers inside a filename.
//!
//! Re-Number and Zero
//! Padding both need to agree on what a number *is*, so the scan lives here
//! rather than in either of them:
//!
//! > Numbers are maximal digit runs inside the filename; ordinal targeting
//! > (first/second/…/last) counts these runs left-to-right. A leading hyphen is
//! > part of the number only when "Identify minus signs" is on; a period/comma
//! > between two digit runs joins them into a decimal fraction only when
//! > "Identify decimal points" is on.

use std::ops::Range;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// What counts as part of a number, as the two Re-Number checkboxes put it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NumberOptions {
    /// *"if a minus sign is found immediately in front of a number it is
    /// treated as part of the number."*
    pub minus_signs: bool,
    /// *"If a period or comma is found between two numbers it is interpreted as
    /// a decimal point."*
    pub decimal_points: bool,
}

impl NumberOptions {
    pub fn with_minus_signs(mut self, yes: bool) -> Self {
        self.minus_signs = yes;
        self
    }

    pub fn with_decimal_points(mut self, yes: bool) -> Self {
        self.decimal_points = yes;
        self
    }
}

/// One number found in a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberSpan {
    /// Byte range of the whole number, sign and decimal point included.
    pub range: Range<usize>,
    /// Digits before the decimal point — what *"zero pad to keep previous
    /// length"* preserves.
    pub int_digits: usize,
    /// Digits after it, if this number has a fraction.
    pub frac_digits: Option<usize>,
    /// The decimal character as it was written, so output can match input.
    pub separator: Option<char>,
    pub negative: bool,
    /// `None` for a run too long for a 96-bit decimal — it can still be padded,
    /// just not added to.
    pub value: Option<Decimal>,
}

impl NumberSpan {
    pub fn text<'a>(&self, subject: &'a str) -> &'a str {
        &subject[self.range.clone()]
    }
}

/// Every number in `subject`, left to right.
pub fn scan(subject: &str, options: NumberOptions) -> Vec<NumberSpan> {
    let bytes = subject.as_bytes();

    // Maximal digit runs first — the same rule with every option off.
    let mut runs: Vec<Range<usize>> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_digit() {
            let start = at;
            while at < bytes.len() && bytes[at].is_ascii_digit() {
                at += 1;
            }
            runs.push(start..at);
        } else {
            at += 1;
        }
    }

    let mut spans: Vec<NumberSpan> = Vec::with_capacity(runs.len());
    let mut index = 0;
    while index < runs.len() {
        let run = runs[index].clone();
        let mut span = NumberSpan {
            int_digits: run.len(),
            range: run,
            frac_digits: None,
            separator: None,
            negative: false,
            value: None,
        };

        // "If a period or comma is found between two numbers" — exactly one
        // separator character, and only one fraction per number, so `1.2.3`
        // stays a version string rather than becoming nonsense.
        if options.decimal_points
            && span.range.end < bytes.len()
            && let Some(next) = runs.get(index + 1)
            && next.start == span.range.end + 1
        {
            let separator = bytes[span.range.end] as char;
            if separator == '.' || separator == ',' {
                span.frac_digits = Some(next.len());
                span.separator = Some(separator);
                span.range.end = next.end;
                index += 1;
            }
        }

        // "if a minus sign is found immediately in front of a number"
        if options.minus_signs && span.range.start > 0 && bytes[span.range.start - 1] == b'-' {
            span.negative = true;
            span.range.start -= 1;
        }

        span.value = parse_value(&subject[span.range.clone()], span.separator);
        spans.push(span);
        index += 1;
    }
    spans
}

/// Parses the matched text, with whichever decimal character it used.
fn parse_value(text: &str, separator: Option<char>) -> Option<Decimal> {
    let normalised = match separator {
        Some(',') => text.replace(',', "."),
        _ => text.to_owned(),
    };
    normalised.parse::<Decimal>().ok()
}

/// *"With … in the filename"* — which of the numbers to process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberTarget {
    #[default]
    All,
    /// `the first number` … `the 9:th number`, counted from 1.
    Nth(usize),
    SecondToLast,
    Last,
}

impl NumberTarget {
    /// The dropdown, verbatim and in order.
    pub const LABELS: [&'static str; 12] = [
        "all numbers",
        "the first number",
        "the second number",
        "the third number",
        "the 4:th number",
        "the 5:th number",
        "the 6:th number",
        "the 7:th number",
        "the 8:th number",
        "the 9:th number",
        "the 2nd to last number",
        "the last number",
    ];

    pub fn all() -> Vec<Self> {
        let mut targets = vec![Self::All];
        targets.extend((1..=9).map(Self::Nth));
        targets.push(Self::SecondToLast);
        targets.push(Self::Last);
        targets
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => Self::LABELS[0],
            Self::Nth(n) if (1..=9).contains(&n) => Self::LABELS[n],
            Self::Nth(_) => "a number past the ninth",
            Self::SecondToLast => Self::LABELS[10],
            Self::Last => Self::LABELS[11],
        }
    }

    /// Which of `count` numbers this selects. Out of range selects nothing,
    /// which leaves the file alone.
    pub fn selects(self, index: usize, count: usize) -> bool {
        match self {
            Self::All => true,
            Self::Nth(n) => n >= 1 && index + 1 == n,
            Self::SecondToLast => count >= 2 && index + 2 == count,
            Self::Last => count >= 1 && index + 1 == count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(subject: &str, options: NumberOptions) -> Vec<String> {
        scan(subject, options)
            .iter()
            .map(|s| s.text(subject).to_owned())
            .collect()
    }

    #[test]
    fn numbers_are_maximal_digit_runs() {
        assert_eq!(
            texts("Track 12 of 100", NumberOptions::default()),
            ["12", "100"]
        );
        // The scan sees exactly the slice it is given, extension included —
        // hiding the extension is the engine's job (Scope), not the scanner's.
        assert_eq!(texts("Track 1.mp3", NumberOptions::default()), ["1", "3"]);
        assert_eq!(
            texts("no numbers here", NumberOptions::default()),
            Vec::<String>::new()
        );
        assert_eq!(texts("007", NumberOptions::default()), ["007"]);
    }

    /// "if a minus sign is found immediately in front of a number it is treated
    /// as part of the number" — and otherwise "this hyphen is simply ignored".
    #[test]
    fn a_minus_sign_joins_the_number_only_when_asked() {
        let plain = NumberOptions::default();
        let signed = NumberOptions::default().with_minus_signs(true);

        assert_eq!(texts("temp -5 C", plain), ["5"]);
        assert_eq!(texts("temp -5 C", signed), ["-5"]);
        assert_eq!(texts("a - 5", signed), ["5"], "not immediately in front");
        assert!(scan("temp -5 C", signed)[0].negative);
    }

    /// "If a period or comma is found between two numbers it is interpreted as
    /// a decimal point."
    #[test]
    fn a_decimal_point_joins_two_runs_only_when_asked() {
        let plain = NumberOptions::default();
        let decimal = NumberOptions::default().with_decimal_points(true);

        assert_eq!(texts("price 12.50 eur", plain), ["12", "50"]);
        assert_eq!(texts("price 12.50 eur", decimal), ["12.50"]);
        assert_eq!(texts("price 12,50 eur", decimal), ["12,50"]);
        assert_eq!(texts("v1.2.3", decimal), ["1.2", "3"], "one fraction only");
        assert_eq!(texts("12 .50", decimal), ["12", "50"], "not between them");
    }

    #[test]
    fn the_parsed_value_survives_both_decimal_characters() {
        let decimal = NumberOptions::default().with_decimal_points(true);
        let spans = scan("a 12,50 b 12.50", decimal);
        assert_eq!(spans[0].value, spans[1].value);
        assert_eq!(spans[0].separator, Some(','));
        assert_eq!(spans[1].separator, Some('.'));
        assert_eq!(spans[0].int_digits, 2);
        assert_eq!(spans[0].frac_digits, Some(2));
    }

    #[test]
    fn a_signed_fraction_reads_as_one_number() {
        let both = NumberOptions::default()
            .with_minus_signs(true)
            .with_decimal_points(true);
        let spans = scan("delta -3.75 units", both);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text("delta -3.75 units"), "-3.75");
        assert_eq!(spans[0].value, Some("-3.75".parse().unwrap()));
    }

    #[test]
    fn a_run_too_long_for_a_decimal_still_has_a_span() {
        let spans = scan(&"9".repeat(40), NumberOptions::default());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].int_digits, 40);
        assert_eq!(spans[0].value, None, "too big to do arithmetic on");
    }

    /// The dropdown, verbatim.
    #[test]
    fn every_target_has_the_label_the_original_shows() {
        let all = NumberTarget::all();
        assert_eq!(all.len(), NumberTarget::LABELS.len());
        for (target, label) in all.iter().zip(NumberTarget::LABELS) {
            assert_eq!(target.label(), label);
        }
    }

    #[test]
    fn ordinal_targets_pick_the_number_they_name() {
        let count = 4;
        let picked = |t: NumberTarget| -> Vec<usize> {
            (0..count).filter(|&i| t.selects(i, count)).collect()
        };
        assert_eq!(picked(NumberTarget::All), [0, 1, 2, 3]);
        assert_eq!(picked(NumberTarget::Nth(1)), [0]);
        assert_eq!(picked(NumberTarget::Nth(3)), [2]);
        assert_eq!(picked(NumberTarget::Last), [3]);
        assert_eq!(picked(NumberTarget::SecondToLast), [2]);
    }

    #[test]
    fn a_target_that_is_not_there_selects_nothing() {
        assert!(!NumberTarget::Nth(9).selects(0, 1));
        assert!(!NumberTarget::SecondToLast.selects(0, 1));
        assert!(!NumberTarget::Last.selects(0, 0));
    }
}
