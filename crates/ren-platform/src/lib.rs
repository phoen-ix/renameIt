//! OS-specific behaviour for RenameIt, behind a single trait.
//!
//! **D3:** this crate is the *only* place `cfg(windows)` may appear. `ren-core`
//! must build and test green on Linux at all times.
//!
//! **P5:** capabilities a platform does not have return
//! [`PlatformError::CapabilityUnsupported`] — never a silent no-op. Setting the
//! created date on Linux is the motivating case.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

pub mod guarded;
pub mod list_file;
pub mod naming;
pub mod shell;
pub mod visibility;

pub use naming::{NameProblem, NamingRules, POSIX, WINDOWS};
pub use visibility::{Visibility, visibility};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// A thing a platform may or may not be able to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    CreatedTime,
    AccessedTime,
    ModifiedTime,
    ReadOnlyAttribute,
    HiddenAttribute,
    SystemAttribute,
    ArchiveAttribute,
    RevealInFileManager,
    /// The file manager's *"Open with RenameIt"* entry (D7).
    ShellContextMenu,
    /// Making a symbolic link *itself* read-only (D205, D241).
    ///
    /// Separate from [`Self::ReadOnlyAttribute`] because the two differ on
    /// Unix: `chmod` follows a link to its target and Linux has no `lchmod`,
    /// so a link has no read-only setting of its own. The planner asks for it
    /// only on a row that is a link and only for *setting* read-only, so the
    /// refusal shows in the preview rather than part-way through a run.
    LinkReadOnly,
}

impl Capability {
    /// Every capability, so a caller can ask about all of them without
    /// hand-listing the set and silently missing the next one added.
    pub const ALL: [Self; 10] = [
        Self::CreatedTime,
        Self::AccessedTime,
        Self::ModifiedTime,
        Self::ReadOnlyAttribute,
        Self::HiddenAttribute,
        Self::SystemAttribute,
        Self::ArchiveAttribute,
        Self::RevealInFileManager,
        Self::ShellContextMenu,
        Self::LinkReadOnly,
    ];
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::CreatedTime => "setting the created date",
            Self::AccessedTime => "setting the accessed date",
            Self::ModifiedTime => "setting the modified date",
            Self::ReadOnlyAttribute => "the read-only attribute",
            Self::HiddenAttribute => "the hidden attribute",
            Self::SystemAttribute => "the system attribute",
            Self::ArchiveAttribute => "the archive attribute",
            Self::RevealInFileManager => "revealing a file in the file manager",
            Self::ShellContextMenu => "a file manager context-menu entry",
            Self::LinkReadOnly => "making a symbolic link read-only",
        };
        f.write_str(s)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{capability} is not supported on {platform}")]
    CapabilityUnsupported {
        capability: Capability,
        platform: &'static str,
    },
    /// The rename target already exists. We never silently overwrite — unlike
    /// `std::fs::rename`, which is exactly why `Platform::rename` exists.
    #[error("{path} already exists")]
    TargetExists { path: PathBuf },
    /// [`Platform::replace_file`] removed `target` and could not move `temp`
    /// into its place: the file's contents now exist **only** at `temp`.
    ///
    /// Distinct from [`Self::Io`] because the caller must not do what it does
    /// after any other failed swap, which is delete `temp` as a copy nobody
    /// needs. Windows reports this as `ERROR_UNABLE_TO_MOVE_REPLACEMENT` when
    /// something — a sync client, a virus scanner — holds the new file open.
    #[error(
        "{target} was taken out of its place and its contents could not be moved back in: they \
         are at {temp} — rename that file to {target} to get it back ({source})"
    )]
    ReplacedButNotMoved {
        temp: PathBuf,
        target: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, PlatformError>;

impl PlatformError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// The four DOS attribute bits a rename may set.
///
/// **No `deny_unknown_fields`**, unlike every operation struct: this one goes
/// into the journal, and a journal is read by builds other than the one that
/// wrote it. A field added here in a later version would otherwise make an
/// older build call the whole file corrupt — the same hazard D73 closed one
/// level up, and `Journal::read` runs as the application opens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FileAttributes {
    pub read_only: bool,
    pub hidden: bool,
    pub system: bool,
    pub archive: bool,
}

