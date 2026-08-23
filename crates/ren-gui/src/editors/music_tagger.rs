//! Music Tagger: the Quick Setup button, then Track / Title / Artist / Album /
//! Year / Genre / Comm, each an enable checkbox beside a tag-accepting box.
//!
//! Genre is the one exception — a plain dropdown with no tag button. It is
//! restricted for a reason: lofty writes an ID3v1 genre by looking the string
//! up in the numeric table, and anything not in it is written as *absent*,
//! silently.
//!
//! Unticking a box clears the field, because that is what the operation stores
//! and what keeps a job file honest. The text is remembered in the ui's own
//! memory so re-ticking brings it back.

use ren_core::meta::write::{GENRES, MusicField};
use ren_core::ops::MusicTagger;
use ren_core::template::TextTemplate;

use super::EditorCx;
use crate::widgets::tag_field::tag_field;

pub fn ui(ui: &mut egui::Ui, op: &mut MusicTagger, cx: &EditorCx<'_>) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.menu_button("✨ Quick Setup", |ui| {
            ui.label(
                egui::RichText::new("My filenames look like…")
                    .strong()
                    .small(),
            );
            for style in cx.music_styles {
                // A style naming something that is not a writable field — say
                // `<Bitrate>` — cannot be turned into field mappings, so it is
                // offered greyed rather than silently doing nothing.
                let usable = MusicTagger::from_style(style).is_some();
                let response = ui.add_enabled(usable, egui::Button::new(style));
                if !usable {
                    response.on_hover_text("This style names something the tagger cannot write.");
                } else if response.clicked() {
                    if let Some((parts, built)) = MusicTagger::from_style(style) {
                        *op = built;
                        *cx.requests.set_parts.borrow_mut() = Some(parts);
                        changed = true;
                    }
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Edit Styles…").clicked() {
                cx.requests.edit_music_styles.set(true);
                ui.close();
            }
        })
        .response
        .on_hover_text(
            "Pick the shape your filenames already have. It sets Setup Parts and points \
             each field at the right part.",
        );

        ui.label(
            egui::RichText::new("sets Setup Parts and the fields together")
                .weak()
                .small(),
        );
    });

    ui.add_space(4.0);

    for (field, slot) in op.boxes() {
        ui.horizontal(|ui| {
            let mut on = slot.is_some();
            let id = ui.id().with(("tagger", field.label()));
            if ui
                .checkbox(&mut on, field.label())
                .on_hover_text(
                    "Clear this and the field is not written — whatever the file already \
                     says is left intact.",
                )
                .changed()
            {
                *slot = if on {
                    // Whatever was in the box last time, so unticking is not a
                    // punishment for changing your mind.
                    let remembered: String = ui.data(|d| d.get_temp(id).unwrap_or_default());
                    Some(TextTemplate::new(remembered))
                } else {
                    if let Some(had) = slot.as_ref() {
                        let text = had.as_str().to_owned();
                        ui.data_mut(|d| d.insert_temp(id, text));
                    }
                    None
                };
                changed = true;
            }

            match slot {
                None => {
                    // Greyed rather than removed, so the row does not jump
                    // around as boxes are ticked.
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut String::new()).desired_width(200.0),
                    );
                }
                Some(template) if field == MusicField::Genre => {
                    let mut picked = template.as_str().to_owned();
                    egui::ComboBox::from_id_salt(id)
                        .selected_text(if picked.is_empty() { "—" } else { &picked })
                        .height(320.0)
                        .show_ui(ui, |ui| {
                            for genre in GENRES {
                                if ui.selectable_label(picked == *genre, *genre).clicked() {
                                    picked = (*genre).to_owned();
                                }
                            }
                        });
                    if picked != template.as_str() {
                        *template = TextTemplate::new(picked);
                        changed = true;
                    }
                }
                Some(template) => {
                    changed |= tag_field(ui, &format!("tagger_{}", field.label()), template, 200.0);
                }
            }
        });
    }

    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("⚠ Writing tags changes the file and cannot be undone.")
            .color(ui.visuals().warn_fg_color)
            .small(),
    );
    ui.label(
        egui::RichText::new(
            "Fields you have not ticked are left exactly as they are, and so is anything \
             else in the tag — cover art included. A track number that is not a plain \
             number is skipped rather than written as 0.",
        )
        .weak()
        .small(),
    );
    ui.label(
        egui::RichText::new(
            "ID3v2 is written on every file. An ID3v1 tag is refreshed only where one \
             already exists — it truncates at 30 characters and cannot hold non-Latin \
             text, so it is never created for you.",
        )
        .weak()
        .small(),
    );

    changed
}
