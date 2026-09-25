//! Parts — `<%1>` … `<%9>`.
//!
//! Parts is how you say what shape your filenames already have: the pieces are
//! separated by characters common to all of them.
//!
//! One example shows all of it:
//!
//! ```text
//! name    01. Metallica (S&M) Nothing Else Matters
//! parts   <%1>. <%2> (<%3>) <%4>
//! format  <%2> - <%1> - <%4>
//!    →    Metallica - 01 - Nothing Else Matters
//! ```

use serde::{Deserialize, Serialize};

/// The pattern that says how a filename is put together.
///
/// Stored as the raw text the user typed, so a half-finished pattern is just
/// text rather than a parse error to fight.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PartsSpec {
    pub pattern: String,
}

impl PartsSpec {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pattern.trim().is_empty()
    }

    /// Splits `name` into its parts.
    ///
    /// Returns slot → text for slots 1–9. A slot the pattern does not mention,
    /// or that the name does not reach, is absent — which is what makes
    /// "Only rename if all tags are available" able to skip a file.
    ///
    /// Compiles the pattern every call. The engine does not go through here:
    /// `RunContext::build` compiles once per run and hands
    /// [`CompiledParts::split`] to every file, because this used to run once
    /// per file per keystroke and allocate the whole segment list each time.
    pub fn split(&self, name: &str) -> Parts {
        self.compile().split(name)
    }

    /// The pattern, parsed once, for a run's worth of names.
    pub fn compile(&self) -> CompiledParts {
        CompiledParts {
            segments: compile(&self.pattern),
        }
    }

    /// The magic wand: guesses a parts pattern from one sample name.
    ///
    /// The separators are the fourteen that bracket or divide a filename — a
    /// spaced dash, a dot and a space, and each bracket pair, open and closed,
    /// with and without a space outside it. Earliest match wins; at the same
    /// position the longest does, so `" ("` beats `"("`.
    ///
    /// All fourteen, not the five this used to carry. The closers were missing,
    /// which meant the wand could open a bracketed part and never close it —
    /// so it could not reproduce the pattern in the module example.
    pub fn detect(sample: &str) -> Self {
        const SEPARATORS: [&str; 14] = [
            " - ", ". ", " (", ") ", "(", ")", " [", "] ", "[", "]", " {", "} ", "{", "}",
        ];

        let mut pattern = String::new();
        let mut slot = 1u8;
        let mut rest = sample;

        while slot < 9 {
            let Some((at, separator)) = SEPARATORS
                .iter()
                .filter_map(|s| rest.find(s).map(|at| (at, *s)))
                .min_by_key(|(at, s)| (*at, std::cmp::Reverse(s.len())))
            else {
                break;
            };
            if at == 0 {
                // A separator at the very start would make an empty part.
                break;
            }
            pattern.push_str(&format!("<%{slot}>"));
            pattern.push_str(separator);
            rest = &rest[at + separator.len()..];
            slot += 1;
        }

        pattern.push_str(&format!("<%{slot}>"));
        Self { pattern }
    }
}

/// A [`PartsSpec`] parsed into its literal separators and slots.
///
/// Held by `RunContext` (D28's serial pre-pass) rather than re-derived per
/// file: the pattern is run-wide and constant, and parsing it is a `Vec` of
/// owned `String`s — exactly the per-file allocation the pre-pass exists to
/// hoist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompiledParts {
    /// `None` for a pattern with no slot in it, which describes nothing.
    segments: Option<Vec<Segment>>,
}

impl CompiledParts {
    /// See [`PartsSpec::split`].
    pub fn split(&self, name: &str) -> Parts {
        let Some(segments) = &self.segments else {
            return Parts::default();
        };
        Parts {
            values: extract(segments, name),
        }
    }
}

/// The parts of one filename.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parts {
    values: Vec<(u8, String)>,
}