/// Tri-state attribute edit: `None` leaves the bit alone.
///
/// A journal payload, so it tolerates fields it does not know — see
/// [`FileAttributes`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AttributeChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive: Option<bool>,
}

impl AttributeChange {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    pub fn apply_to(self, base: FileAttributes) -> FileAttributes {
        FileAttributes {
            read_only: self.read_only.unwrap_or(base.read_only),
            hidden: self.hidden.unwrap_or(base.hidden),
            system: self.system.unwrap_or(base.system),
            archive: self.archive.unwrap_or(base.archive),
        }
    }

    /// Which capabilities this edit actually needs, so a platform can reject
    /// precisely the parts it cannot do.
    pub fn required_capabilities(self) -> Vec<Capability> {
        let mut v = Vec::new();
        if self.read_only.is_some() {
            v.push(Capability::ReadOnlyAttribute);
        }
        if self.hidden.is_some() {
            v.push(Capability::HiddenAttribute);
        }
        if self.system.is_some() {
            v.push(Capability::SystemAttribute);
        }
        if self.archive.is_some() {
            v.push(Capability::ArchiveAttribute);
        }
        v
    }

    /// Which capabilities this edit needs *given what the file already is*.
    ///
    /// A bit already at the requested value needs nothing, because setting it
    /// changes nothing by definition. That is not a loophole in P5 — P5 forbids
    /// silently failing to make a change the user asked for, and there is no
    /// change here to fail at.
    ///
    /// It is what makes undo work on Linux. `get_attributes` reports `hidden`
    /// from a leading dot, so reading a dotfile's attributes and writing them
    /// back — precisely what undo does — would otherwise be rejected for asking
    /// to set a bit that is already set.
    pub fn required_capabilities_from(self, base: FileAttributes) -> Vec<Capability> {
        let mut v = Vec::new();
        let mut want = |field: Option<bool>, current: bool, capability| {
            if field.is_some_and(|wanted| wanted != current) {
                v.push(capability);
            }
        };
        want(
            self.read_only,
            base.read_only,
            Capability::ReadOnlyAttribute,
        );
        want(self.hidden, base.hidden, Capability::HiddenAttribute);
        want(self.system, base.system, Capability::SystemAttribute);
        want(self.archive, base.archive, Capability::ArchiveAttribute);
        v
    }
}

/// Timestamps as read from disk. `None` means the filesystem did not report it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileTimes {
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub modified: Option<SystemTime>,
}

/// Timestamp edit: `None` leaves that component alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeChange {
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub modified: Option<SystemTime>,
}

impl TimeChange {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// Which capabilities this edit needs, mirroring [`AttributeChange`].
    ///
    /// Created comes first deliberately: on Linux it is the one that cannot be
    /// done, and a user who ticked all three should read the documented
    /// limitation rather than whichever field happened to be checked first.
    pub fn required_capabilities(self) -> Vec<Capability> {
        let mut v = Vec::new();
        if self.created.is_some() {
            v.push(Capability::CreatedTime);
        }
        if self.accessed.is_some() {
            v.push(Capability::AccessedTime);
        }
        if self.modified.is_some() {
            v.push(Capability::ModifiedTime);
        }
        v
    }
}

/// What [`Platform::case_sensitivity`] found.
///
/// Implemented and deliberately **not called by the planner** (P96): the
/// probe writes a file into the folder it is asked about, and the preview
/// reruns on every keystroke. The planner folds case from the static
/// [`NamingRules::case_insensitive`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseSensitivity {
    Sensitive,
    Insensitive,
    /// Could not be probed: the folder refused the probe's write.
    Unknown,
}

/// Everything RenameIt needs from the operating system.
pub trait Platform: Send + Sync + fmt::Debug {
    /// Short identifier used in error messages and journals.
    fn name(&self) -> &'static str;

