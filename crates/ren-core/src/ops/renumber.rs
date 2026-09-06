//! Re-Number — *"Can pinpoint numbers inside your filenames and process
//! them."*
//!
//! Re-Number. Three
//! stages, exactly as the panel reads top to bottom: pick which numbers, limit
//! them by value, then do one thing to each.
//!
//! Arithmetic runs on `rust_decimal`, not `f64`: a renamer that turns `1.10`
//! into `1.1000000000000001` has failed at its one job.

use std::borrow::Cow;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};

use super::numbers::{NumberOptions, NumberTarget, scan};
use super::{EvalCx, NameTransform, OpError};
use crate::template::TextTemplate;

/// *"Select what to do with the number"*.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberAction {
    ReplaceWithCounter,
    #[default]
    ReplaceWith,
    InsertBefore,
    InsertAfter,
    ZeroPadTo,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remove,
    RoundTo,
}

impl NumberAction {
    /// The dropdown in order.
    pub const LABELS: [&'static str; 11] = [
        "Replace with Counter",
        "Replace with:",
        "Insert this before:",
        "Insert this after:",
        "Zero pad to:",
        "[+] Add:",
        "[-] Subtract:",
        "[x] Multiply by:",
        "[/] Divide by:",
        "Remove",
        "Round to:",
    ];

    pub fn all() -> [Self; 11] {
        [
            Self::ReplaceWithCounter,
            Self::ReplaceWith,
            Self::InsertBefore,
            Self::InsertAfter,
            Self::ZeroPadTo,
            Self::Add,
            Self::Subtract,
            Self::Multiply,
            Self::Divide,
            Self::Remove,
            Self::RoundTo,
        ]
    }

    pub fn label(self) -> &'static str {
        Self::LABELS[Self::all().iter().position(|a| *a == self).unwrap_or(0)]
    }

    /// Whether the operand field is used at all — the two that are not are the
    /// two whose labels carry no colon.
    pub fn needs_operand(self) -> bool {
        !matches!(self, Self::ReplaceWithCounter | Self::Remove)
    }

    /// Whether the operand has to be a number.
    pub fn needs_number(self) -> bool {
        matches!(
            self,
            Self::ZeroPadTo
                | Self::Add
                | Self::Subtract
                | Self::Multiply
                | Self::Divide
                | Self::RoundTo
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReNumber {
    /// *"With … in the filename"*.
    pub target: NumberTarget,
    /// *"only process numbers that are >= to the specified number"*.
    pub at_least: Option<i64>,
    /// *"only process numbers that are <= to the specified number"*.
    pub at_most: Option<i64>,
    pub action: NumberAction,
    /// The one operand field, which *"supports `<tags>`"* — so a number can be
    /// replaced with `<Counter>`, `<Parent>` or anything else.
    pub operand: TextTemplate,
    pub numbers: NumberOptions,
    /// *"Numbers will be padded with zeros to make sure that their new length
    /// is the same as the old one."* — *"(Not available when using decimal
    /// fractions.)"*
    pub keep_length: bool,
    /// Advanced setting `ReNumberDecimalRounding`: *"the number of digits after
    /// the decimal point, or -1 for no rounding (default)"*.
    ///
    /// **P31.** The Re-Number checkbox tooltip claims 3 decimals by default and
    /// the advanced setting claims none; the advanced setting is the one that
    /// says "(default)" next to the value, so it wins. Rounding silently is the
    /// more destructive of the two readings, which settles the tie.
    pub rounding: Option<u32>,
}

impl ReNumber {
    pub fn new(target: NumberTarget, action: NumberAction) -> Self {
        Self {
            target,
            action,
            ..Default::default()
        }
    }

    pub fn with_operand(mut self, operand: impl Into<TextTemplate>) -> Self {
        self.operand = operand.into();
        self
    }

    pub fn between(mut self, at_least: Option<i64>, at_most: Option<i64>) -> Self {
        self.at_least = at_least;
        self.at_most = at_most;
        self
    }

    pub fn keeping_length(mut self, yes: bool) -> Self {
        self.keep_length = yes;
        self
    }

    pub fn identifying(mut self, options: NumberOptions) -> Self {
        self.numbers = options;
        self
    }

    /// The value filters, which only ever *narrow* the selection.
    fn in_range(&self, value: Option<Decimal>) -> bool {
        if self.at_least.is_none() && self.at_most.is_none() {
            return true;
        }
        let Some(value) = value else {
            return false; // No value means nothing to compare.
        };
        self.at_least.is_none_or(|low| value >= Decimal::from(low))
            && self.at_most.is_none_or(|high| value <= Decimal::from(high))
    }
}

impl NameTransform for ReNumber {
    fn id(&self) -> &'static str {
        "renumber"
    }

    fn summary(&self) -> String {
        let mut summary = format!("With {} {}", self.target.label(), self.action.label());
        if self.action.needs_operand() {
            summary.push(' ');
            summary.push_str(&format!("{:?}", self.operand.as_str()));
        }
        summary
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        let spans = scan(subject, self.numbers);
        if spans.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }

        // The operand is the same for every number in this name, so it is
        // rendered once rather than once per span.
        let operand = if self.action.needs_operand() {
            match cx.render(&self.operand)? {
                Some(text) => text,
                None => return Ok(Cow::Borrowed(subject)),
            }
        } else {
            Cow::Borrowed("")
        };

        let count = spans.len();
        let mut out = String::with_capacity(subject.len());
        let mut at = 0;
        for (index, span) in spans.iter().enumerate() {
            if !self.target.selects(index, count) || !self.in_range(span.value) {
                continue;
            }
            let replacement = self.rewrite(span, subject, &operand, cx)?;
            out.push_str(&subject[at..span.range.start]);
            out.push_str(&replacement);
            at = span.range.end;
        }
        if at == 0 {
            return Ok(Cow::Borrowed(subject)); // Nothing was selected.
        }
        out.push_str(&subject[at..]);

        if out == subject {
            Ok(Cow::Borrowed(subject))
        } else {
            Ok(Cow::Owned(out))
        }
    }

    fn needs(&self) -> crate::template::TagNeeds {
        self.operand.needs()
    }

    fn asks(&self) -> Vec<crate::run::AskSpec> {
        self.operand.asks()
    }
}

