//! The add-operation palette.
//!
//! `docs/DESIGN.md` S2: *"Modal popover with search box; operations grouped
//! under General / Music / Numbers / Advanced, each with icon + one-line
//! description. Enter adds and expands the card. Also reachable via Ctrl+K."*
//!
//! Four groups. Presets is deliberately not one of them — it became the
//! pipeline itself (D8). Operations that are not built yet are
//! listed greyed, naming the milestone that brings them: the groups are the
//! user's mental map of the product, and hiding half of them makes the app look
//! smaller than it is while leaving "when does X arrive" unanswerable.

use ren_core::ops::OpKind;

/// One entry in the catalogue.
pub struct Item {
    pub label: &'static str,
    pub what: &'static str,
    /// Extra words the search should match, beyond the label.
    pub keywords: &'static str,
    /// `None` for an operation that does not exist yet.
    ///
    /// A function pointer rather than a value, which is what lets the whole
    /// catalogue be a `const`.
    pub build: Option<fn() -> OpKind>,
    /// When it arrives, for the ones that have not.
    pub note: &'static str,
}

impl Item {
    pub fn is_ready(&self) -> bool {
        self.build.is_some()
    }
}

pub struct Group {
    pub name: &'static str,
    pub items: &'static [Item],
}

/// Everything the app offers, and everything it is going to.
pub const CATALOGUE: [Group; 4] = [
    Group {
        name: "General",
        items: &[
            Item {
                label: "Replace",
                what: "Find and replace text, with wildcards or a regular expression",
                keywords: "find swap regex wildcard",
                build: Some(|| OpKind::Replace(Default::default())),
                note: "",
            },
            Item {
                label: "Batch Replace",
                what: "Run a whole list of replacements in one go",
                keywords: "list rules corrections contractions",
                build: Some(|| OpKind::BatchReplace(Default::default())),
                note: "",
            },
            Item {
                label: "Set Casing",
                what: "UPPER, lower, Sentence, Title, iNVERT or rANdOm",
                keywords: "case upper lower title capitalise capitalize",
                build: Some(|| OpKind::Casing(Default::default())),
                note: "",
            },
            Item {
                label: "Add / Remove",
                what: "Insert text at a position, or delete characters from one",
                keywords: "insert delete prefix suffix",
                build: Some(|| OpKind::AddRemove(Default::default())),
                note: "",
            },
            Item {
                label: "Move Section",
                what: "Cut part of the name and paste it somewhere else",
                keywords: "cut paste rearrange",
                build: Some(|| OpKind::MoveSection(Default::default())),
                note: "",
            },
            Item {
                label: "Space Trimming",
                what: "Tidy up spaces, and turn underscores back into them",
                keywords: "spaces trim underscore whitespace",
                build: Some(|| OpKind::SpaceTrim(Default::default())),
                note: "",
            },
        ],
    },
    Group {
        name: "Numbers",
        items: &[
            Item {
                label: "Add Counter",
                what: "Number the files, first or last, with a separator",
                keywords: "counter number sequence enumerate",
                build: Some(|| OpKind::AddCounter(Default::default())),
                note: "",
            },
            Item {
                label: "Re-Number",
                what: "Find the numbers already in the name and do arithmetic on them",
                keywords: "renumber add subtract multiply divide round pad",
                build: Some(|| OpKind::ReNumber(Default::default())),
                note: "",
            },
            Item {
                label: "Zero Padding",
                what: "Pad numbers with zeros so they sort properly",
                keywords: "zero pad sort digits",
                build: Some(|| OpKind::ZeroPadding(Default::default())),
                note: "",
            },
        ],
    },
    Group {
        name: "Music",
        items: &[
            Item {
                label: "Music Rename",
                what: "Name files from their artist, title, album and track tags",
                keywords: "mp3 id3 artist title album track",
                build: Some(|| OpKind::MusicRename(Default::default())),
                note: "",
            },
            Item {
                label: "Music Tagger",
                what: "Write tags from the filename",
                keywords: "mp3 id3 write tags parts tagger",
                build: Some(|| OpKind::MusicTagger(Default::default())),
                note: "",
            },
            Item {
                label: "Remove Tags",
                what: "Strip tags out of music files",
                keywords: "mp3 id3 strip remove untag lyrics",
                build: Some(|| OpKind::RemoveTags(Default::default())),
                note: "",
            },
        ],
    },
    Group {
        name: "Advanced",
        items: &[
            Item {
                label: "Free Format",
                what: "Build the whole name from <tags> and literal text",
                keywords: "format template tags pattern",
                build: Some(|| OpKind::FreeFormat(Default::default())),
                note: "",
            },
            Item {
                label: "Set Attributes",
                what: "Change read-only, hidden, system and archive",
                keywords: "attributes readonly hidden system archive",
                build: Some(|| OpKind::SetAttributes(Default::default())),
                note: "",
            },
            Item {
                label: "Set Date & Time",
                what: "Change created, modified and accessed times",
                keywords: "date time stamp created modified exif",
                build: Some(|| OpKind::SetDate(Default::default())),
                note: "",
            },
            Item {
                label: "CSV List Rename",
                what: "Rename from a two-column list of old and new names",
                keywords: "csv list spreadsheet import",
                build: Some(|| OpKind::CsvList(Default::default())),
                note: "",
            },
            Item {
                label: "Filename Editor",
                what: "Type the new names directly, one per line",
                keywords: "editor manual lines text",
                build: Some(|| OpKind::FilenameEditor(Default::default())),
                note: "",
            },
            Item {
                label: "Scripting",
                what: "Rename with a script, for anything the other operations cannot express",
                keywords: "script koto code frs vbscript macro",
                build: Some(|| OpKind::Script(Default::default())),
                note: "",
            },
        ],
    },
];

