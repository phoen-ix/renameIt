//! The pre-processor, as a section of the card's Scope expander.
//!
//! A pre-processor narrows a name to one section before the card's operation
//! sees it, so an operation can rename part of a name and leave the rest
//! exactly as it was.
//!
//! Laid out in the order the stages run: an *Enable Pre-Processor* checkbox,
//! then two named groups of two stages each, then an advanced filter, then the
//! two switches that qualify every text search. The engine has been complete
//! and tested since M4; until then the only way to build one was hand-writing
//! a job file, and `panels::operation` drew a read-only summary that said so.
//!
//! A widget module rather than another function in `panels::operation`, for the
//! same reason [`crate::widgets::filter_editor`] is one: it owns the sibling
//! control on the same expander, and the two are the only things in the app
//! that turn a `MatchSpec` back into something editable.

use ren_core::MatchSpec;
use ren_core::preproc::PreProcessor;

/// Draws the whole section. Returns true if anything changed.
///
/// Takes the card's `Option<PreProcessor>` rather than a `PreProcessor`, because
/// the *Enable* checkbox is the affordance that creates one — that is the thing
/// the feature was missing, not the fields.
pub fn ui(ui: &mut egui::Ui, preproc: &mut Option<PreProcessor>) -> bool {
    let mut changed = false;

    // The whole pre-processor stashes and comes back, like every stage below:
    // unticking is how you compare *with* and *without*, and losing the
    // configuration on the way makes that a retype rather than a click.
    let mut on = preproc.is_some();
    if ui
        .checkbox(&mut on, "Enable Pre-Processor")
        .on_hover_text(
            "Narrows what this operation sees, so it renames part of a name instead of all of it",
        )
        .changed()
    {
        let id = ui.id().with("preproc_stash");
        if on {
            *preproc = Some(
                ui.data_mut(|d| d.get_temp::<PreProcessor>(id))
                    .unwrap_or_default(),
            );
        } else if let Some(old) = preproc.take() {
            ui.data_mut(|d| d.insert_temp(id, old));
        }
        changed = true;
    }

    let Some(preproc) = preproc else {
        return changed;
    };

    // How the searches read *before* this frame's edits, so a search armed
    // below is armed in the reading the switch shows.
    let reading = regex_state(preproc);

    ui.indent("preproc_body", |ui| {
        ui.label(egui::RichText::new("Offset beginning").strong().small());
        changed |= count(
            ui,
            "preproc_skip_first",
            "Skip the first",
            "characters",
            &mut preproc.skip_first,
        );
        changed |= search(
            ui,
            "preproc_skip_until",
            "Skip until string is found:",
            &mut preproc.skip_until,
            reading,
        );

        ui.add_space(4.0);
        ui.label(egui::RichText::new("Limit length").strong().small());
        changed |= count(
            ui,
            "preproc_limit_to",
            "Include up to",
            "characters",
            &mut preproc.limit_to,
        );
        changed |= search(
            ui,
            "preproc_cut_at",
            "Cut if string is found:",
            &mut preproc.cut_at,
            reading,
        );

        ui.add_space(4.0);
        ui.label(egui::RichText::new("Advanced filter").strong().small());
        changed |= search(
            ui,
            "preproc_section",
            "Keep only the section matching:",
            &mut preproc.section,
            reading,
        );

        ui.add_space(6.0);
        // **Both switches sit here, below all three searches, rather than
        // inside *Advanced filter*.** They cover all three text searches, and
        // a checkbox drawn inside one group would be a control lying about its
        // own scope.
        let armed =
            preproc.skip_until.is_some() || preproc.cut_at.is_some() || preproc.section.is_some();
        ui.add_enabled_ui(armed, |ui| {
            changed |= ui
                .checkbox(&mut preproc.case_sensitive, "Case sensitive")
                .on_hover_text("Applies to all three text searches above")
                .on_disabled_hover_text("Nothing here searches for text yet")
                .changed();

            // An on/off switch that can *show* a third state. A job file can
            // hold a mix — regex in one stage and wildcards in another — and
            // one checkbox cannot say that, so a mix is drawn grey, which
            // reaches the accessibility tree as `Toggled::Mixed` and is
            // announced rather than silently normalised. It is not a
            // three-state *control*: there is no "keep" to choose here, and
            // cycling through one meant a ticked box never unticked. A click
            // on grey makes every search a regex; on ticked, none.
            let now = regex_state(preproc);
            let mut shown = now.unwrap_or(false);
            if ui
                .add(
                    egui::Checkbox::new(&mut shown, "Regular expression")
                        .indeterminate(now.is_none()),
                )
                .on_hover_text(match now {
                    None => {
                        "Some searches above are regular expressions and some are not. \
                         Click to make them all regular expressions."
                    }
                    Some(_) => "Read all three text searches above as regular expressions",
                })
                .clicked()
            {
                set_regex(preproc, !now.unwrap_or(false));
                changed = true;
            }
        });

        if changed {
            preproc.invalidate();
        }
    });

    changed
}

