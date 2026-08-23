//! The hardened Koto engine, and the two gates that make it safe to run a file
//! somebody else wrote.
//!
//! # What is taken away, and why each one matters
//!
//! Koto's default prelude is not a sandbox. Every row below is reachable from a
//! bare `Koto::default()`:
//!
//! | Removed | What it would give a script |
//! |---|---|
//! | `io` | `create`, `open`, `remove_file`, `read_to_string`, `current_dir`, `temp_dir`, `exists`, `extend_path` |
//! | `os` | **`command`** — process execution — plus `time`, `start_timer`, `process_id`, `name` |
//! | `print` | stdout, which a GUI has no console for |
//! | `koto.load`, `koto.run` | compile and run a string **at runtime** |
//! | `koto.script_dir`, `koto.script_path` | host paths |
//!
//! `koto.run` is the one that has to go first, because it is what makes the
//! others recoverable: it compiles a string *while the script is running*, so
//! anything checked at compile time can be smuggled past by building it out of
//! pieces. Removing `io` while leaving `koto.run` in place would be theatre.
//!
//! `os` is removed for a second reason beyond `os.command`: `os.time` and
//! `os.start_timer` are clocks, and a script that can read a clock is a script
//! whose preview does not reproduce: reseeding from the clock on every preview
//! means the numbers in the preview column are not the numbers written to disk.
//! We seed from `RunSettings::seed` instead, and removing the clock is what keeps
//! that honest rather than merely conventional.
//!
//! # `import` cannot be closed by configuration
//!
//! `KotoVm::run_import` checks the prelude first and otherwise calls
//! `koto_bytecode::find_module`, which — for a chunk with no script path, which
//! is ours — searches **`std::env::current_dir()`** for `<name>.koto`.
//! `ModuleLoader` is a concrete struct with no trait seam, so unlike Rhai there
//! is no null resolver to install.
//!
//! So the gate is a bytecode scan instead: a compiled chunk containing `Import`
//! or `ImportAll` is rejected before it can ever run. Koto compiles nested
//! functions into the same chunk, so one scan covers a script's whole body —
//! including the `f = || import os` case that a source-text grep would miss.
//!
//! # Both gates are load-bearing, and neither subsumes the other
//!
//! The prelude removals stop a script reaching `io`; the import scan stops it
//! reaching `io` *by another name*. Neither is redundant, and both are pinned by
//! their own tests, because removal-based hardening fails silently: a koto
//! release that adds one function to `io` widens the sandbox and nothing breaks.
//! The tests are the boundary, not this comment.

use koto::Koto;
use koto::prelude::*;
use koto::runtime::Ptr;
use koto_bytecode::{Chunk, Instruction, InstructionReader};
use std::time::Duration;

/// How long one call into a script may take before it is abandoned.
///
/// **Wall clock, because Koto has nothing else.** `with_execution_limit` takes a
/// `Duration`; there is no instruction budget to spend, so this cannot be made
/// machine-independent the way `regex_flavor`'s backtrack limit is.
///
/// Per *row*, deliberately, and with no whole-run budget above it (D95). A
/// run-wide budget would make *which* rows evaluated depend on how fast the
/// machine was, which is a worse trade. A slow script over a large folder is
/// simply slow — M0's generation counter cancels stale recomputes.
///
/// A bound is needed at all because a script is user code: an accidental
/// infinite loop must not hang the preview.
///
/// # What it does **not** cover
///
/// Read this before trusting it. `KotoVm` samples the deadline in its
/// instruction dispatch loop — `koto_runtime/src/vm.rs:689`, inside
/// `while let Some(instruction) = self.reader.next()` — so it can only
/// interrupt a script **between** instructions. It cannot interrupt one native
/// function that runs for a long time, because that native call *is* one
/// instruction as far as the loop is concerned.
///
/// That is not theoretical. `iterator.repeat(0).count()` composes an infinite
/// pure-Rust generator with a pure-Rust consuming loop: it never returns to the
/// dispatch loop, so the deadline never fires and the call never ends. The
/// first version of this module shipped that hole with a test that certified
/// against it — the test used `loop\n  x = 1`, which is the one shape the limit
/// *does* catch. [`REMOVED_FROM_ITERATOR`] and its neighbours close the vectors
/// that are reachable in O(1) of source; the general limitation stands, and
/// D95 records it.
pub const SCRIPT_DEADLINE: Duration = Duration::from_millis(10);