/// The palette while it is open.
#[derive(Debug, Default)]
pub struct PaletteState {
    pub query: String,
    focused: bool,
}

pub enum Outcome {
    Open,
    Add(OpKind),
    Cancelled,
}

pub fn ui(ctx: &egui::Context, state: &mut PaletteState) -> Outcome {
    let mut outcome = Outcome::Open;

    let modal = egui::Modal::new(egui::Id::new("op_palette")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading("Add operation");
        ui.add_space(4.0);

        let search = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .desired_width(f32::INFINITY)
                .hint_text("Search…")
                .id_salt("palette_search"),
        );
        // Once, not every frame: asking for focus continuously would keep the
        // frame dirty and never settle (D26).
        if !state.focused {
            search.request_focus();
            state.focused = true;
        }

        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let matches = search_matches(&state.query);
        if enter && let Some(build) = matches.first().and_then(|item| item.build) {
            outcome = Outcome::Add(build());
        }

        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("palette_list")
            .max_height(crate::theme::modal_body_height(ctx, 120.0))
            .show(ui, |ui| {
                for group in &CATALOGUE {
                    let visible: Vec<&Item> = group
                        .items
                        .iter()
                        .filter(|item| matches_query(item, &state.query))
                        .collect();
                    if visible.is_empty() {
                        continue;
                    }
                    ui.label(egui::RichText::new(group.name).strong().small());
                    for item in visible {
                        if entry_ui(ui, item)
                            && let Some(build) = item.build
                        {
                            outcome = Outcome::Add(build());
                        }
                    }
                    ui.add_space(4.0);
                }
            });
    });

    if modal.should_close() && matches!(outcome, Outcome::Open) {
        outcome = Outcome::Cancelled;
    }
    outcome
}

