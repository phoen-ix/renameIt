//! The marks no bundled font has, drawn instead of typed.
//!
//! The app's icon vocabulary used to be literal Unicode in `&str` labels, and
//! egui's `Proportional` family resolves Ubuntu-Light → NotoEmoji →
//! emoji-icon-font. Thirteen code points the UI relied on were in none of them
//! and shipped as `◻` — epaint's replacement character — including the
//! operation card's drag handle and overflow menu, the About button, the file
//! table's sort indicator, and four of the eight controls on every Batch
//! Replace row.
//!
//! Seven of the thirteen are in `Hack`, which epaint already bundles for the
//! `Monospace` family; [`crate::theme::install_fonts`] names it as a
//! `Proportional` fallback and they cost nothing further. That is the whole
//! fix for the ones that appear *inside a sentence* — `Settings ▸ File System`
//! is prose, and prose does not want an icon.
//!
//! The other six are in no bundled font at all:
//!
//! | was | is |
//! | --- | --- |
//! | `⠿` U+283F | [`Icon::Grip`] |
//! | `⋮` U+22EE | [`Icon::Kebab`] |
//! | `ⓘ` U+24D8 | [`Icon::Info`] |
//! | `🗎` U+1F5CE | [`Icon::File`] |
//! | `✎` U+270E | `∆`, which Ubuntu-Light has — a log line is prose |
//! | `↩` U+21A9 | `↺`, likewise |
//!
//! Four more are painted despite rendering: [`Icon::Chevron`],
//! [`Icon::CaretUp`], [`Icon::CaretDown`] and [`Icon::NoPreview`] are
//! icon-only *controls*, and a fallback glyph at emoji scale beside a drawn one
//! looks like a mistake. [`Icon::Gear`] and [`Icon::Folder`] are there for the
//! same reason.
//!
//! Painting rather than adding an icon font is what keeps **D2** quiet (no new
//! dependency to licence) and **D14**'s binary budget flat, and a vector mark is
//! the one kind that is still sharp at the 125 % and 150 % scaling
//! `docs/manual-checks.md` asks to be eyeballed.
//!
//! **Every icon-only control carries its own name.** For a button whose label
//! *was* the glyph, the glyph was also what AccessKit and the `egui_kittest`
//! harness saw — so the tofu was in the accessibility tree too, and D24 drives
//! the GUI through nothing else. [`icon_button`] takes the name as an argument
//! and there is no way to call it without one.

use egui::{
    Color32, Painter, Pos2, Rect, Response, Sense, Ui, Vec2, Widget, WidgetInfo, WidgetType,
};

/// The side of the mark itself, before button padding.
pub(crate) const MARK: f32 = 14.0;

/// A mark with no glyph behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Drag handle — two columns of three dots. Was `⠿`.
    Grip,
    /// Overflow menu — one column of three dots. Was `⋮`.
    Kebab,
    /// About — a circled *i*. Was `ⓘ`.
    Info,
    /// A file that is not a picture. Was `🗎`.
    File,
    /// A picture that could not be decoded. Was `⊘`, which `Hack` does have —
    /// kept here because it sits beside [`Icon::File`] in the same tile and two
    /// marks from two different sources at two different weights read as a bug.
    NoPreview,
    /// A folder, beside [`Icon::File`] in the same column. Was `📁`, which
    /// NotoEmoji does have — but at epaint's 0.81 fallback scale, next to a
    /// painted mark, one of the two always looks wrong.
    Folder,
    /// This control opens something below it. Was `▾`.
    Chevron,
    /// Move up. Was `▲`.
    CaretUp,
    /// Move down. Was `▼`.
    CaretDown,
    /// Settings. Was `⚙`, which emoji-icon-font does have — painted for the
    /// same reason as [`Icon::Folder`]: it sits next to [`Icon::Info`], and a
    /// fallback glyph at 0.90 scale beside a drawn one looks like a mistake.
    Gear,
}

impl Icon {
    /// Draw into `rect`, which the caller has already made square.
    pub fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        // Everything is expressed in a 0..1 unit square and scaled, so the mark
        // is resolution-independent and a DPI change is a bigger rect rather
        // than a different drawing.
        let at = |x: f32, y: f32| Pos2 {
            x: rect.min.x + x * rect.width(),
            y: rect.min.y + y * rect.height(),
        };
        let unit = rect.width();

