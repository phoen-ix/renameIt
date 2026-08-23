//! `fr` — the object a script talks to, and the session it lives inside.
//!
//! `fr` is a read-only object with eleven members, in Koto's idiom:
//! `fr.filename`, `fr.args`, `fr.seed` and the rest.
//!
//! # A native function cannot borrow the row
//!
//! The constraint that shapes this file. Koto's `KotoFunction` is
//! `Fn(&mut CallContext) -> Result<KValue> + Send + Sync + 'static`, so
//! `format_tags` — which has to render against *the file currently being
//! processed* — cannot close over an [`EvalCx`], which borrows both the entry
//! and the run context.
//!
//! So the two callable members close over an [`Arc`] of shared state that the
//! session updates before each call, and rebuild an `EvalCx` from owned data
//! when they run. That is also why the listing is cloned once into the session
//! rather than borrowed: one clone of N entries per run, which is the same
//! total work as cloning one entry per row and is done where it can be seen.
//!
//! # Session
//!
//! Global (script-level) variables are static per rename session, and a session
//! starts on each run or each fresh preview.
//!
//! One [`Session`] is one of those. Running the compiled chunk *is* `init` —
//! top-level code and the `init` call are the same event — then `rename` is
//! called per row in list order, then `done`. Because a script step forces the
//! evaluation pass to run serially (D94), "in list order" is a guarantee a
//! script may rely on rather than an accident of scheduling.

use super::engine::{Compiled, ScriptError, hardened};
use crate::model::FileEntry;
use crate::ops::{EvalCx, FileWrite, RunOutcome};
use crate::run::RunContext;
use crate::template::TextTemplate;
use koto::Koto;
use koto::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// What the two callable members need, and what changes per row.
struct Shared {
    /// The whole listing, so `all_filenames` can answer and so `format_tags`
    /// has an entry to render against. Cloned once per session.
    entries: Arc<[FileEntry]>,
    run: Arc<RunContext>,
    /// Compiled templates, kept because `format_tags` is called once per row
    /// and `TextTemplate` caches its compile behind `&self`.
    templates: Mutex<HashMap<String, TextTemplate>>,
    row: RwLock<Row>,
}

/// The cursor into the listing, moved before each `rename` call.
#[derive(Debug, Default, Clone)]
struct Row {
    index: usize,
    /// The name as the pipeline has it *so far*, which is what `fr.filename`
    /// reports and what `format_tags`'s `<Name>` must see.
    current: String,
}

impl Shared {
    /// The entry being processed, or `None` outside a `rename` call — which is
    /// exactly the `init`/`done` case, where there is no current file.
    fn entry(&self, index: usize) -> Option<&FileEntry> {
        self.entries.get(index)
    }
}

/// Zero-padded to the width of the largest index, so the values sort
/// lexicographically.
///
/// Only sortability is promised — the width and base are ours to pick, and a
/// script should rely on nothing else. It is **0-based**
/// and padded to `num_items`' digit count, which is what makes it sort (D97).
fn item_order(index: usize, total: usize) -> String {
    let width = total.max(1).to_string().len();
    format!("{index:0width$}")
}

/// The folder part of a path, with a trailing separator.
///
/// The separator is part of the contract: a script that concatenates the folder
/// and the filename must get a usable full path out of it.
fn parent_with_separator(entry: &FileEntry) -> String {
    match entry.path.parent() {
        Some(parent) => {
            let mut text = parent.to_string_lossy().into_owned();
            if !text.ends_with(std::path::MAIN_SEPARATOR) {
                text.push(std::path::MAIN_SEPARATOR);
            }
            text
        }
        None => String::new(),
    }
}

/// A running script.
pub struct Session {
    koto: Koto,
    fr: KMap,
    shared: Arc<Shared>,
    deadline: Duration,
    total: usize,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("total", &self.total)
            .finish()
    }
}

