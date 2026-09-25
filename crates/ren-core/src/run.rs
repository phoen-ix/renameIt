//! What a run knows before it starts.
//!
//! **D28.** `docs/DESIGN.md` Part 1 §4 planned a serial *finalize* pass:
//! emit placeholders for stateful tags during the parallel pass, then walk
//! every file again in order to substitute them. That is not necessary. Every
//! tag that looks stateful is a pure function of the **input** listing:
//!
//! * `<Counter>` — its resets read the entry's own folder and name, never a
//!   transformed one, so the whole sequence is `fn(&[FileEntry]) -> Vec<i64>`.
//! * `<Rnd*>` — seeded (P16).
//! * `<Ask>` and `<Clipboard>` — collected once per run *before* evaluation,
//!   which `docs/DESIGN.md` itself specifies.
//! * `<NumFiles>`, `<NowDate>` — run-wide constants.
//!
//! So a cheap serial **pre**-pass fills this struct, and the per-file pass stays
//! parallel and order-independent.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::counter::CounterSetup;
use crate::model::FileEntry;
use crate::parts::{CompiledParts, PartsSpec};

/// What the engine needs from a user, without knowing there is a user.
///
/// The core never blocks on UI: the GUI collects every `<Ask>` in one modal
/// before the run, the CLI reads stdin.
pub trait Interaction {
    fn ask(&self, spec: &AskSpec) -> Option<String>;
    fn clipboard(&self) -> Option<String>;
}

/// One `<Ask>` the template contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskSpec {
    /// 0 for `<Ask>`, 1–9 for `<Ask-1>`…`<Ask-9>`.
    pub slot: u8,
}

impl AskSpec {
    pub fn prompt(&self) -> String {
        if self.slot == 0 {
            "Enter a value for <Ask>".to_owned()
        } else {
            format!("Enter a value for <Ask-{}>", self.slot)
        }
    }
}

/// Collects everything the pipeline wants asked, before the run.
///
/// The order is fixed by the slot number, so a script feeding stdin knows what
/// it is answering.
pub fn collect(asks: &[AskSpec], clipboard: bool, interaction: &dyn Interaction) -> Answers {
    let mut answers = Answers::default();
    for spec in asks {
        if let Some(text) = interaction.ask(spec) {
            answers.asks.insert(spec.slot, text);
        }
    }
    if clipboard {
        answers.clipboard = interaction.clipboard();
    }
    answers
}

/// An interaction that answers nothing — the preview before the user is asked.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoInteraction;

impl Interaction for NoInteraction {
    fn ask(&self, _spec: &AskSpec) -> Option<String> {
        None
    }
    fn clipboard(&self) -> Option<String> {
        None
    }
}

/// Answers gathered before a run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Answers {
    /// Slot 0 is `<Ask>`; 1–9 are `<Ask-1>`…`<Ask-9>`.
    pub asks: BTreeMap<u8, String>,
    pub clipboard: Option<String>,
}

impl Answers {
    pub fn ask(&self, slot: u8) -> Option<&str> {
        self.asks.get(&slot).map(String::as_str)
    }
}

/// Run-wide settings, stored alongside a pipeline and saved with a preset.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunSettings {
    pub counter: CounterSetup,
    pub parts: PartsSpec,
    /// Only rename a file if every tag its template names is available for it
    /// — the checkbox in Run Settings. A file missing one keeps its name,
    /// rather than being renamed with a gap where the tag would have been.
    pub require_all_tags: bool,
    /// Keeps `<Rnd*>` reproducible between a preview and the rename that
    /// follows it (P16).
    ///
    /// **Zero means "nobody has chosen one"**, and the front ends replace it —
    /// the GUI once per session, `ren-cli` once per invocation, both via
    /// [`Self::reseed`]. Left at zero, every `<Rnd>` in the application would
    /// produce the same value on every run, forever, which is not what a random
    /// tag is for: two batches renamed a week apart would collide.
    ///
    /// It stays serialisable so a job file can pin one and get a reproducible
    /// run, which is the other half of P16.
    pub seed: u64,
    /// Where Script cards look for their `.koto` files.
    ///
    /// `None` is the user's script folder. Not serialised — a preset carrying
    /// one machine's script folder would be a preset that only works there —
    /// so this is the caller's own setting, supplied per run.
    #[serde(skip)]
    pub script_dir: Option<std::path::PathBuf>,
}

impl RunSettings {
    /// Choose a fresh seed.
    ///
    /// Per *session*, not per preview: a seed that changed between the preview
    /// and the rename that follows it would break the property P16 exists
    /// for.
    pub fn reseed(&mut self) {
        self.seed = fresh_seed();
    }
}

/// A seed from the clock, never zero — zero is the "unset" sentinel.
fn fresh_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    // Spread the low bits, which are the ones a modulo will look at.
    let mixed = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (nanos >> 31);
    mixed.max(1)
}

