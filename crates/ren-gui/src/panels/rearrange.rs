//! The Rearrange builder (P124): cut a sample name into blocks, drag them
//! into a new order, check the result on real files — and a Rearrange card
//! comes out.
//!
//! One window with three areas, top to bottom — **1 Cut**, **2 Build**,
//! **3 Check** — and a **Guide me** button that shows the same three one at a
//! time. Simple by default (the cut is guessed, the blocks are dragged),
//! configurable underneath: **Advanced** splits by a typed pattern or a
//! regular expression, and every block in the result has its own case and
//! trim.
//!
//! The pure half — [`Cutter`] and [`Builder`]'s editing methods — is plain
//! data, so the tests drive it directly; the egui half only draws it.

use std::ops::Range;
use std::sync::Arc;

use ren_core::ops::casing::CaseMode;
use ren_core::ops::rearrange::{Piece, Rearrange, Split};
use ren_core::parts::PartsSpec;

use crate::widgets::diff_text::{self, DiffStyle};

/// At most nine blocks: the parts language numbers `<%1>`…`<%9>`.
pub const MAX_BLOCKS: usize = 9;

/// A sample name and the separators marked in it, as byte ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cutter {
    pub sample: String,
    seps: Vec<Range<usize>>,
}

/// One stretch of the sample: a block (numbered from 1) or a separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Block { slot: u8, text: String },
    Separator { index: usize, text: String },
}

impl Cutter {
    /// Cut at the usual separators — `PartsSpec::detect`, the Music Tagger's
    /// magic wand, so both guess alike.
    pub fn guess(sample: &str) -> Self {
        let pattern = PartsSpec::detect(sample).pattern;
        let mut seps = Vec::new();
        let mut pos = 0;
        let mut rest = pattern.as_str();
        while !rest.is_empty() {
            if let Some(after) = strip_slot(rest) {
                rest = after;
                continue;
            }
            let end = rest.find("<%").unwrap_or(rest.len());
            let literal = &rest[..end];
            rest = &rest[end..];
            if let Some(at) = sample[pos..].find(literal) {
                seps.push(pos + at..pos + at + literal.len());
                pos += at + literal.len();
            }
        }
        Self {
            sample: sample.to_owned(),
            seps,
        }
    }

    /// The sample as alternating blocks and separators.
    pub fn segments(&self) -> Vec<Segment> {
        let mut out = Vec::new();
        let mut pos = 0;
        let mut slot = 1u8;
        for (index, sep) in self.seps.iter().enumerate() {
            if sep.start > pos {
                out.push(Segment::Block {
                    slot,
                    text: self.sample[pos..sep.start].to_owned(),
                });
                slot += 1;
            }
            out.push(Segment::Separator {
                index,
                text: self.sample[sep.clone()].to_owned(),
            });
            pos = sep.end;
        }
        if pos < self.sample.len() {
            out.push(Segment::Block {
                slot,
                text: self.sample[pos..].to_owned(),
            });
        }
        out
    }

    pub fn blocks(&self) -> Vec<String> {
        self.segments()
            .into_iter()
            .filter_map(|s| match s {
                Segment::Block { text, .. } => Some(text),
                Segment::Separator { .. } => None,
            })
            .collect()
    }

    pub fn separators(&self) -> Vec<String> {
        self.seps
            .iter()
            .map(|r| self.sample[r.clone()].to_owned())
            .collect()
    }

    /// The cut in the parts language: `<%1> - <%2>`.
    pub fn pattern(&self) -> String {
        self.segments()
            .into_iter()
            .map(|s| match s {
                Segment::Block { slot, .. } => format!("<%{slot}>"),
                Segment::Separator { text, .. } => text,
            })
            .collect()
    }

    /// Joins the two blocks either side of separator `index`.
    pub fn merge(&mut self, index: usize) {
        if index < self.seps.len() {
            self.seps.remove(index);
        }
    }

    /// Marks `range` (bytes) of the sample as a separator.
    pub fn split(&mut self, range: Range<usize>) -> Result<(), &'static str> {
        if range.is_empty()
            || range.end > self.sample.len()
            || !self.sample.is_char_boundary(range.start)
            || !self.sample.is_char_boundary(range.end)
        {
            return Err("Select some text in the name first");
        }
        if self
            .seps
            .iter()
            .any(|s| s.start < range.end && range.start < s.end)
        {
            return Err("That is already a separator");
        }
        let mut next = self.clone();
        next.seps.push(range);
        next.seps.sort_by_key(|r| r.start);
        if next.blocks().len() > MAX_BLOCKS {
            return Err("Nine blocks at most");
        }
        *self = next;
        Ok(())
    }
}

