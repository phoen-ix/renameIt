//! The file list — screen S3 of `docs/DESIGN.md` Part 2 §3.
//!
//! Virtualized through `egui_table`, so a 100 000-file listing costs the
//! renderer exactly what a 1 000-file one does (Spike B measured 132 cells laid
//! out either way). Everything expensive — the diff spans, the conflict
//! tooltips — is computed for **visible rows only**.

use ren_core::model::FileEntry;
use ren_core::{Plan, PlanItem};

use super::rows::{self, RenameEdit, RowAction, cell_of, item_of, new_name_cell, visible_rows};
use super::tile;
use crate::thumbs::Thumbs;
use crate::viewmodel::{Column, ColumnKind, Columns, RowFilter, SortColumn, TableStyle};

/// What the table needs to draw one frame.
/// Breathing room above and below a row's text.
///
/// With `Body` at 13 this reproduced the old hardcoded row height of 20; the
/// point is that it now moves with the type scale instead of being a number
/// that happened to fit one of them.
const ROW_PADDING: f32 = 2.5;

/// What the header needs beyond a row: its label is a frameless `Button`, so
/// it carries `button_padding` as well.
const HEADER_EXTRA: f32 = 3.0;

/// Horizontal breathing room inside a cell.
///
/// `egui_table` allocates a cell at exactly the column rect, so without this
/// two columns' text touch.
const CELL_GUTTER: f32 = 8.0;

/// The rows a drag is carrying.
///
/// A newtype, as `panels::operation`'s own payload comment predicted:
/// `DragAndDrop` matches on the type, so a bare `Vec<usize>` would let a
/// dragged operation card land on a file row.
#[derive(Debug, Clone)]
struct RowDrag(Vec<usize>);

pub struct FileTable<'a> {
    pub entries: &'a [FileEntry],
    /// The plan, if the worker has delivered one.
    pub plan: Option<&'a Plan>,
    /// Entry index → index into `plan.items`, for rows that are in the run.
    pub plan_index: &'a [Option<usize>],
    pub selection: &'a mut crate::viewmodel::Selection,
    pub row_filter: RowFilter,
    pub sort: crate::viewmodel::Sort,
    /// An entry the keyboard reached, to bring into view once.
    pub scroll_to: Option<usize>,
    /// Row currently being renamed with F2, and the text being edited.
    pub inline_rename: &'a mut Option<rows::InlineRename>,
    /// The decode threads and the texture cache, for the Thumb column.
    thumbs: &'a mut Thumbs,
    /// How big a thumbnail is drawn, and whether it gets a border.
    thumb: tile::Look,
    /// The screen's scale, read once in `show`.
    ///
    /// `prepare` decides which pictures to ask for and is handed no `Ui`, so
    /// the one thing it needs from the screen has to be carried here — and
    /// carrying it is what keeps the request and the lookup naming the same
    /// size on a 2x display.
    points_per_pixel: f32,
    /// How tall one line of `Body` text is, read once in `show`.
    ///
    /// Carried here for the same reason `points_per_pixel` is:
    /// `default_row_height` is a `TableDelegate` method and is handed no `Ui`,
    /// so a metric it needs has to be measured where there is one.
    row_text_height: f32,
    /// The columns on screen this frame, in order.
    shown: Vec<Column>,
    style: TableStyle,
    /// Set by the table when the user asks to sort by a column.
    pub sort_request: Option<SortColumn>,
    /// Set when the New name header was clicked — a reorder command, not a
    /// sort (D119), so it travels separately.
    pub reorder_request: bool,
    /// A drag that landed: the entries to move, and where they go.
    pub move_request: Option<(Vec<usize>, usize)>,
    /// Set when an inline rename was confirmed with Enter.
    pub rename_confirmed: Option<(usize, String)>,
    /// What the right-click menu asked for, read once the table is done.
    pub row_action: Option<RowAction>,
    /// Rows that pass `row_filter`, in display order.
    visible: Vec<usize>,
}

