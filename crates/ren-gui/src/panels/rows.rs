//! What a row *means*, apart from how it is drawn.
//!
//! Lifted out of [`super::file_table`] when the thumbnail grid arrived, because
//! a second view of the same listing has to agree with the first about every
//! one of these — which rows the filter hides, whether a row is dimmed, what a
//! click does to the selection, what the right-click menu offers, and what the
//! "New name" cell says. Two copies of that would drift, and the drift would be
//! silent: nobody reports that Ctrl-click deselects in the list and not in the
//! grid, they just stop using the grid.
//!
//! [`cell_of`] is the sharp one. Its own comment already records why: dimming
//! must read the *classified cell* and not `RowState`, because
//! `RowState::Unchanged` is only about the name, and a row the run is about to
//! act on would otherwise be greyed out as untouched at the very moment it
//! matters. A grid that classified rows for itself would be one `matches!` away
//! from shipping that bug a second time.

use ren_core::model::FileEntry;
use ren_core::{ConflictKind, Plan, PlanItem, RowState};

use crate::viewmodel::RowFilter;
use crate::widgets::diff_text::{self, DiffStyle};

/// The plan item for one entry, if this run covers it.
pub(crate) fn item_of<'a>(
    plan: Option<&'a Plan>,
    plan_index: &[Option<usize>],
    entry_index: usize,
) -> Option<&'a PlanItem> {
    plan?.items.get((*plan_index.get(entry_index)?)?)
}

/// Which entries a view shows, in listing order.
///
/// Both views filter through this, which is what makes a hidden row a hidden
/// *tile* as well. A view that iterated `entries` directly would quietly ignore
/// the Changed chip.
pub(crate) fn visible_rows(
    count: usize,
    plan: Option<&Plan>,
    plan_index: &[Option<usize>],
    row_filter: RowFilter,
) -> Vec<usize> {
    (0..count)
        .filter(|&i| match item_of(plan, plan_index, i) {
            Some(item) => row_filter.accepts(item),
            // A row with no plan yet is only hidden by a filter that is
            // explicitly asking for something.
            None => row_filter == RowFilter::All,
        })
        .collect()
}

/// What the "New name" column says about one row.
///
/// A pure classifier so every shape is testable without a frame — and so `dim`
/// reads *this* rather than `RowState`. That is the fix for the sharpest silent
/// bug M5 could have shipped: `RowState::Unchanged` is only about the name, so
/// a Set Date row would have been greyed out as untouched at the very moment
/// the run was about to change it.
#[derive(Debug, PartialEq)]
pub(crate) enum Cell<'a> {
    /// No plan for this row: it is outside the current run.
    Outside,
    Untouched,
    Renamed {
        new: &'a str,
    },
    Acted {
        what: String,
    },
    Both {
        new: &'a str,
        what: String,
    },
    Conflict {
        kind: &'a ConflictKind,
        new: &'a str,
    },
    Error(&'a str),
}

impl Cell<'_> {
    /// Whether the run leaves this row alone, so it is drawn faded.
    ///
    /// One function rather than a `matches!` at each drawing site: the list and
    /// the grid have to agree, and the rule they have to agree on is the one
    /// this whole type exists to get right.
    pub(crate) fn is_dimmed(&self) -> bool {
        matches!(self, Self::Untouched | Self::Outside)
    }
}

pub(crate) fn cell_of<'a>(item: Option<&'a PlanItem>) -> Cell<'a> {
    let Some(item) = item else {
        return Cell::Outside;
    };
    match &item.state {
        // A conflicting or errored row shows only that. P4 blocks the whole
        // run, so advertising an action that will not happen is a lie.
        RowState::Conflict(kind) => Cell::Conflict {
            kind,
            new: &item.new_name,
        },
        RowState::Error(message) => Cell::Error(message),
        state => {
            let what = describe_actions(item);
            match (state.is_changed(), what) {
                (true, None) => Cell::Renamed {
                    new: &item.new_name,
                },
                (true, Some(what)) => Cell::Both {
                    new: &item.new_name,
                    what,
                },
                (false, Some(what)) => Cell::Acted { what },
                (false, None) => Cell::Untouched,
            }
        }
    }
}

