//! Set Attributes.
//!
//! A 2×2 grid of tri-state boxes, laid out as the dialog has
//! them: Write Protect and System on the first row, Hidden and Archive on the
//! second.

use ren_core::ops::SetAttributes;
use ren_platform::{Capability, Platform};

use crate::widgets::tri_checkbox::tri_checkbox;

/// Which capability each box needs, in the order `SetAttributes::bits` yields.
const NEEDS: [Capability; 4] = [
    Capability::ReadOnlyAttribute,
    Capability::HiddenAttribute,
    Capability::SystemAttribute,
    Capability::ArchiveAttribute,
];

pub fn ui(ui: &mut egui::Ui, op: &mut SetAttributes, platform: &dyn Platform) -> bool {
    let mut changed = false;

    // Every bit is shown on every platform (D40). A preset built on Windows has
    // to stay legible on Linux, and the limit is far better learned here than
    // discovered when the run is blocked.
    let mut unsupported: Vec<&'static str> = Vec::new();
    egui::Grid::new("attributes")
        .num_columns(2)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            // Column-major, because that is how the dialog reads: Write
            // Protect | System on the first row, Hidden | Archive on the
            // second. `op.bits()` is in bit order, and egui fills a Grid
            // row-major, so walking it straight put Write Protect beside
            // Hidden — the transpose of the layout this file cites.
            const ROWS: [[usize; 2]; 2] = [[0, 2], [1, 3]];
            let mut bits = op.bits();
            for row in ROWS {
                for index in row {
                    let (label, _) = bits[index];
                    let supported = platform.supports(NEEDS[index]);
                    if !supported && !unsupported.contains(&label) {
                        unsupported.push(label);
                    }
                    let bit = &mut bits[index].1;
                    changed |= ui
                        .add_enabled_ui(supported, |ui| tri_checkbox(ui, bit, label))
                        .inner;
                }
                ui.end_row();
            }
        });

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Grey keeps the current value. Click to cycle keep → clear → set.")
            .weak()
            .small(),
    );

    if !unsupported.is_empty() {
        ui.label(
            egui::RichText::new(format!(
                "{} {} Windows-only — this is {}. They are shown so a preset built on \
                 Windows still reads here, and a run that asks for one is refused with \
                 that reason rather than half-finishing.",
                unsupported.join(", "),
                if unsupported.len() == 1 { "is" } else { "are" },
                platform.name(),
            ))
            .weak()
            .small(),
        );
    }

    if op.is_empty() {
        ui.label(
            egui::RichText::new("All four are grey, so this operation leaves every file alone.")
                .weak()
                .small(),
        );
    }

    changed
}
