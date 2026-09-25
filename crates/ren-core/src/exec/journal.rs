//! Write-ahead JSONL journal.
//!
//! One line per record, `fsync`ed before the corresponding filesystem call.
//! The invariant that makes undo and crash recovery possible: **if a rename
//! happened, a `plan_rename` line for it is already durable on disk.**
//!
//! Three things a reader of this folder can rely on besides the records:
//!
//! * **A live journal is locked.** A [`Journal`] holds an exclusive lock on
//!   its file for as long as it lives, so a scan tells a run still going on
//!   in another window from one that crashed ([`ExecError::JournalInUse`]).
//!   A crashed process holds nothing, so its journal is offered for recovery
//!   as before.
//! * **A torn last line is not corruption.** The writer ends every record in
//!   `\n`; an unterminated last segment that does not parse is what a full
//!   disk or a power cut leaves, and is dropped (see [`Journal::read`]).
//! * **Names sort in the order journals were written**, whatever the clock
//!   did in between (see `new_txn_id`).

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::ExecError;
use crate::effect::{Before, Effect};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    Begin {
        platform: String,
        items: usize,
    },
    /// Written *before* the rename is attempted.
    PlanRename {
        seq: u64,
        #[serde(with = "raw_path")]
        from: PathBuf,
        #[serde(with = "raw_path")]
        to: PathBuf,
    },
    /// Written *before* a subfolder is created (D31). Only folders this
    /// transaction created are candidates for removal on undo.
    PlanCreateDir {
        seq: u64,
        #[serde(with = "raw_path")]
        path: PathBuf,
    },
    /// Written *before* a file a script asked for is written (M7).
    ///
    /// `replaced` is the whole undo story. A file this transaction created can
    /// be removed again and the folder is as it was; one it *overwrote* cannot
    /// be put back, because the previous contents were never kept — so undo
    /// reports it rather than pretending. Recorded here rather than re-derived
    /// at undo time: by then the file exists either way, and the answer to "was
    /// something already here" is gone.
    PlanWriteFile {
        seq: u64,
        #[serde(with = "raw_path")]
        path: PathBuf,
        replaced: bool,
        /// What a create is about to put there, so undo can tell the file it
        /// made from one the user has edited since: it removes the first and
        /// leaves the second, reported (see `undo_transaction`).
        ///
        /// Optional and absent from journals written before it existed —
        /// which undo treats as it always did. An older build reading a line
        /// that carries it ignores the field and removes the file unchecked,
        /// which is exactly what it would have done with the line before, so
        /// the version does not have to move.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        written: Option<Written>,
    },
    /// Written *before* a metadata change is attempted (M5).
    ///
    /// Carries the state it is about to replace, which no other record needs:
    /// a rename can be inverted from the intent alone, but "set modified to X"
    /// does not say what X was. So the invariant reads, for this record: *if a
    /// metadata change happened, a durable line names it **and** records what
    /// it replaced.*
    PlanAct {
        seq: u64,
        #[serde(with = "raw_path")]
        path: PathBuf,
        /// The operation's id, e.g. `set_date` — for reading the journal, not
        /// for replaying it.
        op: String,
        change: Effect,
        before: Before,
    },
    /// Written *before* a change that **cannot** be taken back (P2).
    ///
    /// A separate kind rather than a `PlanAct` with its before-image made
    /// optional, and the distinction matters: `#[serde(other)]` fires on an
    /// unrecognised *kind*, not on a missing field. A v2 build meeting a
    /// `plan_act` line with no `before` would call the whole journal corrupt —
    /// and `Journal::read` runs at GUI startup, so that is the app failing to
    /// open. Meeting an unknown *kind*, it produces `Record::Unknown` and
    /// carries on, which is exactly what D44 built.
    PlanIrreversible {
        seq: u64,
        #[serde(with = "raw_path")]
        path: PathBuf,
        op: String,
        change: Effect,
    },
    /// Written after the rename or the folder creation succeeded. Only
    /// completed operations are undone.
    Completed {
        seq: u64,
    },
    Failed {
        seq: u64,
        error: String,
    },
    Commit {
        renamed: usize,
        failed: usize,
    },
    /// Marks the transaction as already reverted, so it is not undone twice.
    Undone {
        restored: usize,
        skipped: usize,
    },
    /// A record kind this build does not understand, i.e. a journal written by
    /// a newer RenameIt.
    ///
    /// Read-only: `write` refuses to emit it. Its whole job is to let
    /// [`Journal::read`] finish a file it does not fully understand, so one
    /// such journal in the directory cannot break `list` → `latest_undoable` →
    /// `recover::unfinished` and take crash recovery down with it at startup.
    /// Undoing a transaction containing one is still refused — see
    /// [`ExecError::JournalNotUnderstood`].
    #[serde(other)]
    Unknown,
}