impl<'a> FileTable<'a> {
    #[expect(clippy::too_many_arguments, reason = "a view over the whole session")]
    pub fn new(
        entries: &'a [FileEntry],
        plan: Option<&'a Plan>,
        plan_index: &'a [Option<usize>],
        selection: &'a mut crate::viewmodel::Selection,
        row_filter: RowFilter,
        sort: crate::viewmodel::Sort,
        columns: &'a Columns,
        style: TableStyle,
        inline_rename: &'a mut Option<rows::InlineRename>,
        thumbs: &'a mut Thumbs,
        thumb: tile::Look,
    ) -> Self {
        let visible = visible_rows(entries.len(), plan, plan_index, row_filter);

        Self {
            entries,
            plan,
            plan_index,
            selection,
            row_filter,
            sort,
            // Set by the caller after construction rather than taken as a
            // twelfth positional argument, which nobody could read.
            scroll_to: None,
            // Snapshotted, so the header, the body and `egui_table`'s own
            // column array cannot disagree about how many there are inside one
            // frame. That is what `col_nr` indexes into.
            shown: columns.visible().into_iter().cloned().collect(),
            style,
            inline_rename,
            sort_request: None,
            reorder_request: false,
            move_request: None,
            rename_confirmed: None,
            row_action: None,
            visible,
            thumbs,
            thumb,
            points_per_pixel: 1.0,
            row_text_height: 0.0,
        }
    }

    /// Whether the Thumb column is on screen this frame.
    fn shows_thumbnails(&self) -> bool {
        self.shown.iter().any(|c| c.kind == ColumnKind::Thumbnail)
    }

    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.points_per_pixel = ui.pixels_per_point();
        self.row_text_height = ui.text_style_height(&egui::TextStyle::Body);
        let columns: Vec<egui_table::Column> = self
            .shown
            .iter()
            .map(|c| {
                egui_table::Column::new(c.width)
                    .resizable(true)
                    .range(c.kind.width_range())
                    // Keyed by *kind*, not by position. `egui_table` defaults
                    // to `Id::new(col_idx)`, so turning a column off in
                    // Settings > Display used to hand the next column along
                    // its predecessor's width.
                    .id(egui::Id::new(("file_table_column", c.kind)))
            })
            .collect();

        let mut table = egui_table::Table::new()
            .id_salt("renameit_file_table")
            .num_rows(self.visible.len() as u64)
            // Without this the columns are laid out once, by a content
            // sizing pass on the first frame, and never grow again: `Name`
            // came out at 85 pt and `New name` at 64 pt while 1 230 pt of the
            // window — two thirds of it — sat empty to their right. `Never` is
            // `egui_table`'s default and it is the wrong one for a table whose
            // point is a before/after pair of filenames.
            //
            // `Always` rather than `OnParentResize`, which does not work here:
            // the content sizing pass runs *after* the auto-size on the same
            // frame and stores what it measured, and `OnParentResize` then
            // declines to run again until the pane changes width — so the
            // collapsed widths are what stick. `Always` re-spreads the stored
            // widths over the pane every frame, which also means a column the
            // user drags wider takes its extra points from the columns that
            // have room rather than from the right-hand margin.
            .auto_size_mode(egui_table::AutoSizeMode::Always)
            .columns(columns)
            .headers(vec![egui_table::HeaderRow::new(self.header_height())]);
        // `None` rather than `Align::Center`: minimal scroll, so a lead that is
        // already on screen does not make the whole list lurch on every arrow
        // press. The index is an *entry*; the table counts visible rows.
        if let Some(entry) = self.scroll_to
            && let Some(row) = self.visible.iter().position(|&i| i == entry)
        {
            table = table.scroll_to_row(row as u64, None);
        }
        table.show(ui, self);