/// Everything resolved before the parallel pass.
#[derive(Debug, Clone)]
pub struct RunContext {
    /// One counter value per entry, in listing order.
    counters: Vec<i64>,
    /// Digits to pad counter values to; 0 means no padding.
    counter_width: usize,
    pub answers: Answers,
    pub num_files: usize,
    /// *"Time at start of rename"* — one timestamp for the whole run, so every
    /// file in a batch agrees.
    pub now: SystemTime,
    /// The Parts pattern, compiled once for the run rather than per file.
    pub parts: CompiledParts,
    pub require_all_tags: bool,
    pub seed: u64,
    /// The folder the run's files are in, with a trailing separator — a
    /// script's `fr.browser_path` (D108).
    ///
    /// Derived rather than plumbed: it is the folder every listed item shares.
    /// A free-select set or a recursive listing spans more than one folder, and
    /// both answer empty — which is the answer that makes a script fall back to
    /// "beside the first file". So does a folder whose name is not valid
    /// Unicode (D159): it has no text form a script could build a path from.
    ///
    /// Run-level, so it lives here rather than on the Script operation: a card
    /// cannot know where the listing came from, and a preset that carried one
    /// machine's folder would be a preset that only works there.
    pub browser_path: String,
    /// Where Script cards look for their `.koto` files; `None` is the user's
    /// folder. See [`RunSettings::script_dir`].
    pub script_dir: Option<std::path::PathBuf>,
}

impl Default for RunContext {
    fn default() -> Self {
        Self {
            counters: Vec::new(),
            counter_width: 0,
            answers: Answers::default(),
            num_files: 0,
            now: SystemTime::now(),
            parts: CompiledParts::default(),
            require_all_tags: false,
            seed: 0,
            browser_path: String::new(),
            script_dir: None,
        }
    }
}

/// The folder every entry sits in, or empty when they do not agree — or when
/// its name is not text.
///
/// A lossy rendering would hand a script `Caf\u{FFFD}/`, the name of a folder
/// that does not exist, and a playlist built on it would be refused by the
/// planner (P60 as amended) after the script had run. Empty is the contract's
/// existing answer for "no single folder a script can name".
fn common_parent(entries: &[FileEntry]) -> String {
    let Some(first) = entries.first().and_then(|e| e.path.parent()) else {
        return String::new();
    };
    // On the bytes, with the component comparison as the fallback: every
    // parent here is cut from one listing, so equal folders are equal bytes,
    // and a component walk per entry made this the dearest line of the
    // pre-pass.
    if entries.iter().any(|e| {
        e.path
            .parent()
            .is_none_or(|p| p.as_os_str() != first.as_os_str() && p != first)
    }) {
        return String::new();
    }
    let Some(text) = first.to_str() else {
        return String::new();
    };
    let mut text = text.to_owned();
    // The trailing separator is part of the contract: the playlist script
    // concatenates this straight onto a filename.
    if !text.is_empty() && !text.ends_with(std::path::MAIN_SEPARATOR) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    text
}

impl RunContext {
    /// The serial pre-pass (D28). O(n) and allocation-light.
    pub fn build(entries: &[FileEntry], settings: &RunSettings, answers: Answers) -> Self {
        let counters = settings.counter.sequence(entries);
        let counter_width = settings.counter.width_for(&counters);
        Self {
            counters,
            counter_width,
            answers,
            num_files: entries.len(),
            now: SystemTime::now(),
            parts: settings.parts.compile(),
            require_all_tags: settings.require_all_tags,
            seed: settings.seed,
            browser_path: common_parent(entries),
            script_dir: settings.script_dir.clone(),
        }
    }

    /// The counter value for the file at `index`.
    pub fn counter(&self, index: usize) -> i64 {
        self.counters.get(index).copied().unwrap_or(0)
    }

    /// The counter for `index`, zero-padded as configured.
    pub fn counter_text(&self, index: usize) -> String {
        crate::counter::pad(self.counter(index), self.counter_width)
    }

    /// The value the running counter should start at next time: the one this
    /// run's sequence would have produced after its last file, Reset at and
    /// all — [`CounterSetup::advance`] from the last value used, or the start
    /// if the run numbered nothing.
    pub fn next_start(&self, settings: &CounterSetup) -> i64 {
        self.counters
            .last()
            .map(|last| settings.advance(*last))
            .unwrap_or(settings.start)
    }
}

/// Reads answers from the terminal — the CLI's half of `<Ask>`.
///
/// A closed stdin answers nothing, which leaves the tag unavailable rather than
/// hanging a scripted run.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdinInteraction;

