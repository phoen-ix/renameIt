//! **The planning budget, as CI enforces it.**
//!
//! Plans the bench's ten-thousand-file listing through the pipeline a preset
//! actually produces — built from `OpKind` per keystroke, the way the app
//! does it — through the real `ren_core::plan`, and exits non-zero when the
//! 95th percentile is over budget.
//!
//! This is the number of record for the *engine*. Spike B's harness
//! (`ren-gui/examples/spike_preview_headless.rs`) measures a whole frame, but
//! its recompute is `Pipeline::evaluate` alone — no naming validation, no
//! conflict detection, no rename ordering, none of the serial passes that
//! turn evaluations into a plan. It reported four milliseconds while the
//! shipped path took sixty-eight. `cargo bench --bench plan` measures the
//! right thing but asserts nothing and runs on nobody's runner; this is the
//! bench's `preset` arm with an exit code.
//!
//! ```text
//! cargo run --release -p ren-core --example plan_budget            # 10 000 rows, 50 ms
//! cargo run --release -p ren-core --example plan_budget -- 50000 40 200
//! ```
//!
//! The budget is an argument because the runners differ: this box plans the
//! listing in roughly twenty milliseconds, and a two-core CI VM is slower by
//! about the ratio of its cores.

use std::time::{Duration, Instant};

use ren_core::model::{FileEntry, Scope};
use ren_core::ops::{
    AddCounter, CaseMode, Casing, CounterPlacement, FreeFormat, OpKind, Replace, SpaceTrim,
    ZeroPadding,
};
use ren_core::{Pipeline, StepConfig};

/// The bench's listing, verbatim: something a user would actually open.
fn entries(count: usize) -> Vec<FileEntry> {
    const WORDS: [&str; 6] = [
        "Holiday_photo",
        "IMG",
        "the_quick_brown_fox",
        "Ünïcödé Trâck",
        "Meeting Notes (final)",
        "日本語のファイル",
    ];
    const EXTENSIONS: [&str; 4] = ["mp3", "JPG", "txt", "flac"];
    (0..count)
        .map(|i| {
            let file_name = format!(
                "{} {:05} - {}.{}",
                WORDS[i % WORDS.len()],
                i,
                WORDS[(i / 3) % WORDS.len()],
                EXTENSIONS[i % EXTENSIONS.len()]
            );
            FileEntry::synthetic(std::path::PathBuf::from("/bench").join(&file_name))
        })
        .collect()
}

/// The bench's `preset` pipeline, verbatim: what a cleanup preset looks like,
/// including the fifty-one-rule Batch Replace.
fn preset_like() -> Vec<(OpKind, StepConfig)> {
    vec![
        (
            OpKind::Replace(Replace::new("_", " ")),
            StepConfig::default(),
        ),
        (
            OpKind::Replace(Replace::new(r"^(\d+) - ", "$1. ").regex(true)),
            StepConfig::default(),
        ),
        (
            OpKind::BatchReplace(Default::default()),
            StepConfig::default(),
        ),
        (
            OpKind::Casing(Casing {
                lowercase_exceptions: true,
                preserve_all_upper: true,
                ..Casing::new(CaseMode::Title)
            }),
            StepConfig::scoped(Scope::Name),
        ),
        (
            OpKind::Casing(Casing::new(CaseMode::Lower)),
            StepConfig::scoped(Scope::Extension),
        ),
        (
            OpKind::SpaceTrim(SpaceTrim::default()),
            StepConfig {
                filter: Some(
                    ren_core::IncludeFilter::new()
                        .including(ren_core::MatchSpec::Substring("a".into())),
                ),
                ..StepConfig::default()
            },
        ),
        (
            OpKind::ZeroPadding(ZeroPadding::new(4)),
            StepConfig::default(),
        ),
        (
            OpKind::AddCounter(AddCounter::new(CounterPlacement::First, ". ")),
            StepConfig::default(),
        ),
        (
            OpKind::FreeFormat(FreeFormat::new("<Counter> <Name>")),
            StepConfig::scoped(Scope::Both),
        ),
    ]
}

fn build(steps: &[(OpKind, StepConfig)]) -> Pipeline {
    let mut pipeline = Pipeline::new();
    for (op, config) in steps {
        pipeline.push(op.to_step(), config.clone());
    }
    pipeline
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rows: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(10_000);
    let iterations: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(40);
    let budget_ms: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(50);
    let budget = Duration::from_millis(budget_ms);

    println!("RenameIt — planning budget: {rows} rows, {iterations} plans, {budget_ms} ms");

    let platform = ren_platform::host();
    let files = entries(rows);
    let steps = preset_like();

    // The first plan compiles every pattern into the process-wide caches
    // (P40) and warms the allocator; a user's first keystroke pays that once
    // and every later one does not, so it is not what is measured.
    for _ in 0..3 {
        let pipeline = build(&steps);
        std::hint::black_box(ren_core::plan(&files, &pipeline, platform.as_ref()));
    }

    let mut times = Vec::with_capacity(iterations);
    let mut changed = 0;
    for _ in 0..iterations {
        let started = Instant::now();
        // The pipeline is built inside the timing, because that is what the
        // app does on every keystroke: `to_step` clones, and a clone resets
        // every operation's compiled cache (D21).
        let pipeline = build(&steps);
        let plan = ren_core::plan(&files, &pipeline, platform.as_ref());
        times.push(started.elapsed());
        changed = plan.changed();
        std::hint::black_box(plan);
    }
    times.sort_unstable();

    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "plan       p50 {:>7.2}ms  p95 {:>7.2}ms  max {:>7.2}ms",
        ms(percentile(&times, 0.50)),
        ms(percentile(&times, 0.95)),
        ms(percentile(&times, 1.00)),
    );
    println!("rows renamed by the plan: {changed} (of {rows})");

    if changed == 0 {
        println!("\nVERDICT: INVALID — the plan renamed nothing, the timings are meaningless");
        std::process::exit(2);
    }
    let p95 = percentile(&times, 0.95);
    if p95 <= budget {
        println!(
            "\nVERDICT: within budget — p95 {:.2}ms <= {budget_ms}ms",
            ms(p95)
        );
    } else {
        println!(
            "\nVERDICT: OVER BUDGET — p95 {:.2}ms > {budget_ms}ms",
            ms(p95)
        );
        std::process::exit(1);
    }
}