/// Prelude names removed wholesale. See the module docs for what each one is.
const REMOVED_FROM_PRELUDE: &[&str] = &["io", "os", "print"];

/// Members removed from the `koto` module, which stays for `copy`, `deep_copy`,
/// `hash`, `size` and `type`.
const REMOVED_FROM_KOTO_MODULE: &[&str] = &["load", "run", "script_dir", "script_path"];

/// The three infinite generators, removed because the deadline cannot stop
/// them.
///
/// Each builds a pure-Rust iterator with no end, and koto's consuming functions
/// (`count`, `to_list`, `sum`, …) drain them in a pure-Rust loop. Composing the
/// two never re-enters the VM, so `iterator.repeat(0).count()` runs forever
/// against a 10 ms deadline — verified, not inferred.
///
/// Removing them from this map closes the method form too: `[1, 2].cycle()`
/// resolves through the VM's iterator fallback to the *same* `KMap`, which is
/// `Ptr`-backed and therefore shared with the prelude entry. Also verified,
/// because "the same map" was a guess worth checking.
const REMOVED_FROM_ITERATOR: &[&str] = &["repeat", "cycle", "generate"];

/// Allocators a script can reach with a constant amount of source.
///
/// `'x'.repeat 1000000000000000000` asks for an exabyte in one call. Rust's
/// allocation-failure handler **aborts the process** rather than unwinding, so
/// this is not a row error or even a panic — it takes the whole application
/// down, GUI included. There is no allocation limit to configure, so the
/// functions go instead.
///
/// This is a narrower loss than it looks: none of the nine ported scripts uses
/// any of them, and `list.push` in a loop reaches the same places under the
/// deadline's eye.
const REMOVED_FROM_STRING: &[&str] = &["repeat"];
const REMOVED_FROM_LIST: &[&str] = &["resize", "resize_with", "fill"];

/// koto's `arc` feature, asserted where a mistake would be cheapest to read.
///
/// The default is `rc`, which builds `Koto` out of `Rc` and `RefCell` and is
/// therefore neither `Send` nor `Sync` — and `NameTransform` requires both. A
/// slip back to default features would otherwise surface as a wall of trait
/// errors in `ops/script.rs`, three files away from the line that caused it.
const _: fn() = || {
    fn both<T: Send + Sync>() {}
    both::<Koto>();
};

/// How deeply a script may nest before it is refused unparsed.
///
/// Koto's parser is recursive descent with no depth limit of its own, and it
/// overflows the stack at around six thousand — which in Rust is an **abort**,
/// not an error: the process dies at *compile* time, before the import gate or
/// the deadline have run at all.
///
/// 256 is far below the cliff and far above anything a person writes; the
/// deepest of the nine ported scripts nests four. Measured rather than guessed:
/// `((((…1…))))` and `[[[[…]]]]` dump core at 8 000, `not not not …` at 12 000.
const MAX_NESTING: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScriptError {
    #[error("script will not compile: {0}")]
    Compile(String),
    /// Not a compile error in Koto's eyes — a deliberate refusal. Worth its own
    /// variant so the message can say *why* rather than reporting a syntax
    /// problem the script does not have.
    #[error(
        "a script may not `import`: imports read files from disk, and that is the one thing the sandbox cannot close"
    )]
    Import,
    #[error("script too slow: gave up after {}ms", .deadline.as_millis())]
    TooSlow { deadline: Duration },
    #[error("{0}")]
    Runtime(String),
    #[error("the script has no `{0}` function")]
    MissingFunction(String),
    #[error("the script nests more than {MAX_NESTING} deep, which the parser cannot survive")]
    TooDeep,
}

