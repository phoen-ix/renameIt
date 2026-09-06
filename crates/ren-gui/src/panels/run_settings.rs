//! Counter Setup and Setup Parts — the two run-wide objects.
//!
//! They belong to the *run*, not to any one operation, which is why they sit
//! below the operation editor rather than inside it.

use ren_core::model::FileEntry;
use ren_core::{CounterSetup, PartsSpec, RunSettings};

/// Draws the two pinned chips and the tag policy. Returns true if anything
/// changed.
///
/// Counter and Parts appear as two pinned chips at the top of the panel —
/// "Counter: 1, step 1, pad auto" and "Parts: `<%1> - <%2>`" — each opening its
/// own dialog. They are run-wide, not per-card, which is why they sit above the
/// stack rather than inside any of it.
pub fn ui(ui: &mut egui::Ui, settings: &mut RunSettings, sample: Option<&FileEntry>) -> bool {
    let mut changed = false;

    ui.horizontal_wrapped(|ui| {
        let chip = ui
            .selectable_label(
                false,
                format!("Counter setup: {}", counter_chip(&settings.counter)),
            )
            .on_hover_text("Start, step, padding and the resets");
        egui::Popup::from_toggle_button_response(&chip).show(|ui| {
            ui.set_min_width(300.0);
            changed |= counter_ui(ui, &mut settings.counter);
        });

        let chip = ui
            .selectable_label(
                false,
                format!("Setup parts: {}", parts_chip(&settings.parts)),
            )
            .on_hover_text("How your filenames are put together, for <%1> … <%9>");
        egui::Popup::from_toggle_button_response(&chip).show(|ui| {
            ui.set_min_width(320.0);
            changed |= parts_ui(ui, &mut settings.parts, sample);
        });
    });

    changed |= ui
        .checkbox(
            &mut settings.require_all_tags,
            "Only rename if all tags are available",
        )
        .on_hover_text(
            "A file whose tags cannot all be filled in is left alone rather than \
             renamed with the gaps left empty.",
        )
        .changed();

    changed
}

fn counter_ui(ui: &mut egui::Ui, counter: &mut CounterSetup) -> bool {
    let mut changed = false;

    egui::Grid::new("counter_grid")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Start value:");
            changed |= crate::widgets::number::add(
                ui,
                egui::DragValue::new(&mut counter.start).speed(0.2),
            )
            .on_hover_text("Negative values are allowed.")
            .changed();
            ui.end_row();

            ui.label("Step:");
            changed |=
                crate::widgets::number::add(ui, egui::DragValue::new(&mut counter.step).speed(0.2))
                    .on_hover_text("The counter will increase or decrease by this for each file.")
                    .changed();
            ui.end_row();

            ui.label("Zero pad:");
            ui.horizontal(|ui| {
                changed |= crate::widgets::number::add_enabled(
                    ui,
                    !counter.auto_pad,
                    egui::DragValue::new(&mut counter.pad)
                        .range(0..=12)
                        .speed(0.2),
                )
                .changed();
                changed |= ui
                    .checkbox(&mut counter.auto_pad, "Auto")
                    .on_hover_text(
                        "Automatically pad the count number with zeros so they all end \
                         up the same length.",
                    )
                    .changed();
            });
            ui.end_row();
        });

    changed |= ui
        .checkbox(&mut counter.reset_each_folder, "Reset each folder")
        .on_hover_text(
            "Resets the counter to the start value if the folder of the renamed file \
             differs from the previous file's.",
        )
        .changed();
    changed |= ui
        .checkbox(&mut counter.reset_on_base_name, "Reset on base name change")
        .on_hover_text("The base filename is the filename without any numbers.")
        .changed();

    ui.horizontal(|ui| {
        let mut on = counter.reset_at.is_some();
        if ui.checkbox(&mut on, "Reset at:").changed() {
            counter.reset_at = on.then_some(10);
            changed = true;
        }
        if let Some(limit) = counter.reset_at.as_mut() {
            changed |= crate::widgets::number::add(ui, egui::DragValue::new(limit).speed(0.2))
                .on_hover_text("That value will be the last number in the sequence.")
                .changed();
        }
    });

    changed |= ui
        .checkbox(&mut counter.running, "Running counter")
        .on_hover_text(
            "After a rename, the start value becomes the number that would have been \
             next in line.",
        )
        .changed();

    // "Dialog shows a live Example preview."
    ui.add_space(4.0);
    ui.label(egui::RichText::new(format!("Example: {}", example(counter))).weak());

    changed
}

/// The chip's own one-line summary, as DESIGN spells it out.
fn counter_chip(counter: &CounterSetup) -> String {
    let padding = if counter.auto_pad {
        "pad auto".to_owned()
    } else if counter.pad == 0 {
        "no padding".to_owned()
    } else {
        format!("pad {}", counter.pad)
    };
    format!("{}, step {}, {padding}", counter.start, counter.step)
}

fn parts_chip(parts: &PartsSpec) -> String {
    if parts.is_empty() {
        "none set".to_owned()
    } else {
        parts.pattern.clone()
    }
}

/// The first few values the current setup would produce.
fn example(counter: &CounterSetup) -> String {
    let entries: Vec<FileEntry> = (0..3)
        .map(|i| FileEntry::synthetic(format!("/example/file {i}.txt")))
        .collect();
    let values = counter.sequence(&entries);
    let width = counter.width_for(&values);
    values
        .iter()
        .map(|v| ren_core::counter::pad(*v, width))
        .collect::<Vec<_>>()
        .join(", ")
        + ", …"
}

fn parts_ui(ui: &mut egui::Ui, parts: &mut PartsSpec, sample: Option<&FileEntry>) -> bool {
    let mut changed = false;

    ui.label(
        egui::RichText::new(
            "Separate the parts of your filenames with the characters they have in \
             common, then use <%1> … <%9> as tags.",
        )
        .weak()
        .small(),
    );

    ui.horizontal(|ui| {
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut parts.pattern)
                    .desired_width(200.0)
                    .hint_text("<%1>. <%2> (<%3>) <%4>")
                    .id_salt("parts_pattern"),
            )
            .changed();

        // "Click on the magic wand button to automatically detect the parts of
        // the filename that is selected in the preview box!"
        let wand = ui.add_enabled(sample.is_some(), egui::Button::new("✨"));
        if wand
            .on_hover_text("Detect the parts of the selected filename")
            .clicked()
            && let Some(entry) = sample
        {
            *parts = PartsSpec::detect(entry.split().0);
            changed = true;
        }
    });

    if let Some(entry) = sample
        && !parts.is_empty()
    {
        let split = parts.split(entry.split().0);
        let mut summary = String::new();
        for slot in 1..=9u8 {
            if let Some(value) = split.get(slot) {
                if !summary.is_empty() {
                    summary.push_str(" · ");
                }
                summary.push_str(&format!("<%{slot}> {value}"));
            }
        }
        if summary.is_empty() {
            summary.push_str("no parts matched the selected file");
        }
        ui.label(egui::RichText::new(summary).weak().small());
    }

    changed
}