fn describe_actions(item: &PlanItem) -> Option<String> {
    if item.actions.is_empty() {
        return None;
    }
    Some(
        item.actions
            .iter()
            .map(|a| a.describe.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// What a click on a row does to the selection.
///
/// What the right-click menu asked the app to do.
///
/// A request rather than an action, like `sort_request` beside it: the view has
/// the row under the cursor and nothing else — not the platform, not the
/// session, not the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAction {
    /// Open the inline editor, as F2 and a double-click do.
    Rename(usize),
    /// *"show in Explorer"* (DESIGN §S3).
    Reveal(usize),
    /// *"Right click on one or more files in file browser mode, and choose
    /// 'add to free select'"*.
    AddToFreeSelect(Vec<usize>),
    Copy {
        what: CopyWhat,
        rows: Vec<usize>,
    },
}

/// Which text *Copy to Clipboard* puts there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyWhat {
    Names,
    /// *"Copy to Clipboard ▸ All Previews"* — the worked example
    /// for getting a listing out of the app and into a text editor.
    NewNames,
    Paths,
    /// Both columns, tab separated, which is what a spreadsheet wants.
    Both,
}

/// The right-click menu, drawn wherever a row is.
pub(crate) fn row_menu(ui: &mut egui::Ui, index: usize, selected: &[usize]) -> Option<RowAction> {
    let count = selected.len().max(1);
    let mut asked: Option<RowAction> = None;

    if ui.button("Rename…").clicked() {
        asked = Some(RowAction::Rename(index));
    }
    if ui.button("Show in file manager").clicked() {
        asked = Some(RowAction::Reveal(index));
    }

    ui.separator();
    ui.menu_button("Copy to clipboard", |ui| {
        for (label, what) in [
            ("Names", CopyWhat::Names),
            ("New names", CopyWhat::NewNames),
            ("Full paths", CopyWhat::Paths),
            ("Name and new name", CopyWhat::Both),
        ] {
            if ui.button(label).clicked() {
                asked = Some(RowAction::Copy {
                    what,
                    rows: selected.to_vec(),
                });
                ui.close();
            }
        }
    })
    .response
    .on_hover_text(format!("{count} row(s) — the selection, or all of them"));

    ui.separator();
    let free_select = ui.button("Add to Free Select");
    if free_select.clicked() {
        asked = Some(RowAction::AddToFreeSelect(selected.to_vec()));
    }
    free_select.on_hover_text(
        "Move these rows into Free Select, where a run can mix files from different folders",
    );

    if asked.is_some() {
        ui.close();
    }
    asked
}

/// What happened to an open inline-rename editor this frame.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RenameEdit {
    /// Still being typed into.
    Editing,
    /// Enter, with the text as it stood.
    Confirmed,
    /// Escape.
    Cancelled,
}

/// F2's editor: which entry, the text, and whether the caret has been placed.
///
/// A struct rather than a `(usize, String)`, because the seed has to happen
/// exactly **once** and two loose values that must agree is the shape this
/// codebase avoids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineRename {
    pub index: usize,
    pub text: String,
    /// Cleared the first frame the box actually has the keyboard.
    seed: bool,
}

impl InlineRename {
    pub fn opening(index: usize, name: &str) -> Self {
        Self {
            index,
            text: name.to_owned(),
            seed: true,
        }
    }
}

/// The field's id. **Fixed, not a salt.**
///
/// A salt is combined with the cell's `Ui` id, which `egui_table` derives from
/// the *visible row number* — so the id moved whenever the list was scrolled or
/// filtered, and there was nothing stable to hand `TextEdit::store_state`. Only
/// one of these is ever on screen: `inline_rename` is an `Option`, and the list
/// and the grid never draw at once.
pub(crate) fn inline_rename_id() -> egui::Id {
    egui::Id::new("inline_rename")
}

/// How many **characters** of a name the editor selects.
///
/// Characters, not bytes: `CCursor` counts characters and `split_file_name`
/// returns byte slices, so `日本.txt` is six bytes of stem and two characters
/// of it.
pub(crate) fn stem_chars(name: &str) -> usize {
    ren_core::split_file_name(name).0.chars().count()
}

