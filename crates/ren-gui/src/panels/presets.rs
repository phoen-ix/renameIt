//! The preset drawer.
//!
//! `docs/DESIGN.md` S8: *"Slide-over listing saved pipelines with descriptions;
//! actions: Load (replaces stack), Append, Run directly, rename/delete/
//! duplicate, import/export."*
//!
//! Not an animated slide-over, though: an animation asks egui to keep
//! repainting, which is exactly what D26 bans — a headless run would never
//! settle. A resizable side panel says the same thing and keeps the file table
//! visible, which matters when Run is pressed from here.

use std::path::PathBuf;

use ren_core::preset::{PresetEntry, PresetProblem};

use crate::dialogs::FileDialogs;

/// What the drawer is in the middle of.
#[derive(Debug, Default)]
pub struct DrawerState {
    /// The name typed into *Save as preset*.
    pub new_name: String,
    /// `(path, new name)` while a rename is being typed.
    pub renaming: Option<(PathBuf, String)>,
    /// The preset a delete is waiting to be confirmed for.
    pub confirming_delete: Option<PathBuf>,
}

/// What the drawer asked the app to do.
#[derive(Debug, Default)]
pub struct DrawerOutput {
    pub save_as: Option<String>,
    pub load: Option<PathBuf>,
    pub append: Option<PathBuf>,
    pub run: Option<PathBuf>,
    pub rename: Option<(PathBuf, String)>,
    pub duplicate: Option<PathBuf>,
    pub delete: Option<PathBuf>,
    pub import: Option<PathBuf>,
    pub export: Option<(PathBuf, PathBuf)>,
    pub close: bool,
}

impl DrawerOutput {
    /// Whether the drawer asked for anything at all this frame.
    ///
    /// Deliberately **over-broad**: `load`, `append`, `run` and `export` leave
    /// the preset folder exactly as they found it, and each of them still
    /// answers yes. The alternative is a list of the five actions that do
    /// change it, and the whole reason the Explorer menu is refreshed from this
    /// one funnel is that a tenth action must not be able to forget. A spare
    /// rewrite costs forty registry writes on a click the user made; a missed
    /// one costs a menu that is quietly wrong until something else repairs it.
    pub fn asked_for_something(&self) -> bool {
        // **Destructured on purpose, and without `..`.** A tenth field stops
        // the build here until someone decides whether it changes the preset
        // folder — which is a stronger guarantee than any test of this function
        // could give, because the thing being guarded against is a field that
        // does not exist yet.
        let Self {
            save_as,
            load,
            append,
            run,
            rename,
            duplicate,
            delete,
            import,
            export,
            // Closing the drawer changes nothing on disk.
            close: _,
        } = self;
        save_as.is_some()
            || load.is_some()
            || append.is_some()
            || run.is_some()
            || rename.is_some()
            || duplicate.is_some()
            || delete.is_some()
            || import.is_some()
            || export.is_some()
    }
}

pub fn ui(
    ui: &mut egui::Ui,
    state: &mut DrawerState,
    entries: &[PresetEntry],
    problems: &[PresetProblem],
    pipeline_name: &str,
    dialogs: &dyn FileDialogs,
) -> DrawerOutput {
    let mut out = DrawerOutput::default();

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Presets").heading());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("✖").on_hover_text("Close the drawer").clicked() {
                out.close = true;
            }
        });
    });
    ui.label(
        egui::RichText::new(
            "A preset is this pipeline, saved. It runs over whatever you have listed.",
        )
        .weak()
        .small(),
    );

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if state.new_name.is_empty() && !pipeline_name.is_empty() {
            state.new_name = pipeline_name.to_owned();
        }
        ui.add(
            egui::TextEdit::singleline(&mut state.new_name)
                .desired_width(150.0)
                .hint_text("Name this pipeline")
                .id_salt("preset_name"),
        );
        if ui
            .add_enabled(
                !state.new_name.trim().is_empty(),
                egui::Button::new("Save as preset"),
            )
            .clicked()
        {
            out.save_as = Some(state.new_name.trim().to_owned());
        }
    });

    ui.add_space(6.0);
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("preset_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if entries.is_empty() && problems.is_empty() {
                ui.label(
                    egui::RichText::new("No presets saved yet.")
                        .weak()
                        .italics(),
                );
            }
            for entry in entries {
                row(ui, state, entry, &mut out, dialogs);
                ui.add_space(4.0);
            }
            // A file that will not load is named, not hidden: a preset that
            // stopped working is exactly what the user needs told.
            for problem in problems {
                let name = problem
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| problem.path.display().to_string());
                ui.label(
                    egui::RichText::new(format!("⚠ {name}: {}", problem.error))
                        .color(ui.visuals().warn_fg_color)
                        .small(),
                );
            }
        });

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Import…").clicked()
            && let Some(path) = dialogs.open_preset()
        {
            out.import = Some(path);
        }
    });

    out
}

