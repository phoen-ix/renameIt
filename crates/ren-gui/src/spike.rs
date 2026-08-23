//! **Spike B** — throwaway harness for M0's preview-performance spike.
//!
//! Question: can we recompute the "new name" of every row in a 10 000-file
//! listing on *every keystroke*, and still paint a frame, inside 50 ms?
//!
//! This is not production code. It exists to produce the number in
//! `docs/spikes/preview-perf.md` and to prove the architecture in
//! `docs/DESIGN.md` Part 1 §4 — parallel pure pass, generation counter,
//! virtualized table — before M2 commits to it.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use ren_core::model::{FileEntry, Scope};
use ren_core::ops::{EvalCx, NameTransform, OpError};
use ren_core::pipeline::{Pipeline, Step, StepConfig};
use ren_core::regex_flavor::{Pattern, PatternOptions};

/// A regex Find & Replace, i.e. the most expensive thing a user can type into
/// the box that recomputes per keystroke. Using `AppendSuffix` here would
/// flatter the numbers.
#[derive(Debug)]
struct RegexReplace {
    pattern: Pattern,
    replacement: String,
}

impl NameTransform for RegexReplace {
    fn id(&self) -> &'static str {
        "spike_regex_replace"
    }

    fn summary(&self) -> String {
        format!("Regex replace -> {:?}", self.replacement)
    }

    fn apply<'a>(&self, subject: &'a str, _cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        match self.pattern.replace_all(subject, &self.replacement) {
            Ok(s) if s == subject => Ok(Cow::Borrowed(subject)),
            Ok(s) => Ok(Cow::Owned(s)),
            Err(e) => Err(OpError::Failed {
                operation: "regex replace",
                message: e.to_string(),
            }),
        }
    }
}

/// A listing that looks like something a user would actually open: mixed
/// extensions, digits, spaces, unicode, varying lengths.
pub fn synthetic_entries(count: usize) -> Vec<FileEntry> {
    const EXTENSIONS: [&str; 6] = ["mp3", "jpg", "txt", "flac", "MP4", "tar.gz"];
    const WORDS: [&str; 8] = [
        "Holiday",
        "IMG",
        "the_quick_brown_fox",
        "Ünïcödé Trâck",
        "DSC",
        "Meeting Notes (final)",
        "日本語のファイル",
        "report-v2",
    ];
    (0..count)
        .map(|i| {
            let file_name = format!(
                "{} {:04} - {}.{}",
                WORDS[i % WORDS.len()],
                i,
                WORDS[(i / 3) % WORDS.len()],
                EXTENSIONS[i % EXTENSIONS.len()]
            );
            FileEntry::synthetic(std::path::PathBuf::from("/spike").join(&file_name))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct PreviewRow {
    pub old_name: String,
    pub new_name: String,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RecomputeStats {
    pub generation: u64,
    pub duration: Duration,
    pub changed: usize,
    /// True if a newer keystroke landed while this pass was running.
    pub cancelled: bool,
    /// False when the typed pattern does not compile — the common case while
    /// the user is still typing, and it must stay cheap.
    pub pattern_valid: bool,
}

/// The parallel pure pass with a generation counter, exactly as designed.
pub struct PreviewEngine {
    entries: Vec<FileEntry>,
    generation: AtomicU64,
    pub rows: Vec<PreviewRow>,
}

impl PreviewEngine {
    pub fn new(entries: Vec<FileEntry>) -> Self {
        let rows = entries
            .iter()
            .map(|e| PreviewRow {
                old_name: e.file_name.clone(),
                new_name: e.file_name.clone(),
                changed: false,
            })
            .collect();
        Self {
            entries,
            generation: AtomicU64::new(0),
            rows,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// One keystroke: bump the generation, rebuild the pipeline, recompute
    /// every row in parallel, bail out if a newer generation started.
    pub fn recompute(&mut self, find: &str, replace: &str) -> RecomputeStats {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let started = Instant::now();

        let Ok(pattern) = Pattern::compile(find, PatternOptions::default()) else {
            return RecomputeStats {
                generation,
                duration: started.elapsed(),
                changed: 0,
                cancelled: false,
                pattern_valid: false,
            };
        };

        let pipeline = Pipeline::new().with(
            Step::Name(Box::new(RegexReplace {
                pattern,
                replacement: replace.to_owned(),
            })),
            StepConfig::scoped(Scope::Name),
        );

        let total = self.entries.len();
        let cancelled = AtomicBool::new(false);
        let names: Vec<String> = self
            .entries
            .par_iter()
            .enumerate()
            .map(|(index, entry)| {
                // Stale-generation check: a newer keystroke wins, this pass stops
                // doing work immediately.
                if self.generation.load(Ordering::Relaxed) != generation {
                    cancelled.store(true, Ordering::Relaxed);
                    return entry.file_name.clone();
                }
                pipeline
                    .evaluate(&EvalCx::simple(entry, index, total))
                    .map(|ev| ev.name)
                    .unwrap_or_else(|_| entry.file_name.clone())
            })
            .collect();

        let mut changed = 0;
        for (row, new_name) in self.rows.iter_mut().zip(names) {
            row.changed = new_name != row.old_name;
            changed += usize::from(row.changed);
            row.new_name = new_name;
        }

        RecomputeStats {
            generation,
            duration: started.elapsed(),
            changed,
            cancelled: cancelled.load(Ordering::Relaxed),
            pattern_valid: true,
        }
    }
}

/// Renders the preview rows into an `egui_table`, which only asks for the rows
/// that are actually visible.
pub struct PreviewTable<'a> {
    pub rows: &'a [PreviewRow],
    /// How many cells the table asked us to draw — the proof that virtualization
    /// is doing its job.
    pub cells_drawn: usize,
}

impl<'a> PreviewTable<'a> {
    pub fn new(rows: &'a [PreviewRow]) -> Self {
        Self {
            rows,
            cells_drawn: 0,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        egui_table::Table::new()
            .id_salt("spike_preview")
            .num_rows(self.rows.len() as u64)
            .columns(vec![
                egui_table::Column::new(120.0),
                egui_table::Column::new(320.0),
                egui_table::Column::new(320.0),
            ])
            .headers(vec![egui_table::HeaderRow::new(22.0)])
            .show(ui, self);
    }
}

impl egui_table::TableDelegate for PreviewTable<'_> {
    fn header_cell_ui(&mut self, ui: &mut egui::Ui, cell: &egui_table::HeaderCellInfo) {
        let title = match cell.col_range.start {
            0 => "#",
            1 => "Name",
            _ => "New name",
        };
        ui.label(egui::RichText::new(title).strong());
    }

    fn cell_ui(&mut self, ui: &mut egui::Ui, cell: &egui_table::CellInfo) {
        self.cells_drawn += 1;
        let Some(row) = self.rows.get(cell.row_nr as usize) else {
            return;
        };
        match cell.col_nr {
            0 => {
                ui.label(format!("{}", cell.row_nr + 1));
            }
            1 => {
                ui.label(&row.old_name);
            }
            _ => {
                let text = egui::RichText::new(&row.new_name);
                ui.label(if row.changed {
                    text.color(egui::Color32::from_rgb(0x4c, 0xaf, 0x50))
                } else {
                    text.weak()
                });
            }
        }
    }
}
