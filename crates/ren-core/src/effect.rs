//! What a side-effect step decided to do to one file.
//!
//! Data, not behaviour. The same value goes into the plan (so the preview can
//! show it), onto the wire in the journal (so undo can invert it), and into the
//! executor (so it can happen). Keeping it a plain enum is what lets the
//! parallel evaluation pass stay pure: an action *computes* one of these, and
//! the executor is the only thing that touches the filesystem.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ren_platform::{AttributeChange, Capability, FileAttributes, FileTimes, TimeChange};
use serde::{Deserialize, Serialize};

use crate::meta::write::{FieldWrite, MusicField, TagKind};

/// A point in time, as the journal stores it.
///
/// Not `SystemTime`. Serde's impl for it **returns an error for any instant
/// before 1970**, and `Journal::write` treats a failed serialisation as
/// unreachable — so a file with a 1969 modified date would panic mid-batch with
/// the filesystem half-changed. "Subtract 40 years" is one dropdown away from
/// reachable. Signed seconds cost nothing and cannot do that.
///
/// It is also simply readable, which matters for a file whose whole purpose is
/// being inspectable after a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TimeStamp {
    pub secs: i64,
    /// Always `0..1_000_000_000`.
    pub nanos: u32,
}

impl TimeStamp {
    pub fn from_system(time: SystemTime) -> Self {
        match time.duration_since(UNIX_EPOCH) {
            Ok(d) => Self {
                secs: d.as_secs() as i64,
                nanos: d.subsec_nanos(),
            },
            // Before the epoch. `duration_since` gives the magnitude, so the
            // seconds borrow one when there is a fractional part: 0.25 s before
            // the epoch is -1 s plus 750 ms, not -0 s plus 250 ms.
            Err(e) => {
                let d = e.duration();
                let subsec = d.subsec_nanos();
                if subsec == 0 {
                    Self {
                        secs: -(d.as_secs() as i64),
                        nanos: 0,
                    }
                } else {
                    Self {
                        secs: -(d.as_secs() as i64) - 1,
                        nanos: 1_000_000_000 - subsec,
                    }
                }
            }
        }
    }

    /// `None` when this platform cannot represent the instant at all.
    pub fn to_system(self) -> Option<SystemTime> {
        if self.secs >= 0 {
            UNIX_EPOCH.checked_add(Duration::new(self.secs as u64, self.nanos))
        } else {
            // secs is the floor, so the nanos are added back after subtracting.
            let magnitude = Duration::new(self.secs.unsigned_abs(), 0);
            UNIX_EPOCH
                .checked_sub(magnitude)
                .and_then(|t| t.checked_add(Duration::from_nanos(u64::from(self.nanos))))
        }
    }
}

/// The three stamps, as an edit or as a before-image.
///
/// `None` means "this one is not part of the change" on the way in, and "the
/// filesystem did not report it" on the way back — in both readings it means
/// *leave it alone*, so one type serves both.
/// A journal payload, so it tolerates unknown fields — a later build adding
/// one must not make this build declare the file corrupt (D73).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimeSet {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<TimeStamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accessed: Option<TimeStamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<TimeStamp>,
}

