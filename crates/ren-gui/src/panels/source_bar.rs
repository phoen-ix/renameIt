//! Where the files come from — the top zone of screen S1.
//!
//! A segmented control (Browser | Free Select), with the listing switches as
//! chips on the same row.

use std::path::PathBuf;

use crate::dialogs::FileDialogs;
use crate::viewmodel::{Session, SourceMode, ViewMode};
use crate::widgets::filter_editor::FilterForm;

/// The address box's widget id, so F6 can focus it from outside this panel.
///
/// A function rather than a constant because `Id::with` hashes at run time.
pub fn address_box() -> egui::Id {
    egui::Id::new("source_bar_address")
}

/// What the source bar wants the app to do after this frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SourceBarOutput {
    /// The listing must be rebuilt from disk.
    pub relist: bool,
    /// The filter changed, so the preview must be recomputed.
    pub refilter: bool,
    /// The Settings button was pressed.
    pub open_settings: bool,
    /// The About button was pressed.
    pub open_about: bool,
}

impl SourceBarOutput {
    fn merge(&mut self, other: Self) {
        self.relist |= other.relist;
        self.refilter |= other.refilter;
        self.open_settings |= other.open_settings;
        self.open_about |= other.open_about;
    }
}

pub fn ui(
    ui: &mut egui::Ui,
    session: &mut Session,
    filter: &mut FilterForm,
    dialogs: &dyn FileDialogs,
    listing: bool,
) -> SourceBarOutput {
    let mut out = SourceBarOutput::default();

    ui.horizontal(|ui| {
        // Browser | Free Select
        for (mode, label) in [
            (SourceMode::Browser, "Browser"),
            (SourceMode::FreeSelect, "Free Select"),
        ] {
            if ui
                .selectable_label(session.settings.mode == mode, label)
                .clicked()
                && session.settings.mode != mode
            {
                session.settings.mode = mode;
                out.relist = true;
            }
        }

        ui.separator();

        match session.settings.mode {
            SourceMode::Browser => out.merge(browser_controls(ui, session, dialogs)),
            SourceMode::FreeSelect => out.merge(free_select_controls(ui, session, dialogs)),
        }

        // Settings and About: the window's own commands, at the window's own
        // top-right corner. They used to live in the *Pipeline* panel's header,
        // which scoped them wrongly — neither has anything to do with the card
        // stack, and the panel is resizable, so they moved when it did.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            use crate::widgets::icons::{Icon, icon_button};
            out.open_settings |= icon_button(ui, Icon::Gear, "Settings")
                .on_hover_text("Settings (F8)")
                .clicked();
            out.open_about |= icon_button(ui, Icon::Info, "About RenameIt").clicked();
        });
    });

    ui.horizontal(|ui| {
        // The Include chips.
        for (label, flag) in [
            ("Files", &mut session.settings.files),
            ("Folders", &mut session.settings.folders),
            ("Subfolders", &mut session.settings.subfolders),
        ] {
            let enabled = session.settings.mode == SourceMode::Browser;
            let chip = ui.add_enabled_ui(enabled, |ui| ui.selectable_label(*flag, label));
            if chip.inner.clicked() {
                *flag = !*flag;
                out.relist = true;
            }
        }

        ui.separator();

        // The include filter, as a chip that opens a popover.
        let chip = ui
            .selectable_label(filter.is_active(), "Include filter…")
            .on_hover_ui(|ui| {
                ui.label(filter.summary());
            });
        egui::Popup::from_toggle_button_response(&chip).show(|ui| {
            if filter.ui(ui) {
                out.refilter = true;
            }
        });

        if listing {
            // The walk is on its own thread and the rows on screen are the
            // previous folder's until it lands. A static label, not a spinner
            // (D26): the worker wakes the UI once when it is done.
            ui.separator();
            ui.label(egui::RichText::new("listing…").weak().italics());
        }
        if let Some(error) = &session.error {
            ui.separator();
            ui.colored_label(egui::Color32::from_rgb(0xe5, 0x73, 0x73), error);
        }
        // Amber, not red, and beside a listing that is still showing: these
        // rows are missing, the rest are fine (P63).
        if !session.problems.is_empty() {
            ui.separator();
            let count = session.problems.len();
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("⚠ {count} item(s) could not be read"),
            )
            .on_hover_ui(|ui| {
                ui.label(
                    session
                        .problems
                        .iter()
                        .take(8)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            });
        }

        // List | Grid, at this row's far end and in the same shape as Browser |
        // Free Select — they are the same kind of choice, one about where the
        // files come from and one about how they are shown. On the row with
        // the other switches that decide what the list contains, rather than
        // stranded at the end of the row about *where* it comes from. Neither
        // relists nor refilters: both views draw the same listing through the
        // same filter.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for mode in [ViewMode::Grid, ViewMode::List] {
                if ui
                    .selectable_label(session.settings.view == mode, mode.label())
                    .clicked()
                {
                    session.settings.view = mode;
                }
            }
        });
    });

    out
}

fn browser_controls(
    ui: &mut egui::Ui,
    session: &mut Session,
    dialogs: &dyn FileDialogs,
) -> SourceBarOutput {
    let mut out = SourceBarOutput::default();

    if ui
        .button("📂")
        .on_hover_text("Choose a folder… (F12)")
        .clicked()
        && let Some(dir) = dialogs.pick_folder(&session.settings.dir)
    {
        session.settings.dir = dir;
        out.relist = true;
    }

    let mut path = session.settings.dir.display().to_string();
    let response = ui.add(
        egui::TextEdit::singleline(&mut path)
            .desired_width(360.0)
            .hint_text("Folder to rename")
            // A fixed id, not a salt: F6 focuses this box (`ADDRESS_BOX`) and
            // has no Ui in hand to derive a salted one from.
            .id(address_box()),
    );
    response
        .clone()
        .on_hover_text("Type a path, or press F6 to jump here");
    if response.changed() {
        session.settings.dir = PathBuf::from(path);
    }
    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        out.relist = true;
    }

    ui.label("Pattern:");
    let pattern = ui.add(
        egui::TextEdit::singleline(&mut session.settings.pattern)
            .desired_width(90.0)
            .hint_text("*.*"),
    );
    if pattern.changed() {
        out.relist = true;
    }

    if ui.button("⟳").on_hover_text("Refresh (F9)").clicked() {
        out.relist = true;
    }

    out
}

fn free_select_controls(
    ui: &mut egui::Ui,
    session: &mut Session,
    dialogs: &dyn FileDialogs,
) -> SourceBarOutput {
    let out = SourceBarOutput::default();

    if ui.button("Add files…").clicked()
        && let Some(paths) = dialogs.pick_files()
    {
        session.accept_dropped(paths);
    }
    if ui.button("Clear").clicked() {
        session.clear_free_select();
    }

    ui.label(format!(
        "{} file(s) from {} folder(s)",
        session.free_select.len(),
        session.free_select_folder_count()
    ));
    if session.free_select.is_empty() {
        ui.label(
            egui::RichText::new("Drag files here from your file manager")
                .weak()
                .italics(),
        );
    }

    out
}
