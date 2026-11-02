//! Counter setup, Setup Parts and the tag policy — the run-wide settings,
//! drawn inside the cards that use them.
//!
//! They belong to the *run*, not to any one operation: one counter numbers the
//! files for every card, and `<%1>` means the same part everywhere. D251 put
//! all three behind a *Run settings* button under the stack, where they read as
//! settings for nothing in particular and sat there with a pipeline that used
//! none of them. D254 draws each one in the expanded card that depends on it,
//! saying it is shared — the counter in Add Counter, the parts in Music Tagger,
//! the tag rule in Music Rename, and each in any card whose tags need it.

use ren_core::model::FileEntry;
use ren_core::ops::OpKind;
use ren_core::template::TagNeeds;
use ren_core::{CounterSetup, PartsSpec, RunSettings};

/// The sections `op` depends on, if any. Returns true if anything changed.
pub fn for_card(
    ui: &mut egui::Ui,
    op: &OpKind,
    run: &mut RunSettings,
    sample: Option<&FileEntry>,
) -> bool {
    let needs = op.needs();
    let mut changed = false;

    if matches!(op, OpKind::AddCounter(_)) || needs.contains(TagNeeds::COUNTER) {
        heading(ui, "Counter", "Start, step, padding and the resets");
        changed |= counter_ui(ui, &mut run.counter);
    }
    if matches!(op, OpKind::MusicTagger(_)) || needs.contains(TagNeeds::PARTS) {
        heading(
            ui,
            "Filename parts",
            "How your filenames are put together, for <%1> … <%9>",
        );
        changed |= parts_ui(ui, &mut run.parts, sample);
    }
    if matches!(op, OpKind::MusicRename(_))
        || needs.contains(TagNeeds::FILE_CONTENT)
        || needs.contains(TagNeeds::PARTS)
    {
        ui.separator();
        changed |= ui
            .checkbox(
                &mut run.require_all_tags,
                "Only rename if all tags are available",
            )
            .on_hover_text(
                "A file whose tags cannot all be filled in is left alone rather than \
                 renamed with the gaps left empty. Shared by every card.",
            )
            .changed();
    }
    changed
}

/// A section's title, and the note that it is not this card's alone.
fn heading(ui: &mut egui::Ui, title: &str, hover: &str) {
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong())
            .on_hover_text(hover);
        ui.label(egui::RichText::new("shared by every card").weak().small());
    });
}

fn counter_ui(ui: &mut egui::Ui, counter: &mut CounterSetup) -> bool {
    let mut changed = false;

    egui::Grid::new("counter_grid")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            let label = ui.label("Start value:");
            changed |= crate::widgets::number::add(
                ui,
                egui::DragValue::new(&mut counter.start).speed(0.2),
            )
            .labelled_by(label.id)
            .on_hover_text("Negative values are allowed.")
            .changed();
            ui.end_row();

            let label = ui.label("Step:");
            changed |=
                crate::widgets::number::add(ui, egui::DragValue::new(&mut counter.step).speed(0.2))
                    .labelled_by(label.id)
                    .on_hover_text("The counter will increase or decrease by this for each file.")
                    .changed();
            ui.end_row();

            let label = ui.label("Zero pad:");
            ui.horizontal(|ui| {
                changed |= crate::widgets::number::add_enabled(
                    ui,
                    !counter.auto_pad,
                    egui::DragValue::new(&mut counter.pad)
                        .range(0..=12)
                        .speed(0.2),
                )
                .labelled_by(label.id)
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
        let check = ui.checkbox(&mut on, "Reset at:");
        if check.changed() {
            counter.reset_at = on.then_some(10);
            changed = true;
        }
        if let Some(limit) = counter.reset_at.as_mut() {
            changed |= crate::widgets::number::add(ui, egui::DragValue::new(limit).speed(0.2))
                .labelled_by(check.id)
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