/// `<%n>` at the start of `text`, and what follows it.
fn strip_slot(text: &str) -> Option<&str> {
    let b = text.as_bytes();
    (b.len() >= 4 && b[0] == b'<' && b[1] == b'%' && b[2].is_ascii_digit() && b[3] == b'>')
        .then(|| &text[4..])
}

/// How the blocks are found: cut by hand, or typed under Advanced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Cut(Cutter),
    /// The parts language typed by hand.
    Pattern(String),
    Regex(String),
}

/// A change to the builder decided while it is being drawn, made after.
type Edit = Box<dyn FnOnce(&mut Builder)>;

/// What a drag carries.
#[derive(Debug, Clone)]
enum Drag {
    Palette(Piece),
    Result(usize),
}

/// The builder while it is open.
#[derive(Debug, Clone)]
pub struct Builder {
    /// The card being edited, or `None` for a new one.
    pub card: Option<crate::viewmodel::CardId>,
    pub source: Source,
    pub layout: Vec<Piece>,
    pub case_sensitive: bool,
    /// Stems of the listed files: the sample picker and the Check area.
    names: Vec<String>,
    /// `Some(step)` while Guide me walks the three areas.
    pub wizard: Option<usize>,
    pub only_misfits: bool,
    new_text: String,
    selection: Option<Range<usize>>,
    message: Option<String>,
}

/// What the window asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Cancel,
    Done(Rearrange),
}

impl Builder {
    /// A builder on `sample`, with the listed `names` to check against. With
    /// `existing`, it reopens that rule.
    pub fn new(
        sample: &str,
        names: Vec<String>,
        existing: Option<&Rearrange>,
        card: Option<crate::viewmodel::CardId>,
    ) -> Self {
        let mut builder = Self {
            card,
            source: Source::Cut(Cutter::guess(sample)),
            layout: Vec::new(),
            case_sensitive: false,
            names,
            wizard: None,
            only_misfits: false,
            new_text: String::new(),
            selection: None,
            message: None,
        };
        match existing {
            Some(rule) if !rule.layout.is_empty() => {
                let sample = if rule.sample.is_empty() {
                    sample
                } else {
                    &rule.sample
                };
                builder.source = match &rule.split {
                    Split::Regex(regex) => Source::Regex(regex.clone()),
                    Split::Parts(spec) => {
                        let cut = Cutter::guess(sample);
                        if cut.pattern() == spec.pattern {
                            Source::Cut(cut)
                        } else {
                            Source::Pattern(spec.pattern.clone())
                        }
                    }
                };
                if let Source::Cut(cut) = &mut builder.source {
                    cut.sample = sample.to_owned();
                }
                builder.layout = rule.layout.clone();
                builder.case_sensitive = rule.case_sensitive;
            }
            _ => builder.layout = builder.identity(),
        }
        builder
    }

    pub fn sample(&self) -> String {
        match &self.source {
            Source::Cut(cut) => cut.sample.clone(),
            _ => self.names.first().cloned().unwrap_or_default(),
        }
    }

    /// The rule as it stands.
    pub fn rule(&self) -> Rearrange {
        let split = match &self.source {
            Source::Cut(cut) => Split::Parts(PartsSpec::new(cut.pattern())),
            Source::Pattern(pattern) => Split::Parts(PartsSpec::new(pattern.clone())),
            Source::Regex(regex) => Split::Regex(regex.clone()),
        };
        Rearrange::new(split, self.layout.clone())
            .with_sample(self.sample())
            .matching_case(self.case_sensitive)
    }

    /// The sample's blocks under the current split, by slot.
    pub fn block_texts(&self) -> Vec<String> {
        let blocks = Rearrange::new(self.rule().split, Vec::new())
            .matching_case(self.case_sensitive)
            .blocks(&self.sample())
            .unwrap_or_default();
        blocks.into_iter().map_while(|b| b).collect()
    }

