//! Zero Padding — *"Zero Pad Numbers"*.
//!
//! Zero Padding, plus the tooltip from the
//! binary, which documents the half the help page leaves out:
//!
//! > *"Numbers in filenames will be padded with zeros to attain the length
//! > specified here. If a number is longer than this length, it will be cropped
//! > instead."*
//!
//! The point is lexicographic sorting: `1, 10, 2` becomes `01, 02, 10`.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::numbers::{NumberOptions, scan};
use super::{EvalCx, NameTransform, OpError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ZeroPadding {
    /// *"The number you enter here is the number of digits all number found
    /// will have after the rename operation."*
    pub digits: usize,
}

impl Default for ZeroPadding {
    fn default() -> Self {
        // Two digits by default.
        Self { digits: 2 }
    }
}

impl ZeroPadding {
    pub fn new(digits: usize) -> Self {
        Self { digits }
    }
}

impl NameTransform for ZeroPadding {
    fn id(&self) -> &'static str {
        "zero_padding"
    }

    fn summary(&self) -> String {
        format!("Zero pad numbers to {} digits", self.digits)
    }

    fn apply<'a>(&self, subject: &'a str, _cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        // Plain digit runs only: this function has no sign or decimal options.
        let spans = scan(subject, NumberOptions::default());
        if spans.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }

        let mut out = String::with_capacity(subject.len() + spans.len() * self.digits);
        let mut at = 0;
        for span in &spans {
            out.push_str(&subject[at..span.range.start]);
            out.push_str(&resize(span.text(subject), self.digits));
            at = span.range.end;
        }
        out.push_str(&subject[at..]);

        if out == subject {
            Ok(Cow::Borrowed(subject))
        } else {
            Ok(Cow::Owned(out))
        }
    }
}

/// Pads with zeros, or — *"if a number is longer than this length"* — crops.
///
/// **P30:** cropping keeps the *last* `digits` characters. The tooltip does not
/// say which end, but this is the only reading that serves the feature's stated
/// purpose: `007` padded to 2 is `07`, not `00`. It also matches what the
/// padding does in reverse, so padding to 4 and back to 3 is idempotent for any
/// number that fits.
fn resize(digits_text: &str, width: usize) -> String {
    if width == 0 {
        return digits_text.to_owned();
    }
    let len = digits_text.len();
    match len.cmp(&width) {
        std::cmp::Ordering::Equal => digits_text.to_owned(),
        std::cmp::Ordering::Less => {
            let mut out = String::with_capacity(width);
            for _ in 0..width - len {
                out.push('0');
            }
            out.push_str(digits_text);
            out
        }
        std::cmp::Ordering::Greater => digits_text[len - width..].to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::run;
    use super::*;

    /// "Zeros will be used to fill the missing digits." The purpose, from the
    /// help page: 1, 10, 2 sorts as 01, 02, 10.
    #[test]
    fn numbers_are_padded_to_the_requested_width() {
        let op = ZeroPadding::new(2);
        assert_eq!(run(&op, "Track 1"), "Track 01");
        assert_eq!(run(&op, "Track 10"), "Track 10");
        assert_eq!(run(&op, "Track 2"), "Track 02");
    }

    #[test]
    fn every_number_in_the_name_is_padded() {
        assert_eq!(run(&ZeroPadding::new(3), "1 of 2"), "001 of 002");
    }

    /// "If a number is longer than this length, it will be cropped instead."
    #[test]
    fn a_longer_number_is_cropped_rather_than_left_alone() {
        assert_eq!(run(&ZeroPadding::new(2), "Track 007"), "Track 07");
        assert_eq!(run(&ZeroPadding::new(1), "Track 123"), "Track 3");
    }

    #[test]
    fn a_name_with_no_numbers_is_untouched() {
        assert_eq!(run(&ZeroPadding::new(3), "no digits"), "no digits");
    }

    /// The commented-out help line suggested 0 once meant "remove all numbers".
    /// It does not here: 0 leaves numbers exactly as they are, so a stray 0 in
    /// the field cannot silently strip every number in a batch.
    #[test]
    fn a_width_of_zero_changes_nothing() {
        assert_eq!(run(&ZeroPadding::new(0), "Track 7"), "Track 7");
    }

    #[test]
    fn the_minus_sign_is_not_part_of_the_number_here() {
        // This function has no "identify minus signs" option.
        assert_eq!(run(&ZeroPadding::new(3), "-5"), "-005");
    }

    #[test]
    fn padding_is_reversible_for_numbers_that_fit() {
        let widened = run(&ZeroPadding::new(4), "Track 7 of 12");
        assert_eq!(widened, "Track 0007 of 0012");
        let op = ZeroPadding::new(2);
        let entry = crate::model::FileEntry::synthetic("/tmp/x");
        let cx = EvalCx::simple(&entry, 0, 1);
        assert_eq!(op.apply(&widened, &cx).unwrap(), "Track 07 of 12");
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = ZeroPadding::new(5);
        let back: ZeroPadding = toml::from_str(&toml::to_string(&op).unwrap()).unwrap();
        assert_eq!(back, op);
    }
}
