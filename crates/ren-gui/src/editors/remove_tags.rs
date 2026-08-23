//! Remove Tags: "Remove these tags, if present:" over three independent
//! checkboxes.

use ren_core::ops::RemoveTags;

pub fn ui(ui: &mut egui::Ui, op: &mut RemoveTags) -> bool {
    let mut changed = false;

    ui.label("Remove these tags, if present:");
    ui.add_space(2.0);
    for (kind, on) in op.boxes() {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            changed |= ui.checkbox(on, kind.label()).changed();
        });
    }

    ui.add_space(6.0);
    if op.id3v1 || op.id3v2 || op.lyrics {
        ui.label(
            egui::RichText::new("⚠ Removing tags cannot be undone.")
                .color(ui.visuals().warn_fg_color)
                .small(),
        );
    } else {
        ui.label(
            egui::RichText::new("Nothing is selected, so nothing will be removed.")
                .weak()
                .small(),
        );
    }
    ui.label(
        egui::RichText::new(
            "The audio itself is untouched — only the tag blocks go. A file that does not \
             have the tag you ticked, or a format that cannot carry it, is left alone.",
        )
        .weak()
        .small(),
    );

    changed
}
