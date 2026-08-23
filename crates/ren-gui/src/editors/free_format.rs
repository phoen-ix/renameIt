//! Free Format.

use ren_core::ops::FreeFormat;

use crate::widgets::tag_field::tag_field_with_history;

pub fn ui(ui: &mut egui::Ui, op: &mut FreeFormat, cx: &super::EditorCx<'_>) -> bool {
    let mut changed = false;

    ui.label("Build the whole name:");
    changed |= tag_field_with_history(
        ui,
        "free_format_pattern",
        &mut op.pattern,
        260.0,
        cx.history("free_format_pattern"),
    );

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Literal text and <tags>, in any mix — the worked example is \
             <Parent>_<FullName>. This one replaces the name outright, so it \
             normally processes the extension too.",
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
