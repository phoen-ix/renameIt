//! Write-ahead JSONL journal.
//!
//! One line per record, `fsync`ed before the corresponding filesystem call.
//! The invariant that makes undo and crash recovery possible: **if a rename
//! happened, a `plan_rename` line for it is already durable on disk.**

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
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

fn new_txn_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Lexicographic order == chronological order, which is how `latest` works.
    format!(
        "{:011}-{:06}-{}",
        now.as_secs(),
        now.subsec_micros(),
        std::process::id()
    )
}

#[derive(Debug)]
pub struct Journal {
    txn: String,
    path: PathBuf,
    file: File,
    n: u64,
}

impl Journal {
    pub fn create(dir: &Path) -> Result<Self, ExecError> {
        std::fs::create_dir_all(dir).map_err(|e| ExecError::io(dir, e))?;
        let txn = new_txn_id();
        let path = dir.join(format!("{txn}.jsonl"));
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|e| ExecError::io(&path, e))?;
        Ok(Self {
            txn,
            path,
            file,
            n: 0,
        })
    }

    pub fn txn(&self) -> &str {
        &self.txn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one record and makes it durable before returning.
    pub fn write(&mut self, record: Record) -> Result<u64, ExecError> {
        debug_assert_ne!(
            record,
            Record::Unknown,
            "Unknown is a read-only variant — it stands for a record some *other* build wrote"
        );
        let n = self.n;
        self.n += 1;
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
        self.file
            .sync_data()
            .map_err(|e| ExecError::io(&self.path, e))?;
        Ok(n)
    }

    pub fn read(path: &Path) -> Result<Vec<Line>, ExecError> {
        let file = File::open(path).map_err(|e| ExecError::io(path, e))?;
        let mut out = Vec::new();
        for (i, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|e| ExecError::io(path, e))?;
            if line.trim().is_empty() {
                continue;
            }
            let parsed =
                serde_json::from_str(&line).map_err(|source| ExecError::CorruptJournal {
                    path: path.to_path_buf(),
                    line: i + 1,
                    source,
                })?;
            out.push(parsed);
        }
        Ok(out)
    }

    /// Appends to an existing journal — used to stamp `Undone`.
    pub fn append_to(path: &Path, txn: &str, n: u64, record: Record) -> Result<(), ExecError> {
        let mut file = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|e| ExecError::io(path, e))?;
        let line = Line {
            txn: txn.to_owned(),
            n,
            version: version_for(&record),
            record,
        };
        let mut buf =
            serde_json::to_vec(&line).map_err(|source| ExecError::JournalNotWritable {
                path: path.to_path_buf(),
                source,
            })?;
        buf.push(b'\n');
        file.write_all(&buf).map_err(|e| ExecError::io(path, e))?;
        file.sync_data().map_err(|e| ExecError::io(path, e))
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
    /// one file from a newer build must not make the older build unusable.
    pub fn latest_undoable(dir: &Path) -> Result<Option<PathBuf>, ExecError> {
        for path in Self::list(dir)? {
            let lines = Self::read(&path)?;
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

        let lines = Journal::read(journal.path()).unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.txn == journal.txn()));
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

        assert_eq!(
            Journal::latest_undoable(dir.path()).unwrap(),
            Some(second_path.clone())
        );

        Journal::append_to(
            &second_path,
            second.txn(),
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

        let lines = Journal::read(journal.path()).unwrap();
        assert_eq!(lines[0].version, UTF8_PATH_VERSION, "a record with no path");
        assert_eq!(lines[1].version, UTF8_PATH_VERSION, "UTF-8 paths");
        let raw = std::fs::read_to_string(journal.path()).unwrap();
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

        let lines = Journal::read(journal.path()).unwrap();
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

    fn unfinished_is_fine(dir: &Path) -> bool {
        crate::exec::unfinished(dir).is_ok()
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
        assert_eq!(Journal::latest_undoable(dir.path()).unwrap(), None);
    }
}
