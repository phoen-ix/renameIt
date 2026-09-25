//! Set Date & Time — changing the three timestamps a file carries.
//!
//! Nine sources for the date, in the order the Source dropdown lists them, and
//! six interval units for the two that shift a date.
//!
//! The largest operation in the app. Almost all of the actual arithmetic lives
//! in [`crate::datetime`], which is pure and generic over the time zone; what
//! is here is *which* date to use and *which* stamps to write it to.

use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use super::{EvalCx, OpError, SideEffectAction};
use crate::datetime::{self, DateComponents, DateProblem, IntervalUnit};
use crate::effect::{Effect, TimeSet, TimeStamp, Undoability};
use crate::meta::exif;
use crate::template::TagNeeds;

/// The `Source:` dropdown, in the order the card lists it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateSource {
    #[default]
    EnterNewDate,
    ImageExif,
    FileCreated,
    FileAccessed,
    FileModified,
    NowAtStartOfRun,
    AddInterval,
    SubtractInterval,
    FromFilename,
}

impl DateSource {
    pub const ALL: [Self; 9] = [
        Self::EnterNewDate,
        Self::ImageExif,
        Self::FileCreated,
        Self::FileAccessed,
        Self::FileModified,
        Self::NowAtStartOfRun,
        Self::AddInterval,
        Self::SubtractInterval,
        Self::FromFilename,
    ];

    /// The dropdown's text. The eighth is abbreviated to fit beside its
    /// neighbours in the combo.
    pub fn label(self) -> &'static str {
        match self {
            Self::EnterNewDate => "Enter new date (set below)",
            Self::ImageExif => "Get from image Exif tag",
            Self::FileCreated => "Use files' created date",
            Self::FileAccessed => "Use files' accessed date",
            Self::FileModified => "Use files' modified date",
            Self::NowAtStartOfRun => "Now (time at start of rename)",
            Self::AddInterval => "Add interval to files' current date",
            Self::SubtractInterval => "Subtract interval from files' curr. date",
            // Names the slots, because nothing else in the app does: without
            // `<%4>` set up as the year every row is silently left alone.
            Self::FromFilename => "Get from filename (Parts <%4>–<%9>)",
        }
    }

    /// Whether the interval spinner and unit combo are live.
    pub fn uses_interval(self) -> bool {
        matches!(self, Self::AddInterval | Self::SubtractInterval)
    }

    /// Whether the date and time boxes are live.
    pub fn uses_wall_clock(self) -> bool {
        self == Self::EnterNewDate
    }
}

/// Which of the three stamps a file and a folder carry to write.
///
/// Created and Accessed start unticked, Modified ticked: the modified date is
/// the one file managers show and sort by, and so the one a user means by
/// "the file's date".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DateTargets {
    pub created: bool,
    pub accessed: bool,
    pub modified: bool,
}

impl Default for DateTargets {
    fn default() -> Self {
        Self {
            created: false,
            accessed: false,
            modified: true,
        }
    }
}

impl DateTargets {
    pub const LABELS: [&'static str; 3] = ["Created", "Accessed", "Modified"];

    pub fn none(self) -> bool {
        !(self.created || self.accessed || self.modified)
    }
}

/// The date and time boxes, as a wall-clock reading with no zone.
///
/// Six numbers rather than a formatted string: the preset reads like the
/// card, and there is no format to get wrong. The card may *display* the date
/// in the system's format; what is *stored* has to mean the same thing on
/// every machine (D30).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WallClock {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// The bottom of the supported range (1980–2099), so `Default` is a
/// constant.
///
/// A constant rather than "today" because presets have to be deterministic and
/// the preset writer's `default_of` compares against this. The *GUI* seeds a
/// freshly added card with the current date instead, which is what a user
/// expects to see in the boxes.
impl Default for WallClock {
    fn default() -> Self {
        Self {
            year: datetime::YEAR_MIN,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }
}

impl WallClock {
    pub fn from_naive(when: NaiveDateTime) -> Self {
        use chrono::{Datelike, Timelike};
        Self {
            year: when.year(),
            month: when.month(),
            day: when.day(),
            hour: when.hour(),
            minute: when.minute(),
            second: when.second(),
        }
    }

