//! Zero Padding.

use ren_core::ops::ZeroPadding;

use crate::widgets::form::{After, Form};

pub fn ui(ui: &mut egui::Ui, op: &mut ZeroPadding) -> bool {
    let mut changed = false;

    Form::new("zero_pad").show(ui, |form| {
        let digits = form.row("Digits:", After::Nothing, |row| {
            super::number_box(row.ui(), &mut op.digits).changed()
        });
        changed |= digits.inner;
        digits.response.on_hover_text(
            "Numbers in filenames will be padded with zeros to attain this length. \
             If a number is longer, it is cropped instead.",
        );
        form.note("So that 1, 10, 2 sorts as 01, 02, 10.");
    });

    changed
}