impl Interaction for StdinInteraction {
    fn ask(&self, spec: &AskSpec) -> Option<String> {
        use std::io::Write;
        print!("{}: ", spec.prompt());
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_owned()),
        }
    }

    /// The CLI has no clipboard of its own; a job that wants one can set it in
    /// the job file.
    fn clipboard(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_context_answers_sensibly() {
        let cx = RunContext::default();
        assert_eq!(cx.counter(0), 0);
        assert_eq!(cx.counter(99), 0);
        assert_eq!(cx.num_files, 0);
    }

    #[test]
    fn ask_slots_are_addressable() {
        let mut answers = Answers::default();
        answers.asks.insert(0, "plain".into());
        answers.asks.insert(3, "third".into());
        assert_eq!(answers.ask(0), Some("plain"));
        assert_eq!(answers.ask(3), Some("third"));
        assert_eq!(answers.ask(1), None);
    }

    #[test]
    fn an_ask_spec_describes_itself() {
        assert!(AskSpec { slot: 0 }.prompt().contains("<Ask>"));
        assert!(AskSpec { slot: 4 }.prompt().contains("<Ask-4>"));
    }

    /// One place decides what a run asks for, so the GUI modal and the CLI
    /// prompt can never drift apart.
    #[test]
    fn collect_gathers_every_slot_the_pipeline_named() {
        struct Fixed;
        impl Interaction for Fixed {
            fn ask(&self, spec: &AskSpec) -> Option<String> {
                Some(format!("answer {}", spec.slot))
            }
            fn clipboard(&self) -> Option<String> {
                Some("pasted".to_owned())
            }
        }

        let asks = [AskSpec { slot: 0 }, AskSpec { slot: 4 }];
        let answers = collect(&asks, true, &Fixed);
        assert_eq!(answers.ask(0), Some("answer 0"));
        assert_eq!(answers.ask(4), Some("answer 4"));
        assert_eq!(answers.ask(1), None);
        assert_eq!(answers.clipboard.as_deref(), Some("pasted"));

        let answers = collect(&asks, false, &Fixed);
        assert_eq!(answers.clipboard, None);
    }

    #[test]
    fn no_interaction_answers_nothing() {
        let interaction = NoInteraction;
        assert_eq!(interaction.ask(&AskSpec { slot: 0 }), None);
        assert_eq!(interaction.clipboard(), None);
    }

    /// A folder whose name is not valid Unicode has no text form, so it has no
    /// browser path either. Handing a script `Caf\u{FFFD}/` would hand it the
    /// name of a folder that does not exist; empty is the answer the contract
    /// already has for "no single folder a script can name" (D108).
    #[cfg(unix)]
    #[test]
    fn a_folder_whose_name_is_not_unicode_has_no_browser_path() {
        use std::os::unix::ffi::OsStrExt as _;
        let folder = std::path::Path::new("/x").join(std::ffi::OsStr::from_bytes(b"Caf\xE9"));
        let entries = [FileEntry::synthetic(folder.join("a.mp3"))];
        let run = RunContext::build(&entries, &RunSettings::default(), Answers::default());
        assert_eq!(run.browser_path, "");

        let entries = [FileEntry::synthetic("/x/Cafe/a.mp3")];
        let run = RunContext::build(&entries, &RunSettings::default(), Answers::default());
        assert_eq!(
            run.browser_path,
            format!("/x/Cafe{}", std::path::MAIN_SEPARATOR)
        );
    }

    /// A **running** counter stores where the next run should begin, and it has
    /// to obey Reset at — otherwise the value saved is one the counter's own
    /// range would never have produced, and the next run starts outside it.
    ///
    /// This is what `next_start` used to get wrong: a bare `last + step`, while
    /// the sequence itself applied the reset. Both now go through
    /// `CounterSetup::advance`, so they cannot drift apart again.
    #[test]
    fn the_next_start_obeys_reset_at() {
        let entries: Vec<FileEntry> = ["a", "b", "c"]
            .iter()
            .map(|n| FileEntry::synthetic(std::path::PathBuf::from("/x").join(n)))
            .collect();

        // 1, 2, 3 with no limit: the next run picks up at 4.
        let plain = CounterSetup {
            start: 1,
            step: 1,
            ..Default::default()
        };
        let run = RunContext::build(
            &entries,
            &RunSettings {
                counter: plain.clone(),
                ..Default::default()
            },
            Answers::default(),
        );
        assert_eq!(run.next_start(&plain), 4);

        // The same three files with "reset at 3": the last value used *is* the
        // limit, so the next run starts over at 1 rather than at 4.
        let limited = CounterSetup {
            reset_at: Some(3),
            ..plain.clone()
        };
        let run = RunContext::build(
            &entries,
            &RunSettings {
                counter: limited.clone(),
                ..Default::default()
            },
            Answers::default(),
        );
        assert_eq!(run.counter(2), 3, "the third file is the limit");
        assert_eq!(run.next_start(&limited), 1);

        // And a run that stops short of the limit still advances normally.
        let short = CounterSetup {
            reset_at: Some(9),
            ..plain
        };
        let run = RunContext::build(
            &entries,
            &RunSettings {
                counter: short.clone(),
                ..Default::default()
            },
            Answers::default(),
        );
        assert_eq!(run.next_start(&short), 4);
    }
}
