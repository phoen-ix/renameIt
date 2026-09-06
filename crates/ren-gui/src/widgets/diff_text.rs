//! Showing what changed in a name.
//!
//! `docs/DESIGN.md` Part 2 §3 (S3): *"deleted spans struck/red-tinted, inserted
//! spans green-tinted"*. Names are short and almost every edit is one
//! contiguous change, so trimming the common prefix and suffix says everything
//! a character-level diff would — for a fraction of the work, and this runs for
//! every visible row of every frame.

/// A name split into the part that survived and the part that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diff<'a> {
    pub prefix: &'a str,
    /// The stretch of the old name that went away.
    pub removed: &'a str,
    /// The stretch of the new name that took its place.
    pub added: &'a str,
    pub suffix: &'a str,
}

impl Diff<'_> {
    #[cfg(test)]
    pub fn is_unchanged(&self) -> bool {
        self.removed.is_empty() && self.added.is_empty()
    }
}

/// Splits `old` and `new` at their common prefix and suffix.
pub fn diff<'a>(old: &'a str, new: &'a str) -> Diff<'a> {
    let prefix_len = common_prefix(old, new);
    // The suffix search starts after the prefix in *both* strings, so the two
    // can never overlap — "aa" -> "aaa" must not claim two 'a's twice.
    let suffix_len = common_suffix(&old[prefix_len..], &new[prefix_len..]);

    Diff {
        prefix: &old[..prefix_len],
        removed: &old[prefix_len..old.len() - suffix_len],
        added: &new[prefix_len..new.len() - suffix_len],
        suffix: &old[old.len() - suffix_len..],
    }
}

/// Byte length of the longest shared prefix, on a character boundary.
fn common_prefix(a: &str, b: &str) -> usize {
    let mut len = 0;
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            break;
        }
        len += ca.len_utf8();
    }
    len
}

/// Byte length of the longest shared suffix, on a character boundary.
fn common_suffix(a: &str, b: &str) -> usize {
    let mut len = 0;
    for (ca, cb) in a.chars().rev().zip(b.chars().rev()) {
        if ca != cb {
            break;
        }
        len += ca.len_utf8();
    }
    len
}

/// Colours for the two halves of a diff, resolved against the current theme.
#[derive(Debug, Clone, Copy)]
pub struct DiffStyle {
    pub removed: egui::Color32,
    pub added: egui::Color32,
    pub unchanged: egui::Color32,
}

impl DiffStyle {
    pub fn for_ui(ui: &egui::Ui) -> Self {
        let dark = ui.visuals().dark_mode;
        Self {
            removed: if dark {
                egui::Color32::from_rgb(0xef, 0x9a, 0x9a)
            } else {
                egui::Color32::from_rgb(0xc6, 0x28, 0x28)
            },
            added: if dark {
                egui::Color32::from_rgb(0xa5, 0xd6, 0xa7)
            } else {
                egui::Color32::from_rgb(0x2e, 0x7d, 0x32)
            },
            unchanged: ui.visuals().text_color(),
        }
    }
}

/// Renders the *new* name with its inserted stretch highlighted.
pub fn new_name(ui: &mut egui::Ui, old: &str, new: &str, style: DiffStyle) {
    let diff = diff(old, new);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();

    let mut push = |text: &str, color: egui::Color32| {
        if !text.is_empty() {
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color,
                    ..Default::default()
                },
            );
        }
    };
    push(diff.prefix, style.unchanged);
    push(diff.added, style.added);
    push(diff.suffix, style.unchanged);

    ui.label(job);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identical_pair_has_nothing_to_highlight() {
        let d = diff("song.mp3", "song.mp3");
        assert!(d.is_unchanged());
        assert_eq!(d.prefix, "song.mp3");
    }

    #[test]
    fn a_middle_edit_keeps_the_ends() {
        let d = diff("my_song.mp3", "my song.mp3");
        assert_eq!(d.prefix, "my");
        assert_eq!(d.removed, "_");
        assert_eq!(d.added, " ");
        assert_eq!(d.suffix, "song.mp3");
    }

    #[test]
    fn an_insertion_removes_nothing() {
        let d = diff("song.mp3", "song_v2.mp3");
        assert_eq!(d.prefix, "song");
        assert_eq!(d.removed, "");
        assert_eq!(d.added, "_v2");
        assert_eq!(d.suffix, ".mp3");
    }

    #[test]
    fn a_deletion_adds_nothing() {
        let d = diff("song_v2.mp3", "song.mp3");
        assert_eq!(d.removed, "_v2");
        assert_eq!(d.added, "");
    }

    /// Prefix and suffix must not both claim the same characters.
    #[test]
    fn repeated_characters_do_not_get_counted_twice() {
        let d = diff("aa", "aaa");
        assert_eq!(d.prefix.len() + d.removed.len() + d.suffix.len(), 2);
        assert_eq!(d.prefix.len() + d.added.len() + d.suffix.len(), 3);
        assert_eq!(d.added, "a");
    }

    #[test]
    fn a_complete_replacement_highlights_everything() {
        let d = diff("abc", "xyz");
        assert_eq!(d.prefix, "");
        assert_eq!(d.removed, "abc");
        assert_eq!(d.added, "xyz");
        assert_eq!(d.suffix, "");
    }

    #[test]
    fn multi_byte_characters_split_on_boundaries() {
        let d = diff("Ünïcödé.txt", "Ünïcödé Träck.txt");
        assert_eq!(d.prefix, "Ünïcödé");
        assert_eq!(d.added, " Träck");
        assert_eq!(d.suffix, ".txt");

        // Every field must still be a valid str — this panics if not.
        let _ = format!("{}{}{}{}", d.prefix, d.removed, d.added, d.suffix);
    }

    #[test]
    fn an_emoji_change_is_not_split_mid_character() {
        let d = diff("track 🎵.mp3", "track 🎸.mp3");
        assert_eq!(d.prefix, "track ");
        assert_eq!(d.removed, "🎵");
        assert_eq!(d.added, "🎸");
        assert_eq!(d.suffix, ".mp3");
    }

    #[test]
    fn an_empty_new_name_removes_everything() {
        let d = diff("gone.txt", "");
        assert_eq!(d.removed, "gone.txt");
        assert_eq!(d.added, "");
    }

    /// Reassembling the two halves must give back exactly what went in — the
    /// property that keeps the table from lying about what will happen.
    #[test]
    fn the_spans_always_reassemble_into_the_originals() {
        for (old, new) in [
            ("song.mp3", "Song.mp3"),
            ("a", "b"),
            ("", "new.txt"),
            ("same", "same"),
            ("aaa", "aa"),
            ("Ünïcödé", "ÜNÏCÖDÉ"),
        ] {
            let d = diff(old, new);
            assert_eq!(format!("{}{}{}", d.prefix, d.removed, d.suffix), old);
            assert_eq!(format!("{}{}{}", d.prefix, d.added, d.suffix), new);
        }
    }
}
