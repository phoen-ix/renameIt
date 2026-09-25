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
/// *does* catch. And a native loop that calls back into the script is worse
/// than one that does not: koto arms a **fresh** limit for every callback.
///
/// So the sandbox closes each route to a long native loop instead:
///
/// * the endless generators and the one-call allocators are removed
///   ([`REMOVED_FROM_ITERATOR`] and its neighbours);
/// * a script may not define an iterator of its own — a generator or an
///   `@next`/`@iterator` — because nothing bounds one ([`compile`] refuses);
/// * every iterator function, and the three elsewhere that walk an iterable,
///   refuse a range or iterator longer than [`MAX_NATIVE_ITEMS`];
/// * every library function refuses to start once the budget of the call in
///   progress is spent ([`within`]), and hands the script's callbacks on
///   behind the same check — so a native loop of callbacks, or of callbacks
///   that start loops of their own, stops soon after the budget does.
///
/// What is left is a native loop over at most a million items with no
/// callback in it: milliseconds.
///
/// Per row. A script's top level and its `done` get [`SCRIPT_SETUP_DEADLINE`].
pub const SCRIPT_DEADLINE: Duration = Duration::from_millis(10);

/// The budget of a script's top level — its `init` — and of `done`.
///
/// Each runs once per session rather than once per row, and each is where
/// the whole-listing work belongs: loading a table into a map at the top,
/// building a playlist in `done`. Held to a row's 10 ms, a few thousand lines
/// of either failed on a slow machine and passed on a fast one, and a `done`
/// that timed out wrote nothing. A second is generous for that and still
/// bounds an accidental infinite loop.
pub const SCRIPT_SETUP_DEADLINE: Duration = Duration::from_secs(1);

/// The longest range or iterator a library call accepts.
///
/// A million items is far past anything a rename script walks — the whole
/// listing is one item per file — and small enough that a native loop over
/// it, the one kind the deadline cannot interrupt, ends in milliseconds.
pub const MAX_NATIVE_ITEMS: usize = 1_000_000;

/// The widest a format spec may pad or round a value.
///
/// `'{x:4000000000}'` is one native `repeat` asking for four gigabytes, and
/// an allocation failure aborts the process. A filename has no use for more
/// than this.
const MAX_FORMAT_WIDTH: u32 = 1024;

/// Prelude names removed wholesale. See the module docs for what each one is.
const REMOVED_FROM_PRELUDE: &[&str] = &["io", "os", "print"];

/// Members removed from the `koto` module, which stays for `copy`, `hash`,
/// `size` and `type`.
///
/// `deep_copy` recurses natively with no cycle check, so a list that contains
/// itself overflows the stack — an abort, not an error.
const REMOVED_FROM_KOTO_MODULE: &[&str] =
    &["load", "run", "script_dir", "script_path", "deep_copy"];

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
/// Koto's parser is recursive descent with no depth limit of its own, and a
/// stack overflow in Rust is an **abort**, not an error: the process dies at
/// *compile* time, before the import gate or the deadline have run at all.
/// Measured on an 8 MB stack: `((((…))))` dies past 6 600 levels, a chain of
/// lambdas past 3 600, nested `if` blocks past 3 200 — 1.3 to 2.6 KB of stack
/// a level. The compiler has a limit of its own besides: a list literal nested
/// past about 120 exhausts its registers, and that one is a panic.
///
/// 100 is under both and far above anything a person writes; the deepest of
/// the nine ported scripts nests four. The count ([`nesting_depth`]) is a
/// friendly refusal for the shapes it can see; the stack [`compile`] runs on is
/// what bounds the ones it cannot.
const MAX_NESTING: usize = 100;

/// The longest script accepted, in bytes.
///
/// Sixteen times the longest shipped port, and the bound that makes the
/// compile stack finite: no shape of source costs the parser more than about
/// 0.8 KB of stack per byte, measured, so a script this long cannot outgrow
/// [`compile_stack`].
const MAX_SOURCE_BYTES: usize = 64 * 1024;