        // An empty table is column dividers ruled the full height of the panel
        // and nothing else, which reads as a table that failed rather than a
        // folder with nothing in it. The message is painted over the empty
        // area rather than replacing the table, so the headers stay put and
        // stay clickable.
        if self.visible.is_empty() {
            self.empty_state(ui);
        }
    }

    /// Why the list is empty, in the words that name the way out of it.
    fn empty_state(&self, ui: &mut egui::Ui) {
        let (what, how) = if self.entries.is_empty() {
            (
                "Nothing here",
                "Choose a folder above, or drag files in from your file manager.",
            )
        } else {
            // There *are* files; a filter is hiding all of them, and saying so
            // is the difference between "empty folder" and "you cannot see your
            // files" (P63's principle: never let rows vanish quietly).
            (
                "Every row is filtered out",
                "The Files/Folders switches, the pattern box and the All/Changed/Conflicts \
                 chips each hide rows.",
            )
        };

        let mut area = ui.max_rect();
        area.min.y += self.header_height() + CELL_GUTTER; // below the header row
        if area.height() < 48.0 {
            return;
        }
        let painter = ui.painter_at(area);
        let centre = area.center();
        let heading = painter.layout(
            what.to_owned(),
            egui::TextStyle::Heading.resolve(ui.style()),
            ui.visuals().weak_text_color(),
            area.width() - 4.0 * CELL_GUTTER,
        );
        let body = painter.layout(
            how.to_owned(),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().weak_text_color(),
            (area.width() - 4.0 * CELL_GUTTER).min(420.0),
        );
        let total = heading.size().y + CELL_GUTTER + body.size().y;
        let top = centre.y - total / 2.0;
        painter.galley(
            egui::pos2(centre.x - heading.size().x / 2.0, top),
            heading,
            egui::Color32::PLACEHOLDER,
        );
        painter.galley(
            egui::pos2(centre.x - body.size().x / 2.0, top + total - body.size().y),
            body,
            egui::Color32::PLACEHOLDER,
        );
    }

    /// The header row, which carries a frameless `Button` and so needs its
    /// padding on top of the text.
    fn header_height(&self) -> f32 {
        self.row_text_height + 2.0 * ROW_PADDING + 2.0 * HEADER_EXTRA
    }

    fn entry_index(&self, row: u64) -> Option<usize> {
        self.visible.get(row as usize).copied()
    }

    fn item(&self, entry_index: usize) -> Option<&'a PlanItem> {
        item_of(self.plan, self.plan_index, entry_index)
    }
}

