//! The date arithmetic Set Date needs, with no IO and no clock.
//!
//! The zone is a parameter on purpose. Production passes
//! [`TimeZone::system`]; tests pass UTC for determinism and a POSIX rule for
//! the two daylight saving cases, which are otherwise unreachable — CI runs
//! under UTC and Asia/Kolkata (P61), neither of which has daylight saving, and
//! setting `TZ` from a test is `unsafe` in edition 2024 and racy across threads.

use jiff::Span;
use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, TimeZone};

use crate::effect::TimeStamp;

/// Set Date's range: 1 January 1980 to 31 December 2099. A date outside it is
/// an error on that row (D46).
///
/// Enforced here, universally, rather than per platform — deliberately stricter
/// than NTFS (1601–9999) *and* ext4 (1901–2446). Three reasons: preview must
/// equal apply on both platforms or the M1 property tests are lying; the same
/// preset must mean the same thing everywhere (D30's argument); and the check's
/// real value is catching a bad *source* — a filename parsed as year 12 — not
/// modelling a filesystem.
pub const YEAR_MIN: i32 = 1980;
pub const YEAR_MAX: i32 = 2099;

/// Which parts of a date a change touches — change only the hour, say, and
/// leave the rest as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DateComponents {
    pub year: bool,
    pub month: bool,
    pub day: bool,
    pub hour: bool,
    pub minute: bool,
    pub second: bool,
}

/// All six ticked, as the dialog has them.
impl Default for DateComponents {
    fn default() -> Self {
        Self {
            year: true,
            month: true,
            day: true,
            hour: true,
            minute: true,
            second: true,
        }
    }
}

impl DateComponents {
    /// The dialog's captions, in its own order — note `Min.` and `Sec.`.
    pub const LABELS: [&'static str; 6] = ["Year", "Month", "Day", "Hour", "Min.", "Sec."];

    /// True when nothing has to be read off the file first.
    pub fn all(self) -> bool {
        self.year && self.month && self.day && self.hour && self.minute && self.second
    }

    pub fn none(self) -> bool {
        !(self.year || self.month || self.day || self.hour || self.minute || self.second)
    }
}

/// Why a date could not be produced. Never a crash, always a reportable row.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DateProblem {
    #[error("{0} is outside the supported range ({YEAR_MIN}–{YEAR_MAX})")]
    OutOfRange(String),
    #[error("{0} is not a real date")]
    NotADate(String),
    #[error("{0} does not exist — daylight saving skips it")]
    SkippedByDaylightSaving(String),
    #[error(
        "the file does not report a {0} date, so it cannot be changed a part at a time — tick every component, or untick this date"
    )]
    NoPreviousValue(&'static str),
}

/// A wall-clock reading from its six numbers; `None` when it is not a real
/// date, or lies outside the years -9999 to 9999 jiff holds.
pub fn civil(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<DateTime> {
    DateTime::new(
        year.try_into().ok()?,
        month.try_into().ok()?,
        day.try_into().ok()?,
        hour.try_into().ok()?,
        minute.try_into().ok()?,
        second.try_into().ok()?,
        0,
    )
    .ok()
}

/// Merges the components the mask selects from `candidate` into `previous`.
///
/// The day is **clamped**, never rolled: setting the month to February on a
/// file dated the 31st gives the 28th or 29th, not the 2nd of March. Rolling
/// would silently change the very month the user just set.
///
/// Sub-second precision rides with the `Sec.` box: ticked, it comes from the
/// source; unticked, the previous nanoseconds survive. Nothing in the dialog
/// addresses sub-seconds, so this is our decision to make.
pub fn merge(previous: DateTime, candidate: DateTime, mask: DateComponents) -> Option<DateTime> {
    let year = if mask.year {
        candidate.year()
    } else {
        previous.year()
    };
    let month = if mask.month {
        candidate.month()
    } else {
        previous.month()
    };
    let day = if mask.day {
        candidate.day()
    } else {
        previous.day()
    };
    let hour = if mask.hour {
        candidate.hour()
    } else {
        previous.hour()
    };
    let minute = if mask.minute {
        candidate.minute()
    } else {
        previous.minute()
    };
    let (second, nano) = if mask.second {
        (candidate.second(), candidate.subsec_nanosecond())
    } else {
        (previous.second(), previous.subsec_nanosecond())
    };

    let last = Date::new(year, month, 1).ok()?.days_in_month();
    DateTime::new(year, month, day.min(last), hour, minute, second, nano).ok()
}

/// How far to move a date, and in what units.
///
/// Exactly these six, in this order. There is deliberately no `Week(s)` —
/// worth recording so nobody later "improves" it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntervalUnit {
    #[default]
    Years,
    Months,
    Days,
    Hours,
    Minutes,
    Seconds,
}