    /// The name back as it was: every block and separator in place.
    fn identity(&self) -> Vec<Piece> {
        match &self.source {
            Source::Cut(cut) => cut
                .segments()
                .into_iter()
                .map(|s| match s {
                    Segment::Block { slot, .. } => Piece::part(slot),
                    Segment::Separator { text, .. } => Piece::Text(text),
                })
                .collect(),
            _ => (1..=self.block_texts().len() as u8)
                .map(Piece::part)
                .collect(),
        }
    }

    /// After the cut changed: the old order means nothing, so start again
    /// from the name as it is.
    fn recut(&mut self) {
        self.layout = self.identity();
    }

    pub fn push(&mut self, piece: Piece) {
        self.layout.push(piece);
    }

    pub fn insert(&mut self, at: usize, piece: Piece) {
        let at = at.min(self.layout.len());
        self.layout.insert(at, piece);
    }

    /// Moves result block `from` to sit in front of `before` (`len` = end).
    pub fn move_to(&mut self, from: usize, before: usize) {
        if from >= self.layout.len() {
            return;
        }
        let piece = self.layout.remove(from);
        let before = if before > from { before - 1 } else { before };
        self.insert(before, piece);
    }

    pub fn move_left(&mut self, index: usize) {
        if index > 0 {
            self.layout.swap(index, index - 1);
        }
    }

    pub fn move_right(&mut self, index: usize) {
        if index + 1 < self.layout.len() {
            self.layout.swap(index, index + 1);
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.layout.len() {
            self.layout.remove(index);
        }
    }

    /// Cuts the sample at `range` (bytes). Only while cutting by hand.
    pub fn split_at(&mut self, range: Range<usize>) {
        if let Source::Cut(cut) = &mut self.source {
            match cut.split(range) {
                Ok(()) => {
                    self.message = None;
                    self.recut();
                }
                Err(why) => self.message = Some(why.to_owned()),
            }
        }
    }

    pub fn merge(&mut self, index: usize) {
        if let Source::Cut(cut) = &mut self.source {
            cut.merge(index);
            self.recut();
        }
    }

    pub fn set_sample(&mut self, sample: &str) {
        self.source = Source::Cut(Cutter::guess(sample));
        self.recut();
    }

    /// `(old, new)` for each listed name — `new` is `None` for one that
    /// does not fit. Tags show as themselves: they are filled per file only
    /// once the card is in the pipeline.
    pub fn check(&self) -> Vec<(String, Option<String>)> {
        let rule = self.rule();
        self.names
            .iter()
            .map(|name| (name.clone(), rule.preview(name).ok().flatten()))
            .collect()
    }

    pub fn misfits(&self) -> usize {
        self.check().iter().filter(|(_, new)| new.is_none()).count()
    }
}

/// One colour per block, so a block reads as the same thing in all three
/// areas.
fn block_colour(slot: u8, dark: bool) -> egui::Color32 {
    const LIGHT: [(u8, u8, u8); 9] = [
        (0xbb, 0xde, 0xfb),
        (0xc8, 0xe6, 0xc9),
        (0xff, 0xe0, 0xb2),
        (0xe1, 0xbe, 0xe7),
        (0xff, 0xcd, 0xd2),
        (0xb2, 0xeb, 0xf2),
        (0xf0, 0xf4, 0xc3),
        (0xd7, 0xcc, 0xc8),
        (0xcf, 0xd8, 0xdc),
    ];
    let (r, g, b) = LIGHT[usize::from(slot.saturating_sub(1)) % 9];
    if dark {
        egui::Color32::from_rgb(r / 3, g / 3, b / 3)
    } else {
        egui::Color32::from_rgb(r, g, b)
    }
}

fn piece_label(piece: &Piece, blocks: &[String]) -> String {
    match piece {
        Piece::Part { slot, .. } => blocks
            .get(usize::from(slot.saturating_sub(1)))
            .cloned()
            .unwrap_or_else(|| format!("<%{slot}>")),
        Piece::Text(text) => format!("“{text}”"),
        Piece::Tag(tag) => tag.as_str().to_owned(),
    }
}

/// A block as a coloured chip; the caller decides what it does.
fn chip(ui: &mut egui::Ui, piece: &Piece, blocks: &[String]) -> egui::Response {
    chip_labelled(ui, piece, &piece_label(piece, blocks))
}

fn chip_labelled(ui: &mut egui::Ui, piece: &Piece, label: &str) -> egui::Response {
    let dark = ui.visuals().dark_mode;
    let fill = match piece {
        Piece::Part { slot, .. } => block_colour(*slot, dark),
        _ => ui.visuals().widgets.inactive.weak_bg_fill,
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(label)
                    .selectable(false)
                    .sense(egui::Sense::click()),
            )
        })
        .inner
}

