//! Rearrange — a new name built from the old one's blocks (D258).
//!
//! The name is cut into numbered blocks, and the new name is laid out from
//! them, from literal text and from tags:
//!
//! ```text
//! name    Dune - Frank Herbert
//! split   <%1> - <%2>
//! layout  [2] " - " [1]
//!    →    Frank Herbert - Dune
//! ```
//!
//! The split is either the filename-parts language (`<%1> - <%2>`, the same
//! one `<%n>` tags use, so a pattern learned there means the same here) or a
//! regular expression whose capture groups are the blocks. The GUI's builder
//! writes this; a power user can write it by hand.
//!
//! A file the split does not describe is left alone and **says so** through
//! [`NameTransform::warn`] — the list flags it rather than renaming it to
//! the gaps between its missing blocks.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::casing::{CaseMode, Casing};
use super::{EvalCx, NameTransform, OpError};
use crate::cache::Cached;
use crate::parts::{CompiledParts, PartsSpec};
use crate::regex_flavor::{Pattern, PatternOptions, RegexError};
use crate::run::AskSpec;
use crate::template::{TagNeeds, TextTemplate};

/// How a name is cut into blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    /// `<%1> - <%2>`: blocks between literal separators, in order.
    Parts(PartsSpec),
    /// Capture groups 1–9 are the blocks.
    Regex(String),
}

impl Default for Split {
    fn default() -> Self {
        Self::Parts(PartsSpec::default())
    }
}

/// One piece of the new name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Piece {
    /// Block `slot` (1–9) of the old name.
    Part {
        slot: u8,
        #[serde(default = "as_is", skip_serializing_if = "is_as_is")]
        case: CaseMode,
        /// Strip spaces around it — a separator that was ` - ` but typed as
        /// `-` leaves them on the blocks.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        trim: bool,
    },
    Text(String),
    /// Any tag the template engine knows: `<Counter>`, `<Date>`, `<Artist>`…
    Tag(TextTemplate),
}

fn as_is() -> CaseMode {
    CaseMode::NoChange
}

fn is_as_is(case: &CaseMode) -> bool {
    *case == CaseMode::NoChange
}

