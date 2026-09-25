//! The `<Ask>` modal.
//!
//! `docs/DESIGN.md` Part 1 §3 and D28: the engine never blocks on a user, so
//! every `<Ask>` in the pipeline is collected **once, before the run**, and the
//! answers travel with it. One form with a field per slot, rather than one
//! dialog per tag and four of them in a row.

use ren_core::{Answers, AskSpec};

/// The form while it is open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AskForm {
    /// `(slot, text)`, in slot order.
    pub fields: Vec<(u8, String)>,
    /// Whether the pipeline also wants the clipboard.
    pub clipboard: bool,
    /// Whether the first field has been handed the keyboard yet — once, as
    /// the palette does, because asking every frame keeps it from settling.
    focused: bool,
}

impl AskForm {
    pub fn new(asks: &[AskSpec], clipboard: bool) -> Self {
        Self {
            fields: asks.iter().map(|a| (a.slot, String::new())).collect(),
            clipboard,
            focused: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && !self.clipboard
    }

    /// What the run will use.
    pub fn answers(&self) -> Answers {
        Answers {
            asks: self.fields.iter().cloned().collect(),
            clipboard: self.clipboard.then(read_clipboard).flatten(),
        }
    }
}

/// What the user did with the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskOutcome {
    Open,
    Confirmed,
    Cancelled,
}

/// Draws the form.
///
/// Typed into from the keyboard alone: the first field has the keyboard when
/// the form opens, and Enter in any field is **Rename**. A modal that asked a
/// question and then needed the mouse to start answering it, and again to
/// submit, was the one place in the run where a keyboard user got stuck.
pub fn ui(ctx: &egui::Context, form: &mut AskForm) -> AskOutcome {
    let mut outcome = AskOutcome::Open;

    let modal = egui::Modal::new(egui::Id::new("ask_modal")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.heading("Enter text");
        ui.add_space(6.0);

        let first_frame = !form.focused;
        form.focused = true;
        for (index, (slot, text)) in form.fields.iter_mut().enumerate() {
            ui.label(if *slot == 0 {
                "<Ask>".to_owned()
            } else {
                format!("<Ask-{slot}>")
            });
            let field = ui.add(
                egui::TextEdit::singleline(text)
                    .desired_width(f32::INFINITY)
                    .id_salt(("ask_field", *slot)),
            );
            if first_frame && index == 0 {
                field.request_focus();
            }
            // A single-line box gives the keyboard up on Enter, which is what
            // `lost_focus` reads — the same test the inline rename uses.
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                outcome = AskOutcome::Confirmed;
            }
            ui.add_space(4.0);
        }
        if form.clipboard {
            ui.label(
                egui::RichText::new("<Clipboard> will be read when the rename starts.")
                    .weak()
                    .small(),
            );
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Rename").clicked() {
                outcome = AskOutcome::Confirmed;
            }
            if ui.button("Cancel").clicked() {
                outcome = AskOutcome::Cancelled;
            }
        });
    });

    if modal.should_close() && outcome == AskOutcome::Open {
        outcome = AskOutcome::Cancelled;
    }
    outcome
}

/// `<Clipboard>` — best effort. A machine with no clipboard (a CI runner, a
/// bare tty) simply leaves the tag unavailable, which is exactly what Run
/// Settings' *Only rename if all tags are available* is for.
fn read_clipboard() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_has_one_field_per_distinct_slot() {
        let form = AskForm::new(&[AskSpec { slot: 0 }, AskSpec { slot: 3 }], false);
        assert_eq!(form.fields.len(), 2);
        assert!(!form.is_empty());
        assert!(AskForm::default().is_empty());
    }

    #[test]
    fn the_answers_carry_what_was_typed() {
        let mut form = AskForm::new(&[AskSpec { slot: 0 }, AskSpec { slot: 2 }], false);
        form.fields[0].1 = "holiday".into();
        form.fields[1].1 = "2024".into();

        let answers = form.answers();
        assert_eq!(answers.ask(0), Some("holiday"));
        assert_eq!(answers.ask(2), Some("2024"));
        assert_eq!(answers.clipboard, None);
    }
}
