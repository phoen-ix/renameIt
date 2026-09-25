//! Operations — every function group: the name transforms, and the actions
//! that change a file without renaming it.
//!
//! A name transform works on the slice of the name the engine hands it. Scope,
//! the pre-processor and the include filter are all resolved before it runs
//! (`docs/DESIGN.md` Part 1 §2), which is why each one here is small enough to
//! test exhaustively. What either kind may do beyond its own string — read a
//! file, depend on row order — is stated on [`NameTransform`] and
//! [`SideEffectAction`].

use std::borrow::Cow;

use crate::effect::{Effect, Undoability};
use crate::model::FileEntry;
use crate::run::RunContext;

pub mod add_counter;
pub mod add_remove;
pub mod casing;
pub mod csv_list;
pub mod filename_editor;
pub mod free_format;
pub mod kind;
pub mod move_section;
pub mod music_rename;
pub mod music_tagger;
pub mod numbers;
pub mod remove_tags;
pub mod renumber;
pub mod replace;
pub mod script;
pub mod set_attributes;
pub mod set_date;
pub mod space_trim;
pub mod zero_pad;

pub use add_counter::{AddCounter, CounterPlacement};
pub use add_remove::{AddRemove, AddRemoveMode};
pub use casing::{CaseMode, Casing, CasingRules, ExceptionRules, TitleCaseRules};
pub use csv_list::{CsvList, CsvSeparator};
pub use filename_editor::FilenameEditor;
pub use free_format::FreeFormat;
pub use kind::{OpGroup, OpKind, Produces, StepRef, UnknownOp};
pub use move_section::MoveSection;
pub use music_rename::MusicRename;
pub use music_tagger::MusicTagger;
pub use numbers::{MAX_PAD_WIDTH, NumberOptions, NumberSpan, NumberTarget};
pub use remove_tags::RemoveTags;
pub use renumber::{NumberAction, ReNumber};
pub use replace::{BatchReplace, Replace, regex_escape};
pub use script::Script;
pub use set_attributes::SetAttributes;
pub use set_date::{DateSource, DateTargets, SetDate, WallClock};
pub use space_trim::SpaceTrim;
pub use zero_pad::ZeroPadding;

/// Everything an operation is allowed to know about the file it is renaming.
#[derive(Debug, Clone, Copy)]
pub struct EvalCx<'a> {
    pub entry: &'a FileEntry,
    /// The whole file name as the pipeline has it *so far*. `<Name>` means the
    /// current name: a template in step three sees what steps one and two
    /// produced.
    pub current: &'a str,
    /// Position in the visible list — this is what drives the counter.
    pub index: usize,
    pub total: usize,
    /// Everything resolved before the parallel pass (D28): the counter
    /// sequence, `<Ask>` answers, the clipboard, the run's timestamp.
    pub run: &'a RunContext,
}

impl<'a> EvalCx<'a> {
    pub fn new(entry: &'a FileEntry, index: usize, total: usize, run: &'a RunContext) -> Self {
        Self {
            entry,
            current: &entry.file_name,
            index,
            total,
            run,
        }
    }