    /// Everything this platform can actually do.
    ///
    /// Constant per platform and free to ask — no probing, no syscall — so the
    /// planner can pre-flight a whole batch (P4) and the GUI can grey a control
    /// the user's machine cannot honour. It must agree with what `set_times`
    /// and `set_attributes` really accept;
    /// `every_capability_the_platform_claims_it_can_do_it_actually_does` in
    /// `tests/platform.rs` is the guard.
    fn capabilities(&self) -> &'static [Capability];

    fn supports(&self, capability: Capability) -> bool {
        self.capabilities().contains(&capability)
    }

    /// The first thing in `wanted` this platform cannot do.
    ///
    /// This is what turns "half the batch ran, then Linux said no" into a
    /// blocked plan with a reason the user can read before pressing anything.
    fn unsupported(&self, wanted: &[Capability]) -> Option<Capability> {
        wanted.iter().copied().find(|c| !self.supports(*c))
    }

    /// Renames `from` to `to`, **failing** with [`PlatformError::TargetExists`]
    /// if `to` already exists.
    ///
    /// `std::fs::rename` silently replaces the destination on every platform,
    /// which for a batch renamer means silent data loss.
    ///
    /// Atomic on Windows (`MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`)
    /// and on Linux (`renameat2` with `RENAME_NOREPLACE`, P13). A Linux
    /// filesystem that refuses that flag — NFS, SMB, some FUSE — gets
    /// `link` + `unlink` for a file, which is just as atomic about an existing
    /// target, and a probe-then-rename only for a folder or where hard links
    /// are refused too; that last path, and other Unixes, keep a window of
    /// microseconds in which a file created by another process is replaced.
    ///
    /// A case-only rename on a case-insensitive volume — `a.JPG` to `a.jpg`,
    /// where the two names are one file — is a rename, not a collision, and
    /// it really happens: on Linux it goes through a temporary name, because
    /// the kernel treats a rename onto the same file as already done.
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;

    /// Puts the contents of `temp` in place of `target`, which must exist,
    /// in one step.
    ///
    /// For rewriting a file's contents without a moment in which it is half
    /// written: the new bytes go to a sibling `temp` in the same folder, and
    /// this swaps it in. A crash leaves either the old file or the new one.
    ///
    /// **What survives differs, which is why it is a platform call.** On
    /// Windows it is `ReplaceFileW`, which keeps what makes `target` the file
    /// it is — its created date, its ACLs, its alternate data streams, its
    /// attributes. On Unix it is `rename`: the file at `temp` becomes
    /// `target` exactly as it is, with `temp`'s permissions, owner and times,
    /// so a caller copies across whatever it wants kept *before* the swap.
    ///
    /// Refuses when `target` does not exist, on every platform, so that the
    /// two agree: `ReplaceFileW` cannot create a file, and a caller that
    /// wanted one created has asked for something else.
    ///
    /// Any error leaves `target` as it was, except
    /// [`PlatformError::ReplacedButNotMoved`]: `target` is gone and `temp`
    /// holds the only copy of the file, which the caller must keep and name.
    fn replace_file(&self, temp: &Path, target: &Path) -> Result<()>;

    fn get_attributes(&self, path: &Path) -> Result<FileAttributes>;
    fn set_attributes(&self, path: &Path, change: AttributeChange) -> Result<()>;

    fn get_times(&self, path: &Path) -> Result<FileTimes>;
    fn set_times(&self, path: &Path, change: TimeChange) -> Result<()>;

    /// Naming rules for the volume `path` lives on.
    fn naming_rules(&self, path: &Path) -> &'static NamingRules;