/// The stack a script of `len` bytes is compiled on: 2 KB per byte — more
/// than twice the worst measured — over an 8 MB floor. At the size limit that
/// is 136 MB of address space, reserved rather than used; an ordinary script
/// gets the floor.
fn compile_stack(len: usize) -> usize {
    8 * 1024 * 1024 + 2 * 1024 * len
}

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
    #[error("the script is longer than {} KB, which is more than the parser is given", MAX_SOURCE_BYTES / 1024)]
    TooLong,
    /// A construct the deadline cannot police, refused at compile time. The
    /// text says which, and why.
    #[error("a script may not {0}")]
    Refused(&'static str),
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

    // Every library function goes behind the guard ([`bounded`]): each
    // checks the budget of the call in progress and hands the script's
    // callbacks on behind the same check. The ones that loop over an iterable
    // argument — `iterator` wholesale, and three elsewhere — also refuse one
    // longer than `MAX_NATIVE_ITEMS`; the rest (`range.contains`, `koto.size`)
    // answer in O(1) whatever the length, and are left to. The method form
    // (`(0..9).count()`) reaches the same map entry, as the removals do.
    let modules: Vec<String> = prelude.data().keys().map(ToString::to_string).collect();
    for module in modules {
        let Some(KValue::Map(map)) = prelude.get(module.as_str()) else {
            continue;
        };
        let names: Vec<String> = map.data().keys().map(ToString::to_string).collect();
        for name in names {
            let lengths = module == "iterator"
                || LOOPS_OVER_AN_ARGUMENT.contains(&(module.as_str(), name.as_str()));
            if let Some(KValue::NativeFunction(function)) = map.get(name.as_str()) {
                map.insert(name.as_str(), bounded(function, lengths));
            }
        }
    }

    koto
}

thread_local! {
    /// When the script call in progress on this thread runs out of budget.
    static CALL_ENDS: std::cell::Cell<Option<std::time::Instant>> =
        const { std::cell::Cell::new(None) };
    /// Guarded calls since the clock was last read ([`in_time`]).
    static CALLS_SINCE_CHECK: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// How many guarded calls pass between two reads of the clock.
///
/// Reading it is not free everywhere — about 4 µs on a machine whose clock
/// source is not the TSC, which on every `map.insert` of a loop made a script
/// six times slower. koto samples its own limit the same way, every so many
/// instructions. The cost is overshoot: at most this many short calls, or
/// this many callbacks that each stay within their own limit.
const CHECK_EVERY: u32 = 32;

/// Runs `call` — one call into a script — with its budget known to the guard
/// [`bounded`] puts on the library.
///
/// koto's own limit covers the instructions of one call, and restarts inside
/// every callback a native function makes. This one covers the call as a
/// whole: once it is spent, every guarded library function refuses to start,
/// so a native loop whose callbacks each start another one stops.
pub(crate) fn within<T>(budget: Duration, call: impl FnOnce() -> T) -> T {
    struct Restore(Option<std::time::Instant>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CALL_ENDS.set(self.0);
        }
    }
    let _restore = Restore(CALL_ENDS.replace(std::time::Instant::now().checked_add(budget)));
    call()
}

/// The library functions outside `iterator` that walk an iterable argument in
/// one native loop, and so are held to [`MAX_NATIVE_ITEMS`].
const LOOPS_OVER_AN_ARGUMENT: &[(&str, &str)] = &[
    ("list", "extend"),
    ("map", "extend"),
    ("string", "from_bytes"),
];

/// Refuses to go on once the call in progress is out of budget ([`within`]).
///
/// Worded as koto's own timeout, so it is reported the same way
/// ([`ScriptError::from_run`]).
fn in_time() -> koto::runtime::Result<()> {
    let Some(end) = CALL_ENDS.get() else {
        return Ok(());
    };
    let calls = CALLS_SINCE_CHECK.get() + 1;
    if calls < CHECK_EVERY {
        CALLS_SINCE_CHECK.set(calls);
        return Ok(());
    }
    CALLS_SINCE_CHECK.set(0);
    if std::time::Instant::now() >= end {
        return Err(koto::runtime::Error::from(
            "execution timed out".to_string(),
        ));
    }
    Ok(())
}

