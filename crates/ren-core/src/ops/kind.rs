//! One value that can be any operation.
//!
//! **D23.** Three places need "an operation the user configured": the GUI's
//! editor panel edits one, job files deserialise into one, and M4's presets
//! store a list of them. Without this they would each hand-roll the same
//! name→type mapping and drift apart.
//!
//! `AppendSuffix` is deliberately absent: it is M0's placeholder, reachable
//! from Rust for tests and the CLI's `--suffix` smoke path, but it is not an
//! operation a user chooses.

use serde::{Deserialize, Serialize};

use super::{
    AddCounter, AddRemove, BatchReplace, Casing, CsvList, FilenameEditor, FreeFormat, MoveSection,
    MusicRename, MusicTagger, NameTransform, ReNumber, RemoveTags, Replace, Script, SetAttributes,
    SetDate, SideEffectAction, SpaceTrim, ZeroPadding,
};
use crate::model::Scope;
use crate::pipeline::Step;

/// The "General" and "Numbers" function groups, as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OpKind {
    Replace(Replace),
    BatchReplace(BatchReplace),
    Casing(Casing),
    AddRemove(AddRemove),
    MoveSection(MoveSection),
    SpaceTrim(SpaceTrim),
    AddCounter(AddCounter),
    /// The wire name is `renumber`, not serde's `re_number`.
    ///
    /// [`OpKind::name`] and [`OpKind::from_table`] have always said `renumber`,
    /// so a value written straight through serde produced a file the job-file
    /// parser then rejected. Nothing wrote `OpKind` out before M4, which is why
    /// it went unnoticed; `every_operation_serialises_under_the_name_it_parses_from`
    /// makes sure the next operation cannot repeat it.
    ///
    /// The alias is load-bearing: the GUI persists its state as RON through
    /// eframe, and blobs written before this fix contain `re_number`. Without
    /// it, upgrading would fail to parse and silently reset every setting.
    #[serde(rename = "renumber", alias = "re_number")]
    ReNumber(ReNumber),
    ZeroPadding(ZeroPadding),
    FreeFormat(FreeFormat),
    CsvList(CsvList),
    FilenameEditor(FilenameEditor),
    SetAttributes(SetAttributes),
    SetDate(SetDate),
    MusicRename(MusicRename),
    /// Boxed, and it is the only variant that is. Seven tag templates come to
    /// 672 bytes against 160 for the next largest operation, and an enum is as
    /// big as its biggest arm — so without this every `OpKind` anywhere in the
    /// program, including the fourteen that are a handful of `Option<bool>`,
    /// would carry the tagger's footprint.
    MusicTagger(Box<MusicTagger>),
    RemoveTags(RemoveTags),
    Script(Script),
}

/// The four operation groups. Presets is deliberately not one — it became the
/// pipeline itself (D8). Used by the palette to group what it offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OpGroup {
    General,
    Numbers,
    Music,
    Advanced,
}

impl OpGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Numbers => "Numbers",
            Self::Music => "Music",
            Self::Advanced => "Advanced",
        }
    }
}

/// Whether an operation produces a new name or performs an action.
///
/// Every operation today produces a name. M5's Set Attributes and Set Date do
/// not — they show what they will *do* rather than what the file will be
/// called. Naming the distinction now means M5 adds a variant instead of
/// teaching the pipeline UI a concept it never had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Produces {
    Name,
    Action,
}

