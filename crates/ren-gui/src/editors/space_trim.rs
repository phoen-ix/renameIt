//! Space Trimming.

use ren_core::ops::SpaceTrim;

use crate::widgets::form::{After, Form};

pub fn ui(ui: &mut egui::Ui, op: &mut SpaceTrim) -> bool {
    let mut changed = false;

    changed |= ui
        .checkbox(&mut op.leading, "Remove leading spaces")
        .changed();
    changed |= ui
        .checkbox(&mut op.trailing, "Remove trailing spaces")
        .changed();
    changed |= ui
        .checkbox(&mut op.shrink, "Shrink multiple spaces into one")
        .changed();

    ui.add_space(6.0);
    ui.label(egui::RichText::new("Maintain space").strong());
    Form::new("space_trim").show(ui, |form| {
        let before = form.row("Before:", After::Nothing, |row| {
            let width = row.field_width();
            row.ui()
                .add(
                    egui::TextEdit::singleline(&mut op.maintain_before)
                        .desired_width(width)
                        .id_salt("maintain_before"),
                )
                .changed()
        });
        changed |= before.inner;
        before
            .response
            .on_hover_text("Insert a space before each of these characters, if needed");

        let after = form.row("After:", After::Nothing, |row| {
            let width = row.field_width();
            row.ui()
                .add(
                    egui::TextEdit::singleline(&mut op.maintain_after)
                        .desired_width(width)
                        .id_salt("maintain_after"),
                )
                .changed()
        });
        changed |= after.inner;
        after
            .response
            .on_hover_text("Insert a space after each of these characters, if needed");
    });

    ui.add_space(6.0);
    changed |= ui
        .checkbox(
            &mut op.underscores_to_spaces,
            "Replace underscores with spaces",
        )
        .changed();

    changed
}