impl Session {
    /// Start a session: build `fr`, run the script's top level, and check that
    /// it defines the one function that is not optional.
    ///
    /// *"The path that is currently loaded in the file browser"* comes from the
    /// run context rather than from a parameter: it is a property of the
    /// listing, and nothing at the call site knows it that the run does not.
    pub fn start(
        compiled: &Compiled,
        args: &str,
        entries: &[FileEntry],
        run: &RunContext,
        deadline: Duration,
    ) -> Result<Self, ScriptError> {
        let shared = Arc::new(Shared {
            entries: entries.into(),
            run: Arc::new(run.clone()),
            templates: Mutex::new(HashMap::new()),
            row: RwLock::new(Row::default()),
        });

        let mut koto = hardened(deadline);
        let fr = build_facade(&shared, args, entries.len());
        koto.prelude().insert("fr", fr.clone());

        // Running the top level *is* init: `init` and top-level code are the
        // same event in Koto, where in VBScript they were two.
        koto.run(compiled.chunk().clone())
            .map_err(|e| ScriptError::from_run(&e, deadline))?;

        // A missing `rename` is an error rather than a silent no-op. The
        // original leaves it undefined-and-skip, but there the function name is
        // a VBScript declaration the engine looks up — here a typo would make
        // every row silently unchanged with nothing to see.
        if koto.exports().get("rename").is_none() {
            return Err(ScriptError::MissingFunction("rename".into()));
        }

        Ok(Self {
            koto,
            fr,
            shared,
            deadline,
            total: entries.len(),
        })
    }

    /// Call `rename` for one row.
    ///
    /// `Ok(None)` means *leave this file alone* — the script returned an empty
    /// string.
    pub fn rename(&mut self, index: usize, current: &str) -> Result<Option<String>, ScriptError> {
        self.point_at(index, current);
        let value = self
            .koto
            .call_exported_function("rename", CallArgs::Separate(&[]))
            .map_err(|e| ScriptError::from_run(&e, self.deadline))?;
        self.coerce_name(value)
    }

    /// Call `done`, if the script has one, and interpret what it returned.
    ///
    /// Two shapes, and only two:
    ///
    /// * a **map** with `path` and `contents` — *"write this file"*. An
    ///   optional `log` key is the line for the log.
    /// * anything else — stringified into the log.
    ///
    /// Nothing is written here. The request travels out through
    /// [`crate::ops::RunOutcome`] to `plan`, which turns it into a
    /// `PlannedOp::WriteFile` for the executor. That is what makes a preview
    /// structurally incapable of writing, rather than leaving each script to
    /// remember to check a preview flag for itself.
    pub fn finish(&mut self) -> RunOutcome {
        if self.koto.exports().get("done").is_none() {
            return RunOutcome::default();
        }
        // A script that fails in `done` is a warning and the batch carries
        // on: the renames themselves already succeeded.
        let value = match self
            .koto
            .call_exported_function("done", CallArgs::Separate(&[]))
        {
            Ok(value) => value,
            Err(error) => {
                return RunOutcome {
                    notes: vec![format!(
                        "script: done() failed: {}",
                        ScriptError::from_run(&error, self.deadline)
                    )],
                    ..Default::default()
                };
            }
        };
        self.interpret_done(value)
    }

    fn interpret_done(&mut self, value: KValue) -> RunOutcome {
        let KValue::Map(map) = &value else {
            return self.note_of(value);
        };
        let (Some(path), Some(contents)) = (map.get("path"), map.get("contents")) else {
            return self.note_of(value);
        };
        let (KValue::Str(path), KValue::Str(contents)) = (path, contents) else {
            return RunOutcome {
                notes: vec![
                    "script: done() returned a write with a non-string path or contents".into(),
                ],
                ..Default::default()
            };
        };
        let mut outcome = RunOutcome {
            writes: vec![FileWrite {
                path: PathBuf::from(path.as_str()),
                contents: contents.to_string(),
            }],
            notes: Vec::new(),
        };
        if let Some(KValue::Str(log)) = map.get("log") {
            outcome.notes.push(log.to_string());
        }
        outcome
    }

    /// Whatever `done` returned, as a line for the log.
    fn note_of(&mut self, value: KValue) -> RunOutcome {
        if matches!(value, KValue::Null) {
            return RunOutcome::default();
        }
        match self.koto.value_to_string(value) {
            Ok(text) if !text.is_empty() => RunOutcome {
                notes: vec![text],
                ..Default::default()
            },
            _ => RunOutcome::default(),
        }
    }

    /// Move the cursor, and refresh the members that change per row.
    fn point_at(&mut self, index: usize, current: &str) {
        if let Ok(mut row) = self.shared.row.write() {
            row.index = index;
            row.current = current.to_owned();
        }
        let entry = self.shared.entry(index);
        self.fr.insert("filename", current);
        self.fr
            .insert("item_order", item_order(index, self.total).as_str());
        if let Some(entry) = entry {
            self.fr.insert("full_filename", entry.file_name.as_str());
            self.fr.insert("disk_name", entry.file_name.as_str());
            self.fr
                .insert("path", parent_with_separator(entry).as_str());
        }
    }