    /// The same context, with the name an earlier step produced.
    ///
    /// The borrowed name lives only as long as the step, which is shorter than
    /// the listing it came from — hence the second lifetime.
    #[must_use]
    pub fn with_current<'b>(&self, current: &'b str) -> EvalCx<'b>
    where
        'a: 'b,
    {
        EvalCx {
            entry: self.entry,
            current,
            index: self.index,
            total: self.total,
            run: self.run,
        }
    }

    /// A context with default run state, for operations that ignore it.
    pub fn simple(entry: &'a FileEntry, index: usize, total: usize) -> Self {
        Self::new(entry, index, total, default_run())
    }

    /// This file's counter value, already padded as configured.
    pub fn counter_text(&self) -> String {
        self.run.counter_text(self.index)
    }

    /// Renders one of an operation's tag fields.
    ///
    /// `None` means the run's *Only rename if all tags are available* switch is
    /// on and this file could not fill one in, or an `<Ask>` has not been
    /// answered yet — either way the caller must leave the name alone.
    /// A field with no tags in it is borrowed rather than rendered — that is
    /// most fields, and this runs once per file per keystroke.
    pub fn render<'f>(
        &self,
        field: &'f crate::template::TextTemplate,
    ) -> Result<Option<Cow<'f, str>>, OpError> {
        self.render_with(field, |value, out| out.push_str(value))
    }

    /// The same, with every **tag** value passed through `on_tag`.
    ///
    /// See `Template::render_with`: this exists for Find & Replace's
    /// replacement field, the one field whose output is read back by a regex
    /// engine. The all-literal fast path below is why a field without tags
    /// never pays for it — and why a `$1` the user typed is never touched.
    pub fn render_with<'f>(
        &self,
        field: &'f crate::template::TextTemplate,
        on_tag: impl FnMut(&str, &mut String),
    ) -> Result<Option<Cow<'f, str>>, OpError> {
        let template = field
            .compiled()
            .map_err(|e| OpError::new("tags", e.to_string()))?;
        if let Some(text) = template.literal_text() {
            return Ok(Some(Cow::Borrowed(text)));
        }
        let rendered = template.render_with(self, on_tag);
        // A question nobody has been asked yet is not an empty answer. The
        // preview runs before the `<Ask>` modal by construction (D28), so
        // without this every name would be rewritten around the gap and, for
        // an operation like Free Format that replaces the whole name, *become*
        // the gap.
        if rendered.awaiting_input || (self.run.require_all_tags && !rendered.available()) {
            return Ok(None);
        }
        Ok(Some(Cow::Owned(rendered.text)))
    }
}

/// A shared empty run context, so a caller that does not care about counters
/// or `<Ask>` need not build one.
fn default_run() -> &'static RunContext {
    static RUN: std::sync::OnceLock<RunContext> = std::sync::OnceLock::new();
    RUN.get_or_init(RunContext::default)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpError {
    #[error("{operation}: {message}")]
    Failed {
        operation: &'static str,
        message: String,
    },
}

impl OpError {
    pub(crate) fn new(operation: &'static str, message: impl std::fmt::Display) -> Self {
        Self::Failed {
            operation,
            message: message.to_string(),
        }
    }
}

/// A step that produces a new name.
///
/// **Never writes.** `apply` runs once per file per keystroke, inside the
/// parallel preview pass (`docs/DESIGN.md` Part 1 §4), so a write here would
/// happen on every keystroke and before anyone confirmed anything. It *may*
/// read — CSV List Rename reads its list, and a template's `<Artist>` or
/// `<Crc32>` opens the file — but only behind a process-wide cache keyed on the
/// file's stamp (P44), declared through [`Self::needs`], so a warm keystroke
/// costs no IO.
///
/// **Order-independent**, unless [`Self::is_order_sensitive`] says otherwise:
/// the default is what lets the pass use rayon, and Scripting is the one
/// operation that opts out.
pub trait NameTransform: Send + Sync + std::fmt::Debug {
    /// Stable identifier, used in presets and journals.
    fn id(&self) -> &'static str;

    /// One-line summary for the operation card in the GUI.
    fn summary(&self) -> String;

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError>;

    /// What this operation's tag fields will cost — empty for the operations
    /// that have none, which is most of them.
    fn needs(&self) -> crate::template::TagNeeds {
        crate::template::TagNeeds::NONE
    }

    /// The `<Ask>` slots its tag fields use, so the run can collect them all in
    /// one go before evaluating anything (D28).
    fn asks(&self) -> Vec<crate::run::AskSpec> {
        Vec::new()
    }

    /// Whether this operation must see rows **in list order**.
    ///
    /// Almost nothing does, and the default answer is what keeps the preview
    /// fast: [`crate::pipeline::evaluate_all_with`] is a rayon `par_iter`
    /// precisely because a transform is a pure function of one string.
    ///
    /// Scripting is the exception, and the exception is real rather than
    /// convenient: a script's globals stay static for the whole rename
    /// session, and three of the nine shipped scripts count, accumulate or
    /// hand out values in sequence.
    /// Evaluated in parallel, such a script produces a different preview every
    /// time it runs. A pipeline containing one is therefore evaluated serially
    /// — a real cost, paid only by the runs that need it.
    fn is_order_sensitive(&self) -> bool {
        false
    }