/// One line for the collapsed Scope header — the five stages in the order they
/// run, so a card that carries a pre-processor says so without being opened.
pub fn summary(preproc: &PreProcessor) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = preproc.skip_first {
        parts.push(format!("skip the first {n}"));
    }
    if let Some(spec) = &preproc.skip_until {
        parts.push(format!("skip until {:?}", spec.text()));
    }
    if let Some(n) = preproc.limit_to {
        parts.push(format!("take {n}"));
    }
    if let Some(spec) = &preproc.cut_at {
        parts.push(format!("cut at {:?}", spec.text()));
    }
    if let Some(spec) = &preproc.section {
        parts.push(format!("section matching {:?}", spec.text()));
    }
    if parts.is_empty() {
        return "on, doing nothing".to_owned();
    }
    // Said last because it qualifies every search above it, and omitted when it
    // is off — the summary should read like the setting, not like a dump.
    if preproc.case_sensitive {
        parts.push("case sensitive".to_owned());
    }
    parts.join(", ")
}

/// A checkbox that arms a character count.
fn count(
    ui: &mut egui::Ui,
    key: &str,
    label: &str,
    suffix: &str,
    field: &mut Option<usize>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= arm(ui, key, label, field, usize::default);
        if let Some(n) = field {
            changed |= crate::widgets::number::add(
                ui,
                egui::DragValue::new(n).range(0..=9_999).speed(0.2),
            )
            .changed();
            ui.label(suffix);
        }
    });
    changed
}

/// A checkbox that arms a text search.
///
/// `reading` is how the other searches read: a search armed while the switch
/// is ticked is a regular expression too, text kept from an earlier untick
/// included, rather than the one literal among regexes that only a hand-written
/// job file used to produce. A mix (`None`) leaves the armed search as it was.
fn search(
    ui: &mut egui::Ui,
    key: &str,
    label: &str,
    field: &mut Option<MatchSpec>,
    reading: Option<bool>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let armed = arm(
            ui,
            key,
            label,
            field,
            || MatchSpec::Substring(String::new()),
        );
        if armed && let (Some(spec), Some(regex)) = (field.as_mut(), reading) {
            *spec = read_as(spec.text().to_owned(), regex);
        }
        changed |= armed;
        if let Some(spec) = field {
            let mut text = spec.text().to_owned();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(160.0)
                        .id_salt(key),
                )
                .changed()
            {
                *spec = retype(spec, text);
                changed = true;
            }
        }
    });
    changed
}

/// The checkbox half of both, including the stash.
///
/// **Unticking keeps what was typed.** egui's own temp store is where transient
/// UI state goes — the same bargain `rule_table`'s bulk box strikes — and it is
/// keyed under the card's id, because `panels::operation` wraps every card body
/// in a `push_id`. Without it, unticking a stage to see the difference and
/// ticking it back is a retype rather than a click.
fn arm<T>(
    ui: &mut egui::Ui,
    key: &str,
    label: &str,
    field: &mut Option<T>,
    fresh: impl FnOnce() -> T,
) -> bool
where
    T: Clone + Send + Sync + 'static,
{
    let mut on = field.is_some();
    if !ui.checkbox(&mut on, label).changed() {
        return false;
    }
    let id = ui.id().with(key);
    if on {
        // `MatchSpec` has no `Default` — and should not, since "an empty
        // substring" is a choice rather than an absence. The caller says what a
        // fresh one is.
        *field = Some(ui.data_mut(|d| d.get_temp::<T>(id)).unwrap_or_else(fresh));
    } else if let Some(old) = field.take() {
        ui.data_mut(|d| d.insert_temp(id, old));
    }
    true
}

/// Whether the searches read as regular expressions — `None` for a mix.
fn regex_state(preproc: &PreProcessor) -> Option<bool> {
    let mut seen = [false, false];
    for spec in [&preproc.skip_until, &preproc.cut_at, &preproc.section]
        .into_iter()
        .flatten()
    {
        seen[usize::from(matches!(spec, MatchSpec::Regex(_)))] = true;
    }
    match seen {
        [_, true] if !seen[0] => Some(true),
        [true, false] => Some(false),
        // Nothing armed reads as "no", so a fresh pre-processor shows a cleared
        // box rather than a grey one it never earned.
        [false, false] => Some(false),
        _ => None,
    }
}

