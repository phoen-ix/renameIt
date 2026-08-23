//! The Batch Replace list: find → replace pairs, run top to bottom.
//!
//! *"From the settings window you can manage the list of
//! replace commands, by adding, deleting or editing the commands. You can also
//! move them up of down in the list, which can be useful if you want certain
//! commands to be executed before others. The batch replace is executed from
//! the first item and them stepping down in the list."*
//!
//! So the verb set is add / delete / edit / move up / move down, plus a way
//! back to the fifty-one rules we ship (D27).

use ren_core::ops::{BatchReplace, Replace};

/// Draws the list. Returns true if anything changed.
pub fn ui(ui: &mut egui::Ui, rules: &mut Vec<Replace>) -> bool {
    let mut changed = false;
    let mut command: Option<Command> = None;

    // The box beside *Add rule* is scratch: it belongs to this frame's widget,
    // not to the rule list, and threading it through both owners of a rule
    // list would put a field on two structs so one text box could remember
    // what was typed into it. egui's own temp store is where transient UI
    // state goes.
    let bulk_id = ui.id().with("rule_bulk_add");
    let mut bulk: String = ui.data_mut(|d| d.get_temp(bulk_id).unwrap_or_default());
    let bulk = &mut bulk;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(match rules.len() {
                0 => "No rules — this operation would do nothing.".to_owned(),
                1 => "1 rule, run top to bottom.".to_owned(),
                n => format!("{n} rules, run top to bottom."),
            })
            .weak()
            .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Add rule").clicked() {
                command = Some(Command::Add);
            }
            // *"Tip: If want to add multiple items in one go, use the add
            // button here and separate each item with a colon (:)."*
            //
            // One control rather than two: empty adds a blank row, which is
            // what the button always did, and text splits on `:`. A second
            // button for the second behaviour would be two ways to do one
            // thing.
            ui.add(
                egui::TextEdit::singleline(bulk)
                    .desired_width(160.0)
                    .hint_text("find text, or a:b:c")
                    .id_salt("rule_bulk_add"),
            )
            .on_hover_text(
                "Type what to find and press Add rule. Separate several with a colon to add \
                 them all at once.",
            );
            let shipped = *rules == *ren_core::ops::replace::shipped_rules();
            if ui
                .add_enabled(!shipped, egui::Button::new("Restore defaults"))
                .on_hover_text("Back to the list RenameIt ships with")
                .on_disabled_hover_text("Already the list RenameIt ships with")
                .clicked()
            {
                command = Some(Command::Restore);
            }
        });
    });

    ui.add_space(crate::theme::space::TIGHT);

    // A header row. Fifty-one rows of `[find] [->] [replace] [x regex]
    // [... extras] [^] [v] [x]` and nothing at all saying which was which:
    // three of the eight controls were unlabelled squares, and even once they
    // render, an unnamed control repeated fifty-one times is a control the
    // user has to click to identify.
    //
    // Column widths are the row's own literals — the row is a
    // `ui.horizontal` of fixed-width widgets rather than a grid, so the header
    // has to agree with it by hand. Kept next to each other for that reason.
    ui.horizontal(|ui| {
        let heading = |ui: &mut egui::Ui, width: f32, text: &str| {
            ui.allocate_ui_with_layout(
                egui::vec2(width, ui.spacing().interact_size.y),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(egui::RichText::new(text).weak().small());
                },
            );
        };
        heading(ui, 140.0 + 4.0, "Find");
        heading(ui, 18.0, "");
        heading(ui, 140.0 + 4.0, "Replace with");
        heading(ui, 62.0, "As");
        heading(ui, 40.0, "More");
        heading(ui, 0.0, "Order");
    });
    ui.add_space(2.0);

    egui::ScrollArea::vertical()
        .id_salt("batch_rules")
        .max_height(320.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for index in 0..rules.len() {
                ui.push_id(index, |ui| {
                    ui.horizontal(|ui| {
                        let rule = &mut rules[index];
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut rule.find)
                                    .desired_width(140.0)
                                    .hint_text("find")
                                    .id_salt("find"),
                            )
                            .changed();
                        ui.label("→");
                        changed |= crate::widgets::tag_field::tag_text_edit(
                            ui,
                            "replace",
                            &mut rule.replace,
                            140.0,
                            "replace with",
                        );
                        changed |= ui
                            .checkbox(&mut rule.regex, "regex")
                            .on_hover_text("Read the find text as a regular expression")
                            .changed();

                        // > *"You can also add the current Replace function
                        // > settings to the list."*
                        //
                        // The **settings**, plural: a batch entry is a whole
                        // `Replace`, and a rule carries seven settings for the
                        // same reason. Four of them live here
                        // rather than on the row, because at fifty-one rules
                        // four more inline controls do not fit — but the label
                        // carries a summary whenever any of them is off its
                        // default, so a rule that skips three occurrences
                        // cannot look identical to one that skips none.
                        //
                        // `⋯` (U+22EF), **not** the `⋮` of the card overflow
                        // menu and the preset row menu. Different code point,
                        // different meaning: this opens fields, those open
                        // commands.
                        let extras = extras(rule);
                        let chip = ui
                            .selectable_label(
                                extras.is_some(),
                                match &extras {
                                    Some(summary) => format!("⋯ {summary}"),
                                    None => "⋯".to_owned(),
                                },
                            )
                            .on_hover_text("Case, swap, skip and max for this rule");
                        egui::Popup::from_toggle_button_response(&chip).show(|ui| {
                            ui.set_min_width(220.0);
                            changed |= ui
                                .checkbox(&mut rule.case_sensitive, "Case sensitive")
                                .on_hover_text(
                                    "Otherwise \"Photo\" and \"photo\" are the same word",
                                )
                                .changed();
                            changed |= ui
                                .checkbox(&mut rule.swap, "Swap mode")
                                .on_hover_text("Find becomes replace, and replace becomes find")
                                .changed();
                            // The same note the single-card editor draws, for
                            // the same reason: swap is ignored with wildcards,
                            // a regex, or tags in the replace box, and a ticked
                            // box that does nothing is worse than no box.
                            if rule.swap && !rule.swap_applies() {
                                ui.label(
                                    egui::RichText::new("swap ignored here")
                                        .weak()
                                        .italics()
                                        .small(),
                                );
                            }
                            ui.horizontal(|ui| {
                                changed |= crate::editors::number(ui, "Skip:", &mut rule.skip);
                            });
                            ui.horizontal(|ui| {
                                changed |= crate::editors::number(ui, "Max:", &mut rule.max);
                            });
                        });

                        if ui
                            .add_enabled(
                                index > 0,
                                crate::widgets::icons::IconButton::new(
                                    crate::widgets::icons::Icon::CaretUp,
                                    "Move up",
                                ),
                            )
                            .on_hover_text("Move up")
                            .clicked()
                        {
                            command = Some(Command::Up(index));
                        }
                        if ui
                            .add_enabled(
                                index + 1 < rules.len(),
                                crate::widgets::icons::IconButton::new(
                                    crate::widgets::icons::Icon::CaretDown,
                                    "Move down",
                                ),
                            )
                            .on_hover_text("Move down")
                            .clicked()
                        {
                            command = Some(Command::Down(index));
                        }
                        if ui.button("✖").on_hover_text("Delete this rule").clicked() {
                            command = Some(Command::Delete(index));
                        }
                    });
                });
            }
        });

    ui.data_mut(|d| d.insert_temp(bulk_id, bulk.clone()));

    match command {
        Some(Command::Add) => {
            let added = added_by(bulk);
            let bulk_used = !added.is_empty();
            rules.extend(added);
            if bulk_used {
                ui.data_mut(|d| d.insert_temp(bulk_id, String::new()));
            } else {
                rules.push(Replace::default());
            }
            changed = true;
        }
        Some(Command::Delete(index)) => {
            rules.remove(index);
            changed = true;
        }
        Some(Command::Up(index)) => {
            rules.swap(index - 1, index);
            changed = true;
        }
        Some(Command::Down(index)) => {
            rules.swap(index, index + 1);
            changed = true;
        }
        Some(Command::Restore) => {
            *rules = BatchReplace::default().rules;
            changed = true;
        }
        None => {}
    }

    changed
}

