//! Job files: a listing plus an ordered pipeline, as TOML.
//!
//! **D18.** This is deliberately the same schema M4's presets will use, so
//! there is one format to document, test and evolve rather than a CLI
//! mini-language beside a preset format. A job file is an early preset that
//! happens to name its own source directory.
//!
//! ```toml
//! [source]
//! dir = "~/music"
//! subfolders = true
//!
//! [[step]]
//! op = "replace"
//! scope = "name"
//! find = "_"
//! replace = " "
//!
//! [[step]]
//! op = "casing"
//! mode = "title"
//! preserve_all_upper = true
//!
//! [settings]
//! parts = "<%1> - <%2>"
//! require_all_tags = true
//!
//! [settings.counter]
//! start = 1
//! step = 1
//! ```
//!
//! Each `[[step]]` carries the step settings (`scope`, `enabled`, `filter`,
//! `preproc`) alongside the operation's own fields, tagged by `op`. Unknown
//! keys are an error rather than a silent no-op — a typo in a job file that
//! renames a thousand files should stop the run, not change its meaning.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::listing::ListOptions;
use crate::model::Scope;
use crate::ops::{OpKind, UnknownOp};
use crate::pipeline::{Pipeline, StepConfig};
use crate::run::{Answers, RunSettings};

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("{0}")]
    Syntax(#[from] toml::de::Error),
    #[error("step {index}: missing `op`")]
    MissingOp { index: usize },
    #[error("step {index}: unknown operation {name:?} (expected one of: {known})")]
    UnknownOp {
        index: usize,
        name: String,
        known: String,
    },
    #[error("step {index} ({op}): {source}")]
    Step {
        index: usize,
        op: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("a job file needs at least one [[step]]")]
    NoSteps,
    #[error("this file needs a newer RenameIt (it says version {found}, this build reads {known})")]
    FromTheFuture { found: u32, known: u32 },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    File {
        path: PathBuf,
        #[source]
        source: Box<JobError>,
    },
    #[error("could not write: {0}")]
    Write(#[from] toml::ser::Error),
}

/// Which files the job runs over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Source {
    /// A leading `~/` is expanded, since a job file is not shell-expanded.
    pub dir: PathBuf,
    pub files: bool,
    pub folders: bool,
    pub subfolders: bool,
    /// The pattern box, e.g. `*.mp3`. Empty, `*` and `*.*` all mean everything.
    pub pattern: String,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("."),
            files: true,
            folders: false,
            subfolders: false,
            pattern: String::new(),
        }
    }
}

impl Source {
    pub fn list_options(&self) -> ListOptions {
        ListOptions {
            files: self.files,
            folders: self.folders,
            subfolders: self.subfolders,
            ..Default::default()
        }
        .with_pattern(&self.pattern)
    }

    /// The directory with `~` expanded.
    pub fn dir(&self) -> PathBuf {
        expand_home(&self.dir)
    }
}

fn expand_home(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(rest) = text.strip_prefix("~/") else {
        return path.to_path_buf();
    };
    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) => PathBuf::from(home).join(rest),
        None => path.to_path_buf(),
    }
}

/// What a preset calls itself. Absent from a hand-written job file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PresetMeta {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl PresetMeta {
    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.description.is_empty()
    }
}

/// The schema version a file declares. Bumped only by a change older builds
/// could not read correctly.
pub const SCHEMA_VERSION: u32 = 1;

/// A parsed file, before anyone decides whether it is a job or a preset.
///
/// **D33.** One grammar, two readings. A job file names its own source and must
/// do something; a preset borrows the caller's source and may legitimately be
/// empty (the **New** button creates one). Splitting those rules out
/// of the parser is what lets a preset and a job file be the same file — which
/// is D18's whole point, and why `examples/cleanup.toml` can be imported as a
/// preset unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document {
    pub meta: PresetMeta,
    /// `None` when the file names no source. Distinct from "the current
    /// directory": loading a preset must not move the user's browsing folder.
    pub source: Option<Source>,
    pub steps: Vec<(OpKind, StepConfig)>,
    pub settings: RunSettings,
}

/// A parsed job file.
#[derive(Debug)]
pub struct Job {
    pub source: Source,
    /// The operations as data, in file order — what the GUI edits and what M4
    /// will save as a preset.
    pub steps: Vec<(OpKind, StepConfig)>,
    /// `[settings]` — the counter, the parts pattern and the tag policy, which
    /// belong to the run rather than to any one step.
    pub settings: RunSettings,
}

