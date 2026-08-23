# Spike B — preview performance verdict

*M0's preview-performance spike. Written 2026-08-15. Harness:
`crates/ren-gui/examples/spike_preview_headless.rs` (numbers of record) and
`crates/ren-gui/examples/spike_preview_app.rs` (windowed confirmation), sharing
`crates/ren-gui/src/spike.rs`.*

## Question

The modernised UI recomputes every row's "new name" on **every keystroke** in
the Find box. `docs/DESIGN.md` calls this "an architectural commitment, not an
optimization to defer" and budgets **< 50 ms end to end at 10 000 files**. Does
the design — rayon parallel pure pass, generation counter, virtualized
`egui_table` — actually hold?

## Verdict

**Yes, with roughly 13× headroom at the target size.** p95 for a full
keystroke at 10 000 rows is **4.0 ms** against a 50 ms budget. The design in
`docs/DESIGN.md` Part 1 §4 stands unchanged; no architectural revision is
needed for M2.

## Results

AMD Ryzen 7 3700X (8 cores), 23 GB RAM, Linux 6.8, rustc 1.97.1, release
profile, 200 simulated keystrokes per row count, 1400×900 viewport.

Each "keystroke" is measured end to end: bump the generation counter → compile
the pattern → recompute **every** row's new name with rayon → run a full egui
pass (layout of the visible rows in `egui_table`) → tessellate into draw
commands.

| Rows | recompute p50 / p95 | frame p50 / p95 | **TOTAL p50 / p95 / max** | Budget |
|---:|---|---|---|---|
| 1 000 | 0.48 / 0.72 ms | 0.51 / 0.71 ms | **1.00 / 1.31 / 2.08 ms** | ✅ |
| 10 000 | 1.69 / 3.49 ms | 0.56 / 0.83 ms | **2.29 / 4.01 / 6.77 ms** | ✅ |
| 50 000 | 7.12 / 9.95 ms | 0.70 / 0.94 ms | **7.83 / 10.73 / 14.69 ms** | ✅ |
| 100 000 | 14.62 / 17.25 ms | 0.71 / 0.94 ms | **15.30 / 18.24 / 28.75 ms** | ✅ |

This closes the `docs/DESIGN.md` risk item *"validate the two-phase preview
holds the <50 ms feel at 50k–100k files before freezing the design"*: it does,
with 100 000 files still under 19 ms p95.

### Windows

The harness needs no display, so it also runs in CI. On the `windows-latest`
runner — a **2-core** VM, i.e. a quarter of the dev box's parallelism — at
10 000 rows:

| | p50 | p95 | max |
|---|---|---|---|
| recompute | 3.01 ms | 4.67 ms | 5.59 ms |
| frame | 0.58 ms | 0.89 ms | 1.13 ms |
| **TOTAL** | **3.59 ms** | **5.25 ms** | **6.53 ms** |

Same 132 cells, same 10 640 vertices. The shipping platform is comfortably
inside budget on the weakest hardware we test on, which is the number that
actually matters.

The workload is deliberately unflattering. The transform is a **regex** Find &
Replace (the most expensive thing the box accepts), not the trivial
append-suffix operation M0 otherwise ships, and it runs over synthetic names
containing spaces, digits, accented Latin, and CJK. Roughly 21 % of rows change
on the measured keystroke.

## What the numbers show

**Recompute dominates and scales linearly.** 10k → 100k is a 10× row count for
a 8.7× time increase; rayon is doing its job across 8 cores. Frame time is
essentially flat because it depends on the *viewport*, not the listing.

**Virtualization is real.** At every row count the table laid out exactly **132
cells** — 44 visible rows × 3 columns — and tessellated ~10 640 vertices. A
100 000-row listing costs the renderer the same as a 1 000-row one. (The vertex
count is asserted non-zero in the harness: a silently empty table would
otherwise look wonderfully fast.)

**The generation counter works.** Each pass reads the counter inside the
parallel map and abandons its row immediately if a newer keystroke has landed,
so a superseded recompute stops paying for itself rather than racing to
completion. The windowed app surfaces this as a "· superseded" marker.

**An invalid pattern is free.** While the user is mid-way through typing a
regex, most intermediate strings do not compile. That path returns before any
row is touched, which is why the p50 stays low across a whole typed word.

## Consequences for M2

1. **Recompute must move off the UI thread.** 17 ms at 100k rows is fine as a
   budget but is more than a 60 Hz frame; the measured cost is the *work*, not
   the *stall*. The channel + generation-counter design in `docs/DESIGN.md`
   Part 2 §2 stays mandatory.
2. **No debouncing needed at realistic sizes.** At 10k rows a keystroke costs
   4 ms; adding input latency to save that would make the app feel worse.
   Revisit only if metadata-backed tags (M6) push the pure pass up.
3. **`egui_table` is confirmed** as the file-table widget (0.10 ↔ egui 0.36).
4. **The budget line is `recompute`, not `frame`.** Future work that threatens
   it is per-row *IO* — audio tags, EXIF, CRC32. That is exactly what
   `TagNeeds` + `MetaCache` + `Pending` placeholders exist for; keep them.
5. Everything here was measured with a **software** rasteriser and no GPU. Real
   hardware can only be faster.

## Caveats

- Two machines only: one dev box and the CI runners. Neither has a GPU, and
  neither is a 4K high-DPI display, which is where frame cost would grow.
- The pure pass has no stateful tags. Counters and unique-random tags force the
  serial finalize pass described in `docs/DESIGN.md` Part 1 §4; that pass is
  O(n) string patching and is expected to be cheap, but it is **M3's** measure
  to take, not this spike's.
- No metadata IO. Music/EXIF tags (M6) hit the disk and are cache-backed by
  design; they are out of scope here.

## Reproducing

```sh
cargo run --release -p ren-gui --example spike_preview_headless             # 10 000 rows
cargo run --release -p ren-gui --example spike_preview_headless -- 100000 200
cargo run --release -p ren-gui --example spike_preview_headless -- 10000 100 grid

# Windowed confirmation (needs a display; software GL is fine)
cargo run --release -p ren-gui --example spike_preview_app
xvfb-run -a cargo run --release -p ren-gui --example spike_preview_app
```

The headless harness exits non-zero if p95 exceeds the 50 ms budget, so it can
be wired into CI as a regression gate whenever that becomes worthwhile.

## M8: the `grid` arm

A third argument draws the same listing as **thumbnail tiles** instead of table
rows, against the same 50 ms budget. The same budget deliberately: two budgets
would let the grid go slow while the number of record stayed green.

Its real assertion is not the timing. This machine measures 12–30% apart on two
runs of *identical* code (measured again in M8), so a millisecond figure there
cannot distinguish a regression from a busy afternoon. The **tile count** can:
the harness computes how many tiles a 1400×900 window holds from the grid's own
cell size and exits non-zero above four screenfuls. A lost virtualization shows
up there long before it shows up as a stopwatch reading — 10 000 rows currently
lay out **104 tiles**, which is exactly the window's capacity.

One caveat, and it is the same one this whole document carries: headless has no
GPU, so this measures **layout**. Texture upload is not in it, and cannot be.