/// A library function behind the guard: refused when the call in progress is
/// out of budget, and — where `lengths` says it loops over its argument —
/// when it is handed a range or iterator longer than [`MAX_NATIVE_ITEMS`].
///
/// And every script function it is handed — the `|n| …` of `keep`, `each`,
/// `fold`, `sort` — is handed on behind the same check ([`checked`]), because
/// the library calls it from inside its own loop and koto gives each of those
/// calls a fresh limit. Without it a loop of a million callbacks, each well
/// inside its own 10 ms, ran for hours.
fn bounded(function: KNativeFunction, lengths: bool) -> KValue {
    KValue::NativeFunction(KNativeFunction::new(move |ctx: &mut CallContext| {
        in_time()?;
        let values = std::iter::once(ctx.instance()).chain(ctx.args());
        for value in values.filter(|_| lengths) {
            let length = match value {
                KValue::Range(range) => range.size(),
                KValue::Iterator(iterator) => {
                    let (low, high) = iterator.size_hint();
                    Some(high.unwrap_or(low))
                }
                _ => None,
            };
            if length.is_some_and(|n| n > MAX_NATIVE_ITEMS) {
                return Err(koto::runtime::Error::from(format!(
                    "a range or iterator of more than {MAX_NATIVE_ITEMS} items is too long to \
                     hand to the library in one call; walk it with a for loop instead"
                )));
            }
        }
        if !ctx.args().iter().any(is_script_callable) {
            return (function.function)(ctx);
        }
        // Called again through the VM with the callbacks swapped for checked
        // ones; the instance goes with it, so the method form is unchanged.
        let args: Vec<KValue> = ctx
            .args()
            .iter()
            .map(|arg| {
                if is_script_callable(arg) {
                    checked(arg.clone())
                } else {
                    arg.clone()
                }
            })
            .collect();
        let instance = ctx.instance().clone();
        ctx.vm.call_instance_function(
            instance,
            KValue::NativeFunction(function.clone()),
            CallArgs::Separate(&args),
        )
    }))
}

/// Whether a value is something the script defined that the library would
/// call back into — a function, or a map with `@call`. A native function is
/// not: its time is the library's, which the checks here already cover.
fn is_script_callable(value: &KValue) -> bool {
    value.is_callable() && !matches!(value, KValue::NativeFunction(_))
}

/// `callback`, checking the budget of the call in progress before each run.
///
/// It is called exactly as the library would have called the original: the
/// same arguments, passed on as they came — a map's key and value arrive as
/// one tuple either way.
fn checked(callback: KValue) -> KValue {
    KValue::NativeFunction(KNativeFunction::new(move |ctx: &mut CallContext| {
        in_time()?;
        let args = ctx.args().to_vec();
        ctx.vm
            .call_function(callback.clone(), CallArgs::Separate(&args))
    }))
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
///
/// **On a thread of its own**, with a stack sized for the source
/// ([`compile_stack`]). The nesting count refuses the common deep shapes with
/// a message; the stack is what makes the rest — a chain of assignments, a
/// run of paren-less calls — something the parser survives rather than an
/// abort. A panic in koto's compiler (it has a few, such as running out of
/// registers) ends that thread and is reported as a script that will not
/// compile.
pub fn compile(source: &str) -> Result<Compiled, ScriptError> {
    // Before the parser sees it: a stack overflow there aborts the process, so
    // it cannot be caught afterwards.
    if source.len() > MAX_SOURCE_BYTES {
        return Err(ScriptError::TooLong);
    }
    if nesting_depth(source) > MAX_NESTING {
        return Err(ScriptError::TooDeep);
    }
    let owned = source.to_owned();
    on_compile_stack(source.len(), move || compile_here(&owned))
}

/// Runs `work` on a thread whose stack fits a script of `len` bytes, and
/// turns a panic on it into a compile error.
fn on_compile_stack<T: Send + 'static>(
    len: usize,
    work: impl FnOnce() -> Result<T, ScriptError> + Send + 'static,
) -> Result<T, ScriptError> {
    std::thread::Builder::new()
        .name("script compile".into())
        .stack_size(compile_stack(len))
        .spawn(work)
        .map_err(|e| ScriptError::Compile(format!("could not start the compiler: {e}")))?
        .join()
        .unwrap_or_else(|_| {
            Err(ScriptError::Compile(
                "the compiler failed on this script (it may be nested too deeply)".into(),
            ))
        })
}