    /// What a `rename` return value means.
    ///
    /// A **number** is accepted and stringified. Refusing it would break the
    /// shipped length-of-filename script, so it coerces (D98).
    fn coerce_name(&mut self, value: KValue) -> Result<Option<String>, ScriptError> {
        Ok(match value {
            KValue::Null => None,
            KValue::Str(s) if s.is_empty() => None,
            KValue::Str(s) => Some(s.to_string()),
            KValue::Number(n) => Some(n.to_string()),
            other => {
                let text = self
                    .koto
                    .value_to_string(other)
                    .map_err(|e| ScriptError::from_run(&e, self.deadline))?;
                (!text.is_empty()).then_some(text)
            }
        })
    }
}

/// Build the `fr` object.
///
/// The nine members that do not change per row are inserted once;
/// [`Session::point_at`] refreshes the four that do.
fn build_facade(shared: &Arc<Shared>, args: &str, total: usize) -> KMap {
    let fr = KMap::default();

    fr.insert("args", args);
    fr.insert("browser_path", shared.run.browser_path.as_str());
    fr.insert("num_items", total as i64);
    // Permanently true, and this is a guarantee rather than a stub. A script's
    // `rename` only ever runs while *planning*: `crate::exec` replays a plan and
    // never re-evaluates one. So a script can never write during what the user
    // is looking at, rather than leaving each script to check this flag
    // itself.
    fr.insert("preview", true);
    // Placeholders, so the shape of `fr` never changes between init and the
    // first row — a script reading `fr.filename` from its top level gets an
    // empty string rather than a missing-key error.
    fr.insert("filename", "");
    fr.insert("full_filename", "");
    fr.insert("disk_name", "");
    fr.insert("path", "");
    fr.insert("item_order", item_order(0, total).as_str());

    let for_tags = shared.clone();
    fr.add_fn("format_tags", move |ctx| match ctx.args() {
        [KValue::Str(template)] => format_tags(&for_tags, template),
        unexpected => unexpected_args("|String|", unexpected),
    });

    // The run's seed (P16), so a script that needs randomness can be random
    // *and* reproducible. Seeding from the clock on every preview would mean
    // the numbers in the preview column are not the numbers written to disk.
    // This is what lets the shipped random-number script be correct rather than
    // merely different.
    fr.insert("seed", shared.run.seed as i64);

    let for_contents = shared.clone();
    fr.add_fn("contents", move |_| Ok(contents_of(&for_contents)));

    // The file named in the Arguments box, read once at session start.
    //
    // Narrow on purpose: the path is the one the **user** typed into this
    // card, captured here, and a script cannot change it — assigning to
    // `fr.args` rewrites a map entry that this closure never consults. So it
    // is the same trust as `CsvList`'s own file field, and nothing like
    // reopening `io.read_to_string`.
    let args_path = args.to_owned();
    fr.add_fn("args_file", move |_| {
        Ok(KValue::Str(
            read_capped(std::path::Path::new(&args_path))
                .as_str()
                .into(),
        ))
    });

    // A case-insensitive index-of, in Rust, returning a **byte offset into the
    // haystack that was passed in**.
    //
    // Not a convenience. Koto's string module has no index-of at all, so the
    // idiom is to fold the haystack and search the copy — and then every offset
    // it yields indexes the *copy*, not the original. Slicing the original with
    // them is wrong wherever the two differ in length, which `str::to_lowercase`
    // genuinely does: 'İ' (U+0130) is two bytes and lower-cases to three. On a
    // 5 MB document the only correct alternative in script — folding a window
    // per character position — is far outside the per-row deadline.
    //
    // Returns `null` when there is no match, and clamps a `from` that is past
    // the end or off a character boundary rather than failing.
    fr.add_fn("find_ci", move |ctx| match ctx.args() {
        [KValue::Str(haystack), KValue::Str(needle)] => Ok(find_ci(haystack, needle, 0)),
        [
            KValue::Str(haystack),
            KValue::Str(needle),
            KValue::Number(from),
        ] => Ok(find_ci(haystack, needle, usize::from(*from))),
        unexpected => unexpected_args("|String, String|, or |String, String, Number|", unexpected),
    });

    let for_names = shared.clone();
    fr.add_fn("all_filenames", move |_| {
        Ok(KValue::List(KList::with_data(
            for_names
                .entries
                .iter()
                .map(|e| KValue::Str(e.file_name.as_str().into()))
                .collect(),
        )))
    });

    fr
}