        match self {
            Self::Grip => {
                let r = unit * 0.075;
                for x in [0.32, 0.68] {
                    for y in [0.22, 0.5, 0.78] {
                        painter.circle_filled(at(x, y), r, color);
                    }
                }
            }
            Self::Kebab => {
                let r = unit * 0.085;
                for y in [0.2, 0.5, 0.8] {
                    painter.circle_filled(at(0.5, y), r, color);
                }
            }
            Self::Info => {
                let stroke = egui::Stroke::new((unit * 0.09).max(1.0), color);
                painter.circle_stroke(rect.center(), unit * 0.42, stroke);
                painter.circle_filled(at(0.5, 0.28), unit * 0.075, color);
                painter.line_segment([at(0.5, 0.44), at(0.5, 0.74)], stroke);
            }
            Self::File => {
                let stroke = egui::Stroke::new((unit * 0.085).max(1.0), color);
                // A page with the top-right corner folded away, drawn as one
                // open path so the fold and the outline cannot drift apart.
                painter.add(egui::Shape::line(
                    vec![
                        at(0.62, 0.1),
                        at(0.22, 0.1),
                        at(0.22, 0.9),
                        at(0.78, 0.9),
                        at(0.78, 0.26),
                        at(0.62, 0.1),
                        at(0.62, 0.26),
                        at(0.78, 0.26),
                    ],
                    stroke,
                ));
            }
            Self::Folder => {
                let stroke = egui::Stroke::new((unit * 0.085).max(1.0), color);
                painter.add(egui::Shape::line(
                    vec![
                        at(0.1, 0.82),
                        at(0.1, 0.2),
                        at(0.4, 0.2),
                        at(0.5, 0.32),
                        at(0.9, 0.32),
                        at(0.9, 0.82),
                        at(0.1, 0.82),
                    ],
                    stroke,
                ));
            }
            Self::Gear => {
                let stroke = egui::Stroke::new((unit * 0.085).max(1.0), color);
                let centre = rect.center();
                painter.circle_stroke(centre, unit * 0.20, stroke);
                // Eight teeth, as radial spokes between two radii — a cog read
                // at 14 pt is "a circle with regular bumps", and spokes survive
                // the size where a toothed outline turns to mush.
                for i in 0..8 {
                    let angle = std::f32::consts::TAU * (i as f32) / 8.0;
                    let (sin, cos) = angle.sin_cos();
                    let dir = egui::vec2(cos, sin);
                    painter.line_segment(
                        [centre + dir * unit * 0.28, centre + dir * unit * 0.44],
                        stroke,
                    );
                }
            }
            Self::Chevron => caret(painter, at, color, unit, 1.0),
            Self::CaretUp => caret(painter, at, color, unit, -1.0),
            Self::CaretDown => caret(painter, at, color, unit, 1.0),
            Self::NoPreview => {
                let stroke = egui::Stroke::new((unit * 0.09).max(1.0), color);
                painter.circle_stroke(rect.center(), unit * 0.4, stroke);
                let d = unit * 0.4 * std::f32::consts::FRAC_1_SQRT_2;
                let c = rect.center();
                painter.line_segment(
                    [Pos2::new(c.x - d, c.y + d), Pos2::new(c.x + d, c.y - d)],
                    stroke,
                );
            }
        }
    }
}

/// The one shape three icons share, pointing `down` (+1) or up (-1).
fn caret(painter: &Painter, at: impl Fn(f32, f32) -> Pos2, color: Color32, unit: f32, down: f32) {
    let stroke = egui::Stroke::new((unit * 0.11).max(1.25), color);
    let (near, far) = if down > 0.0 {
        (0.34, 0.66)
    } else {
        (0.66, 0.34)
    };
    painter.add(egui::Shape::line(
        vec![at(0.22, near), at(0.5, far), at(0.78, near)],
        stroke,
    ));
}

/// A square button carrying nothing but [`Icon`], and a name for everything
/// that cannot see it.
///
/// `name` is the accessible name, not a tooltip: egui only puts a tooltip into
/// the accessibility tree while it is hovered (P89), so a tooltip is invisible
/// to a screen reader and to the headless harness alike. Add
/// [`Response::on_hover_text`] as well when there is more to say than the name.
#[must_use = "an icon button is only useful if you check whether it was clicked"]
pub struct IconButton<'a> {
    icon: Icon,
    name: &'a str,
    frame: bool,
}

impl<'a> IconButton<'a> {
    pub fn new(icon: Icon, name: &'a str) -> Self {
        Self {
            icon,
            name,
            frame: true,
        }
    }

    /// Drop the button frame — for a mark that sits inside another control's
    /// chrome, like the card's drag handle.
    pub fn frameless(mut self) -> Self {
        self.frame = false;
        self
    }
}

impl Widget for IconButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let padding = if self.frame {
            ui.spacing().button_padding
        } else {
            Vec2::splat(2.0)
        };
        let size = Vec2::splat(MARK) + 2.0 * padding;
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());

        if ui.is_rect_visible(rect) {
            let visuals = *ui.style().interact(&response);
            if self.frame {
                ui.painter().rect(
                    rect,
                    visuals.corner_radius,
                    visuals.weak_bg_fill,
                    visuals.bg_stroke,
                    egui::StrokeKind::Inside,
                );
            }
            let mark = Rect::from_center_size(rect.center(), Vec2::splat(MARK));
            self.icon.paint(ui.painter(), mark, visuals.fg_stroke.color);
        }

        let enabled = ui.is_enabled();
        let name = self.name;
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, name.to_owned()));
        response
    }
}

/// The common case: an icon button with a frame.
pub fn icon_button(ui: &mut Ui, icon: Icon, name: &str) -> Response {
    ui.add(IconButton::new(icon, name))
}