impl Job {
    /// Builds a runnable pipeline from the steps.
    pub fn pipeline(&self) -> Pipeline {
        self.pipeline_with(Answers::default())
    }

    /// The same, with the `<Ask>` answers the caller collected.
    pub fn pipeline_with(&self, answers: Answers) -> Pipeline {
        let mut pipeline = Pipeline::new();
        for (op, config) in &self.steps {
            pipeline.push(op.to_step(), config.clone());
        }
        pipeline.settings = self.settings.clone();
        pipeline.answers = answers;
        pipeline
    }
}

/// The step settings every operation shares. Pulled out before the rest of the
/// table is handed to the operation itself.
///
/// Every field is optional so an *absent* `scope` can fall back to the
/// operation's own default rather than to `Scope`'s.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawStepConfig {
    scope: Option<Scope>,
    enabled: Option<bool>,
    filter: Option<crate::filter::IncludeFilter>,
    preproc: Option<crate::preproc::PreProcessor>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJob {
    /// Reserved so a later schema can be *recognised* rather than reported as
    /// an unknown field. `deny_unknown_fields` is what makes this a one-line
    /// decision now and an impossible one afterwards.
    #[serde(default = "one")]
    version: u32,
    #[serde(default)]
    preset: PresetMeta,
    #[serde(default)]
    source: Option<Source>,
    #[serde(default)]
    settings: RunSettings,
    #[serde(default)]
    step: Vec<toml::Table>,
}

fn one() -> u32 {
    SCHEMA_VERSION
}

/// Fields consumed by the step itself rather than by the operation.
const STEP_KEYS: [&str; 5] = ["op", "scope", "enabled", "filter", "preproc"];

impl Document {
    pub fn parse(text: &str) -> Result<Self, JobError> {
        let raw: RawJob = toml::from_str(text)?;
        if raw.version > SCHEMA_VERSION {
            return Err(JobError::FromTheFuture {
                found: raw.version,
                known: SCHEMA_VERSION,
            });
        }

        let mut steps = Vec::with_capacity(raw.step.len());
        for (index, mut table) in raw.step.into_iter().enumerate() {
            let op_name = table
                .remove("op")
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or(JobError::MissingOp { index })?;

            // Split the table: step settings here, operation settings there.
            let mut config_table = toml::Table::new();
            for key in STEP_KEYS {
                if let Some(value) = table.remove(key) {
                    config_table.insert(key.to_owned(), value);
                }
            }

            let raw_config: RawStepConfig =
                config_table.try_into().map_err(|source| JobError::Step {
                    index,
                    op: op_name.clone(),
                    source,
                })?;

            // The operation is built first, because what `scope` defaults to
            // depends on which operation it is.
            let op = build_op(index, &op_name, table)?;
            let config = StepConfig {
                scope: raw_config.scope.unwrap_or_else(|| op.default_scope()),
                enabled: raw_config.enabled.unwrap_or(true),
                filter: raw_config.filter,
                preproc: raw_config.preproc,
            };

            steps.push((op, config));
        }

        Ok(Self {
            meta: raw.preset,
            source: raw.source,
            steps,
            settings: raw.settings,
        })
    }

    pub fn from_file(path: &Path) -> Result<Self, JobError> {
        let text = std::fs::read_to_string(path).map_err(|source| JobError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text).map_err(|source| JobError::File {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    }
}

impl Job {
    /// A job file: it names its own source, and it must do something.
    pub fn parse(text: &str) -> Result<Self, JobError> {
        let doc = Document::parse(text)?;
        if doc.steps.is_empty() {
            return Err(JobError::NoSteps);
        }
        Ok(Self {
            source: doc.source.unwrap_or_default(),
            steps: doc.steps,
            settings: doc.settings,
        })
    }

    pub fn from_file(path: &Path) -> Result<Self, JobError> {
        let text = std::fs::read_to_string(path).map_err(|source| JobError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text).map_err(|source| JobError::File {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    }
}

// --- Writing ----------------------------------------------------------------

impl Document {
    /// The exact inverse of [`Document::parse`].
    ///
    /// Everything in the schema is `#[serde(default)]`, so omitting a value
    /// that equals its default is provably lossless — and it is what makes a
    /// saved preset read like a hand-written one instead of a dump of every
    /// field every operation has.
    pub fn to_toml(&self) -> Result<String, JobError> {
        let out = RawDocOut {
            version: SCHEMA_VERSION,
            preset: &self.meta,
            source: self.source.as_ref(),
            settings: stripped(&self.settings, &RunSettings::default())?,
            step: self
                .steps
                .iter()
                .map(|(op, config)| StepTable { op, config })
                .collect(),
        };
        toml::to_string_pretty(&out).map_err(JobError::Write)
    }
}

#[derive(Serialize)]
struct RawDocOut<'a> {
    version: u32,
    #[serde(skip_serializing_if = "PresetMeta::is_empty")]
    preset: &'a PresetMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a Source>,
    #[serde(skip_serializing_if = "toml::Table::is_empty")]
    settings: toml::Table,
    step: Vec<StepTable<'a>>,
}

/// One `[[step]]`: the step's own settings and the operation's fields in a
/// single table, which is what [`Document::parse`] splits back apart.
struct StepTable<'a> {
    op: &'a OpKind,
    config: &'a StepConfig,
}

impl serde::Serialize for StepTable<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error, SerializeMap};

