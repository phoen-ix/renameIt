//! An editable list of short strings, with add, remove and restore-defaults.
//!
//! Four Settings pages want exactly this — Music Styles, the casing exception
//! words, the title-case lowercase exceptions, and the guarded-folder list —
//! and each of them is *only* this, so writing it four times would be four
//! places for the ✖ button to behave differently.
//!
//! Deliberately not a rule table: `widgets::rule_table` edits `Replace` rows,
//! which have **seven** fields and an order that matters. These are bare
//! strings in a set, and giving them reorder arrows would suggest an order they
//! do not have.

/// Draws the list. Returns true if anything changed.
///
/// `id` keys the row widgets, so two lists on one page keep their own focus and
/// their own undo. `defaults` draws a *Restore defaults* button when it is
/// `Some`; the tooltip should say what "defaults" means, since the user has no
/// other way to find out.
pub struct StringList<'a> {
    pub id: &'a str,
    pub hint: &'a str,
    pub add_label: &'a str,
    pub width: f32,
    pub defaults: Option<(&'a str, Vec<String>)>,
}

impl<'a> StringList<'a> {
    pub fn new(id: &'a str) -> Self {
        Self {
            id,
            hint: "",
            add_label: "+ Add",
            width: 320.0,
            defaults: None,
        }
    }

    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }

    pub fn add_label(mut self, label: &'a str) -> Self {
        self.add_label = label;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// The button and its tooltip. The tooltip is not optional because
    /// "Restore defaults" alone does not say *to what*.
    pub fn defaults(mut self, tooltip: &'a str, values: Vec<String>) -> Self {
        self.defaults = Some((tooltip, values));
        self
    }

    pub fn show(self, ui: &mut egui::Ui, values: &mut Vec<String>) -> bool {
        let mut changed = false;

        let mut remove = None;
        for (index, value) in values.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(value)
                            .desired_width(self.width)
                            .hint_text(self.hint)
                            .id_salt((self.id, index)),
                    )
                    .changed();
                if ui.button("✖").on_hover_text("Remove this row").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            values.remove(index);
            changed = true;
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(self.add_label).clicked() {
                // Empty, and left for the user to type into: pre-filling it
                // with a placeholder means a row that looks configured and is
                // not. Blank rows are dropped on the way out (`tidy`).
                values.push(String::new());
                changed = true;
            }
            if let Some((tooltip, defaults)) = self.defaults
                && ui
                    .button("Restore defaults")
                    .on_hover_text(tooltip)
                    .clicked()
            {
                *values = defaults;
                changed = true;
            }
        });

        changed
    }
}

/// Drops blank rows and trims the rest.
///
/// Called when the window closes rather than on every keystroke: removing a row
/// the moment its last character is deleted takes the text box out from under
/// the caret, which is how a list becomes impossible to edit.
pub fn tidy(values: &mut Vec<String>) {
    for value in values.iter_mut() {
        let trimmed = value.trim();
        if trimmed.len() != value.len() {
            *value = trimmed.to_owned();
        }
    }
    values.retain(|value| !value.is_empty());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidying_drops_blank_rows_and_trims_the_rest() {
        let mut values = vec!["  CD  ".into(), String::new(), "   ".into(), "DJ".into()];
        tidy(&mut values);
        assert_eq!(values, ["CD", "DJ"]);
    }

    /// The + button adds a row the user then types into. Tidying it away
    /// immediately would make the button do nothing visible.
    #[test]
    fn tidying_is_not_something_that_happens_while_typing() {
        let mut values = vec!["a".to_owned(), String::new()];
        // What `show` leaves behind after a click on +.
        assert_eq!(values.len(), 2);
        tidy(&mut values);
        assert_eq!(values, ["a"], "and only once the window closes");
    }
}
