//! Move Section.
//!
//! Note the *two* independent "counting backwards" switches, one per
//! position.

use ren_core::ops::MoveSection;

use crate::widgets::form::{After, Form};

pub fn ui(ui: &mut egui::Ui, op: &mut MoveSection, cx: &super::EditorCx<'_>) -> bool {
    let mut changed = false;

    Form::new("move_section").show(ui, |form| {
        changed |= form
            .row("Cut:", After::Note("character(s)"), |row| {
                let changed = super::number_box(row.ui(), &mut op.cut).changed();
                row.ui().label("character(s)");
                changed
            })
            .inner;
        changed |= form
            .row("from pos:", After::Check("counting backwards"), |row| {
                let mut changed = super::number_box(row.ui(), &mut op.from_pos).changed();
                changed |= row
                    .ui()
                    .checkbox(&mut op.from_backwards, "counting backwards")
                    .changed();
                super::assist_button(row.ui(), cx, super::assist::AssistTarget::MoveCut);
                changed
            })
            .inner;

        changed |= form
            .row(
                "and paste at position:",
                After::Check("counting backwards"),
                |row| {
                    let mut changed = super::number_box(row.ui(), &mut op.to_pos).changed();
                    changed |= row
                        .ui()
                        .checkbox(&mut op.to_backwards, "counting backwards")
                        .changed();
                    changed
                },
            )
            .inner;
        // A qualifier of the paste position, so it sits under that box rather
        // than under the labels.
        changed |= form
            .unlabelled(After::Nothing, |row| {
                row.ui()
                    .checkbox(&mut op.relative, "relative to original pos")
                    .on_hover_text(
                        "Move this many steps from where the section was cut, rather than to a \
                         fixed position",
                    )
                    .changed()
            })
            .inner;
    });

    ui.label(
        egui::RichText::new("The paste position is measured after the cut has taken place.")
            .weak()
            .small(),
    );

    changed
}