    pub fn to_naive(self) -> Option<NaiveDateTime> {
        NaiveDate::from_ymd_opt(self.year, self.month, self.day)?.and_hms_opt(
            self.hour,
            self.minute,
            self.second,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetDate {
    pub source: DateSource,
    /// Used by `Enter new date (set below)`.
    pub date: WallClock,
    /// The spinner beside the unit combo. 1 by default.
    pub interval: u32,
    pub unit: IntervalUnit,
    /// The `Change:` grid's left-hand column.
    pub change: DateComponents,
    /// Its right-hand column.
    pub targets: DateTargets,
    /// This also works on folders: the first image inside supplies the Exif
    /// date.
    ///
    /// Only consulted by `Get from image Exif tag`.
    pub folder_peek: bool,
}

impl Default for SetDate {
    fn default() -> Self {
        Self {
            source: DateSource::EnterNewDate,
            date: WallClock::default(),
            interval: 1,
            unit: IntervalUnit::Years,
            change: DateComponents::default(),
            targets: DateTargets::default(),
            folder_peek: true,
        }
    }
}

/// What a source resolved to for one file.
enum Resolved {
    /// One value, written to every selected target.
    Fixed(NaiveDateTime),
    /// Computed from **each selected target's own** current value.
    Shift { amount: i64, unit: IntervalUnit },
    /// This file cannot supply one. Not an error — a row left alone.
    Unavailable,
}

/// Which of the three stamps a target reads its "current" value from.
#[derive(Clone, Copy)]
enum Which {
    Created,
    Accessed,
    Modified,
}

impl Which {
    fn name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Accessed => "accessed",
            Self::Modified => "modified",
        }
    }

    fn of(self, entry: &crate::model::FileEntry) -> Option<std::time::SystemTime> {
        match self {
            Self::Created => entry.created,
            Self::Accessed => entry.accessed,
            Self::Modified => entry.modified,
        }
    }
}

impl SetDate {
    /// The whole operation is inert when it would write nothing.
    pub fn is_empty(self) -> bool {
        self.targets.none() || self.change.none()
    }

    fn resolve(&self, cx: &EvalCx<'_>) -> Result<Resolved, DateProblem> {
        let entry = cx.entry;
        let local = |t: Option<std::time::SystemTime>| {
            t.map(TimeStamp::from_system)
                .and_then(|s| datetime::to_local(&Local, s))
        };
        Ok(match self.source {
            DateSource::EnterNewDate => match self.date.to_naive() {
                Some(when) => Resolved::Fixed(when),
                None => {
                    return Err(DateProblem::NotADate(format!(
                        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                        self.date.year,
                        self.date.month,
                        self.date.day,
                        self.date.hour,
                        self.date.minute,
                        self.date.second
                    )));
                }
            },
            DateSource::ImageExif => {
                // Reads the file, behind the mtime-keyed cache in `meta::exif`
                // — the same bargain `<Crc32>` already makes.
                let found = if entry.is_dir && !self.folder_peek {
                    // A folder with the peek off has no date of its own.
                    exif::date_of(&entry.path)
                } else {
                    exif::date_of_entry(entry)
                };
                match found {
                    Some(when) => Resolved::Fixed(when),
                    None => Resolved::Unavailable,
                }
            }
            DateSource::FileCreated => match local(entry.created) {
                Some(when) => Resolved::Fixed(when),
                None => Resolved::Unavailable,
            },
            DateSource::FileAccessed => match local(entry.accessed) {
                Some(when) => Resolved::Fixed(when),
                None => Resolved::Unavailable,
            },
            DateSource::FileModified => match local(entry.modified) {
                Some(when) => Resolved::Fixed(when),
                None => Resolved::Unavailable,
            },
            // D28 already makes this one timestamp for the whole run, so every
            // file in a batch agrees.
            DateSource::NowAtStartOfRun => match local(Some(cx.run.now)) {
                Some(when) => Resolved::Fixed(when),
                None => Resolved::Unavailable,
            },
            DateSource::AddInterval => Resolved::Shift {
                amount: i64::from(self.interval),
                unit: self.unit,
            },
            DateSource::SubtractInterval => Resolved::Shift {
                amount: -i64::from(self.interval),
                unit: self.unit,
            },
            DateSource::FromFilename => match self.date_in_filename(cx) {
                Some(when) => Resolved::Fixed(when),
                None => Resolved::Unavailable,
            },
        })
    }

