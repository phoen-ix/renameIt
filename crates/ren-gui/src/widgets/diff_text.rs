//! Showing what changed in a name.
//!
//! `docs/DESIGN.md` Part 2 §3 (S3): *"deleted spans struck/red-tinted, inserted
//! spans green-tinted"* — with one narrowing, explained on [`new_name`]: the
//! deleted span is shown only when nothing was inserted in its place. Names are
//! short and almost every edit is one contiguous change, so trimming the
//! common prefix and suffix says everything a character-level diff would — for
//! a fraction of the work, and this runs for every visible row of every frame.

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

/// Renders the *new* name with what changed marked.
///
/// The inserted stretch is green. A rename that only **deletes** —
/// `song_v2.mp3` → `song.mp3` — has nothing inserted to colour, and used to be
/// drawn as a plain name indistinguishable from one the run leaves alone; so
/// then the deleted stretch is shown where it was, struck through in red. Only
/// then: shown beside an insertion as well, it would double the cell for the
/// commonest edit there is, a replacement.
///
/// The accessible name is the new name alone. A struck-through stretch is
/// paint, and a screen reader reading `song_v2.mp3` as the name the run will
/// write would be reading the one name it will not.
///
/// The font follows the `Ui`'s text-style override, as a plain label does —
/// the grid draws this line at `Small`, and a `LayoutJob` with a font of its
/// own ignores the override.
pub fn new_name(ui: &mut egui::Ui, old: &str, new: &str, style: DiffStyle) {
    let font = ui
        .style()
        .override_text_style
        .clone()
        .unwrap_or(egui::TextStyle::Body)
        .resolve(ui.style());
    let response = ui.label(job(old, new, style, &font));
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), new));
}

/// The sections [`new_name`] draws, apart from any `Ui` so they are testable.
fn job(old: &str, new: &str, style: DiffStyle, font: &egui::FontId) -> egui::text::LayoutJob {
    let diff = diff(old, new);
    let mut job = egui::text::LayoutJob::default();
    let mut push = |text: &str, color: egui::Color32, struck: bool| {
        if !text.is_empty() {
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color,
                    strikethrough: if struck {
                        egui::Stroke::new(1.0, color)
                    } else {
                        egui::Stroke::NONE
                    },
                    ..Default::default()
                },
            );
        }
    };
    push(diff.prefix, style.unchanged, false);
    if diff.added.is_empty() {
        push(diff.removed, style.removed, true);
    } else {
        push(diff.added, style.added, false);
    }
    push(diff.suffix, style.unchanged, false);
    job
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

    fn style() -> DiffStyle {
        DiffStyle {
            removed: egui::Color32::RED,
            added: egui::Color32::GREEN,
            unchanged: egui::Color32::GRAY,
        }
    }

    fn sections(old: &str, new: &str) -> Vec<(String, egui::Color32, bool)> {
        let job = job(old, new, style(), &egui::FontId::proportional(14.0));
        job.sections
            .iter()
            .map(|s| {
                (
                    job.text[s.byte_range.start.0..s.byte_range.end.0].to_owned(),
                    s.format.color,
                    s.format.strikethrough != egui::Stroke::NONE,
                )
            })
            .collect()
    }

    /// A rename that only deletes has nothing inserted to colour. The deleted
    /// stretch is shown struck through, so the cell does not read as a name
    /// the run leaves alone.
    #[test]
    fn a_deletion_shows_what_went_struck_through() {
        assert_eq!(
            sections("song_v2.mp3", "song.mp3"),
            [
                ("song".to_owned(), egui::Color32::GRAY, false),
                ("_v2".to_owned(), egui::Color32::RED, true),
                (".mp3".to_owned(), egui::Color32::GRAY, false),
            ]
        );
    }

    /// Beside an insertion the deleted text is left out, or a replacement —
    /// the commonest edit — would draw both names in one cell.
    #[test]
    fn a_replacement_shows_only_what_arrived() {
        assert_eq!(
            sections("my_song.mp3", "my song.mp3"),
            [
                ("my".to_owned(), egui::Color32::GRAY, false),
                (" ".to_owned(), egui::Color32::GREEN, false),
                ("song.mp3".to_owned(), egui::Color32::GRAY, false),
            ]
        );
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
