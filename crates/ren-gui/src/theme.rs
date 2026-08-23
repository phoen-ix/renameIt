//! The app's `Style`, for both themes.
//!
//! Until this module existed the GUI ran on stock `egui::Visuals`, and stock is
//! tuned for a demo window rather than for a table a user reads all day. Three
//! things were actually broken rather than merely plain:
//!
//! * **Unchecked checkboxes were invisible inside an operation card.**
//!   `Ui::dnd_drop_zone` fills its frame with `widgets.inactive.bg_fill`, which
//!   is the same colour egui paints an unchecked checkbox with, and stock
//!   `inactive.bg_stroke` is `Stroke::NONE`. Card fill and box fill were both
//!   `gray(230)` in light mode and `gray(60)` in dark — a contrast ratio of
//!   1.00:1. *Case sensitive*, *Swap mode* and *Regular expression* had no
//!   visible control at all. [`inactive_outline`] is what fixes it, and
//!   it has to be a stroke rather than a different fill because `dnd_drop_zone`
//!   overrides the fill we hand it.
//! * **Secondary text sat at 2.7:1.** `weak_text_alpha` defaults to 0.6 over a
//!   `noninteractive.fg_stroke` of `gray(140)`, which lands weak text on
//!   `gray(95)`. Raising the body colour first is what buys room for a weak
//!   colour that is both readable and still visibly secondary.
//! * **The accent was invisible in light mode.** Stock `selection.bg_fill` is
//!   `(144, 209, 255)`, which is 1.55:1 against the `gray(248)` panel — a
//!   "selected" segment you cannot see is not a segment.
//!
//! Every colour below was solved against WCAG AA (4.5:1 body, 3:1 UI) using
//! egui's own compositing rule — `weak`/`disabled` are `gamma_multiply` in
//! gamma space, so the rendered value is `fg * alpha + bg * (1 - alpha)`.
//! `theme_contrast` in the tests re-derives the whole table, so a future tweak
//! that drops a value below the floor fails the build rather than the review.
//!
//! Related decisions: D8 (modernized UI), D22 (theme preference lives in egui,
//! not `dark-light`), D26 (no animation — nothing here animates).

use egui::{Color32, CornerRadius, Stroke, Visuals};

/// The vertical rhythm, in points.
///
/// Before this existed `ui.add_space` was called with 4, 6, 8, 10 and 12
/// interchangeably across the crate. These are the only values that should
/// appear from here on.
pub mod space {
    /// Between a label and the control it labels.
    pub const TIGHT: f32 = 4.0;
    /// Between controls inside one group.
    pub const SNUG: f32 = 8.0;
    /// Between groups inside one panel.
    pub const LOOSE: f32 = 14.0;
    /// Between top-level sections, either side of a separator.
    pub const SECTION: f32 = 20.0;
}

/// Per-theme colours that egui has no slot for, so widgets read them from here.
#[derive(Debug, Clone, Copy)]
pub struct Accents {
    /// Fill for a card surface that must read as raised above the panel.
    pub surface: Color32,
    /// The primary action's fill — `Rename`, and nothing else.
    pub primary: Color32,
    /// Text drawn on [`Self::primary`].
    pub on_primary: Color32,
}

impl Accents {
    pub fn of(dark: bool) -> Self {
        if dark {
            Self {
                surface: Color32::from_gray(40),
                primary: Color32::from_rgb(24, 110, 155),
                on_primary: Color32::WHITE,
            }
        } else {
            Self {
                surface: Color32::from_gray(238),
                primary: Color32::from_rgb(21, 101, 192),
                on_primary: Color32::WHITE,
            }
        }
    }

    /// The accents matching whatever theme `ui` is currently drawing in.
    pub fn from_ui(ui: &egui::Ui) -> Self {
        Self::of(ui.visuals().dark_mode)
    }
}