impl IntervalUnit {
    pub const ALL: [Self; 6] = [
        Self::Years,
        Self::Months,
        Self::Days,
        Self::Hours,
        Self::Minutes,
        Self::Seconds,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Years => "Year(s)",
            Self::Months => "Month(s)",
            Self::Days => "Day(s)",
            Self::Hours => "Hour(s)",
            Self::Minutes => "Minute(s)",
            Self::Seconds => "Second(s)",
        }
    }
}

/// Moves `base` by `amount` units. Negative subtracts.
///
/// Calendar units clamp the same way `merge` does: 31 January plus one month
/// is 28 or 29 February, never 2 or 3 March. Time units count on the wall
/// clock, with no zone. `None` when the result leaves the representable range
/// — reported, never wrapped.
pub fn shift(base: DateTime, amount: i64, unit: IntervalUnit) -> Option<DateTime> {
    let span = match unit {
        IntervalUnit::Years => Span::new().try_years(amount),
        IntervalUnit::Months => Span::new().try_months(amount),
        IntervalUnit::Days => Span::new().try_days(amount),
        IntervalUnit::Hours => Span::new().try_hours(amount),
        IntervalUnit::Minutes => Span::new().try_minutes(amount),
        IntervalUnit::Seconds => Span::new().try_seconds(amount),
    };
    base.checked_add(span.ok()?).ok()
}

/// Rejects anything outside the supported range.
pub fn check_range(when: DateTime) -> Result<(), DateProblem> {
    if (YEAR_MIN..=YEAR_MAX).contains(&i32::from(when.year())) {
        Ok(())
    } else {
        Err(DateProblem::OutOfRange(format(when)))
    }
}

/// Turns a wall-clock reading into an instant.
///
/// This is where daylight saving bites, and both answers are deliberate:
///
/// * **Ambiguous** — the hour that happens twice in autumn. Takes the earlier
///   of the two, because a preview must be reproducible and "whichever" is not
///   an answer a plan can be built from.
/// * **Skipped** — the hour that does not exist in spring. Reported, never
///   nudged forward by an hour. Quietly writing a different time than the one
///   asked for is the kind of thing nobody finds until much later.
pub fn localise(tz: &TimeZone, local: DateTime) -> Result<TimeStamp, DateProblem> {
    let ambiguous = tz.to_ambiguous_timestamp(local);
    if let AmbiguousOffset::Gap { .. } = ambiguous.offset() {
        return Err(DateProblem::SkippedByDaylightSaving(format(local)));
    }
    let instant = ambiguous
        .earlier()
        .map_err(|_| DateProblem::OutOfRange(format(local)))?;
    // `TimeStamp` borrows a second before the epoch; jiff signs both parts.
    let nanos = instant.as_nanosecond();
    Ok(TimeStamp {
        secs: nanos.div_euclid(1_000_000_000) as i64,
        nanos: nanos.rem_euclid(1_000_000_000) as u32,
    })
}

/// The wall-clock reading of an instant, so a mask can be applied to it.
/// `None` beyond the years -9999 to 9999.
pub fn to_local(tz: &TimeZone, stamp: TimeStamp) -> Option<DateTime> {
    let instant = jiff::Timestamp::new(stamp.secs, i32::try_from(stamp.nanos).ok()?).ok()?;
    Some(tz.to_datetime(instant))
}

/// `1999-05-09 10:18:05` — one format everywhere, deliberately not the system's.
///
/// D30 already rejects locale-dependence: the same preset has to mean the same
/// thing on every machine, and a date a user reads in the preview has to be the
/// one they can type back in.
pub fn format(when: DateTime) -> String {
    when.strftime("%Y-%m-%d %H:%M:%S").to_string()
}

