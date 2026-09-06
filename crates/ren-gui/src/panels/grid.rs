//! The thumbnail grid — the same listing, drawn as tiles.
//!
//! Virtualized by hand. `egui_table` gives the list its virtualization for
//! free, and there is no grid equivalent, so this computes which rows fall in
//! the viewport from the scroll offset and draws only those. That is not an
//! optimisation to defer: an unvirtualized grid puts one accessibility node per
//! file into the tree, so a ten-thousand-file folder would change what
//! `query_by_label_contains` means for every test in the suite — quietly, and
//! in a way that looks like a test-harness problem rather than a grid problem.
//!
//! A tile carries the picture, the name on disk **and** the name the run will
//! write, with the same conflict badge and the same dimming the table uses —
//! all of it from `super::rows`, so the two views cannot drift.

use ren_core::model::FileEntry;
use ren_core::{Plan, PlanItem};

use super::rows::{self, Cell, RenameEdit, RowAction, cell_of, item_of, visible_rows};
use super::tile;
use crate::thumbs::Thumbs;
use crate::viewmodel::RowFilter;

/// Space around a tile's picture beyond the two lines of text it carries: the
/// badge, and the gap between one tile and the next.
///
/// The caption itself is measured rather than assumed — see
/// [`Grid::caption_height`]. This used to be one constant of 38 covering both,
/// which made it the grid's first clipping point the moment the type scale
/// moved.
const CAPTION_EXTRA: f32 = 10.0;
const PADDING: f32 = 8.0;

/// What the grid needs to draw one frame.
pub struct Grid<'a> {
    entries: &'a [FileEntry],
    plan: Option<&'a Plan>,
    plan_index: &'a [Option<usize>],
    selection: &'a mut crate::viewmodel::Selection,
    inline_rename: &'a mut Option<rows::InlineRename>,
    thumbs: &'a mut Thumbs,
    look: tile::Look,
    /// The two text lines a caption needs, measured in `show`.
    ///
    /// A tile's caption is a `Body` name over a `Small` new-name, so its
    /// height is a fact about the current type scale. Carried on the struct
    /// because `cell` has no `Ui` — and because `cell_size` is public and the
    /// benchmark harness calls it after `show` has run.
    caption_text: f32,
    /// Rows that pass the filter, in display order — the *same* list the table
    /// builds, so a row the Changed chip hides has no tile either.
    visible: Vec<usize>,
    /// An entry the keyboard reached, to bring into view once.
    pub scroll_to: Option<usize>,
    /// Set when an inline rename was confirmed with Enter.
    pub rename_confirmed: Option<(usize, String)>,
    /// What the right-click menu asked for.
    pub row_action: Option<RowAction>,
    /// Tiles actually laid out on the last frame, for the performance harness.
    pub laid_out: usize,
}

impl<'a> Grid<'a> {
    #[expect(clippy::too_many_arguments, reason = "a view over the whole session")]
    pub fn new(
        entries: &'a [FileEntry],
        plan: Option<&'a Plan>,
        plan_index: &'a [Option<usize>],
        selection: &'a mut crate::viewmodel::Selection,
        row_filter: RowFilter,
        inline_rename: &'a mut Option<rows::InlineRename>,
        thumbs: &'a mut Thumbs,
        look: tile::Look,
    ) -> Self {
        let visible = visible_rows(entries, plan, plan_index, row_filter);
        Self {
            entries,
            plan,
            plan_index,
            selection,
            inline_rename,
            thumbs,
            look,
            // Replaced on the first `show`; this is roughly the old constant's
            // worth of text, so a `cell_size` read before then is not nonsense.
            caption_text: 28.0,
            visible,
            scroll_to: None,
            rename_confirmed: None,
            row_action: None,
            laid_out: 0,
        }
    }

    /// One tile's footprint, picture plus caption.
    ///
    /// Public so the performance harness can work out how many tiles a given
    /// window *should* hold rather than being handed a magic number that stops
    /// meaning anything the first time the caption gains a line.
    pub fn cell_size(&self) -> egui::Vec2 {
        self.cell()
    }

    fn cell(&self) -> egui::Vec2 {
        let side = self.look.size as f32;
        egui::vec2(side + PADDING, side + self.caption_text + CAPTION_EXTRA)
    }