/// An operation, classified.
///
/// An enum rather than an `Option<&dyn NameTransform>`, and the shape matters
/// (D43). An `Option` compiles everywhere and is *wrong* wherever a caller
/// writes `if let Some(t) = …` and forgets the other case: a card checking
/// itself that way would report no problem for any action at all — silently,
/// forever. A two-variant enum forces every consumer to say what it does with
/// an action.
pub enum StepRef<'a> {
    Name(&'a dyn NameTransform),
    Action(&'a dyn SideEffectAction),
}

impl Default for OpKind {
    fn default() -> Self {
        Self::Replace(Replace::default())
    }
}

impl OpKind {
    /// Every operation, in the order the UI offers them: Replace, Casing,
    /// Add / Remove, Move, Spaces, then the rest.
    pub fn all() -> Vec<Self> {
        vec![
            Self::Replace(Replace::default()),
            Self::BatchReplace(BatchReplace::default()),
            Self::Casing(Casing::default()),
            Self::AddRemove(AddRemove::default()),
            Self::MoveSection(MoveSection::default()),
            Self::SpaceTrim(SpaceTrim::default()),
            Self::AddCounter(AddCounter::default()),
            Self::ReNumber(ReNumber::default()),
            Self::ZeroPadding(ZeroPadding::default()),
            Self::FreeFormat(FreeFormat::default()),
            Self::CsvList(CsvList::default()),
            Self::FilenameEditor(FilenameEditor::default()),
            Self::SetAttributes(SetAttributes::default()),
            Self::SetDate(SetDate::default()),
            Self::MusicRename(MusicRename::default()),
            Self::MusicTagger(Box::default()),
            Self::RemoveTags(RemoveTags::default()),
            Self::Script(Script::default()),
        ]
    }

    /// The name used in job files and presets.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Replace(_) => "replace",
            Self::BatchReplace(_) => "batch_replace",
            Self::Casing(_) => "casing",
            Self::AddRemove(_) => "add_remove",
            Self::MoveSection(_) => "move_section",
            Self::SpaceTrim(_) => "space_trim",
            Self::AddCounter(_) => "add_counter",
            Self::ReNumber(_) => "renumber",
            Self::ZeroPadding(_) => "zero_padding",
            Self::FreeFormat(_) => "free_format",
            Self::CsvList(_) => "csv_list",
            Self::FilenameEditor(_) => "filename_editor",
            Self::SetAttributes(_) => "set_attributes",
            Self::SetDate(_) => "set_date",
            Self::MusicRename(_) => "music_rename",
            Self::MusicTagger(_) => "music_tagger",
            Self::RemoveTags(_) => "remove_tags",
            Self::Script(_) => "script",
        }
    }

    /// Which function group this belongs to.
    pub fn group(&self) -> OpGroup {
        match self {
            Self::Replace(_)
            | Self::BatchReplace(_)
            | Self::Casing(_)
            | Self::AddRemove(_)
            | Self::MoveSection(_)
            | Self::SpaceTrim(_) => OpGroup::General,
            Self::AddCounter(_) | Self::ReNumber(_) | Self::ZeroPadding(_) => OpGroup::Numbers,
            Self::FreeFormat(_)
            | Self::CsvList(_)
            | Self::FilenameEditor(_)
            | Self::SetAttributes(_)
            | Self::SetDate(_) => OpGroup::Advanced,
            Self::MusicRename(_) => OpGroup::Music,
            Self::MusicTagger(_) | Self::RemoveTags(_) => OpGroup::Music,
            Self::Script(_) => OpGroup::Advanced,
        }
    }

    /// What a fresh card of this operation is scoped to, and what a job file
    /// that omits `scope` means.
    ///
    /// The name everywhere except Free Format, whose typical pattern
    /// (`<Parent>_<FullName>` → `May09_0001.jpg`) rebuilds the whole filename:
    /// scoped to the name, the engine would put the extension back on the end
    /// of a pattern that already carries it.
    /// Exhaustive on purpose. The catch-all this replaced answered `Scope::Name`
    /// for anything it did not recognise — a *silently* wrong answer for the
    /// next operation added, rather than a compile error.
    pub fn default_scope(&self) -> Scope {
        match self {
            Self::FreeFormat(_) => Scope::Both,
            Self::Replace(_)
            | Self::BatchReplace(_)
            | Self::Casing(_)
            | Self::AddRemove(_)
            | Self::MoveSection(_)
            | Self::SpaceTrim(_)
            | Self::AddCounter(_)
            | Self::ReNumber(_)
            | Self::ZeroPadding(_)
            | Self::CsvList(_)
            | Self::FilenameEditor(_) => Scope::Name,
            // What a script is handed depends on the Process Name and Process
            // Extension settings, and the shipped scripts are written for the
            // default of those: `Safe Characters` and
            // `Insert Space Before Caps` transliterate a stem, and the one
            // script that wants the extension asks for `full_filename` by name.
            Self::Script(_) => Scope::Name,
            // An action has no scope at all — it does not touch the name. The
            // value is never read (`Pipeline::evaluate` skips straight past it
            // for an action step); it is here because the match is exhaustive,
            // which is the point.
            Self::SetAttributes(_) | Self::SetDate(_) => Scope::Name,
            // Neither touches the name at all, so scope is meaningless to
            // them; `Scope::Name` is what an action's slicing already ignores
            // (`Pipeline::evaluate` skips scope for `Step::Action`).
            Self::MusicTagger(_) | Self::RemoveTags(_) => Scope::Name,
            // `Scope::Name`, and it is worth saying why it is *not* Free
            // Format's `Both`. D36 gave Free Format `Both` because its
            // typical pattern is `<Parent>_<FullName>` — the pattern
            // already carries the extension, so scoping to the name would
            // append it twice. A music style is `<Artist> - <Title>`, which
            // carries no extension at all, so `Both` would *destroy* it:
            // Process Name ticked, Process Extension clear.
            Self::MusicRename(_) => Scope::Name,
        }
    }

    /// Whether this operation renames a file or acts on it. See [`Produces`].
    ///
    /// Derived from [`Self::as_step`] rather than written out again, so the two
    /// cannot disagree.
    pub fn produces(&self) -> Produces {
        match self.as_step() {
            StepRef::Name(_) => Produces::Name,
            StepRef::Action(_) => Produces::Action,
        }
    }

    /// The label shown to a human.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Replace(_) => "Replace",
            Self::BatchReplace(_) => "Batch Replace",
            Self::Casing(_) => "Set Casing",
            Self::AddRemove(_) => "Add / Remove",
            Self::MoveSection(_) => "Move Section",
            Self::SpaceTrim(_) => "Space Trimming",
            Self::AddCounter(_) => "Add Counter",
            Self::ReNumber(_) => "Re-Number",
            Self::ZeroPadding(_) => "Zero Padding",
            Self::FreeFormat(_) => "Free Format",
            Self::CsvList(_) => "CSV List Rename",
            Self::FilenameEditor(_) => "Filename Editor",
            Self::SetAttributes(_) => "Set Attributes",
            Self::SetDate(_) => "Set Date & Time",
            Self::MusicRename(_) => "Music Rename",
            Self::MusicTagger(_) => "Music Tagger",
            Self::RemoveTags(_) => "Remove Tags",
            Self::Script(_) => "Scripting",
        }
    }

    /// The one-line reason this operation cannot run, if it has one.
    ///
    /// **Configuration only.** The GUI draws this every frame for every card,
    /// so anything it reads must come out of a cache rather than a fresh read
    /// per frame (P44), and it must not depend on the listing — a card cannot
    /// know how many files are showing or what dates they carry. Anything
    /// listing-dependent surfaces through the plan instead, as a row error
    /// (P4).
    ///
    /// Most operations are answered by running them once against a made-up
    /// file, and the made-up file is built so that only the configuration can
    /// fail: it carries all three timestamps, because every real file has a
    /// modified date and an operation that shifts or partly rewrites one
    /// (Set Date's interval sources, a year-only mask) would otherwise be
    /// accused of needing a date that is always there. Two operations answer
    /// for themselves, because running them says the wrong thing: the Filename
    /// Editor pairs lines with the listing, and a one-file listing would
    /// report a line-count mismatch for every editor with more than one line;
    /// a script is too expensive to run per card per frame.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Script(op) => return op.problem(),
            Self::FilenameEditor(op) => return op.problem(),
            _ => {}
        }
        // A broken `<tag>` is the common case, and every tag-bearing operation
        // reports it the same way: by refusing to compile its template.
        let run = super::default_run();
        let mut entry = crate::model::FileEntry::synthetic("/example/name.txt");
        entry.modified = Some(run.now);
        entry.created = Some(run.now);
        entry.accessed = Some(run.now);
        let cx = crate::ops::EvalCx::new(&entry, 0, 1, run);
        match self.as_step() {
            StepRef::Name(transform) => transform.apply("name", &cx).err().map(|e| e.to_string()),
            StepRef::Action(action) => action.effect(&cx).err().map(|e| e.to_string()),
        }
    }

    /// Throws away everything this operation has derived from its
    /// configuration, so the next question is answered from the configuration
    /// as it is now.
    ///
    /// **D21.** Every [`Cached`](crate::cache::Cached) in an operation resets on
    /// `Clone` and on nothing else. That is right for the pipeline — `to_step`
    /// clones — and wrong for a caller that edits a live operation in place,
    /// which is what the GUI's card editors do: they write straight into
    /// `FilenameEditor::text`, `CsvList::file`, `Replace::find` and a Batch
    /// Replace's rule list, and ask the same value for its summary and its
    /// [`Self::problem`] on the next frame. Without this the card keeps
    /// whatever it derived first: a line count of 0, the compile error of a
    /// pattern typed half-way, a Batch Replace prefilter for a rule list that
    /// no longer exists. Call it after every in-place edit.
    ///
    /// A clone rather than a reset per type, because a clone is already what
    /// D21 defines as "the configuration without the work", and it cannot miss
    /// a cache added to an operation later.
    pub fn refresh(&mut self) {
        *self = self.clone();
    }

    /// One line describing what this operation is currently configured to do.
    pub fn summary(&self) -> String {
        match self.as_step() {
            StepRef::Name(t) => t.summary(),
            StepRef::Action(a) => a.summary(),
        }
    }

    /// The one exhaustive classifier. Every new variant must land in an arm
    /// here, which is what stops the next operation being silently misfiled.
    pub fn as_step(&self) -> StepRef<'_> {
        match self {
            Self::Replace(op) => StepRef::Name(op),
            Self::BatchReplace(op) => StepRef::Name(op),
            Self::Casing(op) => StepRef::Name(op),
            Self::AddRemove(op) => StepRef::Name(op),
            Self::MoveSection(op) => StepRef::Name(op),
            Self::SpaceTrim(op) => StepRef::Name(op),
            Self::AddCounter(op) => StepRef::Name(op),
            Self::ReNumber(op) => StepRef::Name(op),
            Self::ZeroPadding(op) => StepRef::Name(op),
            Self::FreeFormat(op) => StepRef::Name(op),
            Self::CsvList(op) => StepRef::Name(op),
            Self::FilenameEditor(op) => StepRef::Name(op),
            Self::SetAttributes(op) => StepRef::Action(op),
            Self::SetDate(op) => StepRef::Action(op),
            Self::MusicTagger(op) => StepRef::Action(op.as_ref()),
            Self::RemoveTags(op) => StepRef::Action(op),
            Self::MusicRename(op) => StepRef::Name(op),
            Self::Script(op) => StepRef::Name(op),
        }
    }

    /// A fresh step for a `Pipeline`.
    ///
    /// Cloning resets the operation's compiled cache (D21), which is correct:
    /// the caller is building a pipeline from possibly-edited configuration.
    pub fn to_step(&self) -> Step {
        use crate::pipeline::Step::{Action, Name};
        match self {
            Self::Replace(op) => Name(Box::new(op.clone())),
            Self::BatchReplace(op) => Name(Box::new(op.clone())),
            Self::Casing(op) => Name(Box::new(op.clone())),
            Self::AddRemove(op) => Name(Box::new(op.clone())),
            Self::MoveSection(op) => Name(Box::new(op.clone())),
            Self::SpaceTrim(op) => Name(Box::new(op.clone())),
            Self::AddCounter(op) => Name(Box::new(op.clone())),
            Self::ReNumber(op) => Name(Box::new(op.clone())),
            Self::ZeroPadding(op) => Name(Box::new(op.clone())),
            Self::FreeFormat(op) => Name(Box::new(op.clone())),
            Self::CsvList(op) => Name(Box::new(op.clone())),
            Self::FilenameEditor(op) => Name(Box::new(op.clone())),
            Self::SetAttributes(op) => Action(Box::new(*op)),
            Self::SetDate(op) => Action(Box::new(*op)),
            // `clone`, not `*op`: a tagger owns its seven templates.
            Self::MusicTagger(op) => Action(Box::new(op.as_ref().clone())),
            Self::RemoveTags(op) => Action(Box::new(*op)),
            Self::MusicRename(op) => Name(Box::new(op.clone())),
            Self::Script(op) => Name(Box::new(op.clone())),
        }
    }

    /// Builds an operation from a job-file name plus the rest of its table.
    ///
    /// The single place a name maps to a type — job files, presets and the GUI
    /// all come through here.
    pub fn from_table(name: &str, table: toml::Table) -> Result<Self, UnknownOp> {
        fn parse<T: serde::de::DeserializeOwned>(table: toml::Table) -> Result<T, UnknownOp> {
            table.try_into().map_err(UnknownOp::Invalid)
        }
        Ok(match name {
            "replace" => Self::Replace(parse(table)?),
            "batch_replace" => Self::BatchReplace(parse(table)?),
            "casing" => Self::Casing(parse(table)?),
            "add_remove" => Self::AddRemove(parse(table)?),
            "move_section" => Self::MoveSection(parse(table)?),
            "space_trim" => Self::SpaceTrim(parse(table)?),
            "add_counter" => Self::AddCounter(parse(table)?),
            "renumber" => Self::ReNumber(parse(table)?),
            "zero_padding" => Self::ZeroPadding(parse(table)?),
            "free_format" => Self::FreeFormat(parse(table)?),
            "csv_list" => Self::CsvList(parse(table)?),
            "filename_editor" => Self::FilenameEditor(parse(table)?),
            "set_attributes" => Self::SetAttributes(parse(table)?),
            "set_date" => Self::SetDate(parse(table)?),
            "music_tagger" => Self::MusicTagger(Box::new(parse(table)?)),
            "remove_tags" => Self::RemoveTags(parse(table)?),
            "music_rename" => Self::MusicRename(parse(table)?),
            "script" => Self::Script(parse(table)?),
            other => return Err(UnknownOp::Name(other.to_owned())),
        })
    }

    /// Comma-separated list of every valid name, for error messages.
    pub fn known_names() -> String {
        Self::all()
            .iter()
            .map(Self::name)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug)]
