//! The update check, the download and the handover to the installer (D250).
//!
//! **Which release is the latest** comes from where
//! `github.com/<repo>/releases/latest` redirects to, read without following it.
//! That is one request with no API behind it. The GitHub API allows 60
//! unauthenticated requests an hour per IP, which a whole office behind one
//! address can use up.
//!
//! **What is downloaded** is never discovered. The asset names are fixed
//! ([`installer_asset`], [`SUMS`]) and so are the URLs ([`asset_url`]), and
//! `tests/installer_contract.rs` holds `release.yml` to them. `SHA256SUMS.txt`
//! is fetched first, so a release whose files are still uploading is refused
//! before anything large moves. The installer is then checked against it.
//! That catches a truncated or corrupted download. It does not catch a
//! compromised repository, because the list and the installer come from the
//! same release; signing would close that, and there is none yet.
//!
//! **Nothing goes online until the user asks** ([`UpdateMode::Manual`]).
//!
//! The parts that decide anything are plain functions, tested on every
//! platform. The rest is a worker thread in the shape `ApplyWorker` set: an
//! mpsc channel each way, and a repaint when an answer lands.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use ren_platform::{Capability, Platform};
use serde::{Deserialize, Serialize};

/// What every update download folder under `%TEMP%` is called, followed by
/// something unique. The uninstaller deletes `$TEMP\RenameIt-update-*`.
pub const TEMP_PREFIX: &str = "RenameIt-update-";

/// The checksum list every release carries, in `sha256sum`'s format.
pub const SUMS: &str = "SHA256SUMS.txt";

/// What the installer is passed: silent, and in the scope that is installed
/// already. `/RESTART` is added for an update the user asked for, so the app
/// comes back; the one the Automatic mode runs at exit does not reopen the
/// window the user just closed. `packaging/renameit.nsi` parses each of
/// these, which `tests/installer_contract.rs` checks.
pub const SILENT: &str = "/S";
pub const PER_USER: &str = "/CurrentUser";
pub const PER_MACHINE: &str = "/AllUsers";
pub const RESTART: &str = "/RESTART";

const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const DAY: i64 = 24 * 60 * 60;
/// How long an Automatic install that did not take is left before the same
/// version is tried again. Without it, an installer that fails every time
/// would run on every exit.
const RETRY_AFTER: i64 = 7 * DAY;

/// A release number, `major.minor.patch`.
///
/// Three numbers and nothing else, so a pre-release tag such as `v2.0.0-rc1`
/// does not parse and is never offered. `releases/latest` skips pre-releases
/// anyway; this keeps one that slipped through from being installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.').map(|part| {
            // `u64::from_str` takes a leading `+`, which no tag has.
            part.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| part.parse().ok())
                .flatten()
        });
        let version = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    /// The version of this build.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the workspace version is major.minor.patch")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Settings ▸ Updates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateMode {
    /// Nothing goes online until *Check now*.
    #[default]
    Manual,
    /// A check once a day, and a notice when there is something newer.
    Notify,
    /// The same check, and a newer release is downloaded, verified and
    /// installed when RenameIt is closed. Only for a per-user install: one
    /// for all users needs the administrator prompt, which is not something
    /// to spring on somebody closing a window.
    Automatic,
}

/// The page that redirects to the newest release.
pub fn latest_url() -> String {
    format!("{REPO}/releases/latest")
}

/// The page of one release, for a copy the app cannot update itself.
pub fn release_page(version: Version) -> String {
    format!("{REPO}/releases/tag/v{version}")
}

/// The installer a release carries.
pub fn installer_asset(version: Version) -> String {
    format!("RenameIt-Setup-{version}.exe")
}

pub fn asset_url(version: Version, name: &str) -> String {
    format!("{REPO}/releases/download/v{version}/{name}")
}

/// The release a `releases/latest` redirect points at.
///
/// Strict, because this decides what gets downloaded: https, github.com, this
/// repository, `/releases/tag/v<version>` and nothing after. The owner and
/// repository are compared ignoring case, since GitHub treats them that way.
pub fn version_from_location(location: &str) -> Option<Version> {
    let repo = REPO.strip_prefix("https://github.com/")?;
    let path = location.strip_prefix("https://github.com/")?;
    let (owner_repo, rest) = path.split_at_checked(repo.len())?;
    if !owner_repo.eq_ignore_ascii_case(repo) {
        return None;
    }
    Version::parse(rest.strip_prefix("/releases/tag/v")?)
}