/// Puts every armed search into one reading.
fn set_regex(preproc: &mut PreProcessor, yes: bool) {
    for spec in [
        &mut preproc.skip_until,
        &mut preproc.cut_at,
        &mut preproc.section,
    ]
    .into_iter()
    .flatten()
    {
        *spec = read_as(spec.text().to_owned(), yes);
    }
}

/// One search's text, read as a regex or not.
fn read_as(text: String, regex: bool) -> MatchSpec {
    if regex {
        MatchSpec::Regex(text)
    } else {
        MatchSpec::auto(text)
    }
}

/// Retypes a search box without changing how it is read.
///
/// A `*` typed into a regex box is a quantifier, not a request to switch to the
/// wildcard language — so only the non-regex case goes back through
/// `MatchSpec::auto`, which is the one thing `auto` is documented to decide.
fn retype(spec: &MatchSpec, text: String) -> MatchSpec {
    match spec {
        MatchSpec::Regex(_) => MatchSpec::Regex(text),
        _ => MatchSpec::auto(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed(skip: Option<&str>, cut: Option<&str>, section: Option<MatchSpec>) -> PreProcessor {
        let mut pp = PreProcessor::new();
        pp.skip_until = skip.map(|s| MatchSpec::auto(s.to_owned()));
        pp.cut_at = cut.map(|s| MatchSpec::auto(s.to_owned()));
        pp.section = section;
        pp
    }

    /// *Regular expression* is one switch over all three searches, as
    /// `case_sensitive` already is. A mix is only reachable from a
    /// hand-written job file — and it must show grey rather than being
    /// normalised to whichever answer we happened to check first.
    #[test]
    fn a_pre_processor_that_is_regex_in_one_search_and_not_another_reads_as_a_mix() {
        assert_eq!(
            regex_state(&armed(Some("a"), None, Some(MatchSpec::Regex("b".into())))),
            None
        );
        assert_eq!(
            regex_state(&armed(None, None, Some(MatchSpec::Regex("b".into())))),
            Some(true)
        );
        assert_eq!(regex_state(&armed(Some("a"), Some("b"), None)), Some(false));
    }

    /// A pre-processor with nothing armed has no reading to be uncertain about,
    /// so the box is cleared rather than grey — grey would claim a mix that
    /// does not exist.
    #[test]
    fn nothing_armed_is_not_a_mix() {
        assert_eq!(regex_state(&PreProcessor::new()), Some(false));
    }

    /// Flipping the switch rewrites every armed search and leaves the text
    /// alone, so it is a reading change rather than an edit.
    #[test]
    fn the_switch_changes_how_every_search_is_read_and_not_what_it_says() {
        let mut pp = armed(Some("a*b"), Some("x"), None);
        set_regex(&mut pp, true);
        assert_eq!(pp.skip_until, Some(MatchSpec::Regex("a*b".into())));
        assert_eq!(pp.cut_at, Some(MatchSpec::Regex("x".into())));
        assert_eq!(pp.section, None, "an unarmed stage is left unarmed");

        set_regex(&mut pp, false);
        assert_eq!(
            pp.skip_until,
            Some(MatchSpec::Wildcard("a*b".into())),
            "`auto` decides substring-vs-wildcard for the non-regex case"
        );
        assert_eq!(pp.cut_at, Some(MatchSpec::Substring("x".into())));
    }

    /// Typing into a regex box must not switch it to the wildcard language
    /// behind the user's back: `*` is a quantifier there, and `MatchSpec::auto`
    /// would read it as "match anything".
    #[test]
    fn typing_a_star_into_a_regex_box_keeps_it_a_regex() {
        assert_eq!(
            retype(&MatchSpec::Regex("a".into()), "a*".to_owned()),
            MatchSpec::Regex("a*".into())
        );
        assert_eq!(
            retype(&MatchSpec::Substring("a".into()), "a*".to_owned()),
            MatchSpec::Wildcard("a*".into()),
            "and a plain box still infers"
        );
    }

    /// The header has to say a pre-processor is on even when it does nothing,
    /// or *Enable* looks like it did not take.
    #[test]
    fn an_enabled_pre_processor_with_no_stages_still_announces_itself() {
        assert_eq!(summary(&PreProcessor::new()), "on, doing nothing");
    }

    /// The five stages in the order they run, with the qualifier last.
    #[test]
    fn the_summary_reads_the_stages_in_the_order_they_run() {
        let mut pp = PreProcessor::new().skipping_first(14).limited_to(3);
        pp.cut_at = Some(MatchSpec::Substring(" - ".into()));
        pp.case_sensitive = true;
        assert_eq!(
            summary(&pp),
            "skip the first 14, take 3, cut at \" - \", case sensitive"
        );
    }
}