    /// Whether `path` is somewhere the operating system needs left alone.
    ///
    /// System locations — the Windows folder, the program folders and their
    /// equivalents — that a batch rename must not be turned loose on.
    ///
    /// The one way this tool can break a machine badly enough that undo does
    /// not help: a rename inside `C:\Windows` or `/usr` succeeds, the journal
    /// records it faithfully, and the system stops booting before anybody
    /// presses Ctrl+Z. So it is refused by default and overridable, rather than
    /// hidden — the user who means it can say so (**D127**).
    ///
    /// True for the folder itself and for anything under it. Compared through
    /// the same fold `naming_rules` gives, so `c:\windows` and `C:\Windows`
    /// are one answer.
    fn is_system_folder(&self, path: &Path) -> bool {
        guarded::is_system_folder(path, self.naming_rules(path))
    }

    /// Whether names differing only in case collide in `dir`.
    fn case_sensitivity(&self, dir: &Path) -> CaseSensitivity;

    fn reveal_in_file_manager(&self, path: &Path) -> Result<()>;

    /// Best-effort notification that the folder holding `path` changed, so
    /// the OS file manager refreshes it. Never fails.
    ///
    /// The *parent*, whatever `path` is: a renamed file or folder changes the
    /// listing it appears in. Taken from the path's text rather than by
    /// asking the disk whether `path` is a folder, because this runs once per
    /// change.
    fn notify_shell_changed(&self, path: &Path);

    /// Whether the file manager's RenameIt menu is installed, in the shape
    /// *this* build writes — `None` where there is no such thing to install
    /// (D7, D132).
    ///
    /// Read from the system rather than remembered. A menu can be removed by
    /// another install of the program, by a registry cleaner, or by hand, and a
    /// remembered `true` would then be a Settings page confidently describing
    /// something that is not there.
    fn context_menu_installed(&self) -> Option<bool> {
        None
    }

    /// Installs or removes it, with one item per preset. `HKCU` only, so no
    /// administrator and nothing an installer has to own.
    ///
    /// **The presets are a parameter because the menu is static.** Nothing in
    /// the registry can enumerate a folder at right-click time, so the item
    /// list is a snapshot: every call replaces the whole menu with the list it
    /// was given, and a preset added or deleted since the last call is only
    /// reflected by calling again.
    fn set_context_menu(&self, _installed: bool, _presets: &[shell::MenuPreset<'_>]) -> Result<()> {
        Err(PlatformError::CapabilityUnsupported {
            capability: Capability::ShellContextMenu,
            platform: self.name(),
        })
    }
}

/// Per-user directory for application data — journals now, settings later.
///
/// Hand-rolled rather than taken from the `directories` crate, whose
/// `option-ext` dependency is MPL-2.0 and therefore outside the **D2**
/// allowlist. "Where does this OS keep application data" is exactly the kind of
/// question **D3** says belongs in this crate anyway.
///
/// * Windows — `%APPDATA%\<app>` (roaming)
/// * macOS — `~/Library/Application Support/<app>`
/// * Linux and other Unix — `$XDG_DATA_HOME/<app>`, else `~/.local/share/<app>`
///
/// Falls back to a per-app directory under the system temp dir if the
/// environment says nothing useful, so callers never have to handle `None`.
pub fn app_data_dir(app: &str) -> PathBuf {
    // A portable install answers this differently, and answers it once: the
    // journal, the presets and the scripts all come through here, and an app
    // that wrote its journal beside the exe and its presets under `%APPDATA%`
    // would be neither portable nor normal (D131).
    if let Some(root) = PORTABLE_ROOT.get() {
        return root.clone();
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|p| PathBuf::from(p).join("AppData").join("Roaming"))
        })
    } else if cfg!(target_os = "macos") {
        home_dir().map(|h| h.join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home_dir().map(|h| h.join(".local").join("share")))
    };
    match base {
        Some(base) => base.join(app),
        None => std::env::temp_dir().join(app),
    }
}

/// Where a portable install keeps everything, set once at startup.
static PORTABLE_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Redirects [`app_data_dir`] to `root`, for a portable install.
///
/// Called from `main` before anything reads a path, and callable exactly once —
/// a second call returns `Err` rather than silently winning or silently losing,
/// because half the app writing to one place and half to another is worse than
/// either.
///
/// A global, deliberately. "Where does this installation keep its files" is a
/// property of the process, decided before the first read, and threading it
/// through `PresetStore::user`, `default_journal_dir` and `ScriptStore::user`
/// would put an argument on three constructors so that one caller could set it.
pub fn use_portable_root(root: PathBuf) -> std::result::Result<(), PathBuf> {
    PORTABLE_ROOT.set(root)
}