    /// The date in the name, read through Setup Parts: `<%4>` is the year,
    /// `<%5>` the month, `<%6>` the day, `<%7>`–`<%9>` hour, minute and
    /// second. The same mapping is in `docs/tags.md` and in the source's label.
    ///
    /// Splits the **stem** of the current name: `My File 2000-12-31.txt` with
    /// `<%1> <%2> <%4>-<%5>-<%6>` gives `<%6> = "31"`. Splitting the whole name
    /// would give `"31.txt"`, which is not a day.
    ///
    /// It also means an earlier Replace card can tidy a name before the date is
    /// read out of it — which is the pipeline paying for itself.
    fn date_in_filename(&self, cx: &EvalCx<'_>) -> Option<NaiveDateTime> {
        let (stem, _) = crate::model::split_name(cx.current, cx.entry.is_dir);
        let parts = cx.run.parts.split(stem);
        // Year is required; everything else has a sensible floor, so a
        // year-only filename means the first instant of that year.
        let year = datetime::widen_year(parts.get(4)?)?;
        let number = |slot: u8, fallback: u32| -> Option<u32> {
            match parts.get(slot) {
                Some(text) if !text.trim().is_empty() => text.trim().parse().ok(),
                _ => Some(fallback),
            }
        };
        NaiveDate::from_ymd_opt(year, number(5, 1)?, number(6, 1)?)?.and_hms_opt(
            number(7, 0)?,
            number(8, 0)?,
            number(9, 0)?,
        )
    }

    /// The stamp to write to one target, or why not.
    fn stamp_for(
        &self,
        cx: &EvalCx<'_>,
        resolved: &Resolved,
        which: Which,
    ) -> Result<Option<TimeStamp>, DateProblem> {
        let previous = which
            .of(cx.entry)
            .map(TimeStamp::from_system)
            .and_then(|s| datetime::to_local(&Local, s));

        let candidate = match resolved {
            Resolved::Unavailable => return Ok(None),
            Resolved::Fixed(when) => *when,
            Resolved::Shift { amount, unit } => {
                // "files' current date" is per target: +1 hour with Created and
                // Modified both ticked moves each from its own value. Folding
                // them onto one number would destroy the difference between
                // them without being asked, and would make the commonest use of
                // the feature — "the camera clock was an hour out" — wrong.
                let Some(base) = previous else {
                    return Err(DateProblem::NoPreviousValue(which.name()));
                };
                datetime::shift(base, *amount, *unit)
                    .ok_or_else(|| DateProblem::OutOfRange(datetime::format(base)))?
            }
        };

        // A partial mask has to read what is already there. A stamp the
        // filesystem never reported cannot be partly changed — the same reading
        // P12 gives an extensionless file under `Scope::Extension`.
        let merged = if self.change.all() {
            candidate
        } else {
            let Some(previous) = previous else {
                return Err(DateProblem::NoPreviousValue(which.name()));
            };
            datetime::merge(previous, candidate, self.change)
                .ok_or_else(|| DateProblem::NotADate(datetime::format(candidate)))?
        };

        // Checked on what will actually be written, not on the source: a source
        // year of 2150 with `Year` unticked never reaches disk, and rejecting
        // it would be wrong.
        datetime::check_range(merged)?;
        datetime::localise(&Local, merged).map(Some)
    }
}

impl SideEffectAction for SetDate {
    fn id(&self) -> &'static str {
        "set_date"
    }

    fn summary(&self) -> String {
        if self.targets.none() {
            return "Set Date & Time (no date selected)".to_owned();
        }
        if self.change.none() {
            return "Set Date & Time (nothing to change)".to_owned();
        }
        let targets: Vec<&str> = DateTargets::LABELS
            .iter()
            .zip([
                self.targets.created,
                self.targets.accessed,
                self.targets.modified,
            ])
            .filter_map(|(label, on)| on.then_some(*label))
            .collect();
        format!("{} ← {}", targets.join(", "), self.source.label())
    }