/// A mark at a caller-chosen size, carrying no name at all.
///
/// For the thumbnail column, where the tile stands in for a file the app has
/// deliberately not named: P72 keeps a filename off every surface outside a
/// tile, and a hidden row must not become findable through the glyph that
/// replaced its picture. An anonymous painted mark is a better answer than the
/// old `Label::new("🗎")`, whose accessible name was the tofu itself.
pub fn sized_icon(ui: &mut Ui, icon: Icon, side: f32, sense: Sense) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), sense);
    if ui.is_rect_visible(rect) {
        // Marks are drawn for a ~14pt box; a 96pt tile wants a mark that reads
        // as an emblem rather than a hairline, but not one that fills the tile.
        let mark = (side * 0.45).clamp(MARK, 64.0);
        icon.paint(
            ui.painter(),
            Rect::from_center_size(rect.center(), Vec2::splat(mark)),
            ui.visuals().weak_text_color(),
        );
    }
    response
}

/// The mark alone, at whatever size the caller has already reserved.
///
/// For a tile, where the icon is the content rather than a decoration.
pub fn paint_into(painter: &Painter, icon: Icon, rect: Rect, color: Color32) {
    let side = rect.width().min(rect.height());
    icon.paint(
        painter,
        Rect::from_center_size(rect.center(), Vec2::splat(side)),
        color,
    );
}

/// How wide an [`IconButton`] comes out, so a caller laying out a row by hand
/// does not have to guess.
pub fn button_size(ui: &Ui) -> Vec2 {
    Vec2::splat(MARK) + 2.0 * ui.spacing().button_padding
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 10] = [
        Icon::Grip,
        Icon::Kebab,
        Icon::Info,
        Icon::File,
        Icon::NoPreview,
        Icon::Folder,
        Icon::Chevron,
        Icon::CaretUp,
        Icon::CaretDown,
        Icon::Gear,
    ];

    /// Paint one icon into a rect at `origin` and hand back what it drew.
    fn shapes_at(icon: Icon, origin: Pos2) -> Vec<egui::Shape> {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                icon.paint(
                    ui.painter(),
                    Rect::from_min_size(origin, Vec2::splat(MARK)),
                    Color32::WHITE,
                );
            });
        });
        // The panel paints its own background first; the icon's shapes are the
        // ones this test added, so drop anything that is not a stroke or a
        // circle we could have produced.
        let mut output = output;
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .map(|clipped| clipped.shape)
            .filter(|shape| {
                let bounds = shape.visual_bounding_rect();
                bounds.is_finite() && bounds.width() <= MARK * 2.0
            })
            .collect()
    }

    /// Painting must be relative to the rect, or an icon in a scrolled list
    /// draws somewhere else entirely.
    #[test]
    fn a_mark_is_drawn_relative_to_its_rect() {
        let offset = Vec2::new(320.0, 640.0);
        for icon in ALL {
            let near = shapes_at(icon, Pos2::new(20.0, 20.0));
            let far = shapes_at(icon, Pos2::new(20.0, 20.0) + offset);
            assert!(!near.is_empty(), "{icon:?} drew nothing");
            assert_eq!(
                near.len(),
                far.len(),
                "{icon:?} drew a different number of shapes at a different origin"
            );
            for (a, b) in near.iter().zip(far.iter()) {
                let moved = a.visual_bounding_rect().translate(offset);
                let actual = b.visual_bounding_rect();
                assert!(
                    (moved.min - actual.min).length() < 0.01
                        && (moved.max - actual.max).length() < 0.01,
                    "{icon:?} does not translate with its rect: {moved:?} vs {actual:?}"
                );
            }
        }
    }

    /// Every mark stays inside the box it was given — a grip that bleeds into
    /// the checkbox beside it is a click target that lies.
    #[test]
    fn a_mark_stays_inside_its_rect() {
        let origin = Pos2::new(20.0, 20.0);
        // Half a stroke width of overhang is the stroke itself, not a bug.
        let room = Rect::from_min_size(origin, Vec2::splat(MARK)).expand(1.0);
        for icon in ALL {
            for shape in shapes_at(icon, origin) {
                let bounds = shape.visual_bounding_rect();
                assert!(
                    room.contains_rect(bounds),
                    "{icon:?} painted {bounds:?}, outside {room:?}"
                );
            }
        }
    }

    /// The whole point of the module: an icon-only control still has a name in
    /// the accessibility tree, which is the only thing D24's harness can see.
    #[test]
    fn an_icon_button_carries_its_name() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = icon_button(ui, Icon::Kebab, "Reorder, duplicate or delete");
            });
        });
        let mut output = output;
        // epaint panics if a texture delta is dropped unapplied.
        output.textures_delta.clear();
        let tree = output
            .platform_output
            .accesskit_update
            .expect("accesskit was enabled");
        let names: Vec<String> = tree
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label())
            .map(str::to_owned)
            .collect();
        assert!(
            names.iter().any(|n| n == "Reorder, duplicate or delete"),
            "the kebab button is nameless: {names:?}"
        );
    }
}
