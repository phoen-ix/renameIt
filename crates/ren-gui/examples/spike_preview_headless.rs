//! **Spike B, headless.** The number of record for `docs/spikes/preview-perf.md`.
//!
//! Measures a whole keystroke end to end:
//!
//! 1. parallel recompute of every row's new name (rayon + generation counter),
//! 2. a full egui frame — layout of the *visible* rows in a virtualized
//!    `egui_table`, plus tessellation into draw commands.
//!
//! It never opens a window, so it produces the same number on a CI runner, on
//! a headless box over SSH, and on the user's Windows machine. Budget: 50 ms.
//!
//! A third argument, `grid`, measures the **thumbnail grid** over the same
//! listing instead of the table. The same budget, deliberately: two budgets
//! would let the grid be slow while the number of record stayed green. And the
//! assertion that matters there is the integer, not the milliseconds — this
//! machine shows 12-30% run-to-run variance on identical code (M8), so
//! a timing cannot tell a regression from a noisy afternoon. A tile count can.
//!
//! Headless has no GPU, so what this measures is **layout**, never upload.
//!
//! ```text
//! cargo run --release -p ren-gui --example spike_preview_headless
//! cargo run --release -p ren-gui --example spike_preview_headless -- 50000 200
//! cargo run --release -p ren-gui --example spike_preview_headless -- 10000 100 grid
//! ```

use std::time::{Duration, Instant};

use ren_gui::panels::grid::Grid;
use ren_gui::panels::tile::Look;
use ren_gui::spike::{PreviewEngine, PreviewTable, synthetic_entries};
use ren_gui::thumbs::Thumbs;
use ren_gui::viewmodel::{RowFilter, THUMB_DEFAULT};

/// A user typing `Holiday` one character at a time, then deleting it again.
fn keystrokes() -> Vec<String> {
    let word = "Holiday";
    let mut out: Vec<String> = (1..=word.len()).map(|n| word[..n].to_owned()).collect();
    out.extend((1..word.len()).rev().map(|n| word[..n].to_owned()));
    out
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
    let iterations: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(100);
    let as_grid = args.next().is_some_and(|a| a == "grid");

    println!("RenameIt — Spike B (preview performance), headless");
    println!(
        "view: {}",
        if as_grid {
            "grid (thumbnail tiles — layout only, there is no GPU here)"
        } else {
            "list (virtualized table)"
        }
    );
    println!("rows: {rows}, keystrokes: {iterations}");
    println!("threads: {}", rayon::current_num_threads());
    println!(
        "profile: {}",
        if cfg!(debug_assertions) {
            "debug (numbers are NOT representative — use --release)"
        } else {
            "release"
        }
    );

    let entries = synthetic_entries(rows);
    let mut engine = PreviewEngine::new(entries.clone());
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 900.0));
    let look = Look {
        size: THUMB_DEFAULT,
        border: false,
    };
    // The real grid, not a stand-in: a copy of its arithmetic here would prove
    // only that the copy is fast.
    let mut thumbs = Thumbs::new(|| {});
    let mut cell_size = egui::Vec2::ZERO;

    let mut frame = |engine: &PreviewEngine, thumbs: &mut Thumbs| -> (Duration, usize, usize) {
        let raw_input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut cells = 0;
        let started = Instant::now();
        thumbs.begin_frame();
        thumbs.poll();
        let output = ctx.run_ui(raw_input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if as_grid {
                    let mut selection = ren_gui::viewmodel::Selection::default();
                    let mut inline = None;
                    let mut grid = Grid::new(
                        &entries,
                        None,
                        &[],
                        &mut selection,
                        RowFilter::All,
                        &mut inline,
                        thumbs,
                        look,
                    );
                    cell_size = grid.cell_size();
                    grid.show(ui);
                    cells = grid.laid_out;
                } else {
                    let mut table = PreviewTable::new(&engine.rows);
                    table.show(ui);
                    cells = table.cells_drawn;
                }
            });
        });
        // Tessellation is part of a frame's cost, so it is part of the budget.
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        // Vertex count is the proof that something was actually drawn — a
        // silently-empty table would otherwise look brilliantly fast.
        let vertices: usize = primitives
            .iter()
            .map(|p| match &p.primitive {
                egui::epaint::Primitive::Mesh(mesh) => mesh.vertices.len(),
                _ => 0,
            })
            .sum();
        (started.elapsed(), cells, vertices)
    };

    // egui does font loading and a full sizing pass on the first frames; warm up
    // so we measure the steady state a user actually experiences.
    for _ in 0..8 {
        engine.recompute("Holiday", "Vacation");
        frame(&engine, &mut thumbs);
    }

    let strokes = keystrokes();
    let mut computes = Vec::with_capacity(iterations);
    let mut frames = Vec::with_capacity(iterations);
    let mut totals = Vec::with_capacity(iterations);
    let mut cells_last = 0;
    let mut vertices_last = 0;
    let mut changed_last = 0;

    for i in 0..iterations {
        let find = &strokes[i % strokes.len()];
        let stats = engine.recompute(find, "Vacation");
        let (frame_time, cells, vertices) = frame(&engine, &mut thumbs);

        computes.push(stats.duration);
        frames.push(frame_time);
        totals.push(stats.duration + frame_time);
        cells_last = cells;
        vertices_last = vertices;
        changed_last = stats.changed;
    }

    for series in [&mut computes, &mut frames, &mut totals] {
        series.sort_unstable();
    }

    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!("\n                 p50        p95        max");
    for (label, series) in [
        ("recompute ", &computes),
        ("frame     ", &frames),
        ("TOTAL     ", &totals),
    ] {
        println!(
            "{label}  {:>7.2}ms {:>7.2}ms {:>7.2}ms",
            ms(percentile(series, 0.50)),
            ms(percentile(series, 0.95)),
            ms(percentile(series, 1.00)),
        );
    }

    let what = if as_grid { "tiles" } else { "cells" };
    println!("\nrows changed on the last keystroke: {changed_last}");
    println!("{what} laid out on the last frame:   {cells_last} (of {rows} rows)");
    println!("vertices tessellated:               {vertices_last}");

    if vertices_last == 0 {
        println!("\nVERDICT: INVALID — nothing was drawn, the timings are meaningless");
        std::process::exit(2);
    }

    // The integer, not the timing. Four screenfuls of slack: the grid draws one
    // row of overscan on purpose, and a window is not an exact number of tiles.
    if as_grid {
        let per_row = (screen.width() / cell_size.x).floor().max(1.0);
        let visible_rows = (screen.height() / cell_size.y).ceil() + 1.0;
        let capacity = (per_row * visible_rows) as usize;
        println!(
            "a {}x{} window holds about {capacity} tiles",
            screen.width() as u32,
            screen.height() as u32
        );
        if cells_last > capacity * 4 {
            println!(
                "\nVERDICT: NOT VIRTUALIZED — {cells_last} tiles laid out for a window that \
                 holds {capacity}"
            );
            std::process::exit(3);
        }
    }

    let budget = Duration::from_millis(50);
    let p95 = percentile(&totals, 0.95);
    if p95 <= budget {
        println!("\nVERDICT: within budget — p95 {:.2}ms <= 50ms", ms(p95));
    } else {
        println!("\nVERDICT: OVER BUDGET — p95 {:.2}ms > 50ms", ms(p95));
        std::process::exit(1);
    }
}