/// A path inside a journal record, encoded so it cannot be lost.
///
/// Serialises as a plain JSON string whenever the path is valid UTF-8 — which
/// is what every journal written before version 6 contains, and what nearly
/// every journal written after it will contain too. That keeps the format
/// byte-identical for the ordinary case, so an older build can still read and
/// undo an ordinary transaction.
///
/// A path that is **not** valid UTF-8 has no string form to write.
/// `serde`'s own `impl Serialize for Path` returns
/// `Err("path contains invalid UTF-8 characters")` for it, and that error used
/// to reach an `.expect` and take the process down *after* the journal was
/// open — so a single oddly-named file aborted the whole batch. Those paths
/// serialise as a one-key object carrying the platform's own encoding:
/// `{"bytes": [..]}` on Unix, `{"wide": [..]}` on Windows.
///
/// Per-platform on purpose. A journal lives in the per-user data folder of the
/// machine that wrote it and is never read anywhere else, so the encoding only
/// has to round-trip locally — and `OsStr`'s own stable conversions
/// (`as_bytes`/`from_bytes`, `encode_wide`/`from_wide`) are exact, where
/// `as_encoded_bytes` is documented as unspecified between Rust versions.
#[cfg(not(any(unix, windows)))]
compile_error!("the journal's path encoding is defined for unix and windows only");

mod raw_path {
    use std::path::{Path, PathBuf};

    use serde::de::{self, MapAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserializer, Serializer};

    /// Whether this path needs the raw form — i.e. whether a plain string
    /// would lose it.
    pub(super) fn is_raw(path: &Path) -> bool {
        path.to_str().is_none()
    }

    pub(super) fn serialize<S: Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
        if let Some(text) = path.to_str() {
            return s.serialize_str(text);
        }
        let mut map = s.serialize_map(Some(1))?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            map.serialize_entry("bytes", path.as_os_str().as_bytes())?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
            map.serialize_entry("wide", &wide)?;
        }
        map.end()
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
        d.deserialize_any(RawPathVisitor)
    }

    struct RawPathVisitor;

    impl<'de> Visitor<'de> for RawPathVisitor {
        type Value = PathBuf;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a path, as a string or as its platform encoding")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(PathBuf::from(v))
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let Some(key) = map.next_key::<String>()? else {
                return Err(de::Error::custom("empty path object"));
            };
            match key.as_str() {
                #[cfg(unix)]
                "bytes" => {
                    use std::os::unix::ffi::OsStrExt as _;
                    let bytes: Vec<u8> = map.next_value()?;
                    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
                }
                #[cfg(windows)]
                "wide" => {
                    use std::os::windows::ffi::OsStringExt as _;
                    let wide: Vec<u16> = map.next_value()?;
                    Ok(PathBuf::from(std::ffi::OsString::from_wide(&wide)))
                }
                // A journal carried over from the other platform. Readable in
                // principle, but a path from a filesystem this machine cannot
                // address is not one it can undo either, so it is refused
                // rather than approximated.
                other => Err(de::Error::custom(format!(
                    "path encoded as {other:?}, which this platform cannot read"
                ))),
            }
        }
    }
}

/// The length and CRC-32 of a file's contents — enough to tell "the file this
/// run wrote" from "a file somebody has changed since", which is the only
/// question undo asks of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Written {
    pub len: u64,
    pub crc32: u32,
}

impl Written {
    pub fn of(contents: &[u8]) -> Self {
        Self {
            len: contents.len() as u64,
            crc32: crc32fast::hash(contents),
        }
    }
}

impl Record {
    /// The paths this record carries, so the writer can tell whether the line
    /// needs the raw encoding — and therefore version 6.
    fn paths(&self) -> impl Iterator<Item = &Path> {
        let slice: [Option<&Path>; 2] = match self {
            Self::PlanRename { from, to, .. } => [Some(from), Some(to)],
            Self::PlanCreateDir { path, .. }
            | Self::PlanWriteFile { path, .. }
            | Self::PlanAct { path, .. }
            | Self::PlanIrreversible { path, .. } => [Some(path), None],
            _ => [None, None],
        };
        slice.into_iter().flatten()
    }
}

/// What this build writes.
///
/// 1 was M1–M4: renames and folder creations only. 2 added the side-effect
/// records M5 needs. 3 adds the record for a change that cannot be taken back.
/// 4 adds the tag effects — a *payload* an older build cannot read, inside a
/// record kind it can. The number has to move even though no new kind appeared,
/// because `Line::understood()` is the only thing that stops an older build
/// acting on a change it half understands.
/// 5 adds `PlanWriteFile`, for the files a script asks for (M7).
/// 6 adds the raw path encoding — see [`raw_path`].
/// A line with no `v` is a version-1 line.
pub const JOURNAL_VERSION: u32 = 6;

/// The version a line carrying only UTF-8 paths is written at.
///
/// Version 6 is emitted **per line, only when a path actually needs the raw
/// form**, rather than for everything this build writes. The encoding is
/// byte-identical to version 5 for a path that is valid UTF-8, so stamping
/// every line as 6 would tell an older build to refuse a transaction it can
/// read perfectly well — and `Journal::read` runs at GUI startup, so that cost
/// lands on downgrade rather than on the odd file that earned it.
const UTF8_PATH_VERSION: u32 = 5;

/// The version this record has to be written at.
fn version_for(record: &Record) -> u32 {
    if record.paths().any(raw_path::is_raw) {
        JOURNAL_VERSION
    } else {
        UTF8_PATH_VERSION
    }
}