/// A `SHA256SUMS.txt`, as a map from file name to digest.
///
/// `sha256sum`'s two forms are both read: `<hex>  <name>`, and `<hex> *<name>`
/// for binary mode. Windows line endings and a byte-order mark are tolerated.
/// Anything else — an HTML error page, a truncated line — refuses the whole
/// file. So does a name listed twice with two different digests, which
/// cannot be told apart from tampering.
pub fn parse_sums(text: &str) -> Result<HashMap<String, [u8; 32]>, String> {
    let refuse = |line: &str| {
        Err(format!(
            "{SUMS} has a line that is not a checksum: {line:?}"
        ))
    };
    let mut sums = HashMap::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((hex, name)) = line.split_once("  ").or_else(|| line.split_once(" *")) else {
            return refuse(line);
        };
        let Some(digest) = digest_from_hex(hex) else {
            return refuse(line);
        };
        if name.is_empty() {
            return refuse(line);
        }
        if sums
            .insert(name.to_owned(), digest)
            .is_some_and(|old| old != digest)
        {
            return Err(format!(
                "{SUMS} lists {name} twice, with two different checksums"
            ));
        }
    }
    if sums.is_empty() {
        return Err(format!("{SUMS} is empty"));
    }
    Ok(sums)
}

fn digest_from_hex(hex: &str) -> Option<[u8; 32]> {
    // Checked first: `from_str_radix` would take a `+` sign.
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut digest = [0u8; 32];
    for (byte, pair) in digest.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(digest)
}

/// A scheduled check is due a day after the last one, or at once if the
/// clock says the last one was in the future.
pub fn check_due(last: Option<i64>, now: i64) -> bool {
    last.is_none_or(|last| now - last >= DAY || last > now)
}

/// Whether the Automatic mode may install `version`, given the last version
/// it tried and when.
pub fn may_auto_install(attempt: Option<&(String, i64)>, version: Version, now: i64) -> bool {
    attempt.is_none_or(|(tried, at)| {
        *tried != version.to_string() || now - at >= RETRY_AFTER || *at > now
    })
}

/// How this copy came to be here, which decides what *Update* can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    /// No update check on this platform.
    Unsupported,
    /// Not put here by the installer: a portable copy, or a build run from
    /// `target`. Told about a release, and pointed at its page.
    Portable,
    PerUser,
    /// For all users. Updating needs the administrator prompt.
    PerMachine,
}

/// What the updater knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Nothing asked yet this session.
    Idle,
    Checking,
    UpToDate,
    Available(Version),
    Downloading(Version),
    /// Downloaded and verified, waiting for the window to close (Automatic).
    Ready(Version),
    Failed(String),
}

enum Request {
    Check,
    Download(Version),
}

enum Answer {
    Checked(Result<Option<Version>, String>),
    Downloaded(Version, Result<PathBuf, String>),
}

/// The updater, owned by the app.
pub struct Updates {
    platform: Arc<dyn Platform>,
    requests: Sender<Request>,
    answers: Receiver<Answer>,
    pub mode: UpdateMode,
    /// When a check last went out, in seconds since 1970. Stamped whether or
    /// not it succeeded, so a machine with no network asks once a day rather
    /// than every frame.
    pub last_check: Option<i64>,
    /// The version the Automatic mode last downloaded to install, and when.
    pub auto_attempt: Option<(String, i64)>,
    install: Install,
    policy_off: bool,
    status: Status,
    /// The verified installer, while `status` is `Ready`.
    installer: Option<PathBuf>,
    /// The user pressed *Update* and is waiting: install as soon as the
    /// download lands, rather than at exit.
    install_when_ready: bool,
    /// An installer was started, and the window must close so it can
    /// replace the program. Read with [`Self::take_close`].
    close: bool,
}