impl ReNumber {
    fn rewrite(
        &self,
        span: &super::numbers::NumberSpan,
        subject: &str,
        operand: &str,
        cx: &EvalCx<'_>,
    ) -> Result<String, OpError> {
        let original = span.text(subject);

        Ok(match self.action {
            NumberAction::ReplaceWithCounter => cx.counter_text(),
            // "To remove a number, simple choose 'replace with' and leave the
            // input field empty!" — which falls out of this for free.
            NumberAction::ReplaceWith => operand.to_owned(),
            NumberAction::Remove => String::new(),
            NumberAction::InsertBefore => format!("{operand}{original}"),
            NumberAction::InsertAfter => format!("{original}{operand}"),
            NumberAction::ZeroPadTo => {
                let width = whole(operand, "zero pad to")? as usize;
                pad_integer_part(original, span, width)
            }
            NumberAction::RoundTo => {
                let places = whole(operand, "round to")?;
                let value = self.value_of(span, original)?;
                self.format(value.round_dp(places), span)
            }
            NumberAction::Add
            | NumberAction::Subtract
            | NumberAction::Multiply
            | NumberAction::Divide => {
                let value = self.value_of(span, original)?;
                let by = operand.trim().parse::<Decimal>().map_err(|_| {
                    OpError::new(
                        "re-number",
                        format!("{operand:?} is not a number to {}", self.action.label()),
                    )
                })?;
                let result = match self.action {
                    NumberAction::Add => value + by,
                    NumberAction::Subtract => value - by,
                    NumberAction::Multiply => value * by,
                    NumberAction::Divide => {
                        if by.is_zero() {
                            return Err(OpError::new("re-number", "cannot divide by zero"));
                        }
                        value / by
                    }
                    _ => unreachable!("only the four arithmetic actions reach here"),
                };
                self.format(result, span)
            }
        })
    }

    fn value_of(
        &self,
        span: &super::numbers::NumberSpan,
        original: &str,
    ) -> Result<Decimal, OpError> {
        span.value.ok_or_else(|| {
            OpError::new(
                "re-number",
                format!("{original:?} is too long to do arithmetic on"),
            )
        })
    }

    /// Renders a computed value back into the name.
    fn format(&self, value: Decimal, span: &super::numbers::NumberSpan) -> String {
        let value = match self.rounding {
            Some(places) => value.round_dp(places),
            // Keep the scale arithmetic produced, minus trailing zeros: 12.50
            // doubled is 25, not 25.00.
            None => value.normalize(),
        };
        let mut text = value.to_string();

        // "same as input; if no input is found locale settings are used" — but
        // fixed rather than locale-dependent, for the reason D30 gives.
        if span.separator == Some(',') {
            text = text.replace('.', ",");
        }

        // "(Not available when using decimal fractions.)"
        if self.keep_length && span.frac_digits.is_none() && !text.contains(['.', ',']) {
            text = pad_digits(&text, span.int_digits);
        }
        text
    }
}

/// Zero pads the digits before the decimal point, leaving sign and fraction
/// where they are.
fn pad_integer_part(original: &str, span: &super::numbers::NumberSpan, width: usize) -> String {
    let (sign, rest) = match original.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", original),
    };
    let (int, fraction) = match span.separator {
        Some(separator) => match rest.split_once(separator) {
            Some((int, fraction)) => (int, Some((separator, fraction))),
            None => (rest, None),
        },
        None => (rest, None),
    };
    let mut out = format!("{sign}{}", pad_digits(int, width));
    if let Some((separator, fraction)) = fraction {
        out.push(separator);
        out.push_str(fraction);
    }
    out
}