/// Add `Hack` to the `Proportional` fallback chain.
///
/// epaint already bundles Hack — it is the whole `Monospace` family — but
/// `Proportional` resolves Ubuntu-Light → NotoEmoji → emoji-icon-font, and
/// none of those three has `▾ ▲ ▼ ⋯ ⊘ → ▸`. Every one of them shipped as `◻`:
/// the file table's sort indicator, the `Presets ▾` button, the tag field's
/// history menu, four controls on every Batch Replace row, and — worse,
/// because it is prose rather than an icon — the `Settings ▸ File System`
/// inside the blocked-Rename explanation and the `→` inside the confirmation
/// dialog's summary.
///
/// Naming a font that is already linked in costs nothing: no new dependency to
/// audit under D2, no growth in the binary D14 watches. The fallback only
/// fires for a code point Ubuntu-Light lacks, so ordinary text is untouched.
/// The five marks Hack does not have either are painted instead — see
/// [`crate::widgets::icons`].
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(chain) = fonts.families.get_mut(&egui::FontFamily::Proportional)
        && !chain.iter().any(|name| name == "Hack")
    {
        chain.push("Hack".to_owned());
    }
    for (name, bytes) in BUNDLED_FACES {
        fonts.font_data.insert(
            (*name).to_owned(),
            // `from_static`, not `from_owned`: epaint clones the blob into
            // every `FontsImpl`, and it rebuilds one whenever the glyph atlas
            // is recreated. An owned `Vec` would memcpy 16 MB each time; a
            // `&'static [u8]` costs nothing.
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
        // Appended, never prepended. `FontDefinitions::families` makes the
        // first face primary, so a CJK font at the front would take over Latin
        // too — and the icon glyphs and the `Hack` fallback both have to keep
        // winning over these.
        if let Some(chain) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            chain.push((*name).to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

/// The faces bundled for the scripts the default chain cannot draw.
///
/// A rename tool shows filenames from disk, and a filename is whatever the
/// person who made it typed. Latin, Greek and Cyrillic are what egui ships
/// with; everything below was a row of `◻` before, which for a tool whose
/// whole job is showing names is a failure rather than a rough edge.
///
/// CJK is the only expensive one — the other four together are under 550 KB,
/// which is why they are here too rather than left for later. All are
/// SIL Open Font Licence 1.1; `deny.toml` allows `OFL-1.1` and **D15** set the
/// precedent for scoping a font licence. The licence text and the provenance
/// of each file are in `THIRD-PARTY-FONTS.md`, and the release staging ships
/// it, because the OFL requires the notice to travel with the font and
/// `cargo-deny` cannot check an asset that is not a crate.
const BUNDLED_FACES: &[(&str, &[u8])] = &[
    (
        "NotoSansCJK",
        include_bytes!("../assets/fonts/NotoSansCJKjp-Regular.otf"),
    ),
    (
        "NotoSansHebrew",
        include_bytes!("../assets/fonts/NotoSansHebrew-Regular.ttf"),
    ),
    (
        "NotoSansArabic",
        include_bytes!("../assets/fonts/NotoSansArabic-Regular.ttf"),
    ),
    (
        "NotoSansThai",
        include_bytes!("../assets/fonts/NotoSansThai-Regular.ttf"),
    ),
    (
        "NotoSansDevanagari",
        include_bytes!("../assets/fonts/NotoSansDevanagari-Regular.ttf"),
    ),
];

/// Install both themes' styles. Call once, before the first frame; egui then
/// picks between them from the [`egui::ThemePreference`].
pub fn install(ctx: &egui::Context) {
    for (theme, visuals) in [
        (egui::Theme::Dark, visuals(true)),
        (egui::Theme::Light, visuals(false)),
    ] {
        ctx.style_mut_of(theme, |style| {
            style.visuals = visuals;
            style.text_styles = text_styles();
            spacing(style);
        });
    }
}

/// The type scale.
///
/// egui's defaults are `Small 9 / Body 13 / Button 13 / Heading 18`, and the
/// app had never overridden them. That made the scale **inverted**: `Small` is
/// meant for incidental annotation — a unit beside a number, a hint under a
/// field — and this app uses `.small()` for the actual explanatory copy, in
/// **106 places**, twenty-three of them in the Settings window alone. On the
/// Problem Solver page almost every word that carries meaning was at 9 pt
/// while the four-word headings got 13.
///
/// So the important number is not the absolute size but the ratio: prose moves
/// from 0.69x body to 0.79x body, which is the difference between "annotation"
/// and "text you are meant to read". Raising the style fixes all 106 call
/// sites without touching one of them.
///
/// This is *not* the same knob as the interface size. `zoom_factor` scales
/// everything proportionally, so it cannot fix a wrong ratio — at 150 % the
/// 9 pt prose would still have been the smallest thing on the page. See
/// [`crate::panels::settings`] for that control, and note that egui persists
/// `zoom_factor` itself while `text_styles` is `serde(skip)` and would not be.
fn text_styles() -> std::collections::BTreeMap<egui::TextStyle, egui::FontId> {
    use egui::{FontFamily::Monospace, FontFamily::Proportional, FontId, TextStyle};
    [
        (TextStyle::Small, FontId::new(11.0, Proportional)),
        (TextStyle::Body, FontId::new(14.0, Proportional)),
        (TextStyle::Button, FontId::new(14.0, Proportional)),
        (TextStyle::Heading, FontId::new(20.0, Proportional)),
        // Left at 13: the two places it is used — the config paths and the
        // filename editor — are dense by nature, and a monospace face is
        // already wider per character than the proportional one beside it.
        (TextStyle::Monospace, FontId::new(13.0, Monospace)),
    ]
    .into()
}

/// Spacing is theme-independent, so both styles get the same pass.
///
/// Deliberately a short list. Stock `button_padding` is `(4, 1)`, which is
/// what made every icon button a 6-pixel-tall target; widening it is the one
/// change here that a user feels. `interact_size.y` and `menu_margin` are
/// *not* touched, and the reason is worth writing down: several containers in
/// the app — the palette's list, the settings modal, the operation panel — are
/// sized against the stock values, and raising a widget's minimum height by
/// two points is enough to push a row inside a centred `Modal` out of reach of
/// the pointer while leaving it in the accessibility tree. Vertical room is
/// bought at section boundaries with [`space`], not a point at a time on every
/// row.
fn spacing(style: &mut egui::Style) {
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 3.0);
    s.button_padding = egui::vec2(6.0, 2.0);
    s.icon_width = 16.0;
    s.icon_width_inner = 10.0;
    s.icon_spacing = 6.0;
    s.window_margin = egui::Margin::same(10);
}

fn visuals(dark: bool) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let a = Accents::of(dark);

    // Body text. Stock dark is gray(140) (5.12:1), which leaves no room for a
    // weak colour that is both readable and distinguishable from it.
    let body = if dark {
        Color32::from_gray(205)
    } else {
        Color32::from_gray(45)
    };
    let button_text = if dark {
        Color32::from_gray(215)
    } else {
        Color32::from_gray(35)
    };

    // 0.72 rather than stock 0.6: weak text lands at 6.2:1 dark / 5.4:1 light
    // and is still ~1.9x lighter than body, so the hierarchy survives.
    v.weak_text_alpha = 0.72;
    // 0.5 put the disabled Rename reason — text a blocked user has to read — at
    // 2.8:1. 0.62 lifts it over 4:1 while still reading as unavailable.
    v.disabled_alpha = 0.62;

    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, body);
    v.widgets.noninteractive.bg_stroke = Stroke::new(
        1.0,
        if dark {
            Color32::from_gray(70)
        } else {
            Color32::from_gray(200)
        },
    );

    // `bg_fill` is the checkbox interior *and* the card surface `dnd_drop_zone`
    // paints; `weak_bg_fill` is the button background. Stock ties them together,
    // which is half of why the card swallowed its own checkboxes.
    v.widgets.inactive.bg_fill = a.surface;
    v.widgets.inactive.weak_bg_fill = if dark {
        Color32::from_gray(58)
    } else {
        Color32::from_gray(230)
    };
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, inactive_outline(dark));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, button_text);

    for (w, fill_d, fill_l) in [
        (&mut v.widgets.hovered, 72, 220),
        (&mut v.widgets.active, 88, 205),
        (&mut v.widgets.open, 66, 224),
    ] {
        let fill = Color32::from_gray(if dark { fill_d } else { fill_l });
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(
            1.0,
            if dark {
                Color32::from_gray(165)
            } else {
                Color32::from_gray(95)
            },
        );
        w.fg_stroke = Stroke::new(
            1.0,
            if dark {
                Color32::from_gray(240)
            } else {
                Color32::BLACK
            },
        );
    }

    // Selection. `selection.stroke.color` is what egui uses for the *text* of a
    // selected button (`widget_style::button_style`), so it is the one knob that
    // fixes every segmented chip in the app at once.
    //
    // Light mode cannot satisfy both "accent reads against the panel" and "dark
    // text reads on the accent" — the two constraints have no overlapping
    // luminance. So light mode takes a saturated fill with white text, and the
    // chip's `inactive.bg_stroke` border is what keeps its edge legible.
    v.selection.bg_fill = if dark {
        a.primary
    } else {
        Color32::from_rgb(150, 200, 245)
    };
    v.selection.stroke = Stroke::new(
        1.0,
        if dark {
            Color32::WHITE
        } else {
            Color32::from_rgb(12, 42, 82)
        },
    );

    // Text fields. Stock dark `extreme_bg_color` is gray(10) — a near-black
    // well in a gray(27) panel. gray(16) still reads as recessed without the
    // hole.
    if dark {
        v.extreme_bg_color = Color32::from_gray(16);
    }

    v.widgets.noninteractive.corner_radius = CornerRadius::same(4);
    v.widgets.inactive.corner_radius = CornerRadius::same(4);
    v.widgets.hovered.corner_radius = CornerRadius::same(4);
    v.widgets.active.corner_radius = CornerRadius::same(4);
    v.widgets.open.corner_radius = CornerRadius::same(4);

    v
}