impl fmt::Debug for Updates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Updates")
            .field("mode", &self.mode)
            .field("install", &self.install)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Updates {
    /// Spawns the worker. Downloads go into a new folder under `temp`.
    pub fn spawn(
        platform: Arc<dyn Platform>,
        temp: PathBuf,
        repaint: impl Fn() + Send + 'static,
    ) -> Self {
        let (request_tx, request_rx) = channel::<Request>();
        let (answer_tx, answer_rx) = channel::<Answer>();
        let worker = platform.clone();
        std::thread::Builder::new()
            .name("renameit-update".into())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    let answer = match request {
                        Request::Check => Answer::Checked(caught(|| check(worker.as_ref()))),
                        Request::Download(version) => Answer::Downloaded(
                            version,
                            caught(|| download(worker.as_ref(), version, &temp)),
                        ),
                    };
                    if answer_tx.send(answer).is_err() {
                        return; // The UI is gone.
                    }
                    repaint();
                }
            })
            .expect("the update worker thread should start");

        let install = if !platform.supports(Capability::Updates) {
            Install::Unsupported
        } else {
            match platform.install_record() {
                None => Install::Portable,
                Some(record) if record.per_machine => Install::PerMachine,
                Some(_) => Install::PerUser,
            }
        };
        Self {
            policy_off: platform.updates_disabled_by_policy(),
            platform,
            requests: request_tx,
            answers: answer_rx,
            mode: UpdateMode::default(),
            last_check: None,
            auto_attempt: None,
            install,
            status: Status::Idle,
            installer: None,
            install_when_ready: false,
            close: false,
        }
    }

    pub fn install(&self) -> Install {
        self.install
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Off for this machine: no updater on this platform, or an
    /// administrator's policy.
    pub fn disabled(&self) -> bool {
        self.install == Install::Unsupported || self.policy_off
    }

    pub fn policy_off(&self) -> bool {
        self.policy_off
    }

    /// Whether the Automatic mode can do what it says on this copy. Anywhere
    /// else it behaves like Notify.
    pub fn automatic_possible(&self) -> bool {
        self.install == Install::PerUser
    }

    /// Not while a request is out, and not once an installer is downloaded
    /// and waiting for exit: a fresh answer would replace the status that
    /// installer is waiting under.
    pub fn can_check(&self) -> bool {
        !self.disabled() && !self.is_busy() && !matches!(self.status, Status::Ready(_))
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.status, Status::Checking | Status::Downloading(_))
    }

    /// *Check now*.
    pub fn check_now(&mut self, now: i64) {
        if !self.can_check() {
            return;
        }
        self.last_check = Some(now);
        self.status = Status::Checking;
        let _ = self.requests.send(Request::Check);
    }

    /// Whether an installer has been started since the last call, so the
    /// window must close.
    pub fn take_close(&mut self) -> bool {
        std::mem::take(&mut self.close)
    }

    /// Reads what the worker answered, and starts the daily check when it is
    /// due.
    pub fn poll(&mut self, now: i64) {
        while let Ok(answer) = self.answers.try_recv() {
            match answer {
                Answer::Checked(Ok(Some(version))) => {
                    self.status = Status::Available(version);
                    if self.mode == UpdateMode::Automatic
                        && self.automatic_possible()
                        && may_auto_install(self.auto_attempt.as_ref(), version, now)
                    {
                        self.download(version);
                    }
                }
                Answer::Checked(Ok(None)) => self.status = Status::UpToDate,
                Answer::Checked(Err(error)) | Answer::Downloaded(_, Err(error)) => {
                    self.install_when_ready = false;
                    self.status = Status::Failed(error);
                }
                Answer::Downloaded(version, Ok(path)) => {
                    self.installer = Some(path.clone());
                    self.status = Status::Ready(version);
                    if std::mem::take(&mut self.install_when_ready) {
                        self.launch(&path, true);
                    } else {
                        self.auto_attempt = Some((version.to_string(), now));
                    }
                }
            }
        }
        let waiting = self.is_busy() || matches!(self.status, Status::Ready(_));
        if self.mode != UpdateMode::Manual && !waiting && check_due(self.last_check, now) {
            self.check_now(now);
        }
    }

    /// *Update to X* and *Install now*: download if need be, then start the
    /// installer, after which [`Self::take_close`] says so.
    ///
    /// Only for an installed copy; a portable one is sent to the release
    /// page instead, by the caller.
    pub fn install_now(&mut self) {
        match self.status.clone() {
            Status::Ready(_) => {
                if let Some(path) = self.installer.clone() {
                    self.launch(&path, true);
                }
            }
            Status::Available(version) => {
                self.install_when_ready = true;
                self.download(version);
            }
            _ => {}
        }
    }

    /// The Automatic mode's install, as the app exits.
    pub fn on_exit(&mut self) {
        if self.mode == UpdateMode::Automatic
            && self.automatic_possible()
            && matches!(self.status, Status::Ready(_))
            && let Some(path) = self.installer.clone()
        {
            self.launch(&path, false);
        }
    }

    fn download(&mut self, version: Version) {
        self.status = Status::Downloading(version);
        let _ = self.requests.send(Request::Download(version));
    }

    /// Starts the installer. A failure, such as a declined administrator
    /// prompt, becomes the status and the window stays open.
    fn launch(&mut self, installer: &Path, restart: bool) {
        let per_machine = self.install == Install::PerMachine;
        let mut args = vec![SILENT, if per_machine { PER_MACHINE } else { PER_USER }];
        if restart {
            args.push(RESTART);
        }
        match self
            .platform
            .launch_installer(installer, &args, per_machine)
        {
            Ok(()) => self.close = true,
            Err(error) => {
                self.status = Status::Failed(format!("The installer did not start: {error}"));
            }
        }
    }

    /// Blocks until the worker has answered. Tests only, like
    /// `RenameItApp::settle`.
    pub fn settle(&mut self, now: i64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        self.poll(now);
        while self.is_busy() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll(now);
        }
    }
}

