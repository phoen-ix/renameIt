//! CSV List Rename.
//!
//! Top to bottom in the order the choices are made: the file and its browse
//! button, the separator combo with its custom box, the two column numbers,
//! and the case switch.

use ren_core::ops::{CsvList, CsvSeparator};

use crate::dialogs::FileDialogs;
use crate::theme::width::FIELD_CHAR;
use crate::widgets::form::{After, Form};
use crate::widgets::icons::{Icon, icon_button};

pub fn ui(ui: &mut egui::Ui, op: &mut CsvList, dialogs: &dyn FileDialogs) -> bool {
    let mut changed = false;

    Form::new("csv_list").show(ui, |form| {
        changed |= form
            .row("CSV File:", After::Buttons(1), |row| {
                let width = row.field_width();
                let ui = row.ui();
                let mut changed = false;
                // A real text box as well as a button: a path can be pasted, and a
                // headless test can type one without a desktop to click.
                let mut text = op.file.display().to_string();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut text)
                            .desired_width(width)
                            .hint_text("path to the list")
                            .id_salt("csv_path"),
                    )
                    .changed()
                {
                    op.file = text.into();
                    changed = true;
                }
                // A painted mark rather than a glyph, so the room `After::Buttons`
                // reserves for it is the room it takes.
                if icon_button(ui, Icon::Folder, "Choose a CSV file")
                    .on_hover_text("Choose a CSV file")
                    .clicked()
                    && let Some(path) = dialogs.open_csv()
                {
                    op.file = path;
                    changed = true;
                }
                changed
            })
            .inner;

        changed |= form
            .row("Separator:", After::Nothing, |row| {
                let ui = row.ui();
                let mut changed = false;
                egui::ComboBox::from_id_salt("csv_separator")
                    .selected_text(op.separator.label())
                    .show_ui(ui, |ui| {
                        for candidate in CsvSeparator::ALL {
                            changed |= ui
                                .selectable_value(&mut op.separator, candidate, candidate.label())
                                .changed();
                        }
                    });
                // Dead unless "Other:" is chosen.
                let custom = op.separator == CsvSeparator::Other;
                ui.add_enabled_ui(custom, |ui| {
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut op.separator_char)
                                .desired_width(FIELD_CHAR)
                                .hint_text("char")
                                .id_salt("csv_separator_char"),
                        )
                        .on_hover_text("One character. {TAB} and {ENTER} also work.")
                        .changed();
                });
                changed
            })
            .inner;

        // Columns, not string positions — so these are 1-based and P15's zero-based
        // rule does not reach them.
        changed |= form
            .row("Column with current filename:", After::Nothing, |row| {
                crate::widgets::number::add(
                    row.ui(),
                    egui::DragValue::new(&mut op.old_column).range(1..=99),
                )
                .on_hover_text("The first column is 1.")
                .changed()
            })
            .inner;
        changed |= form
            .row("Column with new filename:", After::Nothing, |row| {
                crate::widgets::number::add(
                    row.ui(),
                    egui::DragValue::new(&mut op.new_column).range(1..=99),
                )
                .on_hover_text("The first column is 1.")
                .changed()
            })
            .inner;
    });

    changed |= ui
        .checkbox(&mut op.case_sensitive, "Case Sensitive")
        .on_hover_text("Off by default, so `Lorem` in the list matches `lorem.txt` on disk.")
        .changed();

    ui.add_space(4.0);

    // Read from the same shared parse the preview uses, so this costs a `stat`
    // rather than a second read of the file.
    if !op.file.as_os_str().is_empty() {
        match op.status() {
            Ok(rows) => ui.label(
                egui::RichText::new(match rows {
                    1 => "1 row loaded.".to_owned(),
                    n => format!("{n} rows loaded."),
                })
                .weak()
                .small(),
            ),
            Err(e) => ui.label(
                egui::RichText::new(format!("⚠ {e}"))
                    .color(ui.visuals().error_fg_color)
                    .small(),
            ),
        };
    }

    ui.label(
        egui::RichText::new(
            "Names in the list are matched against the part this operation is scoped to — \
             stems by default, so extensions survive. If your list carries extensions, \
             set Scope to Both.",
        )
        .weak()
        .small(),
    );
    // D39: the path is stored verbatim, so say so before the preset is shared
    // rather than after somebody else opens it.
    ui.label(
        egui::RichText::new(
            "A preset stores this path exactly as written — an absolute path will not \
             resolve on another machine.",
        )
        .weak()
        .small(),
    );

    changed
}