pub fn ui(ctx: &egui::Context, builder: &mut Builder) -> Outcome {
    let mut outcome = Outcome::Open;
    let modal = egui::Modal::new(egui::Id::unique("rearrange_modal")).show(ctx, |ui| {
        let width = (ctx.content_rect().width() - 80.0).clamp(480.0, 900.0);
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.heading("Rearrange names");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let guided = builder.wizard.is_some();
                if ui
                    .selectable_label(guided, "Guide me")
                    .on_hover_text("Walk through the three steps one at a time")
                    .clicked()
                {
                    builder.wizard = if guided { None } else { Some(0) };
                }
            });
        });

        // The sample: any listed name.
        ui.horizontal(|ui| {
            let label = ui.label("Sample:");
            let current = builder.sample();
            egui::ComboBox::from_id_salt("rearrange_sample")
                .selected_text(&current)
                .width((width - 120.0).max(200.0))
                .show_ui(ui, |ui| {
                    for name in builder.names.clone().iter().take(500) {
                        if ui.selectable_label(*name == current, name).clicked() {
                            builder.set_sample(name);
                        }
                    }
                })
                .response
                .labelled_by(label.id);
        });
        ui.add_space(6.0);

        let steps: &[usize] = match builder.wizard {
            Some(step) => &[[0usize, 1, 2][step.min(2)]],
            None => &[0, 1, 2],
        };
        for &step in steps {
            match step {
                0 => cut_ui(ui, builder),
                1 => build_ui(ui, builder),
                _ => check_ui(ui, builder, width),
            }
            ui.add_space(8.0);
        }
        if builder.wizard.is_none() {
            advanced_ui(ui, builder);
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                outcome = Outcome::Cancel;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let last = builder.wizard.is_none_or(|s| s >= 2);
                let done = if builder.card.is_some() {
                    "Update card"
                } else {
                    "Add card"
                };
                if last {
                    let ok = !builder.layout.is_empty();
                    if ui
                        .add_enabled(ok, egui::Button::new(done))
                        .on_disabled_hover_text("Build a name first")
                        .clicked()
                    {
                        outcome = Outcome::Done(builder.rule());
                    }
                } else if ui.button("Next").clicked() {
                    builder.wizard = builder.wizard.map(|s| s + 1);
                }
                if let Some(step) = builder.wizard
                    && step > 0
                    && ui.button("Back").clicked()
                {
                    builder.wizard = Some(step - 1);
                }
            });
        });
    });
    if modal.should_close() && outcome == Outcome::Open {
        outcome = Outcome::Cancel;
    }
    outcome
}

fn heading(ui: &mut egui::Ui, title: &str, help: &str, guided: bool) {
    ui.strong(title);
    if guided {
        ui.label(help);
    }
}

fn cut_ui(ui: &mut egui::Ui, builder: &mut Builder) {
    let guided = builder.wizard.is_some();
    heading(
        ui,
        "1  Cut",
        "The name is cut into blocks at its separators. Click a separator to join the \
         blocks either side; select text in the name below and press Split here to cut there.",
        guided,
    );
    let Source::Cut(cut) = &builder.source else {
        ui.weak("Cut by the pattern under Advanced.");
        return;
    };
    let cut = cut.clone();
    let segments = cut.segments();
    let mut merge = None;
    ui.horizontal_wrapped(|ui| {
        for segment in &segments {
            match segment {
                Segment::Block { slot, text } => {
                    chip_labelled(ui, &Piece::part(*slot), text);
                }
                Segment::Separator { index, text } => {
                    if ui
                        .button(format!("“{text}”"))
                        .on_hover_text("A separator — click to join the blocks either side")
                        .clicked()
                    {
                        merge = Some(*index);
                    }
                }
            }
        }
    });
    if let Some(index) = merge {
        builder.merge(index);
    }

    // The name as selectable text: a selection marks a new separator.
    let sample = cut.sample.clone();
    ui.horizontal(|ui| {
        let mut text: &str = &sample;
        let output = egui::TextEdit::singleline(&mut text)
            .id_salt("rearrange_sample_text")
            .desired_width(ui.available_width() - 200.0)
            .show(ui);
        if let Some(range) = output.cursor_range {
            let [a, b] = range.sorted_cursors();
            if a.index != b.index {
                let to_byte = |chars: usize| {
                    sample
                        .char_indices()
                        .nth(chars)
                        .map_or(sample.len(), |(i, _)| i)
                };
                builder.selection = Some(to_byte(a.index.into())..to_byte(b.index.into()));
            }
        }
        if ui
            .add_enabled(builder.selection.is_some(), egui::Button::new("Split here"))
            .on_hover_text("Make the selected text a separator")
            .clicked()
            && let Some(range) = builder.selection.take()
        {
            builder.split_at(range);
        }
        if ui
            .button("Guess again")
            .on_hover_text("Cut at the usual separators")
            .clicked()
        {
            let sample = builder.sample();
            builder.set_sample(&sample);
        }
    });
    if let Some(message) = &builder.message {
        ui.colored_label(ui.visuals().warn_fg_color, message);
    }
}