    /// Called once, before the first row, on an ordered run.
    ///
    /// **Only** called when the pipeline is evaluating in order, which is to
    /// say when something in it answered [`Self::is_order_sensitive`]. An
    /// operation that relies on this being called must say so there.
    ///
    /// This is where a script's session opens: the whole listing and the run
    /// context are both in scope here and neither is available per row.
    fn begin_run(&self, _entries: &[FileEntry], _run: &crate::run::RunContext) {}

    /// Called once, after the last row, on an ordered run.
    ///
    /// The counterpart to [`Self::begin_run`], and the only route by which an
    /// operation can ask for something outside the rename itself. What comes
    /// back is a *request*, not a change: `plan` turns it into a
    /// [`PlannedOp`](crate::plan::PlannedOp) and the executor decides whether
    /// it happens, so a preview still cannot write anything.
    fn end_run(&self) -> RunOutcome {
        RunOutcome::default()
    }
}

/// A file an operation wants written.
///
/// Requested here and performed by the executor, which is the same split that
/// keeps every other side effect undoable: the operation decides *what*, and
/// `crate::exec` does it, journals it, and can take it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    /// Absolute. A relative path would resolve against the process's working
    /// directory, which is nothing the user chose — so the requester is
    /// expected to have built a full path, and `plan` refuses anything else.
    pub path: std::path::PathBuf,
    pub contents: String,
}

/// What an ordered run produced beyond the names themselves.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub writes: Vec<FileWrite>,
    /// Lines for the log.
    ///
    /// Whatever a script's `done()` returns goes to the log, and a script that
    /// went wrong is reported the same way rather than failing the batch. Both
    /// land here.
    pub notes: Vec<String>,
}

/// A step that changes a file without renaming it.
///
/// Pure in the sense that matters, exactly like [`NameTransform`]: `effect`
/// runs inside the parallel evaluation pass, so it must be order-independent
/// and must never *write*. Reading is allowed and sometimes necessary —
/// `<Crc32>` already opens files from the same pass — but anything expensive
/// belongs behind a process-wide, mtime-keyed cache (P44), because this runs
/// once per file per keystroke.
///
/// Reading the before-image and making the syscall stay the executor's job; see
/// [`crate::exec::apply`]. That is the split that keeps undo possible.
///
/// `docs/DESIGN.md` sketched this as `execute(&self, entry, &dyn Platform,
/// &mut Journal)`. That cannot work: the executor owns the journal and the
/// write-ahead ordering, and a `Platform` inside `evaluate_all_with`'s
/// `par_iter` would destroy the property that makes the preview safe to run on
/// every keystroke. So the action decides *what*, and the executor does it.
pub trait SideEffectAction: Send + Sync + std::fmt::Debug {
    /// Stable identifier, used in presets and journals.
    fn id(&self) -> &'static str;

    /// One-line summary for the operation card in the GUI.
    fn summary(&self) -> String;