    fn effect(&self, cx: &EvalCx<'_>) -> Result<Option<Effect>, OpError> {
        if self.is_empty() {
            return Ok(None);
        }
        let fail = |e: DateProblem| OpError::new("Set Date & Time", e.to_string());
        let resolved = self.resolve(cx).map_err(fail)?;

        let stamp_for = |on: bool, which: Which| -> Result<Option<TimeStamp>, OpError> {
            if !on {
                return Ok(None);
            }
            self.stamp_for(cx, &resolved, which).map_err(fail)
        };

        Ok(Some(Effect::Times(TimeSet {
            created: stamp_for(self.targets.created, Which::Created)?,
            accessed: stamp_for(self.targets.accessed, Which::Accessed)?,
            modified: stamp_for(self.targets.modified, Which::Modified)?,
        })))
    }

    fn describe(&self, effect: &Effect) -> String {
        let Effect::Times(times) = effect else {
            return self.summary();
        };
        let named: Vec<(&str, Option<TimeStamp>)> = vec![
            (DateTargets::LABELS[0], times.created),
            (DateTargets::LABELS[1], times.accessed),
            (DateTargets::LABELS[2], times.modified),
        ];
        // At least one stamp is set: `Pipeline::evaluate` drops an empty
        // effect — a file with no date to give — before asking for a
        // description, so that row is left alone rather than described.
        let set: Vec<&str> = named
            .iter()
            .filter_map(|(label, stamp)| stamp.map(|_| *label))
            .collect();
        debug_assert!(
            !set.is_empty(),
            "describe is only asked about a non-empty effect"
        );
        // Every selected target gets the same value unless the source shifts
        // each from its own, in which case the first is representative enough
        // for a one-line badge and the rest follow the same rule.
        let when = named
            .iter()
            .find_map(|(_, stamp)| *stamp)
            .and_then(|s| datetime::to_local(&Local, s))
            .map(datetime::format)
            .unwrap_or_default();
        format!("{} → {when}", set.join(", "))
    }

    /// The executor records the three stamps it is about to replace, so undo
    /// puts them back exactly.
    fn undoable(&self) -> Undoability {
        Undoability::Journaled
    }

