//! The host platform, for a test that needs to watch it or break it.
//!
//! Every call goes through to [`ren_platform::host`]. On top of that it counts
//! the swaps it is asked for, records every shell notice, and can be told to
//! fail a swap or a date change, having done nothing, as a pulled-out USB
//! stick or a filesystem without settable dates would.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ren_platform::{
    AttributeChange, Capability, CaseSensitivity, FileAttributes, FileTimes, NamingRules, Platform,
    PlatformError, Result, TimeChange,
};

#[derive(Debug)]
pub(crate) struct Spy {
    pub inner: Arc<dyn Platform>,
    /// `replace_file` fails.
    pub fail_swap: bool,
    /// `replace_file` fails the way Windows' `ERROR_UNABLE_TO_MOVE_REPLACEMENT`
    /// does when the move back fails too: the target is removed and the new
    /// contents are left at `temp` alone (`PlatformError::ReplacedButNotMoved`).
    pub strand_swap: bool,
    /// `set_times` fails.
    pub fail_dates: bool,
    /// How many times `replace_file` was called, failures included.
    pub swaps: AtomicUsize,
    /// Every path `notify_shell_changed` was given, in order.
    pub notified: Mutex<Vec<PathBuf>>,
}

impl Default for Spy {
    fn default() -> Self {
        Self {
            inner: ren_platform::host(),
            fail_swap: false,
            strand_swap: false,
            fail_dates: false,
            swaps: AtomicUsize::new(0),
            notified: Mutex::new(Vec::new()),
        }
    }
}

impl Spy {
    pub fn swaps(&self) -> usize {
        self.swaps.load(Ordering::SeqCst)
    }

    pub fn notified(&self) -> Vec<PathBuf> {
        self.notified.lock().unwrap().clone()
    }

    fn refuse(path: &Path, what: &str) -> PlatformError {
        PlatformError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(format!("{what} failed, as the test asked")),
        }
    }
}

impl Platform for Spy {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn capabilities(&self) -> &'static [Capability] {
        self.inner.capabilities()
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.inner.rename(from, to)
    }
    fn replace_file(&self, temp: &Path, target: &Path) -> Result<()> {
        self.swaps.fetch_add(1, Ordering::SeqCst);
        if self.fail_swap {
            return Err(Self::refuse(target, "the swap"));
        }
        if self.strand_swap {
            std::fs::remove_file(target).map_err(|e| PlatformError::Io {
                path: target.to_path_buf(),
                source: e,
            })?;
            return Err(PlatformError::ReplacedButNotMoved {
                temp: temp.to_path_buf(),
                target: target.to_path_buf(),
                source: std::io::Error::other("the move back failed, as the test asked"),
            });
        }
        self.inner.replace_file(temp, target)
    }
    fn get_attributes(&self, path: &Path) -> Result<FileAttributes> {
        self.inner.get_attributes(path)
    }
    fn set_attributes(&self, path: &Path, change: AttributeChange) -> Result<()> {
        self.inner.set_attributes(path, change)
    }
    fn get_times(&self, path: &Path) -> Result<FileTimes> {
        self.inner.get_times(path)
    }
    fn set_times(&self, path: &Path, change: TimeChange) -> Result<()> {
        if self.fail_dates {
            return Err(Self::refuse(path, "setting the dates"));
        }
        self.inner.set_times(path, change)
    }
    fn naming_rules(&self, path: &Path) -> &'static NamingRules {
        self.inner.naming_rules(path)
    }
    fn case_sensitivity(&self, dir: &Path) -> CaseSensitivity {
        self.inner.case_sensitivity(dir)
    }
    fn reveal_in_file_manager(&self, path: &Path) -> Result<()> {
        self.inner.reveal_in_file_manager(path)
    }
    fn notify_shell_changed(&self, path: &Path) {
        self.notified.lock().unwrap().push(path.to_path_buf());
        self.inner.notify_shell_changed(path);
    }
}