impl Piece {
    pub fn part(slot: u8) -> Self {
        Self::Part {
            slot,
            case: CaseMode::NoChange,
            trim: false,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rearrange {
    pub split: Split,
    pub layout: Vec<Piece>,
    /// The name the rule was built on: what the card's summary and the
    /// builder show it with. Never read by the engine.
    pub sample: String,
    /// For [`Split::Regex`] and the separators alike.
    pub case_sensitive: bool,

    /// Deliberately private, as on every op that caches (D21).
    #[serde(skip)]
    compiled: Cached<Result<Compiled, RegexError>>,
}

impl Clone for Rearrange {
    fn clone(&self) -> Self {
        Self::new(self.split.clone(), self.layout.clone())
            .with_sample(self.sample.clone())
            .matching_case(self.case_sensitive)
    }
}

impl PartialEq for Rearrange {
    fn eq(&self, other: &Self) -> bool {
        self.split == other.split
            && self.layout == other.layout
            && self.sample == other.sample
            && self.case_sensitive == other.case_sensitive
    }
}

impl Eq for Rearrange {}

#[derive(Debug)]
enum Compiled {
    Parts(CompiledParts),
    Regex(Pattern),
}

impl Rearrange {
    pub fn new(split: Split, layout: Vec<Piece>) -> Self {
        Self {
            split,
            layout,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn with_sample(mut self, sample: impl Into<String>) -> Self {
        self.sample = sample.into();
        self
    }

    #[must_use]
    pub fn matching_case(mut self, yes: bool) -> Self {
        self.case_sensitive = yes;
        self
    }

    fn compiled(&self) -> Result<&Compiled, OpError> {
        self.compiled
            .get_or_init(|| match &self.split {
                Split::Parts(spec) => Ok(Compiled::Parts(spec.compile())),
                Split::Regex(pattern) => Pattern::compile(
                    pattern,
                    PatternOptions {
                        case_sensitive: self.case_sensitive,
                        ..Default::default()
                    },
                )
                .map(Compiled::Regex),
            })
            .as_ref()
            .map_err(|e| OpError::new("rearrange", e))
    }

    /// The blocks of `name`, by slot; `None` where the split does not reach.
    pub fn blocks(&self, name: &str) -> Result<[Option<String>; 9], OpError> {
        let mut out: [Option<String>; 9] = Default::default();
        match self.compiled()? {
            Compiled::Parts(parts) => {
                let split = parts.split(name);
                for slot in 1..=9u8 {
                    out[usize::from(slot - 1)] = split.get(slot).map(str::to_owned);
                }
            }
            Compiled::Regex(pattern) => {
                if let Some(groups) = pattern
                    .captures(name)
                    .map_err(|e| OpError::new("rearrange", e))?
                {
                    for (slot, group) in groups.into_iter().skip(1).take(9).enumerate() {
                        out[slot] = group;
                    }
                }
            }
        }
        Ok(out)
    }

    /// The new name for `name` with no file to ask, for the builder's
    /// preview: tags show as themselves. `None` when it does not fit.
    pub fn preview(&self, name: &str) -> Result<Option<String>, OpError> {
        let blocks = self.blocks(name)?;
        let mut out = String::new();
        for piece in &self.layout {
            match piece {
                Piece::Part { slot, case, trim } => {
                    let Some(Some(text)) = blocks.get(usize::from(slot.saturating_sub(1))) else {
                        return Ok(None);
                    };
                    let text = if *trim { text.trim() } else { text.as_str() };
                    out.push_str(&Casing::new(*case).transform(text));
                }
                Piece::Text(text) => out.push_str(text),
                Piece::Tag(tag) => out.push_str(tag.as_str()),
            }
        }
        Ok(Some(out))
    }

    /// The slots the layout uses, which every file must have to fit.
    fn slots(&self) -> impl Iterator<Item = u8> + '_ {
        self.layout.iter().filter_map(|piece| match piece {
            Piece::Part { slot, .. } => Some(*slot),
            _ => None,
        })
    }

    /// Why `name` does not fit, if it does not.
    pub fn misfit(&self, name: &str) -> Result<Option<String>, OpError> {
        let blocks = self.blocks(name)?;
        let fits = self.slots().all(|slot| {
            blocks
                .get(usize::from(slot.saturating_sub(1)))
                .is_some_and(Option::is_some)
        });
        Ok((!fits).then(|| {
            "Does not have the shape of the Rearrange rule, so it is left alone".to_owned()
        }))
    }
}

impl NameTransform for Rearrange {
    fn id(&self) -> &'static str {
        "rearrange"
    }

    fn summary(&self) -> String {
        if self.layout.is_empty() {
            return "Rearrange (no rule yet)".to_owned();
        }
        let sample = self.sample.as_str();
        let labels = self.blocks(sample).unwrap_or_default();
        let shown: String = self
            .layout
            .iter()
            .map(|piece| match piece {
                Piece::Part { slot, .. } => labels
                    .get(usize::from(slot.saturating_sub(1)))
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| format!("<%{slot}>")),
                Piece::Text(text) => text.clone(),
                Piece::Tag(tag) => tag.as_str().to_owned(),
            })
            .collect();
        if sample.is_empty() {
            format!("Rearrange into {shown:?}")
        } else {
            format!("{sample} → {shown}")
        }
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        // A card with no rule yet changes nothing, as Free Format's empty
        // pattern does — otherwise one keystroke would empty every name.
        if self.layout.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }
        let blocks = self.blocks(subject)?;
        let mut out = String::with_capacity(subject.len() + 8);
        for piece in &self.layout {
            match piece {
                Piece::Part { slot, case, trim } => {
                    let Some(Some(text)) = blocks.get(usize::from(slot.saturating_sub(1))) else {
                        // Does not fit: left alone, and `warn` says why.
                        return Ok(Cow::Borrowed(subject));
                    };
                    let text = if *trim { text.trim() } else { text.as_str() };
                    out.push_str(&Casing::new(*case).transform(text));
                }
                Piece::Text(text) => out.push_str(text),
                Piece::Tag(tag) => match cx.render(tag)? {
                    Some(value) => out.push_str(&value),
                    // A tag this file cannot fill leaves the name alone (P32).
                    None => return Ok(Cow::Borrowed(subject)),
                },
            }
        }
        Ok(if out == subject {
            Cow::Borrowed(subject)
        } else {
            Cow::Owned(out)
        })
    }

    fn warn(&self, subject: &str, _cx: &EvalCx<'_>) -> Option<String> {
        if self.layout.is_empty() {
            return None;
        }
        self.misfit(subject).ok().flatten()
    }

    fn needs(&self) -> TagNeeds {
        self.layout
            .iter()
            .filter_map(|piece| match piece {
                Piece::Tag(tag) => Some(tag.needs()),
                _ => None,
            })
            .fold(TagNeeds::NONE, TagNeeds::union)
    }

    fn asks(&self) -> Vec<AskSpec> {
        self.layout
            .iter()
            .filter_map(|piece| match piece {
                Piece::Tag(tag) => Some(tag.asks()),
                _ => None,
            })
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileEntry;
    use crate::pipeline::{Pipeline, StepConfig};
    use crate::{OpKind, evaluate_all};

    fn swap() -> Rearrange {
        Rearrange::new(
            Split::Parts(PartsSpec::new("<%1> - <%2>")),
            vec![Piece::part(2), Piece::Text(" - ".into()), Piece::part(1)],
        )
    }

    /// Runs the op the way the app does: a pipeline step scoped to the name.
    fn run(op: Rearrange, path: &str) -> (String, Vec<String>) {
        let entry = FileEntry::synthetic(path);
        let pipeline = Pipeline::new().with(
            crate::pipeline::Step::Name(Box::new(op)),
            StepConfig::for_op(&OpKind::Rearrange(Rearrange::default())),
        );
        let ev = evaluate_all(std::slice::from_ref(&entry), &pipeline)
            .pop()
            .unwrap()
            .unwrap();
        (ev.name, ev.warnings)
    }

    /// The user's own example, across two extensions.
    #[test]
    fn book_title_and_author_swap_places_and_the_extension_stays() {
        assert_eq!(
            run(swap(), "/b/Dune - Frank Herbert.pdf").0,
            "Frank Herbert - Dune.pdf"
        );
        assert_eq!(
            run(swap(), "/b/Emma - Jane Austen.epub").0,
            "Jane Austen - Emma.epub"
        );
    }

    #[test]
    fn a_name_that_does_not_fit_is_left_alone_and_says_so() {
        let (name, warnings) = run(swap(), "/b/Notes.txt");
        assert_eq!(name, "Notes.txt");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("left alone"), "{warnings:?}");
        assert!(run(swap(), "/b/Dune - Frank Herbert.pdf").1.is_empty());
    }

    #[test]
    fn blocks_can_be_dropped_cased_trimmed_and_joined_with_text() {
        let op = Rearrange::new(
            Split::Parts(PartsSpec::new("<%1>-<%2>-<%3>")),
            vec![
                Piece::Part {
                    slot: 3,
                    case: CaseMode::Upper,
                    trim: true,
                },
                Piece::Text(", ".into()),
                Piece::Part {
                    slot: 1,
                    case: CaseMode::Title,
                    trim: true,
                },
            ],
        );
        assert_eq!(
            run(op, "/x/the hobbit - 1937 - tolkien.pdf").0,
            "TOLKIEN, The Hobbit.pdf"
        );
    }

    #[test]
    fn a_tag_block_is_filled_per_file() {
        let op = Rearrange::new(
            Split::Parts(PartsSpec::new("<%1> - <%2>")),
            vec![
                Piece::part(2),
                Piece::Text(" (".into()),
                Piece::Tag("<Parent>".into()),
                Piece::Text(")".into()),
            ],
        );
        assert_eq!(
            run(op, "/Sci-Fi/Dune - Frank Herbert.pdf").0,
            "Frank Herbert (Sci-Fi).pdf"
        );
    }

    #[test]
    fn a_regular_expression_cuts_by_its_groups() {
        let op = Rearrange::new(
            Split::Regex(r"^(\d{4})_(.+)$".into()),
            vec![Piece::part(2), Piece::Text(" ".into()), Piece::part(1)],
        );
        assert_eq!(run(op.clone(), "/p/2019_Holiday.jpg").0, "Holiday 2019.jpg");
        assert_eq!(run(op, "/p/Holiday.jpg").1.len(), 1, "no match is a misfit");
    }

    #[test]
    fn an_empty_rule_changes_nothing_and_warns_about_nothing() {
        let (name, warnings) = run(Rearrange::default(), "/b/Dune - Frank Herbert.pdf");
        assert_eq!(name, "Dune - Frank Herbert.pdf");
        assert!(warnings.is_empty());
    }

    #[test]
    fn the_preview_and_the_summary_speak_in_blocks() {
        let op = swap().with_sample("Dune - Frank Herbert");
        assert_eq!(
            op.preview("Emma - Jane Austen").unwrap().as_deref(),
            Some("Jane Austen - Emma")
        );
        assert_eq!(op.preview("Emma").unwrap(), None);
        assert_eq!(op.summary(), "Dune - Frank Herbert → Frank Herbert - Dune");
    }

    /// A preset or job file carries the rule and gives it back.
    #[test]
    fn the_rule_round_trips_through_a_job_file() {
        let op = OpKind::Rearrange(
            swap()
                .with_sample("Dune - Frank Herbert")
                .matching_case(true),
        );
        let text = toml::to_string(&op).unwrap();
        let back: OpKind = toml::from_str(&text).unwrap();
        assert_eq!(back, op, "{text}");
    }
}
