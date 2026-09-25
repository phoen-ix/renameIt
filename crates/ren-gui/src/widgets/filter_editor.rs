//! The include-filter popover.
//!
//! Include and exclude boxes, each
//! interpreted as plain text, wildcards or a regular expression, plus switches
//! for testing the whole path and the extension.

use ren_core::{IncludeFilter, MatchSpec};

/// How the text in a box should be read. Mirrors `MatchSpec` but is a plain
/// `Copy` value, so it can drive a radio group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Plain text, or wildcards if any of `* : ?` appear.
    #[default]
    Auto,
    Regex,
}

impl Mode {
    fn spec(self, text: &str) -> MatchSpec {
        match self {
            Self::Auto => MatchSpec::auto(text),
            Self::Regex => MatchSpec::Regex(text.to_owned()),
        }
    }
}

/// The editable form behind the filter chip.
///
/// Kept as strings rather than `MatchSpec`s so a half-typed regular expression
/// is just text, not a parse error the user has to fight.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterForm {
    pub include: String,
    pub exclude: String,
    pub mode: Mode,
    pub whole_path: bool,
    pub extension: bool,
    pub case_sensitive: bool,
}

impl FilterForm {
    pub fn is_active(&self) -> bool {
        !self.include.trim().is_empty() || !self.exclude.trim().is_empty()
    }

    pub fn to_filter(&self) -> IncludeFilter {
        let mut filter = IncludeFilter::new()
            .testing_whole_path(self.whole_path)
            .testing_extension(self.extension)
            .matching_case(self.case_sensitive);
        if !self.include.trim().is_empty() {
            filter = filter.including(self.mode.spec(&self.include));
        }
        if !self.exclude.trim().is_empty() {
            filter = filter.excluding(self.mode.spec(&self.exclude));
        }
        filter
    }

    /// Reads an engine filter back into an editable form.
    ///
    /// The inverse of [`FilterForm::to_filter`], and what lets a filter stored
    /// in a preset be *edited* rather than only run. One asymmetry to know
    /// about: the form has a single mode for both boxes, so a filter whose two
    /// halves disagree — only reachable from a hand-written job file — comes
    /// back as `Regex`, the reading that cannot silently mean something else.
    pub fn from_filter(filter: &IncludeFilter) -> Self {
        let text = |spec: &Option<MatchSpec>| {
            spec.as_ref()
                .map(|s| s.text().to_owned())
                .unwrap_or_default()
        };
        let is_regex = |spec: &Option<MatchSpec>| matches!(spec, Some(MatchSpec::Regex(_)));

        Self {
            include: text(&filter.include),
            exclude: text(&filter.exclude),
            mode: if is_regex(&filter.include) || is_regex(&filter.exclude) {
                Mode::Regex
            } else {
                Mode::Auto
            },
            whole_path: filter.whole_path,
            extension: filter.extension,
            case_sensitive: filter.case_sensitive,
        }
    }

    /// One line for the chip's tooltip.
    pub fn summary(&self) -> String {
        match (self.include.trim(), self.exclude.trim()) {
            ("", "") => "No filter".to_owned(),
            (include, "") => format!("Include {include:?}"),
            ("", exclude) => format!("Exclude {exclude:?}"),
            (include, exclude) => format!("Include {include:?}, exclude {exclude:?}"),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        ui.set_min_width(280.0);

        ui.label("Include only files matching:");
        changed |= ui.text_edit_singleline(&mut self.include).changed();

        ui.add_space(4.0);
        ui.label("Exclude files matching:");
        changed |= ui.text_edit_singleline(&mut self.exclude).changed();

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            changed |= ui
                .selectable_value(&mut self.mode, Mode::Auto, "Text / wildcards")
                .changed();
            changed |= ui
                .selectable_value(&mut self.mode, Mode::Regex, "Regular expression")
                .changed();
        });
        ui.label(
            egui::RichText::new("Wildcards: * zero or more, : one or more, ? exactly one")
                .weak()
                .small(),
        );