/// F2's text box, drawn in place of a name.
///
/// > *"It will figure out where the filename ends and the extension begins, and
/// > place the cursor there."*
///
/// The stem is **selected**, with the caret at that boundary —
/// `CCursorRange::two(0, boundary)` puts the primary cursor on the second end,
/// so this is literally the documented behaviour *and* what Explorer does. A bare
/// caret would mean typing gives `namexxx.ext`; selecting the stem means typing
/// replaces it, which is what anyone's hands expect.
pub(crate) fn inline_rename_field(ui: &mut egui::Ui, edit: &mut InlineRename) -> RenameEdit {
    let id = inline_rename_id();

    // Seeded on the frame the box **has** the keyboard, not the frame it first
    // appears on: an unfocused `TextEdit` collapses a stored range to its
    // primary cursor before it draws, so a selection seeded a frame early
    // arrives as a bare caret.
    //
    // And seeded **once**. The field re-requests focus every frame, so a
    // per-frame seed would pin the selection and the caret could never be
    // moved — the box would be unusable.
    if edit.seed && ui.memory(|m| m.has_focus(id)) {
        edit.seed = false;
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(stem_chars(&edit.text)),
            )));
        egui::TextEdit::store_state(ui.ctx(), id, state);
    }

    let response = ui.add(
        egui::TextEdit::singleline(&mut edit.text)
            .desired_width(f32::INFINITY)
            .id(id),
    );

    // **Asked for once, not every frame — and that was a defect, not a tidy-up.**
    //
    // egui *surrenders* focus when Enter is pressed in a single-line box, which
    // is what `lost_focus()` reads. Re-requesting focus straight after the
    // `add` put it back inside the same frame, so `lost_focus()` was never true
    // and **Enter never committed a rename**. The documented behaviour — *"you
    // can enter a new name and press enter to rename the file"* — did not work,
    // and nothing caught it because no test drove the F2 key path.
    //
    // It is also what `palette.rs` warns about for its own field: asking
    // continuously keeps the frame dirty and never settles (D26).
    if !response.has_focus() && edit.seed {
        response.request_focus();
    }

    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        RenameEdit::Confirmed
    } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        RenameEdit::Cancelled
    } else {
        RenameEdit::Editing
    }
}

/// D38: this column means "what this run does to this row", so it carries the
/// action as well as the name — and both at once when a pipeline does both.
pub(crate) fn new_name_cell(ui: &mut egui::Ui, entry: &FileEntry, cell: Cell<'_>) {
    match cell {
        Cell::Outside => {
            ui.label(egui::RichText::new("—").weak());
        }
        Cell::Untouched if entry.name_is_lossy => {
            // Not "unchanged" — it was never a candidate. The engine refuses to
            // build a new name out of one it could not read, because the U+FFFD
            // it would carry is not what is on the disk. Saying "unchanged"
            // here would read as "the pipeline had no effect", which is a
            // different and much less actionable fact.
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("⚠").color(egui::Color32::from_rgb(0xff, 0xb3, 0x00)))
                    .on_hover_text(LOSSY_NAME_HELP);
                ui.label(egui::RichText::new("name is not text").weak().italics())
                    .on_hover_text(LOSSY_NAME_HELP);
            });
        }
        Cell::Untouched => {
            ui.label(egui::RichText::new("unchanged").weak().italics());
        }
        Cell::Renamed { new } => renamed(ui, entry, new),
        Cell::Acted { what } => {
            ui.label(what);
        }
        Cell::Both { new, what } => {
            ui.horizontal(|ui| {
                renamed(ui, entry, new);
                ui.label(egui::RichText::new("•").weak());
                ui.label(egui::RichText::new(what).weak());
            });
        }
        Cell::Conflict { kind, new } => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("⛔").color(egui::Color32::from_rgb(0xe5, 0x73, 0x73)),
                )
                .on_hover_text(conflict_help(kind));
                ui.label(egui::RichText::new(new).strikethrough());
            });
        }
        Cell::Error(message) => {
            ui.label(
                egui::RichText::new("✖ error").color(egui::Color32::from_rgb(0xe5, 0x73, 0x73)),
            )
            .on_hover_text(message.to_owned());
        }
    }
}

pub(crate) fn renamed(ui: &mut egui::Ui, entry: &FileEntry, new: &str) {
    let style = DiffStyle::for_ui(ui);
    ui.horizontal(|ui| {
        diff_text::new_name(ui, &entry.file_name, new, style);
        // The "warn about changing extensions" setting, shown inline instead
        // of as a dialog after the fact.
        if extension_changed(
            &entry.file_name,
            new,
            ren_platform::host().naming_rules(&entry.path),
        ) {
            ui.label(egui::RichText::new("⚠").color(egui::Color32::from_rgb(0xff, 0xb3, 0x00)))
                .on_hover_text("The file extension changes");
        }
    });
}

/// Why a row was left alone, and what to do about it.
///
/// The way out is the point: the file's *path* is byte-exact even though its
/// name could not be read as text, so renaming it directly is completely safe —
/// and that is the whole reason such a row is still listed rather than dropped.
pub(crate) const LOSSY_NAME_HELP: &str = "This name is not valid Unicode, so no new name can \
     be built from it — a pipeline would write \u{FFFD} over whatever is really there.\n\
     Press F2 to give it a name directly. That is safe, and it can be undone.";