/// How tall a modal's scrolling body may be, given the window behind it.
///
/// A hard-coded `max_height` is a bug waiting for a longer list or a larger
/// font: the palette's 360 pt was chosen when the catalogue was shorter, and
/// anything past it is in the accessibility tree but under the fold. `chrome`
/// is what the modal spends on everything that is not the scrolling body —
/// heading, search box, footer.
pub fn modal_body_height(ctx: &egui::Context, chrome: f32) -> f32 {
    let screen = ctx.content_rect().height();
    // 0.72 of what is left, not all of it: a `Modal` is centred from the height
    // it measured on the frame it opened, so a body that only *just* fits ends
    // up positioned against a stale height and the rows below the fold stop
    // taking clicks. Leaving headroom is cheaper than fighting the sizing pass.
    ((screen - chrome) * 0.72).clamp(200.0, 560.0)
}

/// The outline that makes an unchecked checkbox visible on a card.
///
/// Kept public so the contrast test can assert it against the surface it has to
/// survive, which is the whole reason it exists.
pub fn inactive_outline(dark: bool) -> Color32 {
    if dark {
        Color32::from_gray(140)
    } else {
        Color32::from_gray(115)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every non-ASCII character the crate puts in a string literal must have
    /// a glyph in the font that will draw it.
    ///
    /// This is the test the app did not have. Twelve code points shipped as
    /// `◻` — the card's drag handle and overflow menu, the About button, the
    /// sort indicator, four controls on every Batch Replace row, and the `▸`
    /// and `→` inside sentences — because nothing anywhere asserted that a
    /// glyph the code typed was a glyph the font could draw.
    ///
    /// Literals only, not comments: a doc comment is free to name `⠿` as the
    /// thing that used to be there, and a `#[cfg(test)]` module is free to
    /// name a fixture file in any script it likes. `spike.rs` is skipped
    /// outright — M0's throwaway benchmark harness is fixtures end to end.
    ///
    /// That limitation is now closed for the scripts a filename is most likely
    /// to be in: `BUNDLED_FACES` adds CJK, Hebrew, Arabic, Thai and
    /// Devanagari, and `every_bundled_script_rasterises` holds it. What is
    /// still missing is everything outside that set — Armenian, Georgian,
    /// Ethiopic and the rest — which will draw as boxes until a face for them
    /// is bundled too.
    #[test]
    fn every_glyph_the_ui_types_is_one_the_fonts_can_draw() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        // Fonts are built during a frame, not on `set_fonts`.
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing: Vec<(String, char)> = Vec::new();
        for file in rust_files(&src) {
            if file.file_name().is_some_and(|name| name == "spike.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("readable source");
            let where_ = file
                .strip_prefix(&src)
                .unwrap_or(&file)
                .display()
                .to_string();
            for c in literal_chars(&text) {
                let ok = ctx.fonts_mut(|fonts| {
                    fonts.has_glyph(&egui::FontId::proportional(13.0), c)
                        || fonts.has_glyph(&egui::FontId::monospace(13.0), c)
                });
                if !ok && !missing.iter().any(|(_, seen)| *seen == c) {
                    missing.push((where_.clone(), c));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "these render as tofu — add them to widgets::icons, or use a mark \
             the bundled fonts have: {}",
            missing
                .iter()
                .map(|(file, c)| format!("{c:?} (U+{:04X}) in {file}", *c as u32))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    fn rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)
                .expect("readable directory")
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        out
    }

    /// Non-ASCII characters inside double-quoted literals, skipping comments.
    ///
    /// Deliberately simple rather than a parser: it only has to be right about
    /// this crate, and a false positive is a compile-time nag rather than a
    /// shipped bug.
    fn literal_chars(text: &str) -> Vec<char> {
        let mut out = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim_start();
            // Test fixtures are data, not chrome — a unit test is entitled to a
            // filename in a script the bundled fonts cannot draw, and in this
            // crate the test module is always last in the file.
            if trimmed == "#[cfg(test)]" {
                break;
            }
            if trimmed.starts_with("//") {
                continue;
            }
            let mut in_string = false;
            let mut escaped = false;
            for c in line.chars() {
                if escaped {
                    escaped = false;
                    continue;
                }
                match c {
                    '\\' if in_string => escaped = true,
                    '"' => in_string = !in_string,
                    c if in_string && !c.is_ascii() => out.push(c),
                    _ => {}
                }
            }
        }
        out
    }

    /// Every script the app bundles a face for can actually be drawn.
    ///
    /// Both halves are needed, and neither alone is enough.
    ///
    /// `has_glyph` only says some face's charmap knows the character — it says
    /// nothing about whether the outline can be read. Laying the text out only
    /// says *something* was drawn, and the `◻` replacement rasterises to a
    /// perfectly good rect, so that check passes with no fonts installed at
    /// all. Together they mean: a real face claims it, and epaint got an
    /// outline out of it. The CJK face is CFF (`OTTO`) rather than `glyf`, so
    /// the second half is not hypothetical.
    #[test]
    fn every_bundled_script_rasterises() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let mut warmup = ctx.run_ui(egui::RawInput::default(), |_| {});
        warmup.textures_delta.clear();

        // One character per bundled face, plus a Latin one to prove the
        // primary face still wins.
        for (script, sample) in [
            ("Han", '日'),
            ("Kana", 'あ'),
            ("Hangul", '한'),
            ("Hebrew", 'א'),
            ("Arabic", 'ب'),
            ("Thai", 'ก'),
            ("Devanagari", 'क'),
            ("Latin", 'A'),
        ] {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(14.0), sample)),
                "no bundled face claims {script} ({sample:?})"
            );
            let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let galley = ui.painter().layout_no_wrap(
                        sample.to_string(),
                        egui::FontId::proportional(14.0),
                        egui::Color32::WHITE,
                    );
                    let drawn = galley
                        .rows
                        .iter()
                        .flat_map(|row| row.glyphs.iter())
                        .any(|g| g.uv_rect.min != [0, 0] || g.uv_rect.max != [0, 0]);
                    assert!(drawn, "{script} ({sample:?}) laid out to nothing");
                });
            });
            output.textures_delta.clear();
        }
    }

    /// The union check above passes whether or not Hack is reachable from
    /// `Proportional`, because Hack is `Monospace`'s primary font. This is
    /// what actually holds the fallback in place.
    #[test]
    fn hack_is_a_proportional_fallback() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        assert!(
            ctx.fonts_mut(|fonts| fonts.has_glyph(&egui::FontId::proportional(13.0), '▾')),
            "`▾` is Hack-only, so Proportional has lost its Hack fallback"
        );
    }

    /// Prose must not be set smaller than the labels around it.
    ///
    /// egui's defaults are `Small 9 / Body 13` — a ratio of 0.69, which is an
    /// annotation size. This app spends 106 call sites of `.small()` on actual
    /// explanatory copy, so that ratio was the bug: on the Settings pages
    /// almost every word carrying meaning was the smallest text on screen.
    ///
    /// The upper bound matters as much as the lower one. If `Small` creeps up
    /// to `Body` the distinction stops meaning anything and every page becomes
    /// one undifferentiated block.
    #[test]
    fn prose_is_a_readable_fraction_of_body() {
        let styles = text_styles();
        let size = |style: egui::TextStyle| styles[&style].size;

        let (small, body) = (size(egui::TextStyle::Small), size(egui::TextStyle::Body));
        let ratio = small / body;
        assert!(
            (0.74..=0.88).contains(&ratio),
            "Small is {small} against Body {body} — a ratio of {ratio:.2}, and egui's own \
             default of 9/13 (0.69) is what this exists to stay away from"
        );

        // Buttons carry labels, and a label smaller than the prose beside it
        // reads as an afterthought.
        assert!(size(egui::TextStyle::Button) >= body);
        assert!(size(egui::TextStyle::Heading) > body);
    }

    /// Relative luminance, per WCAG 2.1.
    fn luminance(c: Color32) -> f64 {
        let ch = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// egui composites `weak`/`disabled` with `Color32::gamma_multiply`, which
    /// scales in gamma space and leaves the result premultiplied — so what the
    /// screen shows is `fg * alpha + bg * (1 - alpha)`.
    fn faded(fg: Color32, bg: Color32, alpha: f32) -> Color32 {
        let mix = |f: u8, b: u8| (f32::from(f) * alpha + f32::from(b) * (1.0 - alpha)) as u8;
        Color32::from_rgb(
            mix(fg.r(), bg.r()),
            mix(fg.g(), bg.g()),
            mix(fg.b(), bg.b()),
        )
    }

    /// The floor this module exists to hold. AA is 4.5:1 for body text and
    /// 3:1 for a UI component's boundary; the disabled and outline rows are the
    /// two that stock egui failed hardest.
    #[test]
    fn every_theme_pair_clears_wcag_aa() {
        for dark in [true, false] {
            let v = visuals(dark);
            let a = Accents::of(dark);
            let panel = v.panel_fill;
            let card = a.surface;
            let button = v.widgets.inactive.weak_bg_fill;
            let field = v.extreme_bg_color;
            let body = v.widgets.noninteractive.fg_stroke.color;
            let button_text = v.widgets.inactive.fg_stroke.color;

            let cases: [(&str, Color32, Color32, f64); 9] = [
                ("body on panel", body, panel, 4.5),
                ("body on card", body, card, 4.5),
                (
                    "weak on panel",
                    faded(body, panel, v.weak_text_alpha),
                    panel,
                    4.5,
                ),
                (
                    "weak on card",
                    faded(body, card, v.weak_text_alpha),
                    card,
                    4.5,
                ),
                (
                    "placeholder in field",
                    faded(body, field, v.weak_text_alpha),
                    field,
                    4.5,
                ),
                ("button text", button_text, button, 4.5),
                (
                    "disabled button text",
                    faded(button_text, button, v.disabled_alpha),
                    button,
                    3.0,
                ),
                // The one this module was written for: 1.00:1 before.
                (
                    "checkbox outline on card",
                    inactive_outline(dark),
                    card,
                    3.0,
                ),
                (
                    "chip text on accent",
                    v.selection.stroke.color,
                    v.selection.bg_fill,
                    4.5,
                ),
            ];

            for (name, fg, bg, floor) in cases {
                let got = contrast(fg, bg);
                assert!(
                    got >= floor,
                    "{} theme: {name} is {got:.2}:1, needs {floor}:1",
                    if dark { "dark" } else { "light" },
                );
            }
        }
    }

    /// Weak text has to be readable *and* still read as secondary. Without the
    /// upper bound, "fix the contrast" degenerates into "delete the hierarchy".
    #[test]
    fn weak_text_stays_visibly_weaker_than_body() {
        for dark in [true, false] {
            let v = visuals(dark);
            let body = v.widgets.noninteractive.fg_stroke.color;
            let weak = faded(body, v.panel_fill, v.weak_text_alpha);
            let step = contrast(body, v.panel_fill) / contrast(weak, v.panel_fill);
            assert!(
                (1.4..=2.6).contains(&step),
                "{} theme: body/weak step is {step:.2}x, want 1.4-2.6x",
                if dark { "dark" } else { "light" },
            );
        }
    }
}
