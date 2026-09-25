//! Numeric spinners, and the one fact about them the rest of the app needs.
//!
//! A `DragValue` that has the keyboard **is a text box**. egui builds one out of
//! the other — `drag_value.rs:554` adds a `TextEdit::singleline(…).id(id)` under
//! the spinner's *own* id whenever `has_focus(id)` — so it stores a
//! `TextEditState` there, and `Context::text_edit_focused()`, which asks exactly
//! "does the focused id have a `TextEditState`", cannot tell the two apart.
//!
//! `RenameItApp::handle_hotkeys` stands every shortcut down while that is true,
//! for a reason that is right about text and wrong about numbers: *"Ctrl+Z in a
//! Find box means 'undo that keystroke'"*. So tabbing onto **from pos:** — or
//! clicking it, which is how anyone configures a card — silently disabled F2,
//! F4, F5, F6, F8, F9, F12, Ctrl+Z and Ctrl+K until the user clicked elsewhere.
//! [`has_focus`] brings the function keys back. Ctrl+Z and Ctrl+K stay down:
//! the box's own `TextEdit` answers Ctrl+Z by undoing the digit, and that
//! keystroke must not also undo the last batch on disk (P81).
//!
//! Nothing in egui distinguishes them: `WidgetRect` carries no role, and reading
//! the accessibility tree would answer differently in the app (where AccessKit
//! is off unless a screen reader asks for it) than under `egui_kittest` (where
//! it is always on) — a test that passes for a reason the app does not share.
//! So the answer comes from the only place that knows: here.

/// Where the spinner with the keyboard records itself.
fn marker() -> egui::Id {
    egui::Id::new("renameit_focused_number_box")
}

/// Adds a numeric spinner, remembering it if it has the keyboard.
///
/// Every `DragValue` in the app goes through this. One that did not would be
/// invisible to [`has_focus`], and the symptom — a dead F5 on one card and a
/// live one on the next — is the kind nobody reports precisely.
pub fn add(ui: &mut egui::Ui, drag: egui::DragValue<'_>) -> egui::Response {
    let response = ui.add(drag);
    if response.has_focus() {
        let id = response.id;
        ui.ctx().data_mut(|d| d.insert_temp(marker(), id));
    }
    response
}

/// The same, for a spinner that is greyed out under some condition.
///
/// A disabled one cannot take the keyboard, so it records nothing — but it goes
/// through here anyway, because the next person to flip the condition should not
/// have to know that this file exists.
pub fn add_enabled(ui: &mut egui::Ui, enabled: bool, drag: egui::DragValue<'_>) -> egui::Response {
    let response = ui.add_enabled(enabled, drag);
    if response.has_focus() {
        let id = response.id;
        ui.ctx().data_mut(|d| d.insert_temp(marker(), id));
    }
    response
}

/// Whether the widget holding the keyboard is a number box.
///
/// Self-correcting rather than cleared per frame: it compares the id that was
/// last recorded against the id that actually has focus now, so focus moving to
/// a real text field makes this false on the very next frame with no bookkeeping
/// to forget.
pub fn has_focus(ctx: &egui::Context) -> bool {
    let focused = ctx.memory(|m| m.focused());
    focused.is_some() && ctx.data(|d| d.get_temp::<egui::Id>(marker())) == focused
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing focused is not a number box focused — the case that would make
    /// the hotkey guard a no-op for everyone.
    #[test]
    fn an_idle_window_has_no_number_box_focused() {
        let ctx = egui::Context::default();
        assert!(!has_focus(&ctx));
    }

    /// A stale record must not outlive the focus it described. Without the
    /// comparison against the *currently* focused id, one visit to a spinner
    /// would exempt every text field in the app from the guard for ever after.
    #[test]
    fn a_remembered_box_stops_counting_once_something_else_has_the_keyboard() {
        let ctx = egui::Context::default();
        let spinner = egui::Id::new("a number");
        let field = egui::Id::new("a find box");

        ctx.data_mut(|d| d.insert_temp(marker(), spinner));
        ctx.memory_mut(|m| m.request_focus(spinner));
        assert!(has_focus(&ctx));

        ctx.memory_mut(|m| m.request_focus(field));
        assert!(
            !has_focus(&ctx),
            "the record is about a widget that lost it"
        );
    }
}
