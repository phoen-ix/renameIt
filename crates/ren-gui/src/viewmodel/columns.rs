//! Which columns the file table shows, in what order, and how wide.
//!
//! The Display settings page lets the user choose this, and until now there
//! was nothing for such a page to edit: the table matched
//! on `cell.col_nr` against a fixed array of four, so "column 2" meant "size"
//! in three different functions and nowhere said so.
//!
//! A descriptor list instead. The table renders whatever is in it, the header
//! reads its label out of it, and a settings page can reorder and hide entries
//! without any of those three knowing about each other.
//!
//! **Not** in here: a column that renders `<tags>` per row. See D124 — that has
//! to be computed by the preview worker with the plan, not by the table on the
//! UI thread, and it is a chunk of its own rather than a variant of this enum.

use serde::{Deserialize, Serialize};

use super::SortColumn;

/// The two things about the table that are not columns — the Display page's
/// Display page has both.
///
/// Here rather than in a module of their own: this file is already what the
/// table's appearance is decided in, and two booleans do not need a third file
/// to live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TableStyle {
    /// > *"Draw gray background on every other row"*
    ///
    /// **On by default** (P91). P70 already
    /// waived "Show Guidelines" on the grounds that this is "the thing that
    /// actually makes a wide row readable" — and now that the name columns
    /// take the window's spare width instead of collapsing to their content,
    /// every row is a wide row.
    pub stripes: bool,
    /// > *"Full Row Select - Normally you must click on the filename to select
    /// > it. This option allows you to click anywhere on the row."*
    ///
    /// Off by default — and it was the only behaviour until now, since only
    /// the Name cell ever sensed a click.
    pub full_row_select: bool,
}

impl Default for TableStyle {
    fn default() -> Self {
        Self {
            stripes: true,
            full_row_select: false,
        }
    }
}

/// What one column shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColumnKind {
    /// A small picture of the file, for the formats `image` can decode.
    ///
    /// > *"In thumbnail mode image files will show as a small preview of the
    /// > image."*
    ///
    /// A column as well as a view (D133): which of the two a single checkbox
    /// ought to mean is genuinely ambiguous, so it is offered as a preference
    /// rather than settled by fiat. Turning it on makes every
    /// row in the table taller, because a row is as tall as its tallest cell.
    Thumbnail,
    /// The name on disk now.
    Name,
    /// What this run does to the row (D38) — a name, an action, or both.
    NewName,
    Size,
    Modified,
    Created,
    /// The extension on its own, which is what a run over mixed files is
    /// usually grouped by.
    Extension,
    /// The containing folder. Worth a column only in Free Select or with
    /// Subfolders on, which is why it is off by default.
    Folder,
}