impl ScriptError {
    /// Classify an error that came back out of the engine.
    ///
    /// `koto::Error` flattens every runtime error to a string on its way out of
    /// the façade — `koto_runtime::ErrorKind::Timeout` does **not** survive as a
    /// variant — so the deadline has to be recognised from the message.
    /// [`tests::the_timeout_message_is_the_one_we_match_on`] pins that string,
    /// so a koto release that rewords it fails a test rather than quietly
    /// reclassifying every slow script as a generic error.
    pub fn from_run(error: &koto::Error, deadline: Duration) -> Self {
        let text = error.to_string();
        if text.contains("execution timed out") {
            Self::TooSlow { deadline }
        } else {
            Self::Runtime(text)
        }
    }
}

/// A sandboxed engine.
///
/// Not reusable across threads by sharing — `Koto` needs `&mut self` to run
/// anything — but `Send + Sync` under koto's `arc` feature, which is what lets
/// a `NameTransform` own one.
pub fn hardened(deadline: Duration) -> Koto {
    let koto = Koto::with_settings(KotoSettings::default().with_execution_limit(deadline));
    let prelude = koto.prelude();

    for name in REMOVED_FROM_PRELUDE {
        prelude.remove(*name);
    }
    // Each of these modules stays — they are ordinary language facilities — and
    // loses only the members the deadline cannot police.
    for (module, names) in [
        ("koto", REMOVED_FROM_KOTO_MODULE),
        ("iterator", REMOVED_FROM_ITERATOR),
        ("string", REMOVED_FROM_STRING),
        ("list", REMOVED_FROM_LIST),
    ] {
        if let Some(KValue::Map(map)) = prelude.get(module) {
            for name in names {
                map.remove(*name);
            }
        }
    }

    koto
}

/// A script that has compiled and passed the import gate.
///
/// Holding this is the proof: there is no way to build one that can import, so
/// callers do not have to remember to check.
#[derive(Clone)]
pub struct Compiled {
    chunk: Ptr<Chunk>,
    header: super::Header,
}

impl std::fmt::Debug for Compiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The chunk's own Display is a full bytecode dump — far too much for a
        // `{:?}` of an operation card.
        f.write_str("Compiled(<chunk>)")
    }
}

impl Compiled {
    pub fn chunk(&self) -> &Ptr<Chunk> {
        &self.chunk
    }

    /// The script's description and argument hint.
    ///
    /// Carried here rather than re-read, so the card can show a description
    /// without paying a second read per frame: the compile is already behind a
    /// process-wide, mtime-keyed cache.
    pub fn header(&self) -> &super::Header {
        &self.header
    }
}

/// Compile a script, and refuse it if it can import.
///
/// `export_top_level_ids` is what lets a script write a bare `rename = |…|`
/// instead of `export rename = |…|`, so a port reads like the VBScript
/// `Function Rename()` it replaces.
pub fn compile(source: &str) -> Result<Compiled, ScriptError> {
    // Before the parser sees it: a stack overflow there aborts the process, so
    // it cannot be caught afterwards.
    if nesting_depth(source) > MAX_NESTING {
        return Err(ScriptError::TooDeep);
    }
    let mut koto = hardened(SCRIPT_DEADLINE);
    let chunk = koto
        .compile(CompileArgs::new(source).export_top_level_ids(true))
        .map_err(|e| ScriptError::Compile(e.to_string()))?;

    if can_import(&chunk) {
        return Err(ScriptError::Import);
    }
    Ok(Compiled {
        chunk,
        header: super::Header::parse(source),
    })
}