fn compile_here(source: &str) -> Result<Compiled, ScriptError> {
    let mut koto = hardened(SCRIPT_DEADLINE);
    let chunk = koto
        .compile(CompileArgs::new(source).export_top_level_ids(true))
        .map_err(|e| ScriptError::Compile(e.to_string()))?;

    if can_import(&chunk) {
        return Err(ScriptError::Import);
    }
    if let Some(reason) = refused(&chunk) {
        return Err(ScriptError::Refused(reason));
    }
    Ok(Compiled {
        chunk,
        header: super::Header::parse(source),
    })
}

/// How deeply `source` nests, as the parser will recurse through it.
///
/// Two counts, added. **Brackets**: every `(`, `[` and `{` not yet closed,
/// outside strings and comments — and an interpolation `{…}` inside a string
/// is a bracket too, with strings inside it, so `'{'{'{…}'}'}'` counts. And
/// the **prefix run**: consecutive prefix operators (`-`, `!`, `~`, `not`) and
/// lambda heads (`|a|`, `||`), which nest without a bracket; whitespace leaves
/// it alone, a line break or anything else ends it.
///
/// Deliberately crude — a bound on the parser's recursion, not a parse, cheap
/// enough to run on every script the picker touches. It cannot see every
/// shape (a chain of assignments, or of paren-less calls, recurses too), and
/// it does not need to: the stack [`compile`] runs on bounds those.
fn nesting_depth(source: &str) -> usize {
    enum Frame {
        /// Code, and how many brackets were open when it began — an
        /// interpolation ends at the `}` that brings the count back there.
        Code(usize),
        /// A string, and its quote.
        Text(char),
    }

    let chars: Vec<char> = source.chars().collect();
    let mut frames = vec![Frame::Code(0)];
    let (mut open, mut run, mut deepest) = (0usize, 0usize, 0usize);
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        at += 1;
        if let Some(Frame::Text(quote)) = frames.last() {
            match c {
                '\\' => at += 1,
                '{' => {
                    open += 1;
                    frames.push(Frame::Code(open));
                }
                c if c == *quote => {
                    frames.pop();
                }
                _ => {}
            }
            deepest = deepest.max(open);
            continue;
        }
        let entered = match frames.last() {
            Some(Frame::Code(entered)) => *entered,
            _ => 0,
        };
        match c {
            '#' if chars.get(at) == Some(&'-') => {
                // `#- … -#`: skip to the close.
                while at < chars.len() && !(chars[at] == '-' && chars.get(at + 1) == Some(&'#')) {
                    at += 1;
                }
                at += 2;
            }
            '#' => {
                while at < chars.len() && chars[at] != '\n' {
                    at += 1;
                }
            }
            'r' if !chars
                .get(at.wrapping_sub(2))
                .is_some_and(|p| p.is_alphanumeric() || *p == '_')
                && matches!(chars.get(at), Some('\'' | '"' | '#')) =>
            {
                // A raw string: no escapes, no interpolation, `#`s either side.
                let hashes = chars[at..].iter().take_while(|h| **h == '#').count();
                at += hashes;
                let Some(&quote) = chars.get(at) else { break };
                at += 1;
                while at < chars.len()
                    && !(chars[at] == quote
                        && chars[at + 1..]
                            .iter()
                            .take(hashes)
                            .filter(|h| **h == '#')
                            .count()
                            == hashes)
                {
                    at += 1;
                }
                at += 1 + hashes;
                run = 0;
            }
            '\'' | '"' => {
                frames.push(Frame::Text(c));
                run = 0;
            }
            '(' | '[' | '{' => {
                open += 1;
                run = 0;
            }
            '}' if frames.len() > 1 && open == entered => {
                open -= 1;
                frames.pop();
            }
            ')' | ']' | '}' => {
                open = open.saturating_sub(1);
                run = 0;
            }
            '-' | '!' | '~' => run += 1,
            '|' => {
                // A lambda head: skip its arguments to the closing bar.
                while at < chars.len() && chars[at] != '|' && chars[at] != '\n' {
                    at += 1;
                }
                at += 1;
                run += 1;
            }
            'n' if chars[at..].starts_with(&['o', 't'])
                && !chars
                    .get(at + 2)
                    .is_some_and(|n| n.is_alphanumeric() || *n == '_')
                && !chars
                    .get(at.wrapping_sub(2))
                    .is_some_and(|p| p.is_alphanumeric() || *p == '_') =>
            {
                at += 2;
                run += 1;
            }
            '\n' => run = 0,
            c if c.is_whitespace() => {}
            _ => run = 0,
        }
        deepest = deepest.max(open + run);
    }
    deepest
}