impl TimeSet {
    pub fn of(times: FileTimes) -> Self {
        Self {
            created: times.created.map(TimeStamp::from_system),
            accessed: times.accessed.map(TimeStamp::from_system),
            modified: times.modified.map(TimeStamp::from_system),
        }
    }

    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// The platform edit this describes. Components outside this platform's
    /// representable range are dropped rather than clamped to a wrong instant.
    pub fn to_change(self) -> TimeChange {
        TimeChange {
            created: self.created.and_then(TimeStamp::to_system),
            accessed: self.accessed.and_then(TimeStamp::to_system),
            modified: self.modified.and_then(TimeStamp::to_system),
        }
    }

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

/// One metadata change, ready to be shown, journalled and performed.
///
/// **Not `Copy`.** It was until M6: both of M5's variants are a handful of
/// `Option`s. A tag write carries the text it is going to write, so the type
/// owns a `String` and every site that moved it by dereference now borrows it.
/// Worth the churn rather than boxing the payload — the borrow is what the
/// executor wants anyway, since it holds the effect while it reads the file it
/// is about to merge into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    Attributes(AttributeChange),
    Times(TimeSet),
    /// Music Tagger: the fields to write, already resolved from their
    /// templates. The read-modify-write itself is the executor's, because this
    /// value is computed in the parallel preview pass which must never write.
    ///
    /// A **struct** variant, and that is not a style choice. `Effect` is
    /// `#[serde(tag = "effect")]`, and serde cannot serialise a tagged newtype
    /// variant containing a sequence — `Effect::WriteTags(Vec<_>)` compiles,
    /// then fails at runtime with *"cannot serialize tagged newtype variant
    /// containing a sequence"*, which `Journal::write`'s
    /// `expect("journal records are always serialisable")` turns into a panic
    /// mid-batch with the filesystem half-changed. Exactly the failure
    /// [`TimeStamp`] was introduced to prevent, one type along.
    WriteTags {
        fields: Vec<FieldWrite>,
        /// Fields the user enabled that will **not** be written, and why.
        ///
        /// Carried rather than dropped because P5 forbids silently failing to
        /// make a change the user asked for. A Track mapped to a filename part
        /// that turns out to be `A1` is skipped — lofty would otherwise write
        /// `0` over a real track number — and if it were dropped here the row
        /// would simply show nothing and the user would conclude it worked.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skipped: Vec<Skipped>,
    },
    /// Remove Tags: which blocks to strip.
    RemoveTags {
        kinds: Vec<TagKind>,
    },
    /// An effect written by a newer build.
    ///
    /// D44 gave the journal tolerance for an unrecognised record *kind*; this
    /// is the same hazard one level deeper, because a kind we do know can carry
    /// a payload we do not. Without it, a build predating the tag effects meets
    /// a `plan_irreversible` line saying `"effect":"write_tags"`, fails to
    /// deserialise it, and calls the whole journal corrupt — and `Journal::read`
    /// runs at GUI startup, so that is the application refusing to open rather
    /// than one undo refusing to run.
    ///
    /// Nothing ever acts on it. `Line::understood()` already rejects a line
    /// whose version is newer than ours, and every arm below treats this as
    /// something to report rather than to perform.
    #[serde(other)]
    Unknown,
}

impl Effect {
    /// Nothing to do. A Set Attributes card with all four boxes on "keep" is a
    /// no-op, exactly as Replace with an empty Find box is (P34).
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Attributes(change) => change.is_empty(),
            Self::Times(times) => times.is_empty(),
            // A run that skips every field it was given still has something to
            // say, so it is not empty — `Pipeline::evaluate` drops empty
            // effects, and dropping this one is exactly the silence P5 forbids.
            Self::WriteTags { fields, skipped } => fields.is_empty() && skipped.is_empty(),
            Self::RemoveTags { kinds } => kinds.is_empty(),
            // Not empty: "we do not know what this is" is not "there is
            // nothing to do", and calling it empty would let it be dropped
            // quietly rather than reported.
            Self::Unknown => false,
        }
    }

    /// What this platform must be able to do for the change to happen.
    pub fn required_capabilities(&self) -> Vec<Capability> {
        match self {
            Self::Attributes(change) => change.required_capabilities(),
            Self::Times(times) => times.required_capabilities(),
            // None. Tag writing is plain file IO and crosses platforms
            // cleanly (D3) — and a capability here would be worse than
            // useless, because the planner turns an unsupported one into a
            // blocking conflict (P4). Claiming one would make the operation
            // unrunnable rather than unsupported.
            Self::WriteTags { .. } | Self::RemoveTags { .. } | Self::Unknown => Vec::new(),
        }
    }

    /// How much of this change a run can take back.
    ///
    /// Both of M5's effects are `Journaled`: the executor reads what they
    /// replace and writes it into the journal beside the intent. An effect that
    /// cannot say what it replaced answers `None`, and the machinery in
    /// `crate::exec` then refuses to run it unless the caller has said, in as
    /// many words, that it may.
    pub fn undoability(&self) -> Undoability {
        match self {
            Self::Attributes(_) | Self::Times(_) => Undoability::Journaled,
            // P2. The file's contents changed and nothing was kept — there is
            // no before-image that could put a tag block back, because the
            // block is gone.
            // `Unknown` too: the safest reading of a change we cannot name is
            // that it cannot be taken back.
            Self::WriteTags { .. } | Self::RemoveTags { .. } | Self::Unknown => Undoability::None,
        }
    }
}

/// A field that was enabled and is not being written.
///
/// A journal payload, so no `deny_unknown_fields` — see [`TimeSet`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub field: MusicField,
    /// One clause, joined onto the field's name in the preview.
    pub why: String,
}

/// The state an effect replaced, so undo can put it back.
///
/// Read by the executor immediately before the change and written to the
/// journal with it. A rename can be inverted from the intent alone; a metadata
/// change cannot — "set modified to X" does not say what X replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "of", rename_all = "snake_case")]
pub enum Before {
    Attributes(FileAttributes),
    Times(TimeSet),
}

