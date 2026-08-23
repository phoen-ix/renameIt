//! Counting things out loud.

/// `n` of `singular`, pluralised — `1 item`, `2 items`, `0 items`.
///
/// The app used to write `item(s)` everywhere, in the status bar, the
/// confirmation dialog, the undo log and the CLI. **D81** fixes *item* as the
/// noun a count is counted in and says nothing about the brackets, which were
/// only ever there because the number is not known until it is formatted — and
/// it is known here.
///
/// English-only, and deliberately: the app ships one language, so a plural
/// rule is a formatting concern rather than an i18n one. A word whose plural
/// is not `+s` does not go through here.
pub fn plural(n: usize, singular: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {singular}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_is_singular_and_everything_else_is_not() {
        assert_eq!(plural(0, "item"), "0 items");
        assert_eq!(plural(1, "item"), "1 item");
        assert_eq!(plural(2, "item"), "2 items");
        // Zero is plural in English, which is the case a naive `n > 1` gets
        // wrong and the one a status bar shows most often.
        assert_eq!(plural(0, "conflict"), "0 conflicts");
    }
}