/// The largest file `fr.contents()` will read.
///
/// 5 MB. A script that walks a document's text is reading it into memory a
/// character at a time, and a cap keeps a stray large file from stalling a
/// preview.
const MAX_CONTENTS: u64 = 5_000_000;

/// The **current file's** text — the one member that reads a file.
///
/// Removing `io` from the prelude closed the general route to the filesystem,
/// and this is the narrow replacement rather than a reopening: it takes no
/// path and can only ever read the file the
/// pipeline is already processing. A script cannot name a different one.
///
/// Uncached and read per row, which is the bargain `<Crc32>` already makes in
/// the same pass — it hashes the whole file every keystroke — bounded here by
/// the size cap above.
///
/// Empty for a file that is too large, unreadable, or has no current row. The
/// original conflates those too: each one is an `exit function`, which returns
/// the empty string.
fn contents_of(shared: &Arc<Shared>) -> KValue {
    let empty = KValue::Str("".into());
    let Ok(row) = shared.row.read() else {
        return empty;
    };
    let Some(entry) = shared.entry(row.index) else {
        return empty;
    };
    if entry.size > MAX_CONTENTS {
        return empty;
    }
    KValue::Str(read_capped(&entry.path).as_str().into())
}

/// Read a file as text, or return empty.
///
/// Lossy rather than refusing. A file that is not UTF-8 should still yield its
/// ASCII content — plenty of text on disk is CP1252 or similar, so this is not
/// a hypothetical.
fn read_capped(path: &std::path::Path) -> String {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() <= MAX_CONTENTS => std::fs::read(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Case-insensitive search returning a byte offset into `haystack`.
///
/// Compares folded candidates rather than folding the haystack, so the offset
/// it returns is always valid for the string the caller handed in. Only
/// positions whose first character folds to the needle's first character are
/// examined, which keeps it linear in practice on the documents this exists for.
fn find_ci(haystack: &str, needle: &str, from: usize) -> KValue {
    if needle.is_empty() {
        return KValue::Number((from.min(haystack.len()) as i64).into());
    }
    // A `from` that is past the end, or lands mid-character, must not panic.
    let from = (from..=haystack.len())
        .find(|at| haystack.is_char_boundary(*at))
        .unwrap_or(haystack.len());

    let lowered: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    for (offset, _) in haystack[from..].char_indices() {
        let at = from + offset;
        let candidate = haystack[at..].chars().flat_map(char::to_lowercase);
        if candidate.take(lowered.len()).eq(lowered.iter().copied()) {
            return KValue::Number((at as i64).into());
        }
    }
    KValue::Null
}

/// Render a format string against the row the session is pointing at.
///
/// This is the member that makes scripting worth having: it is the *real*
/// template engine, so `<Artist>`, `<Exif-DateTimeOriginal>` and `<Crc32>` all
/// work inside a script exactly as they do in a Free Format field.
fn format_tags(shared: &Arc<Shared>, template: &str) -> koto::runtime::Result<KValue> {
    let row = shared
        .row
        .read()
        .map_err(|_| koto::runtime::Error::from("script state was poisoned".to_string()))?
        .clone();

    let Some(entry) = shared.entry(row.index) else {
        // init and done have no current file, so there is nothing to render
        // tags against.
        return Ok(KValue::Str("".into()));
    };

    let cx = EvalCx::new(entry, row.index, shared.entries.len(), &shared.run);
    let cx = cx.with_current(&row.current);

    let mut templates = shared
        .templates
        .lock()
        .map_err(|_| koto::runtime::Error::from("script state was poisoned".to_string()))?;
    let compiled = templates
        .entry(template.to_owned())
        .or_insert_with(|| TextTemplate::new(template));

    match compiled.render(&cx) {
        // A bad tag name is the script's mistake and is reported as one, the
        // same way D29 reports it for a Free Format field, rather than
        // rendering to nothing and leaving the author guessing.
        Err(error) => Err(koto::runtime::Error::from(error.to_string())),
        Ok(rendered) => Ok(KValue::Str(rendered.text.as_str().into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Answers, RunSettings};
    use crate::script::compile;
    use std::path::PathBuf;

    fn entries(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|n| FileEntry::synthetic(PathBuf::from("/music").join(n)))
            .collect()
    }

    /// Run a script over a listing and collect what each row became.
    fn run(source: &str, args: &str, names: &[&str]) -> Vec<Option<String>> {
        let entries = entries(names);
        let run = RunContext::build(&entries, &RunSettings::default(), Answers::default());
        let compiled = compile(source).expect("compiles");
        let mut session = Session::start(&compiled, args, &entries, &run, Duration::from_secs(5))
            .expect("starts");
        let out = entries
            .iter()
            .enumerate()
            .map(|(i, e)| session.rename(i, &e.file_name).expect("renames"))
            .collect();
        session.finish();
        out
    }

    #[test]
    fn a_script_sees_the_name_it_is_asked_about() {
        assert_eq!(
            run(
                "rename = || fr.filename.to_uppercase()",
                "",
                &["a.mp3", "b.mp3"]
            ),
            [Some("A.MP3".into()), Some("B.MP3".into())]
        );
    }

    #[test]
    fn the_arguments_box_reaches_the_script() {
        assert_eq!(
            run("rename = || fr.args + fr.filename", "x-", &["a"]),
            [Some("x-a".into())]
        );
    }

    /// *"Return an empty string to skip renaming the file."*
    #[test]
    fn an_empty_return_skips_the_file() {
        assert_eq!(run("rename = || ''", "", &["a", "b"]), [None, None]);
    }

    /// A script may return a number rather than a string. Ours stringifies it
    /// rather than refusing — refusing would break the shipped
    /// length-of-filename example.
    #[test]
    fn a_number_is_accepted_as_a_name() {
        assert_eq!(
            run("rename = || size fr.full_filename", "", &["abc.mp3"]),
            [Some("7".into())]
        );
    }

    #[test]
    fn item_order_is_zero_padded_to_the_width_of_the_listing() {
        assert_eq!(item_order(0, 9), "0");
        assert_eq!(item_order(0, 10), "00");
        assert_eq!(item_order(7, 100), "007");
        assert_eq!(item_order(0, 0), "0", "an empty listing still has a width");
    }

    /// The property `Create Mp3 Playlist.frs` actually depends on: it sorts
    /// `ItemOrder & "|" & path` as strings, so the padding has to make the
    /// order lexicographic.
    #[test]
    fn item_order_sorts_lexicographically() {
        let mut keys: Vec<String> = (0..12).map(|i| item_order(i, 12)).collect();
        let expected = keys.clone();
        keys.sort();
        assert_eq!(keys, expected);
    }

    #[test]
    fn num_items_and_item_order_are_visible() {
        assert_eq!(
            run(
                "rename = || '{fr.item_order} of {fr.num_items}'",
                "",
                &["a", "b", "c"]
            ),
            [
                Some("0 of 3".into()),
                Some("1 of 3".into()),
                Some("2 of 3".into())
            ]
        );
    }

    /// `Path` carries its trailing separator — undocumented, but load-bearing:
    /// the playlist script concatenates it straight onto `FullFilename`.
    ///
    /// The expectation is *joined*, not a literal: `parent_with_separator`
    /// appends `MAIN_SEPARATOR`, so on Windows the answer is `/music\a.mp3` and
    /// a hard-coded forward slash tested the separator rather than the
    /// concatenation.
    #[test]
    fn path_ends_with_a_separator_so_it_concatenates() {
        let out = run("rename = || fr.path + fr.full_filename", "", &["a.mp3"]);
        let joined = out[0].as_deref().unwrap();
        assert_eq!(
            joined,
            PathBuf::from("/music").join("a.mp3").to_string_lossy()
        );
    }

    #[test]
    fn all_filenames_returns_the_whole_listing() {
        assert_eq!(
            run(
                "rename = || fr.all_filenames().intersperse('*').to_string()",
                "",
                &["a", "b", "c"]
            ),
            vec![Some("a*b*c".to_string()); 3]
        );
    }

    /// The member that makes scripting worth having: the real tag engine.
    #[test]
    fn format_tags_renders_through_the_real_template_engine() {
        assert_eq!(
            run(
                "rename = || fr.format_tags '<Name>-<Ext>'",
                "",
                &["song.mp3"]
            ),
            [Some("song-mp3".into())]
        );
    }

    /// A tag that does not exist is the script author's mistake, and is
    /// reported as one rather than rendering to nothing.
    #[test]
    fn a_misspelled_tag_is_an_error_not_a_blank() {
        let entries = entries(&["a.mp3"]);
        let run_cx = RunContext::build(&entries, &RunSettings::default(), Answers::default());
        let compiled = compile("rename = || fr.format_tags '<Nmae>'").unwrap();
        let mut session =
            Session::start(&compiled, "", &entries, &run_cx, Duration::from_secs(5)).unwrap();
        let err = session.rename(0, "a.mp3").unwrap_err();
        assert!(
            matches!(err, ScriptError::Runtime(ref m) if m.contains("Nmae")),
            "{err}"
        );
    }

    /// The whole point of a session, and **the one place Koto does not behave
    /// like VBScript**.
    ///
    /// Globals stay static for the whole rename session, and three of the nine
    /// shipped scripts lean on that: `Unique Random Number` walks a counter,
    /// `Create Mp3 Playlist` appends to an array, `Example - Base` counts.
    ///
    /// Koto captures by **value**. A function that closes over a scalar gets a
    /// copy, so `n += 1` writes to the copy and every row sees `1` — no error,
    /// no warning, just a counter that never counts. Maps and lists are
    /// reference types and do carry state.
    ///
    /// So the porting rule is: *session state lives in a map*. Both halves are
    /// asserted here so that a later "simplification" back to a bare scalar
    /// fails loudly instead of silently breaking three scripts.
    #[test]
    fn session_state_persists_in_a_map_but_not_in_a_scalar() {
        assert_eq!(
            run(
                "s = {n: 0}\nrename = ||\n  s.n += 1\n  '{s.n}'",
                "",
                &["a", "b", "c"]
            ),
            [Some("1".into()), Some("2".into()), Some("3".into())],
            "a map is how session state is carried"
        );
        assert_eq!(
            run(
                "n = 0\nrename = ||\n  n += 1\n  '{n}'",
                "",
                &["a", "b", "c"]
            ),
            [Some("1".into()), Some("1".into()), Some("1".into())],
            "a captured scalar is copied per call — this is the trap"
        );
    }

    #[test]
    fn a_script_without_a_rename_function_is_rejected() {
        let entries = entries(&["a"]);
        let run_cx = RunContext::build(&entries, &RunSettings::default(), Answers::default());
        let compiled = compile("renmae = || 'oops'").unwrap();
        let err =
            Session::start(&compiled, "", &entries, &run_cx, Duration::from_secs(5)).unwrap_err();
        assert_eq!(err, ScriptError::MissingFunction("rename".into()));
    }

    /// `done` is optional.
    #[test]
    fn done_is_optional_and_its_value_comes_back() {
        let entries = entries(&["a"]);
        let run_cx = RunContext::build(&entries, &RunSettings::default(), Answers::default());

        let compiled = compile("rename = || 'x'").unwrap();
        let mut session =
            Session::start(&compiled, "", &entries, &run_cx, Duration::from_secs(5)).unwrap();
        assert!(session.finish().is_empty());

        let compiled = compile("rename = || 'x'\ndone = || 'all finished'").unwrap();
        let mut session =
            Session::start(&compiled, "", &entries, &run_cx, Duration::from_secs(5)).unwrap();
        assert_eq!(session.finish().notes, ["all finished"]);
    }

    /// The deadline is per call, so one slow row does not poison the next.
    #[test]
    fn a_slow_row_is_an_error_and_the_next_row_still_runs() {
        let entries = entries(&["a", "b"]);
        let run_cx = RunContext::build(&entries, &RunSettings::default(), Answers::default());
        let compiled = compile(
            "rename = ||\n  if fr.filename == 'a'\n    loop\n      x = 1\n  fr.filename.to_uppercase()",
        )
        .unwrap();
        let deadline = Duration::from_millis(50);
        let mut session = Session::start(&compiled, "", &entries, &run_cx, deadline).unwrap();

        assert_eq!(
            session.rename(0, "a").unwrap_err(),
            ScriptError::TooSlow { deadline }
        );
        assert_eq!(session.rename(1, "b").unwrap(), Some("B".into()));
    }
}