fn version_one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub txn: String,
    pub n: u64,
    /// Absent in journals written before M5, which read as 1.
    #[serde(rename = "v", default = "version_one")]
    pub version: u32,
    #[serde(flatten)]
    pub record: Record,
}

impl Line {
    /// Whether this build can act on the line, as opposed to merely list it.
    pub fn understood(&self) -> bool {
        self.version <= JOURNAL_VERSION && self.record != Record::Unknown
    }
}

/// `<per-user data dir>/RenameIt/journal`.
pub fn default_journal_dir() -> PathBuf {
    ren_platform::app_data_dir("RenameIt").join("journal")
}

/// A name for a new journal in `dir`: `{seconds}-{microseconds}-{pid}`.
///
/// Lexicographic order is the order journals were *written*, which is what
/// "the newest transaction" means to `list` and `latest_undoable` (P9). The
/// clock alone does not give that: set back by hand, by a dead RTC battery or
/// a restored VM snapshot, it names the next batch *before* the last one, and
/// `ren-cli undo` then reverts the older batch while the newer one stays
/// applied. So the stamp is the clock or one microsecond past the newest
/// journal already here, whichever is later — still a time on every machine
/// whose clock behaves, and never out of order on one whose clock does not.
fn new_txn_id(dir: &Path) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let micros = newest_stamp(dir).map_or(now, |newest| now.max(newest + 1));
    format!(
        "{:011}-{:06}-{}",
        micros / 1_000_000,
        micros % 1_000_000,
        std::process::id()
    )
}

/// The stamp of the newest journal in `dir`, in microseconds.
fn newest_stamp(dir: &Path) -> Option<u128> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let stem = name.to_str()?.strip_suffix(".jsonl")?;
            let mut parts = stem.split('-');
            let secs: u128 = parts.next()?.parse().ok()?;
            let micros: u128 = parts.next()?.parse().ok()?;
            Some(secs * 1_000_000 + micros)
        })
        .max()
}

/// Whether a byte is padding a line may carry: whitespace, and the NULs a
/// power cut can leave where unsynced data should have been.
fn is_padding(byte: &u8) -> bool {
    byte.is_ascii_whitespace() || *byte == 0
}

fn trim_padding(mut bytes: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = bytes
        && is_padding(first)
    {
        bytes = rest;
    }
    while let [rest @ .., last] = bytes
        && is_padding(last)
    {
        bytes = rest;
    }
    bytes
}

/// How much of a journal's end is read to find its last record.
///
/// A record is a few hundred bytes; one naming two maximal Windows paths in
/// the raw encoding is under this. A journal whose last line is longer still
/// is read in full, which is only slower.
const TAIL: u64 = 64 * 1024;

#[derive(Debug)]
pub struct Journal {
    txn: String,
    path: PathBuf,
    file: File,
    n: u64,
    /// Test seam: the record index at which `write` starts failing, so the
    /// executor's mid-run journal failure can be exercised without a full
    /// disk. Nothing outside a test can set it.
    #[cfg(test)]
    fail_from: Option<u64>,
}

impl Journal {
    pub fn create(dir: &Path) -> Result<Self, ExecError> {
        std::fs::create_dir_all(dir).map_err(|e| ExecError::io(dir, e))?;
        let txn = new_txn_id(dir);
        let path = dir.join(format!("{txn}.jsonl"));
        // Readable as well as appendable: Windows takes a lock only through a
        // handle that may read or write the file's data, and an append-only
        // handle may do neither.
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .append(true)
            .open(&path)
            .map_err(|e| ExecError::io(&path, e))?;
        // Held for the journal's life and released when it is dropped — the
        // mark that tells a scan this run is live (`JournalInUse`). A file
        // just created cannot be locked by anyone else; a filesystem that does
        // not do locks at all runs unlocked, which costs only that mark.
        let _ = file.try_lock();
        Ok(Self {
            txn,
            path,
            file,
            n: 0,
            #[cfg(test)]
            fail_from: None,
        })
    }

    /// Makes every write from record `n` onwards fail as if the disk had gone
    /// away. Test-only; see the field.
    #[cfg(test)]
    pub(crate) fn fail_writes_from(&mut self, n: u64) {
        self.fail_from = Some(n);
    }

