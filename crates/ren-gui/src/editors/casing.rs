//! Set Casing.
//!
//! One mode selector rather than two: under D19 scope is engine-owned, so this
//! edits a single mode and the panel's Scope control decides what it applies
//! to.

use ren_core::ops::{CaseMode, Casing};

const MODES: [(CaseMode, &str); 7] = [
    (CaseMode::Upper, "UPPER CASE"),
    (CaseMode::Lower, "lower case"),
    (CaseMode::Sentence, "Sentence case"),
    (CaseMode::Title, "Title Case"),
    (CaseMode::NoChange, "No change"),
    (CaseMode::Invert, "iNVERT"),
    (CaseMode::Random, "rANdOm"),
];

pub fn ui(ui: &mut egui::Ui, op: &mut Casing) -> bool {
    let mut changed = false;

    egui::Grid::new("casing_modes")
        .num_columns(2)
        .spacing([16.0, 2.0])
        .show(ui, |ui| {
            for chunk in MODES.chunks(2) {
                for (mode, label) in chunk {
                    changed |= ui.radio_value(&mut op.mode, *mode, *label).changed();
                }
                ui.end_row();
            }
        });

    ui.add_space(6.0);
    ui.separator();

    // Disabled outside Title Case, where the engine ignores it entirely
    // (`casing.rs` gates the whole branch on the mode). A tickable box that
    // does nothing is worse than no box: the Seed control twenty lines below
    // is already guarded this way, so this was the odd one out.
    changed |= ui
        .add_enabled_ui(op.mode == CaseMode::Title, |ui| {
            ui.checkbox(&mut op.lowercase_exceptions, "Lowercase exceptions")
                .on_hover_text(
                    "Title Case only: keeps short words like \"of\" and \"the\" lower case, \
                     except as the first word.",
                )
                .changed()
        })
        .inner;
    changed |= ui
        .checkbox(&mut op.preserve_all_upper, "Preserve all upper case words")
        .on_hover_text("Words already spelled in capitals keep them — useful for abbreviations.")
        .changed();
    changed |= ui
        .checkbox(&mut op.preserve_mixed, "Preserve mixed case words")
        .on_hover_text("Words with any capital keep their current spelling, e.g. iPhone.")
        .changed();
    // > *"Exceptions are words that should always be spelled with a certain
    // > case… **Click on the button to edit the list of exceptions.**"*
    //
    // The link edits **this card's** list, not the Settings page's. That is the
    // opposite of Music Rename's `(edit styles)`, and deliberately so: D66
    // makes that one card-independent because the card stores a *pattern* and
    // never looks at the style list again, whereas a Set Casing card stores the
    // words themselves (D35 — Settings holds the copy a *new* card starts
    // from). A link to Settings would therefore edit a list this card has
    // already stopped reading, and visibly do nothing to the preview beside it.
    let open_id = ui.id().with("casing_exceptions_open");
    let mut open: bool = ui.data_mut(|d| d.get_temp(open_id).unwrap_or(false));
    ui.horizontal(|ui| {
        changed |= ui
            .checkbox(&mut op.use_exceptions, "Use the exceptions list")
            .on_hover_text(format!(
                "{} fixed-case words: {}…",
                op.rules.exceptions.words.len(),
                op.rules
                    .exceptions
                    .words
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .changed();
        if ui
            .link(if open {
                "(hide exceptions)"
            } else {
                "(edit exceptions)"
            })
            .on_hover_text("The words this card always spells a fixed way")
            .clicked()
        {
            open = !open;
        }
    });
    ui.data_mut(|d| d.insert_temp(open_id, open));

    if open {
        // Inline, inside the card, rather than a dialog — the same call D143
        // took for Visual Assist. The list is short and the point of editing it
        // is watching the preview change.
        // **No scroll area of its own.** The operation panel already scrolls,
        // and nesting one inside it puts *Add word* below the fold of a box
        // that is itself below the fold — reachable by the accessibility tree
        // and not by a pointer, which is a control that exists for a screen
        // reader and nobody else.
        ui.indent("casing_exceptions", |ui| {
            changed |= crate::widgets::string_list::StringList::new("casing_card_exception_row")
                .hint("CD")
                .add_label("+ Add word")
                .width(200.0)
                .defaults(
                    "The list this card started from",
                    ren_core::ops::CasingRules::default().exceptions.words,
                )
                .show(ui, &mut op.rules.exceptions.words);
        });
    }

    if op.mode == CaseMode::Random {
        ui.horizontal(|ui| {
            ui.label("Seed:");
            changed |=
                crate::widgets::number::add(ui, egui::DragValue::new(&mut op.seed).speed(1.0))
                    .on_hover_text("Random casing is seeded, so the preview and the rename agree.")
                    .changed();
        });
    }

    changed
}