/// Whether this process is running portably.
pub fn is_portable() -> bool {
    PORTABLE_ROOT.get().is_some()
}

/// The file that marks an installation portable, beside the executable.
///
/// Its **presence** is the switch, so making an install portable is copying a
/// folder and touching a file — no registry, no installer, nothing to
/// uninstall. Its contents are ignored.
pub const PORTABLE_MARKER: &str = "renameit-portable.txt";

/// Decides whether this is a portable install, and where its files go.
///
/// Two conditions, both required. The marker file must sit beside the
/// executable, and that directory must be **writable** — a copy on a read-only
/// share or in `Program Files` would otherwise take the portable path and then
/// fail on every write, which is a worse outcome than quietly using `%APPDATA%`.
///
/// Returns the data root to pass to [`use_portable_root`].
pub fn portable_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    portable_root_in(exe.parent()?)
}

/// [`portable_root`] for an executable that lives in `dir`, so the decision
/// is testable without moving the test binary.
fn portable_root_in(dir: &Path) -> Option<PathBuf> {
    if !dir.join(PORTABLE_MARKER).exists() {
        return None;
    }
    // Tested by writing, not by reading permissions: a read-only mount, an
    // ACL, and a full disk all present differently and all mean the same thing
    // here.
    let probe = dir.join(".renameit-write-probe");
    std::fs::write(&probe, b"").ok()?;
    let _ = std::fs::remove_file(&probe);
    Some(dir.join("RenameIt-data"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// How many characters the OS handed this process as its command line.
///
/// `None` where the question has no answer worth acting on, which is
/// everywhere but Windows.
///
/// **Not derivable from `std::env::args`.** By the time that has split and
/// unquoted, the count it implies is smaller than the one Windows measured — and
/// the number that matters is the one Windows measured, because that is what it
/// compares against the cap.
///
/// The cap: a static verb is *"a call to `CreateProcess` with the selected files
/// … passed as the command line"*, and the shell stops at
/// [`SHELL_COMMAND_LINE_LIMIT`]. Past it the menu entry does not appear at all —
/// no error, no shortened list — which is why the caller can only ever say what
/// it received and what the ceiling is.
pub fn command_line_chars() -> Option<usize> {
    #[cfg(windows)]
    {
        Some(command_line_wide().len())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The command line exactly as Windows handed it over, before any splitting.
#[cfg(windows)]
fn command_line_wide() -> &'static [u16] {
    // SAFETY: `GetCommandLineW` returns a pointer to a static, NUL-terminated
    // buffer owned by the process for its lifetime. It never fails and never
    // returns null.
    let ptr = unsafe { windows_sys::Win32::System::Environment::GetCommandLineW() };
    let mut len = 0usize;
    // SAFETY: the buffer above is NUL-terminated, so this stops.
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: the `len` units before the terminator were each just read, and
    // the buffer lives as long as the process.
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

/// This process's command line as one unsplit string, where there is such a
/// thing — Windows. `None` elsewhere, where a process receives its arguments
/// already split and there is nothing to re-read.
///
/// For [`split_verbatim`]; see there for why a caller wants it.
pub fn raw_command_line() -> Option<OsString> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        Some(OsString::from_wide(command_line_wide()))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Splits a command line the way a batch file's author, or the Explorer menu
/// writing `"%V"`, means it: whitespace separates, `"` toggles quoting, and
/// **nothing escapes anything**.
///
/// `std::env::args_os` follows the C runtime's rule instead, in which a
/// backslash before a quote escapes it. That rule can never be what a Windows
/// path means — `"` is not allowed in one — and it breaks the two commonest
/// quoted paths that end in a backslash: a drive root, `"E:\"`, arrives as
/// `E:"` with the quote still open, and a batch file's `"%~dp0"` swallows
/// every switch after it. Split this way, both come out whole.
///
/// Pure, so it is tested everywhere; only [`raw_command_line`] is Windows.
pub fn split_verbatim(line: &OsStr) -> Vec<OsString> {
    let mut args = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut in_quotes = false;
    // `""` is an empty argument, not no argument.
    let mut started = false;
    for &byte in line.as_encoded_bytes() {
        match byte {
            b'"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            b' ' | b'\t' if !in_quotes => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            _ => {
                current.push(byte);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args.into_iter()
        // SAFETY: every piece is bytes from `as_encoded_bytes` on this
        // platform, cut only next to an ASCII byte (a quote or a separator),
        // which the encoding guarantees is a character boundary.
        .map(|piece| unsafe { OsString::from_encoded_bytes_unchecked(piece) })
        .collect()
}

/// What the shell will pass a static verb, in characters.
///
/// Microsoft's documented ceiling for a context-menu verb's command line. It is
/// not the 32767 of `CreateProcess` itself — the shell applies its own, lower
/// limit before it gets there.
pub const SHELL_COMMAND_LINE_LIMIT: usize = 2000;

/// The platform implementation for the machine we are running on.
pub fn host() -> Arc<dyn Platform> {
    static HOST: OnceLock<Arc<dyn Platform>> = OnceLock::new();
    HOST.get_or_init(|| {
        #[cfg(windows)]
        {
            Arc::new(windows::WindowsPlatform::new())
        }
        #[cfg(unix)]
        {
            Arc::new(unix::UnixPlatform::new())
        }
    })
    .clone()
}

/// Reads created/accessed/modified through `std::fs`, which every supported
/// platform can do (reading a birth time works on Linux via `statx`; only
/// *writing* it is Windows-only — see P5).
///
/// **A link's own times, never its target's.** A listing shows a symbolic
/// link as the link it is and a rename renames the link, so a date read here
/// describes that same row — and the setters write to the link as well.
pub(crate) fn read_times(path: &Path) -> Result<FileTimes> {
    let md = std::fs::symlink_metadata(path).map_err(|e| PlatformError::io(path, e))?;
    Ok(FileTimes {
        created: md.created().ok(),
        accessed: md.accessed().ok(),
        modified: md.modified().ok(),
    })
}

/// Probes a directory by creating a lower-case file and looking for it under an
/// upper-case name. Cached per directory — probing is a filesystem write.
#[derive(Debug, Default)]
pub(crate) struct CaseProbeCache {
    seen: Mutex<HashMap<PathBuf, CaseSensitivity>>,
}

impl CaseProbeCache {
    pub(crate) fn get(&self, dir: &Path) -> CaseSensitivity {
        let dir = if dir.is_dir() {
            dir.to_path_buf()
        } else {
            dir.parent().unwrap_or(Path::new(".")).to_path_buf()
        };
        if let Some(hit) = self.seen.lock().unwrap().get(&dir) {
            return *hit;
        }
        let verdict = probe(&dir);
        self.seen.lock().unwrap().insert(dir, verdict);
        verdict
    }
}

fn probe(dir: &Path) -> CaseSensitivity {
    let pid = std::process::id();
    let lower = dir.join(format!(".renameit-case-probe-{pid}"));
    let upper = dir.join(format!(".RENAMEIT-CASE-PROBE-{pid}"));
    if std::fs::File::create(&lower).is_err() {
        return CaseSensitivity::Unknown;
    }
    let verdict = if upper.exists() {
        CaseSensitivity::Insensitive
    } else {
        CaseSensitivity::Sensitive
    };
    let _ = std::fs::remove_file(&lower);
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_change_leaves_untouched_bits_alone() {
        let base = FileAttributes {
            read_only: true,
            hidden: false,
            system: true,
            archive: false,
        };
        let change = AttributeChange {
            hidden: Some(true),
            ..Default::default()
        };
        assert_eq!(
            change.apply_to(base),
            FileAttributes {
                read_only: true,
                hidden: true,
                system: true,
                archive: false,
            }
        );
        assert_eq!(
            change.required_capabilities(),
            vec![Capability::HiddenAttribute]
        );
    }

    #[test]
    fn empty_changes_are_recognised_as_no_ops() {
        assert!(AttributeChange::default().is_empty());
        assert!(TimeChange::default().is_empty());
    }

    #[test]
    fn app_data_dir_is_absolute_and_ends_with_the_app_name() {
        let dir = app_data_dir("RenameIt");
        assert!(dir.is_absolute(), "{dir:?} should be absolute");
        assert_eq!(dir.file_name().unwrap(), "RenameIt");
    }
}

#[cfg(test)]
mod portable_tests {
    use super::*;

    /// Two conditions, both required — tested through the function
    /// `portable_root` actually calls, so a change to the rule fails here.
    #[test]
    fn a_marker_in_a_writable_folder_is_what_makes_an_install_portable() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(
            portable_root_in(dir.path()),
            None,
            "no marker, not portable"
        );

        std::fs::write(dir.path().join(PORTABLE_MARKER), b"").unwrap();
        assert_eq!(
            portable_root_in(dir.path()),
            Some(dir.path().join("RenameIt-data"))
        );
        assert!(
            !dir.path().join(".renameit-write-probe").exists(),
            "the write probe cleans up after itself"
        );
    }

    /// A copy on a read-only share must **not** take the portable path: it
    /// would then fail on every write, which is worse than quietly using the
    /// per-user folder.
    #[test]
    #[cfg(unix)]
    fn a_read_only_folder_is_not_portable_however_it_is_marked() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(PORTABLE_MARKER), b"").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let verdict = portable_root_in(dir.path());
        let writable_anyway = std::fs::write(dir.path().join("root-check"), b"").is_ok();

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        if writable_anyway {
            // Root ignores the mode bits, so there is nothing to observe.
            eprintln!("skipped: running as root, so the folder is writable anyway");
            return;
        }
        assert_eq!(
            verdict, None,
            "a read-only folder cannot hold a portable install"
        );
    }

    /// Nothing here is portable unless something says so, which is what keeps
    /// every other test in the workspace looking at the per-user folder.
    #[test]
    fn an_ordinary_process_is_not_portable() {
        assert!(!is_portable());
        assert!(app_data_dir("RenameIt").is_absolute());
    }
}

#[cfg(test)]
mod command_line_tests {
    use super::*;

    fn split(line: &str) -> Vec<String> {
        split_verbatim(OsStr::new(line))
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect()
    }

    /// The lines the C runtime's rule breaks, whole again.
    #[test]
    fn a_quoted_path_ending_in_a_backslash_keeps_it() {
        assert_eq!(
            split(r#"renameit.exe --from-shell --start-in "E:\""#),
            ["renameit.exe", "--from-shell", "--start-in", r"E:\"]
        );
        assert_eq!(
            split(r#"ren-cli.exe /p "C:\Scripts\" /r "my preset""#),
            ["ren-cli.exe", "/p", r"C:\Scripts\", "/r", "my preset"]
        );
        // Explorer's per-item quoting of two drive roots.
        assert_eq!(
            split(r#"x --from-shell "E:\" "F:\""#),
            ["x", "--from-shell", r"E:\", r"F:\"]
        );
    }

    #[test]
    fn whitespace_separates_and_quotes_group() {
        assert_eq!(
            split("  a\tb  \"c d\"e \"\"  "),
            ["a", "b", "c de", ""],
            "a run of whitespace is one separator, and a bare pair of quotes is \
             an empty argument"
        );
        assert_eq!(
            split(r#""C:\Program Files\x.exe" y"#),
            [r"C:\Program Files\x.exe", "y"]
        );
    }

    /// Only Windows has an unsplit line to give.
    #[test]
    fn there_is_a_raw_command_line_only_on_windows() {
        assert_eq!(raw_command_line().is_some(), cfg!(windows));
        assert_eq!(command_line_chars().is_some(), cfg!(windows));
    }
}