/// A construct the deadline cannot police, if the chunk has one.
///
/// Three, all found in the bytecode — which covers nested functions, as the
/// import scan does:
///
/// * a **generator** (`yield`) or a map with its own `@next`, `@next_back`
///   or `@iterator`: an iterator that re-enters the VM for every item, with a
///   fresh limit each time, so a library loop over an endless one never ends;
/// * a **format spec** padding or rounding past [`MAX_FORMAT_WIDTH`], which
///   is one native allocation of that size.
fn refused(chunk: &Ptr<Chunk>) -> Option<&'static str> {
    use koto::parser::MetaKeyId;
    InstructionReader::new(chunk.clone()).find_map(|instruction| match instruction {
        Instruction::Yield { .. } => Some(
            "define a generator (`yield`): a loop the engine runs over one cannot be interrupted",
        ),
        Instruction::MetaInsert { id, .. } | Instruction::MetaInsertNamed { id, .. }
            if matches!(
                id,
                MetaKeyId::Next | MetaKeyId::NextBack | MetaKeyId::Iterator
            ) =>
        {
            Some(
                "define its own iterator (`@next`, `@next_back` or `@iterator`): a loop the \
                 engine runs over one cannot be interrupted",
            )
        }
        Instruction::StringPush {
            format_options: Some(options),
            ..
        } if options.min_width.unwrap_or(0) > MAX_FORMAT_WIDTH
            || options.precision.unwrap_or(0) > MAX_FORMAT_WIDTH =>
        {
            Some("pad or round a value to more than 1024 characters")
        }
        _ => None,
    })
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

    /// A range is a known length in O(1) of source, and the library walks it
    /// in a native loop the deadline never sees: `to_list` asks the allocator
    /// for its whole length up front (an abort at a hundred billion), and
    /// `count` simply never returns. A library call is refused a range or an
    /// iterator longer than a million items.
    #[test]
    fn a_script_cannot_hand_the_library_an_enormous_range() {
        for source in [
            "(0..100000000000).to_list()",
            "(0..10000000000000).count()",
            "(0..10000000000000).keep(|n| n > 0).count()",
            "iterator.sum 0..10000000000000",
            "x = []\nx.extend 0..100000000000",
            "x = {}\nx.extend 0..100000000000",
            "string.from_bytes 0..100000000000",
        ] {
            let err = run(source).unwrap_err();
            assert!(
                matches!(err, ScriptError::Runtime(ref m) if m.contains("too long")),
                "{source}: {err}"
            );
        }
        // A range of an ordinary length is ordinary.
        assert_eq!(run("(0..1000).count()").unwrap(), "1000");
    }

    /// A native loop that calls back into the script restarts koto's limit
    /// on every callback, so nesting two of them multiplies: a million
    /// callbacks each counting a million. Every library call also checks the
    /// budget of the call that is running, so the inner ones stop being made.
    #[test]
    fn nested_library_loops_still_run_out_of_time() {
        let deadline = Duration::from_millis(50);
        let compiled =
            compile("rename = || (0..1000000).each(|_| (0..1000000).count()).count()").unwrap();
        let mut koto = hardened(deadline);
        koto.run(compiled.chunk().clone()).unwrap();
        let started = std::time::Instant::now();
        let err = within(deadline, || {
            koto.call_exported_function("rename", CallArgs::Separate(&[]))
        })
        .map_err(|e| ScriptError::from_run(&e, deadline))
        .unwrap_err();
        assert_eq!(err, ScriptError::TooSlow { deadline });
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    /// And a library loop of callbacks that each do real work: every callback
    /// gets koto's limit afresh, so a million of them that each spin for a
    /// millisecond ran for a quarter of an hour. The callbacks the library is
    /// handed check the budget of the call they belong to before each run.
    #[test]
    fn a_library_loop_of_slow_callbacks_still_runs_out_of_time() {
        let deadline = Duration::from_millis(50);
        let compiled = compile(
            "spin = |_|\n  n = 0\n  for i in 0..20000\n    n += 1\n  n\n\
             rename = || (0..1000000).each(spin).count()",
        )
        .unwrap();
        let mut koto = hardened(deadline);
        koto.run(compiled.chunk().clone()).unwrap();
        let started = std::time::Instant::now();
        let err = within(deadline, || {
            koto.call_exported_function("rename", CallArgs::Separate(&[]))
        })
        .map_err(|e| ScriptError::from_run(&e, deadline))
        .unwrap_err();
        assert_eq!(err, ScriptError::TooSlow { deadline });
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    /// A callback wrapped for the budget still behaves as the one it wraps:
    /// its arguments, a map's key and value pairs, and what it returns.
    #[test]
    fn a_callback_behind_the_guard_behaves_as_before() {
        assert_eq!(
            run("(1..=4).keep(|n| n % 2 == 0).to_list()").unwrap(),
            "[2, 4]"
        );
        assert_eq!(run("[1, 2, 3].fold 0, |acc, n| acc + n").unwrap(), "6");
        assert_eq!(
            run("m = {a: 1, b: 2}\nm.each(|(k, v)| '{k}{v}').to_tuple()").unwrap(),
            "('a1', 'b2')"
        );
        assert_eq!(run("[3, 1, 2].to_list().sort()").unwrap(), "[1, 2, 3]");
    }

    /// `koto.deep_copy` recurses natively with no cycle check, so a list that
    /// contains itself overflows the stack.
    #[test]
    fn a_script_cannot_deep_copy() {
        let err = run("x = [0]\nx[0] = x\nkoto.deep_copy x").unwrap_err();
        assert!(
            matches!(err, ScriptError::Runtime(ref m) if m.contains("not found")),
            "{err}"
        );
    }

    /// A format width is one native `repeat`: `'{x:4000000000}'` asks for
    /// four gigabytes in a single call. The spec is in the bytecode, so it is
    /// refused at compile time.
    #[test]
    fn a_script_cannot_pad_to_an_absurd_width() {
        for source in [
            "x = 1\ny = '{x:4000000000}'",
            "x = 1.5\ny = '{x:.4000000000}'",
            "x = 'a'\ny = '{x:_>100000}'",
        ] {
            assert!(
                matches!(compile(source), Err(ScriptError::Refused(_))),
                "{source}"
            );
        }
        assert!(compile("x = 1\ny = '{x:08.2}'").is_ok());
    }

    /// A generator or an object with its own `@next` is an iterator whose
    /// every step re-enters the VM with a fresh limit, so a library loop over
    /// an endless one never ends. Neither is anything a rename needs.
    #[test]
    fn a_script_cannot_define_its_own_iterator() {
        for source in [
            "gen = ||\n  loop\n    yield 1\nx = gen().count()",
            "x = {@next: || 1}",
            "x = {@next_back: || 1}",
            "x = {@iterator: || 0..10}",
        ] {
            assert!(
                matches!(compile(source), Err(ScriptError::Refused(_))),
                "{source}"
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

    /// The shapes a count of *consecutive* openers could not see: brackets
    /// with something between them, lambdas, and interpolation inside
    /// interpolation. Each is one level of the parser's recursion per level
    /// of source, exactly like `((((`.
    #[test]
    fn interleaved_nesting_is_refused_too() {
        for source in [
            format!("x = {}1{}", "[1, ".repeat(400), "]".repeat(400)),
            format!("x = {}1{}", "f(".repeat(400), ")".repeat(400)),
            format!("x = {}1{}", "{a: ".repeat(400), "}".repeat(400)),
            format!("x = {}1", "|| ".repeat(400)),
            format!("x = {}1", "|a| ".repeat(400)),
            format!("x = {}1{}", "'{".repeat(400), "}'".repeat(400)),
        ] {
            assert_eq!(
                compile(&source).unwrap_err(),
                ScriptError::TooDeep,
                "{}",
                &source[..24]
            );
        }
    }

    /// Brackets in strings and comments are text, not nesting.
    #[test]
    fn brackets_in_text_are_not_nesting() {
        let parens = "(".repeat(400);
        assert!(compile(&format!("x = '{parens}'\ny = \"{parens}\"")).is_ok());
        assert!(compile(&format!("# {parens}\nx = 1")).is_ok());
        assert!(compile(&format!("x = r'{parens}'")).is_ok());
        assert!(compile("x = '{[1, 2].size()} and {{a: 1}.a}'").is_ok());
    }

    /// The count cannot see every shape the parser recurses on — a chain of
    /// assignments is one — so the compile also runs on a stack sized for the
    /// longest script accepted. This one is far past what a default thread's
    /// 2 MB survives, and must come back as an answer rather than an abort.
    #[test]
    fn a_shape_the_count_cannot_see_does_not_take_the_process_down() {
        let source = format!("x = {}1", "a = ".repeat(3000));
        assert!(
            nesting_depth(&source) <= MAX_NESTING,
            "the count sees it after all"
        );
        assert!(compile(&source).is_ok());
    }

    /// And the compiler has limits of its own that are panics rather than
    /// errors — a list literal nested past its register file is one, which
    /// the count refuses first today. Whatever the next one is, a panic in the
    /// compile is a script that will not compile, not a dead worker.
    #[test]
    fn a_compiler_panic_is_a_compile_error() {
        let result: Result<(), _> = on_compile_stack(0, || panic!("chunk size must be non-zero"));
        assert!(
            matches!(result, Err(ScriptError::Compile(ref m)) if m.contains("compiler failed")),
            "{result:?}"
        );
    }

    /// A script longer than any person writes is refused before it is parsed:
    /// the stack the parse is given is sized for the longest one accepted.
    #[test]
    fn a_script_past_the_size_limit_is_refused() {
        let source = format!("x = 1\n# {}", "x".repeat(MAX_SOURCE_BYTES));
        assert_eq!(compile(&source).unwrap_err(), ScriptError::TooLong);
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

    /// The character-counting forms the migration guide gives for `Len`,
    /// `Mid`, `Left` and `Right` — `size` and `[a..b]` count UTF-8 bytes, which
    /// is not what the VBScript functions did.
    #[test]
    fn the_character_idioms_the_migration_guide_teaches_work() {
        assert_eq!(run("'Björk'.chars().count()").unwrap(), "5");
        assert_eq!(run("size 'Björk'").unwrap(), "6", "size is bytes");
        assert_eq!(
            run("'Ärger.txt'.chars().skip(1).take(4).to_string()").unwrap(),
            "rger"
        );
        assert_eq!(run("'Ärger'.chars().take(2).to_string()").unwrap(), "Är");
        assert_eq!(
            run("s = 'Björk'\ns.chars().skip(s.chars().count() - 2).to_string()").unwrap(),
            "rk"
        );
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