impl Before {
    /// The effect that puts the previous state back.
    ///
    /// Only the components the change actually touched: restoring a stamp we
    /// never wrote would undo somebody else's edit, and on Unix restoring an
    /// attribute we never wrote can outright fail (a dotfile's `hidden`).
    pub fn restoring(self, change: &Effect) -> Effect {
        match (self, change) {
            (Self::Attributes(was), Effect::Attributes(change)) => {
                Effect::Attributes(AttributeChange {
                    read_only: change.read_only.map(|_| was.read_only),
                    hidden: change.hidden.map(|_| was.hidden),
                    system: change.system.map(|_| was.system),
                    archive: change.archive.map(|_| was.archive),
                })
            }
            (Self::Times(was), Effect::Times(change)) => Effect::Times(TimeSet {
                created: change.created.and(was.created),
                accessed: change.accessed.and(was.accessed),
                modified: change.modified.and(was.modified),
            }),
            // The executor pairs these itself, so a mismatch is a bug rather
            // than a state a journal can be in. Restoring nothing is the safe
            // reading of one anyway — which is also the only honest answer for
            // the two that keep no before-image at all: there is nothing a tag
            // write replaced that could be put back, and `undo` routes them to
            // its `irreversible` bucket long before reaching here (D54).
            _ => match change {
                Effect::Attributes(_) => Effect::Attributes(AttributeChange::default()),
                Effect::Times(_) => Effect::Times(TimeSet::default()),
                Effect::WriteTags { .. } => Effect::WriteTags {
                    fields: Vec::new(),
                    skipped: Vec::new(),
                },
                Effect::RemoveTags { .. } => Effect::RemoveTags { kinds: Vec::new() },
                Effect::Unknown => Effect::Unknown,
            },
        }
    }
}

/// How much of an action a run can take back.
///
/// **P2**: *"Tag writes / tag removal (Music Tagger, Untagger): no undo in 1.0
/// (same as original), but always confirmation-gated with an explicit 'cannot be
/// undone' warning."* Up to M5 that was a promise about a milestone; from M6 it
/// is something the engine carries, because the engine is what has to refuse.
///
/// Ordered worst-last on purpose, so `max()` over a run gives what the run as a
/// whole costs — which is the number the confirmation dialog has to quote. The
/// order is load-bearing and pinned by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Undoability {
    /// A rename: the intent alone inverts it.
    Full,
    /// A metadata change: invertible, but only because the executor records the
    /// value it replaced. Set Attributes and Set Date.
    Journaled,
    /// Gone. The file's contents changed and nothing was kept.
    None,
}

impl Undoability {
    /// The words the confirmation dialog and the run log use.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Full => "can be undone",
            Self::Journaled => "can be undone",
            Self::None => "cannot be undone",
        }
    }

    pub fn is_reversible(self) -> bool {
        self != Self::None
    }
}