/// Seconds since 1970, the unit [`Updates`] keeps its stamps in.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A panic in the worker becomes an error, not a dead thread that never
/// answers and leaves the page saying *Checking…* forever.
fn caught<T>(job: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
        .unwrap_or_else(|payload| Err(crate::viewmodel::preview::panic_message(payload.as_ref())))
}

/// The newest release, if it is newer than this build.
fn check(platform: &dyn Platform) -> Result<Option<Version>, String> {
    let head = platform
        .http_get(&latest_url(), false, &mut std::io::sink())
        .map_err(|error| format!("Could not reach GitHub: {error}"))?;
    let location = head
        .location
        .filter(|_| (300..400).contains(&head.status))
        .ok_or_else(|| {
            format!(
                "GitHub answered {} rather than naming a release",
                head.status
            )
        })?;
    let latest = version_from_location(&location)
        .ok_or_else(|| format!("GitHub pointed at {location}, which is not a RenameIt release"))?;
    Ok((latest > Version::current()).then_some(latest))
}

/// The release's installer, verified, in a folder of its own under `temp`.
/// The folder is deleted again if anything goes wrong.
fn download(platform: &dyn Platform, version: Version, temp: &Path) -> Result<PathBuf, String> {
    sweep(
        temp,
        std::time::SystemTime::now() - std::time::Duration::from_secs(DAY as u64),
    );
    let mut sums = Vec::new();
    let head = platform
        .http_get(&asset_url(version, SUMS), true, &mut sums)
        .map_err(|error| format!("Could not download {SUMS}: {error}"))?;
    if head.status != 200 {
        return Err(format!(
            "{SUMS} for {version} is not there yet (GitHub answered {})",
            head.status
        ));
    }
    let sums = parse_sums(&String::from_utf8_lossy(&sums))?;
    let name = installer_asset(version);
    let expected = *sums
        .get(&name)
        .ok_or_else(|| format!("{SUMS} for {version} does not list {name}"))?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = temp.join(format!("{TEMP_PREFIX}{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let path = dir.join(&name);
    let fetched = (|| {
        let mut file =
            std::fs::File::create(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let head = platform
            .http_get(&asset_url(version, &name), true, &mut file)
            .map_err(|error| format!("Could not download {name}: {error}"))?;
        drop(file);
        if head.status != 200 {
            return Err(format!("{name}: GitHub answered {}", head.status));
        }
        let digest = platform
            .sha256_file(&path)
            .map_err(|error| error.to_string())?;
        if digest != expected {
            return Err(format!(
                "The download of {name} does not match its checksum, so it was thrown away"
            ));
        }
        Ok(path)
    })();
    if fetched.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    fetched
}

/// Deletes download folders older than `cutoff`, a day ago: an installer
/// that ran, or one that never did. Today's are left, in case one is waiting
/// for exit.
fn sweep(temp: &Path, cutoff: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified < cutoff);
        if stale && entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_three_numbers_and_nothing_else() {
        assert_eq!(Version::parse("1.6.0"), Some(Version(1, 6, 0)));
        assert_eq!(Version::parse("10.20.300"), Some(Version(10, 20, 300)));
        for bad in [
            "1.6",
            "1.6.0.1",
            "1.6.0-rc1",
            "v1.6.0",
            "1.+6.0",
            "",
            "1..0",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert!(Version(1, 10, 0) > Version(1, 9, 9), "numeric, not text");
        assert_eq!(Version(1, 7, 2).to_string(), "1.7.2");
    }

    #[test]
    fn the_latest_release_is_read_from_the_redirect_and_nowhere_else() {
        let base = "https://github.com/phoen-ix/renameIt/releases/tag/v";
        assert_eq!(
            version_from_location(&format!("{base}1.7.0")),
            Some(Version(1, 7, 0))
        );
        assert_eq!(
            version_from_location("https://github.com/PHOEN-IX/RENAMEIT/releases/tag/v1.7.0"),
            Some(Version(1, 7, 0)),
            "GitHub ignores case in an owner or repository"
        );
        for bad in [
            "http://github.com/phoen-ix/renameIt/releases/tag/v1.7.0",
            "https://example.com/phoen-ix/renameIt/releases/tag/v1.7.0",
            "https://github.com/someone-else/renameIt/releases/tag/v1.7.0",
            "https://github.com/phoen-ix/renameIt2/releases/tag/v1.7.0",
            "https://github.com/phoen-ix/renameIt/releases/tag/v1.7.0/x",
            "https://github.com/phoen-ix/renameIt/releases/tag/v1.7.0-rc1",
            "https://github.com/phoen-ix/renameIt/releases",
            "/phoen-ix/renameIt/releases/tag/v1.7.0",
        ] {
            assert_eq!(version_from_location(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_asset_urls_are_built_not_discovered() {
        let version = Version(1, 7, 0);
        assert_eq!(installer_asset(version), "RenameIt-Setup-1.7.0.exe");
        assert_eq!(
            asset_url(version, SUMS),
            "https://github.com/phoen-ix/renameIt/releases/download/v1.7.0/SHA256SUMS.txt"
        );
        assert_eq!(
            latest_url(),
            "https://github.com/phoen-ix/renameIt/releases/latest"
        );
    }

    const A: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const B: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_checksum_list_reads_in_both_sha256sum_forms() {
        let sums = parse_sums(&format!(
            "\u{feff}{A}  RenameIt-Setup-1.7.0.exe\r\n{B} *RenameIt-windows-x64.zip\r\n\r\n"
        ))
        .unwrap();
        assert_eq!(sums.len(), 2);
        assert_eq!(sums["RenameIt-Setup-1.7.0.exe"][0], 0xba);
        assert_eq!(sums["RenameIt-windows-x64.zip"][31], 0x55);
    }

    #[test]
    fn anything_that_is_not_a_checksum_list_is_refused_whole() {
        for bad in [
            "<!DOCTYPE html><html>Not Found</html>".to_owned(),
            format!("{}  truncated.exe", &A[..60]),
            format!("{A}  "),
            format!("{A} one-space.exe"),
            format!("{}  bad-hex.exe", A.replace('b', "g")),
            String::new(),
            format!("{A}  same.exe\n{B}  same.exe"),
        ] {
            assert!(parse_sums(&bad).is_err(), "{bad:?}");
        }
        assert!(
            parse_sums(&format!("{A}  same.exe\n{A}  same.exe")).is_ok(),
            "the same line twice is harmless"
        );
    }

    #[test]
    fn a_daily_check_is_due_after_a_day_or_after_the_clock_went_back() {
        assert!(check_due(None, 1_000));
        assert!(!check_due(Some(1_000), 1_000 + DAY - 1));
        assert!(check_due(Some(1_000), 1_000 + DAY));
        assert!(check_due(Some(1_000 + DAY), 1_000), "a stamp in the future");
    }

    #[test]
    fn the_same_version_is_not_retried_for_a_week() {
        let version = Version(1, 7, 0);
        let tried = ("1.7.0".to_owned(), 1_000);
        assert!(may_auto_install(None, version, 1_000));
        assert!(!may_auto_install(
            Some(&tried),
            version,
            1_000 + RETRY_AFTER - 1
        ));
        assert!(may_auto_install(Some(&tried), version, 1_000 + RETRY_AFTER));
        assert!(may_auto_install(Some(&tried), Version(1, 7, 1), 1_000));
    }
}

/// A platform that serves one fake release, for the tests here and the app's.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use ren_platform::update::{HttpHead, InstallRecord};
    use std::sync::Mutex;

    pub const INSTALLER: &[u8] = b"MZ pretend installer";

    #[derive(Debug)]
    pub struct Release {
        inner: Arc<dyn Platform>,
        pub latest: Version,
        pub install: Option<InstallRecord>,
        /// What the checksum list says the installer's digest is.
        pub listed: [u8; 32],
        pub launched: Mutex<Vec<(PathBuf, Vec<String>, bool)>>,
    }

    impl Release {
        pub fn new(latest: Version, install: Option<InstallRecord>) -> Arc<Self> {
            Arc::new(Self {
                inner: ren_platform::host(),
                latest,
                install,
                listed: Self::digest(INSTALLER),
                launched: Mutex::new(Vec::new()),
            })
        }

        /// Not SHA-256, which only Windows computes here; any function of the
        /// bytes will do, since the fake computes both sides.
        pub fn digest(bytes: &[u8]) -> [u8; 32] {
            let mut digest = [0u8; 32];
            for (index, byte) in bytes.iter().enumerate() {
                digest[index % 32] ^= byte.rotate_left(index as u32 % 8);
            }
            digest
        }

        fn hex(digest: [u8; 32]) -> String {
            digest.iter().map(|b| format!("{b:02x}")).collect()
        }
    }

    impl Platform for Release {
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn capabilities(&self) -> &'static [Capability] {
            &Capability::ALL
        }
        fn rename(&self, from: &Path, to: &Path) -> ren_platform::Result<()> {
            self.inner.rename(from, to)
        }
        fn replace_file(&self, temp: &Path, target: &Path) -> ren_platform::Result<()> {
            self.inner.replace_file(temp, target)
        }
        fn get_attributes(
            &self,
            path: &Path,
        ) -> ren_platform::Result<ren_platform::FileAttributes> {
            self.inner.get_attributes(path)
        }
        fn set_attributes(
            &self,
            path: &Path,
            change: ren_platform::AttributeChange,
        ) -> ren_platform::Result<()> {
            self.inner.set_attributes(path, change)
        }
        fn get_times(&self, path: &Path) -> ren_platform::Result<ren_platform::FileTimes> {
            self.inner.get_times(path)
        }
        fn set_times(
            &self,
            path: &Path,
            change: ren_platform::TimeChange,
        ) -> ren_platform::Result<()> {
            self.inner.set_times(path, change)
        }
        fn naming_rules(&self, path: &Path) -> &'static ren_platform::NamingRules {
            self.inner.naming_rules(path)
        }
        fn case_sensitivity(&self, dir: &Path) -> ren_platform::CaseSensitivity {
            self.inner.case_sensitivity(dir)
        }
        fn reveal_in_file_manager(&self, path: &Path) -> ren_platform::Result<()> {
            self.inner.reveal_in_file_manager(path)
        }
        fn notify_shell_changed(&self, path: &Path) {
            self.inner.notify_shell_changed(path);
        }

        fn http_get(
            &self,
            url: &str,
            follow_redirects: bool,
            body: &mut dyn std::io::Write,
        ) -> ren_platform::Result<HttpHead> {
            let ok = HttpHead {
                status: 200,
                location: None,
            };
            if url == latest_url() {
                assert!(!follow_redirects, "the check reads the redirect itself");
                return Ok(HttpHead {
                    status: 302,
                    location: Some(release_page(self.latest)),
                });
            }
            if url == asset_url(self.latest, SUMS) {
                let line = format!(
                    "{}  {}\n",
                    Self::hex(self.listed),
                    installer_asset(self.latest)
                );
                body.write_all(line.as_bytes()).unwrap();
                return Ok(ok);
            }
            if url == asset_url(self.latest, &installer_asset(self.latest)) {
                body.write_all(INSTALLER).unwrap();
                return Ok(ok);
            }
            Ok(HttpHead {
                status: 404,
                location: None,
            })
        }

        fn sha256_file(&self, path: &Path) -> ren_platform::Result<[u8; 32]> {
            Ok(Self::digest(&std::fs::read(path).unwrap()))
        }

        fn install_record(&self) -> Option<InstallRecord> {
            self.install
        }

        fn launch_installer(
            &self,
            installer: &Path,
            args: &[&str],
            elevate: bool,
        ) -> ren_platform::Result<()> {
            self.launched.lock().unwrap().push((
                installer.to_path_buf(),
                args.iter().map(|a| (*a).to_owned()).collect(),
                elevate,
            ));
            Ok(())
        }
    }
}

#[cfg(test)]
mod flow_tests {
    use super::fake::{INSTALLER, Release};
    use super::*;
    use ren_platform::update::InstallRecord;

    const NOW: i64 = 1_800_000_000;
    const PER_USER_INSTALL: Option<InstallRecord> = Some(InstallRecord { per_machine: false });

    fn newer() -> Version {
        let Version(major, minor, patch) = Version::current();
        Version(major, minor, patch + 1)
    }

    fn updates(release: &Arc<Release>, temp: &Path) -> Updates {
        Updates::spawn(release.clone(), temp.to_path_buf(), || {})
    }

    #[test]
    fn check_now_finds_a_newer_release_and_update_hands_over_to_the_installer() {
        let temp = tempfile::TempDir::new().unwrap();
        let release = Release::new(newer(), PER_USER_INSTALL);
        let mut updates = updates(&release, temp.path());
        assert_eq!(updates.install(), Install::PerUser);

        updates.check_now(NOW);
        updates.settle(NOW);
        assert_eq!(updates.status(), &Status::Available(newer()));
        assert_eq!(updates.last_check, Some(NOW));

        updates.install_now();
        assert!(!updates.take_close(), "the download goes first");
        updates.settle(NOW);
        assert!(updates.take_close(), "then the window closes");
        let launched = release.launched.lock().unwrap();
        let (path, args, elevate) = &launched[0];
        assert_eq!(args, &["/S", "/CurrentUser", "/RESTART"]);
        assert!(!elevate);
        assert_eq!(std::fs::read(path).unwrap(), INSTALLER);
        assert!(
            path.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(TEMP_PREFIX)
        );
    }

    #[test]
    fn a_release_that_is_not_newer_is_up_to_date() {
        let temp = tempfile::TempDir::new().unwrap();
        let release = Release::new(Version::current(), PER_USER_INSTALL);
        let mut updates = updates(&release, temp.path());
        updates.check_now(NOW);
        updates.settle(NOW);
        assert_eq!(updates.status(), &Status::UpToDate);
    }

    #[test]
    fn a_download_that_does_not_match_its_checksum_is_thrown_away() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut release = Release::new(newer(), PER_USER_INSTALL);
        Arc::get_mut(&mut release).unwrap().listed = [0; 32];
        let mut updates = updates(&release, temp.path());
        updates.check_now(NOW);
        updates.settle(NOW);
        updates.install_now();
        updates.settle(NOW);
        assert!(!updates.take_close());
        assert!(
            matches!(updates.status(), Status::Failed(e) if e.contains("does not match")),
            "{:?}",
            updates.status()
        );
        assert!(release.launched.lock().unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(temp.path()).unwrap().count(),
            0,
            "nothing left behind"
        );
    }

    #[test]
    fn a_per_machine_install_asks_for_an_administrator() {
        let temp = tempfile::TempDir::new().unwrap();
        let release = Release::new(newer(), Some(InstallRecord { per_machine: true }));
        let mut updates = updates(&release, temp.path());
        updates.check_now(NOW);
        updates.settle(NOW);
        updates.install_now();
        updates.settle(NOW);
        assert!(updates.take_close());
        let launched = release.launched.lock().unwrap();
        assert_eq!(launched[0].1, ["/S", "/AllUsers", "/RESTART"]);
        assert!(launched[0].2, "runas");
    }

    #[test]
    fn manual_never_checks_by_itself_and_notify_checks_once_a_day() {
        let temp = tempfile::TempDir::new().unwrap();
        let release = Release::new(newer(), PER_USER_INSTALL);
        let mut updates = updates(&release, temp.path());
        updates.poll(NOW);
        assert_eq!(updates.status(), &Status::Idle);

        updates.mode = UpdateMode::Notify;
        updates.poll(NOW);
        updates.settle(NOW);
        assert_eq!(updates.status(), &Status::Available(newer()));
        assert!(
            release.launched.lock().unwrap().is_empty(),
            "Notify only tells"
        );

        updates.poll(NOW + DAY - 1);
        assert!(!updates.is_busy(), "not again the same day");
    }

    #[test]
    fn automatic_downloads_then_installs_at_exit_without_reopening() {
        let temp = tempfile::TempDir::new().unwrap();
        let release = Release::new(newer(), PER_USER_INSTALL);
        let mut updates = updates(&release, temp.path());
        updates.mode = UpdateMode::Automatic;
        updates.poll(NOW);
        // The check, then the download it starts: `settle` waits for both.
        updates.settle(NOW);
        assert_eq!(updates.status(), &Status::Ready(newer()));
        assert_eq!(updates.auto_attempt, Some((newer().to_string(), NOW)));
        assert!(release.launched.lock().unwrap().is_empty(), "not yet");

        updates.check_now(NOW + DAY);
        assert_eq!(
            updates.status(),
            &Status::Ready(newer()),
            "a check would drop the installer waiting for exit"
        );

        updates.on_exit();
        assert_eq!(
            release.launched.lock().unwrap()[0].1,
            ["/S", "/CurrentUser"]
        );
    }

    #[test]
    fn automatic_only_tells_a_copy_it_cannot_update_quietly() {
        let temp = tempfile::TempDir::new().unwrap();
        for install in [None, Some(InstallRecord { per_machine: true })] {
            let release = Release::new(newer(), install);
            let mut updates = updates(&release, temp.path());
            updates.mode = UpdateMode::Automatic;
            updates.poll(NOW);
            updates.settle(NOW);
            assert_eq!(updates.status(), &Status::Available(newer()));
            updates.on_exit();
            assert!(release.launched.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn automatic_does_not_retry_a_version_it_tried_this_week() {
        let temp = tempfile::TempDir::new().unwrap();
        let release = Release::new(newer(), PER_USER_INSTALL);
        let mut updates = updates(&release, temp.path());
        updates.mode = UpdateMode::Automatic;
        updates.auto_attempt = Some((newer().to_string(), NOW - DAY));
        updates.poll(NOW);
        updates.settle(NOW);
        assert_eq!(updates.status(), &Status::Available(newer()));
    }

    #[test]
    fn download_folders_older_than_the_cutoff_are_swept_and_nothing_else() {
        let temp = tempfile::TempDir::new().unwrap();
        let ours = temp.path().join(format!("{TEMP_PREFIX}1-2"));
        let other = temp.path().join("somebody-else");
        for dir in [&ours, &other] {
            std::fs::create_dir(dir).unwrap();
        }
        let now = std::time::SystemTime::now();
        sweep(temp.path(), now - std::time::Duration::from_secs(60));
        assert!(ours.exists(), "newer than the cutoff");

        sweep(temp.path(), now + std::time::Duration::from_secs(60));
        assert!(!ours.exists());
        assert!(other.exists(), "only our own prefix");
    }
}