/// The longest run of things the parser will recurse through.
///
/// Openers and prefix operators count; whitespace is neutral so `not not not`
/// reads as depth three; anything else resets the run. Deliberately crude — it
/// is a bound on the parser's recursion, not a parse, and it has to be cheap
/// enough to run on every script the picker touches.
fn nesting_depth(source: &str) -> usize {
    let mut depth = 0usize;
    let mut deepest = 0usize;
    let mut rest = source;
    while let Some(c) = rest.chars().next() {
        let step = c.len_utf8();
        match c {
            '(' | '[' | '{' | '-' | '!' | '~' => depth += 1,
            c if c.is_whitespace() => {}
            _ if rest.starts_with("not") => {
                depth += 1;
                rest = &rest[3..];
                deepest = deepest.max(depth);
                continue;
            }
            _ => depth = 0,
        }
        deepest = deepest.max(depth);
        rest = &rest[step..];
    }
    deepest
}

/// Whether a chunk contains an import instruction.
///
/// Deliberately conservative: it asks whether the bytecode *can* import, not
/// whether it will. A script that guards an import behind a condition that is
/// never true is still refused, which is the right way round for this gate.
fn can_import(chunk: &Ptr<Chunk>) -> bool {
    InstructionReader::new(chunk.clone()).any(|instruction| {
        matches!(
            instruction,
            Instruction::Import { .. } | Instruction::ImportAll { .. }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a snippet through the real sandbox and return whatever came back.
    fn run(source: &str) -> Result<String, ScriptError> {
        run_with(source, SCRIPT_DEADLINE)
    }

    fn run_with(source: &str, deadline: Duration) -> Result<String, ScriptError> {
        let compiled = compile(source)?;
        let mut koto = hardened(deadline);
        let value = koto
            .run(compiled.chunk().clone())
            .map_err(|e| ScriptError::from_run(&e, deadline))?;
        koto.value_to_string(value)
            .map_err(|e| ScriptError::from_run(&e, deadline))
    }

    #[test]
    fn an_ordinary_script_still_works() {
        assert_eq!(run("'a-b'.replace '-', ' '").unwrap(), "a b");
    }

    // --- The prelude removals ------------------------------------------------
    //
    // One test per removed name, on purpose. A single test asserting "the
    // sandbox holds" would keep passing when a koto upgrade restores one of
    // them, because the others would carry it. Removal-based hardening only
    // stays honest if each removal is named.

    #[test]
    fn a_script_cannot_read_a_file() {
        let err = run("io.read_to_string 'Cargo.toml'").unwrap_err();
        assert!(
            matches!(err, ScriptError::Runtime(ref m) if m.contains("'io' not found")),
            "{err}"
        );
    }

    #[test]
    fn a_script_cannot_write_or_delete_a_file() {
        // Same missing module, but these are the two calls that would be
        // unrecoverable, so they are worth naming separately from the read.
        for source in ["io.create 'x'", "io.remove_file 'x'", "io.temp_dir()"] {
            let err = run(source).unwrap_err();
            assert!(
                matches!(err, ScriptError::Runtime(ref m) if m.contains("'io' not found")),
                "{source}: {err}"
            );
        }
    }

    #[test]
    fn a_script_cannot_run_a_process() {
        let err = run("os.command 'id'").unwrap_err();
        assert!(
            matches!(err, ScriptError::Runtime(ref m) if m.contains("'os' not found")),
            "{err}"
        );
    }

    /// Not a security hole — a reproducibility one. A script that can read a
    /// clock is a script whose preview does not agree with itself.
    #[test]
    fn a_script_cannot_read_the_clock() {
        for source in ["os.time()", "os.start_timer()"] {
            let err = run(source).unwrap_err();
            assert!(
                matches!(err, ScriptError::Runtime(ref m) if m.contains("'os' not found")),
                "{source}: {err}"
            );
        }
    }

    #[test]
    fn a_script_cannot_print_to_stdout() {
        let err = run("print 'x'").unwrap_err();
        assert!(
            matches!(err, ScriptError::Runtime(ref m) if m.contains("'print' not found")),
            "{err}"
        );
    }

    /// The removal that protects the others: `koto.run` compiles a string
    /// *while the script is running*, so leaving it in place would let a script
    /// rebuild anything the import scan rejected at compile time.
    #[test]
    fn a_script_cannot_compile_more_script_at_run_time() {
        for source in [
            "koto.run \"io.read_to_string 'Cargo.toml'\"",
            "koto.load \"import io\"",
        ] {
            let err = run(source).unwrap_err();
            assert!(
                matches!(err, ScriptError::Runtime(ref m) if m.contains("not found")),
                "{source}: {err}"
            );
        }
    }

    #[test]
    fn a_script_cannot_ask_where_it_lives() {
        for source in ["koto.script_dir()", "koto.script_path()"] {
            let err = run(source).unwrap_err();
            assert!(
                matches!(err, ScriptError::Runtime(ref m) if m.contains("not found")),
                "{source}: {err}"
            );
        }
    }

    /// The rest of the `koto` module is ordinary language furniture and stays.
    #[test]
    fn the_harmless_half_of_the_koto_module_survives() {
        assert_eq!(run("koto.type 'x'").unwrap(), "String");
        assert_eq!(run("koto.size [1, 2, 3]").unwrap(), "3");
    }

    // --- The import gate -----------------------------------------------------

    #[test]
    fn a_script_cannot_import() {
        assert_eq!(compile("import io").unwrap_err(), ScriptError::Import);
        assert_eq!(
            compile("from io import open").unwrap_err(),
            ScriptError::Import
        );
    }

    /// The case a source-text grep misses: koto compiles nested functions into
    /// the same chunk, so scanning the chunk catches an import that never
    /// appears at the top level.
    #[test]
    fn a_script_cannot_hide_an_import_inside_a_function() {
        let source = "\
rename = ||
  sneaky = ||
    import os
    os.command 'id'
  sneaky()
";
        assert_eq!(compile(source).unwrap_err(), ScriptError::Import);
    }

    /// The other half of the gate: it must not reject scripts that do nothing
    /// wrong, or every port fails and somebody widens it.
    #[test]
    fn an_import_free_script_compiles() {
        assert!(compile("rename = || 'x'").is_ok());
        assert!(compile("x = 1 + 1").is_ok());
    }

    // --- The deadline --------------------------------------------------------

    #[test]
    fn a_runaway_script_does_not_hang_the_preview() {
        let deadline = Duration::from_millis(50);
        let started = std::time::Instant::now();
        let err = run_with("loop\n  x = 1", deadline).unwrap_err();
        assert_eq!(err, ScriptError::TooSlow { deadline });
        assert!(
            started.elapsed() < deadline * 20,
            "the limit did not stop it promptly: {:?}",
            started.elapsed()
        );
    }

    // --- What the deadline cannot police -----------------------------------
    //
    // These are the holes the first version of this module shipped. Each one
    // was a script that never returned, or one that took the whole process
    // down, under a 10 ms limit that the test above certifies as working.
    //
    // Every test here runs *without* a wall-clock assertion on purpose: if the
    // guard is removed the test does not fail slowly, it hangs the suite. That
    // is the honest signal — the failure mode being guarded against IS a hang.

    /// The one that matters most: an infinite generator composed with a
    /// consuming loop never re-enters the VM, so `with_execution_limit` never
    /// gets a chance to fire.
    #[test]
    fn a_script_cannot_build_an_endless_iterator() {
        for source in [
            "iterator.repeat(0).count()",
            "iterator.generate(|| 1).count()",
            "iterator.cycle([1, 2]).count()",
        ] {
            let err = run(source).unwrap_err();
            assert!(
                matches!(err, ScriptError::Runtime(ref m) if m.contains("not found")),
                "{source}: {err}"
            );
        }
    }

    /// And the method spelling of the same thing. `[1, 2].cycle()` resolves
    /// through the VM's iterator fallback rather than the prelude entry, so
    /// this is a genuinely separate route — it just happens to reach the same
    /// `KMap`, which is why removing it once is enough.
    #[test]
    fn the_method_form_of_an_endless_iterator_is_closed_too() {
        for source in ["[1, 2].cycle().count()", "'ab'.repeat(2)"] {
            let err = run(source).unwrap_err();
            assert!(
                matches!(err, ScriptError::Runtime(ref m) if m.contains("not found")),
                "{source}: {err}"
            );
        }
    }

    /// An allocation a script asks for in one call is not a row error and not
    /// even a panic: Rust aborts the process when the allocator fails, so it
    /// takes the GUI with it.
    #[test]
    fn a_script_cannot_ask_for_an_absurd_allocation() {
        for source in [
            "size 'x'.repeat 1000000000000000000",
            "x = []\nx.resize 100000000000",
            "x = []\nx.resize_with 100000000000, || 0",
            "x = []\nx.fill 0",
        ] {
            let err = run(source).unwrap_err();
            assert!(
                matches!(err, ScriptError::Runtime(ref m) if m.contains("not found")),
                "{source}: {err}"
            );
        }
    }

    /// A script the parser cannot survive is refused before the parser sees it.
    ///
    /// Koto's parser is recursive descent with no depth limit, and overflowing
    /// it **aborts the process** — so this cannot be a caught error, it has to
    /// be a refusal in front. The counts here are well under the measured
    /// cliff, so the test cannot itself crash the suite.
    #[test]
    fn a_script_that_would_overflow_the_parser_is_refused() {
        for source in [
            format!("x = {}1{}", "(".repeat(400), ")".repeat(400)),
            format!("x = {}1{}", "[".repeat(400), "]".repeat(400)),
            format!("x = {}1", "-".repeat(400)),
            format!("x = {}true", "not ".repeat(400)),
        ] {
            assert_eq!(compile(&source).unwrap_err(), ScriptError::TooDeep);
        }
    }

    /// And ordinary nesting is nowhere near it — the deepest of the nine
    /// shipped ports nests four.
    #[test]
    fn ordinary_nesting_is_not_refused() {
        assert!(compile("x = ((((1 + 2))))").is_ok());
        assert!(compile("x = [[1, 2], [3, 4]]").is_ok());
        assert!(compile("rename = || if not fr.filename.is_empty() then 'a' else 'b'").is_ok());
        assert_eq!(nesting_depth("x = -1"), 1);
        assert_eq!(nesting_depth("not not true"), 2);
        assert_eq!(nesting_depth("a = 1\nb = 2"), 0);
    }

    /// The other half: the removals are surgical, and everything the nine
    /// ported scripts actually use still works.
    #[test]
    fn the_iterators_the_ports_use_all_survive() {
        assert_eq!(
            run("['a', 'b'].intersperse('*').to_string()").unwrap(),
            "a*b"
        );
        assert_eq!(run("(1..5).keep(|n| n > 2).count()").unwrap(), "2");
        assert_eq!(run("[3, 1, 2].sort()\n'sorted'").unwrap(), "sorted");
        assert_eq!(run("size 'a-b'.split('-').to_tuple()").unwrap(), "2");
        assert_eq!(run("[1, 2].to_tuple()").unwrap(), "(1, 2)");
    }

    /// `koto::Error` flattens `ErrorKind::Timeout` into a string on its way out
    /// of the façade, so [`ScriptError::from_run`] has to recognise the message.
    /// This pins the message. If a koto upgrade rewords it, this fails here —
    /// loudly and in one place — rather than silently reclassifying every slow
    /// script as a generic runtime error.
    #[test]
    fn the_timeout_message_is_the_one_we_match_on() {
        let deadline = Duration::from_millis(20);
        let compiled = compile("loop\n  x = 1").unwrap();
        let mut koto = hardened(deadline);
        let error = koto.run(compiled.chunk().clone()).unwrap_err();
        assert!(
            error.to_string().contains("execution timed out"),
            "koto's timeout wording changed: {error}"
        );
    }

    /// A script that fails for its own reasons must not be reported as slow.
    #[test]
    fn an_ordinary_error_is_not_reported_as_a_timeout() {
        let err = run("throw 'nope'").unwrap_err();
        assert!(
            matches!(err, ScriptError::Runtime(ref m) if m.contains("nope")),
            "{err}"
        );
    }
}
