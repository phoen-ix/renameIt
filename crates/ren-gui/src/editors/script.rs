//! Scripting.
//!
//! A *Choose script* combo, a *Description:* line under it, an *Arguments* box,
//! and a button that opens the script with whatever the desktop uses rather
//! than hard-coding an editor.
//!
//! The folder is only enumerated while the combo is **open**. Drawing a card
//! happens every frame, and a `read_dir` per frame per card is a cost the
//! preview budget has no room for — where reading the *chosen* script is free,
//! because it comes from the same mtime-keyed compile cache the preview uses.

use ren_core::ops::Script;

use crate::widgets::form::{After, Form};
use crate::widgets::icons::{Icon, icon_button};

pub fn ui(ui: &mut egui::Ui, op: &mut Script, cx: &crate::editors::EditorCx<'_>) -> bool {
    let mut changed = false;
    let store = op.store();
    let error = ui.visuals().error_fg_color;

    Form::new("script").show(ui, |form| {
        changed |= form
            .row("Choose script:", After::Buttons(1), |row| {
                let width = row.field_width();
                let ui = row.ui();
                let mut changed = false;
                egui::ComboBox::from_id_salt("script_choice")
                    .selected_text(if op.script.is_empty() {
                        "— none —".to_owned()
                    } else {
                        op.script.clone()
                    })
                    .width(width)
                    .show_ui(ui, |ui| {
                        // Only here, so the folder is read when the user opens the
                        // list rather than sixty times a second.
                        let (scripts, legacy) = store.list();
                        changed |= ui
                            .selectable_value(&mut op.script, String::new(), "— none —")
                            .changed();
                        for entry in &scripts {
                            let response = ui
                                .selectable_value(&mut op.script, entry.name.clone(), &entry.name)
                                .on_hover_text(if entry.header.description.is_empty() {
                                    entry.path.display().to_string()
                                } else {
                                    entry.header.description.clone()
                                });
                            changed |= response.changed();
                        }
                        if scripts.is_empty() {
                            ui.label(
                                egui::RichText::new("No scripts in the folder.")
                                    .weak()
                                    .small(),
                            );
                        }
                        // Listed, not hidden: a folder of .frs files that simply did
                        // not appear would leave the user with nothing to go on.
                        if !legacy.is_empty() {
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} .frs script(s) here cannot run — see docs/MIGRATION-legacy-scripts.md",
                                    legacy.len()
                                ))
                                .weak()
                                .small(),
                            );
                        }
                    });

                // Opening the folder rather than one file: it avoids hard-coding an
                // editor, and it is also the only way to *add* a script.
                if icon_button(ui, Icon::Folder, "Open the scripts folder")
                    .on_hover_text(format!("Open {}", store.dir().display()))
                    .clicked()
                {
                    let _ = cx.platform.reveal_in_file_manager(store.dir());
                }
                changed
            })
            .inner;

        // The description, and the argument hint, both read off the chosen script.
        let chosen = op.chosen();
        if let Some(chosen) = &chosen {
            match &**chosen {
                Ok(compiled) => {
                    let header = compiled.header();
                    if !header.description.is_empty() {
                        form.note(format!("Description: {}", header.description));
                    }
                }
                Err(error_text) => {
                    form.note(egui::RichText::new(format!("⚠ {error_text}")).color(error));
                }
            }
        }

        let hint = chosen
            .as_ref()
            .and_then(|c| c.as_ref().as_ref().ok())
            .and_then(|c| c.header().args.clone());

        changed |= form
            .row("Arguments:", After::Nothing, |row| {
                let width = row.field_width();
                let field = row.ui().add(
                    egui::TextEdit::singleline(&mut op.args)
                        .desired_width(width)
                        .hint_text(hint.clone().unwrap_or_default())
                        .id_salt("script_args"),
                );
                let changed = field.changed();
                // `# args:` pulls the argument syntax out of the description
                // paragraph to where it is actually needed.
                if let Some(hint) = &hint
                    && !hint.is_empty()
                {
                    field.on_hover_text(hint);
                }
                changed
            })
            .inner;

        // Only for a script that compiled and declares no `# args:` line. A
        // missing or broken one has said so above, and cannot say whether it
        // takes arguments — the note beside that error read as "delete what
        // is in the box", which a script that is merely missing today will
        // want back.
        if chosen
            .as_ref()
            .is_some_and(|c| c.as_ref().as_ref().is_ok_and(|c| c.header().args.is_none()))
        {
            form.note("This script does not take arguments.");
        }
    });

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "A script runs once per file, in list order, and its globals last for the \
             whole run. Rows are evaluated one at a time while a script is in the pipeline.",
        )
        .weak()
        .small(),
    );

    changed
}
