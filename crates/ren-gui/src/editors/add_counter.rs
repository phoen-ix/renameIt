//! Add Counter.

use ren_core::ops::{AddCounter, CounterPlacement};

use crate::theme::width::FIELD_TOKEN;
use crate::widgets::form::{After, Form};
use crate::widgets::tag_field::tag_field;

pub fn ui(ui: &mut egui::Ui, op: &mut AddCounter) -> bool {
    let mut changed = false;

    Form::new("add_counter").show(ui, |form| {
        let placement = form.row("Put counter:", After::Nothing, |row| {
            let mut changed = false;
            for placement in [CounterPlacement::First, CounterPlacement::Last] {
                changed |= row
                    .ui()
                    .radio_value(&mut op.placement, placement, placement.label())
                    .changed();
            }
            changed
        });
        changed |= placement.inner;
        placement
            .response
            .on_hover_text("Select where to place the counter, relative to the filename.");

        changed |= form
            .row("Separator:", After::Nothing, |row| {
                row.ui()
                    .add(
                        egui::TextEdit::singleline(&mut op.separator)
                            .desired_width(FIELD_TOKEN)
                            .id_salt("counter_separator"),
                    )
                    .on_hover_text("Separates the filename and the added number")
                    .changed()
            })
            .inner;
    });

    ui.add_space(4.0);
    changed |= ui
        .radio_value(&mut op.replace_name, false, "Keep current filename.")
        .changed();
    // Its own form: the radio's text is the label of the box beside it, and a
    // column that wide would push the two short rows above it off to the right.
    Form::new("counter_replacement").show(ui, |form| {
        let replace = form.radio(
            op.replace_name,
            "Replace current filename with:",
            After::TagPicker,
            |row| {
                if !op.replace_name {
                    return false;
                }
                let width = row.field_width();
                row.column(|ui| tag_field(ui, "counter_replacement", &mut op.replacement, width))
            },
        );
        changed |= replace.inner;
        if replace.label.clicked() && !op.replace_name {
            op.replace_name = true;
            changed = true;
        }
    });

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
