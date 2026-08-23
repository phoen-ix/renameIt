//! Move Section.
//!
//! Note the *two* independent "counting backwards" switches, one per
//! position.

use ren_core::ops::MoveSection;

pub fn ui(ui: &mut egui::Ui, op: &mut MoveSection, cx: &super::EditorCx<'_>) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        changed |= super::number(ui, "Cut:", &mut op.cut);
        ui.label("character(s)");
    });
    ui.horizontal(|ui| {
        changed |= super::number(ui, "from pos:", &mut op.from_pos);
        changed |= ui
            .checkbox(&mut op.from_backwards, "counting backwards")
            .changed();
        super::assist_button(ui, cx, super::assist::AssistTarget::MoveCut);
    });

    ui.add_space(6.0);
    ui.label("and paste at position:");
    ui.horizontal(|ui| {
        changed |= super::number(ui, "", &mut op.to_pos);
        changed |= ui
            .checkbox(&mut op.to_backwards, "counting backwards")
            .changed();
    });
    changed |= ui
        .checkbox(&mut op.relative, "relative to original pos")
        .on_hover_text(
            "Move this many steps from where the section was cut, rather than to a \
                        fixed position",
        )
        .changed();

    ui.label(
        egui::RichText::new("The paste position is measured after the cut has taken place.")
            .weak()
            .small(),
    );

    changed
}
