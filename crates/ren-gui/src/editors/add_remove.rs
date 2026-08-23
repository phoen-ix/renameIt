//! Add & Remove.

use ren_core::ops::{AddRemove, AddRemoveMode};

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
            ui.horizontal(|ui| {
                changed |= super::number(ui, "Delete:", &mut op.delete);
                ui.label("character(s)");
            })
            .response
            .on_hover_text("999 removes everything from the position onwards");
            ui.horizontal(|ui| {
                changed |= super::number(ui, "from pos:", &mut op.remove_pos);
                changed |= ui
                    .checkbox(&mut op.remove_backwards, "counting backwards")
                    .changed();
                super::assist_button(ui, cx, super::assist::AssistTarget::RemoveSection);
            });
        });
    }

    if adds {
        ui.add_space(4.0);
        ui.group(|ui| {
            ui.label(egui::RichText::new("Add").strong());
            ui.label("Insert:");
            changed |= crate::widgets::tag_field::tag_field_with_history(
                ui,
                "add_insert",
                &mut op.insert,
                200.0,
                cx.history("add_insert"),
            );
            ui.horizontal(|ui| {
                changed |= super::number(ui, "at pos:", &mut op.add_pos);
                changed |= ui
                    .checkbox(&mut op.add_backwards, "counting backwards")
                    .on_hover_text("Position 0 backwards is the very end of the name")
                    .changed();
                super::assist_button(ui, cx, super::assist::AssistTarget::AddPos);
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