/// One row. Returns true when it was chosen.
fn entry_ui(ui: &mut egui::Ui, item: &Item) -> bool {
    if item.is_ready() {
        let response = ui.add(egui::Button::new(item.label).min_size(egui::vec2(120.0, 0.0)));
        ui.label(egui::RichText::new(item.what).weak().small());
        return response.clicked();
    }

    // Not built yet. A plain label rather than a disabled button, so it still
    // reaches the accessibility tree and can be read out.
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(item.label)
                .color(ui.visuals().weak_text_color())
                .strikethrough(),
        );
        ui.label(egui::RichText::new(item.note).weak().small().italics());
    });
    ui.label(egui::RichText::new(item.what).weak().small());
    false
}

fn matches_query(item: &Item, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    query.split_whitespace().all(|word| {
        item.label.to_lowercase().contains(word)
            || item.keywords.contains(word)
            || item.what.to_lowercase().contains(word)
    })
}

/// The operations a query matches, ready ones first — so Enter never picks
/// something that is not built.
fn search_matches(query: &str) -> Vec<&'static Item> {
    let mut items: Vec<&'static Item> = CATALOGUE
        .iter()
        .flat_map(|group| group.items.iter())
        .filter(|item| matches_query(item, query))
        .collect();
    items.sort_by_key(|item| !item.is_ready());
    items
}

/// Every catalogue entry that maps to a real operation.
pub fn implemented() -> Vec<OpKind> {
    CATALOGUE
        .iter()
        .flat_map(|group| group.items.iter())
        .filter_map(|item| item.build.map(|build| build()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue is hand-written, so it can drift from the engine. This is
    /// the guard — the same job `menu_tags` does for the tag picker.
    #[test]
    fn every_ready_entry_is_a_real_operation_and_every_one_is_listed() {
        let mut offered: Vec<&'static str> = implemented().iter().map(|op| op.name()).collect();
        offered.sort_unstable();
        let before = offered.len();
        offered.dedup();
        assert_eq!(offered.len(), before, "an operation is listed twice");

        let mut known: Vec<&'static str> = OpKind::all().iter().map(|op| op.name()).collect();
        known.sort_unstable();
        assert_eq!(offered, known, "the palette and the engine disagree");
    }

    /// When an operation lands, this fails until its greyed entry is turned
    /// live — which is the point. It caught both of M5's list operations.
    #[test]
    fn nothing_is_listed_as_unbuilt_that_has_since_been_built() {
        let built: Vec<&'static str> = OpKind::all().iter().map(|op| op.label()).collect();
        for group in &CATALOGUE {
            for item in group.items.iter().filter(|i| !i.is_ready()) {
                assert!(
                    !built.contains(&item.label),
                    "{} is built now — take it out of the 'coming later' list",
                    item.label
                );
                assert!(
                    !item.note.is_empty(),
                    "{} should say when it arrives",
                    item.label
                );
            }
        }
    }

    #[test]
    fn the_catalogue_has_exactly_four_groups_in_a_fixed_order() {
        let names: Vec<&str> = CATALOGUE.iter().map(|g| g.name).collect();
        assert_eq!(names, ["General", "Numbers", "Music", "Advanced"]);
    }

    #[test]
    fn search_matches_a_label_a_keyword_or_a_description() {
        let names = |query: &str| -> Vec<&'static str> {
            search_matches(query).iter().map(|i| i.label).collect()
        };

        assert_eq!(names("zero"), ["Zero Padding"]);
        assert_eq!(names("capitalise"), ["Set Casing"], "by keyword");
        assert!(
            names("regular expression").contains(&"Replace"),
            "by description"
        );
        assert!(names("").len() > 10, "an empty query offers everything");
        assert!(names("nothing at all matches this").is_empty());
    }

    /// Enter adds the first match, so a query that matches something unbuilt
    /// must never put it first.
    #[test]
    fn a_search_puts_the_operations_that_exist_first() {
        let matches = search_matches("tags");
        assert!(!matches.is_empty());
        assert!(
            matches[0].is_ready(),
            "the first match was {}, which is not built",
            matches[0].label
        );
    }
}