impl Parts {
    pub fn get(&self, slot: u8) -> Option<&str> {
        self.values
            .iter()
            .find(|(n, _)| *n == slot)
            .map(|(_, v)| v.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// The pattern as alternating placeholders and literal separators.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Slot(u8),
    Literal(String),
}

fn compile(pattern: &str) -> Option<Vec<Segment>> {
    if pattern.trim().is_empty() {
        return None;
    }
    let mut segments = Vec::new();
    let mut literal = String::new();
    let bytes: Vec<char> = pattern.chars().collect();
    let mut i = 0;

    while i < bytes.len() {
        // `<%n>`
        if bytes[i] == '<'
            && i + 3 < bytes.len()
            && bytes[i + 1] == '%'
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3] == '>'
        {
            let slot = bytes[i + 2] as u8 - b'0';
            if slot >= 1 {
                if !literal.is_empty() {
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                }
                segments.push(Segment::Slot(slot));
                i += 4;
                continue;
            }
        }
        literal.push(bytes[i]);
        i += 1;
    }
    if !literal.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    segments
        .iter()
        .any(|s| matches!(s, Segment::Slot(_)))
        .then_some(segments)
}

/// Greedy left to right: each part runs up to the next literal separator.
fn extract(segments: &[Segment], name: &str) -> Vec<(u8, String)> {
    let mut values = Vec::new();
    let mut rest = name;
    let mut index = 0;

    while index < segments.len() {
        match &segments[index] {
            Segment::Literal(text) => {
                // The name must continue with the separator here, or the
                // pattern does not describe this file.
                let Some(after) = rest.strip_prefix(text.as_str()) else {
                    return values;
                };
                rest = after;
                index += 1;
            }
            Segment::Slot(slot) => {
                let next_literal = segments.get(index + 1).and_then(|s| match s {
                    Segment::Literal(text) => Some(text.as_str()),
                    Segment::Slot(_) => None,
                });
                let value = match next_literal {
                    Some(separator) => match rest.find(separator) {
                        Some(at) => {
                            let value = &rest[..at];
                            rest = &rest[at..];
                            value
                        }
                        // The separator is missing, so this part and everything
                        // after it are unavailable.
                        None => return values,
                    },
                    // Last slot, or two slots in a row: take the rest.
                    None => {
                        let value = rest;
                        rest = "";
                        value
                    }
                };
                values.push((*slot, value.to_owned()));
                index += 1;
            }
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The module example, end to end.
    #[test]
    fn the_module_example_splits_into_four_parts() {
        let spec = PartsSpec::new("<%1>. <%2> (<%3>) <%4>");
        let parts = spec.split("01. Metallica (S&M) Nothing Else Matters");

        assert_eq!(parts.get(1), Some("01"));
        assert_eq!(parts.get(2), Some("Metallica"));
        assert_eq!(parts.get(3), Some("S&M"));
        assert_eq!(parts.get(4), Some("Nothing Else Matters"));
    }

    #[test]
    fn a_two_part_pattern_splits_on_its_separator() {
        let spec = PartsSpec::new("<%1>-<%2>");
        let parts = spec.split("Artist-Title");
        assert_eq!(parts.get(1), Some("Artist"));
        assert_eq!(parts.get(2), Some("Title"));
    }

    /// Greedy left to right: the *first* separator ends the part.
    #[test]
    fn parts_are_taken_greedily_from_the_left() {
        let spec = PartsSpec::new("<%1>-<%2>");
        let parts = spec.split("a-b-c");
        assert_eq!(parts.get(1), Some("a"));
        assert_eq!(parts.get(2), Some("b-c"), "the last part takes the rest");
    }

    #[test]
    fn a_name_the_pattern_does_not_describe_yields_what_it_can() {
        let spec = PartsSpec::new("<%1>. <%2> (<%3>)");
        let parts = spec.split("no separators here");
        assert_eq!(parts.get(1), None);
        assert!(parts.is_empty());
    }

    #[test]
    fn a_partially_matching_name_gives_up_at_the_missing_separator() {
        let spec = PartsSpec::new("<%1>. <%2> (<%3>)");
        let parts = spec.split("01. Metallica");
        assert_eq!(parts.get(1), Some("01"));
        assert_eq!(parts.get(2), None, "no ' (' to close part 2");
    }

    #[test]
    fn an_empty_pattern_produces_no_parts() {
        assert!(PartsSpec::default().split("anything").is_empty());
        assert!(PartsSpec::new("   ").split("anything").is_empty());
        assert!(PartsSpec::new("no slots").split("anything").is_empty());
    }

    #[test]
    fn slots_may_appear_in_any_order_and_be_skipped() {
        let spec = PartsSpec::new("<%3>-<%1>");
        let parts = spec.split("first-second");
        assert_eq!(parts.get(3), Some("first"));
        assert_eq!(parts.get(1), Some("second"));
        assert_eq!(parts.get(2), None);
    }

    #[test]
    fn a_leading_literal_must_match() {
        let spec = PartsSpec::new("Track <%1>");
        assert_eq!(spec.split("Track 07").get(1), Some("07"));
        assert_eq!(spec.split("Song 07").get(1), None);
    }

    /// The wand, on the module example's name.
    #[test]
    fn auto_detect_reproduces_the_module_example_pattern() {
        let sample = "01. Metallica (S&M) Nothing Else Matters";
        let spec = PartsSpec::detect(sample);
        let parts = spec.split(sample);
        assert_eq!(parts.get(1), Some("01"), "pattern was {:?}", spec.pattern);
        assert_eq!(
            parts.get(2),
            Some("Metallica"),
            "pattern {:?}",
            spec.pattern
        );
        // The bracketed part is *closed*, which is the half that needed all
        // fourteen separators. With only the five openers the wand could open
        // `(` and never find `)`, so everything after it collapsed into one
        // part and the module example could not be reproduced.
        assert_eq!(parts.get(3), Some("S&M"), "pattern {:?}", spec.pattern);
    }

    /// Every closer in the separator list, not just the openers.
    #[test]
    fn the_wand_closes_the_brackets_it_opens() {
        for (sample, inner) in [
            ("Artist (Live) Title", "Live"),
            ("Artist [Demo] Title", "Demo"),
            ("Artist {Remix} Title", "Remix"),
        ] {
            let spec = PartsSpec::detect(sample);
            let parts = spec.split(sample);
            assert_eq!(
                parts.get(2),
                Some(inner),
                "{sample} -> pattern {:?}",
                spec.pattern
            );
        }
    }

    #[test]
    fn auto_detect_finds_the_common_dash_separator() {
        let spec = PartsSpec::detect("Artist - Title");
        assert_eq!(spec.pattern, "<%1> - <%2>");
        let parts = spec.split("Artist - Title");
        assert_eq!(parts.get(1), Some("Artist"));
        assert_eq!(parts.get(2), Some("Title"));
    }

    #[test]
    fn auto_detect_on_a_name_with_no_separators_makes_one_part() {
        let spec = PartsSpec::detect("singleword");
        assert_eq!(spec.pattern, "<%1>");
        assert_eq!(spec.split("singleword").get(1), Some("singleword"));
    }

    #[test]
    fn detected_patterns_round_trip_through_serde() {
        let spec = PartsSpec::detect("Artist - Title");
        let text = toml::to_string(&Wrapper {
            parts: spec.clone(),
        })
        .unwrap();
        let back: Wrapper = toml::from_str(&text).unwrap();
        assert_eq!(back.parts, spec);
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Wrapper {
        parts: PartsSpec,
    }
}
