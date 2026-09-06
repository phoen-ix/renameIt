//! Music Rename: the styles as radio buttons, an *(edit styles)* link above
//! them, a Custom row with its own tag button, and a note about which files can
//! be read at all.
//!
//! Two choices worth stating, both in the same direction — the pattern is never
//! hidden from the person choosing it:
//!
//! * **The Custom box is always live**, showing the selected style and letting
//!   you edit it. That turns "pick the nearest style, then adjust it" from a
//!   copy-retype-and-hope into one keystroke.
//! * **Which radio is lit is derived from the pattern**, because the pattern is
//!   the only thing the operation stores (D35 — a preset that said *"style 2"*
//!   would mean something else on a machine whose styles had been edited). The
//!   one thing that cannot be derived is which of two identical-looking states
//!   the user meant when the text happens to match a style exactly, and that is
//!   the only thing kept in the ui's own memory.

use ren_core::ops::MusicRename;
use ren_core::template::TextTemplate;

use super::EditorCx;
use crate::widgets::form::{After, Form};
use crate::widgets::tag_field::tag_field;

pub fn ui(ui: &mut egui::Ui, op: &mut MusicRename, cx: &EditorCx<'_>) -> bool {
    let mut changed = false;

    // Pure presentation: it decides which radio is lit when the text matches a
    // style exactly, and nothing else. `insert_temp`, so it is not persisted —
    // reopening the app on a pattern that matches a style should light that
    // style, which is the truth about what the card will do.
    let custom_id = ui.id().with("music_custom");
    let mut sticky_custom = ui.data(|d| d.get_temp::<bool>(custom_id).unwrap_or(false));

    let matched = cx
        .music_styles
        .iter()
        .position(|style| style == op.style.as_str());
    let custom = sticky_custom || matched.is_none();

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Rename music based on tags").strong());
        if ui
            .link("(edit styles)")
            .on_hover_text("Settings ▸ Music Styles — the list these radios offer")
            .clicked()
        {
            cx.requests.edit_music_styles.set(true);
        }
    });
    ui.add_space(2.0);

    for (index, style) in cx.music_styles.iter().enumerate() {
        let lit = !custom && matched == Some(index);
        if ui.radio(lit, style).clicked() {
            op.style = TextTemplate::new(style.clone());
            sticky_custom = false;
            changed = true;
        }
    }
    // An empty list is reachable: Settings lets you delete the last one.
    if cx.music_styles.is_empty() {
        ui.label(
            egui::RichText::new("No styles are set up — type one below, or add some in Settings.")
                .weak()
                .small(),
        );
    }

    Form::new("music_custom").show(ui, |form| {
        let line = form.radio(custom, "Custom:", After::TagPicker, |row| {
            let width = row.field_width();
            // Deliberately outside an `add_enabled_ui`: the box stays live.
            row.column(|ui| tag_field(ui, "music_style", &mut op.style, width))
        });
        if line
            .label
            .on_hover_text("Anything you like, in the box beside it")
            .clicked()
        {
            sticky_custom = true;
        }
        if line.inner {
            // Typing is what makes it custom, and it stays custom even if the
            // text lands on a style exactly — otherwise a radio would light up
            // under the cursor mid-word.
            sticky_custom = true;
            changed = true;
        }
    });

    ui.data_mut(|d| d.insert_temp(custom_id, sticky_custom));

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Reads MP3, FLAC, Ogg Vorbis, Opus, Speex, MP4/M4A, Musepack, WavPack and APE — \
             ID3v1, ID3v2, Vorbis comments, MP4 atoms and APEv2 alike. A folder takes its \
             name from the first track inside it.",
        )
        .weak()
        .small(),
    );
    ui.label(
        egui::RichText::new(
            "A file with none of these tags is left alone rather than renamed to the \
             pattern's punctuation. Run Settings ▸ \"Only rename if all tags are available\" \
             does the same for a file that is missing just one.",
        )
        .weak()
        .small(),
    );

    changed
}