fn build_ui(ui: &mut egui::Ui, builder: &mut Builder) {
    let guided = builder.wizard.is_some();
    heading(
        ui,
        "2  Build",
        "Drag blocks into the new name, or click one to add it at the end. Drag a block out \
         of the new name, or press ×, to leave it out.",
        guided,
    );
    let blocks = builder.block_texts();

    // The palette: every block, the separators found, text and tags.
    let mut palette: Vec<Piece> = (1..=blocks.len() as u8).map(Piece::part).collect();
    if let Source::Cut(cut) = &builder.source {
        for sep in cut.separators() {
            let piece = Piece::Text(sep);
            if !palette.contains(&piece) {
                palette.push(piece);
            }
        }
    }
    let mut add = None;
    let (_, dropped_out) = ui.dnd_drop_zone::<Drag, _>(egui::Frame::new(), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Blocks:");
            for (i, piece) in palette.iter().enumerate() {
                let id = ui.scope_id().with(("palette", i));
                let response = ui
                    .dnd_drag_source(id, Drag::Palette(piece.clone()), |ui| {
                        chip(ui, piece, &blocks)
                    })
                    .inner;
                if response.clicked() {
                    add = Some(piece.clone());
                }
            }
            ui.add(
                egui::TextEdit::singleline(&mut builder.new_text)
                    .hint_text("text")
                    .desired_width(70.0),
            );
            if ui
                .add_enabled(!builder.new_text.is_empty(), egui::Button::new("+ Text"))
                .clicked()
            {
                add = Some(Piece::Text(std::mem::take(&mut builder.new_text)));
            }
            ui.menu_button("+ Tag ▾", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for tag in crate::widgets::tag_field::menu_tags() {
                            if ui.button(tag).clicked() {
                                add = Some(Piece::Tag(tag.into()));
                                ui.close();
                            }
                        }
                    });
            });
        });
    });
    if let Some(piece) = add {
        builder.push(piece);
    }
    if let Some(drag) = dropped_out
        && let Drag::Result(index) = *drag
    {
        builder.remove(index);
    }

    // The result: drop targets, each with ◀ ▶ × for the keyboard.
    let mut action: Option<Edit> = None;
    let layout = builder.layout.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("New name:");
        for (i, piece) in layout.iter().enumerate() {
            let (_, dropped) = ui.dnd_drop_zone::<Drag, _>(egui::Frame::new(), |ui| {
                ui.horizontal(|ui| {
                    let id = ui.scope_id().with(("result", i));
                    ui.dnd_drag_source(id, Drag::Result(i), |ui| chip(ui, piece, &blocks));
                    if let Piece::Part { case, trim, .. } = piece {
                        let mut case = *case;
                        let mut trim = *trim;
                        let before = (case, trim);
                        ui.menu_button("Aa", |ui| {
                            for (mode, label) in [
                                (CaseMode::NoChange, "As is"),
                                (CaseMode::Title, "Title Case"),
                                (CaseMode::Sentence, "Sentence case"),
                                (CaseMode::Upper, "UPPER"),
                                (CaseMode::Lower, "lower"),
                            ] {
                                ui.radio_value(&mut case, mode, label);
                            }
                            ui.checkbox(&mut trim, "Trim spaces");
                        })
                        .response
                        .on_hover_text("Case and spaces for this block");
                        if (case, trim) != before {
                            action = Some(Box::new(move |b: &mut Builder| {
                                if let Some(Piece::Part {
                                    case: c, trim: t, ..
                                }) = b.layout.get_mut(i)
                                {
                                    *c = case;
                                    *t = trim;
                                }
                            }));
                        }
                    }
                    if ui.small_button("◀").on_hover_text("Move left").clicked() {
                        action = Some(Box::new(move |b: &mut Builder| b.move_left(i)));
                    }
                    if ui.small_button("▶").on_hover_text("Move right").clicked() {
                        action = Some(Box::new(move |b: &mut Builder| b.move_right(i)));
                    }
                    if ui.small_button("×").on_hover_text("Leave out").clicked() {
                        action = Some(Box::new(move |b: &mut Builder| b.remove(i)));
                    }
                });
            });
            if let Some(drag) = dropped {
                let drag = Arc::unwrap_or_clone(drag);
                action = Some(Box::new(move |b: &mut Builder| match drag {
                    Drag::Palette(piece) => b.insert(i, piece),
                    Drag::Result(from) => b.move_to(from, i),
                }));
            }
        }
        // The end of the row: drop here to append.
        let (_, dropped) = ui.dnd_drop_zone::<Drag, _>(egui::Frame::new(), |ui| {
            ui.weak(if layout.is_empty() {
                "drop blocks here"
            } else {
                "  ⟵ drop here  "
            });
        });
        if let Some(drag) = dropped {
            let drag = Arc::unwrap_or_clone(drag);
            action = Some(Box::new(move |b: &mut Builder| match drag {
                Drag::Palette(piece) => b.push(piece),
                Drag::Result(from) => b.move_to(from, b.layout.len()),
            }));
        }
    });
    if let Some(action) = action {
        action(builder);
    }
    if !layout.is_empty() && ui.small_button("Start again").clicked() {
        builder.recut();
    }
}