    fn needs(&self) -> TagNeeds {
        let mut needs = TagNeeds::NONE;
        // Says out loud that this configuration opens files, exactly as a
        // template mentioning `<Crc32>` does.
        if self.source == DateSource::ImageExif {
            needs.insert(TagNeeds::FILE_CONTENT);
        }
        if self.source == DateSource::FromFilename {
            needs.insert(TagNeeds::PARTS);
        }
        needs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileEntry;
    use crate::ops::OpKind;
    use crate::run::{Answers, RunContext, RunSettings};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn naive(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    /// A `SystemTime` whose *local* reading is the wall clock given, so the
    /// tests read as wall-clock dates regardless of the runner's zone.
    fn system_at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> SystemTime {
        let stamp = datetime::localise(&Local, naive(y, mo, d, h, mi, s)).unwrap();
        UNIX_EPOCH + Duration::new(stamp.secs as u64, stamp.nanos)
    }

    fn entry_dated(modified: Option<SystemTime>, created: Option<SystemTime>) -> FileEntry {
        let mut entry = FileEntry::synthetic("/photos/My File 2000-12-31.txt");
        entry.modified = modified;
        entry.created = created;
        entry
    }

    fn effect_of(op: &SetDate, entry: &FileEntry) -> Result<Option<Effect>, OpError> {
        let cx = EvalCx::simple(entry, 0, 1);
        op.effect(&cx)
    }

    fn times_of(op: &SetDate, entry: &FileEntry) -> TimeSet {
        match effect_of(op, entry).unwrap() {
            Some(Effect::Times(times)) => times,
            other => panic!("expected times, got {other:?}"),
        }
    }

    fn local_of(stamp: Option<TimeStamp>) -> Option<NaiveDateTime> {
        datetime::to_local(&Local, stamp?)
    }

    /// The dropdown, in order — a preset stores the variant, and the card
    /// shows the label.
    #[test]
    fn the_source_dropdown_lists_the_nine_sources_in_order() {
        assert_eq!(
            DateSource::ALL.map(DateSource::label),
            [
                "Enter new date (set below)",
                "Get from image Exif tag",
                "Use files' created date",
                "Use files' accessed date",
                "Use files' modified date",
                "Now (time at start of rename)",
                "Add interval to files' current date",
                "Subtract interval from files' curr. date",
                "Get from filename (Parts <%4>–<%9>)",
            ]
        );
    }

    /// The defaults: every component ticked, Modified alone.
    #[test]
    fn a_fresh_card_changes_every_component_of_the_modified_date_only() {
        let op = SetDate::default();
        assert!(op.change.all());
        assert!(!op.targets.created);
        assert!(!op.targets.accessed);
        assert!(op.targets.modified);
        assert_eq!(op.interval, 1);
        assert_eq!(op.unit, IntervalUnit::Years);
        assert_eq!(op.source, DateSource::EnterNewDate);
        assert_eq!(
            OpKind::SetDate(SetDate::default()).label(),
            "Set Date & Time"
        );
    }

    #[test]
    fn entering_a_date_writes_it_to_the_selected_targets_only() {
        let op = SetDate {
            date: WallClock::from_naive(naive(2008, 2, 17, 11, 23, 50)),
            ..Default::default()
        };
        let times = times_of(
            &op,
            &entry_dated(Some(system_at(1999, 5, 9, 10, 18, 5)), None),
        );
        assert_eq!(
            local_of(times.modified),
            Some(naive(2008, 2, 17, 11, 23, 50))
        );
        assert_eq!(times.created, None, "not selected");
        assert_eq!(times.accessed, None, "not selected");
    }

    /// A year-only mask, end to end through the operation.
    #[test]
    fn the_component_mask_changes_only_the_year() {
        let op = SetDate {
            date: WallClock::from_naive(naive(2008, 2, 17, 11, 23, 50)),
            change: DateComponents {
                year: true,
                month: false,
                day: false,
                hour: false,
                minute: false,
                second: false,
            },
            ..Default::default()
        };
        let times = times_of(
            &op,
            &entry_dated(Some(system_at(1999, 5, 9, 10, 18, 5)), None),
        );
        assert_eq!(
            local_of(times.modified),
            Some(naive(2008, 5, 9, 10, 18, 5)),
            "only the year moved"
        );
    }

    /// The decision this test exists for: each selected stamp shifts from its
    /// **own** value, not from a single shared one.
    #[test]
    fn add_interval_moves_each_selected_target_relative_to_itself() {
        let op = SetDate {
            source: DateSource::AddInterval,
            interval: 1,
            unit: IntervalUnit::Days,
            targets: DateTargets {
                created: true,
                accessed: false,
                modified: true,
            },
            ..Default::default()
        };
        let entry = entry_dated(
            Some(system_at(2001, 6, 15, 12, 0, 0)),
            Some(system_at(1999, 1, 2, 3, 4, 5)),
        );
        let times = times_of(&op, &entry);
        assert_eq!(local_of(times.modified), Some(naive(2001, 6, 16, 12, 0, 0)));
        assert_eq!(
            local_of(times.created),
            Some(naive(1999, 1, 3, 3, 4, 5)),
            "created moved from its own value, not from modified's"
        );
    }

    #[test]
    fn subtracting_an_interval_moves_the_other_way() {
        let op = SetDate {
            source: DateSource::SubtractInterval,
            interval: 2,
            unit: IntervalUnit::Hours,
            ..Default::default()
        };
        let times = times_of(
            &op,
            &entry_dated(Some(system_at(2001, 6, 15, 12, 0, 0)), None),
        );
        assert_eq!(local_of(times.modified), Some(naive(2001, 6, 15, 10, 0, 0)));
    }

    #[test]
    fn use_files_created_date_copies_one_timestamp_onto_another() {
        let op = SetDate {
            source: DateSource::FileCreated,
            ..Default::default()
        };
        let entry = entry_dated(
            Some(system_at(2001, 6, 15, 12, 0, 0)),
            Some(system_at(1999, 1, 2, 3, 4, 5)),
        );
        assert_eq!(
            local_of(times_of(&op, &entry).modified),
            Some(naive(1999, 1, 2, 3, 4, 5))
        );
    }

    /// D28 makes this one instant for the whole run, so a batch agrees.
    #[test]
    fn now_is_the_time_at_the_start_of_the_rename_not_per_file() {
        let entries = [
            entry_dated(Some(system_at(2001, 1, 1, 0, 0, 0)), None),
            entry_dated(Some(system_at(2002, 2, 2, 0, 0, 0)), None),
        ];
        let op = SetDate {
            source: DateSource::NowAtStartOfRun,
            ..Default::default()
        };
        let run = RunContext::build(&entries, &RunSettings::default(), Answers::default());
        let stamps: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let cx = EvalCx::new(e, i, entries.len(), &run);
                match op.effect(&cx).unwrap() {
                    Some(Effect::Times(t)) => t.modified,
                    other => panic!("{other:?}"),
                }
            })
            .collect();
        assert_eq!(stamps[0], stamps[1], "one timestamp for the whole run");
    }

