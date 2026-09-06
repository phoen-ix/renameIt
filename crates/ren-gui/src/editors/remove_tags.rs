//! Remove Tags: "Remove these tags, if present:" over three independent
//! checkboxes.

use ren_core::ops::RemoveTags;

use crate::widgets::form::{After, Form, Row};

pub fn ui(ui: &mut egui::Ui, op: &mut RemoveTags) -> bool {
    let mut changed = false;

    // One label for the three boxes: the first shares its row, the other two
    // continue it.
    Form::new("remove_tags").show(ui, |form| {
        for (index, (kind, on)) in op.boxes().into_iter().enumerate() {
            let content = |row: &mut Row<'_>| row.ui().checkbox(on, kind.label()).changed();
            changed |= if index == 0 {
                form.row("Remove these tags, if present:", After::Nothing, content)
            } else {
                form.unlabelled(After::Nothing, content)
            }
            .inner;
        }
    });

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