fn check_ui(ui: &mut egui::Ui, builder: &mut Builder, width: f32) {
    let guided = builder.wizard.is_some();
    heading(
        ui,
        "3  Check",
        "What the files in the list would be called. A file that does not have the \
         sample's shape is left alone and flagged in the list.",
        guided,
    );
    let check = builder.check();
    let misfits = check.iter().filter(|(_, new)| new.is_none()).count();
    ui.horizontal(|ui| {
        if misfits > 0 {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                if misfits == 1 {
                    "⚠ 1 file doesn't fit and will be left alone".to_owned()
                } else {
                    format!("⚠ {misfits} files don't fit and will be left alone")
                },
            );
            ui.checkbox(&mut builder.only_misfits, "Show only these");
        } else {
            ui.label(format!("All {} fit", ren_core::plural(check.len(), "file")));
        }
    });
    let style = DiffStyle::for_ui(ui);
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("rearrange_check")
                .num_columns(2)
                .spacing([24.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    let column = (width / 2.0 - 24.0).max(160.0);
                    let left = egui::Layout::left_to_right(egui::Align::Center);
                    for (old, new) in check
                        .iter()
                        .filter(|(_, new)| !builder.only_misfits || new.is_none())
                        .take(50)
                    {
                        let cell = egui::vec2(column, 18.0);
                        ui.allocate_ui_with_layout(cell, left, |ui| {
                            ui.add(egui::Label::new(old).truncate());
                        });
                        ui.allocate_ui_with_layout(cell, left, |ui| match new {
                            Some(new) => diff_text::new_name(ui, old, new, style),
                            None => {
                                ui.colored_label(ui.visuals().warn_fg_color, "⚠ doesn't fit");
                            }
                        });
                        ui.end_row();
                    }
                });
        });
}