        let mut map = serializer.serialize_map(None)?;
        // From `name()`, never from the enum's serde tag. D23 makes `name()`
        // the wire name, and the two disagreed for a whole milestone without
        // anyone noticing, because nothing wrote an operation out.
        map.serialize_entry("op", self.op.name())?;
        // Always spelled out: it is the one setting whose default depends on
        // which operation this is, so leaving it implicit would make a preset
        // readable only next to the source.
        map.serialize_entry("scope", &self.config.scope)?;
        if !self.config.enabled {
            map.serialize_entry("enabled", &false)?;
        }
        for (key, value) in op_fields(self.op).map_err(S::Error::custom)? {
            map.serialize_entry(&key, &value)?;
        }
        if let Some(filter) = &self.config.filter {
            map.serialize_entry("filter", filter)?;
        }
        if let Some(preproc) = &self.config.preproc {
            map.serialize_entry("preproc", preproc)?;
        }
        map.end()
    }
}

/// The operation's own fields, minus serde's tag and minus anything still at
/// its default.
fn op_fields(op: &OpKind) -> Result<toml::Table, JobError> {
    let mut table = stripped(op, &default_of(op))?;
    table.remove("op");
    Ok(table)
}

/// The same operation, freshly defaulted. `from_table` with an empty table is
/// exactly that, and `a_name_round_trips_through_from_table` already proves it
/// works for every operation.
fn default_of(op: &OpKind) -> OpKind {
    OpKind::from_table(op.name(), toml::Table::new())
        .expect("every operation builds from an empty table")
}

/// `value` as a table, with every key that matches `base` removed.
fn stripped<T: Serialize>(value: &T, base: &T) -> Result<toml::Table, JobError> {
    let mut table = toml::Table::try_from(value).map_err(JobError::Write)?;
    let base = toml::Table::try_from(base).map_err(JobError::Write)?;
    strip_defaults(&mut table, &base);
    Ok(table)
}

/// Removes every key whose value is identical to the same key in `base`.
///
/// Recurses into sub-tables and drops one that empties out. Arrays are compared
/// whole and never recursed into: a Batch Replace list that still equals the
/// shipped 51 rules vanishes, and one the user has touched is written in full.
fn strip_defaults(table: &mut toml::Table, base: &toml::Table) {
    table.retain(|key, value| {
        let Some(default) = base.get(key) else {
            return true;
        };
        match (value, default) {
            (toml::Value::Table(inner), toml::Value::Table(inner_base)) => {
                strip_defaults(inner, inner_base);
                !inner.is_empty()
            }
            (value, default) => value != default,
        }
    });
}