        ui.add_space(6.0);
        changed |= ui
            .checkbox(&mut self.extension, "Also test the extension")
            .on_hover_text(
                "Tests the extension on its own and the whole name with it, so *.bak \
                 matches notes.bak",
            )
            .changed();
        changed |= ui
            .checkbox(&mut self.whole_path, "Also test the whole path")
            .changed();
        changed |= ui
            .checkbox(&mut self.case_sensitive, "Case sensitive")
            .changed();

        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_form_is_inactive_and_accepts_everything() {
        let form = FilterForm::default();
        assert!(!form.is_active());
        assert!(form.to_filter().is_identity());
        assert_eq!(form.summary(), "No filter");
    }

    #[test]
    fn an_include_box_produces_an_include_filter() {
        let form = FilterForm {
            include: "live".into(),
            ..Default::default()
        };
        assert!(form.is_active());
        let filter = form.to_filter();
        assert!(filter.accepts("/m/live.mp3", "live.mp3").unwrap());
        assert!(!filter.accepts("/m/studio.mp3", "studio.mp3").unwrap());
    }

    #[test]
    fn wildcards_are_picked_up_without_a_mode_switch() {
        let form = FilterForm {
            include: "track ?".into(),
            ..Default::default()
        };
        let filter = form.to_filter();
        assert!(filter.accepts("/m/track 1.mp3", "track 1.mp3").unwrap());
        assert!(!filter.accepts("/m/track 10.mp3", "track 10.mp3").unwrap());
    }

    #[test]
    fn regex_mode_is_explicit() {
        let form = FilterForm {
            include: r"^\d+$".into(),
            mode: Mode::Regex,
            ..Default::default()
        };
        let filter = form.to_filter();
        assert!(filter.accepts("/m/123.mp3", "123.mp3").unwrap());
        assert!(!filter.accepts("/m/abc.mp3", "abc.mp3").unwrap());
    }

    #[test]
    fn whitespace_only_boxes_do_not_activate_the_filter() {
        let form = FilterForm {
            include: "   ".into(),
            ..Default::default()
        };
        assert!(!form.is_active());
        assert!(form.to_filter().is_identity());
    }

    #[test]
    fn the_summary_says_what_is_configured() {
        let form = FilterForm {
            include: "a".into(),
            exclude: "b".into(),
            ..Default::default()
        };
        let summary = form.summary();
        assert!(summary.contains("Include"), "{summary}");
        assert!(summary.contains("exclude"), "{summary}");
    }

    /// A filter stored in a preset has to come back editable, or a per-card
    /// filter could be run but never changed.
    #[test]
    fn a_filter_round_trips_back_into_the_form() {
        for form in [
            FilterForm {
                include: "live".into(),
                ..Default::default()
            },
            FilterForm {
                include: "*.mp3".into(),
                exclude: "demo".into(),
                whole_path: true,
                extension: true,
                case_sensitive: true,
                ..Default::default()
            },
            FilterForm {
                include: r"^\d+".into(),
                mode: Mode::Regex,
                ..Default::default()
            },
        ] {
            let back = FilterForm::from_filter(&form.to_filter());
            assert_eq!(back, form, "{form:?}");
            // And the engine filter it produces is the same one.
            assert_eq!(back.to_filter(), form.to_filter());
        }
    }

    /// Auto mode re-derives substring-vs-wildcard from the text itself, so the
    /// distinction survives even though the form does not store it.
    #[test]
    fn auto_mode_recovers_whether_the_text_held_wildcards() {
        let plain = FilterForm {
            include: "holiday".into(),
            ..Default::default()
        };
        assert!(matches!(
            plain.to_filter().include,
            Some(MatchSpec::Substring(_))
        ));
        assert!(matches!(
            FilterForm::from_filter(&plain.to_filter())
                .to_filter()
                .include,
            Some(MatchSpec::Substring(_))
        ));

        let wild = FilterForm {
            include: "*.mp3".into(),
            ..Default::default()
        };
        assert!(matches!(
            FilterForm::from_filter(&wild.to_filter())
                .to_filter()
                .include,
            Some(MatchSpec::Wildcard(_))
        ));
    }
}
