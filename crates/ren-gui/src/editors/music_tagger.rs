//! Music Tagger: the Quick Setup button, then Track / Title / Artist / Album /
//! Year / Genre / Comm, each an enable checkbox beside a tag-accepting box.
//!
//! Genre is the one exception — a dropdown over the standard genre list, with
//! no tag button. The list is what an ID3v1 tag can hold: lofty writes an
//! ID3v1 genre by looking the string up in the numeric table, and anything
//! not in it is written there as *absent*, silently. Quick Setup can still
//! point Genre at a part of the name (`<%3>`), and a preset can carry text of
//! its own; the dropdown then offers that value first, so opening it to look
//! does not lose it.
//!
//! Unticking a box clears the field, because that is what the operation stores
//! and what keeps a job file honest. The text is remembered in the ui's own
//! memory so re-ticking brings it back.

use ren_core::meta::write::{GENRES, MusicField};
use ren_core::ops::MusicTagger;
use ren_core::template::TextTemplate;

use super::EditorCx;
use crate::widgets::form::{After, Form};
use crate::widgets::tag_field;

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

    // The row's own memory is keyed off the card's id, and read through the
    // context because the form holds the `Ui` while its rows are drawn.
    let base = ui.id();
    let ctx = ui.ctx().clone();
    Form::new("music_tagger").show(ui, |form| {
        for (field, slot) in op.boxes() {
            let id = base.with(("tagger", field.label()));
            let dropdown = field == MusicField::Genre;
            let after = if dropdown {
                After::Nothing
            } else {
                After::TagPicker
            };
            let mut on = slot.is_some();
            // Drawn from the state the row started the frame in; the tick is
            // applied below, once the checkbox that is the row's label has
            // answered.
            let line = form.check(&mut on, field.label(), after, |row| {
                let width = row.field_width();
                match slot {
                    None => {
                        // Greyed rather than removed, so the row does not jump
                        // around as boxes are ticked — and with the picker the
                        // live row will have, so the box keeps its width too.
                        row.ui().add_enabled_ui(false, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut String::new()).desired_width(width),
                            );
                            if !dropdown {
                                let _ =
                                    tag_field::menu(ui, &format!("tagger_{}_off", field.label()));
                            }
                        });
                        false
                    }
                    Some(template) if dropdown => {
                        let mut picked = template.as_str().to_owned();
                        let own = (!picked.is_empty() && !GENRES.contains(&picked.as_str()))
                            .then(|| picked.clone());
                        egui::ComboBox::from_id_salt(id)
                            .selected_text(if picked.is_empty() { "—" } else { &picked })
                            .height(320.0)
                            .show_ui(row.ui(), |ui| {
                                // A value the list does not have — Setup
                                // Parts, or a preset's own text — is kept as
                                // the first choice rather than lost to the
                                // first click.
                                if let Some(own) = &own {
                                    ui.selectable_label(true, format!("{own} — keep"))
                                        .on_hover_text(
                                            "Not one of the listed genres. Written as it is \
                                             to the file's own tag; an ID3v1 tag, which only \
                                             holds the listed ones, leaves it out.",
                                        );
                                    ui.separator();
                                }
                                for genre in GENRES {
                                    if ui.selectable_label(picked == *genre, *genre).clicked() {
                                        picked = (*genre).to_owned();
                                    }
                                }
                            });
                        if picked != template.as_str() {
                            *template = TextTemplate::new(picked);
                            true
                        } else {
                            false
                        }
                    }
                    Some(template) => row.column(|ui| {
                        tag_field::tag_field(
                            ui,
                            &format!("tagger_{}", field.label()),
                            template,
                            width,
                        )
                    }),
                }
            });
            changed |= line.inner;
            if line
                .label
                .on_hover_text(
                    "Clear this and the field is not written — whatever the file already \
                     says is left intact.",
                )
                .changed()
            {
                *slot = if on {
                    // Whatever was in the box last time, so unticking is not a
                    // punishment for changing your mind.
                    let remembered: String = ctx.data(|d| d.get_temp(id).unwrap_or_default());
                    Some(TextTemplate::new(remembered))
                } else {
                    if let Some(had) = slot.as_ref() {
                        let text = had.as_str().to_owned();
                        ctx.data_mut(|d| d.insert_temp(id, text));
                    }
                    None
                };
                changed = true;
            }
        }
    });

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
            "Each file's own tag block is written — ID3v2 on MP3, Vorbis comments on FLAC, \
             Ogg, Opus and Speex, MP4 atoms on M4A, APEv2 on WavPack and Musepack. On an MP3 \
             an existing ID3v1 tag is refreshed too; one is never created, because it \
             truncates at 30 characters and cannot hold non-Latin text.",
        )
        .weak()
        .small(),
    );

    changed
}
