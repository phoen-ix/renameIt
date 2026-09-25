//! The counter — run-wide settings, not an operation.
//!
//! The counter drives both the Add Counter operation
//! and the `<Counter>` tag, which is why it lives here rather than in `ops/`.
//!
//! **D28:** the whole sequence is computed from the input listing before the
//! parallel pass, because every reset reads the entry's own folder and name.

use serde::{Deserialize, Serialize};

use crate::model::FileEntry;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CounterSetup {
    /// The first value. May be negative.
    pub start: i64,
    /// Added for each file; negative counts down.
    pub step: i64,
    /// Zero-pad to this many digits. 0 means no padding.
    pub pad: usize,
    /// Pad to the width of the longest value in the run instead, so every
    /// name comes out the same length and sorts correctly.
    pub auto_pad: bool,
    /// Carry on from where the last run stopped: the start value is updated
    /// after each rename — persisted by the caller, since the engine does not
    /// own settings storage.
    pub running: bool,
    /// Back to the start whenever a file is in a different folder from the
    /// one before it.
    pub reset_each_folder: bool,
    /// Back to the start whenever the base name — the name with its digits
    /// removed — differs from the one before it.
    pub reset_on_base_name: bool,
    /// The last value the counter may produce (the lowest, counting down).
    /// The next file goes back to the start, whether the step lands on the
    /// limit or would jump past it.
    pub reset_at: Option<i64>,
}

impl Default for CounterSetup {
    fn default() -> Self {
        Self {
            start: 1,
            step: 1,
            pad: 0,
            // On by default: names sort correctly without anyone thinking
            // about digit counts.
            auto_pad: true,
            running: false,
            reset_each_folder: false,
            reset_on_base_name: false,
            reset_at: None,
        }
    }
}

impl CounterSetup {
    /// The counter value for every entry, in listing order: the first file gets
    /// the start value, and each file after it one step more.
    pub fn sequence(&self, entries: &[FileEntry]) -> Vec<i64> {
        let mut values = Vec::with_capacity(entries.len());
        let mut current = self.start;
        let mut previous_folder: Option<&std::path::Path> = None;
        let mut previous_base: Option<String> = None;

        for entry in entries {
            let folder = entry.parent();
            let base = self.reset_on_base_name.then(|| base_name(&entry.file_name));

            let folder_changed =
                self.reset_each_folder && previous_folder.is_some_and(|p| p != folder);
            let base_changed = self.reset_on_base_name
                && previous_base.is_some()
                && previous_base.as_deref() != base.as_deref();

            if folder_changed || base_changed {
                current = self.start;
            }

            values.push(current);
            previous_folder = Some(folder);
            previous_base = base;

            current = self.advance(current);
        }
        values
    }

    /// What the counter becomes after producing `current`.
    ///
    /// Extracted so the sequence and [`RunContext::next_start`] cannot disagree
    /// — and they did. `next_start` used to be a bare `last + step`, which
    /// ignored Reset at entirely, so a **running** counter with one stored a
    /// value past the limit and the next run began outside its own range
    /// instead of back at the start.
    ///
    /// The limit is a ceiling (a floor, counting down), not a value the step
    /// has to hit: with start 1, step 4 and Reset at 10 the sequence is
    /// 1, 5, 9, 1 — never 13.
    pub fn advance(&self, current: i64) -> i64 {
        let next = current.saturating_add(self.step);
        match self.reset_at {
            Some(limit) if passed(current, limit, self.step) || beyond(next, limit, self.step) => {
                self.start
            }
            _ => next,
        }
    }

    /// Digits to pad to: the configured width, or — with Auto — enough for the
    /// longest value in the sequence.
    pub fn width_for(&self, values: &[i64]) -> usize {
        if self.auto_pad {
            values
                .iter()
                .map(|v| v.unsigned_abs().to_string().len())
                .max()
                .unwrap_or(1)
        } else {
            self.pad
        }
    }
}