/// Reads a two-digit year through a DOS-style window: 80–99 are 19xx, 00–79 are
/// 20xx.
///
/// The window is the supported range's own (D42): `Photo 99-12-31.jpg` means
/// 1999, and `Photo 05-01-02.jpg` means 2005. Anything *written* with three or
/// more digits is taken at face value, leading zeros included: `0017` is more
/// likely a sequence number than a year, and at face value D46's range check
/// reports it instead of it quietly becoming 2017.
pub fn widen_year(text: &str) -> Option<i32> {
    let digits = text.trim();
    let value: i32 = digits.parse().ok()?;
    if digits.len() <= 2 && (0..=99).contains(&value) {
        Some(if value >= 80 {
            1900 + value
        } else {
            2000 + value
        })
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i16, mo: i8, d: i8, h: i8, mi: i8, s: i8) -> DateTime {
        jiff::civil::datetime(y, mo, d, h, mi, s, 0)
    }

    /// Central European rules: winter +01:00, summer +02:00, and the clocks go
    /// forward at 1999-03-28 02:00 local — so 02:30 that day does not exist,
    /// and 02:30 on 1999-10-31 happens twice.
    fn dst_zone() -> TimeZone {
        TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap()
    }

    fn only(component: &str) -> DateComponents {
        let mut mask = DateComponents {
            year: false,
            month: false,
            day: false,
            hour: false,
            minute: false,
            second: false,
        };
        match component {
            "year" => mask.year = true,
            "month" => mask.month = true,
            "day" => mask.day = true,
            "hour" => mask.hour = true,
            other => panic!("unknown component {other}"),
        }
        mask
    }

    /// Change only the hour, and leave the rest as it was.
    #[test]
    fn only_change_the_hour_and_leave_the_rest_unchanged() {
        let previous = at(1999, 5, 9, 10, 18, 5);
        let candidate = at(2008, 2, 17, 11, 23, 50);
        assert_eq!(
            merge(previous, candidate, only("hour")),
            Some(at(1999, 5, 9, 11, 18, 5))
        );
    }

    /// A second pass that sets only the year, after the Exif date has
    /// supplied the rest.
    #[test]
    fn the_second_pass_sets_only_the_year() {
        assert_eq!(
            merge(
                at(1999, 5, 9, 10, 18, 5),
                at(2008, 2, 17, 11, 23, 50),
                only("year")
            ),
            Some(at(2008, 5, 9, 10, 18, 5))
        );
    }

    /// Setting the month must not silently change the month.
    #[test]
    fn setting_only_the_month_clamps_the_day_instead_of_rolling_over() {
        // 31 January, asked to become February.
        let previous = at(2001, 1, 31, 12, 0, 0);
        let candidate = at(2001, 2, 1, 0, 0, 0);
        assert_eq!(
            merge(previous, candidate, only("month")),
            Some(at(2001, 2, 28, 12, 0, 0)),
            "clamped to the last day of February, not rolled into March"
        );

        // And a leap year has the extra day available.
        let leap = at(2004, 1, 31, 12, 0, 0);
        assert_eq!(
            merge(leap, at(2004, 2, 1, 0, 0, 0), only("month")),
            Some(at(2004, 2, 29, 12, 0, 0))
        );
    }

    #[test]
    fn an_all_on_mask_takes_the_candidate_whole() {
        let candidate = at(2008, 2, 17, 11, 23, 50);
        assert_eq!(
            merge(
                at(1999, 5, 9, 10, 18, 5),
                candidate,
                DateComponents::default()
            ),
            Some(candidate)
        );
        assert!(DateComponents::default().all());
    }

    #[test]
    fn a_fresh_mask_has_every_component_and_the_dialogs_labels() {
        assert!(DateComponents::default().all());
        assert!(!DateComponents::default().none());
        assert_eq!(
            DateComponents::LABELS,
            ["Year", "Month", "Day", "Hour", "Min.", "Sec."]
        );
    }

    /// Six units and no weeks: a week is seven days, which `Day(s)` already
    /// says.
    #[test]
    fn the_interval_units_are_the_six_the_dialog_offers() {
        assert_eq!(
            IntervalUnit::ALL.map(IntervalUnit::label),
            [
                "Year(s)",
                "Month(s)",
                "Day(s)",
                "Hour(s)",
                "Minute(s)",
                "Second(s)"
            ]
        );
    }

    #[test]
    fn an_interval_moves_a_date_both_ways() {
        let base = at(2000, 6, 15, 12, 30, 45);
        assert_eq!(
            shift(base, 1, IntervalUnit::Years),
            Some(at(2001, 6, 15, 12, 30, 45))
        );
        assert_eq!(
            shift(base, -1, IntervalUnit::Years),
            Some(at(1999, 6, 15, 12, 30, 45))
        );
        assert_eq!(
            shift(base, 1, IntervalUnit::Hours),
            Some(at(2000, 6, 15, 13, 30, 45))
        );
        assert_eq!(
            shift(base, -90, IntervalUnit::Minutes),
            Some(at(2000, 6, 15, 11, 0, 45))
        );
    }

    #[test]
    fn a_month_interval_clamps_instead_of_rolling_over() {
        assert_eq!(
            shift(at(2001, 1, 31, 9, 0, 0), 1, IntervalUnit::Months),
            Some(at(2001, 2, 28, 9, 0, 0))
        );
    }

    /// An interval must not wrap silently at the edge of what jiff can hold.
    #[test]
    fn an_absurd_interval_is_reported_rather_than_wrapped() {
        assert_eq!(
            shift(at(2000, 1, 1, 0, 0, 0), 1_000_000, IntervalUnit::Years),
            None
        );
    }

    #[test]
    fn the_supported_range_is_enforced() {
        assert!(check_range(at(1980, 1, 1, 0, 0, 0)).is_ok());
        assert!(check_range(at(2099, 12, 31, 23, 59, 59)).is_ok());
        assert!(check_range(at(1979, 12, 31, 23, 59, 59)).is_err());
        assert!(check_range(at(2100, 1, 1, 0, 0, 0)).is_err());
        // And the message names the range, so the user knows what to do.
        let message = check_range(at(1900, 1, 1, 0, 0, 0))
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("1980") && message.contains("2099"),
            "{message}"
        );
    }

    #[test]
    fn an_ordinary_local_time_becomes_an_instant() {
        let stamp = localise(&TimeZone::UTC, at(2000, 1, 1, 0, 0, 0)).unwrap();
        assert_eq!(stamp.secs, 946_684_800);
        assert_eq!(
            to_local(&TimeZone::UTC, stamp),
            Some(at(2000, 1, 1, 0, 0, 0))
        );
    }

    /// The hour that happens twice: whichever we pick, it has to be the same
    /// one every time, or the preview is not reproducible.
    #[test]
    fn an_ambiguous_local_time_takes_the_earlier_of_the_two() {
        let ambiguous = at(1999, 10, 31, 2, 30, 0);
        let first = localise(&dst_zone(), ambiguous).unwrap();
        let again = localise(&dst_zone(), ambiguous).unwrap();
        assert_eq!(first, again);
        // Earlier in real time means the summer offset, i.e. the smaller
        // timestamp of the two candidates.
        let winter = localise(&dst_zone(), at(1999, 10, 31, 3, 30, 0)).unwrap();
        assert!(first.secs < winter.secs);
    }

    /// The hour that does not exist: reported, never nudged.
    #[test]
    fn a_time_that_daylight_saving_skips_is_reported_not_nudged() {
        let err = localise(&dst_zone(), at(1999, 3, 28, 2, 30, 0)).unwrap_err();
        assert!(
            matches!(err, DateProblem::SkippedByDaylightSaving(_)),
            "{err:?}"
        );
        assert!(err.to_string().contains("1999-03-28 02:30:00"), "{err}");
        // The hour either side is perfectly fine.
        assert!(localise(&dst_zone(), at(1999, 3, 28, 1, 30, 0)).is_ok());
        assert!(localise(&dst_zone(), at(1999, 3, 28, 3, 30, 0)).is_ok());
    }

    /// D30: one format everywhere, not the system's.
    #[test]
    fn a_date_reads_the_same_on_every_machine() {
        assert_eq!(format(at(2008, 2, 17, 11, 23, 50)), "2008-02-17 11:23:50");
    }

    /// The user's call: a DOS-style window, so `Photo 99-12-31.jpg` works.
    #[test]
    fn a_two_digit_year_reads_through_a_dos_window() {
        assert_eq!(widen_year("99"), Some(1999));
        assert_eq!(widen_year("80"), Some(1980), "the bottom of the window");
        assert_eq!(widen_year("79"), Some(2079), "and the top");
        assert_eq!(widen_year("00"), Some(2000));
        assert_eq!(widen_year("05"), Some(2005));
        // Anything already written in full is taken at face value.
        assert_eq!(widen_year("1999"), Some(1999));
        assert_eq!(widen_year("2005"), Some(2005));
        assert_eq!(widen_year(" 2005 "), Some(2005), "trimmed");
        assert_eq!(widen_year("not a year"), None);
    }

    /// Only a year *written* with two digits is widened. `0017` is four digits
    /// — a sequence number, most likely — and is taken at face value, so D46's
    /// range check can report it instead of it silently becoming 2017.
    #[test]
    fn a_zero_padded_long_year_is_not_widened() {
        assert_eq!(widen_year("0012"), Some(12));
        assert_eq!(widen_year("0017"), Some(17));
        assert_eq!(widen_year("005"), Some(5));
        assert_eq!(widen_year("000"), Some(0));
        assert_eq!(widen_year("5"), Some(2005), "one digit is still short");
    }

    /// Every widened two-digit year lands inside the supported range, which is
    /// what makes the window the right one rather than an arbitrary choice.
    #[test]
    fn every_two_digit_year_lands_inside_the_supported_range() {
        for value in 0..=99 {
            let year = widen_year(&format!("{value:02}")).unwrap();
            assert!(
                (YEAR_MIN..=YEAR_MAX).contains(&year),
                "{value:02} widened to {year}, outside {YEAR_MIN}–{YEAR_MAX}"
            );
        }
    }
}