impl egui_table::TableDelegate for FileTable<'_> {
    /// Asks for the thumbnails of the rows about to be drawn, and only those.
    ///
    /// This is what makes a folder of ten thousand photographs cost a
    /// screenful of decodes rather than ten thousand. `egui_table` calls it
    /// before any `cell_ui`, which is the one moment where the visible range is
    /// known and nothing has been drawn yet.
    fn prepare(&mut self, info: &egui_table::PrefetchInfo) {
        if !self.shows_thumbnails() {
            return;
        }
        let look = self.thumb;
        let ppp = self.points_per_pixel;
        let wanted: Vec<_> = info
            .visible_rows
            .clone()
            .filter_map(|row| self.entry_index(row))
            .filter_map(|index| self.entries.get(index))
            .filter_map(|entry| tile::key_for(entry, look, ppp))
            .collect();
        self.thumbs.want(wanted);
    }

    /// A row is as tall as its tallest cell, so the Thumb column sets the
    /// height of the whole table while it is on.
    ///
    /// Otherwise it follows the text. This used to be a bare `20.0` with no
    /// metric behind it, which left about four points of headroom at the old
    /// `Body` size of 13 — enough that raising the type scale would have
    /// sliced the glyphs, and `egui_table` hard-clips a cell. `Truncate`
    /// handles the horizontal case; there is no vertical equivalent.
    fn default_row_height(&self) -> f32 {
        if self.shows_thumbnails() {
            self.thumb.size as f32 + 4.0
        } else {
            self.row_text_height + 2.0 * ROW_PADDING
        }
    }

    fn header_cell_ui(&mut self, ui: &mut egui::Ui, cell: &egui_table::HeaderCellInfo) {
        let Some(column) = self.shown.get(cell.col_range.start) else {
            return;
        };
        let kind = column.kind;
        // The New name header owns the arrow while the order is hand-set, and
        // no other header does — the listing is in neither column's order then.
        // The New name header owns the arrow only while the hand-set order is
        // **its own**. After a row drag the listing is in nobody's column
        // order, so no header claims it — `move_rows` sets `manual` and leaves
        // `column` alone, while `reorder_by_new_names` sets both.
        let active = if kind == ColumnKind::NewName {
            self.sort.manual && !self.sort.dragged
        } else {
            !self.sort.manual && kind.sort() == Some(self.sort.column)
        };
        let arrow = if active {
            if self.sort.ascending { " ▲" } else { " ▼" }
        } else {
            ""
        };
        let label = egui::RichText::new(format!("{}{arrow}", kind.label())).strong();
        let button = ui.add(egui::Button::new(label).frame(false));
        if kind == ColumnKind::NewName {
            button
                .on_hover_text(
                    "Reorder the list by the names it is about to write — once. The list order \
                     is the run order, so a counter added afterwards numbers them this way. \
                     Click again to reverse.",
                )
                .clicked()
                .then(|| self.reorder_request = true);
            return;
        }
        if button.clicked()
            && let Some(sort) = kind.sort()
        {
            self.sort_request = Some(sort);
        }
    }

    fn row_ui(&mut self, ui: &mut egui::Ui, row: u64) {
        // Selecting a row is what scopes the run, so the whole row is the
        // click target rather than a checkbox column.
        let Some(index) = self.entry_index(row) else {
            return;
        };
        let rect = ui.max_rect();

        // *"Draw gray background on every other row."* Painted before the
        // selection, so a selected stripe still reads as selected.
        if self.style.stripes && row % 2 == 1 {
            ui.painter()
                .rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
        }
        if self.selection.contains(index) {
            ui.painter().rect_filled(
                rect,
                0.0,
                ui.visuals().selection.bg_fill.gamma_multiply(0.35),
            );
        }
        // Where the keyboard is. Inset inside the row's own rect, because the
        // next row paints afterwards and would cover a line drawn on the shared
        // edge. It has no accessible node of its own — a focus ring is paint —
        // so the tests assert `selection.lead` and `docs/manual-checks.md`
        // carries the ring itself, beside Full Row Select which is there for
        // the same reason.
        if self.selection.lead == Some(index) {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                0.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }

        // > *"You can drag files up and down in the listview to change the
        // > file order and thus the enumeration index for the file."*
        //
        // **The whole row is the drag source, and the cells keep their clicks.**
        // egui resolves click hits and drag hits independently, so a
        // click-sensing widget inside a drag-sensing one yields *both* — the
        // Name label keeps its click, its double-click and its menu. That is
        // the opposite of the Full Row Select case below, which had to move
        // into the cells because a *click* must be topmost.
        let drag = ui.interact(rect, ui.id().with(("row_drag", index)), egui::Sense::drag());
        if drag.drag_started() {
            // Dragging one of several selected rows takes all of them, which is
            // the gesture every file manager has. Dragging an unselected row
            // takes it alone and leaves the selection be — changing it mid-drag
            // would re-scope the run and re-plan under the pointer.
            let rows: Vec<usize> = if self.selection.contains(index) {
                self.selection.iter().collect()
            } else {
                vec![index]
            };
            egui::DragAndDrop::set_payload(ui.ctx(), RowDrag(rows));
        }
        if egui::DragAndDrop::has_payload_of_type::<RowDrag>(ui.ctx()) && drag.contains_pointer() {
            // Which half of the row the pointer is over decides whether the
            // files land in front of it or behind it.
            let after = ui
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|p| p.y > rect.center().y);
            // Inset one pixel inside this row's own rect: the next row paints
            // afterwards and would cover a line drawn on the shared edge.
            let y = if after {
                rect.bottom() - 1.0
            } else {
                rect.top() + 1.0
            };
            ui.painter()
                .hline(rect.x_range(), y, ui.visuals().selection.stroke);

            if let Some(payload) = drag.dnd_release_payload::<RowDrag>() {
                // **`index`, not `row`.** `index` is the entry; `row` is the
                // position in the *visible* list. With a row filter on they are
                // different numbers and the wrong file moves — silently,
                // because with the filter off they are equal.
                //
                // **No test covers this line**, and swapping it for `row`
                // leaves the suite green: the harness drives the accessibility
                // tree and a drag is a pointer gesture, so
                // `a_row_dropped_while_the_filter_hides_others_lands_where_it_
                // looks_like_it_lands` exercises `move_rows` directly and
                // proves only that *it* takes entry indices. The call site is a
                // `docs/manual-checks.md` row, said here rather than implied by
                // a comment that reads like a guarantee.
                self.move_request = Some((payload.0.clone(), index + usize::from(after)));
            }
        }

        // *"This option allows you to click anywhere on the row."* The name
        // cell is drawn after this and sits on top, so it still wins where the
        // two overlap — which is what keeps double-click-to-rename working.
        if self.style.full_row_select {
            let response = ui.interact(rect, ui.id().with(("row", index)), egui::Sense::click());
            if response.clicked() {
                let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
                self.selection.click(index, ctrl, shift, &self.visible);
            }
        }
    }

    fn cell_ui(&mut self, ui: &mut egui::Ui, cell: &egui_table::CellInfo) {
        let Some(index) = self.entry_index(cell.row_nr) else {
            return;
        };
        let entry = &self.entries[index];
        let cell_kind = cell_of(self.item(index));

        // Rows the run leaves alone are dimmed so the ones it touches stand
        // out. Reads the classified cell rather than `RowState`, so a row that
        // is only *acted* on is not dimmed for looking unchanged.
        if cell_kind.is_dimmed() {
            ui.style_mut().visuals.override_text_color = Some(ui.visuals().weak_text_color());
        }

        let Some(kind) = self.shown.get(cell.col_nr).map(|c| c.kind) else {
            return;
        };

        // `egui_table` gives a cell the column rect with no inner margin and
        // hard-clips it, and sets `TextWrapMode::Extend` for the whole table —
        // so before this, `renameit.exe` ran straight into `unchanged` into
        // `18.9 MB` with no gap anywhere, and each was sliced mid-glyph at the
        // column edge with nothing to say it had been. A gutter separates the
        // columns; `Truncate` is what puts the ellipsis on.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.add_space(CELL_GUTTER);

        // *"Full Row Select — … allows you to click anywhere on the row."*
        //
        // Registered per **cell** rather than once over the whole row: the row
        // is painted before the cells, so an interact there is covered by them
        // and never sees a click.
        // The Name and Thumb cells are skipped because each already senses its
        // own click — and their double-click is what opens the editor.
        if self.style.full_row_select && !matches!(kind, ColumnKind::Name | ColumnKind::Thumbnail) {
            let response = ui.interact(
                ui.max_rect(),
                ui.id().with(("row", index, cell.col_nr)),
                egui::Sense::click(),
            );
            if response.clicked() {
                let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
                self.selection.click(index, ctrl, shift, &self.visible);
            }
        }

        match kind {
            ColumnKind::Thumbnail => {
                let look = self.thumb;
                let response = tile::picture(ui, self.thumbs, entry, look);
                if response.clicked() {
                    let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
                    self.selection.click(index, ctrl, shift, &self.visible);
                }
                if response.double_clicked() {
                    *self.inline_rename = Some(crate::panels::rows::InlineRename::opening(
                        index,
                        &entry.file_name,
                    ));
                }
            }
            ColumnKind::Name => self.name_cell(ui, index, entry),
            ColumnKind::NewName => new_name_cell(ui, entry, cell_kind),
            // A folder has no size worth showing: the number the filesystem
            // reports for one is its own directory entry, not its contents.
            // Right-aligned, because a column of sizes is read by comparing
            // magnitudes and ragged-right digits do not line up.
            ColumnKind::Size => {
                if !entry.is_dir {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(CELL_GUTTER);
                        ui.label(human_size(entry.size));
                    });
                }
            }
            ColumnKind::Modified => {
                if let Some(modified) = entry.modified {
                    ui.label(human_time(modified));
                }
            }
            ColumnKind::Created => {
                if let Some(created) = entry.created {
                    ui.label(human_time(created));
                }
            }
            ColumnKind::Extension => {
                if let Some(ext) = ren_core::split_file_name(&entry.file_name).1 {
                    ui.label(ext);
                }
            }
            ColumnKind::Folder => {
                if let Some(parent) = entry.path.parent() {
                    let text = parent.to_string_lossy().into_owned();
                    // The full path in a 220px cell is the tail, not the head.
                    ui.label(egui::RichText::new(shorten_front(&text, 40)))
                        .on_hover_text(text);
                }
            }
        }
    }
}

