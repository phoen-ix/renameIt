//! The window a simulated run opens (P123): every change it would have made,
//! old against new, before anything is written.
//!
//! Built **once**, when the simulated run finishes, from the plan that ran and
//! its report — a snapshot, like the confirmation's, so what the user reviews
//! cannot shift underneath them. *Before* values come from the listing the
//! plan was made over; nothing is read from disk just for this window, so a
//! tag's current value shows as a dash.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ren_core::exec::ApplyReport;
use ren_core::{Effect, FileEntry, Plan};

use crate::widgets::diff_text::{self, DiffStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Renamed,
    /// Dates, attributes, tags: anything an action changes besides the name.
    Changed,
    Failed,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Renamed => "Renamed",
            Self::Changed => "Changed",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    /// The file before the run, and where a rename would put it.
    pub from: PathBuf,
    pub to: PathBuf,
    pub before: String,
    pub after: String,
}

/// Which rows the window shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Show {
    #[default]
    All,
    Only(Kind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationResult {
    pub rows: Vec<Row>,
    /// Lines a script's `done()` returned.
    pub notes: Vec<String>,
    pub show: Show,
    pub search: String,
    pub full_paths: bool,
}

/// What the window asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Close,
    RenameNow,
}

const NOTHING: &str = "—";

impl SimulationResult {
    pub fn new(plan: &Plan, report: &ApplyReport, entries: &[FileEntry]) -> Self {
        let listed: HashMap<&Path, &FileEntry> =
            entries.iter().map(|e| (e.path.as_path(), e)).collect();
        let name = |path: &Path| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut rows = Vec::new();
        for item in &plan.items {
            if item.target != item.source {
                rows.push(Row {
                    kind: Kind::Renamed,
                    from: item.source.clone(),
                    to: item.target.clone(),
                    before: name(&item.source),
                    after: item.new_name.clone(),
                });
            }
            for action in &item.actions {
                rows.push(Row {
                    kind: Kind::Changed,
                    from: item.source.clone(),
                    to: item.target.clone(),
                    before: before_value(&action.effect, listed.get(item.source.as_path())),
                    after: action.describe.clone(),
                });
            }
        }
        for (path, reason) in &report.failed {
            rows.push(Row {
                kind: Kind::Failed,
                from: path.clone(),
                to: path.clone(),
                before: name(path),
                after: reason.clone(),
            });
        }
        Self {
            rows,
            notes: report.notes.clone(),
            show: Show::All,
            search: String::new(),
            full_paths: false,
        }
    }

    pub fn count(&self, kind: Kind) -> usize {
        self.rows.iter().filter(|r| r.kind == kind).count()
    }

