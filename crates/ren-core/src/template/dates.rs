//! The date format mini-language.
//!
//! The date/time format codes, which follow VB6's `Format`
//! reference. It has genuine oddities that a naive chrono mapping would get
//! wrong:
//!
//! * **`m` after `h` or `Hh` means *minute*, not month.** That is the
//!   difference between `10:05` and `10:18`. Separators and literal text in
//!   between do not break it — the default
//!   `Hh:Mm:Ss` would otherwise render the month.
//! * `N` and `Nn` are *also* minutes. We match those two **case-sensitively**,
//!   exactly as documented, so ordinary words survive: a
//!   case-insensitive `n` turns the literal "Taken" into "Take18".
//! * `\` escapes the next character, so any token letter can be written
//!   literally. VB6 has this; nothing documents it.
//! * `y` is the **day of the year** (1–366), not the year.
//! * `w` is the weekday with **Sunday = 1**.
//! * `Hh` and `Ss` are the zero-padded forms; `h` and `S` are not.
//! * Longer tokens win: `mmmm` before `mmm` before `mm` before `m`.
//!
//! **D30:** named formats render fixed, sortable output rather than following
//! the machine's regional settings. The same job file then produces the same
//! filenames everywhere — and a date can never emit `/`, which is illegal in a
//! filename on Windows.

use chrono::{DateTime, Datelike, Local, Timelike};

/// `yyyy-mm-dd Hh:Mm:Ss` — ISO 8601, and the most useful default because a
/// listing sorts correctly by it.
pub const DEFAULT_DATE: &str = "yyyy-mm-dd";
pub const DEFAULT_TIME: &str = "Hh.Mm.Ss";

