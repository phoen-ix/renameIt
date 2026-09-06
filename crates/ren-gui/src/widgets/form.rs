//! A two-column form: labels in one column, controls in the other.
//!
//! Every operation editor used to draw its rows as independent `ui.horizontal`
//! blocks, which produced two faults at once. The obvious one is a ragged left
//! edge — a field starts at `label_width + item_spacing.x`, so `Find:` and
//! `Replace with:` began 54 points apart. The subtler one is that the fields
//! were not even the same *width*: a `TextEdit` clamps its requested width to
//! what is left in the row, so the field after the longer label came out
//! shorter, and the button after it was pushed against the card's edge. What
//! looked like a right-aligned `<tags>` button was an overflow artefact.
//!
//! Both follow from asking for a width instead of being given one. Here the
//! label column is measured once by the grid, and a field is told what is left:
//!
//! ```ignore
//! Form::new("replace").show(ui, |form| {
//!     changed |= form.row("Find:", After::Marker("⌖ find"), |row| {
//!         let w = row.field_width();
//!         tag_field::text_with_history(row.ui(), "replace_find", &mut op.find, w, hint, past)
//!     }).inner;
//! });
//! ```
//!
//! # Two things that must not change
//!
//! [`Form::show`] sets `num_columns(2)` because egui gives the *last* column
//! the "take what is left" rule and only when the count is known. And it must
//! never set `max_col_width`: that turns wrapping on for the whole grid, and
//! the labels start breaking across two lines.
//!
//! # Why [`After`] is an enum
//!
//! A field's width is `what is left − what comes after it`, so the width of
//! what comes after has to be known *before* the field is drawn. The tempting
//! way to find it is to draw the trailing widget into a sizing pass and measure
//! the result. That is not available: a widget registers itself with the
//! accessibility tree even when it is drawn invisibly, so anything measured
//! that way appears **twice** to a test that looks widgets up by role or label.
//! Every variant here is measured by arithmetic instead — a galley for text,
//! [`icons::button_size`](crate::widgets::icons::button_size) for an icon.

use crate::theme::width;