impl FileTable<'_> {
    fn name_cell(&mut self, ui: &mut egui::Ui, index: usize, entry: &FileEntry) {
        // F2 turns this cell into a text box.
        if let Some(edit) = self.inline_rename.as_mut()
            && edit.index == index
        {
            match rows::inline_rename_field(ui, edit) {
                RenameEdit::Confirmed => self.rename_confirmed = Some((index, edit.text.clone())),
                RenameEdit::Cancelled => *self.inline_rename = None,
                RenameEdit::Editing => {}
            }
            return;
        }

        let icon = if entry.is_dir { "📁 " } else { "" };
        let response = ui.add(
            egui::Label::new(format!("{icon}{}", entry.file_name)).sense(egui::Sense::click()),
        );

        // The menu hangs off the name, which is already this row's click
        // target. A right-click on an unselected row selects it first, or
        // "Copy the selection" would copy something else entirely.
        if response.secondary_clicked() && !self.selection.contains(index) {
            self.selection.set([index]);
        }
        let selected: Vec<usize> = self.selection.iter().collect();
        let mut asked = None;
        response.context_menu(|ui| asked = rows::row_menu(ui, index, &selected));
        if asked.is_some() {
            self.row_action = asked;
        }

        if response.clicked() {
            let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
            self.selection.click(index, ctrl, shift, &self.visible);
        }
        if response.double_clicked() {
            *self.inline_rename = Some(crate::panels::rows::InlineRename::opening(
                index,
                &entry.file_name,
            ));
        }
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `YYYY-MM-DD hh:mm`, in **local** time.
///
/// Local because everything else is. This column used to do its own
/// days-since-epoch arithmetic and render UTC, while every date the engine puts
/// into a filename goes through `chrono::Local` (P48). In any zone but UTC the
/// same file therefore read one time in the list and renamed to another — and
/// the gap moved by an hour across a daylight-saving boundary, which is exactly
/// a genuine confusion, and one the spec listed
/// as a thing for us to get right.
fn human_time(time: std::time::SystemTime) -> String {
    human_time_in(&chrono::Local, time)
}

/// The same, in a zone you name.
///
/// Split out so the formatting can be *tested* against a zone that is not the
/// machine's. On a UTC build agent — which this one is — `Local` and `Utc` are
/// the same thing, so a test written against `Local` cannot tell a correct
/// implementation from the UTC-hardcoded one this replaced. Handing the zone in
/// makes the offset observable everywhere.
fn human_time_in<Tz>(tz: &Tz, time: std::time::SystemTime) -> String
where
    Tz: chrono::TimeZone,
    Tz::Offset: std::fmt::Display,
{
    let utc: chrono::DateTime<chrono::Utc> = time.into();
    tz.from_utc_datetime(&utc.naive_utc())
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// Keeps the **end** of a long path, which is the half that identifies it.
fn shorten_front(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let skip = count - max_chars.saturating_sub(1);
    format!("…{}", text.chars().skip(skip).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_shown_in_readable_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    /// The column honours the zone it is given — which is what "local" means.
    ///
    /// Asserted against an explicit offset rather than against `Local`, because
    /// this agent runs in UTC: a test written against `Local` passes just as
    /// happily with the UTC arithmetic this replaced, and would have certified
    /// the bug. A fixed +05:30 cannot be confused with UTC anywhere.
    #[test]
    fn the_modified_column_renders_in_the_zone_it_is_given() {
        use chrono::TimeZone;
        let india = chrono::FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
        let t = std::time::UNIX_EPOCH;

        assert_eq!(human_time_in(&chrono::Utc, t), "1970-01-01 00:00");
        assert_eq!(human_time_in(&india, t), "1970-01-01 05:30");

        // And a date that rolls over the day boundary because of the offset.
        let late = std::time::UNIX_EPOCH + std::time::Duration::from_secs(20 * 3600);
        assert_eq!(human_time_in(&chrono::Utc, late), "1970-01-01 20:00");
        assert_eq!(human_time_in(&india, late), "1970-01-02 01:30");

        // `human_time` is the same function bound to the machine's zone.
        let expected = chrono::Local
            .timestamp_opt(0, 0)
            .single()
            .expect("a real instant")
            .format("%Y-%m-%d %H:%M")
            .to_string();
        assert_eq!(human_time(t), expected);
    }

    /// The engine and the table must agree about what time it is: `<Date>` and
    /// `<Time>` go through `chrono::Local` (P48), and so does the column now.
    ///
    /// This one only *discriminates* off UTC — on a UTC agent the old buggy
    /// column agreed too. It is the invariant that matters, so it stays; the
    /// test above is the one that bites everywhere.
    #[test]
    fn the_column_agrees_with_the_tag_that_renames_from_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x").unwrap();
        let entry = ren_core::FileEntry::from_path(&path).unwrap();

        let shown = human_time(entry.modified.expect("a modified time"));
        let run = ren_core::RunContext::default();
        let cx = ren_core::ops::EvalCx::new(&entry, 0, 1, &run);
        let rendered = ren_core::TextTemplate::new("<Date> <Time>")
            .render(&cx)
            .expect("the tags compile")
            .text;

        // `<Date>` and `<Time>` default to the *modified* stamp, which is what
        // this column shows, and render `yyyy-mm-dd` and `Hh.Mm.Ss` (D30). Only
        // the overlap is comparable — the column carries no seconds.
        let (date, time) = shown.split_once(' ').unwrap();
        assert!(rendered.starts_with(date), "date: {shown} vs {rendered}");
        assert!(
            rendered.contains(&time.replace(':', ".")),
            "time: {shown} vs {rendered}"
        );
    }
}