/// One action a pipeline step produced for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAction {
    /// Which step in the pipeline, so two Set Date cards stay distinguishable
    /// and the GUI can point at the one that is responsible.
    pub step: usize,
    /// The operation's stable id, e.g. `set_date`.
    pub op: &'static str,
    pub effect: Effect,
    /// Carried rather than derived from `effect`.
    ///
    /// Reversibility really is a property of the change, not of the operation
    /// that asked for it — but carrying it is what lets the machinery land, and
    /// be proved, before the first irreversible operation exists. A test pins
    /// the two readings together for every effect the engine knows.
    pub undoability: Undoability,
    /// One line for the "New name" column and the run log.
    pub describe: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The direction the journal actually uses: a time read off a real file is
    /// recorded, and later handed back to the filesystem. It has to be exact,
    /// and it is by construction — the value came from a `SystemTime` in the
    /// first place, so the platform can hold it.
    #[test]
    fn a_time_read_off_a_file_round_trips_exactly() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();

        for time in [
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            SystemTime::now(),
            UNIX_EPOCH,
            UNIX_EPOCH + Duration::new(1_714_560_000, 123_456_700),
        ] {
            let stamp = TimeStamp::from_system(time);
            assert_eq!(
                stamp.to_system(),
                Some(time),
                "{stamp:?} is not the time it was made from"
            );
        }
    }

    /// The other direction, for values a filesystem can express.
    ///
    /// Windows backs `SystemTime` with `FILETIME`, which counts **100-ns**
    /// ticks, so anything finer is truncated there and not here — a real
    /// platform difference rather than a bug, and one no journal can hit,
    /// because every value it stores came off a file to begin with. The test
    /// therefore uses the coarsest resolution any supported platform has.
    #[test]
    fn a_timestamp_round_trips_at_the_resolution_a_filesystem_offers() {
        for secs in [0i64, 1, -1, 1_714_560_000, -86_400] {
            for nanos in [0u32, 100, 500_000_000, 999_999_900] {
                let stamp = TimeStamp { secs, nanos };
                let back = TimeStamp::from_system(stamp.to_system().expect("representable"));
                assert_eq!(back, stamp, "{secs}.{nanos:09} did not survive");
            }
        }
    }

    /// And the truncation, said out loud where it exists, so the difference is
    /// documented rather than discovered.
    #[test]
    fn sub_tick_precision_is_the_platforms_business_not_ours() {
        let stamp = TimeStamp { secs: 1, nanos: 1 };
        let back = TimeStamp::from_system(stamp.to_system().unwrap());
        assert_eq!(back.secs, 1);
        if cfg!(windows) {
            assert_eq!(back.nanos, 0, "FILETIME counts 100-ns ticks");
        } else {
            assert_eq!(back.nanos, 1);
        }
    }

    /// The reason this type exists: serde refuses to write a `SystemTime`
    /// before 1970, and the journal writer treats that as unreachable.
    #[test]
    fn a_pre_1970_timestamp_survives_the_wire() {
        let christmas_1969 = UNIX_EPOCH - Duration::new(604_800, 250_000_000);
        let stamp = TimeStamp::from_system(christmas_1969);
        assert!(stamp.secs < 0);
        assert_eq!(stamp.nanos, 750_000_000, "the seconds borrow one");

        let json = serde_json::to_string(&stamp).expect("must serialise");
        assert_eq!(
            serde_json::from_str::<TimeStamp>(&json).unwrap(),
            stamp,
            "{json}"
        );
        assert_eq!(stamp.to_system(), Some(christmas_1969));

        // And the thing it is standing in for still cannot.
        assert!(
            serde_json::to_string(&christmas_1969).is_err(),
            "if this ever starts working, TimeStamp is no longer load-bearing"
        );
    }

    #[test]
    fn restoring_only_touches_what_the_change_touched() {
        let was = FileAttributes {
            read_only: true,
            hidden: true,
            system: false,
            archive: true,
        };
        let change = Effect::Attributes(AttributeChange {
            read_only: Some(false),
            ..Default::default()
        });
        assert_eq!(
            Before::Attributes(was).restoring(&change),
            Effect::Attributes(AttributeChange {
                read_only: Some(true),
                ..Default::default()
            }),
            "hidden was never written, so undo must not write it either"
        );
    }

    /// A stamp the filesystem never reported cannot be restored, and must not
    /// be invented.
    #[test]
    fn restoring_a_stamp_the_filesystem_never_reported_restores_nothing() {
        let was = TimeSet {
            modified: Some(TimeStamp { secs: 5, nanos: 0 }),
            ..Default::default()
        };
        let change = Effect::Times(TimeSet {
            created: Some(TimeStamp { secs: 9, nanos: 0 }),
            modified: Some(TimeStamp { secs: 9, nanos: 0 }),
            ..Default::default()
        });
        assert_eq!(
            Before::Times(was).restoring(&change),
            Effect::Times(TimeSet {
                modified: Some(TimeStamp { secs: 5, nanos: 0 }),
                ..Default::default()
            })
        );
    }

    #[test]
    fn an_empty_effect_is_recognised_as_a_no_op() {
        assert!(Effect::Attributes(AttributeChange::default()).is_empty());
        assert!(Effect::Times(TimeSet::default()).is_empty());
        assert!(
            !Effect::Attributes(AttributeChange {
                hidden: Some(true),
                ..Default::default()
            })
            .is_empty()
        );
    }

    #[test]
    fn an_effect_declares_what_the_platform_must_be_able_to_do() {
        let effect = Effect::Times(TimeSet {
            created: Some(TimeStamp { secs: 0, nanos: 0 }),
            modified: Some(TimeStamp { secs: 0, nanos: 0 }),
            ..Default::default()
        });
        assert_eq!(
            effect.required_capabilities(),
            vec![Capability::CreatedTime, Capability::ModifiedTime],
            "created first, so a Linux user reads the documented limitation"
        );
    }

    #[test]
    fn an_effect_round_trips_through_json() {
        for effect in [
            Effect::Attributes(AttributeChange {
                read_only: Some(false),
                ..Default::default()
            }),
            Effect::Times(TimeSet {
                modified: Some(TimeStamp { secs: -1, nanos: 5 }),
                ..Default::default()
            }),
        ] {
            let json = serde_json::to_string(&effect).unwrap();
            assert_eq!(
                serde_json::from_str::<Effect>(&json).unwrap(),
                effect,
                "{json}"
            );
        }
    }
}
