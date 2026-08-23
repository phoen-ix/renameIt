//! Planning throughput.
//!
//! M1 budgets **a 10 000-file plan under 50 ms**, because this
//! runs on every keystroke behind the preview. Spike B already measured the
//! whole GUI round trip (`docs/spikes/preview-perf.md`); this measures the
//! engine on its own, so a regression can be attributed without a GUI in the
//! way.
//!
//! ```text
//! cargo bench -p ren-core --bench plan
//! ```

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use ren_core::model::{FileEntry, Scope};
use ren_core::ops::{AddRemove, CaseMode, Casing, OpKind, Replace, SpaceTrim};
use ren_core::{Pipeline, StepConfig, plan};

/// A listing that looks like something a user would actually open.
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

/// One trivial step, to isolate the planner's own overhead.
fn minimal() -> Pipeline {
    Pipeline::new().then(Replace::new("_", " "))
}

/// What a real cleanup preset looks like: five steps, two of them scoped.
fn realistic() -> Pipeline {
    Pipeline::new()
        .then(Replace::new("_", " "))
        .then(Replace::new(r"^(\d+) - ", "$1. ").regex(true))
        .then_scoped(
            Casing {
                lowercase_exceptions: true,
                preserve_all_upper: true,
                ..Casing::new(CaseMode::Title)
            },
            Scope::Name,
        )
        .then_scoped(Casing::new(CaseMode::Lower), Scope::Extension)
        .then(SpaceTrim::default())
        .then(AddRemove::add(" [v2]", 0).backwards(true))
}

/// What M4 actually ships: a pipeline built from `OpKind` on every request,
/// the way the GUI does it.
///
/// `realistic()` above builds its `Pipeline` once, outside `b.iter`, so every
/// regex is compiled exactly once and the tag engine never appears. The app
/// rebuilds from `Vec<(OpKind, StepConfig)>` per preview — `to_transform()`
/// clones, and a clone resets the compiled cache (D21) — so this measures the
/// clones and the recompiles too.
fn preset_like() -> Vec<(OpKind, StepConfig)> {
    use ren_core::ops::{AddCounter, CounterPlacement, FreeFormat, ZeroPadding};

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

fn bench_plan(c: &mut Criterion) {
    let platform = ren_platform::host();
    let mut group = c.benchmark_group("plan");

    for count in [1_000usize, 10_000, 50_000] {
        let files = entries(count);
        group.throughput(criterion::Throughput::Elements(count as u64));

        group.bench_with_input(BenchmarkId::new("minimal", count), &files, |b, files| {
            let pipeline = minimal();
            b.iter(|| black_box(plan(files, &pipeline, platform.as_ref())));
        });

        group.bench_with_input(BenchmarkId::new("realistic", count), &files, |b, files| {
            let pipeline = realistic();
            b.iter(|| black_box(plan(files, &pipeline, platform.as_ref())));
        });

        // The pipeline is built *inside* the loop, because that is what the app
        // does on every keystroke.
        group.bench_with_input(BenchmarkId::new("preset", count), &files, |b, files| {
            let steps = preset_like();
            b.iter(|| {
                let pipeline = build(&steps);
                black_box(plan(files, &pipeline, platform.as_ref()))
            });
        });
    }

    group.finish();
}

/// The only arm that opens a file.
///
/// Every other arm — and Spike B, and the whole CI performance gate — plans
/// over `FileEntry::synthetic` paths under `/bench` that **do not exist**, so
/// `tags_of` returns `None` on the first `stat` and no metadata is ever read.
/// That is fine for measuring the planner, and it means the recorded budgets
/// have never included the cost of a music pipeline. This one does.
///
/// Tags without audio frames — ~40 bytes each, the shape the cache test uses —
/// because the question is what opening, parsing and caching ten thousand files
/// costs, not what walking MPEG frames costs. The corpus is written once,
/// outside the measured loop; the cache is *not* cleared between iterations,
/// because a warm cache is what a keystroke actually meets.
fn bench_music(c: &mut Criterion) {
    use ren_core::meta::testing::Mp3;

    let platform = ren_platform::host();
    let dir = tempfile::TempDir::new().expect("tempdir");
    for i in 0..10_000 {
        Mp3 {
            id3v2: vec![("TPE1", format!("Artist {i:05}")), ("TIT2", "Title".into())],
            audio: false,
            ..Default::default()
        }
        .write(dir.path(), &format!("track_{i:05}.mp3"));
    }
    let files = ren_core::list(dir.path(), Default::default()).expect("listing");
    assert_eq!(files.len(), 10_000);

    let pipeline = Pipeline::new().with(
        ren_core::Step::Name(Box::new(ren_core::ops::MusicRename::new(
            "<Artist> - <Title>",
        ))),
        StepConfig::for_op(&OpKind::MusicRename(Default::default())),
    );

    let mut group = c.benchmark_group("plan");
    group.throughput(criterion::Throughput::Elements(files.len() as u64));
    group.bench_function(BenchmarkId::new("music", files.len()), |b| {
        b.iter(|| black_box(plan(&files, &pipeline, platform.as_ref())));
    });
    group.finish();
}

/// The pure pass on its own, without conflict detection or the disk probe —
/// this is the part that must stay parallel.
fn bench_evaluate(c: &mut Criterion) {
    let files = entries(10_000);
    let pipeline = realistic();
    c.bench_function("evaluate_all/10000", |b| {
        b.iter(|| black_box(ren_core::evaluate_all(&files, &pipeline)));
    });
}

/// A scripted plan over ten thousand files.
///
/// The arm that measures what M7 gave up. A script step makes the evaluation
/// pass **serial** (D94) — session-global script state is sequential, so a
/// stateful script evaluated through rayon would preview differently every
/// time — and this is the price of that in wall clock.
///
/// It is not held to the 50 ms keystroke budget, and pretending otherwise
/// would be the dishonest move: a script is an interpreter call per file, on
/// one core, and no amount of care makes ten thousand of those free. What the
/// budget buys instead is that the window never blocks, because M0's
/// generation counter cancels a stale recompute while it is still running.
/// This number is here so that a future change which makes it *worse* is
/// visible.
fn bench_script(c: &mut Criterion) {
    let platform = ren_platform::host();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let scripts = tempfile::TempDir::new().expect("tempdir");
    for i in 0..10_000 {
        std::fs::write(dir.path().join(format!("file_{i:05}.txt")), b"x").expect("write");
    }
    std::fs::write(
        scripts.path().join("Bench.koto"),
        "state = {n: 0}\nrename = ||\n  state.n = state.n + 1\n  '{state.n}-{fr.filename}'",
    )
    .expect("write script");

    let files = ren_core::list(dir.path(), Default::default()).expect("listing");
    assert_eq!(files.len(), 10_000);

    let pipeline = Pipeline::new().then(ren_core::ops::Script::new("Bench").in_dir(scripts.path()));

    let mut group = c.benchmark_group("plan");
    group.throughput(criterion::Throughput::Elements(files.len() as u64));
    group.bench_function(BenchmarkId::new("script", files.len()), |b| {
        b.iter(|| black_box(plan(&files, &pipeline, platform.as_ref())));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_plan,
    bench_music,
    bench_evaluate,
    bench_script
);
criterion_main!(benches);
