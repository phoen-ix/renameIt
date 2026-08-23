//! The counter — a global object, not an operation.
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
    /// *"The counter will start at this value. Negative values are allowed."*
    pub start: i64,
    /// *"The counter will increase or decrease with this value for each file."*
    pub step: i64,
    /// *"The count number will be padded with zeros to attain the number of
    /// digits specified here."* 0 means no padding.
    pub pad: usize,
    /// *"Automatically pad the count number with zeros to ensure that all end
    /// up the same length."*
    pub auto_pad: bool,
    /// *"the start value is updated after each rename operation"* — persisted
    /// by the caller, since the engine does not own settings storage.
    pub running: bool,
    /// *"Resets the counter to the start value if the folder name of the
    /// renamed file is different from the previous file's."*
    pub reset_each_folder: bool,
    /// *"resets when the base name changes […] the filename without any
    /// numbers."*
    pub reset_on_base_name: bool,
    /// *"The counter will reset to the start value once the counter is equal or
    /// passed this number."*
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
    /// The counter value for every entry, in listing order.
    ///
    /// *"The first file will get the initial number and then this number is
    /// increased for each file."*
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
    /// ignores *"will reset to the start value once the counter is equal or
    /// passed this number"* entirely, so a **running** counter with a Reset at
    /// stored a value past the limit and the next run began outside its own
    /// range instead of back at the start.
    pub fn advance(&self, current: i64) -> i64 {
        match self.reset_at {
            // The limit is the last value *used*, not the next one produced.
            Some(limit) if passed(current, limit, self.step) => self.start,
            _ => current.saturating_add(self.step),
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

/// *"The base filename is simply the filename without any numbers."*
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

    /// "The first file will get the initial number and then this number is
    /// increased for each file."
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

    /// "Negative values are allowed."
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

    /// "Resets the counter to the start value if the folder name of the renamed
    /// file is different from the previous file's."
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

    /// "resets when the base name changes […] The base filename is simply the
    /// filename without any numbers."
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

    /// "The counter will reset to the start value once the counter is equal or
    /// passed this number", and that value "will be the last number in the
    /// sequence".
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

    /// "Automatically pad the count number with zeros to ensure that all end up
    /// the same length."
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

    /// "3 digits would result in 001, 010 and 100. Set zero padding to 0 if you
    /// don't want any zeros."
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
