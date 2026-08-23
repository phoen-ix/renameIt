//! Add Counter.

use ren_core::ops::{AddCounter, CounterPlacement};

use crate::widgets::tag_field::tag_field;

pub fn ui(ui: &mut egui::Ui, op: &mut AddCounter) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label("Put counter:");
        for placement in [CounterPlacement::First, CounterPlacement::Last] {
            changed |= ui
                .radio_value(&mut op.placement, placement, placement.label())
                .changed();
        }
    })
    .response
    .on_hover_text("Select where to place the counter, relative to the filename.");

    ui.horizontal(|ui| {
        ui.label("Separator:");
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut op.separator)
                    .desired_width(80.0)
                    .id_salt("counter_separator"),
            )
            .on_hover_text("Separates the filename and the added number")
            .changed();
    });

    ui.add_space(4.0);
    changed |= ui
        .radio_value(&mut op.replace_name, false, "Keep current filename.")
        .changed();
    changed |= ui
        .radio_value(&mut op.replace_name, true, "Replace current filename with:")
        .changed();
    if op.replace_name {
        ui.indent("counter_replacement", |ui| {
            changed |= tag_field(ui, "counter_replacement", &mut op.replacement, 220.0);
        });
    }

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "The counter itself — start, step and padding — is set up in Counter Setup. \
             For more precise placement, use the <Counter> tag in another function.",
        )
        .weak()
        .small(),
    );

    changed
}