    pub fn txn(&self) -> &str {
        &self.txn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one record and makes it durable before returning.
    ///
    /// The write-ahead half of the executor's contract: an intent is durable
    /// before the filesystem is touched. For a record whose durability can
    /// wait for the next one — a `Completed`, which the next intent's sync
    /// carries along — see [`Self::append`].
    pub fn write(&mut self, record: Record) -> Result<u64, ExecError> {
        let n = self.append(record)?;
        self.sync()?;
        Ok(n)
    }

    /// Appends one record without waiting for the disk.
    ///
    /// It reaches the disk with the next [`Self::sync`] — or the next
    /// [`Self::write`], whose sync flushes everything appended before it. A
    /// crash in between loses only what a crash between a rename and its
    /// `Completed` always could: a change that happened and was not
    /// confirmed, which recovery decides from the disk (D84). Every
    /// `Completed` used to be its own `fdatasync`, which was half the syncs
    /// of a run and none of its guarantee.
    pub fn append(&mut self, record: Record) -> Result<u64, ExecError> {
        debug_assert_ne!(
            record,
            Record::Unknown,
            "Unknown is a read-only variant — it stands for a record some *other* build wrote"
        );
        let n = self.n;
        self.n += 1;
        #[cfg(test)]
        if self.fail_from.is_some_and(|from| n >= from) {
            return Err(ExecError::io(
                &self.path,
                std::io::Error::new(std::io::ErrorKind::StorageFull, "test: the disk is full"),
            ));
        }
        let line = Line {
            txn: self.txn.clone(),
            n,
            version: version_for(&record),
            record,
        };
        let mut buf =
            serde_json::to_vec(&line).map_err(|source| ExecError::JournalNotWritable {
                path: self.path.clone(),
                source,
            })?;
        buf.push(b'\n');
        self.file
            .write_all(&buf)
            .map_err(|e| ExecError::io(&self.path, e))?;
        Ok(n)
    }

    /// Makes everything appended so far durable.
    pub fn sync(&mut self) -> Result<(), ExecError> {
        self.file
            .sync_data()
            .map_err(|e| ExecError::io(&self.path, e))
    }

    /// Every record in the journal at `path`.
    ///
    /// **A torn last line is dropped, not reported.** The writer ends every
    /// record in `\n`, so an unterminated last segment that does not parse —
    /// `write_all` giving up part-way on a full disk, or the unsynced tail a
    /// power cut leaves as zeros — can only be an intent whose op never ran
    /// (intents are synced before their op) or a confirmation recovery decides
    /// from the disk anyway (D84). One that *does* parse is a whole record
    /// that lost only its newline, and is kept. NUL padding is trimmed from
    /// every line for the same reason. A terminated line that does not parse
    /// is still [`ExecError::CorruptJournal`] (D44).
    pub fn read(path: &Path) -> Result<Vec<Line>, ExecError> {
        let mut file = File::open(path).map_err(|e| ExecError::io(path, e))?;
        Self::read_from(&mut file, path)
    }

    /// [`Self::read`], through a handle the caller already holds — which on
    /// Windows is the only handle that may read a file it has locked.
    pub(crate) fn read_from(file: &mut File, path: &Path) -> Result<Vec<Line>, ExecError> {
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_to_end(&mut bytes))
            .map_err(|e| ExecError::io(path, e))?;
        let terminated = bytes.last() == Some(&b'\n');
        let segments: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
        let count = segments.len();
        let mut out = Vec::new();
        for (i, segment) in segments.into_iter().enumerate() {
            let text = trim_padding(segment);
            if text.is_empty() {
                continue;
            }
            match serde_json::from_slice(text) {
                Ok(line) => out.push(line),
                Err(_) if i + 1 == count && !terminated => {}
                Err(source) => {
                    return Err(ExecError::CorruptJournal {
                        path: path.to_path_buf(),
                        line: i + 1,
                        source,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Opens a journal to read it, unless a run is still writing it.
    ///
    /// A shared lock, so two windows scanning at once do not mistake each
    /// other for a live run; it conflicts only with the writer's exclusive
    /// one. Released when the handle is dropped. A filesystem without locks
    /// answers every journal as idle, which is what this build did before it
    /// locked anything.
    pub(crate) fn open_idle(path: &Path) -> Result<File, ExecError> {
        let file = File::open(path).map_err(|e| ExecError::io(path, e))?;
        match file.try_lock_shared() {
            Ok(()) | Err(TryLockError::Error(_)) => Ok(file),
            Err(TryLockError::WouldBlock) => Err(ExecError::JournalInUse {
                path: path.to_path_buf(),
            }),
        }
    }

    /// Opens a journal to act on it — undo, rollback — holding it exclusively
    /// until the handle is dropped, so nothing else writes it meanwhile and a
    /// second window cannot roll the same batch back twice.
    pub(crate) fn open_exclusive(path: &Path) -> Result<File, ExecError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| ExecError::io(path, e))?;
        match file.try_lock() {
            Ok(()) | Err(TryLockError::Error(_)) => Ok(file),
            Err(TryLockError::WouldBlock) => Err(ExecError::JournalInUse {
                path: path.to_path_buf(),
            }),
        }
    }

    /// Whether the journal's last record is `Commit` or `Undone` — read from
    /// the tail alone.
    ///
    /// The terminal record is always the last one written, so this is all a
    /// startup scan needs to set a finished journal aside, and it no longer
    /// parses every journal ever written in full to learn it. `false` means
    /// "not known to be finished": the caller reads the whole file.
    pub(crate) fn ends_finished(file: &mut File, path: &Path) -> Result<bool, ExecError> {
        let io = |e| ExecError::io(path, e);
        let len = file.seek(SeekFrom::End(0)).map_err(io)?;
        let start = len.saturating_sub(TAIL);
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(start))
            .and_then(|_| file.read_to_end(&mut bytes))
            .map_err(io)?;
        let mut segments: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
        // A segment that starts mid-line (the read began inside it) is not a
        // record, and neither is a torn one at the end.
        if start > 0 && !segments.is_empty() {
            segments.remove(0);
        }
        for segment in segments.into_iter().rev() {
            let text = trim_padding(segment);
            if text.is_empty() {
                continue;
            }
            return Ok(match serde_json::from_slice::<Line>(text) {
                Ok(line) => matches!(line.record, Record::Commit { .. } | Record::Undone { .. }),
                // Torn, or damaged: the full read decides which.
                Err(_) => false,
            });
        }
        Ok(false)
    }

    /// Appends to an existing journal — used to stamp `Undone`.
    pub fn append_to(path: &Path, txn: &str, n: u64, record: Record) -> Result<(), ExecError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| ExecError::io(path, e))?;
        Self::stamp(&mut file, path, txn, n, record)
    }

    /// Appends one record to a finished or abandoned journal and syncs it.
    ///
    /// **A torn tail is repaired first**, not written after. Appending onto
    /// an unterminated fragment would glue the new record to it — turning a
    /// tail [`Self::read`] drops into a terminated line it must call corrupt,
    /// for good. A fragment that parses is a whole record missing its
    /// newline, and gets one; anything else is cut off.
    pub(crate) fn stamp(
        file: &mut File,
        path: &Path,
        txn: &str,
        n: u64,
        record: Record,
    ) -> Result<(), ExecError> {
        let io = |e| ExecError::io(path, e);
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_to_end(&mut bytes))
            .map_err(io)?;
        let clean = bytes
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |at| at + 1);
        let tail = trim_padding(&bytes[clean..]);
        let mut buf = Vec::new();
        if !tail.is_empty() && serde_json::from_slice::<Line>(tail).is_ok() {
            buf.push(b'\n');
        } else if clean < bytes.len() {
            file.set_len(clean as u64).map_err(io)?;
        }
        let line = Line {
            txn: txn.to_owned(),
            n,
            version: version_for(&record),
            record,
        };
        buf.extend(
            serde_json::to_vec(&line).map_err(|source| ExecError::JournalNotWritable {
                path: path.to_path_buf(),
                source,
            })?,
        );
        buf.push(b'\n');
        file.seek(SeekFrom::End(0)).map_err(io)?;
        file.write_all(&buf).map_err(io)?;
        file.sync_data().map_err(io)
    }

    /// Journals in `dir`, newest first.
    pub fn list(dir: &Path) -> Result<Vec<PathBuf>, ExecError> {
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| ExecError::io(dir, e))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        paths.sort();
        paths.reverse();
        Ok(paths)
    }

    /// Whether a reverse replay of `lines` could actually put something back.
    ///
    /// **Not** "did anything complete", which is what this used to be. `apply`
    /// writes a `Completed` for an irreversible act too, so a Remove Tags run
    /// over five hundred files qualified: it was offered for undo, consumed the
    /// undo slot, restored nothing, and reported success — because
    /// `UndoReport::is_complete()` reads only `skipped`, and nothing had been
    /// skipped.
    ///
    /// The journal already knows better. A completed `seq` belongs to exactly
    /// one plan record, and three of the four kinds are reversible: a rename
    /// inverts from its intent, a created folder can be removed, and a
    /// `PlanAct` carries the before-image that is the whole reason that variant
    /// exists. `PlanIrreversible` is excluded structurally rather than by name
    /// — it has no `before` field, which is the same fact stated in the type.
    fn has_reversible_work(lines: &[Line]) -> bool {
        let reversible: std::collections::HashSet<u64> = lines
            .iter()
            .filter_map(|l| match l.record {
                Record::PlanRename { seq, .. }
                | Record::PlanCreateDir { seq, .. }
                | Record::PlanAct { seq, .. } => Some(seq),
                // Only the ones that created a file. An overwrite kept no
                // before-image, so it is as unreversible as `PlanIrreversible`
                // — and offering Undo for a run that only overwrote something
                // would spend the press restoring nothing.
                Record::PlanWriteFile {
                    seq,
                    replaced: false,
                    ..
                } => Some(seq),
                _ => None,
            })
            .collect();
        lines.iter().any(|l| match l.record {
            Record::Completed { seq } => reversible.contains(&seq),
            _ => false,
        })
    }

    /// The newest transaction that has not already been undone.
    ///
    /// A journal this build does not fully understand is skipped rather than
    /// offered: the Undo button must never propose something `undo_transaction`
    /// will then refuse. It is skipped rather than fatal for the same reason —
    /// one file from a newer build must not make the older build unusable. A
    /// journal a run in another window is still writing is skipped too: it is
    /// not a finished batch, and `undo_transaction` would refuse it.
    ///
    /// A journal that cannot be *read* is still an error, deliberately. It
    /// may be the newest batch, and skipping it would undo the one beneath —
    /// an older batch reverted while the newer one stays applied, which is
    /// worse than a refusal that names the file. A torn last line no longer
    /// makes a journal unreadable ([`Self::read`]).
    pub fn latest_undoable(dir: &Path) -> Result<Option<PathBuf>, ExecError> {
        for path in Self::list(dir)? {
            let mut file = match Self::open_idle(&path) {
                Err(ExecError::JournalInUse { .. }) => continue,
                other => other?,
            };
            let lines = Self::read_from(&mut file, &path)?;
            if !lines.iter().all(Line::understood) {
                continue;
            }
            let undone = lines
                .iter()
                .any(|l| matches!(l.record, Record::Undone { .. }));
            if !undone && Self::has_reversible_work(&lines) {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// Refuses if any line is beyond what this build understands.
    ///
    /// Reading is permissive so the app still starts; *acting* is not. Undoing
    /// half a transaction because the other half used a record kind we skipped
    /// would leave the filesystem in a state no version can describe.
    pub fn check_understood(path: &Path, lines: &[Line]) -> Result<(), ExecError> {
        if let Some(line) = lines.iter().find(|l| l.version > JOURNAL_VERSION) {
            return Err(ExecError::JournalFromTheFuture {
                path: path.to_path_buf(),
                found: line.version,
                known: JOURNAL_VERSION,
            });
        }
        if lines.iter().any(|l| l.record == Record::Unknown) {
            return Err(ExecError::JournalNotUnderstood {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn records_round_trip_through_jsonl() {
        let dir = TempDir::new().unwrap();
        let mut journal = Journal::create(dir.path()).unwrap();
        journal
            .write(Record::Begin {
                platform: "test".into(),
                items: 2,
            })
            .unwrap();
        let seq = journal
            .write(Record::PlanRename {
                seq: 0,
                from: "/a".into(),
                to: "/b".into(),
            })
            .unwrap();
        journal.write(Record::Completed { seq }).unwrap();
        let (path, txn) = (journal.path().to_path_buf(), journal.txn().to_owned());
        // Closed first: Windows will not let another handle read a file its
        // writer still has locked.
        drop(journal);

        let lines = Journal::read(&path).unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.txn == txn));
        assert_eq!(
            lines[1].record,
            Record::PlanRename {
                seq: 0,
                from: "/a".into(),
                to: "/b".into()
            }
        );
    }

    #[test]
    fn latest_undoable_skips_transactions_already_reverted() {
        let dir = TempDir::new().unwrap();

        // A plan record as well as the `Completed`: a bare completion is not a
        // transaction any reverse replay could act on, and `latest_undoable`
        // now says so.
        let mut first = Journal::create(dir.path()).unwrap();
        first
            .write(Record::PlanRename {
                seq: 0,
                from: "/x/a".into(),
                to: "/x/b".into(),
            })
            .unwrap();
        first.write(Record::Completed { seq: 0 }).unwrap();
        let first_path = first.path().to_path_buf();
        let first_txn = first.txn().to_owned();
        // A journal that is still open is a run still going on, and is never
        // offered; these two are finished.
        drop(first);

        // Same-microsecond IDs would break ordering; the second journal must
        // sort after the first.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut second = Journal::create(dir.path()).unwrap();
        second
            .write(Record::PlanRename {
                seq: 0,
                from: "/x/c".into(),
                to: "/x/d".into(),
            })
            .unwrap();
        second.write(Record::Completed { seq: 0 }).unwrap();
        let second_path = second.path().to_path_buf();
        let second_txn = second.txn().to_owned();
        drop(second);

        assert_eq!(
            Journal::latest_undoable(dir.path()).unwrap(),
            Some(second_path.clone())
        );

        Journal::append_to(
            &second_path,
            &second_txn,
            99,
            Record::Undone {
                restored: 1,
                skipped: 0,
            },
        )
        .unwrap();
        assert_eq!(
            Journal::latest_undoable(dir.path()).unwrap(),
            Some(first_path.clone())
        );

        Journal::append_to(
            &first_path,
            &first_txn,
            99,
            Record::Undone {
                restored: 1,
                skipped: 0,
            },
        )
        .unwrap();
        assert_eq!(Journal::latest_undoable(dir.path()).unwrap(), None);
    }

    /// Every journal M1–M4 wrote lacks a `v` field entirely.
    #[test]
    fn a_journal_written_before_versioning_still_reads_as_version_one() {
        let line: Line = serde_json::from_str(
            r#"{"txn":"t","n":3,"kind":"plan_rename","seq":7,"from":"/a","to":"/b"}"#,
        )
        .expect("an M4-shaped line must still parse");
        assert_eq!(line.version, 1);
        assert!(line.understood());
        assert_eq!(
            line.record,
            Record::PlanRename {
                seq: 7,
                from: "/a".into(),
                to: "/b".into()
            }
        );
    }

    /// A line is stamped with the version it actually needs, not with the
    /// highest this build knows.
    ///
    /// The distinction is what keeps an ordinary transaction undoable by an
    /// older build: for a UTF-8 path the bytes are identical to version 5, so
    /// claiming 6 would refuse a downgrade nothing was wrong with.
    #[test]
    fn a_line_carries_the_version_it_needs() {
        let dir = TempDir::new().unwrap();
        let mut journal = Journal::create(dir.path()).unwrap();
        journal.write(Record::Completed { seq: 0 }).unwrap();
        journal
            .write(Record::PlanRename {
                seq: 1,
                from: "/a".into(),
                to: "/b".into(),
            })
            .unwrap();
        let path = journal.path().to_path_buf();
        drop(journal);

        let lines = Journal::read(&path).unwrap();
        assert_eq!(lines[0].version, UTF8_PATH_VERSION, "a record with no path");
        assert_eq!(lines[1].version, UTF8_PATH_VERSION, "UTF-8 paths");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains(&format!(r#""v":{UTF8_PATH_VERSION}"#)),
            "{raw}"
        );
    }

    /// The other half: a path that cannot be a string forces version 6, and
    /// survives the round trip byte for byte.
    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_round_trips_and_raises_the_version() {
        use std::os::unix::ffi::OsStrExt as _;

        let dir = TempDir::new().unwrap();
        let mut journal = Journal::create(dir.path()).unwrap();
        let from = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/caf\xE9.txt"));
        journal
            .write(Record::PlanRename {
                seq: 0,
                from: from.clone(),
                to: "/tmp/cafe.txt".into(),
            })
            .unwrap();
        let path = journal.path().to_path_buf();
        drop(journal);

        let lines = Journal::read(&path).unwrap();
        assert_eq!(lines[0].version, JOURNAL_VERSION, "a raw path needs 6");
        let Record::PlanRename { from: back, to, .. } = &lines[0].record else {
            panic!("{:?}", lines[0].record)
        };
        // The bytes, not the lossy string: comparing `to_string_lossy` would
        // pass even if the byte were replaced, which is the whole bug.
        assert_eq!(back.as_os_str().as_bytes(), from.as_os_str().as_bytes());
        assert_eq!(to, Path::new("/tmp/cafe.txt"));
    }

    /// The failure this prevents: one journal from a newer build aborts
    /// `read`, which aborts `list` → `latest_undoable` → `recover::unfinished`,
    /// which is called at GUI startup. A single file would make the app
    /// unusable rather than merely unable to undo one transaction.
    #[test]
    fn an_unknown_record_kind_does_not_abort_the_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("00000000001-000000-1.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"txn":"t","n":0,"v":2,"kind":"plan_rename","seq":0,"from":"/a","to":"/b"}"#,
                "\n",
                r#"{"txn":"t","n":1,"v":2,"kind":"plan_teleport","seq":1,"whither":"mars"}"#,
                "\n",
                r#"{"txn":"t","n":2,"v":2,"kind":"completed","seq":0}"#,
                "\n",
            ),
        )
        .unwrap();

        let lines = Journal::read(&path).expect("an unknown kind must not fail the read");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].record, Record::Unknown);
        assert!(!lines[1].understood());
        assert_eq!(
            lines[0].record,
            Record::PlanRename {
                seq: 0,
                from: "/a".into(),
                to: "/b".into()
            },
            "the records either side of it still parsed"
        );

        // Listing works, so startup recovery survives...
        assert_eq!(Journal::list(dir.path()).unwrap(), vec![path.clone()]);
        assert!(unfinished_is_fine(dir.path()));

        // ...but it is never offered for undo, because undo would refuse it.
        assert_eq!(Journal::latest_undoable(dir.path()).unwrap(), None);
        let err = Journal::check_understood(&path, &lines).expect_err("refused");
        assert!(
            matches!(err, ExecError::JournalNotUnderstood { .. }),
            "{err:?}"
        );
    }

    /// A *known* kind with the wrong shape is corruption, not a newer build,
    /// and must keep saying so.
    #[test]
    fn genuinely_malformed_json_is_still_reported_as_corruption() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("00000000001-000000-1.jsonl");
        std::fs::write(&path, "{\"txn\":\"t\",\"n\":0,\"kind\":\n").unwrap();
        let err = Journal::read(&path).expect_err("truncated JSON is corrupt");
        assert!(
            matches!(err, ExecError::CorruptJournal { line: 1, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_record_this_build_understands_at_a_version_it_does_not_is_still_refused() {
        let path = Path::new("/tmp/example.jsonl");
        let lines = vec![Line {
            txn: "t".into(),
            n: 0,
            version: JOURNAL_VERSION + 1,
            record: Record::Completed { seq: 0 },
        }];
        let err = Journal::check_understood(path, &lines).expect_err("refused");
        match err {
            ExecError::JournalFromTheFuture { found, known, .. } => {
                assert_eq!((found, known), (JOURNAL_VERSION + 1, JOURNAL_VERSION));
            }
            other => panic!("expected JournalFromTheFuture, got {other:?}"),
        }
    }

    /// A line the writer never finished is what a full disk or a power cut
    /// leaves at the end of a journal: `write_all` gives up part-way through a
    /// record, or the unsynced tail comes back as zeros. Every record the
    /// writer emits ends in `\n`, so an unterminated last segment that does not
    /// parse can only be such a tail — an intent whose op never ran, or a
    /// confirmation recovery decides from the disk anyway (D84).
    #[test]
    fn a_torn_final_line_is_dropped_on_read() {
        let dir = TempDir::new().unwrap();
        let first = concat!(
            r#"{"txn":"t","n":0,"v":5,"kind":"plan_rename","seq":0,"from":"/a","to":"/b"}"#,
            "\n"
        );
        for tail in [
            &br#"{"txn":"t","n":1,"v":5,"kind":"compl"#[..],
            &b"\0\0\0\0\0\0"[..],
        ] {
            let path = dir.path().join("00000000001-000000-1.jsonl");
            let mut bytes = first.as_bytes().to_vec();
            bytes.extend_from_slice(tail);
            std::fs::write(&path, bytes).unwrap();
            let lines = Journal::read(&path).unwrap_or_else(|e| panic!("{tail:?}: {e}"));
            assert_eq!(lines.len(), 1, "{tail:?}");
        }
    }

    /// The last record written in full, missing only its newline, is a record:
    /// kept, because a `Completed` is exactly what that tail can be.
    #[test]
    fn a_complete_last_record_missing_only_its_newline_is_kept() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("00000000001-000000-1.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"txn":"t","n":0,"v":5,"kind":"plan_rename","seq":0,"from":"/a","to":"/b"}"#,
                "\n",
                r#"{"txn":"t","n":1,"v":5,"kind":"completed","seq":0}"#,
            ),
        )
        .unwrap();
        let lines = Journal::read(&path).unwrap();
        assert_eq!(lines[1].record, Record::Completed { seq: 0 });
    }

    /// Stamping a journal whose tail was torn must not glue the new record onto
    /// the torn one: that would turn a tail the reader drops into a terminated
    /// line it has to call corrupt, forever.
    #[test]
    fn stamping_after_a_torn_tail_leaves_a_readable_journal() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("00000000001-000000-1.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"txn":"t","n":0,"v":5,"kind":"plan_rename","seq":0,"from":"/a","to":"/b"}"#,
                "\n",
                r#"{"txn":"t","n":1,"v":5,"ki"#,
            ),
        )
        .unwrap();
        Journal::append_to(
            &path,
            "t",
            2,
            Record::Undone {
                restored: 0,
                skipped: 0,
            },
        )
        .unwrap();
        let lines = Journal::read(&path).unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(matches!(lines[1].record, Record::Undone { .. }));
    }

    /// "The newest transaction" is the one written last, whatever the clock
    /// says. A journal named by a clock that has since been set back must not
    /// sort after everything written from then on, or `ren-cli undo` reverts
    /// an older batch while the newer one stays applied.
    #[test]
    fn a_new_journal_sorts_after_one_from_a_clock_that_ran_ahead() {
        let dir = TempDir::new().unwrap();
        let ahead = dir.path().join("99999999998-000000-1.jsonl");
        std::fs::write(
            &ahead,
            concat!(
                r#"{"txn":"99999999998-000000-1","n":0,"v":5,"kind":"plan_rename","seq":0,"from":"/x/a","to":"/x/b"}"#,
                "\n",
                r#"{"txn":"99999999998-000000-1","n":1,"v":5,"kind":"completed","seq":0}"#,
                "\n",
            ),
        )
        .unwrap();

        let mut journal = Journal::create(dir.path()).unwrap();
        journal
            .write(Record::PlanRename {
                seq: 0,
                from: "/x/c".into(),
                to: "/x/d".into(),
            })
            .unwrap();
        journal.write(Record::Completed { seq: 0 }).unwrap();
        let newest = journal.path().to_path_buf();
        drop(journal);

        assert!(newest > ahead, "{newest:?} sorts before {ahead:?}");
        assert_eq!(Journal::latest_undoable(dir.path()).unwrap(), Some(newest));
    }

    fn unfinished_is_fine(dir: &Path) -> bool {
        crate::exec::unfinished(dir).1.is_empty()
    }

    /// The bump must not orphan the journals M5 wrote. D44's whole argument
    /// was that versioning prevents the *next* break, not that it excuses one.
    #[test]
    fn a_journal_written_by_version_two_is_still_read_and_understood() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("00000000001-000000-1.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"txn":"t","n":0,"v":2,"kind":"begin","platform":"test","items":1}"#,
                "\n",
                r#"{"txn":"t","n":1,"v":2,"kind":"plan_act","seq":0,"path":"/x/a.txt","op":"set_date","#,
                r#""change":{"effect":"times","modified":{"secs":5,"nanos":0}},"#,
                r#""before":{"of":"times","modified":{"secs":1,"nanos":0}}}"#,
                "\n",
                r#"{"txn":"t","n":2,"v":2,"kind":"completed","seq":0}"#,
                "\n",
            ),
        )
        .unwrap();

        let lines = Journal::read(&path).expect("a v2 journal still parses");
        assert_eq!(lines.len(), 3);
        assert!(
            lines.iter().all(Line::understood),
            "and is still actionable"
        );
        assert!(Journal::check_understood(&path, &lines).is_ok());
        assert_eq!(
            Journal::latest_undoable(dir.path()).unwrap(),
            Some(path),
            "and is still offered for undo"
        );
    }

    #[test]
    fn a_transaction_that_renamed_nothing_is_not_offered_for_undo() {
        let dir = TempDir::new().unwrap();
        let mut journal = Journal::create(dir.path()).unwrap();
        journal
            .write(Record::Begin {
                platform: "test".into(),
                items: 0,
            })
            .unwrap();
        journal
            .write(Record::Commit {
                renamed: 0,
                failed: 0,
            })
            .unwrap();
        drop(journal);
        assert_eq!(Journal::latest_undoable(dir.path()).unwrap(), None);
    }
}