/// Whether `value` has reached the reset limit, in whichever direction the
/// counter is travelling.
fn passed(value: i64, limit: i64, step: i64) -> bool {
    if step < 0 {
        value <= limit
    } else {
        value >= limit
    }
}

/// Whether `value` is strictly past the limit — a value the counter must not
/// produce.
fn beyond(value: i64, limit: i64, step: i64) -> bool {
    if step < 0 {
        value < limit
    } else {
        value > limit
    }
}

/// The base name the reset compares: the file name with its digits removed.
fn base_name(file_name: &str) -> String {
    file_name.chars().filter(|c| !c.is_ascii_digit()).collect()
}

/// Renders a counter value, zero-padded to `width` digits.
///
/// A minus sign sits outside the padding, so `-5` at width 3 is `-005` rather
/// than `0-5`.
pub fn pad(value: i64, width: usize) -> String {
    let digits = value.unsigned_abs().to_string();
    let padding = width.saturating_sub(digits.len());
    let mut out = String::with_capacity(digits.len() + padding + 1);
    if value < 0 {
        out.push('-');
    }
    for _ in 0..padding {
        out.push('0');
    }
    out.push_str(&digits);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entries(paths: &[&str]) -> Vec<FileEntry> {
        paths.iter().map(FileEntry::synthetic).collect()
    }

    fn in_one_folder(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|n| FileEntry::synthetic(PathBuf::from("/music").join(n)))
            .collect()
    }

    #[test]
    fn the_sequence_starts_at_the_start_value_and_steps() {
        let setup = CounterSetup::default();
        let files = in_one_folder(&["a", "b", "c"]);
        assert_eq!(setup.sequence(&files), vec![1, 2, 3]);
    }

    #[test]
    fn the_step_can_be_any_size_or_direction() {
        let setup = CounterSetup {
            start: 10,
            step: 5,
            ..Default::default()
        };
        assert_eq!(
            setup.sequence(&in_one_folder(&["a", "b", "c"])),
            vec![10, 15, 20]
        );

        let setup = CounterSetup {
            start: 3,
            step: -1,
            ..Default::default()
        };
        assert_eq!(
            setup.sequence(&in_one_folder(&["a", "b", "c"])),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn a_negative_start_value_is_allowed() {
        let setup = CounterSetup {
            start: -2,
            ..Default::default()
        };
        assert_eq!(
            setup.sequence(&in_one_folder(&["a", "b", "c", "d"])),
            vec![-2, -1, 0, 1]
        );
    }

    #[test]
    fn reset_each_folder_restarts_when_the_folder_changes() {
        let setup = CounterSetup {
            reset_each_folder: true,
            ..Default::default()
        };
        let files = entries(&["/a/1", "/a/2", "/b/1", "/b/2", "/b/3"]);
        assert_eq!(setup.sequence(&files), vec![1, 2, 1, 2, 3]);
    }

    #[test]
    fn without_the_option_a_folder_change_does_not_reset() {
        let setup = CounterSetup::default();
        let files = entries(&["/a/1", "/a/2", "/b/1"]);
        assert_eq!(setup.sequence(&files), vec![1, 2, 3]);
    }

    /// The base name is the name without its digits, so numbering within
    /// one series does not reset it.
    #[test]
    fn reset_on_base_name_change_ignores_the_numbers_in_the_name() {
        let setup = CounterSetup {
            reset_on_base_name: true,
            ..Default::default()
        };
        let files = in_one_folder(&[
            "Song 1.mp3",
            "Song 2.mp3",
            "Song 12.mp3",
            "Other 1.mp3",
            "Other 2.mp3",
        ]);
        // "Song .mp3" three times, then "Other .mp3" twice.
        assert_eq!(setup.sequence(&files), vec![1, 2, 3, 1, 2]);
    }

    /// The limit is the last number in the sequence.
    #[test]
    fn reset_at_makes_that_value_the_last_in_the_sequence() {
        let setup = CounterSetup {
            reset_at: Some(3),
            ..Default::default()
        };
        let files = in_one_folder(&["a", "b", "c", "d", "e", "f", "g"]);
        assert_eq!(setup.sequence(&files), vec![1, 2, 3, 1, 2, 3, 1]);
    }

    #[test]
    fn reset_at_works_for_a_descending_counter() {
        let setup = CounterSetup {
            start: 3,
            step: -1,
            reset_at: Some(1),
            ..Default::default()
        };
        let files = in_one_folder(&["a", "b", "c", "d", "e"]);
        assert_eq!(setup.sequence(&files), vec![3, 2, 1, 3, 2]);
    }

    /// A step that jumps over the limit resets instead of producing a value
    /// past it: the limit is a ceiling (a floor, counting down), whether or
    /// not the step lands on it exactly.
    #[test]
    fn reset_at_never_produces_a_value_past_the_limit() {
        let files = in_one_folder(&["a", "b", "c", "d", "e", "f"]);
        let up = CounterSetup {
            start: 1,
            step: 4,
            reset_at: Some(10),
            ..Default::default()
        };
        assert_eq!(up.sequence(&files), vec![1, 5, 9, 1, 5, 9]);

        let down = CounterSetup {
            start: 10,
            step: -4,
            reset_at: Some(1),
            ..Default::default()
        };
        assert_eq!(down.sequence(&files), vec![10, 6, 2, 10, 6, 2]);

        // The running counter's next start agrees with the sequence.
        assert_eq!(up.advance(9), 1);
        assert_eq!(down.advance(2), 10);
    }

    #[test]
    fn auto_padding_takes_its_width_from_the_longest_value() {
        let setup = CounterSetup::default();
        let nine = in_one_folder(&["a"; 9]);
        assert_eq!(setup.width_for(&setup.sequence(&nine)), 1);

        let ten = in_one_folder(&["a"; 10]);
        assert_eq!(setup.width_for(&setup.sequence(&ten)), 2);

        let hundred = in_one_folder(&["a"; 100]);
        assert_eq!(setup.width_for(&setup.sequence(&hundred)), 3);
    }

    /// Three digits give 001, 010 and 100; zero means no padding at all.
    #[test]
    fn a_fixed_width_overrides_the_automatic_one() {
        let setup = CounterSetup {
            auto_pad: false,
            pad: 3,
            ..Default::default()
        };
        assert_eq!(setup.width_for(&[1, 2, 3]), 3);
        assert_eq!(pad(1, 3), "001");
        assert_eq!(pad(10, 3), "010");
        assert_eq!(pad(100, 3), "100");

        let setup = CounterSetup {
            auto_pad: false,
            pad: 0,
            ..Default::default()
        };
        assert_eq!(setup.width_for(&[1, 2, 3]), 0);
        assert_eq!(pad(7, 0), "7");
    }

    #[test]
    fn padding_keeps_the_minus_sign_outside() {
        assert_eq!(pad(-5, 3), "-005");
        assert_eq!(pad(-5, 0), "-5");
        assert_eq!(pad(0, 2), "00");
    }

    #[test]
    fn a_number_longer_than_the_padding_is_not_truncated() {
        assert_eq!(pad(1234, 2), "1234");
    }

    #[test]
    fn an_empty_listing_produces_an_empty_sequence() {
        let setup = CounterSetup::default();
        assert!(setup.sequence(&[]).is_empty());
        assert_eq!(setup.width_for(&[]), 1);
    }

    /// The two resets are independent and can both be on.
    #[test]
    fn folder_and_base_name_resets_compose() {
        let setup = CounterSetup {
            reset_each_folder: true,
            reset_on_base_name: true,
            ..Default::default()
        };
        let files = entries(&["/a/x1", "/a/x2", "/a/y1", "/b/y1"]);
        assert_eq!(setup.sequence(&files), vec![1, 2, 1, 1]);
    }
}
