//! The RenameIt engine.
//!
//! Platform-neutral by construction (D3): everything OS-specific goes through
//! [`ren_platform::Platform`], which is injected by the caller. `ren-core`
//! builds and tests green on Linux even though Windows ships first.
//!
//! The M0 shape, which M1 fills in:
//!
//! ```text
//! listing  ──► Pipeline::evaluate (parallel, pure)  ──► plan() ──► apply()
//!                                                        │           │
//!                                                   conflicts    journal
//!                                                                    │
//!                                                                  undo()
//! ```

pub mod cache;
pub mod counter;
pub mod csv_table;
pub mod datetime;
pub mod effect;
pub mod exec;
pub mod filter;
pub mod job;
pub mod listing;
pub mod matcher;
pub mod meta;
pub mod model;
pub mod ops;
pub mod parts;
pub mod pipeline;
pub mod plan;
pub mod preproc;
pub mod preset;
pub mod regex_flavor;
pub mod run;
pub mod script;
pub mod template;
#[cfg(test)]
mod test_platform;
pub mod text;
pub mod wildcard;

pub use counter::CounterSetup;
pub use csv_table::{CsvError, CsvOptions, CsvTable};
pub use datetime::{DateComponents, DateProblem, IntervalUnit};
pub use effect::{Before, Effect, PlannedAction, TimeSet, TimeStamp, Undoability};
pub use exec::{ApplyOptions, ApplyReport, ExecError, UndoReport, apply, undo_last};
pub use filter::IncludeFilter;
pub use job::{Document, Job, JobError, PresetMeta, Source};
pub use listing::{ListOptions, list};
pub use matcher::{MatchSpec, Matcher};
pub use model::{FileEntry, Scope, Subject, split_file_name, split_name};
pub use ops::{
    AddCounter, AddRemove, AddRemoveMode, AppendSuffix, BatchReplace, CaseMode, Casing,
    CounterPlacement, EvalCx, FreeFormat, MoveSection, NameTransform, NumberAction, NumberOptions,
    NumberTarget, OpError, OpGroup, OpKind, Produces, ReNumber, Replace, Script, SpaceTrim,
    ZeroPadding,
};
pub use parts::{Parts, PartsSpec};
pub use pipeline::{NoSubject, Pipeline, Step, StepConfig, StepSubject, evaluate_all};
pub use plan::{ConflictKind, Counts, Plan, PlanItem, PlannedOp, RenameKind, RowState, plan};
pub use preproc::PreProcessor;
pub use preset::{ImportNotes, Preset, PresetEntry, PresetError, PresetStore};
pub use regex_flavor::{Pattern, PatternOptions, RegexError};
pub use run::{
    Answers, AskSpec, Interaction, NoInteraction, RunContext, RunSettings, StdinInteraction,
};
pub use template::{Rendered, Tag, TagError, Template, TemplateError, TextTemplate};
pub use text::plural;
