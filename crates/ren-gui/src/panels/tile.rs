//! One thumbnail, drawn.
//!
//! Shared by the Thumb column and the grid, so the two cannot disagree about
//! which size they asked for and which they looked up. That disagreement is the
//! bug this module exists to make impossible: a cache keyed on the path alone
//! hands a 96-pixel picture back for a 256-pixel request, and the only symptom
//! is a soft-looking tile — which no headless test can see. [`key_for`] is the
//! one place a key is built, and both the *request* and the *lookup* go through
//! it.

use ren_core::meta::thumb::{NoThumbnail, is_image};
use ren_core::model::FileEntry;

use crate::thumbs::{ThumbKey, Thumbs, Tile, bucket};

/// How a thumbnail is drawn: the two things Settings lets the user decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    /// The longest edge, in points.
    pub size: u32,
    /// > *"Draw a black border around thumbnails"*
    pub border: bool,
}

/// The thumbnail this entry wants, or `None` if it is not a picture.
///
/// The extension check comes first and costs nothing: a folder of ten thousand
/// text files must not queue ten thousand decodes that will each open a file,
/// fail to guess its format, and answer "not a picture".
///
/// `points_per_pixel` is why this takes the screen into account at all. A
/// 96-point tile on a 2× display is 192 physical pixels, and decoding 96 there
/// would show something visibly softer than the text beside it.
pub fn key_for(entry: &FileEntry, look: Look, points_per_pixel: f32) -> Option<ThumbKey> {
    if entry.is_dir || !is_image(&entry.path) {
        return None;
    }
    let pixels = (look.size as f32 * points_per_pixel).ceil() as u32;
    Some(ThumbKey::of(entry, bucket(pixels)))
}

/// Why there is no picture, in words a user can act on.
fn refusal(why: &NoThumbnail) -> &'static str {
    match why {
        NoThumbnail::NotAPicture => "not a picture",
        NoThumbnail::TooLarge => "too large to preview",
        NoThumbnail::Unreadable => "cannot be read",
        NoThumbnail::Panicked => "cannot be read",
    }
}

/// Draws the tile for one entry, and asks for it if it is not held yet.
///
/// The returned `Response` is the click target, so the caller decides what a
/// click means — the column and the grid both want selection, and the grid
/// wants a double-click as well.
///
/// **Never calls `request_repaint`.** The decode threads call the app's injected
/// repaint when a tile lands (D135); a view that asked for a frame while any
/// decode was outstanding would never settle, and `egui_kittest` panics rather
/// than warns when it cannot reach a still frame.
pub fn picture(
    ui: &mut egui::Ui,
    thumbs: &mut Thumbs,
    entry: &FileEntry,
    look: Look,
) -> egui::Response {
    let side = look.size as f32;
    let Some(key) = key_for(entry, look, ui.pixels_per_point()) else {
        // Not a picture, so nothing was ever asked for. A glyph rather than
        // blank space, so the column still reads as a column — and it names
        // nothing, because a row the filter hid must not be findable through
        // its tile (P72 works the other way round too).
        let icon = if entry.is_dir {
            crate::widgets::icons::Icon::Folder
        } else {
            crate::widgets::icons::Icon::File
        };
        return crate::widgets::icons::sized_icon(ui, icon, side, egui::Sense::hover());
    };

    let ctx = ui.ctx().clone();
    let name = entry.file_name.as_str();
    match thumbs.tile(&ctx, &key) {
        Some(Tile::Ready { texture, size }) => {
            let (width, height) = (size[0] as f32, size[1] as f32);
            let longest = width.max(height).max(1.0);
            // Never magnified: a 32-pixel icon drawn at 256 points is a blurry
            // rectangle, and `fit` deliberately does not upscale either.
            let natural = longest / ui.pixels_per_point();
            let scale = side.min(natural) / longest;
            let drawn = egui::vec2(width * scale, height * scale);

            let source = egui::load::SizedTexture::new(texture.id(), drawn);
            let image = egui::Image::new(source)
                .fit_to_exact_size(drawn)
                // The only way a texture reaches the accessibility tree — which
                // makes this both the screen-reader label and the handle every
                // headless test has on a tile. Accessibility and testability
                // have the same answer here (P73).
                .alt_text(format!("thumbnail of {name}"))
                .sense(egui::Sense::click());
            let response = ui.add_sized([side, side], image);
            if look.border {
                ui.painter().rect_stroke(
                    response.rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::BLACK),
                    egui::StrokeKind::Inside,
                );
            }
            response
        }
        Some(Tile::None(why)) => {
            let why = refusal(why);
            crate::widgets::icons::sized_icon(
                ui,
                crate::widgets::icons::Icon::NoPreview,
                side,
                egui::Sense::click(),
            )
            .on_hover_text(format!("{name} {why}"))
        }
        None => {
            // Still decoding. A static placeholder — no spinner, no fade: an
            // animated widget repaints continuously and a continuously
            // repainting frame never settles (D26).
            ui.add_sized(
                [side, side],
                egui::Label::new(egui::RichText::new("…").weak())
                    .sense(egui::Sense::click())
                    .selectable(false),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> FileEntry {
        FileEntry::synthetic(std::path::PathBuf::from("/pictures").join(name))
    }

    fn look(size: u32) -> Look {
        Look {
            size,
            border: false,
        }
    }

    /// A folder of ten thousand text files must not queue ten thousand decodes
    /// that each open a file only to answer "not a picture".
    #[test]
    fn only_a_picture_is_ever_asked_for() {
        assert!(key_for(&entry("holiday.jpg"), look(96), 1.0).is_some());
        assert!(key_for(&entry("notes.txt"), look(96), 1.0).is_none());
        assert!(key_for(&entry("README"), look(96), 1.0).is_none());

        let mut folder = entry("Pictures");
        folder.is_dir = true;
        assert!(key_for(&folder, look(96), 1.0).is_none());
    }

    /// The request and the lookup must name the same size, and on a 2× display
    /// that size is twice the tile. Both callers go through `key_for`, so the
    /// only way they can disagree is if this arithmetic is wrong.
    #[test]
    fn a_tile_on_a_sharper_screen_asks_for_a_sharper_picture() {
        let at_1x = key_for(&entry("a.jpg"), look(96), 1.0).unwrap();
        let at_2x = key_for(&entry("a.jpg"), look(96), 2.0).unwrap();
        assert_eq!(at_1x.edge, 96);
        assert_eq!(at_2x.edge, 256, "192 pixels rounds up to the bucket above");
        assert_ne!(at_1x, at_2x, "and they are different cache entries");
    }

    /// Every refusal says something, or the tile is a blank square with no
    /// explanation and the user is left guessing.
    #[test]
    fn every_refusal_has_words() {
        for why in [
            NoThumbnail::NotAPicture,
            NoThumbnail::TooLarge,
            NoThumbnail::Unreadable,
            NoThumbnail::Panicked,
        ] {
            assert!(!refusal(&why).is_empty(), "{why:?}");
        }
    }
}
