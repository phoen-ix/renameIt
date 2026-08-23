//! A three-state checkbox: set / clear / keep-current.
//!
//! *"To keep the current attributes, set the
//! checkboxes to gray (shows in XP as a 'full' box) for the attribute you don't
//! want to change."*
//!
//! Built on [`egui::Checkbox::indeterminate`] rather than painted by hand,
//! because that is what makes the grey state *readable*: egui maps an
//! indeterminate checkbox to `accesskit::Toggled::Mixed`, so it reaches the
//! accessibility tree — a screen reader announces it, and a headless test can
//! assert on it. A hand-drawn dash would be invisible to both.

/// The next state, in the Win32 `BS_AUTO3STATE` order.
///
/// Grey → cleared → set → grey. Starting from grey, **one** click clears, which
/// is exactly *"simply uncheck the write protected checkbox"* for the
/// files-copied-off-a-CD case. The other order would make that one use case
/// take two clicks.
pub fn next(value: Option<bool>) -> Option<bool> {
    match value {
        None => Some(false),
        Some(false) => Some(true),
        Some(true) => None,
    }
}

/// What the current state means, in words — for the hover text, and so a test
/// can read the intent rather than a tri-state it has to decode.
pub fn state_word(value: Option<bool>) -> &'static str {
    match value {
        None => "keep",
        Some(false) => "clear",
        Some(true) => "set",
    }
}

/// Draws one. Returns true when the user changed it.
pub fn tri_checkbox(ui: &mut egui::Ui, value: &mut Option<bool>, label: &str) -> bool {
    // egui's `Checkbox` owns a `&mut bool` and flips it on click regardless, so
    // it gets a scratch copy and the real cycle happens here.
    let mut shown = value.unwrap_or(false);
    let response = ui
        .add(egui::Checkbox::new(&mut shown, label).indeterminate(value.is_none()))
        .on_hover_text(format!(
            "Currently: {}. Click to cycle keep → clear → set.",
            state_word(*value)
        ));
    if response.clicked() {
        *value = next(*value);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From grey, one click clears — the worked example.
    #[test]
    fn the_cycle_matches_a_windows_three_state_box() {
        assert_eq!(next(None), Some(false), "grey → cleared, in one click");
        assert_eq!(next(Some(false)), Some(true));
        assert_eq!(next(Some(true)), None);
        // And three clicks are the identity, so nothing is unreachable.
        assert_eq!(next(next(next(None))), None);
    }

    #[test]
    fn keep_clear_and_set_each_have_a_word() {
        assert_eq!(state_word(None), "keep");
        assert_eq!(state_word(Some(false)), "clear");
        assert_eq!(state_word(Some(true)), "set");
    }
}
