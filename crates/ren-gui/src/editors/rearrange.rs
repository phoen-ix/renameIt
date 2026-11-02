//! The Rearrange card (D258): what the rule does, a way back into the
//! builder, and the rule as text for whoever wants to type it.

use ren_core::NameTransform;
use ren_core::ops::rearrange::{Rearrange, Split};

pub fn ui(ui: &mut egui::Ui, op: &mut Rearrange, cx: &super::EditorCx<'_>) -> bool {
    let mut changed = false;
    let label = if op.layout.is_empty() {
        "Open the builder…"
    } else {
        "Edit in builder…"
    };
    if ui
        .button(label)
        .on_hover_text("Cut a sample name into blocks and drag them into a new order")
        .clicked()
    {
        cx.requests.open_rearrange.set(Some(cx.card));
    }
    if op.layout.is_empty() {
        ui.weak("No rule yet: this card leaves every name alone.");
        return false;
    }
    ui.label(op.summary());

    // The rule as text, for anyone who would rather type it (P124).
    ui.horizontal(|ui| match &mut op.split {
        Split::Parts(spec) => {
            let label = ui.label("Cut by:");
            changed |= ui
                .text_edit_singleline(&mut spec.pattern)
                .labelled_by(label.id)
                .on_hover_text("<%1> … <%9> are the blocks; anything else is a separator")
                .changed();
        }
        Split::Regex(regex) => {
            let label = ui.label("Cut by regex:");
            changed |= ui
                .text_edit_singleline(regex)
                .labelled_by(label.id)
                .on_hover_text("Each capture group ( … ) is a block, in order")
                .changed();
        }
    });
    if changed {
        // A new cut is a new rule: rebuild so the compiled split is fresh (D21).
        *op = op.clone();
    }
    changed
}
