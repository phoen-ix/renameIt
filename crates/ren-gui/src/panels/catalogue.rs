//! Every operation the app offers, grouped, and the drop-down that picks one.
//!
//! The drop-down sits at the top of each pipeline card, where it switches the
//! card's operation, and on Settings ▸ Startup, where it picks what a new card
//! starts as. It replaced the modal add-operation palette, whose scrolling
//! popup stood between the user and the one thing *+ Add operation* nearly
//! always meant: another Replace.
//!
//! Four groups. Presets is deliberately not one of them — it became the
//! pipeline itself (D8). Every entry is an operation that exists (P112).

use ren_core::ops::OpKind;

/// One entry in the catalogue.
pub struct Item {
    pub label: &'static str,
    pub what: &'static str,
    /// A function pointer rather than a value, which is what lets the whole
    /// catalogue be a `const`.
    pub build: fn() -> OpKind,
}

pub struct Group {
    pub name: &'static str,
    pub items: &'static [Item],
}

/// Everything the app offers.
pub const CATALOGUE: [Group; 4] = [
    Group {
        name: "General",
        items: &[
            Item {
                label: "Replace",
                what: "Find and replace text, with wildcards or a regular expression",
                build: || OpKind::Replace(Default::default()),
            },
            Item {
                label: "Batch Replace",
                what: "Run a whole list of replacements in one go",
                build: || OpKind::BatchReplace(Default::default()),
            },
            Item {
                label: "Set Casing",
                what: "UPPER, lower, Sentence, Title, iNVERT or rANdOm",
                build: || OpKind::Casing(Default::default()),
            },
            Item {
                label: "Add / Remove",
                what: "Insert text at a position, or delete characters from one",
                build: || OpKind::AddRemove(Default::default()),
            },
            Item {
                label: "Move Section",
                what: "Cut part of the name and paste it somewhere else",
                build: || OpKind::MoveSection(Default::default()),
            },
            Item {
                label: "Space Trimming",
                what: "Tidy up spaces, and turn underscores back into them",
                build: || OpKind::SpaceTrim(Default::default()),
            },
        ],
    },
    Group {
        name: "Numbers",
        items: &[
            Item {
                label: "Add Counter",
                what: "Number the files, first or last, with a separator",
                build: || OpKind::AddCounter(Default::default()),
            },
            Item {
                label: "Re-Number",
                what: "Find the numbers already in the name and do arithmetic on them",
                build: || OpKind::ReNumber(Default::default()),
            },
            Item {
                label: "Zero Padding",
                what: "Pad numbers with zeros so they sort properly",
                build: || OpKind::ZeroPadding(Default::default()),
            },
        ],
    },
    Group {
        name: "Music",
        items: &[
            Item {
                label: "Music Rename",
                what: "Name files from their artist, title, album and track tags",
                build: || OpKind::MusicRename(Default::default()),
            },
            Item {
                label: "Music Tagger",
                what: "Write tags from the filename",
                build: || OpKind::MusicTagger(Default::default()),
            },
            Item {
                label: "Remove Tags",
                what: "Strip tags out of music files",
                build: || OpKind::RemoveTags(Default::default()),
            },
        ],
    },
    Group {
        name: "Advanced",
        items: &[
            Item {
                label: "Free Format",
                what: "Build the whole name from <tags> and literal text",
                build: || OpKind::FreeFormat(Default::default()),
            },
            Item {
                label: "Set Attributes",
                what: "Change read-only, hidden, system and archive",
                build: || OpKind::SetAttributes(Default::default()),
            },
            Item {
                label: "Set Date & Time",
                what: "Change created, modified and accessed times",
                build: || OpKind::SetDate(Default::default()),
            },
            Item {
                label: "CSV List Rename",
                what: "Rename from a two-column list of old and new names",
                build: || OpKind::CsvList(Default::default()),
            },
            Item {
                label: "Filename Editor",
                what: "Type the new names directly, one per line",
                build: || OpKind::FilenameEditor(Default::default()),
            },
            Item {
                label: "Scripting",
                what: "Rename with a script, for anything the other operations cannot express",
                build: || OpKind::Script(Default::default()),
            },
        ],
    },
];

/// A drop-down of every operation, grouped, showing `current`'s label and
/// named `name` for a screen reader. Returns the one picked, if it is not
/// `current`'s kind.
///
/// A button and a `Popup` rather than an `egui::ComboBox`: the combo box puts
/// its list in a `ScrollArea`, and egui sizes a new popup within
/// `default_area_size` (400 pt), so eighteen entries came up in a scrolling
/// box — the very thing the old palette was disliked for. With no scroll area
/// the popup grows to fit them all.
pub fn picker(ui: &mut egui::Ui, name: &str, current: &OpKind) -> Option<OpKind> {
    let label = current.label();
    let button = ui.button(format!("{label} ▾"));
    button.widget_info(|| {
        let mut info = egui::WidgetInfo::new(egui::Role::ComboBox);
        info.current_text_value = Some(label.to_owned());
        info
    });
    let button = button.accessible_name(name);

    let mut picked = None;
    egui::Popup::menu(&button).show(|ui| {
        for group in &CATALOGUE {
            ui.label(egui::RichText::new(group.name).strong().small());
            for item in group.items {
                let selected = item.label == label;
                if ui
                    .selectable_label(selected, item.label)
                    .on_hover_text(item.what)
                    .clicked()
                    && !selected
                {
                    picked = Some((item.build)());
                }
            }
        }
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue is hand-written, so it can drift from the engine. This is
    /// the guard — the same job `menu_tags` does for the tag picker: every
    /// operation the engine has is offered, once.
    #[test]
    fn every_operation_is_listed_exactly_once() {
        let mut offered: Vec<&'static str> = CATALOGUE
            .iter()
            .flat_map(|group| group.items.iter())
            .map(|item| (item.build)().name())
            .collect();
        offered.sort_unstable();
        let before = offered.len();
        offered.dedup();
        assert_eq!(offered.len(), before, "an operation is listed twice");

        let mut known: Vec<&'static str> = OpKind::all().iter().map(|op| op.name()).collect();
        known.sort_unstable();
        assert_eq!(offered, known, "the catalogue and the engine disagree");
    }

    #[test]
    fn the_catalogue_has_exactly_four_groups_in_a_fixed_order() {
        let names: Vec<&str> = CATALOGUE.iter().map(|g| g.name).collect();
        assert_eq!(names, ["General", "Numbers", "Music", "Advanced"]);
    }

    /// `picker` marks the current entry by label, so a catalogue label that
    /// differed from the operation's own would never show as selected.
    #[test]
    fn every_entry_is_labelled_as_its_operation_is() {
        for item in CATALOGUE.iter().flat_map(|group| group.items.iter()) {
            assert_eq!(item.label, (item.build)().label());
        }
    }
}