pub enum UnknownOp {
    Name(String),
    Invalid(toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datetime::DateComponents;
    use crate::ops::{CaseMode, DateSource, WallClock};

    #[test]
    fn every_operation_has_a_distinct_name_and_label() {
        let all = OpKind::all();
        let mut names: Vec<_> = all.iter().map(OpKind::name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "names must be unique");

        let mut labels: Vec<_> = all.iter().map(OpKind::label).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), count, "labels must be unique");
    }

    #[test]
    fn a_name_round_trips_through_from_table() {
        for op in OpKind::all() {
            let rebuilt = OpKind::from_table(op.name(), toml::Table::new())
                .unwrap_or_else(|e| panic!("{} should rebuild: {e:?}", op.name()));
            assert_eq!(rebuilt.name(), op.name());
        }
    }

    #[test]
    fn an_unknown_name_is_reported_with_the_offending_text() {
        let err = OpKind::from_table("replase", toml::Table::new()).unwrap_err();
        assert!(matches!(err, UnknownOp::Name(n) if n == "replase"));
        assert!(OpKind::known_names().contains("replace"));
    }

    #[test]
    fn the_summary_reflects_the_current_configuration() {
        let op = OpKind::Replace(Replace::new("_", " "));
        assert!(op.summary().contains('_'), "{}", op.summary());

        let op = OpKind::Casing(Casing::new(CaseMode::Upper));
        assert!(op.summary().contains("Upper"), "{}", op.summary());
    }

    #[test]
    fn to_step_produces_a_working_operation() {
        let op = OpKind::Replace(Replace::new("a", "b"));
        let entry = crate::model::FileEntry::synthetic("/tmp/aaa.txt");
        let cx = crate::ops::EvalCx::simple(&entry, 0, 1);
        match op.to_step() {
            crate::pipeline::Step::Name(transform) => {
                assert_eq!(transform.apply("aaa", &cx).unwrap(), "bbb");
            }
            crate::pipeline::Step::Action(_) => panic!("Replace is a name transform"),
        }
    }

    /// Every operation classifies itself, and the two views agree.
    #[test]
    fn every_operation_is_either_a_name_transform_or_an_action() {
        for op in OpKind::all() {
            match (op.as_step(), op.produces(), op.to_step()) {
                (StepRef::Name(_), Produces::Name, Step::Name(_))
                | (StepRef::Action(_), Produces::Action, Step::Action(_)) => {}
                _ => panic!("{} classifies itself two different ways", op.name()),
            }
            // Whatever it is, it can describe itself.
            assert!(!op.summary().is_empty(), "{}", op.name());
        }
    }

    /// A freshly-defaulted operation must not shout before the user has typed
    /// anything (P34).
    #[test]
    fn a_fresh_card_of_any_operation_reports_no_problem() {
        for op in OpKind::all() {
            assert_eq!(op.problem(), None, "{} complains when brand new", op.name());
        }
    }

    /// D21: a `Cached` resets only on clone, and the GUI edits a card's op in
    /// place — so whatever the card derived before the edit (a line count, a
    /// compiled pattern) outlives it until `refresh`.
    #[test]
    fn refresh_makes_an_in_place_edit_take_effect() {
        let mut op = OpKind::FilenameEditor(FilenameEditor::default());
        assert_eq!(op.summary(), "Filename Editor (empty)");
        let OpKind::FilenameEditor(editor) = &mut op else {
            unreachable!()
        };
        editor.text = "one\ntwo\nthree".to_owned();
        assert_eq!(
            op.summary(),
            "Filename Editor (empty)",
            "the line count the card derived first survives the edit"
        );
        op.refresh();
        assert_eq!(op.summary(), "Names from 3 typed lines");

        // Typing a pattern one key at a time: `(` alone does not compile.
        let mut op = OpKind::Replace(Replace::new("(", "").regex(true));
        assert!(op.problem().is_some());
        let OpKind::Replace(replace) = &mut op else {
            unreachable!()
        };
        replace.find = r"(\d+)".to_owned();
        op.refresh();
        assert_eq!(op.problem(), None);
    }

    /// `problem` is about the configuration. A card that is fine for any real
    /// listing must not be judged against the one-file, no-timestamp listing
    /// the probe makes up.
    #[test]
    fn a_configured_card_is_judged_on_its_configuration_not_on_a_made_up_listing() {
        let three_lines = OpKind::FilenameEditor(FilenameEditor::new("one\ntwo\nthree"));
        assert_eq!(three_lines.problem(), None);
        let typo = OpKind::FilenameEditor(FilenameEditor::new("one\n<Nmae>"));
        let message = typo
            .problem()
            .expect("a bad tag is a configuration problem");
        assert!(message.contains("<Nmae>"), "{message}");

        // The camera-clock case: every real file has a modified date to shift.
        let shift = OpKind::SetDate(SetDate {
            source: DateSource::AddInterval,
            ..Default::default()
        });
        assert_eq!(shift.problem(), None);

        // A year-only mask merges into the date the file already has.
        let year_only = OpKind::SetDate(SetDate {
            date: WallClock {
                year: 2008,
                ..WallClock::default()
            },
            change: DateComponents {
                year: true,
                month: false,
                day: false,
                hour: false,
                minute: false,
                second: false,
            },
            ..Default::default()
        });
        assert_eq!(year_only.problem(), None);

        // And a date that could never be written still says so.
        let out_of_range = OpKind::SetDate(SetDate {
            date: WallClock {
                year: 1970,
                ..WallClock::default()
            },
            ..Default::default()
        });
        assert!(out_of_range.problem().is_some());
    }

    #[test]
    fn operations_serialise_with_their_name_as_the_tag() {
        let op = OpKind::Casing(Casing::new(CaseMode::Title));
        let text = toml::to_string(&op).unwrap();
        assert!(text.contains(r#"op = "casing""#), "{text}");

        let back: OpKind = toml::from_str(&text).unwrap();
        assert_eq!(back, op);
    }

    /// **Every** operation, not just the one that happened to agree.
    ///
    /// D23 makes `name()` the wire name, so serde's own tag has to match it —
    /// otherwise a value written through serde is a file the parser rejects.
    /// `ReNumber` did exactly that until M4: serde spelled it `re_number` while
    /// `from_table` only accepted `renumber`.
    #[test]
    fn every_operation_serialises_under_the_name_it_parses_from() {
        for op in OpKind::all() {
            let text = toml::to_string(&op).unwrap();
            assert!(
                text.contains(&format!("op = {:?}", op.name())),
                "{} serialises as something else:\n{text}",
                op.name()
            );

            // And the whole way back out through the job-file reader, which is
            // where the mismatch actually bit.
            let table: toml::Table = toml::from_str(&text).unwrap();
            let rebuilt = OpKind::from_table(op.name(), strip_tag(table))
                .unwrap_or_else(|e| panic!("{} should rebuild: {e:?}", op.name()));
            assert_eq!(rebuilt, op, "{}", op.name());
        }
    }

    /// A blob written before the `renumber` rename still loads.
    ///
    /// The GUI persists through eframe as RON, so without the alias every user
    /// with a Re-Number configured would have their whole settings blob fail to
    /// parse and silently reset.
    #[test]
    fn the_old_re_number_spelling_still_deserialises() {
        let back: OpKind = toml::from_str(r#"op = "re_number""#).unwrap();
        assert_eq!(back.name(), "renumber");
    }

    fn strip_tag(mut table: toml::Table) -> toml::Table {
        table.remove("op");
        table
    }
}
