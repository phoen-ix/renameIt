//! Filename Editor.
//!
//! One text box, one line per file. The count line under it is this operation's
//! entire failure surface: lines pair with the listing by *position*, so a
//! mismatch means every pairing after the first difference is somebody else's
//! name. M5 makes that a validation error; this is where the user finds
//! out before pressing anything.

use ren_core::ops::FilenameEditor;

use super::EditorCx;
use crate::widgets::tag_field;

pub fn ui(ui: &mut egui::Ui, op: &mut FilenameEditor, cx: &EditorCx<'_>) -> bool {
    let mut changed = false;

    // Shallow on purpose: a link buried two containers deep never received its
    // click in a headless test (the M4 Settings lesson), and this one has to be
    // reachable — it is the fix for the error below it.
    if ui
        .link("(copy current filename list into editor)")
        .on_hover_text("Fills the box with the names showing now, in order")
        .clicked()
    {
        op.text = FilenameEditor::text_for(&cx.listed(), cx.scope);
        changed = true;
    }

    // A name carrying a line break cannot survive a line-based editor: the copy
    // has to flatten it to keep one line per file, and running that back would
    // rename the file to a name the user never typed. Named here, before the
    // click, rather than surfacing as a row error afterwards.
    let unrepresentable = FilenameEditor::unrepresentable(&cx.listed(), cx.scope);
    if !unrepresentable.is_empty() {
        let names: Vec<String> = unrepresentable
            .iter()
            .take(3)
            .map(|n| n.replace(['\n', '\r'], "\u{23ce}"))
            .collect();
        let more = unrepresentable.len().saturating_sub(names.len());
        ui.label(
            egui::RichText::new(format!(
                "\u{26a0} {} contain{} a line break and cannot be edited here{}",
                names.join(", "),
                if unrepresentable.len() == 1 { "s" } else { "" },
                if more > 0 {
                    format!(" (and {more} more)")
                } else {
                    String::new()
                },
            ))
            .color(ui.visuals().warn_fg_color)
            .small(),
        );
    }

    changed |= ui
        .add(
            egui::TextEdit::multiline(&mut op.text)
                .desired_rows(8)
                .desired_width(f32::INFINITY)
                // Monospace, because "line N is file N" only reads when the
                // lines line up.
                .code_editor()
                .hint_text("one new name per file")
                .id_salt("filename_editor"),
        )
        .changed();

    ui.horizontal(|ui| {
        if let Some(tag) = tag_field::menu(ui, "filename_editor_tags") {
            op.text.push_str(tag);
            changed = true;
        }
        ui.label(
            egui::RichText::new("Lines may contain <tags>.")
                .weak()
                .small(),
        );
    });

    let lines = op.line_count();
    let files = cx.scoped.len();
    ui.add_space(4.0);
    if op.text.is_empty() {
        ui.label(
            egui::RichText::new("An empty editor leaves every name alone.")
                .weak()
                .small(),
        );
    } else if lines == files {
        ui.label(
            egui::RichText::new(format!("{lines} line(s) for {files} file(s)."))
                .weak()
                .small(),
        );
    } else {
        ui.label(
            egui::RichText::new(format!(
                "⚠ {lines} line(s) for {files} file(s) — the run is blocked until they match."
            ))
            .color(ui.visuals().error_fg_color)
            .small(),
        );
    }

    // Ordering is this operation's silent hazard: re-sorting the table re-pairs
    // every line with no warning. Showing the first few pairings makes that
    // visible without needing to be explained.
    let pairs: Vec<String> = cx
        .scoped
        .iter()
        .filter_map(|&i| cx.entries.get(i))
        .zip(op.text.lines())
        .take(3)
        .enumerate()
        .map(|(n, (entry, line))| format!("{}  {} → {}", n + 1, entry.file_name, line))
        .collect();
    if !pairs.is_empty() {
        for pair in pairs {
            ui.label(egui::RichText::new(pair).weak().small().monospace());
        }
        ui.label(
            egui::RichText::new("Lines pair with the list in the order it is showing.")
                .weak()
                .small(),
        );
    }

    changed
}
