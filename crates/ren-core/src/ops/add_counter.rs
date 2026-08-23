//! Add Counter — *"Add Counter to Filename"*.
//!
//! Add Counter. The counter
//! itself is a global object ([`crate::counter::CounterSetup`]) resolved before
//! the parallel pass (D28); this operation only decides where its value goes.
//!
//! The form's own note explains why it is this thin: *"For more precise
//! placements and other options you can use the `<counter>` tag in other
//! functions."*

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};
use crate::template::TextTemplate;

/// *"Select where to place the counter, relative to the filename."*
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterPlacement {
    #[default]
    First,
    Last,
}

impl CounterPlacement {
    pub fn label(self) -> &'static str {
        match self {
            Self::First => "First",
            Self::Last => "Last",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AddCounter {
    pub placement: CounterPlacement,
    /// *"that will separate filename and the added number"* — `-` by
    /// default.
    pub separator: String,
    /// The radio pair: *"Keep current filename."* versus *"Replace current
    /// filename with:"*, whose field takes tags.
    pub replace_name: bool,
    pub replacement: TextTemplate,
}

impl Default for AddCounter {
    fn default() -> Self {
        Self {
            placement: CounterPlacement::First,
            separator: "-".to_owned(),
            replace_name: false,
            replacement: TextTemplate::default(),
        }
    }
}

impl AddCounter {
    pub fn new(placement: CounterPlacement, separator: impl Into<String>) -> Self {
        Self {
            placement,
            separator: separator.into(),
            ..Default::default()
        }
    }

    /// *"Replace current filename with:"*
    pub fn replacing(mut self, template: impl Into<TextTemplate>) -> Self {
        self.replace_name = true;
        self.replacement = template.into();
        self
    }
}

impl NameTransform for AddCounter {
    fn id(&self) -> &'static str {
        "add_counter"
    }

    fn summary(&self) -> String {
        let base = if self.replace_name {
            format!("replace name with {:?}", self.replacement.as_str())
        } else {
            "keep name".to_owned()
        };
        format!(
            "Counter {}, separator {:?}, {base}",
            self.placement.label().to_lowercase(),
            self.separator
        )
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        let base = if self.replace_name {
            match cx.render(&self.replacement)? {
                Some(text) => text,
                None => return Ok(Cow::Borrowed(subject)),
            }
        } else {
            Cow::Borrowed(subject)
        };

        let counter = cx.counter_text();
        let out = match self.placement {
            CounterPlacement::First => format!("{counter}{}{base}", self.separator),
            CounterPlacement::Last => format!("{base}{}{counter}", self.separator),
        };
        Ok(Cow::Owned(out))
    }

    fn needs(&self) -> crate::template::TagNeeds {
        self.replacement.needs()
    }

    fn asks(&self) -> Vec<crate::run::AskSpec> {
        self.replacement.asks()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::counter::CounterSetup;
    use crate::model::FileEntry;
    use crate::run::{Answers, RunContext, RunSettings};

    fn entries(count: usize) -> Vec<FileEntry> {
        (0..count)
            .map(|i| FileEntry::synthetic(format!("/music/file{i}.mp3")))
            .collect()
    }

    fn run_over(op: &AddCounter, names: &[&str], setup: CounterSetup) -> Vec<String> {
        let entries: Vec<FileEntry> = names
            .iter()
            .map(|n| FileEntry::synthetic(format!("/music/{n}")))
            .collect();
        let settings = RunSettings {
            counter: setup,
            ..Default::default()
        };
        let run = RunContext::build(&entries, &settings, Answers::default());
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let cx = EvalCx::new(e, i, entries.len(), &run);
                let (stem, _) = e.split();
                op.apply(stem, &cx).unwrap().into_owned()
            })
            .collect()
    }

    /// The worked example, with separator `--`:
    /// "001--firstfile.ext, 002--secondfile.ext, 003--thirdfile.ext".
    #[test]
    fn the_manuals_example_numbers_three_files() {
        let op = AddCounter::new(CounterPlacement::First, "--");
        let setup = CounterSetup {
            auto_pad: false,
            pad: 3,
            ..Default::default()
        };
        assert_eq!(
            run_over(
                &op,
                &["firstfile.ext", "secondfile.ext", "thirdfile.ext"],
                setup
            ),
            ["001--firstfile", "002--secondfile", "003--thirdfile"]
        );
    }

    #[test]
    fn the_counter_can_go_at_either_end() {
        let setup = CounterSetup::default();
        let first = AddCounter::new(CounterPlacement::First, "-");
        assert_eq!(run_over(&first, &["song.mp3"], setup.clone()), ["1-song"]);

        let last = AddCounter::new(CounterPlacement::Last, "-");
        assert_eq!(run_over(&last, &["song.mp3"], setup), ["song-1"]);
    }

    /// "Replace current filename with:" — and the field takes tags.
    #[test]
    fn replacing_the_name_uses_the_template_instead_of_the_current_name() {
        let op = AddCounter::new(CounterPlacement::Last, " ").replacing("<Parent>");
        assert_eq!(
            run_over(&op, &["song.mp3", "other.mp3"], CounterSetup::default()),
            ["music 1", "music 2"]
        );
    }

    #[test]
    fn auto_padding_makes_every_number_the_same_width() {
        let op = AddCounter::new(CounterPlacement::First, "-");
        let names: Vec<String> = (0..12).map(|i| format!("f{i}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let out = run_over(&op, &refs, CounterSetup::default());
        assert!(out[0].starts_with("01-"), "{}", out[0]);
        assert!(out[11].starts_with("12-"), "{}", out[11]);
    }

    #[test]
    fn a_broken_template_is_reported_rather_than_rendered_empty() {
        let op = AddCounter::new(CounterPlacement::First, "-").replacing("<Nmae>");
        let entries = entries(1);
        let run = RunContext::default();
        let cx = EvalCx::new(&entries[0], 0, 1, &run);
        assert!(op.apply("file0", &cx).is_err());
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = AddCounter::new(CounterPlacement::Last, " - ").replacing("<Counter>. <Name>");
        let text = toml::to_string(&op).unwrap();
        let back: AddCounter = toml::from_str(&text).unwrap();
        assert_eq!(back, op);
    }
}