/// Left-pads with zeros, keeping a leading minus outside.
fn pad_digits(text: &str, width: usize) -> String {
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text),
    };
    let padding = width.saturating_sub(digits.len());
    let mut out = String::with_capacity(sign.len() + padding + digits.len());
    out.push_str(sign);
    for _ in 0..padding {
        out.push('0');
    }
    out.push_str(digits);
    out
}

/// A whole-number operand, for the two actions that count digits.
fn whole(operand: &str, what: &str) -> Result<u32, OpError> {
    operand
        .trim()
        .parse::<Decimal>()
        .ok()
        .and_then(|d| d.to_u32())
        .ok_or_else(|| {
            OpError::new(
                "re-number",
                format!("{operand:?} is not a digit count for {what}"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::super::testing::run;
    use super::*;
    use crate::counter::CounterSetup;
    use crate::model::FileEntry;
    use crate::run::{Answers, RunContext, RunSettings};

    fn decimals() -> NumberOptions {
        NumberOptions::default().with_decimal_points(true)
    }

    /// "the first number", "the last number", "all numbers" …
    #[test]
    fn the_target_picks_which_numbers_are_touched() {
        let remove = |target| ReNumber::new(target, NumberAction::Remove);
        assert_eq!(run(&remove(NumberTarget::All), "a1b2c3"), "abc");
        assert_eq!(run(&remove(NumberTarget::Nth(1)), "a1b2c3"), "ab2c3");
        assert_eq!(run(&remove(NumberTarget::Nth(2)), "a1b2c3"), "a1bc3");
        assert_eq!(run(&remove(NumberTarget::Last), "a1b2c3"), "a1b2c");
        assert_eq!(run(&remove(NumberTarget::SecondToLast), "a1b2c3"), "a1bc3");
    }

    #[test]
    fn a_target_that_is_not_present_leaves_the_name_alone() {
        let op = ReNumber::new(NumberTarget::Nth(4), NumberAction::Remove);
        assert_eq!(run(&op, "a1b2c3"), "a1b2c3");
        assert_eq!(run(&op, "no numbers"), "no numbers");
    }

    /// "only process numbers that are >= / <= to the specified number"
    #[test]
    fn the_range_filters_narrow_the_selection() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::Remove).between(Some(10), None);
        assert_eq!(run(&op, "1 10 100"), "1  ");

        let op = ReNumber::new(NumberTarget::All, NumberAction::Remove).between(None, Some(10));
        assert_eq!(run(&op, "1 10 100"), "  100");

        let op = ReNumber::new(NumberTarget::All, NumberAction::Remove).between(Some(2), Some(10));
        assert_eq!(run(&op, "1 5 10 100"), "1   100");
    }

    /// "To remove a number, simple choose 'replace with' and leave the input
    /// field empty!"
    #[test]
    fn replace_with_nothing_is_the_documented_way_to_remove() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::ReplaceWith);
        assert_eq!(run(&op, "Track 07"), "Track ");
    }

    #[test]
    fn replace_insert_before_and_insert_after_place_the_operand() {
        let at = |action| ReNumber::new(NumberTarget::Last, action).with_operand("#");
        assert_eq!(run(&at(NumberAction::ReplaceWith), "Track 7"), "Track #");
        assert_eq!(run(&at(NumberAction::InsertBefore), "Track 7"), "Track #7");
        assert_eq!(run(&at(NumberAction::InsertAfter), "Track 7"), "Track 7#");
    }

    #[test]
    fn the_four_arithmetic_operations_use_exact_decimals() {
        let op = |action, by: &str| {
            ReNumber::new(NumberTarget::All, action)
                .with_operand(by)
                .identifying(decimals())
        };
        assert_eq!(run(&op(NumberAction::Add, "1"), "File 9"), "File 10");
        assert_eq!(run(&op(NumberAction::Subtract, "5"), "File 9"), "File 4");
        assert_eq!(
            run(&op(NumberAction::Multiply, "2"), "File 12.50"),
            "File 25"
        );
        assert_eq!(run(&op(NumberAction::Divide, "4"), "File 10"), "File 2.5");
        // The float trap: 0.1 + 0.2 is exactly 0.3 here.
        assert_eq!(run(&op(NumberAction::Add, "0.2"), "v0.1"), "v0.3");
    }

    #[test]
    fn subtracting_past_zero_produces_a_negative_number() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::Subtract).with_operand("10");
        assert_eq!(run(&op, "File 4"), "File -6");
    }

    #[test]
    fn dividing_by_zero_is_an_error_rather_than_a_crash() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::Divide).with_operand("0");
        let entry = FileEntry::synthetic("/tmp/File 4");
        let cx = EvalCx::simple(&entry, 0, 1);
        assert!(op.apply("File 4", &cx).is_err());
    }

    /// "Numbers will be padded with zeros to make sure that their new length is
    /// the same as the old one." Without it, "modifying 001 yields 1".
    #[test]
    fn keep_previous_length_pads_the_result_back_out() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::Add).with_operand("1");
        assert_eq!(run(&op, "Track 001"), "Track 2", "without the option");

        let op = op.keeping_length(true);
        assert_eq!(run(&op, "Track 001"), "Track 002");
        assert_eq!(run(&op, "Track 009"), "Track 010");
        assert_eq!(run(&op, "Track 099"), "Track 100", "never truncates");
    }

    /// "(Not available when using decimal fractions.)"
    #[test]
    fn keep_previous_length_stands_down_for_a_fraction() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::Add)
            .with_operand("1")
            .identifying(decimals())
            .keeping_length(true);
        assert_eq!(run(&op, "v01.50"), "v2.5");
    }

    /// "Zero pad to:" as a Re-Number action, which — unlike the standalone Zero
    /// Padding function — only pads.
    #[test]
    fn the_zero_pad_action_widens_without_cropping() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::ZeroPadTo).with_operand("3");
        assert_eq!(run(&op, "Track 7"), "Track 007");
        assert_eq!(run(&op, "Track 1234"), "Track 1234");
    }

    #[test]
    fn round_to_cuts_the_fraction_at_the_requested_place() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::RoundTo)
            .with_operand("1")
            .identifying(decimals());
        assert_eq!(run(&op, "pi 3.14159"), "pi 3.1");

        let op = op.with_operand("0");
        assert_eq!(run(&op, "pi 3.14159"), "pi 3");
    }

    #[test]
    fn a_comma_decimal_stays_a_comma_decimal() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::Add)
            .with_operand("0.5")
            .identifying(decimals());
        assert_eq!(run(&op, "price 12,50"), "price 13");
        assert_eq!(run(&op, "price 12,25"), "price 12,75");
    }

    /// "Replace with Counter", and the operand field taking tags.
    #[test]
    fn numbers_can_be_replaced_by_the_counter() {
        let entries: Vec<FileEntry> = ["/a/x 5.txt", "/a/y 6.txt", "/a/z 7.txt"]
            .iter()
            .map(FileEntry::synthetic)
            .collect();
        let settings = RunSettings {
            counter: CounterSetup {
                auto_pad: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let run_cx = RunContext::build(&entries, &settings, Answers::default());
        let op = ReNumber::new(NumberTarget::Last, NumberAction::ReplaceWithCounter);

        let names: Vec<String> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let cx = EvalCx::new(e, i, entries.len(), &run_cx);
                op.apply(e.split().0, &cx).unwrap().into_owned()
            })
            .collect();
        assert_eq!(names, ["x 1", "y 2", "z 3"]);
    }

    #[test]
    fn the_operand_may_itself_be_a_tag() {
        let op =
            ReNumber::new(NumberTarget::All, NumberAction::ReplaceWith).with_operand("<Parent>");
        let entry = FileEntry::synthetic("/music/rock/Track 7.mp3");
        let run_cx = RunContext::default();
        let cx = EvalCx::new(&entry, 0, 1, &run_cx);
        assert_eq!(op.apply("Track 7", &cx).unwrap(), "Track rock");
    }

    /// "if a minus sign is found immediately in front of a number it is treated
    /// as part of the number."
    #[test]
    fn identified_minus_signs_take_part_in_the_arithmetic() {
        let op = ReNumber::new(NumberTarget::All, NumberAction::Add)
            .with_operand("10")
            .identifying(NumberOptions::default().with_minus_signs(true));
        assert_eq!(run(&op, "temp -5 C"), "temp 5 C");
    }

    #[test]
    fn every_action_has_its_documented_label() {
        for (action, label) in NumberAction::all().iter().zip(NumberAction::LABELS) {
            assert_eq!(action.label(), label);
        }
        assert!(!NumberAction::Remove.needs_operand());
        assert!(!NumberAction::ReplaceWithCounter.needs_operand());
        assert!(NumberAction::Add.needs_number());
        assert!(!NumberAction::ReplaceWith.needs_number());
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = ReNumber::new(NumberTarget::Nth(2), NumberAction::Multiply)
            .with_operand("2")
            .between(Some(1), Some(99))
            .keeping_length(true)
            .identifying(decimals());
        let text = toml::to_string(&op).unwrap();
        let back: ReNumber = toml::from_str(&text).unwrap();
        assert_eq!(back, op);
    }
}
