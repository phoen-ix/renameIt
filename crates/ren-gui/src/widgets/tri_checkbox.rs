//! A three-state checkbox: set / clear / keep-current.
//!
//! Grey means *leave this attribute as each file already has it*, which is
//! what makes one Set Attributes card able to change one bit and not the
//! other three.
//!
//! Built on [`egui::Checkbox::indeterminate`] rather than painted by hand,
//! because that is what makes the grey state *readable*: egui maps an
//! indeterminate checkbox to `accesskit::Toggled::Mixed`, so it reaches the
//! accessibility tree — a screen reader announces it, and a headless test can
//! assert on it. A hand-drawn dash would be invisible to both.

/// The next state: grey → cleared → set → grey, the order a Windows
/// three-state box cycles in.
///
/// Starting from grey, **one** click clears — which is the commonest use there
/// is, files copied off a CD arriving write-protected. The other order would
/// make that take two clicks.
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

    /// From grey, one click clears — the files-off-a-CD case.
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
