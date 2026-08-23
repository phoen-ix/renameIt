//! Re-Number.

use ren_core::ops::{NumberAction, NumberTarget, ReNumber};

use crate::widgets::tag_field::tag_field;

pub fn ui(ui: &mut egui::Ui, op: &mut ReNumber) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label("With");
        egui::ComboBox::from_id_salt("renumber_target")
            .selected_text(op.target.label())
            .show_ui(ui, |ui| {
                for target in NumberTarget::all() {
                    changed |= ui
                        .selectable_value(&mut op.target, target, target.label())
                        .changed();
                }
            });
        ui.label("in the filename");
    });

    ui.add_space(4.0);
    ui.group(|ui| {
        ui.label(egui::RichText::new("Only numbers").strong());
        changed |= threshold(ui, "larger or equal to", "renumber_low", &mut op.at_least);
        changed |= threshold(ui, "less or equal to", "renumber_high", &mut op.at_most);
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("renumber_action")
            .selected_text(op.action.label())
            .show_ui(ui, |ui| {
                for action in NumberAction::all() {
                    changed |= ui
                        .selectable_value(&mut op.action, action, action.label())
                        .changed();
                }
            });
    });
    if op.action.needs_operand() {
        changed |= tag_field(ui, "renumber_operand", &mut op.operand, 180.0);
        if op.action.needs_number() {
            ui.label(
                egui::RichText::new("This one takes a number.")
                    .weak()
                    .small(),
            );
        }
    }

    ui.add_space(4.0);
    changed |= ui
        .checkbox(&mut op.numbers.minus_signs, "Identify minus signs")
        .on_hover_text(
            "If a minus sign is found immediately in front of a number it is \
             treated as part of the number.",
        )
        .changed();
    changed |= ui
        .checkbox(&mut op.numbers.decimal_points, "Identify decimal points")
        .on_hover_text(
            "If a period or comma is found between two numbers it is interpreted \
             as a decimal point.",
        )
        .changed();
    changed |= ui
        .checkbox(&mut op.keep_length, "Zero pad to keep previous length")
        .on_hover_text(
            "Numbers will be padded with zeros so their new length matches the old \
             one. Not available when using decimal fractions.",
        )
        .changed();

    changed
}

/// One of the two optional range limits: a checkbox that owns an `Option`.
fn threshold(ui: &mut egui::Ui, label: &str, id: &str, value: &mut Option<i64>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut on = value.is_some();
        if ui.checkbox(&mut on, label).changed() {
            *value = on.then_some(0);
            changed = true;
        }
        if let Some(number) = value.as_mut() {
            changed |= crate::widgets::number::add(
                ui,
                egui::DragValue::new(number)
                    .speed(0.2)
                    .range(-1_000_000..=1_000_000),
            )
            .changed();
        } else {
            ui.add_enabled(false, egui::DragValue::new(&mut 0i64));
        }
        let _ = id;
    });
    changed
}