    #[test]
    fn get_from_filename_reads_the_date_from_the_parts() {
        let op = SetDate {
            source: DateSource::FromFilename,
            ..Default::default()
        };
        let entry = entry_dated(Some(system_at(1999, 5, 9, 10, 18, 5)), None);
        let settings = RunSettings {
            parts: crate::parts::PartsSpec::new("<%1> <%2> <%4>-<%5>-<%6>"),
            ..Default::default()
        };
        let run = RunContext::build(std::slice::from_ref(&entry), &settings, Answers::default());
        let cx = EvalCx::new(&entry, 0, 1, &run);

        match op.effect(&cx).unwrap() {
            Some(Effect::Times(t)) => assert_eq!(
                local_of(t.modified),
                Some(naive(2000, 12, 31, 0, 0, 0)),
                "date from the name, time defaulting to midnight"
            ),
            other => panic!("{other:?}"),
        }
    }

    /// The user's call: `Photo 99-12-31.jpg` means 1999.
    #[test]
    fn get_from_filename_reads_a_two_digit_year_through_the_dos_window() {
        let op = SetDate {
            source: DateSource::FromFilename,
            ..Default::default()
        };
        let mut entry = FileEntry::synthetic("/photos/Photo 99-12-31.jpg");
        entry.modified = Some(system_at(2020, 1, 1, 0, 0, 0));
        let settings = RunSettings {
            parts: crate::parts::PartsSpec::new("<%1> <%4>-<%5>-<%6>"),
            ..Default::default()
        };
        let run = RunContext::build(std::slice::from_ref(&entry), &settings, Answers::default());
        let cx = EvalCx::new(&entry, 0, 1, &run);

        match op.effect(&cx).unwrap() {
            Some(Effect::Times(t)) => {
                assert_eq!(local_of(t.modified), Some(naive(1999, 12, 31, 0, 0, 0)))
            }
            other => panic!("{other:?}"),
        }
    }

    /// A file the pattern does not describe is left alone, not failed.
    #[test]
    fn a_file_the_parts_pattern_does_not_describe_is_left_alone() {
        let op = SetDate {
            source: DateSource::FromFilename,
            ..Default::default()
        };
        let mut entry = FileEntry::synthetic("/photos/no date here.jpg");
        entry.modified = Some(system_at(2020, 1, 1, 0, 0, 0));
        let settings = RunSettings {
            parts: crate::parts::PartsSpec::new("<%1> <%4>-<%5>-<%6>"),
            ..Default::default()
        };
        let run = RunContext::build(std::slice::from_ref(&entry), &settings, Answers::default());
        let cx = EvalCx::new(&entry, 0, 1, &run);

        match op.effect(&cx).unwrap() {
            Some(Effect::Times(t)) => assert!(t.is_empty(), "nothing to write: {t:?}"),
            other => panic!("{other:?}"),
        }
    }

    /// `FileEntry::synthetic` reports no timestamps at all, which is a real
    /// case: a partial mask has nothing to merge into.
    #[test]
    fn a_target_the_filesystem_did_not_report_cannot_be_partially_changed() {
        let bare = FileEntry::synthetic("/photos/a.jpg");
        let partial = SetDate {
            date: WallClock::from_naive(naive(2008, 2, 17, 11, 23, 50)),
            change: DateComponents {
                year: true,
                ..DateComponents {
                    year: false,
                    month: false,
                    day: false,
                    hour: false,
                    minute: false,
                    second: false,
                }
            },
            ..Default::default()
        };
        assert!(
            effect_of(&partial, &bare).is_err(),
            "a partial mask needs a previous value and must say so"
        );

        // An all-on mask needs nothing from the file.
        let whole = SetDate {
            date: WallClock::from_naive(naive(2008, 2, 17, 11, 23, 50)),
            ..Default::default()
        };
        assert_eq!(
            local_of(times_of(&whole, &bare).modified),
            Some(naive(2008, 2, 17, 11, 23, 50))
        );
    }