    /// The rows the chips, the search box and the path switch leave, as
    /// `(what, before, after)` text.
    pub fn shown(&self) -> Vec<(&Row, String, String)> {
        let needle = self.search.trim().to_lowercase();
        self.rows
            .iter()
            .filter(|r| self.show == Show::All || self.show == Show::Only(r.kind))
            .map(|r| {
                let (before, after) = if self.full_paths && r.kind == Kind::Renamed {
                    (r.from.display().to_string(), r.to.display().to_string())
                } else if self.full_paths {
                    (
                        format!("{}: {}", r.from.display(), r.before),
                        r.after.clone(),
                    )
                } else {
                    (r.before.clone(), r.after.clone())
                };
                (r, before, after)
            })
            .filter(|(_, before, after)| {
                needle.is_empty()
                    || before.to_lowercase().contains(&needle)
                    || after.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// What *Copy list* puts on the clipboard: one tab-separated line per
    /// shown row, which is what a spreadsheet wants.
    pub fn text(&self) -> String {
        self.shown()
            .iter()
            .map(|(r, before, after)| format!("{}\t{before}\t{after}\n", r.kind.label()))
            .collect()
    }
}

/// The value an action would replace, where the listing already holds it.
fn before_value(effect: &Effect, entry: Option<&&FileEntry>) -> String {
    let (Effect::Times(set), Some(entry)) = (effect, entry) else {
        return NOTHING.to_owned();
    };
    let parts: Vec<String> = [
        ("Created", set.created.is_some(), entry.created),
        ("Modified", set.modified.is_some(), entry.modified),
        ("Accessed", set.accessed.is_some(), entry.accessed),
    ]
    .into_iter()
    .filter(|(_, changed, _)| *changed)
    .map(|(label, _, time)| {
        let when = time.map_or_else(|| NOTHING.to_owned(), super::file_table::human_time);
        format!("{label} {when}")
    })
    .collect();
    if parts.is_empty() {
        NOTHING.to_owned()
    } else {
        parts.join(", ")
    }
}

pub fn ui(ctx: &egui::Context, state: &mut SimulationResult) -> Outcome {
    let mut outcome = Outcome::Open;
    let modal = egui::Modal::new(egui::Id::unique("simulation_modal")).show(ctx, |ui| {
        let width = (ctx.content_rect().width() - 80.0).clamp(480.0, 1100.0);
        ui.set_width(width);
        ui.heading("Simulation result");
        let (renamed, changed, failed) = (
            state.count(Kind::Renamed),
            state.count(Kind::Changed),
            state.count(Kind::Failed),
        );
        ui.label(format!(
            "{} renamed · {} other changes · {} failed — nothing was written",
            renamed, changed, failed
        ));
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            for (show, label) in [
                (Show::All, format!("All {}", state.rows.len())),
                (Show::Only(Kind::Renamed), format!("Renamed {renamed}")),
                (
                    Show::Only(Kind::Changed),
                    format!("Other changes {changed}"),
                ),
                (Show::Only(Kind::Failed), format!("Failed {failed}")),
            ] {
                if ui.selectable_label(state.show == show, label).clicked() {
                    state.show = show;
                }
            }
            ui.separator();
            let label = ui.label("Search:");
            ui.add(egui::TextEdit::singleline(&mut state.search).desired_width(160.0))
                .labelled_by(label.id);
            ui.checkbox(&mut state.full_paths, "Full paths");
        });
        ui.add_space(4.0);

        let shown = state.shown();
        let style = DiffStyle::for_ui(ui);
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        let height = (ctx.content_rect().height() * 0.6).max(160.0);
        let what_width = 90.0;
        let column = ((width - what_width - 24.0) / 2.0).max(120.0);
        egui::ScrollArea::both()
            .max_height(height)
            .auto_shrink([false, true])
            .show_rows(ui, row_height, shown.len(), |ui, range| {
                egui::Grid::new("simulation_rows")
                    .num_columns(3)
                    .min_col_width(what_width)
                    .spacing([24.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        if range.start == 0 {
                            ui.strong("What");
                            ui.strong("Before");
                            ui.strong("After");
                            ui.end_row();
                        }
                        for (row, before, after) in &shown[range] {
                            ui.label(row.kind.label());
                            // Left-aligned like the file list; `add_sized` would
                            // centre the old name under its header.
                            let cell = egui::vec2(column, row_height);
                            let left = egui::Layout::left_to_right(egui::Align::Center);
                            ui.allocate_ui_with_layout(cell, left, |ui| {
                                ui.add(egui::Label::new(before).truncate());
                            });
                            ui.allocate_ui_with_layout(cell, left, |ui| match row.kind {
                                Kind::Renamed => diff_text::new_name(ui, before, after, style),
                                Kind::Failed => {
                                    ui.colored_label(ui.visuals().error_fg_color, after);
                                }
                                Kind::Changed => {
                                    ui.add(egui::Label::new(after).truncate());
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        if shown.is_empty() {
            ui.weak("Nothing to show.");
        }
        for note in &state.notes {
            ui.label(note);
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .button("Copy list")
                .on_hover_text("The rows shown, as tab-separated text")
                .clicked()
                && let Ok(mut clipboard) = arboard::Clipboard::new()
            {
                let _ = clipboard.set_text(state.text());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    outcome = Outcome::Close;
                }
                if ui
                    .button("Rename now")
                    .on_hover_text("Run this for real. Simulate stays ticked for next time.")
                    .clicked()
                {
                    outcome = Outcome::RenameNow;
                }
            });
        });
    });
    if modal.should_close() && outcome == Outcome::Open {
        outcome = Outcome::Close;
    }
    outcome
}
