//! **Spike B, windowed.** Visual confirmation only — the recorded numbers come
//! from `spike_preview_headless`, which runs anywhere.
//!
//! Type in the Find box and watch a 10 000-row virtualized table update. The
//! header shows the last recompute time and the frame time.
//!
//! ```text
//! cargo run --release -p ren-gui --example spike_preview_app
//! xvfb-run -a cargo run --release -p ren-gui --example spike_preview_app   # headless box
//! ```

use std::time::Duration;

use ren_gui::spike::{PreviewEngine, PreviewTable, RecomputeStats, synthetic_entries};

fn main() -> eframe::Result {
    let rows: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(10_000);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RenameIt — Spike B",
        options,
        Box::new(move |_cc| Ok(Box::new(SpikeApp::new(rows)))),
    )
}

struct SpikeApp {
    engine: PreviewEngine,
    find: String,
    replace: String,
    last: Option<RecomputeStats>,
    frame_time: Duration,
}

impl SpikeApp {
    fn new(rows: usize) -> Self {
        let mut engine = PreviewEngine::new(synthetic_entries(rows));
        let last = Some(engine.recompute("Holiday", "Vacation"));
        Self {
            engine,
            find: "Holiday".to_owned(),
            replace: "Vacation".to_owned(),
            last,
            frame_time: Duration::ZERO,
        }
    }
}

impl eframe::App for SpikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = std::time::Instant::now();

        egui::Panel::top(egui::Id::new("controls")).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Find (regex):");
                let find = ui.add(egui::TextEdit::singleline(&mut self.find).desired_width(220.0));
                ui.label("Replace:");
                let replace =
                    ui.add(egui::TextEdit::singleline(&mut self.replace).desired_width(220.0));
                if find.changed() || replace.changed() {
                    self.last = Some(self.engine.recompute(&self.find, &self.replace));
                }
            });

            ui.horizontal(|ui| {
                ui.label(format!("{} rows", self.engine.len()));
                ui.separator();
                match &self.last {
                    Some(s) if !s.pattern_valid => {
                        ui.colored_label(egui::Color32::from_rgb(0xe5, 0x73, 0x73), "invalid regex")
                    }
                    Some(s) => ui.label(format!(
                        "recompute {:.2} ms · {} changed{}",
                        s.duration.as_secs_f64() * 1000.0,
                        s.changed,
                        if s.cancelled { " · superseded" } else { "" }
                    )),
                    None => ui.label("—"),
                };
                ui.separator();
                ui.label(format!(
                    "last frame {:.2} ms",
                    self.frame_time.as_secs_f64() * 1000.0
                ));
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            PreviewTable::new(&self.engine.rows).show(ui);
        });

        self.frame_time = started.elapsed();
    }
}
