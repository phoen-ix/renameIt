//! Re-Number.

use ren_core::ops::{NumberAction, NumberTarget, ReNumber};

use crate::widgets::form::{After, Form, FormUi};
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
        Form::new("renumber_only").show(ui, |form| {
            changed |= threshold(form, "larger or equal to", &mut op.at_least);
            changed |= threshold(form, "less or equal to", &mut op.at_most);
        });
    });

    ui.add_space(4.0);
    // The action captions the operand — *[x] Multiply by:* and the box it
    // multiplies by read as one line — so the combo is the row's label.
    let needs_operand = op.action.needs_operand();
    Form::new("renumber_action").show(ui, |form| {
        let line = form.labelled(
            |ui| {
                egui::ComboBox::from_id_salt("renumber_action")
                    .selected_text(op.action.label())
                    .show_ui(ui, |ui| {
                        for action in NumberAction::all() {
                            changed |= ui
                                .selectable_value(&mut op.action, action, action.label())
                                .changed();
                        }
                    })
                    .response
            },
            if needs_operand {
                After::TagPicker
            } else {
                After::Nothing
            },
            |row| {
                if !needs_operand {
                    return false;
                }
                let width = row.field_width();
                row.column(|ui| tag_field(ui, "renumber_operand", &mut op.operand, width))
            },
        );
        changed |= line.inner;
        if op.action == NumberAction::ZeroPadTo {
            // The engine refuses a wider one as a row error (`MAX_PAD_WIDTH`),
            // so the limit is said where the width is typed.
            form.note(format!(
                "This one takes a width, up to {}.",
                ren_core::ops::MAX_PAD_WIDTH
            ));
        } else if needs_operand && op.action.needs_number() {
            form.note("This one takes a number.");
        }
    });

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
///
/// The box is drawn from the state the row started the frame in and the
/// toggle is applied after it — the checkbox is the row's label, so it has
/// been drawn before the content is.
fn threshold(form: &mut FormUi<'_>, label: &str, value: &mut Option<i64>) -> bool {
    let mut on = value.is_some();
    let line = form.check(&mut on, label, After::Nothing, |row| match value.as_mut() {
        Some(number) => crate::widgets::number::add(
            row.ui(),
            egui::DragValue::new(number)
                .speed(0.2)
                .range(-1_000_000..=1_000_000),
        )
        .changed(),
        None => {
            crate::widgets::number::add_enabled(row.ui(), false, egui::DragValue::new(&mut 0i64));
            false
        }
    });
    let mut changed = line.inner;
    if line.label.changed() {
        *value = on.then_some(0);
        changed = true;
    }
    changed
}
