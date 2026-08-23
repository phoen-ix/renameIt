//! Native file dialogs, behind a trait so tests never open one.
//!
//! This used to be a `#[cfg(test)]` pair of stubs inside `panels/source_bar.rs`,
//! which does not work and was one preset-import button away from being found
//! out: an integration test in `tests/` links the library compiled **without**
//! `cfg(test)`, so the stubs were inert there. Nothing clicked 📂, so nobody
//! noticed. A real `rfd` dialog in a headless CI run does not fail — it waits
//! forever.
//!
//! So the choice is made at construction instead: [`HostDialogs`] for the app,
//! [`NoDialogs`] for [`crate::RenameItApp::headless`]. A test drives the paths
//! directly (`import_preset_from(path)`), which is the same shape as
//! `set_filter` and `rename_one`.

use std::path::{Path, PathBuf};

/// The file pickers the app needs. Every method may return `None`: the user
/// cancelled, or there is no desktop to ask.
pub trait FileDialogs: Send + Sync + std::fmt::Debug {
    fn pick_folder(&self, start: &Path) -> Option<PathBuf>;
    fn pick_files(&self) -> Option<Vec<PathBuf>>;
    /// Choosing a preset to import.
    fn open_preset(&self) -> Option<PathBuf>;
    /// Choosing where to write a preset.
    fn save_preset(&self, suggested_name: &str) -> Option<PathBuf>;
    /// Choosing the list for a CSV List Rename card.
    fn open_csv(&self) -> Option<PathBuf>;
}

/// The real thing.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostDialogs;

impl FileDialogs for HostDialogs {
    fn pick_folder(&self, start: &Path) -> Option<PathBuf> {
        rfd::FileDialog::new().set_directory(start).pick_folder()
    }

    fn pick_files(&self) -> Option<Vec<PathBuf>> {
        rfd::FileDialog::new().pick_files()
    }

    fn open_preset(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("RenameIt preset", &["toml"])
            .pick_file()
    }

    fn save_preset(&self, suggested_name: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("RenameIt preset", &["toml"])
            .set_file_name(format!("{suggested_name}.toml"))
            .save_file()
    }

    fn open_csv(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("Separated values", &["csv", "tsv", "txt"])
            .add_filter("All files", &["*"])
            .pick_file()
    }
}

/// Answers nothing, and blocks nothing. What the headless app uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDialogs;

impl FileDialogs for NoDialogs {
    fn pick_folder(&self, _start: &Path) -> Option<PathBuf> {
        None
    }
    fn pick_files(&self) -> Option<Vec<PathBuf>> {
        None
    }
    fn open_preset(&self) -> Option<PathBuf> {
        None
    }
    fn save_preset(&self, _suggested_name: &str) -> Option<PathBuf> {
        None
    }
    fn open_csv(&self) -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headless_dialogs_answer_nothing() {
        let dialogs = NoDialogs;
        assert_eq!(dialogs.pick_folder(Path::new("/tmp")), None);
        assert_eq!(dialogs.pick_files(), None);
        assert_eq!(dialogs.open_preset(), None);
        assert_eq!(dialogs.save_preset("Photo cleanup"), None);
        assert_eq!(dialogs.open_csv(), None);
    }
}
