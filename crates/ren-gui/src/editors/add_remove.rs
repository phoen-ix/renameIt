//! Add & Remove.

use ren_core::ops::{AddRemove, AddRemoveMode};

use crate::widgets::form::{After, Form};

pub fn ui(ui: &mut egui::Ui, op: &mut AddRemove, cx: &super::EditorCx<'_>) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        for (mode, label) in [
            (AddRemoveMode::Add, "Add"),
            (AddRemoveMode::Remove, "Remove"),
            (AddRemoveMode::Both, "Both"),
        ] {
            changed |= ui.radio_value(&mut op.mode, mode, label).changed();
        }
    });
    if op.mode == AddRemoveMode::Both {
        ui.label(
            egui::RichText::new(
                "Removes first, then adds — the add position is measured on the \
                                 shortened name.",
            )
            .weak()
            .small(),
        );
    }

    let adds = matches!(op.mode, AddRemoveMode::Add | AddRemoveMode::Both);
    let removes = matches!(op.mode, AddRemoveMode::Remove | AddRemoveMode::Both);

    if removes {
        ui.add_space(4.0);
        ui.group(|ui| {
            ui.label(egui::RichText::new("Remove").strong());
            Form::new("remove").show(ui, |form| {
                let delete = form.row("Delete:", After::Note("character(s)"), |row| {
                    let changed = super::number_box(row.ui(), &mut op.delete).changed();
                    row.ui().label("character(s)");
                    changed
                });
                changed |= delete.inner;
                delete
                    .response
                    .on_hover_text("999 removes everything from the position onwards");

                // A number box is fixed-width, so nothing in a position row
                // reads `field_width`: `After` here names what trails the box
                // rather than sizing anything, and the ⌖ follows the checkbox.
                changed |= form
                    .row("from pos:", After::Check("counting backwards"), |row| {
                        let mut changed = super::number_box(row.ui(), &mut op.remove_pos).changed();
                        changed |= row
                            .ui()
                            .checkbox(&mut op.remove_backwards, "counting backwards")
                            .changed();
                        super::assist_button(
                            row.ui(),
                            cx,
                            super::assist::AssistTarget::RemoveSection,
                        );
                        changed
                    })
                    .inner;
            });
        });
    }

    if adds {
        ui.add_space(4.0);
        ui.group(|ui| {
            ui.label(egui::RichText::new("Add").strong());
            Form::new("add").show(ui, |form| {
                let history = cx.history("add_insert");
                changed |= form
                    .row("Insert:", super::after_tag_field(history), |row| {
                        let width = row.field_width();
                        row.column(|ui| {
                            crate::widgets::tag_field::tag_field_with_history(
                                ui,
                                "add_insert",
                                &mut op.insert,
                                width,
                                history,
                            )
                        })
                    })
                    .inner;

                changed |= form
                    .row("at pos:", After::Check("counting backwards"), |row| {
                        let mut changed = super::number_box(row.ui(), &mut op.add_pos).changed();
                        changed |= row
                            .ui()
                            .checkbox(&mut op.add_backwards, "counting backwards")
                            .on_hover_text("Position 0 backwards is the very end of the name")
                            .changed();
                        super::assist_button(row.ui(), cx, super::assist::AssistTarget::AddPos);
                        changed
                    })
                    .inner;
            });
            changed |= ui
                .checkbox(&mut op.overwrite, "Overwrite instead of insert")
                .changed();
        });
    }

    ui.label(
        egui::RichText::new("Positions are zero-based: 0 is before the first character.")
            .weak()
            .small(),
    );

    changed
}
