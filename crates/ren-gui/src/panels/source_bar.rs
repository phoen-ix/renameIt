//! Where the files come from — the top zone of screen S1.
//!
//! A segmented control (Browser | Free Select), with the listing switches as
//! chips on the same row.

use std::path::PathBuf;

use crate::dialogs::FileDialogs;
use crate::viewmodel::{Session, SourceMode, ViewMode};

/// The address box's widget id, so F6 can focus it from outside this panel.
///
/// A function rather than a constant because `Id::with` hashes at run time.
pub fn address_box() -> egui::Id {
    egui::Id::unique("source_bar_address")
}

/// What the source bar wants the app to do after this frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SourceBarOutput {
    /// The listing must be rebuilt from disk.
    pub relist: bool,
    /// ⟳ was pressed: forget everything read from inside the files, then
    /// relist — F9's `forget_and_relist`, not merely `relist` (D140). A plain
    /// relist re-reads names, sizes and dates, so a file retagged by a tool
    /// that kept its length and mtime went on showing its old tags, and the
    /// button that says "Refresh (F9)" confirmed the stale value.
    pub hard_refresh: bool,
    /// The ⚙ was pressed.
    pub open_settings: bool,
    /// "Open in Explorer": show the folder being browsed in the file manager.
    pub reveal_dir: bool,
    /// *Rearrange names…* (P124).
    pub open_rearrange: bool,
}

impl SourceBarOutput {
    fn merge(&mut self, other: Self) {
        self.relist |= other.relist;
        self.hard_refresh |= other.hard_refresh;
        self.open_settings |= other.open_settings;
        self.reveal_dir |= other.reveal_dir;
        self.open_rearrange |= other.open_rearrange;
    }
}

pub fn ui(
    ui: &mut egui::Ui,
    session: &mut Session,
    dialogs: &dyn FileDialogs,
    listing: bool,
    file_manager: &str,
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
            SourceMode::Browser => out.merge(browser_controls(ui, session, dialogs, file_manager)),
            SourceMode::FreeSelect => out.merge(free_select_controls(ui, session, dialogs)),
        }

        // Settings, at the window's own top-right corner (D255). It was a ⋯
        // menu of Settings, Right-click menu… and About (D251); the middle
        // entry only jumped into Settings and About is now its last page, so
        // the one command left needs no menu.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            use crate::widgets::icons::{Icon, icon_button};
            if icon_button(ui, Icon::Gear, "Settings")
                .on_hover_text("Settings (F8)")
                .clicked()
            {
                out.open_settings = true;
            }
            // The 2.0 headline, where it is seen (P124).
            if ui
                .button("Rearrange names…")
                .on_hover_text("Cut a name into blocks and drag them into a new order")
                .clicked()
            {
                out.open_rearrange = true;
            }
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
    file_manager: &str,
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

    // The one filter (P121): it hides what does not match. Named by its
    // label, or a screen reader announces a bare "edit text".
    let label = ui.label("Filter:");
    let pattern = ui
        .add(
            egui::TextEdit::singleline(&mut session.settings.pattern)
                .desired_width(90.0)
                .hint_text("*.*"),
        )
        .labelled_by(label.id)
        .on_hover_text(
            "Show only names that match, e.g. *.jpg. Plain text matches anywhere in the name; \
             * is any run of characters, ? exactly one.",
        );
    if pattern.changed() {
        out.relist = true;
    }
    out.relist |= filter_extras(ui, &mut session.settings);

    if ui.button("⟳").on_hover_text("Refresh (F9)").clicked() {
        out.hard_refresh = true;
    }

    // The way back out to the file manager, for the folder on screen (D252).
    // The row menu's "Show in file manager" does it for one file; this is the
    // folder itself, which had no way there at all.
    if ui
        .button(format!("Open in {file_manager}"))
        .on_hover_text(format!("Open this folder in {file_manager}"))
        .clicked()
    {
        out.reveal_dir = true;
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

/// The ▾ beside the Filter box: exclude, regex, whole path, case (P121).
/// Lit while any of them is set, so a hidden exclude is never invisible.
fn filter_extras(ui: &mut egui::Ui, settings: &mut crate::viewmodel::SessionSettings) -> bool {
    let active = !settings.filter_exclude.trim().is_empty()
        || settings.filter_regex
        || settings.filter_whole_path
        || settings.filter_case;
    let toggle = ui
        .add(egui::Button::selectable(active, "▾"))
        .on_hover_text("Exclude, regular expression, whole path, case");
    // The glyph is not a name (P92).
    toggle.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, active, "Filter options")
    });
    let mut changed = false;
    // Open until a click lands outside it, so a click into the Exclude box
    // does not shut it before a key arrives.
    egui::Popup::from_toggle_button_response(&toggle)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.horizontal(|ui| {
                let label = ui.label("Exclude:");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut settings.filter_exclude)
                            .desired_width(140.0)
                            .hint_text("*_old*"),
                    )
                    .labelled_by(label.id)
                    .changed();
            });
            ui.horizontal(|ui| {
                changed |= ui
                    .radio_value(&mut settings.filter_regex, false, "Wildcards")
                    .changed();
                changed |= ui
                    .radio_value(&mut settings.filter_regex, true, "Regular expression")
                    .changed();
            });
            changed |= ui
                .checkbox(&mut settings.filter_whole_path, "Test the whole path")
                .changed();
            changed |= ui
                .checkbox(&mut settings.filter_case, "Match case")
                .changed();
        });
    changed
}