    #[test]
    fn a_date_outside_the_supported_range_is_an_error() {
        let op = SetDate {
            date: WallClock {
                year: 1970,
                ..WallClock::default()
            },
            ..Default::default()
        };
        let err = effect_of(
            &op,
            &entry_dated(Some(system_at(2000, 1, 1, 0, 0, 0)), None),
        )
        .expect_err("1970 is outside 1980–2099");
        assert!(err.to_string().contains("1980"), "{err}");
    }

    /// The range applies to what will be written, not to where it came from.
    #[test]
    fn the_range_is_checked_on_what_will_be_written_not_on_the_source() {
        let op = SetDate {
            date: WallClock {
                year: 2150,
                month: 6,
                day: 1,
                hour: 9,
                minute: 0,
                second: 0,
            },
            // Year unticked, so 2150 never reaches disk.
            change: DateComponents {
                year: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let times = times_of(
            &op,
            &entry_dated(Some(system_at(2001, 5, 9, 10, 18, 5)), None),
        );
        assert_eq!(
            local_of(times.modified),
            Some(naive(2001, 6, 1, 9, 0, 0)),
            "the out-of-range year was masked out, so the row is fine"
        );
    }

    #[test]
    fn a_card_with_no_target_or_no_component_does_nothing() {
        let no_target = SetDate {
            targets: DateTargets {
                created: false,
                accessed: false,
                modified: false,
            },
            ..Default::default()
        };
        assert!(no_target.is_empty());
        assert_eq!(
            effect_of(&no_target, &entry_dated(None, None)).unwrap(),
            None
        );
        assert_eq!(no_target.summary(), "Set Date & Time (no date selected)");

        let no_change = SetDate {
            change: DateComponents {
                year: false,
                month: false,
                day: false,
                hour: false,
                minute: false,
                second: false,
            },
            ..Default::default()
        };
        assert!(no_change.is_empty());
        assert_eq!(no_change.summary(), "Set Date & Time (nothing to change)");
    }

    #[test]
    fn it_says_what_it_will_do_to_a_row() {
        let op = SetDate {
            date: WallClock::from_naive(naive(2008, 2, 17, 11, 23, 50)),
            ..Default::default()
        };
        let entry = entry_dated(Some(system_at(1999, 5, 9, 10, 18, 5)), None);
        let effect = effect_of(&op, &entry).unwrap().unwrap();
        assert_eq!(op.describe(&effect), "Modified → 2008-02-17 11:23:50");
    }

    /// It says out loud that the Exif source opens files, as `<Crc32>` does.
    #[test]
    fn the_exif_source_declares_that_it_reads_the_file() {
        let exif = SetDate {
            source: DateSource::ImageExif,
            ..Default::default()
        };
        assert!(exif.needs().contains(TagNeeds::FILE_CONTENT));

        let filename = SetDate {
            source: DateSource::FromFilename,
            ..Default::default()
        };
        assert!(filename.needs().contains(TagNeeds::PARTS));
        assert_eq!(SetDate::default().needs(), TagNeeds::NONE);
    }

    #[test]
    fn it_is_an_action_and_round_trips_through_a_job_file() {
        let op = OpKind::SetDate(SetDate {
            source: DateSource::ImageExif,
            targets: DateTargets {
                created: true,
                accessed: false,
                modified: true,
            },
            ..Default::default()
        });
        assert_eq!(op.produces(), crate::ops::Produces::Action);
        assert!(matches!(op.to_step(), crate::pipeline::Step::Action(_)));

        let OpKind::SetDate(inner) = &op else {
            unreachable!()
        };
        let back: SetDate = toml::from_str(&toml::to_string(inner).unwrap()).unwrap();
        assert_eq!(back, *inner);
    }
}