impl ColumnKind {
    /// Every column there is, in the order a fresh install lists them.
    pub const ALL: [Self; 8] = [
        Self::Thumbnail,
        Self::Name,
        Self::NewName,
        Self::Size,
        Self::Modified,
        Self::Created,
        Self::Extension,
        Self::Folder,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Thumbnail => "Thumb",
            Self::Name => "Name",
            Self::NewName => "New name",
            Self::Size => "Size",
            Self::Modified => "Modified",
            Self::Created => "Created",
            Self::Extension => "Ext",
            Self::Folder => "Folder",
        }
    }

    /// How this column sorts the *listing* — not the view (D28: the order on
    /// screen is the order the run happens in).
    ///
    /// `NewName` is deliberately absent. Its header is a one-shot reorder
    /// command rather than a sort mode (D119), and giving it a `SortColumn`
    /// here is exactly the mistake that made it sort by the old name.
    pub fn sort(self) -> Option<SortColumn> {
        match self {
            Self::Name => Some(SortColumn::Name),
            Self::Size => Some(SortColumn::Size),
            Self::Modified => Some(SortColumn::Modified),
            Self::Created => Some(SortColumn::Created),
            Self::Extension => Some(SortColumn::Extension),
            Self::Folder => Some(SortColumn::Folder),
            // Neither of these is a *listing* order. `NewName`'s header is a
            // one-shot reorder command (D119), and there is no order a picture
            // puts a listing in.
            Self::NewName | Self::Thumbnail => None,
        }
    }

    /// What the column may be resized to, and — through
    /// `egui_table::Column::auto_size` — how it shares out spare width.
    ///
    /// The two name columns are uncapped and everything else is not, which is
    /// the whole trick: `auto_size` hands the slack to whichever columns still
    /// have room, so a wider window widens the names rather than stretching
    /// `18.9 MB` across two hundred points.
    ///
    /// Minimums are "enough to still mean something", not "enough for the
    /// longest value" — a date that has to be truncated is a signal to widen
    /// the column, whereas one that cannot be narrowed steals room from the
    /// name, which is what the user is actually reading.
    pub(crate) fn width_range(self) -> std::ops::RangeInclusive<f32> {
        match self {
            // A picture is the size it was asked for; stretching the column
            // just adds space around it.
            Self::Thumbnail => 120.0..=120.0,
            Self::Name | Self::NewName => 140.0..=f32::INFINITY,
            // Long paths, and the one other column worth growing.
            Self::Folder => 140.0..=f32::INFINITY,
            Self::Size => 64.0..=120.0,
            // The maximum is "a full timestamp, and no more" — it moved with
            // the type scale, because `2026-08-22 14:30` is what has to fit.
            Self::Modified | Self::Created => 118.0..=196.0,
            Self::Extension => 52.0..=110.0,
        }
    }

    fn default_width(self) -> f32 {
        match self {
            // Wide enough for the largest bucket the slider reaches, so turning
            // the column on does not immediately need a drag to be useful.
            Self::Thumbnail => 120.0,
            Self::Name => 260.0,
            Self::NewName => 300.0,
            Self::Size => 90.0,
            Self::Modified => 150.0,
            Self::Created => 150.0,
            Self::Extension => 70.0,
            Self::Folder => 220.0,
        }
    }

    /// The four the app has always shown. The other four are real columns a
    /// user can turn on, not placeholders — they are off because a table that
    /// arrives eight columns wide is a table nobody reads, and because the
    /// thumbnail column costs a decode per visible row.
    fn shown_by_default(self) -> bool {
        matches!(
            self,
            Self::Name | Self::NewName | Self::Size | Self::Modified
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Column {
    pub kind: ColumnKind,
    pub width: f32,
    pub visible: bool,
}

impl Default for Column {
    fn default() -> Self {
        Self::of(ColumnKind::Name)
    }
}

impl Column {
    fn of(kind: ColumnKind) -> Self {
        Self {
            kind,
            width: kind.default_width(),
            visible: kind.shown_by_default(),
        }
    }
}

/// The table's columns, in display order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Columns(Vec<Column>);

impl Default for Columns {
    fn default() -> Self {
        Self(ColumnKind::ALL.map(Column::of).to_vec())
    }
}

impl Columns {
    /// Just the ones on screen, which is what the table is indexed by.
    pub fn visible(&self) -> Vec<&Column> {
        self.0.iter().filter(|c| c.visible).collect()
    }

    /// Every column, shown or not — what a settings page edits.
    pub fn all(&self) -> &[Column] {
        &self.0
    }

    pub fn all_mut(&mut self) -> &mut Vec<Column> {
        &mut self.0
    }

    /// Moves the column at `index` one place towards the front.
    pub fn move_up(&mut self, index: usize) {
        if index > 0 && index < self.0.len() {
            self.0.swap(index - 1, index);
        }
    }

    pub fn move_down(&mut self, index: usize) {
        if index + 1 < self.0.len() {
            self.0.swap(index, index + 1);
        }
    }

    /// Puts back any column a settings file predates, and drops any it no
    /// longer knows about.
    ///
    /// A stored list is data from an older build, so it can be missing an entry
    /// this build added or carry one this build removed. Without this the table
    /// would silently lose a column on upgrade — and, worse, `visible()` would
    /// index into a list of a different length than the header expects.
    pub fn reconcile(&mut self) {
        self.0.retain(|c| ColumnKind::ALL.contains(&c.kind));
        for kind in ColumnKind::ALL {
            if !self.0.iter().any(|c| c.kind == kind) {
                self.0.push(Column::of(kind));
            }
        }
        // Name is the row's identity; a table without it is a list of sizes.
        if !self.0.iter().any(|c| c.visible)
            && let Some(name) = self.0.iter_mut().find(|c| c.kind == ColumnKind::Name)
        {
            name.visible = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_install_shows_the_four_the_app_always_had() {
        let columns = Columns::default();
        let visible: Vec<_> = columns.visible().iter().map(|c| c.kind).collect();
        assert_eq!(
            visible,
            [
                ColumnKind::Name,
                ColumnKind::NewName,
                ColumnKind::Size,
                ColumnKind::Modified
            ]
        );
        assert_eq!(
            columns.all().len(),
            8,
            "the other four are there to turn on"
        );
    }

    /// The New name header is a reorder command (D119), so it has no sort
    /// column — the mistake this guards against is the one that shipped. A
    /// thumbnail has none either, because there is no order a picture puts a
    /// listing in.
    ///
    /// Written as an equality rather than as "some columns do not sort", so it
    /// stays exactly as sharp as it was: a column that quietly loses its sort
    /// still fails here.
    #[test]
    fn every_column_but_the_two_that_cannot_sorts() {
        for kind in ColumnKind::ALL {
            assert_eq!(
                kind.sort().is_none(),
                matches!(kind, ColumnKind::NewName | ColumnKind::Thumbnail),
                "{kind:?}"
            );
        }
    }

    /// A settings file written by an older build is missing whatever this one
    /// added. Without reconciling, the header and the body would disagree about
    /// how many columns there are.
    #[test]
    fn a_stored_list_from_an_older_build_gains_what_it_is_missing() {
        let mut columns = Columns(vec![Column::of(ColumnKind::Name)]);
        columns.reconcile();
        assert_eq!(columns.all().len(), 8);
        assert_eq!(columns.all()[0].kind, ColumnKind::Name, "order is kept");
    }

    #[test]
    fn hiding_every_column_leaves_the_name() {
        let mut columns = Columns::default();
        for column in columns.all_mut() {
            column.visible = false;
        }
        columns.reconcile();
        assert_eq!(
            columns.visible().iter().map(|c| c.kind).collect::<Vec<_>>(),
            [ColumnKind::Name]
        );
    }

    #[test]
    fn columns_reorder_and_stop_at_the_ends() {
        let mut columns = Columns::default();
        columns.move_up(0);
        assert_eq!(
            columns.all()[0].kind,
            ColumnKind::Thumbnail,
            "already first"
        );
        columns.move_down(0);
        assert_eq!(columns.all()[0].kind, ColumnKind::Name);
        assert_eq!(columns.all()[1].kind, ColumnKind::Thumbnail);
        columns.move_down(7);
        assert_eq!(columns.all().len(), 8, "the last one has nowhere to go");
    }
}

#[cfg(test)]
mod style_tests {
    use super::*;

    /// Shading on, full-row select off.
    ///
    /// Both shipped off until the name columns stopped collapsing to their
    /// content and started taking the window's spare width. P70 waived
    /// "Show Guidelines" on the grounds that row
    /// shading is "the thing that actually makes a wide row readable"; P91
    /// finishes that thought by having it on. Full-row select stays off —
    /// it changes what a click *does*, which is not a legibility default.
    #[test]
    fn the_table_style_ships_with_shading_on() {
        let style = TableStyle::default();
        assert!(style.stripes);
        assert!(!style.full_row_select);
    }
}
