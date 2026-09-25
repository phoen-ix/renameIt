//! Zero Padding — every number in the name brought to one width.
//!
//! Shorter numbers are padded with zeros; numbers longer than the width are
//! cropped (P30). The point is lexicographic sorting: `1, 10, 2` becomes
//! `01, 02, 10`.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::numbers::{MAX_PAD_WIDTH, NumberOptions, scan};
use super::{EvalCx, NameTransform, OpError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ZeroPadding {
    /// The number of digits every number in the name has afterwards. 0 leaves
    /// numbers as they are.
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
        // A job file does not clamp the width; see `MAX_PAD_WIDTH`.
        if self.digits > MAX_PAD_WIDTH {
            return Err(OpError::new(
                "zero padding",
                format!("{} digits is longer than any file name can be", self.digits),
            ));
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

/// Pads with zeros, or crops a number longer than the width.
///
/// **P30:** cropping keeps the *last* `digits` characters. It is the only
/// choice that serves the operation's purpose: `007` brought to 2 is `07`, not
/// `00`. It also matches what the padding does in reverse, so padding to 4 and
/// back to 3 is idempotent for any number that fits.
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

    /// The purpose: 1, 10, 2 sorts as 01, 02, 10.
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

    #[test]
    fn a_longer_number_is_cropped_rather_than_left_alone() {
        assert_eq!(run(&ZeroPadding::new(2), "Track 007"), "Track 07");
        assert_eq!(run(&ZeroPadding::new(1), "Track 123"), "Track 3");
    }

    #[test]
    fn a_name_with_no_numbers_is_untouched() {
        assert_eq!(run(&ZeroPadding::new(3), "no digits"), "no digits");
    }

    /// 0 leaves numbers exactly as they are, rather than meaning "remove every
    /// number": a stray 0 in the field must not silently strip every number
    /// in a batch.
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

    /// A job file does not clamp the width, and a width no name can hold only
    /// costs memory: it is a row error instead.
    #[test]
    fn a_width_longer_than_any_name_is_an_error() {
        let entry = crate::model::FileEntry::synthetic("/tmp/x");
        let cx = EvalCx::simple(&entry, 0, 1);
        let err = ZeroPadding::new(50_000_000)
            .apply("Track 7", &cx)
            .unwrap_err();
        assert!(err.to_string().contains("longer than"), "{err}");
        assert_eq!(
            ZeroPadding::new(50_000_000)
                .apply("no digits", &cx)
                .unwrap(),
            "no digits",
            "a name with no number is not touched, so it is not an error"
        );
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = ZeroPadding::new(5);
        let back: ZeroPadding = toml::from_str(&toml::to_string(&op).unwrap()).unwrap();
        assert_eq!(back, op);
    }
}