    /// What this run will do to this file. `None` means "nothing to this one",
    /// the same tri-state [`EvalCx::render`] already uses for names.
    fn effect(&self, cx: &EvalCx<'_>) -> Result<Option<Effect>, OpError>;

    /// How the row describes itself in the preview — "Modified → 2008-02-17
    /// 11:23:50", "Write Protect off". Given the effect it just produced, so it
    /// never has to compute anything twice.
    fn describe(&self, effect: &Effect) -> String;

    /// How much of what this operation does a run can take back (P2).
    ///
    /// Required, with no default, on purpose: the card has to warn *before* a
    /// file exists to compute an effect from, and a default would let the next
    /// irreversible operation be written without anyone deciding.
    fn undoable(&self) -> Undoability;

    fn needs(&self) -> crate::template::TagNeeds {
        crate::template::TagNeeds::NONE
    }

    fn asks(&self) -> Vec<crate::run::AskSpec> {
        Vec::new()
    }
}

// --- Position arithmetic -----------------------------------------------------
//
// P15: positions are zero-based — position 0 is before the first character —
// and count Unicode scalar values, not bytes. Out-of-range values saturate, so
// a large count such as `999` means "everything after here".

/// Number of characters in `s`.
pub(crate) fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Byte offset of character position `pos`, saturating at the end.
pub(crate) fn byte_of_char(s: &str, pos: usize) -> usize {
    s.char_indices().nth(pos).map_or(s.len(), |(i, _)| i)
}

/// Byte offset for a position that may be counted from the end.
///
/// Counting backwards, the position starts at the end of the name and moves
/// towards the beginning. So backwards position 0 is the very end, which is
/// exactly what the shipped "Add suffix to end of filename" preset relies on
/// (add `<Ask>` at position 0, counting backwards).
pub(crate) fn byte_of_position(s: &str, pos: usize, backwards: bool) -> usize {
    let len = char_len(s);
    let forward = if backwards {
        len.saturating_sub(pos)
    } else {
        pos.min(len)
    };
    byte_of_char(s, forward)
}

/// A position as a card's summary writes it: `3`, or `3 from end` when it
/// counts backwards. The summary is the only text on a collapsed card, and
/// without this the shipped prefix and suffix presets read the same.
pub(crate) fn position_label(pos: usize, backwards: bool) -> String {
    if backwards {
        format!("{pos} from end")
    } else {
        pos.to_string()
    }
}

/// Byte range covering `count` characters starting at character `start`,
/// saturating at the end of the string.
pub(crate) fn byte_range(s: &str, start: usize, count: usize) -> std::ops::Range<usize> {
    let from = byte_of_char(s, start);
    let to = byte_of_char(s, start.saturating_add(count));
    from..to
}

/// Appends a fixed string. M0's placeholder operation, kept as the smallest
/// possible `NameTransform` for tests and the CLI's `--suffix` smoke path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendSuffix {
    pub suffix: String,
}

impl AppendSuffix {
    pub fn new(suffix: impl Into<String>) -> Self {
        Self {
            suffix: suffix.into(),
        }
    }
}

impl NameTransform for AppendSuffix {
    fn id(&self) -> &'static str {
        "append_suffix"
    }

    fn summary(&self) -> String {
        format!("Append {:?}", self.suffix)
    }

    fn apply<'a>(&self, subject: &'a str, _cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        if self.suffix.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }
        Ok(Cow::Owned(format!("{subject}{}", self.suffix)))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::path::PathBuf;

    /// A throwaway entry so operations can be exercised without touching disk.
    pub(crate) fn entry(name: &str) -> FileEntry {
        FileEntry::synthetic(PathBuf::from("/tmp").join(name))
    }

    /// Runs `op` over `subject` with a one-item context.
    pub(crate) fn run(op: &dyn NameTransform, subject: &str) -> String {
        let e = entry(subject);
        let cx = EvalCx::simple(&e, 0, 1);
        op.apply(subject, &cx)
            .unwrap_or_else(|err| panic!("{} failed: {err}", op.id()))
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::run;
    use super::*;

    #[test]
    fn append_suffix_appends() {
        assert_eq!(run(&AppendSuffix::new("_v2"), "song"), "song_v2");
    }

    #[test]
    fn an_empty_suffix_borrows_instead_of_allocating() {
        let e = testing::entry("song.mp3");
        let cx = EvalCx::simple(&e, 0, 1);
        let op = AppendSuffix::new("");
        assert!(matches!(op.apply("song", &cx).unwrap(), Cow::Borrowed(_)));
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        // "Ü" is one character, two bytes.
        assert_eq!(char_len("Über"), 4);
        assert_eq!(byte_of_char("Über", 1), 2);
        assert_eq!(byte_of_char("Über", 4), 5);
    }

    #[test]
    fn positions_saturate_instead_of_erroring() {
        assert_eq!(byte_of_char("abc", 99), 3);
        assert_eq!(byte_range("abc", 1, 999), 1..3);
        assert_eq!(byte_range("abc", 99, 999), 3..3);
    }

    /// Backwards position 0 is the very end — what "Add suffix to end of
    /// filename" depends on.
    #[test]
    fn backwards_positions_start_at_the_end() {
        assert_eq!(byte_of_position("abcdef", 0, true), 6);
        assert_eq!(byte_of_position("abcdef", 1, true), 5);
        assert_eq!(byte_of_position("abcdef", 6, true), 0);
        assert_eq!(byte_of_position("abcdef", 99, true), 0);
        assert_eq!(byte_of_position("abcdef", 0, false), 0);
        assert_eq!(byte_of_position("abcdef", 99, false), 6);
    }
}
