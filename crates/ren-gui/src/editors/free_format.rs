//! Free Format.

use ren_core::ops::FreeFormat;

use crate::widgets::form::Form;
use crate::widgets::tag_field::tag_field_with_history;

pub fn ui(ui: &mut egui::Ui, op: &mut FreeFormat, cx: &super::EditorCx<'_>) -> bool {
    let mut changed = false;

    // A form row like every other label-and-field editor, so the box takes the
    // width left after its `<tags>` button and history chevron. A fixed 260 pt
    // box plus those two did not fit a card in the default pipeline panel, and
    // the buttons that spilled past its edge widened the card and the panel.
    Form::new("free_format").show(ui, |form| {
        let history = cx.history("free_format_pattern");
        changed |= form
            .row("Pattern:", super::after_tag_field(history), |row| {
                let width = row.field_width();
                row.column(|ui| {
                    tag_field_with_history(
                        ui,
                        "free_format_pattern",
                        &mut op.pattern,
                        width,
                        history,
                    )
                })
            })
            .inner;
    });

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Builds the whole name from literal text and <tags>, in any mix — for example \
             <Parent>_<FullName>. This one replaces the name outright, so it normally \
             processes the extension too.",
        )
        .weak()
        .small(),
    );

    if op.pattern.is_empty() {
        ui.label(
            egui::RichText::new("An empty pattern leaves every name alone.")
                .weak()
                .small(),
        );
    }

    changed
}