enum Command {
    Add,
    Delete(usize),
    Up(usize),
    Down(usize),
    Restore,
}

/// The four settings that are not on the row, in the dialog's own order —
/// `None` when every one of them is at its default.
///
/// **This is what stops a rule with `skip = 3` looking identical to one with
/// `skip = 0`.** It goes in the toggle's *label*, not its tooltip: egui only
/// puts a tooltip into the accessibility tree while it is hovered, so a tooltip
/// marker is invisible to a screen reader and to the headless harness alike.
///
/// A pure function, like `added_by` below it, so the wording is testable
/// without a window.
fn extras(rule: &Replace) -> Option<String> {
    let mut parts = Vec::new();
    if rule.case_sensitive {
        parts.push("case sensitive".to_owned());
    }
    if rule.swap {
        parts.push("swap".to_owned());
    }
    if rule.skip > 0 {
        parts.push(format!("skip {}", rule.skip));
    }
    if rule.max > 0 {
        parts.push(format!("max {}", rule.max));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// What *Add rule* adds, given whatever is in the box beside it.
///
/// > *"Tip: If want to add multiple items in one go, use the add button here
/// > and separate each item with a colon (:)."*
///
/// Empty gives none, and the caller then adds one blank row — which is what
/// the button always did. A separate function because the widget above it is
/// not something a headless test can type into (see `docs/manual-checks.md`),
/// and the splitting is the part with anything to get wrong.
fn added_by(bulk: &str) -> Vec<Replace> {
    bulk.split(':')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|find| Replace::new(find, ""))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The overwhelming majority of the fifty-one shipped rules, and the row
    /// must not shout about them.
    #[test]
    fn a_rule_at_its_defaults_has_nothing_extra_to_say() {
        assert_eq!(extras(&Replace::new("_", " ")), None);
    }

    /// The point of the whole affordance: a rule whose hidden settings are not
    /// at their defaults says so on its face, because nothing else would.
    #[test]
    fn a_rule_that_skips_the_first_three_says_so_because_nothing_else_would() {
        assert_eq!(
            extras(&Replace::new("a", "b").skip(3)).as_deref(),
            Some("skip 3")
        );
        assert_eq!(
            extras(&Replace::new("a", "b").max(2)).as_deref(),
            Some("max 2")
        );
        assert_eq!(
            extras(&Replace::new("a", "b").case_sensitive(true)).as_deref(),
            Some("case sensitive")
        );
        assert_eq!(
            extras(&Replace::new("a", "b").swap(true)).as_deref(),
            Some("swap")
        );
    }

    /// The dialog's own order — case, swap, skip, max — so the summary reads
    /// the way the popover below it is laid out.
    #[test]
    fn the_four_hidden_settings_are_named_in_the_dialogs_own_order() {
        let rule = Replace::new("a", "b")
            .case_sensitive(true)
            .swap(true)
            .skip(3)
            .max(2);
        assert_eq!(
            extras(&rule).as_deref(),
            Some("case sensitive, swap, skip 3, max 2")
        );
    }

    /// Zero means "unlimited" for Max and "from the beginning" for Skip — both
    /// are the default, and neither is worth a word on the row.
    #[test]
    fn a_zero_is_the_default_and_not_a_setting_to_announce() {
        assert_eq!(extras(&Replace::new("a", "b").skip(0).max(0)), None);
    }

    #[test]
    fn an_empty_box_adds_nothing_and_leaves_the_blank_row_to_the_caller() {
        assert!(added_by("").is_empty());
        assert!(added_by("   ").is_empty());
        assert!(added_by(":::").is_empty());
    }

    #[test]
    fn one_item_is_one_rule() {
        let added = added_by("_");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].find, "_");
        assert_eq!(
            added[0].replace.as_str(),
            "",
            "the replace box is filled in after"
        );
    }

    /// The tip's own example shape, plus the spacing a person actually types.
    #[test]
    fn a_colon_separated_list_is_one_rule_each() {
        let finds: Vec<_> = added_by("aa: bb :cc").into_iter().map(|r| r.find).collect();
        assert_eq!(finds, ["aa", "bb", "cc"]);
    }

    /// An empty item between two colons is a typo, not a blank rule: a rule
    /// with nothing to find matches nothing and would sit in the list forever.
    #[test]
    fn empty_items_are_dropped_rather_than_added_as_blank_rules() {
        let finds: Vec<_> = added_by("aa::bb:").into_iter().map(|r| r.find).collect();
        assert_eq!(finds, ["aa", "bb"]);
    }
}