/// D23: the name→type mapping lives in `OpKind`, not here.
fn build_op(index: usize, name: &str, table: toml::Table) -> Result<OpKind, JobError> {
    OpKind::from_table(name, table).map_err(|e| match e {
        UnknownOp::Name(name) => JobError::UnknownOp {
            index,
            name,
            known: OpKind::known_names(),
        },
        UnknownOp::Invalid(source) => JobError::Step {
            index,
            op: name.to_owned(),
            source,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileEntry;
    use crate::ops::EvalCx;

    fn run(job: &Job, name: &str) -> String {
        let entry = FileEntry::synthetic(PathBuf::from("/tmp/music").join(name));
        job.pipeline()
            .evaluate(&EvalCx::simple(&entry, 0, 1))
            .unwrap()
            .name
    }

    #[test]
    fn a_minimal_job_parses_and_runs() {
        let job = Job::parse(
            r#"
            [[step]]
            op = "replace"
            find = "_"
            replace = " "
        "#,
        )
        .unwrap();

        assert_eq!(job.steps.len(), 1);
        assert_eq!(run(&job, "my_song.mp3"), "my song.mp3");
        // Source defaults to the current directory, files only.
        assert_eq!(job.source.dir(), PathBuf::from("."));
        assert!(job.source.list_options().files);
    }

    #[test]
    fn the_source_section_drives_the_listing() {
        let job = Job::parse(
            r#"
            [source]
            dir = "/music"
            folders = true
            subfolders = true

            [[step]]
            op = "space_trim"
        "#,
        )
        .unwrap();

        assert_eq!(job.source.dir(), PathBuf::from("/music"));
        let options = job.source.list_options();
        assert!(options.files && options.folders && options.subfolders);
    }

    #[test]
    fn steps_run_in_file_order_and_compose() {
        let job = Job::parse(
            r#"
            [[step]]
            op = "replace"
            find = "_"
            replace = " "

            [[step]]
            op = "casing"
            mode = "title"

            [[step]]
            op = "casing"
            scope = "extension"
            mode = "lower"
        "#,
        )
        .unwrap();

        assert_eq!(job.steps.len(), 3);
        assert_eq!(run(&job, "my_holiday_photo.JPG"), "My Holiday Photo.jpg");
    }

    #[test]
    fn step_settings_and_operation_settings_share_one_table() {
        let job = Job::parse(
            r#"
            [[step]]
            op = "add_remove"
            scope = "both"
            enabled = true
            mode = "add"
            insert = "!"
            add_pos = 0
            add_backwards = true
        "#,
        )
        .unwrap();
        assert_eq!(run(&job, "song.mp3"), "song.mp3!");
    }

    #[test]
    fn a_disabled_step_is_kept_but_skipped() {
        let job = Job::parse(
            r#"
            [[step]]
            op = "casing"
            enabled = false
            mode = "upper"
        "#,
        )
        .unwrap();
        assert_eq!(job.steps.len(), 1);
        assert_eq!(run(&job, "song.mp3"), "song.mp3");
    }

    #[test]
    fn filters_and_pre_processors_are_part_of_a_step() {
        let job = Job::parse(
            r#"
            [[step]]
            op = "casing"
            mode = "upper"

            [step.filter]
            include = { substring = "live" }

            [step.preproc]
            skip_first = 5
        "#,
        )
        .unwrap();

        assert_eq!(run(&job, "track live set.mp3"), "track LIVE SET.mp3");
        assert_eq!(run(&job, "studio.mp3"), "studio.mp3");
    }

    #[test]
    fn a_misspelled_operation_names_the_alternatives() {
        let err = Job::parse(
            r#"
            [[step]]
            op = "replase"
            find = "a"
        "#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("replase"), "{message}");
        assert!(message.contains("replace"), "{message}");
    }

    /// A typo in a job file that renames a thousand files must stop the run.
    #[test]
    fn a_misspelled_option_is_an_error_not_a_silent_default() {
        let err = Job::parse(
            r#"
            [[step]]
            op = "replace"
            find = "a"
            replase = "b"
        "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("replase"), "{err}");

        let err = Job::parse(
            r#"
            [[step]]
            op = "replace"
            find = "a"
            scop = "name"
        "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("scop"), "{err}");
    }

    #[test]
    fn a_step_without_an_op_is_rejected() {
        let err = Job::parse("[[step]]\nfind = \"a\"").unwrap_err();
        assert!(matches!(err, JobError::MissingOp { index: 0 }), "{err}");
    }

    #[test]
    fn a_job_with_no_steps_is_rejected() {
        let err = Job::parse("[source]\ndir = \"/tmp\"").unwrap_err();
        assert!(matches!(err, JobError::NoSteps), "{err}");
    }

    #[test]
    fn an_unknown_top_level_section_is_rejected() {
        let err = Job::parse("[sauce]\ndir = \"/tmp\"\n[[step]]\nop = \"space_trim\"").unwrap_err();
        assert!(err.to_string().contains("sauce"), "{err}");
    }

    #[test]
    fn a_leading_tilde_in_the_source_path_is_expanded() {
        let job = Job::parse("[source]\ndir = \"~/music\"\n[[step]]\nop = \"space_trim\"").unwrap();
        let expanded = job.source.dir();
        assert!(!expanded.starts_with("~"), "{expanded:?}");
        assert!(expanded.ends_with("music"));
    }

    #[test]
    fn every_operation_is_reachable_from_a_job_file() {
        let job = Job::parse(
            r#"
            [[step]]
            op = "replace"
            find = "_"
            replace = " "

            [[step]]
            op = "batch_replace"
            rules = [{ find = "  ", replace = " " }]

            [[step]]
            op = "casing"
            mode = "title"

            [[step]]
            op = "add_remove"
            mode = "remove"
            delete = 0

            [[step]]
            op = "move_section"
            cut = 0

            [[step]]
            op = "space_trim"
        "#,
        )
        .unwrap();
        assert_eq!(job.steps.len(), 6);
    }

    // --- M4: the document, the writer, and the round trip --------------------

    fn doc(text: &str) -> Document {
        Document::parse(text).unwrap_or_else(|e| panic!("should parse: {e}"))
    }

    /// A preset names itself. Before M4 `deny_unknown_fields` rejected the
    /// table outright, so no preset could have been written at all.
    #[test]
    fn a_preset_table_sits_alongside_the_steps() {
        let d = doc(r#"
            [preset]
            name = "Photo cleanup"
            description = "Underscores out, title case."

            [[step]]
            op = "replace"
            find = "_"
            replace = " "
        "#);
        assert_eq!(d.meta.name, "Photo cleanup");
        assert_eq!(d.meta.description, "Underscores out, title case.");
        assert!(d.source.is_none(), "a preset names no source");
    }

    #[test]
    fn a_typo_inside_the_preset_table_is_still_an_error() {
        assert!(Document::parse("[preset]\nnmae = \"x\"").is_err());
    }

    /// The GUI must be able to tell "no source" from "the current directory",
    /// or loading a preset would move the user's browsing folder.
    #[test]
    fn an_absent_source_is_distinguishable_from_a_default_one() {
        assert!(doc("[[step]]\nop = \"space_trim\"").source.is_none());
        let named = doc("[source]\ndir = \".\"\n\n[[step]]\nop = \"space_trim\"");
        assert_eq!(named.source.unwrap().dir(), PathBuf::from("."));
    }

    /// "New creates an empty preset" — legal as a document, still refused as a
    /// job, which must actually do something.
    #[test]
    fn an_empty_document_is_legal_but_an_empty_job_is_not() {
        assert!(doc("[preset]\nname = \"Empty\"").steps.is_empty());
        assert!(matches!(
            Job::parse("[preset]\nname = \"Empty\""),
            Err(JobError::NoSteps)
        ));
    }

    /// Reserved in M4 so that a later schema can be *recognised*. Without it,
    /// `deny_unknown_fields` would make an older build report a missing feature
    /// as a typo.
    #[test]
    fn a_file_from_a_newer_version_says_so_rather_than_naming_a_field() {
        let err = Document::parse("version = 99\n").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("newer RenameIt"), "{message}");
        assert!(message.contains("99"), "{message}");
        // And a file with no version at all is simply version 1.
        assert!(Document::parse("[[step]]\nop = \"space_trim\"").is_ok());
    }

    /// Free Format rebuilds the whole name, extension included — so a file that
    /// leaves `scope` out means Both for that operation and Name for the rest.
    #[test]
    fn an_omitted_scope_follows_the_operation() {
        let d = doc(r#"
            [[step]]
            op = "free_format"
            pattern = "<Parent>_<FullName>"

            [[step]]
            op = "space_trim"
        "#);
        assert_eq!(d.steps[0].1.scope, Scope::Both);
        assert_eq!(d.steps[1].1.scope, Scope::Name);
    }

    #[test]
    fn the_writer_omits_what_is_already_the_default() {
        let d = doc("[[step]]\nop = \"space_trim\"");
        let text = d.to_toml().unwrap();

        assert!(text.contains(r#"op = "space_trim""#), "{text}");
        assert!(text.contains(r#"scope = "name""#), "{text}");
        // Space Trimming has six settings, all at their defaults here.
        assert!(!text.contains("leading"), "{text}");
        assert!(!text.contains("enabled"), "{text}");
        assert!(!text.contains("[settings]"), "{text}");
        assert!(!text.contains("[preset]"), "{text}");
        assert!(!text.contains("[source]"), "{text}");
    }

    #[test]
    fn the_writer_keeps_what_differs_from_the_default() {
        let d = doc(r#"
            [[step]]
            op = "space_trim"
            enabled = false
            leading = false
        "#);
        let text = d.to_toml().unwrap();
        assert!(text.contains("enabled = false"), "{text}");
        assert!(text.contains("leading = false"), "{text}");
    }

    /// D27 ships Batch Replace pre-loaded with 51 rules, and D35 makes that
    /// absence rather than a 51-entry blob in every preset that uses it.
    #[test]
    fn an_untouched_batch_replace_list_is_written_as_absence() {
        let d = doc("[[step]]\nop = \"batch_replace\"");
        let text = d.to_toml().unwrap();
        assert!(
            !text.contains("rules"),
            "the shipped list should not be written out:\n{text}"
        );
        // And it comes back as the shipped list.
        assert_eq!(doc(&text).steps[0].0, d.steps[0].0);

        // An edited list is written in full, so a preset stays self-contained.
        let edited = doc(r#"
            [[step]]
            op = "batch_replace"
            rules = [{ find = "a", replace = "b" }]
        "#);
        let text = edited.to_toml().unwrap();
        assert!(text.contains("find"), "{text}");
        assert_eq!(doc(&text).steps[0].0, edited.steps[0].0);
    }

    /// An empty list is a real choice — "run no rules" — and distinct from
    /// leaving the key out.
    #[test]
    fn an_empty_rule_list_survives_the_round_trip() {
        let d = doc("[[step]]\nop = \"batch_replace\"\nrules = []");
        let text = d.to_toml().unwrap();
        assert!(text.contains("rules = []"), "{text}");
        assert_eq!(doc(&text).steps[0].0, d.steps[0].0);
    }

    /// Neither of these has ever been serialised anywhere in the tree, and both
    /// nest a tagged enum before plain fields — the shape most likely to break.
    #[test]
    fn a_step_carrying_a_filter_and_a_preprocessor_round_trips() {
        let d = doc(r#"
            [[step]]
            op = "casing"
            mode = "upper"

            [step.filter]
            include = { substring = "live" }
            case_sensitive = true

            [step.preproc]
            skip_first = 4
        "#);
        let text = d.to_toml().unwrap();
        let back = doc(&text);
        assert_eq!(back.steps, d.steps, "wrote:\n{text}");
    }

    #[test]
    fn the_run_settings_round_trip() {
        let d = doc(r#"
            [settings]
            parts = "<%1> - <%2>"
            require_all_tags = true

            [settings.counter]
            start = 7
            step = 2

            [[step]]
            op = "add_counter"
        "#);
        let text = d.to_toml().unwrap();
        assert_eq!(doc(&text).settings, d.settings, "wrote:\n{text}");
    }

    /// Every operation, through the writer and back — the guard that a new
    /// operation cannot quietly become unwritable.
    #[test]
    fn every_operation_survives_the_writer() {
        for op in OpKind::all() {
            let d = Document {
                steps: vec![(op.clone(), StepConfig::for_op(&op))],
                ..Default::default()
            };
            let text = d.to_toml().unwrap();
            let back = Document::parse(&text)
                .unwrap_or_else(|e| panic!("{} wrote unreadable TOML: {e}\n{text}", op.name()));
            assert_eq!(back.steps, d.steps, "{} round trip:\n{text}", op.name());
        }
    }

    /// Writing is idempotent: the second pass must not drift from the first.
    #[test]
    fn writing_what_was_written_gives_the_same_file() {
        let d = doc(r#"
            [preset]
            name = "Everything"

            [settings.counter]
            start = 3

            [[step]]
            op = "replace"
            find = "_"
            replace = " "

            [[step]]
            op = "renumber"
            action = "add"
            operand = "1"
            enabled = false
        "#);
        let once = d.to_toml().unwrap();
        let twice = doc(&once).to_toml().unwrap();
        assert_eq!(once, twice);
    }

    /// The shipped examples are the D18 contract, and nothing guarded them.
    #[test]
    fn every_shipped_example_parses_and_round_trips() {
        for (name, text) in [
            (
                "cleanup.toml",
                include_str!("../../../examples/cleanup.toml"),
            ),
            (
                "numbered.toml",
                include_str!("../../../examples/numbered.toml"),
            ),
        ] {
            let d = Document::parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            let written = d.to_toml().unwrap_or_else(|e| panic!("{name}: {e}"));
            let back = Document::parse(&written).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(back.steps, d.steps, "{name}");
            assert_eq!(back.settings, d.settings, "{name}");
        }
    }
}
