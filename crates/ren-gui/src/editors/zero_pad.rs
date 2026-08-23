//! Zero Padding.

use ren_core::ops::ZeroPadding;

pub fn ui(ui: &mut egui::Ui, op: &mut ZeroPadding) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        changed |= super::number(ui, "Digits:", &mut op.digits);
    })
    .response
    .on_hover_text(
        "Numbers in filenames will be padded with zeros to attain this length. \
         If a number is longer, it is cropped instead.",
    );

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("So that 1, 10, 2 sorts as 01, 02, 10.")
            .weak()
            .small(),
    );
    changed
}