/// The tooltip a conflict badge shows. P4 makes conflicts a hard block, so the
/// UI owes the user a reason and a way out.
pub(crate) fn conflict_help(kind: &ConflictKind) -> String {
    match kind {
        ConflictKind::DuplicateTarget { others } => format!(
            "{kind}.\n{} want this name. Change the operation, or select fewer files.",
            ren_core::plural(others.len(), "other item")
        ),
        ConflictKind::TargetExists => {
            format!("{kind}.\nRenaming onto it would destroy the existing file.")
        }
        ConflictKind::Unsupported { .. } => format!(
            "{kind}.\nThis machine cannot make that change. Untick it, or run this on Windows."
        ),
        ConflictKind::BlockedByFile { .. } => {
            format!("{kind}.\nRename or move that file first, or send this one somewhere else.")
        }
        // The way out is on *this* row, which is why the message says so: the
        // other row is doing something legal, and the folder it needs is the
        // one thing in the run that cannot move.
        ConflictKind::NeededAsFolder { .. } => format!(
            "{kind}.\nAnother row in this run moves into that folder. Give this file a \
             different name, or untick one of the two."
        ),
        ConflictKind::IntoItself => format!(
            "{kind}.\nThe subfolder <\\> asks for is this folder. Give the subfolder a \
             different name, or untick Folders."
        ),
        ConflictKind::InvalidName(_) => format!("{kind}."),
        ConflictKind::UnresolvedCycle => format!("{kind}."),
    }
}