/// One piece of a compiled format string.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Literal(String),
    /// Day of month, no leading zero.
    D,
    Dd,
    /// Abbreviated weekday.
    Ddd,
    /// Full weekday.
    Dddd,
    /// Complete short date.
    Ddddd,
    /// Complete long date.
    Dddddd,
    /// Weekday number, Sunday = 1.
    W,
    /// Week of year.
    Ww,
    M,
    Mm,
    Mmm,
    Mmmm,
    Q,
    /// Day of the year.
    Y,
    Yy,
    Yyyy,
    H,
    Hh,
    /// Minute, no leading zero.
    N,
    Nn,
    S,
    Ss,
    /// Complete time.
    Ttttt,
    /// Complete date and time.
    C,
    AmPm {
        upper: bool,
        short: bool,
    },
    TimeSeparator,
    DateSeparator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateFormat {
    tokens: Vec<Token>,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown date format {0:?}")]
pub struct UnknownFormat(pub String);

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

impl DateFormat {
    /// Compiles a format string, or resolves one of the named formats.
    pub fn parse(spec: &str) -> Self {
        if let Some(expanded) = named(spec) {
            let mut format = Self::compile(expanded);
            format.source = spec.to_owned();
            return format;
        }
        Self::compile(spec)
    }

    /// The named formats, rendered as fixed sortable strings
    /// (D30) instead of following the machine's regional settings.
    pub fn named(spec: &str) -> Option<&'static str> {
        named(spec)
    }

    fn compile(spec: &str) -> Self {
        let mut tokens = Vec::new();
        let chars: Vec<char> = spec.chars().collect();
        let mut i = 0;
        // Whether an hour token is still "in scope", which is what turns a
        // following `m` into a minute. Separators and literal text do not
        // clear it — `Hh:Mm:Ss` has to mean hour:minute:second.
        let mut after_hour = false;

        while i < chars.len() {
            // A backslash escapes the next character into a literal.
            if chars[i] == '\\' && i + 1 < chars.len() {
                let literal = Token::Literal(chars[i + 1].to_string());
                match (literal, tokens.last_mut()) {
                    (Token::Literal(text), Some(Token::Literal(previous))) => {
                        previous.push_str(&text)
                    }
                    (token, _) => tokens.push(token),
                }
                i += 2;
                continue;
            }

            let (token, len) = next_token(&chars, i, after_hour);
            after_hour = match token {
                Token::H | Token::Hh => true,
                Token::Literal(_) | Token::TimeSeparator | Token::DateSeparator => after_hour,
                _ => false,
            };
            match (token, tokens.last_mut()) {
                (Token::Literal(text), Some(Token::Literal(previous))) => previous.push_str(&text),
                (token, _) => tokens.push(token),
            }
            i += len;
        }

        Self {
            tokens,
            source: spec.to_owned(),
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn render(&self, time: std::time::SystemTime) -> String {
        let local: DateTime<Local> = time.into();
        // VB6's rule: an hour token is 12-hour when the format also carries an
        // AM/PM designator, which is the only thing that makes "Medium Time"
        // read as a clock rather than a duration.
        let twelve_hour = self.tokens.iter().any(|t| matches!(t, Token::AmPm { .. }));
        let mut out = String::with_capacity(self.source.len() + 8);
        for token in &self.tokens {
            render_token(token, &local, twelve_hour, &mut out);
        }
        out
    }
}

fn named(spec: &str) -> Option<&'static str> {
    // Case-insensitive, since tags are.
    let key = spec.trim().to_ascii_lowercase();
    Some(match key.as_str() {
        "general date" => "yyyy-mm-dd Hh.Mm.Ss",
        "long date" => "dddd d mmmm yyyy",
        "medium date" => "d mmm yyyy",
        "short date" => DEFAULT_DATE,
        "long time" => "Hh.Mm.Ss",
        "medium time" => "Hh.Mm AM/PM",
        "short time" => "Hh.Mm",
        _ => return None,
    })
}

/// Longest token wins, so `mmmm` is never read as `mm` + `mm`.
fn next_token(chars: &[char], at: usize, after_hour: bool) -> (Token, usize) {
    let rest: String = chars[at..].iter().collect();
    let lower = rest.to_ascii_lowercase();

    // Case-sensitive forms first: the documented forms distinguish `Hh` from `hh` only
    // by documenting `Hh`, and `AM/PM` from `am/pm` by case.
    for (pattern, token) in [
        (
            "AM/PM",
            Token::AmPm {
                upper: true,
                short: false,
            },
        ),
        (
            "am/pm",
            Token::AmPm {
                upper: false,
                short: false,
            },
        ),
        (
            "A/P",
            Token::AmPm {
                upper: true,
                short: true,
            },
        ),
        (
            "a/p",
            Token::AmPm {
                upper: false,
                short: true,
            },
        ),
        (
            "AMPM",
            Token::AmPm {
                upper: true,
                short: false,
            },
        ),
        // Case-sensitive, as documented: a case-insensitive `n`
        // would eat the letter out of every literal word.
        ("Nn", Token::Nn),
        ("N", Token::N),
    ] {
        if rest.starts_with(pattern) {
            return (token, pattern.chars().count());
        }
    }

    for (pattern, token) in [
        ("dddddd", Token::Dddddd),
        ("ddddd", Token::Ddddd),
        ("dddd", Token::Dddd),
        ("ddd", Token::Ddd),
        ("dd", Token::Dd),
        ("d", Token::D),
        ("ww", Token::Ww),
        ("w", Token::W),
        ("mmmm", Token::Mmmm),
        ("mmm", Token::Mmm),
        ("mm", if after_hour { Token::Nn } else { Token::Mm }),
        ("m", if after_hour { Token::N } else { Token::M }),
        ("q", Token::Q),
        ("yyyy", Token::Yyyy),
        ("yy", Token::Yy),
        ("y", Token::Y),
        ("hh", Token::Hh),
        ("h", Token::H),
        ("ss", Token::Ss),
        ("s", Token::S),
        ("ttttt", Token::Ttttt),
        ("c", Token::C),
    ] {
        if lower.starts_with(pattern) {
            return (token, pattern.chars().count());
        }
    }

    match chars[at] {
        ':' => (Token::TimeSeparator, 1),
        '/' => (Token::DateSeparator, 1),
        c => (Token::Literal(c.to_string()), 1),
    }
}

fn render_token(token: &Token, at: &DateTime<Local>, twelve_hour: bool, out: &mut String) {
    use std::fmt::Write;
    let hour = if twelve_hour {
        let h = at.hour() % 12;
        if h == 0 { 12 } else { h }
    } else {
        at.hour()
    };

    match token {
        Token::Literal(text) => out.push_str(text),
        Token::D => {
            let _ = write!(out, "{}", at.day());
        }
        Token::Dd => {
            let _ = write!(out, "{:02}", at.day());
        }
        Token::Ddd => out.push_str(&WEEKDAYS[weekday_index(at)][..3]),
        Token::Dddd => out.push_str(WEEKDAYS[weekday_index(at)]),
        Token::Ddddd => {
            let _ = write!(out, "{:04}-{:02}-{:02}", at.year(), at.month(), at.day());
        }
        Token::Dddddd => {
            let _ = write!(
                out,
                "{} {:02} {}",
                MONTHS[at.month0() as usize],
                at.day(),
                at.year()
            );
        }
        // "1 for Sunday through 7 for Saturday"
        Token::W => {
            let _ = write!(out, "{}", weekday_index(at) + 1);
        }
        Token::Ww => {
            let _ = write!(out, "{}", at.iso_week().week());
        }
        Token::M => {
            let _ = write!(out, "{}", at.month());
        }
        Token::Mm => {
            let _ = write!(out, "{:02}", at.month());
        }
        Token::Mmm => out.push_str(&MONTHS[at.month0() as usize][..3]),
        Token::Mmmm => out.push_str(MONTHS[at.month0() as usize]),
        Token::Q => {
            let _ = write!(out, "{}", (at.month() - 1) / 3 + 1);
        }
        // "Display the day of the year as a number (1 - 366)"
        Token::Y => {
            let _ = write!(out, "{}", at.ordinal());
        }
        Token::Yy => {
            let _ = write!(out, "{:02}", at.year().rem_euclid(100));
        }
        Token::Yyyy => {
            let _ = write!(out, "{:04}", at.year());
        }
        Token::H => {
            let _ = write!(out, "{hour}");
        }
        Token::Hh => {
            let _ = write!(out, "{hour:02}");
        }
        Token::N => {
            let _ = write!(out, "{}", at.minute());
        }
        Token::Nn => {
            let _ = write!(out, "{:02}", at.minute());
        }
        Token::S => {
            let _ = write!(out, "{}", at.second());
        }
        Token::Ss => {
            let _ = write!(out, "{:02}", at.second());
        }
        Token::Ttttt => {
            let _ = write!(
                out,
                "{:02}.{:02}.{:02}",
                at.hour(),
                at.minute(),
                at.second()
            );
        }
        Token::C => {
            let _ = write!(
                out,
                "{:04}-{:02}-{:02} {:02}.{:02}.{:02}",
                at.year(),
                at.month(),
                at.day(),
                at.hour(),
                at.minute(),
                at.second()
            );
        }
        Token::AmPm { upper, short } => {
            let pm = at.hour() >= 12;
            let text = match (pm, *short, *upper) {
                (false, false, true) => "AM",
                (true, false, true) => "PM",
                (false, false, false) => "am",
                (true, false, false) => "pm",
                (false, true, true) => "A",
                (true, true, true) => "P",
                (false, true, false) => "a",
                (true, true, false) => "p",
            };
            out.push_str(text);
        }
        // A colon is illegal in a Windows filename, so the time separator
        // renders as a period — the one place D30's "fixed output" rule has to
        // deviate from the documented character.
        Token::TimeSeparator => out.push('.'),
        Token::DateSeparator => out.push('-'),
    }
}

/// Sunday = 0, matching the `w` numbering.
fn weekday_index(at: &DateTime<Local>) -> usize {
    at.weekday().num_days_from_sunday() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// 1977-05-09 10:18:00 — the example timestamp, a Monday.
    fn sample() -> std::time::SystemTime {
        let local = Local.with_ymd_and_hms(1977, 5, 9, 10, 18, 5).unwrap();
        local.into()
    }

    fn render(spec: &str) -> String {
        DateFormat::parse(spec).render(sample())
    }

    /// "Dates & times are by default returned in the format yyyy-mm-dd Hh:Mm:Ss,
    /// for example 1977-05-09 10:18:00."
    #[test]
    fn the_default_format_is_the_documented_one() {
        assert_eq!(render(DEFAULT_DATE), "1977-05-09");
        assert_eq!(render(DEFAULT_TIME), "10.18.05");
    }

    /// The single nastiest rule: "If m immediately follows h or hh, the minute
    /// rather than the month is displayed."
    #[test]
    fn m_after_an_hour_means_minute() {
        assert_eq!(render("Hh:mm"), "10.18");
        assert_eq!(render("h:m"), "10.18");
        // Away from an hour it is still the month.
        assert_eq!(render("mm"), "05");
        assert_eq!(render("yyyy-mm-dd"), "1977-05-09");
        // And it reverts once something else intervenes.
        assert_eq!(render("Hh-dd-mm"), "10-09-05");
    }

    #[test]
    fn n_is_also_a_minute() {
        assert_eq!(render("N"), "18");
        assert_eq!(render("Nn"), "18");
        assert_eq!(render("Hh:Nn:Ss"), "10.18.05");
    }

    /// "y — Display the day of the year as a number (1 - 366)."
    #[test]
    fn lowercase_y_is_the_day_of_the_year() {
        assert_eq!(render("y"), "129");
        assert_eq!(render("yy"), "77");
        assert_eq!(render("yyyy"), "1977");
    }

    /// "w — Display the day of the week as a number (1 for Sunday …)"
    #[test]
    fn w_numbers_the_weekday_from_sunday() {
        // 1977-05-09 was a Monday.
        assert_eq!(render("w"), "2");
        assert_eq!(render("dddd"), "Monday");
        assert_eq!(render("ddd"), "Mon");
    }

    #[test]
    fn days_and_months_come_in_four_widths() {
        assert_eq!(render("d"), "9");
        assert_eq!(render("dd"), "09");
        assert_eq!(render("m"), "5");
        assert_eq!(render("mm"), "05");
        assert_eq!(render("mmm"), "May");
        assert_eq!(render("mmmm"), "May");
    }

    #[test]
    fn hours_seconds_and_quarters() {
        assert_eq!(render("h"), "10");
        assert_eq!(render("Hh"), "10");
        assert_eq!(render("S"), "5");
        assert_eq!(render("Ss"), "05");
        assert_eq!(render("q"), "2");
    }

    #[test]
    fn the_twelve_hour_designators_respect_case() {
        assert_eq!(render("AM/PM"), "AM");
        assert_eq!(render("am/pm"), "am");
        assert_eq!(render("A/P"), "A");
        assert_eq!(render("a/p"), "a");

        let afternoon: std::time::SystemTime =
            Local.with_ymd_and_hms(1977, 5, 9, 15, 0, 0).unwrap().into();
        assert_eq!(DateFormat::parse("AM/PM").render(afternoon), "PM");
        assert_eq!(DateFormat::parse("a/p").render(afternoon), "p");
    }

    /// D30: no separator a filename cannot contain.
    #[test]
    fn separators_never_emit_characters_illegal_in_a_filename() {
        assert_eq!(render("Hh:Nn"), "10.18", "a colon would be illegal");
        assert_eq!(render("d/m/yyyy"), "9-5-1977", "a slash would be illegal");
        assert_eq!(render("ttttt"), "10.18.05");
        assert_eq!(render("c"), "1977-05-09 10.18.05");
        assert_eq!(render("ddddd"), "1977-05-09");
    }

    /// The two worked examples.
    #[test]
    fn the_worked_examples_render() {
        assert_eq!(render("Short Date"), "1977-05-09");
        assert_eq!(render("dddd m mmmm"), "Monday 5 May");
    }

    #[test]
    fn every_named_format_resolves_and_is_case_insensitive() {
        for name in [
            "General Date",
            "Long Date",
            "Medium Date",
            "Short Date",
            "Long Time",
            "Medium Time",
            "Short Time",
        ] {
            assert!(DateFormat::named(name).is_some(), "{name}");
            assert!(DateFormat::named(&name.to_uppercase()).is_some(), "{name}");
            assert!(!render(name).is_empty(), "{name}");
        }
        assert!(DateFormat::named("Nonsense Date").is_none());
    }

    #[test]
    fn literal_text_survives_unchanged() {
        assert_eq!(render("Taken yyyy"), "Taken 1977");
        assert_eq!(render("[yyyy]"), "[1977]");
        assert_eq!(render(""), "");
    }

    #[test]
    fn longer_tokens_win_over_shorter_ones() {
        // "mmmm" must not be read as two "mm"s.
        assert_eq!(render("mmmm"), "May");
        assert_eq!(render("dddd"), "Monday");
        assert_eq!(render("yyyy"), "1977");
    }
}
