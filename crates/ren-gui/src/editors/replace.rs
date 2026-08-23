//! Find & Replace, and Batch Replace.

use ren_core::ops::{BatchReplace, Replace};

pub fn ui(ui: &mut egui::Ui, op: &mut Replace, cx: &super::EditorCx<'_>) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label("Find:");
        changed |= crate::widgets::tag_field::text_with_history(
            ui,
            "replace_find",
            &mut op.find,
            220.0,
            if op.regex {
                "regular expression"
            } else {
                "text or * : ?"
            },
            cx.history("replace_find"),
        );
        super::assist_button(ui, cx, super::assist::AssistTarget::ReplaceFind);
    });

    ui.horizontal(|ui| {
        ui.label("Replace with:");
        // *"You can use tags in the replace box"* — so it is a tag field, with
        // the picker and the D29 error report every other one has.
        changed |= crate::widgets::tag_field::tag_field_hinted_with_history(
            ui,
            "replace_with",
            &mut op.replace,
            220.0,
            "leave empty to delete",
            cx.history("replace_with"),
        );
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        changed |= ui
            .checkbox(&mut op.case_sensitive, "Case sensitive")
            .changed();
        changed |= super::number(ui, "Skip:", &mut op.skip);
    })
    .response
    .on_hover_text("Skip the first N occurrences in each name");

    ui.horizontal(|ui| {
        let swap = ui.checkbox(&mut op.swap, "Swap mode");
        changed |= swap.changed();
        swap.on_hover_text(
            "Also replaces the second string with the first. Ignored when the find box holds \
             wildcards or a regular expression, or the replace box holds tags.",
        );
        changed |= super::number(ui, "Max:", &mut op.max);
    })
    .response
    .on_hover_text("Maximum replacements per name; 0 means unlimited");

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