/// Whether the extension the user sees is genuinely a different one.
///
/// Folded through the volume's own rules, which is what makes `SONG.MP3` →
/// `song.mp3` *not* a warning on Windows: the filesystem cannot tell those two
/// extensions apart, so telling the user their extension changed is a false
/// alarm on the single most common thing a Set Casing card does. On a
/// case-sensitive volume the same rename really is a change, and still warns.
pub(crate) fn extension_changed(old: &str, new: &str, rules: &ren_platform::NamingRules) -> bool {
    // Folded *inside* the Option, so "gained an extension" and "lost one" stay
    // changes — only the text is compared case-insensitively.
    let extension = |name: &str| ren_core::split_file_name(name).1.map(|ext| rules.fold(ext));
    extension(old) != extension(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// > *"It will figure out where the filename ends and the extension
    /// > begins, and place the cursor there."*
    ///
    /// **Characters, not bytes.** `CCursor` counts characters; `split_file_name`
    /// returns byte slices. The Japanese case is the only one that catches a
    /// `.len()` here, which is why it is in the list.
    #[test]
    fn the_editor_selects_the_name_and_leaves_the_extension() {
        assert_eq!(stem_chars("song.mp3"), 4);
        assert_eq!(stem_chars("README"), 6, "no extension: the whole name");
        assert_eq!(stem_chars(".gitignore"), 10, "a leading dot is not one");
        assert_eq!(stem_chars("a.b.c"), 3, "the *last* period");
        assert_eq!(stem_chars("日本.txt"), 2, "six bytes, two characters");
    }

    /// A case-only extension change is **not** a change on a volume that cannot
    /// tell the two apart — and Set Casing does exactly that all day.
    #[test]
    fn a_case_only_extension_change_only_warns_where_it_is_real() {
        let insensitive = ren_platform::WINDOWS;
        let sensitive = ren_platform::POSIX;
        assert!(!extension_changed("SONG.MP3", "song.mp3", &insensitive));
        assert!(extension_changed("SONG.MP3", "song.mp3", &sensitive));
        // A real change is still a real change on either.
        assert!(extension_changed("song.mp3", "song.txt", &insensitive));
        assert!(extension_changed("song.mp3", "song.txt", &sensitive));
    }

    #[test]
    fn an_extension_change_is_detected() {
        let rules = ren_platform::POSIX;
        let extension_changed = |a: &str, b: &str| extension_changed(a, b, &rules);
        assert!(extension_changed("song.mp3", "song.txt"));
        assert!(extension_changed("song.mp3", "song"));
        assert!(!extension_changed("song.mp3", "Song.mp3"));
        assert!(!extension_changed("README", "READ ME"));
    }

    #[test]
    fn every_conflict_explains_itself() {
        for kind in [
            ConflictKind::DuplicateTarget { others: vec![1, 2] },
            ConflictKind::TargetExists,
            ConflictKind::UnresolvedCycle,
        ] {
            let help = conflict_help(&kind);
            assert!(help.len() > 10, "{kind:?} needs a real explanation: {help}");
        }
    }

    fn item(state: RowState, new_name: &str, actions: Vec<ren_core::PlannedAction>) -> PlanItem {
        PlanItem {
            index: 0,
            source: "/tmp/a.txt".into(),
            new_name: new_name.into(),
            target: format!("/tmp/{new_name}").into(),
            state,
            actions,
        }
    }

    fn action(describe: &str) -> ren_core::PlannedAction {
        ren_core::PlannedAction {
            step: 0,
            op: "set_attributes",
            effect: ren_core::Effect::Attributes(ren_platform::AttributeChange {
                read_only: Some(false),
                ..Default::default()
            }),
            undoability: ren_core::Undoability::Journaled,
            describe: describe.into(),
        }
    }

    /// The sharpest silent bug M5 could have shipped: `RowState::Unchanged` is
    /// only about the name, so a row the run is about to change would have been
    /// greyed out as untouched.
    #[test]
    fn a_row_that_is_only_acted_on_is_not_dimmed() {
        let acted = item(
            RowState::Unchanged,
            "a.txt",
            vec![action("Write Protect off")],
        );
        let cell = cell_of(Some(&acted));
        assert!(!cell.is_dimmed(), "{cell:?} would be dimmed");
    }

    /// The other half of the same rule: a row nothing happens to *is* faded, in
    /// whichever view is drawing it.
    #[test]
    fn a_row_the_run_leaves_alone_is_dimmed() {
        assert!(cell_of(None).is_dimmed());
        assert!(cell_of(Some(&item(RowState::Unchanged, "a.txt", vec![]))).is_dimmed());
        assert!(!cell_of(Some(&item(RowState::Changed, "z.txt", vec![]))).is_dimmed());
    }

    #[test]
    fn an_action_row_says_what_it_will_do_instead_of_the_word_unchanged() {
        assert_eq!(
            cell_of(Some(&item(
                RowState::Unchanged,
                "a.txt",
                vec![action("Write Protect off")]
            ))),
            Cell::Acted {
                what: "Write Protect off".into()
            }
        );
    }

    /// D38: the column means "what this run does to this row", so a pipeline
    /// that renames *and* acts has to show both.
    #[test]
    fn a_row_that_is_renamed_and_acted_on_shows_both() {
        assert_eq!(
            cell_of(Some(&item(
                RowState::Changed,
                "z.txt",
                vec![action("Write Protect off")]
            ))),
            Cell::Both {
                new: "z.txt",
                what: "Write Protect off".into()
            }
        );
    }

    #[test]
    fn two_actions_on_one_row_are_joined_into_one_cell() {
        assert_eq!(
            cell_of(Some(&item(
                RowState::Unchanged,
                "a.txt",
                vec![action("Write Protect off"), action("Hidden on")]
            ))),
            Cell::Acted {
                what: "Write Protect off, Hidden on".into()
            }
        );
    }

    /// P4 blocks the whole run, so advertising an action that will not happen
    /// would be a lie.
    #[test]
    fn a_conflicting_row_does_not_also_advertise_its_action() {
        let blocked = item(
            RowState::Conflict(ConflictKind::TargetExists),
            "a.txt",
            vec![action("Write Protect off")],
        );
        assert!(matches!(cell_of(Some(&blocked)), Cell::Conflict { .. }));
    }

    #[test]
    fn the_ordinary_shapes_still_read_as_they_did() {
        assert_eq!(cell_of(None), Cell::Outside);
        assert_eq!(
            cell_of(Some(&item(RowState::Unchanged, "a.txt", vec![]))),
            Cell::Untouched
        );
        assert_eq!(
            cell_of(Some(&item(RowState::Changed, "z.txt", vec![]))),
            Cell::Renamed { new: "z.txt" }
        );
        assert!(matches!(
            cell_of(Some(&item(RowState::Error("boom".into()), "a.txt", vec![]))),
            Cell::Error("boom")
        ));
    }

    /// The Changed chip hides rows, and must hide the same rows whichever view
    /// is asking — a view that walked `entries` itself would ignore it.
    #[test]
    fn the_row_filter_decides_what_either_view_shows() {
        let plan = Plan {
            items: vec![
                item(RowState::Changed, "z.txt", vec![]),
                item(RowState::Unchanged, "a.txt", vec![]),
            ],
            ops: Vec::new(),
            notes: Vec::new(),
        };
        let index = [Some(0), Some(1)];

        assert_eq!(visible_rows(2, Some(&plan), &index, RowFilter::All), [0, 1]);
        assert_eq!(
            visible_rows(2, Some(&plan), &index, RowFilter::Changed),
            [0]
        );
        // No plan yet: only the unfiltered view shows anything at all.
        assert_eq!(visible_rows(2, None, &[], RowFilter::All), [0, 1]);
        assert!(visible_rows(2, None, &[], RowFilter::Changed).is_empty());
    }
}