fn advanced_ui(ui: &mut egui::Ui, builder: &mut Builder) {
    egui::CollapsingHeader::new("Advanced")
        .id_salt("rearrange_advanced")
        .show(ui, |ui| {
            let mut mode = match builder.source {
                Source::Cut(_) => 0,
                Source::Pattern(_) => 1,
                Source::Regex(_) => 2,
            };
            let before = mode;
            ui.horizontal(|ui| {
                ui.label("Cut by:");
                ui.radio_value(&mut mode, 0, "Separators in the sample");
                ui.radio_value(&mut mode, 1, "A parts pattern");
                ui.radio_value(&mut mode, 2, "A regular expression");
            });
            if mode != before {
                let sample = builder.sample();
                builder.source = match mode {
                    0 => Source::Cut(Cutter::guess(&sample)),
                    1 => Source::Pattern(Cutter::guess(&sample).pattern()),
                    _ => Source::Regex("^(.*?) - (.*)$".to_owned()),
                };
                builder.recut();
            }
            let mut recut = false;
            match &mut builder.source {
                Source::Cut(cut) => {
                    ui.horizontal(|ui| {
                        ui.label("As a pattern:");
                        ui.monospace(cut.pattern());
                    });
                }
                Source::Pattern(pattern) => {
                    ui.horizontal(|ui| {
                        let label = ui.label("Pattern:");
                        recut |= ui
                            .text_edit_singleline(pattern)
                            .labelled_by(label.id)
                            .on_hover_text(
                                "<%1> … <%9> are the blocks; anything else is a separator",
                            )
                            .changed();
                    });
                }
                Source::Regex(regex) => {
                    ui.horizontal(|ui| {
                        let label = ui.label("Regular expression:");
                        recut |= ui
                            .text_edit_singleline(regex)
                            .labelled_by(label.id)
                            .on_hover_text("Each capture group ( … ) is a block, in order")
                            .changed();
                    });
                }
            }
            if recut {
                builder.recut();
            }
            ui.checkbox(
                &mut builder.case_sensitive,
                "Separators and expressions match case",
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cut_is_guessed_at_the_usual_separators() {
        let cut = Cutter::guess("Dune - Frank Herbert");
        assert_eq!(cut.blocks(), ["Dune", "Frank Herbert"]);
        assert_eq!(cut.separators(), [" - "]);
        assert_eq!(cut.pattern(), "<%1> - <%2>");
    }

    #[test]
    fn a_selection_splits_and_a_separator_merges() {
        let mut cut = Cutter::guess("Dune_Frank Herbert");
        assert_eq!(cut.blocks(), ["Dune_Frank Herbert"]);
        cut.split(4..5).unwrap();
        assert_eq!(cut.blocks(), ["Dune", "Frank Herbert"]);
        assert!(cut.split(4..5).is_err(), "already a separator");
        assert!(cut.split(3..3).is_err(), "nothing selected");
        cut.merge(0);
        assert_eq!(cut.blocks(), ["Dune_Frank Herbert"]);
    }

    /// The user's own example: swap two blocks, keep the separator.
    #[test]
    fn swapping_the_blocks_rearranges_every_name() {
        let names = vec![
            "Dune - Frank Herbert".to_owned(),
            "Emma - Jane Austen".to_owned(),
        ];
        let mut builder = Builder::new("Dune - Frank Herbert", names, None, None);
        assert_eq!(
            builder.layout,
            [Piece::part(1), Piece::Text(" - ".into()), Piece::part(2)],
            "starts as the name it is"
        );
        // Author to the front, title to the end — what two drags do.
        builder.move_to(2, 0);
        builder.move_to(1, 3);
        assert_eq!(
            builder.layout,
            [Piece::part(2), Piece::Text(" - ".into()), Piece::part(1)]
        );
        assert_eq!(builder.check()[1].1.as_deref(), Some("Jane Austen - Emma"));
        assert_eq!(builder.misfits(), 0);
    }

    #[test]
    fn a_name_without_the_separator_does_not_fit() {
        let names = vec!["Dune - Frank Herbert".to_owned(), "Notes".to_owned()];
        let mut builder = Builder::new("Dune - Frank Herbert", names, None, None);
        builder.move_to(2, 0);
        assert_eq!(builder.misfits(), 1);
    }

    #[test]
    fn a_rule_reopens_as_it_was_saved() {
        let mut builder = Builder::new("Dune - Frank Herbert", vec![], None, None);
        builder.move_to(2, 0);
        builder.push(Piece::Text(" (ebook)".into()));
        let rule = builder.rule();
        let again = Builder::new("Other - Name", vec![], Some(&rule), None);
        assert_eq!(again.layout, rule.layout);
        assert_eq!(again.sample(), "Dune - Frank Herbert");
    }
}