/// What sits to the right of the field in a row.
///
/// Only the *width* is computed here. The widgets themselves are still drawn by
/// the caller, in the order they read.
#[derive(Clone, Copy, Default)]
pub enum After<'a> {
    /// The field takes the rest of the row.
    #[default]
    Nothing,
    /// A short note after the field — "character(s)".
    Note(&'a str),
    /// Room for N icon buttons: a history chevron, a folder picker.
    Buttons(usize),
    /// The `<tags>` picker, which is a text button rather than an icon.
    TagPicker,
    /// The `<tags>` picker followed by N icon buttons.
    TagPickerAnd(usize),
    /// A checkbox trailing the field — "counting backwards".
    Check(&'a str),
    /// A `selectable_label` trailing the field — a Visual Assist marker.
    Marker(&'a str),
    /// N icon buttons, then a marker — Find's history chevron and its ⌖.
    ButtonsAndMarker(usize, &'a str),
}

impl After<'_> {
    /// How much room this needs, including the spacing in front of it.
    ///
    /// Zero for [`After::Nothing`], so a row with nothing after it gets the
    /// whole cell rather than the whole cell minus a gap that is not there.
    pub fn width(&self, ui: &egui::Ui) -> f32 {
        let gap = ui.spacing().item_spacing.x;
        match self {
            Self::Nothing => 0.0,
            Self::Note(text) => gap + text_width(ui, text, egui::TextStyle::Body),
            Self::Buttons(n) => (gap + icon_width(ui)) * *n as f32,
            Self::TagPicker => gap + button_width(ui, "<tags>"),
            Self::TagPickerAnd(n) => {
                gap + button_width(ui, "<tags>") + (gap + icon_width(ui)) * *n as f32
            }
            Self::Check(text) => {
                gap + ui.spacing().icon_width
                    + ui.spacing().icon_spacing
                    + text_width(ui, text, egui::TextStyle::Body)
            }
            Self::Marker(text) => gap + button_width(ui, text),
            Self::ButtonsAndMarker(n, text) => {
                (gap + icon_width(ui)) * *n as f32 + gap + button_width(ui, text)
            }
        }
    }
}

fn text_width(ui: &egui::Ui, text: &str, style: egui::TextStyle) -> f32 {
    egui::WidgetText::from(text)
        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, style)
        .size()
        .x
}

fn button_width(ui: &egui::Ui, text: &str) -> f32 {
    text_width(ui, text, egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.x
}

fn icon_width(ui: &egui::Ui) -> f32 {
    crate::widgets::icons::button_size(ui).x
}

/// A two-column form.
pub struct Form {
    id_salt: &'static str,
}

impl Form {
    /// `id_salt` names the grid. It needs to be unique only within one card:
    /// the operation panel already pushes each card's own id, so two cards of
    /// one kind keep their column widths apart without help.
    pub fn new(id_salt: &'static str) -> Self {
        Self { id_salt }
    }

    pub fn show<R>(self, ui: &mut egui::Ui, body: impl FnOnce(&mut FormUi<'_>) -> R) -> R {
        egui::Grid::new(self.id_salt)
            .num_columns(2)
            .spacing([ui.spacing().item_spacing.x, crate::theme::space::TIGHT])
            .show(ui, |ui| {
                let mut form = FormUi { ui };
                body(&mut form)
            })
            .inner
    }
}

/// The form, mid-draw.
pub struct FormUi<'a> {
    ui: &'a mut egui::Ui,
}

impl FormUi<'_> {
    /// A row whose label is plain text.
    pub fn row<R>(
        &mut self,
        label: &str,
        after: After<'_>,
        content: impl FnOnce(&mut Row<'_>) -> R,
    ) -> Line<R> {
        self.build(after, content, |ui| ui.label(label))
    }

    /// A row whose label is a checkbox. `Line::label.changed()` is the answer.
    pub fn check<R>(
        &mut self,
        on: &mut bool,
        label: &str,
        after: After<'_>,
        content: impl FnOnce(&mut Row<'_>) -> R,
    ) -> Line<R> {
        self.build(after, content, |ui| ui.checkbox(on, label))
    }

    /// A row whose label is a radio. `Line::label.clicked()` is the answer.
    pub fn radio<R>(
        &mut self,
        selected: bool,
        label: &str,
        after: After<'_>,
        content: impl FnOnce(&mut Row<'_>) -> R,
    ) -> Line<R> {
        self.build(after, content, |ui| ui.radio(selected, label))
    }

    /// A row with no label, for a control that continues the row above it.
    pub fn unlabelled<R>(
        &mut self,
        after: After<'_>,
        content: impl FnOnce(&mut Row<'_>) -> R,
    ) -> Line<R> {
        self.build(after, content, |ui| ui.label(""))
    }

    /// A row whose label is a control the caller draws — Re-Number's action
    /// combo, whose selected text is the operand's caption. [`Self::row`],
    /// [`Self::check`] and [`Self::radio`] are the three common shapes of this.
    pub fn labelled<R>(
        &mut self,
        label: impl FnOnce(&mut egui::Ui) -> egui::Response,
        after: After<'_>,
        content: impl FnOnce(&mut Row<'_>) -> R,
    ) -> Line<R> {
        self.build(after, content, label)
    }

    /// A weak line under the fields, in the control column so it lines up with
    /// them rather than with the labels.
    ///
    /// Wrapped at the cell, explicitly: a grid extends text unless
    /// `max_col_width` is set, which is the one thing [`Form::show`] must never
    /// do, and a note is the only prose in the form. A colour the text already
    /// carries wins over `weak`, so an error can be reported on the same line.
    pub fn note(&mut self, text: impl Into<egui::RichText>) {
        self.ui.label("");
        self.ui
            .add(egui::Label::new(text.into().weak().small()).wrap());
        self.ui.end_row();
    }

    fn build<R>(
        &mut self,
        after: After<'_>,
        content: impl FnOnce(&mut Row<'_>) -> R,
        label: impl FnOnce(&mut egui::Ui) -> egui::Response,
    ) -> Line<R> {
        let label = label(self.ui);
        let mut inner = None;
        let cell = self
            .ui
            .horizontal(|ui| {
                // `available_width` here is the grid's last-column rule: the card
                // body, less the measured label column and the spacing between
                // them. Not a literal, which is the whole point.
                let field =
                    (ui.available_width() - after.width(ui) - width::GUTTER).max(width::FIELD_MIN);
                let mut row = Row { ui, field };
                inner = Some(content(&mut row));
            })
            .response;
        self.ui.end_row();
        Line {
            inner: inner.expect("the content closure always runs"),
            response: label.union(cell),
            label,
        }
    }
}

/// One row's outcome.
pub struct Line<R> {
    /// Whatever the content closure returned — for every editor here, whether
    /// the operation changed.
    pub inner: R,
    /// The label control itself.
    pub label: egui::Response,
    /// Label and controls together, so `on_hover_text` covers the whole row.
    pub response: egui::Response,
}

/// The control cell, mid-draw.
pub struct Row<'a> {
    ui: &'a mut egui::Ui,
    field: f32,
}

impl Row<'_> {
    /// What a text field in this row should ask for.
    pub fn field_width(&self) -> f32 {
        self.field
    }

    /// The cell itself, for the field and whatever trails it.
    pub fn ui(&mut self) -> &mut egui::Ui {
        self.ui
    }

    /// The cell as a column, for a control that reports *under* itself — a tag
    /// field and its D29 message.
    ///
    /// Drawn straight into the cell, that message lands to the right of the
    /// field, past the card's edge. egui widens the card to include it, the
    /// panel grows to fit the card, and a wider panel is a wider field with the
    /// message still past its edge: the panel grows every frame for as long as
    /// the tag is misspelt. A column wraps the message at the cell instead.
    pub fn column<R>(&mut self, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
        self.ui.vertical(content).inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_ui(check: impl FnMut(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        // Fonts are built during a frame, not before one, so a galley measured
        // on the very first pass would be measured against a font set still
        // being assembled. The deltas have to be cleared or dropping the output
        // panics.
        let mut warmup = ctx.run_ui(egui::RawInput::default(), |_| {});
        warmup.textures_delta.clear();
        let mut output = ctx.run_ui(egui::RawInput::default(), check);
        output.textures_delta.clear();
    }

    /// Every variant is measured, not guessed — so none of them may answer
    /// zero except the one that means "nothing follows".
    #[test]
    fn only_nothing_reserves_no_width() {
        with_ui(|ui| {
            assert_eq!(After::Nothing.width(ui), 0.0);
            for after in [
                After::Note("character(s)"),
                After::Buttons(1),
                After::TagPicker,
                After::TagPickerAnd(1),
                After::Check("counting backwards"),
                After::Marker("\u{2316} find"),
                After::ButtonsAndMarker(1, "\u{2316} find"),
            ] {
                assert!(
                    after.width(ui) > 0.0,
                    "a trailing widget with no width would let the field overlap it"
                );
            }
        });
    }

    /// A picker plus a button is wider than the picker alone, and two buttons
    /// are wider than one. Cheap, and it catches a variant wired to the wrong
    /// measurement.
    #[test]
    fn reserved_width_grows_with_what_it_reserves_for() {
        with_ui(|ui| {
            assert!(After::TagPickerAnd(1).width(ui) > After::TagPicker.width(ui));
            assert!(After::Buttons(2).width(ui) > After::Buttons(1).width(ui));
            assert!(
                After::ButtonsAndMarker(1, "\u{2316} find").width(ui)
                    > After::Marker("\u{2316} find").width(ui)
            );
            assert!(
                After::Note("a much longer note than the other one").width(ui)
                    > After::Note("x").width(ui)
            );
        });
    }
}
