//! Space Trimming.

use ren_core::ops::SpaceTrim;

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
    ui.horizontal(|ui| {
        ui.label("Before:");
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut op.maintain_before)
                    .desired_width(140.0)
                    .id_salt("maintain_before"),
            )
            .on_hover_text("Insert a space before each of these characters, if needed")
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("After:");
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut op.maintain_after)
                    .desired_width(140.0)
                    .id_salt("maintain_after"),
            )
            .on_hover_text("Insert a space after each of these characters, if needed")
            .changed();
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
