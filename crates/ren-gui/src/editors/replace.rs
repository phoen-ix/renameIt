//! Find & Replace, and Batch Replace.

use ren_core::ops::{BatchReplace, Replace};

use crate::widgets::form::{After, Form};

pub fn ui(ui: &mut egui::Ui, op: &mut Replace, cx: &super::EditorCx<'_>) -> bool {
    let mut changed = false;

    Form::new("replace").show(ui, |form| {
        let find_history = cx.history("replace_find");
        let marker = super::assist_label(super::assist::AssistTarget::ReplaceFind);
        // The chevron is only drawn once there is a history to open, so it is
        // only reserved for then.
        let after = if find_history.is_empty() {
            After::Marker(marker)
        } else {
            After::ButtonsAndMarker(1, marker)
        };
        changed |= form
            .row("Find:", after, |row| {
                let width = row.field_width();
                let changed = crate::widgets::tag_field::text_with_history(
                    row.ui(),
                    "replace_find",
                    &mut op.find,
                    width,
                    if op.regex {
                        "regular expression"
                    } else {
                        "text or * : ?"
                    },
                    find_history,
                );
                super::assist_button(row.ui(), cx, super::assist::AssistTarget::ReplaceFind);
                changed
            })
            .inner;

        let with_history = cx.history("replace_with");
        // *"You can use tags in the replace box"* — so it is a tag field, with
        // the picker and the D29 error report every other one has.
        changed |= form
            .row(
                "Replace with:",
                super::after_tag_field(with_history),
                |row| {
                    let width = row.field_width();
                    row.column(|ui| {
                        crate::widgets::tag_field::tag_field_hinted_with_history(
                            ui,
                            "replace_with",
                            &mut op.replace,
                            width,
                            "leave empty to delete",
                            with_history,
                        )
                    })
                },
            )
            .inner;

        let skip = form.row("Skip:", After::Nothing, |row| {
            super::number_box(row.ui(), &mut op.skip).changed()
        });
        changed |= skip.inner;
        skip.response
            .on_hover_text("Skip the first N occurrences in each name");

        let max = form.row("Max:", After::Nothing, |row| {
            super::number_box(row.ui(), &mut op.max).changed()
        });
        changed |= max.inner;
        max.response
            .on_hover_text("Maximum replacements per name; 0 means unlimited");
    });

    ui.add_space(4.0);
    changed |= ui
        .checkbox(&mut op.case_sensitive, "Case sensitive")
        .changed();

    let swap = ui.checkbox(&mut op.swap, "Swap mode");
    changed |= swap.changed();
    swap.on_hover_text(
        "Also replaces the second string with the first. Ignored when the find box holds \
         wildcards or a regular expression, or the replace box holds tags.",
    );

    ui.horizontal(|ui| {
        changed |= ui
            .checkbox(&mut op.regex, "Regular expression")
            .on_hover_text("Capture groups are available in the replace box as $1–$9")
            .changed();
        if op.swap && !op.swap_applies() {
            ui.label(
                egui::RichText::new("swap ignored here")
                    .weak()
                    .italics()
                    .small(),
            );
        }
    });

    ui.add_space(6.0);
    // *"You can also add the current Replace function settings to the list by
    // pressing the button."* The **defaults** list, which is what Settings
    // edits and what a *new* Batch Replace card copies (D35) — adding to a
    // card that already exists would edit somebody's built pipeline from
    // another card's button.
    if ui
        .add_enabled(
            !op.find.is_empty(),
            egui::Button::new("Add to Batch Replace"),
        )
        .on_hover_text(
            "Appends these settings to the Batch Replace list in Settings. A card that \
             already exists keeps its own copy, so nothing you have built changes.",
        )
        .on_disabled_hover_text("Nothing to add: the find box is empty")
        .clicked()
    {
        *cx.requests.add_batch_rule.borrow_mut() = Some(op.clone());
    }

    changed
}

pub fn batch_ui(ui: &mut egui::Ui, op: &mut BatchReplace) -> bool {
    ui.label(
        egui::RichText::new(
            "This card's own list. It started from the one in Settings; changing it here \
             changes only this card, which is what keeps a preset self-contained.",
        )
        .weak()
        .small(),
    );
    ui.add_space(6.0);
    crate::widgets::rule_table::ui(ui, &mut op.rules)
}