fn row(
    ui: &mut egui::Ui,
    state: &mut DrawerState,
    entry: &PresetEntry,
    out: &mut DrawerOutput,
    dialogs: &dyn FileDialogs,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.push_id(&entry.path, |ui| {
            if state
                .renaming
                .as_ref()
                .is_some_and(|(path, _)| path == &entry.path)
            {
                let mut cancel = false;
                ui.horizontal(|ui| {
                    let (_, name) = state.renaming.as_mut().expect("just checked");
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .desired_width(140.0)
                            .id_salt("rename"),
                    );
                    let typed = name.clone();
                    if ui.button("Rename").clicked() {
                        out.rename = Some((entry.path.clone(), typed));
                        cancel = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
                if cancel {
                    state.renaming = None;
                }
                return;
            }

            ui.label(egui::RichText::new(&entry.name).strong());
            let description = if entry.description.is_empty() {
                match entry.steps {
                    1 => "1 operation".to_owned(),
                    n => format!("{n} operations"),
                }
            } else {
                entry.description.clone()
            };
            ui.label(egui::RichText::new(description).weak().small());

            ui.horizontal(|ui| {
                if ui
                    .button("Load")
                    .on_hover_text("Replace the pipeline with this one")
                    .clicked()
                {
                    out.load = Some(entry.path.clone());
                }
                if ui
                    .button("Append")
                    .on_hover_text("Add its operations to the end of this pipeline")
                    .clicked()
                {
                    out.append = Some(entry.path.clone());
                }
                if ui
                    .button("Run")
                    .on_hover_text("Load it and rename straight away")
                    .clicked()
                {
                    out.run = Some(entry.path.clone());
                }

                let row_menu = crate::widgets::icons::icon_button(
                    ui,
                    crate::widgets::icons::Icon::Kebab,
                    "Rename, duplicate, export or delete this preset",
                );
                egui::Popup::menu(&row_menu).show(|ui| {
                    if ui.button("Rename…").clicked() {
                        state.renaming = Some((entry.path.clone(), entry.name.clone()));
                        ui.close();
                    }
                    if ui.button("Duplicate").clicked() {
                        out.duplicate = Some(entry.path.clone());
                        ui.close();
                    }
                    if ui.button("Export…").clicked() {
                        if let Some(to) = dialogs.save_preset(&entry.name) {
                            out.export = Some((entry.path.clone(), to));
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Delete").clicked() {
                        state.confirming_delete = Some(entry.path.clone());
                        ui.close();
                    }
                });
            });

            // Confirmed in the row rather than in a second modal, which would
            // be a dialog on top of a drawer on top of the app.
            if state.confirming_delete.as_ref() == Some(&entry.path) {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Delete “{}”?", entry.name))
                            .color(ui.visuals().warn_fg_color),
                    );
                    if ui.button("Delete").clicked() {
                        out.delete = Some(entry.path.clone());
                        state.confirming_delete = None;
                    }
                    if ui.button("Keep").clicked() {
                        state.confirming_delete = None;
                    }
                });
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame in which the drawer is merely open must not rewrite anything.
    #[test]
    fn a_drawer_that_asked_for_nothing_says_so() {
        assert!(!DrawerOutput::default().asked_for_something());
        assert!(
            !DrawerOutput {
                close: true,
                ..Default::default()
            }
            .asked_for_something(),
            "closing the drawer changes nothing on disk"
        );
    }

    /// Every action, one at a time. The compiler already refuses a tenth field
    /// that nobody classified; this is the other half — that each of the nine
    /// is classified as *something*.
    #[test]
    fn every_drawer_action_counts_as_one() {
        let path = || PathBuf::from("/p/a.toml");
        let each: [DrawerOutput; 9] = [
            DrawerOutput {
                save_as: Some("n".into()),
                ..Default::default()
            },
            DrawerOutput {
                load: Some(path()),
                ..Default::default()
            },
            DrawerOutput {
                append: Some(path()),
                ..Default::default()
            },
            DrawerOutput {
                run: Some(path()),
                ..Default::default()
            },
            DrawerOutput {
                rename: Some((path(), "n".into())),
                ..Default::default()
            },
            DrawerOutput {
                duplicate: Some(path()),
                ..Default::default()
            },
            DrawerOutput {
                delete: Some(path()),
                ..Default::default()
            },
            DrawerOutput {
                import: Some(path()),
                ..Default::default()
            },
            DrawerOutput {
                export: Some((path(), path())),
                ..Default::default()
            },
        ];
        for (index, out) in each.iter().enumerate() {
            assert!(out.asked_for_something(), "action {index}");
        }
    }
}