    /// A `Body` line over a `Small` line — what a tile actually draws.
    fn caption_height(ui: &egui::Ui) -> f32 {
        ui.text_style_height(&egui::TextStyle::Body) + ui.text_style_height(&egui::TextStyle::Small)
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.caption_text = Self::caption_height(ui);
        let cell = self.cell();
        let count = self.visible.len();

        egui::ScrollArea::vertical()
            .id_salt("renameit_grid")
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let columns = ((ui.available_width() / cell.x).floor() as usize).max(1);
                let rows = count.div_ceil(columns);
                // Claimed before anything is drawn, so the scrollbar knows how
                // far it goes without every tile having been laid out.
                ui.set_height(rows as f32 * cell.y);

                // The grid is hand-virtualized, so a tile the keyboard reached
                // may never have been laid out — `scroll_to_me` needs a widget
                // and there is none. Scrolling to its computed rectangle works
                // whether or not it was drawn.
                if let Some(entry) = self.scroll_to
                    && let Some(at) = self.visible.iter().position(|&i| i == entry)
                {
                    let row = at / columns;
                    ui.scroll_to_rect(
                        egui::Rect::from_min_size(
                            ui.min_rect().min + egui::vec2(0.0, row as f32 * cell.y),
                            cell,
                        ),
                        None,
                    );
                }

                let first = (viewport.min.y / cell.y).floor().max(0.0) as usize;
                let last = ((viewport.max.y / cell.y).ceil() as usize + 1).min(rows);
                if first >= last {
                    return;
                }

                // Ask for the pictures of the rows about to be drawn, and only
                // those — the same bound `TableDelegate::prepare` puts on the
                // list, reached differently because there is no delegate here.
                let ppp = ui.pixels_per_point();
                let on_screen: Vec<usize> = self.visible
                    [(first * columns).min(count)..(last * columns).min(count)]
                    .to_vec();
                let wanted: Vec<_> = on_screen
                    .iter()
                    .filter_map(|&index| self.entries.get(index))
                    .filter_map(|entry| tile::key_for(entry, self.look, ppp))
                    .collect();
                self.thumbs.want(wanted);
                self.laid_out = on_screen.len();

                let top = first as f32 * cell.y;
                // Placed at the scroll offset rather than drawn from the top:
                // the rows above are never built, which is the whole point.
                let placed = egui::Rect::from_min_size(
                    ui.min_rect().min + egui::vec2(0.0, top),
                    egui::vec2(ui.available_width(), (last - first) as f32 * cell.y),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(placed), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                        for index in on_screen {
                            ui.allocate_ui(cell, |ui| self.tile(ui, index));
                        }
                    });
                });
            });
    }

    fn tile(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        let item: Option<&PlanItem> = item_of(self.entries, self.plan, self.plan_index, index);
        let cell = cell_of(item);

        if self.selection.contains(index) {
            ui.painter().rect_filled(
                ui.max_rect(),
                2.0,
                ui.visuals().selection.bg_fill.gamma_multiply(0.35),
            );
        }
        // The same rule the table uses, from the same place: a row that is only
        // *acted* on must not be faded for looking unchanged.
        if cell.is_dimmed() {
            ui.style_mut().visuals.override_text_color = Some(ui.visuals().weak_text_color());
        }

        ui.vertical(|ui| {
            let picture = tile::picture(ui, self.thumbs, entry, self.look);
            self.caption(ui, index, entry, cell);

            if picture.secondary_clicked() && !self.selection.contains(index) {
                self.selection.set([index]);
            }
            // Collected inside the closure: the menu is open on one row in
            // one frame in a thousand, and a 10 000-row selection copied for
            // every visible row of every other frame is the waste this avoids.
            let selection = &*self.selection;
            let mut asked = None;
            picture.context_menu(|ui| {
                let selected: Vec<usize> = selection.iter().collect();
                asked = rows::row_menu(ui, index, &selected);
            });
            if asked.is_some() {
                self.row_action = asked;
            }

            if picture.clicked() {
                let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
                self.selection.click(index, ctrl, shift, &self.visible);
            }
            // The same gesture as in the list (P77). The alternative — a
            // double-click that navigates into a folder here and renames there —
            // is one gesture meaning two things depending on which button is
            // lit, which is worse than a feature left out.
            if picture.double_clicked() {
                *self.inline_rename = Some(crate::panels::rows::InlineRename::opening(
                    index,
                    &entry.file_name,
                ));
            }
        });
    }

    /// The two names under the picture.
    ///
    /// Both, because a tile that showed only the current name would make the
    /// grid a viewer rather than a renamer — the whole point of the New name
    /// column is that you can see what the run is about to write before it
    /// writes it.
    fn caption(&mut self, ui: &mut egui::Ui, index: usize, entry: &FileEntry, cell: Cell<'_>) {
        let width = self.look.size as f32;
        ui.set_max_width(width);

        if let Some(edit) = self.inline_rename.as_mut()
            && edit.index == index
        {
            match rows::inline_rename_field(ui, edit) {
                RenameEdit::Confirmed => self.rename_confirmed = Some((index, edit.text.clone())),
                RenameEdit::Cancelled => *self.inline_rename = None,
                RenameEdit::Editing => {}
            }
        } else {
            ui.add(egui::Label::new(&entry.file_name).truncate());
        }

        // Smaller than the name it is about, so a tile reads as one thing with
        // a note under it rather than as two names of equal weight.
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Small);
            rows::new_name_cell(ui, entry, cell);
        });
    }
}
