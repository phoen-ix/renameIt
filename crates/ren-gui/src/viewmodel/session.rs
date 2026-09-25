//! What the user is looking at: where the files came from, which are selected,
//! how the table is sorted and filtered.
//!
//! Two source modes: **Browser** renames a whole folder, **Free Select**
//! collects files from
//! anywhere. Both end as a `Vec<FileEntry>`, so everything downstream — the
//! preview, the plan, the executor — is identical.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ren_core::listing::PatternScope;
use ren_core::model::FileEntry;
use ren_core::{ListOptions, PlanItem, RowState};
use ren_platform::Platform;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceMode {
    /// One folder, with the address box and the pattern box narrowing what
    /// is listed from it.
    #[default]
    Browser,
    /// Files and folders collected from anywhere, by drag, by the command
    /// line or by the row menu.
    FreeSelect,
}

/// The All / Changed / Conflicts chip above the table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowFilter {
    #[default]
    All,
    Changed,
    Conflicts,
}

impl RowFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Changed, Self::Conflicts];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Changed => "Changed",
            Self::Conflicts => "Conflicts",
        }
    }

    /// Takes the whole item, not just its state.
    ///
    /// `RowState` is only about the *name*, so `Changed` used to hide every row
    /// a Set Date pipeline touches — silently, and with nothing failing to
    /// compile. A user narrowing to Changed would see an empty table, conclude
    /// nothing was going to happen, and press the button anyway.
    pub fn accepts(self, item: &PlanItem) -> bool {
        match self {
            Self::All => true,
            Self::Changed => item.affected(),
            Self::Conflicts => item.state.is_conflict() || matches!(item.state, RowState::Error(_)),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortColumn {
    #[default]
    Name,
    /// A column in the table, but **not** a sort mode: clicking this header is
    /// a one-shot reorder command (D119). `Sort::column` never holds it — an
    /// old settings file can, which is why the variant stays and why
    /// `sort_entries` still has an arm for it.
    NewName,
    Size,
    Modified,
    Created,
    /// The extension on its own — how a run over mixed files is usually
    /// grouped, and the order a Set Extension pipeline wants to see.
    Extension,
    /// The containing folder, which only says anything with Subfolders on or in
    /// Free Select. Both are exactly when it says a lot.
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sort {
    pub column: SortColumn,
    pub ascending: bool,
    /// True when the order on screen is the one the New-name reorder produced.
    ///
    /// It stops `sort_entries` from re-deriving an order the user set by hand,
    /// and it moves the arrow to the New name header. Cleared by any real sort
    /// and by a relist (`install_listing`), which rebuilds the listing and so
    /// has no hand-set order left to preserve — except the one a run hands on
    /// through `refresh_after_run`. `#[serde(default)]` on the struct is what
    /// keeps a settings file written before M8 parsing.
    pub manual: bool,
    /// True when the hand-set order came from a **row drag** rather than the
    /// New-name command.
    ///
    /// Implies `manual`, and is set in exactly the two places that produce an
    /// order. It exists so no column header claims a dragged listing: after a
    /// drag the rows are in nobody's column order, and leaving `column` at
    /// whatever it was before would keep an arrow pointing at a claim that has
    /// stopped being true. Storing it rather than clearing `column` means the
    /// column the user *had* chosen survives the drag and takes over again when
    /// a refresh re-derives the order.
    pub dragged: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            column: SortColumn::Name,
            ascending: true,
            manual: false,
            dragged: false,
        }
    }
}

/// The parts of a session worth remembering between runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSettings {
    pub mode: SourceMode,
    pub dir: PathBuf,
    pub pattern: String,
    pub files: bool,
    pub folders: bool,
    pub subfolders: bool,
    pub sort: Sort,
    pub row_filter: RowFilter,
    /// Refuse to run inside a folder the operating system owns (D127). A
    /// refusal rather than a filter, because hiding the folder would hide the
    /// reason nothing happens. On by default, and off is a decision the user
    /// makes in Settings ▸ File System.
    pub guard_system_folders: bool,
    /// Show write-protected, hidden and system files and folders. All three
    /// default to showing (D126).
    pub show_hidden: bool,
    pub show_system: bool,
    pub show_read_only: bool,
    /// With both files and folders listed, which of the two the pattern box
    /// narrows.
    pub pattern_applies: PatternScope,
    /// Rows or tiles.
    pub view: ViewMode,
    /// How big a thumbnail is drawn, in points.
    ///
    /// A slider rather than "small, medium or large" (D142). A `u32` rather
    /// than an `f32` because this struct
    /// derives `Eq`, and because a thumbnail size in fractions of a point is a
    /// distinction nobody can see.
    pub thumb_size: u32,
    /// A thin border around each thumbnail, so a picture with a white edge
    /// does not dissolve into the background.
    pub thumb_border: bool,
}

/// Whether the listing is drawn as rows or as tiles.
///
/// Whether thumbnails are a *mode* or a *column* is genuinely ambiguous: a
/// grid of pictures is a way of looking at a folder, and a small picture
/// beside each name is a column like any other. Both are useful for different
/// folders, so the app ships both and the user picks (D133).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    List,
    Grid,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Grid => "Grid",
        }
    }
}

/// What the size slider spans, and where it starts.
///
/// Three named sizes — small, medium and large — turned into a continuous
/// range, with the middle where Medium sat.
pub const THUMB_MIN: u32 = 48;
pub const THUMB_DEFAULT: u32 = 96;
pub const THUMB_MAX: u32 = 256;

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            mode: SourceMode::default(),
            dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            pattern: String::new(),
            files: true,
            folders: false,
            subfolders: false,
            sort: Sort::default(),
            row_filter: RowFilter::default(),
            guard_system_folders: true,
            show_hidden: true,
            show_system: true,
            show_read_only: true,
            pattern_applies: PatternScope::default(),
            view: ViewMode::default(),
            thumb_size: THUMB_DEFAULT,
            thumb_border: false,
        }
    }
}

/// The rows the user picked, and where the keyboard is.
///
/// One type rather than three fields on `Session`, because all three have to
/// follow a permutation together — and three loose fields is the shape where a
/// remap updates one and quietly skips the others.
///
/// `rows` is the run's **scope** (P22): only the selected rows are renamed,
/// and with nothing selected every row is. `lead` deliberately is not — it is
/// where the next arrow key starts from and what F2 opens, so moving it must
/// never re-plan. `anchor` is what a Shift-extend measures from: the last row
/// picked *without* Shift.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Indices into `entries`.
    pub rows: BTreeSet<usize>,
    /// Where the keyboard is. Not part of the scope.
    pub lead: Option<usize>,
    anchor: Option<usize>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn contains(&self, index: usize) -> bool {
        self.rows.contains(&index)
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows.iter().copied()
    }

    /// Replaces the whole selection, putting the keyboard on its first row.
    pub fn set(&mut self, rows: impl IntoIterator<Item = usize>) {
        self.rows = rows.into_iter().collect();
        self.lead = self.rows.iter().next().copied();
        self.anchor = self.lead;
    }

    /// Forgets everything, including where the keyboard was.
    ///
    /// A lead pointing into a listing that no longer exists is worse than none,
    /// because it is where F2 and Enter would act.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.lead = None;
        self.anchor = None;
    }

    /// Moves the keyboard without touching what the run covers.
    pub fn set_lead(&mut self, lead: Option<usize>) {
        self.lead = lead;
        self.anchor = lead;
    }

    /// What a click on `index` does.
    ///
    /// `order` is the rows **on screen, in display order**. A Shift range built
    /// from `min..=max` over entry indices would silently take in the rows the
    /// Changed chip is hiding — and the selection is the run's scope, so those
    /// files would be renamed without ever having been visible.
    pub fn click(&mut self, index: usize, ctrl: bool, shift: bool, order: &[usize]) {
        // Shift first: it is the only one that reads the anchor rather than
        // setting it, and `additive = ctrl || shift` — the reading before M8 —
        // made a Shift-click pick two endpoints and nothing between them.
        if shift && let Some(anchor) = self.anchor {
            let (Some(from), Some(to)) = (
                order.iter().position(|&i| i == anchor),
                order.iter().position(|&i| i == index),
            ) else {
                return;
            };
            let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
            if !ctrl {
                self.rows.clear();
            }
            self.rows.extend(order[lo..=hi].iter().copied());
            self.lead = Some(index);
            return;
        }

        if !ctrl {
            self.rows.clear();
        }
        if !self.rows.insert(index) && ctrl {
            self.rows.remove(&index);
        }
        self.lead = Some(index);
        self.anchor = Some(index);
    }

    /// Moves the keyboard `delta` rows through what is on screen.
    ///
    /// Returns the entry it landed on, so the caller can scroll to it.
    ///
    /// * plain — the selection collapses onto it, which is what a listview does
    ///   and what makes arrowing a way of scoping the run;
    /// * `Ctrl` — the keyboard moves and the scope does not;
    /// * `Shift` — the range from the anchor grows.
    ///
    /// Clamped at both ends rather than wrapping: a list is not a carousel, and
    /// holding Down past the last row should stop there.
    pub fn arrow(
        &mut self,
        delta: isize,
        ctrl: bool,
        shift: bool,
        order: &[usize],
    ) -> Option<usize> {
        // Nothing on screen, nothing to land on. Said here rather than left to
        // the arithmetic: `clamp(0, -1)` panics and `len - 1` underflows.
        let last = order.len().checked_sub(1)?;
        let at = self
            .lead
            .and_then(|lead| order.iter().position(|&i| i == lead));
        let next = match at {
            Some(at) => (at as isize + delta).clamp(0, last as isize) as usize,
            // Nothing has the keyboard yet: the first press lands on the end
            // the user is heading away from, so Down starts at the top.
            None if delta > 0 => 0,
            None => last,
        };
        self.jump_to(next, ctrl, shift, order)
    }

    /// Home and End, and the landing half of [`Selection::arrow`].
    pub fn jump_to(
        &mut self,
        position: usize,
        ctrl: bool,
        shift: bool,
        order: &[usize],
    ) -> Option<usize> {
        let &entry = order.get(position)?;
        if ctrl && !shift {
            self.lead = Some(entry);
            self.anchor = Some(entry);
        } else {
            self.click(entry, false, shift, order);
        }
        Some(entry)
    }

    /// Follows a permutation, where `moved_to[old] = new`.
    fn remap(&mut self, moved_to: &[usize]) {
        let follow = |i: Option<usize>| i.and_then(|i| moved_to.get(i).copied());
        self.rows = self
            .rows
            .iter()
            .filter_map(|&i| moved_to.get(i).copied())
            .collect();
        self.lead = follow(self.lead);
        self.anchor = follow(self.anchor);
    }
}

#[derive(Debug)]
pub struct Session {
    pub settings: SessionSettings,
    /// Free Select's collected paths, in the order they were added.
    ///
    /// Always absolute, and kept up to date by [`Self::follow_renames`]: a
    /// run renames these files, and a relist of the old paths found none of
    /// them.
    pub free_select: Vec<PathBuf>,
    entries: Arc<Vec<FileEntry>>,
    /// What the run covers, and where the keyboard is.
    pub selection: Selection,
    /// Set when the last refresh failed, so the source bar can say why.
    pub error: Option<String>,
    /// Entries the walk could not read. Not an error: the rest of the listing
    /// is real and usable, and hiding it would be the bug this replaced (P63).
    pub problems: Vec<ren_core::listing::ListProblem>,
    /// A folder the run would touch that the OS needs left alone, found once
    /// per listing rather than once per frame (D127).
    pub guarded: Option<PathBuf>,
    /// Set by anything that changes what the listing should hold — a drop, a
    /// cleared free-select set, a folder change — and drained by the app,
    /// which turns it into a request to the listing worker. The session has
    /// no worker of its own: it is egui-free and thread-free by design, so
    /// it says *that* a relist is wanted and the app says *when*.
    pub relist_wanted: bool,
    /// A hand-set row order to put back when the next listing lands
    /// (D157): the paths in the order the user had them, as the run left
    /// them. Consumed by `install_listing`.
    pending_order: Option<Vec<PathBuf>>,
    /// Who decides which folders are the operating system's (D127): the
    /// app's own platform, so the guard and the run ask the same one.
    platform: Arc<dyn Platform>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            settings: SessionSettings::default(),
            free_select: Vec::new(),
            entries: Arc::new(Vec::new()),
            selection: Selection::default(),
            error: None,
            problems: Vec::new(),
            guarded: None,
            relist_wanted: false,
            pending_order: None,
            platform: ren_platform::host(),
        }
    }
}

impl Session {
    /// A session over `settings`, listing nothing until a relist is asked
    /// for and lands.
    ///
    /// **A stored Free Select opens as Browser.** The mode is remembered and
    /// the set is not, so a session that ended in Free Select would otherwise
    /// start on an empty table with the saved folder unused. A launch that
    /// names files sets the mode again itself (`RenameItApp::start_at`).
    pub fn new(settings: SessionSettings) -> Self {
        Self::with_platform(settings, ren_platform::host())
    }

    /// The same, with the platform the D127 guard should ask.
    pub fn with_platform(mut settings: SessionSettings, platform: Arc<dyn Platform>) -> Self {
        settings.mode = SourceMode::Browser;
        Self {
            settings,
            platform,
            ..Default::default()
        }
    }

    pub fn entries(&self) -> &Arc<Vec<FileEntry>> {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn list_options(&self) -> ListOptions {
        ListOptions {
            files: self.settings.files,
            folders: self.settings.folders,
            subfolders: self.settings.subfolders,
            hidden: self.settings.show_hidden,
            system: self.settings.show_system,
            read_only: self.settings.show_read_only,
            pattern_applies: self.settings.pattern_applies,
            ..Default::default()
        }
        .with_pattern(&self.settings.pattern)
    }

    /// What the listing should be built from right now.
    ///
    /// Handed to the listing worker, which walks it on its own thread and
    /// hands the result back to [`Self::install_listing`].
    pub fn source(&self) -> super::listing::Source {
        match self.settings.mode {
            SourceMode::Browser => super::listing::Source::Browser {
                dir: self.settings.dir.clone(),
                options: self.list_options(),
            },
            SourceMode::FreeSelect => super::listing::Source::FreeSelect {
                paths: self.free_select.clone(),
            },
        }
    }

    /// Says a relist is wanted. The app drains this into a worker request.
    pub fn request_refresh(&mut self) {
        self.relist_wanted = true;
    }

    /// Rebuilds the listing from its source on the calling thread.
    ///
    /// For the session's own tests and for anything with no frame to wait
    /// in. The app goes through the worker instead, because this is the walk
    /// that froze the window, and it lands its result through the same
    /// [`Self::install_listing`].
    pub fn refresh_now(&mut self) {
        self.relist_wanted = false;
        self.make_paths_absolute();
        let listed = self.source().list(&|| false);
        self.install_listing(listed);
    }

    /// Makes the folder and the Free Select set absolute, before they are
    /// listed.
    ///
    /// Every path in a plan comes from here, and the executor refuses a
    /// relative one: a journal would record it as given, and an undo run
    /// from another working directory would replay it somewhere else. It
    /// also keeps Backspace honest (the parent of `.` is an empty path) and
    /// keeps a relative folder out of the saved settings, where the next
    /// launch would read it against a different working directory.
    ///
    /// `std::path::absolute` is lexical: it joins the working directory and
    /// resolves no links, so a folder is still called what the user called
    /// it. Called for every relist rather than at each place a path comes
    /// in, because the address box writes `settings.dir` directly.
    pub fn make_paths_absolute(&mut self) {
        if let Ok(dir) = std::path::absolute(&self.settings.dir) {
            self.settings.dir = dir;
        }
        for path in &mut self.free_select {
            if let Ok(absolute) = std::path::absolute(&*path) {
                *path = absolute;
            }
        }
    }

    /// Takes a finished listing, then sorts it.
    ///
    /// Selection is dropped: the indices it held pointed into the old list, and
    /// silently re-pointing them at different files is how a user renames
    /// something they did not mean to (P23). A hand-set order left by
    /// `refresh_after_run` is put back, because a rename is not a re-listing
    /// (D157); any other relist has no hand-set order left to preserve, and
    /// the previous column takes over again.
    pub fn install_listing(
        &mut self,
        listed: std::io::Result<(Vec<FileEntry>, Vec<ren_core::listing::ListProblem>)>,
    ) {
        // `relist_wanted` is deliberately left alone: a listing landing is
        // not the same event as a request being taken up, and a request
        // made *after* this walk started — a folder changed while it ran —
        // is still owed its own walk. Clearing it here lost exactly that
        // request the first time the two overlapped.
        self.selection.clear();
        self.error = None;
        self.problems.clear();
        self.settings.sort.manual = false;
        self.settings.sort.dragged = false;

        match listed {
            Ok((entries, problems)) => {
                self.entries = Arc::new(entries);
                self.problems = problems;
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.entries = Arc::new(Vec::new());
            }
        }
        self.sort_entries();
        self.guarded = self.find_guarded();

        if let Some(order) = self.pending_order.take() {
            self.restore_order(&order);
        }
    }

    /// The first folder this listing would touch that the OS needs left alone.
    ///
    /// `ren_platform::guarded::first_guarded` is the check `ren-cli` makes
    /// too, so both front ends refuse the same folders: the browse directory
    /// (an empty `C:\Windows` is refused before the user adds an operation and
    /// wonders why nothing happened), each distinct parent, and an entry that
    /// is itself a guarded root.
    fn find_guarded(&self) -> Option<PathBuf> {
        if !self.settings.guard_system_folders {
            return None;
        }
        let dir =
            (self.settings.mode == SourceMode::Browser).then_some(self.settings.dir.as_path());
        ren_platform::guarded::first_guarded(
            self.platform.as_ref(),
            dir,
            self.entries.iter().map(|entry| entry.path.as_path()),
        )
    }

    /// The folder `path` would be renamed in, if the operating system owns
    /// it and the guard is on (D127) — the same question as
    /// [`Self::guarded`], for one file rather than the listing.
    pub fn guarded_for(&self, path: &Path) -> Option<PathBuf> {
        if !self.settings.guard_system_folders {
            return None;
        }
        ren_platform::guarded::first_guarded(self.platform.as_ref(), None, [path])
    }

    /// Adds paths dropped from the file manager, switching to Free Select.
    ///
    /// A single folder dropped in Browser mode navigates there instead, which
    /// is what people expect of a drag. The row menu's *Add to Free Select*
    /// is not a drag and goes through [`Self::add_to_free_select`].
    pub fn accept_dropped(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.settings.mode == SourceMode::Browser && paths.len() == 1 && paths[0].is_dir() {
            self.settings.dir = paths.into_iter().next().expect("checked above");
            self.request_refresh();
            return;
        }
        self.add_to_free_select(paths);
    }

    /// Adds paths to Free Select and switches to it, whatever they are.
    pub fn add_to_free_select(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.settings.mode = SourceMode::FreeSelect;
        // A set, so a large drop is not quadratic: `contains` on the list per
        // dropped path was ten thousand times ten thousand for a folder's
        // worth of files dragged in at once.
        let mut known: HashSet<PathBuf> = self.free_select.iter().cloned().collect();
        for path in paths {
            let path = std::path::absolute(&path).unwrap_or(path);
            if known.insert(path.clone()) {
                self.free_select.push(path);
            }
        }
        self.request_refresh();
    }

    pub fn clear_free_select(&mut self) {
        self.free_select.clear();
        self.request_refresh();
    }

    /// Takes rows out of Free Select (P65). The files stay where they are:
    /// this edits a list, and nothing in the app deletes from disk.
    ///
    /// Returns how many left the list.
    pub fn remove_from_free_select(&mut self, rows: &[usize]) -> usize {
        let leaving: HashSet<&Path> = rows
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .map(|entry| entry.path.as_path())
            .collect();
        let before = self.free_select.len();
        self.free_select
            .retain(|path| !leaving.contains(path.as_path()));
        let removed = before - self.free_select.len();
        if removed > 0 {
            self.request_refresh();
        }
        removed
    }

    /// Carries Free Select's paths through a set of renames, `(from, to)`.
    ///
    /// A run, an F2 rename and an undo all move files the set names, and the
    /// set is only paths — so without this the relist that follows asked for
    /// files that no longer existed under those names.
    pub fn follow_renames(&mut self, renamed: &[(PathBuf, PathBuf)]) {
        if renamed.is_empty() || self.free_select.is_empty() {
            return;
        }
        let moved = moves(renamed);
        for path in &mut self.free_select {
            *path = followed(&moved, path);
        }
    }

    /// How many distinct folders Free Select is drawing from, for its label.
    pub fn free_select_folder_count(&self) -> usize {
        let mut dirs: Vec<&Path> = self.free_select.iter().filter_map(|p| p.parent()).collect();
        dirs.sort_unstable();
        dirs.dedup();
        dirs.len()
    }

    pub fn set_sort(&mut self, column: SortColumn) {
        // New name is a reorder command, not a sort mode — see
        // `reorder_by_new_names`. Guarded here so a stray call cannot put the
        // settings into a state nothing can render.
        if column == SortColumn::NewName {
            return;
        }
        if self.settings.sort.column == column && !self.settings.sort.manual {
            self.settings.sort.ascending = !self.settings.sort.ascending;
        } else {
            self.settings.sort = Sort {
                column,
                ascending: true,
                manual: false,
                dragged: false,
            };
        }
        self.sort_entries();
    }

    /// Orders the listing by the names the plan is *currently* producing, once.
    ///
    /// The point is to number files in the order of their new names: order
    /// by New name, then add a counter. That only works because the listing
    /// order **is** the run order, which from M3 is what numbers the counter
    /// (D28).
    ///
    /// A command rather than a sort mode, because a mode does not converge. The
    /// counter is a function of the input order, so re-sorting rewrites its own
    /// sort key: `<Counter> <Name>` descending goes `a,b,c` → `1 a, 2 b, 3 c` →
    /// sort → `c,b,a` → re-plan → `1 c, 2 b, 3 a` → sort → `a,b,c`, forever.
    /// That is the never-settling UI D26 forbids. Ordering once and stopping
    /// converges by construction (D119).
    ///
    /// Clicking again reverses, which is the only part of a sort mode worth
    /// keeping.
    pub fn reorder_by_new_names(&mut self, plan: &ren_core::Plan) {
        let ascending = if self.settings.sort.manual {
            !self.settings.sort.ascending
        } else {
            true
        };

        // **By file, not by `PlanItem::index`.** That index counts the
        // entries the plan was *handed*, which with a selection is the
        // scoped subset, not the listing — so the selected row's new name
        // was filed under whichever row happened to be first. Mapping by
        // source is right for a scoped plan and for one built before a sort
        // alike, because sources are unique.
        //
        // A row the plan does not cover — filtered out of the run — sorts under
        // the name it already has, which is the name the column shows for it.
        let new_names: HashMap<&Path, &str> = plan
            .items
            .iter()
            .map(|item| (item.source.as_path(), item.new_name.as_str()))
            .collect();
        let keys: Vec<String> = self
            .entries
            .iter()
            .map(|e| {
                new_names
                    .get(e.path.as_path())
                    .copied()
                    .unwrap_or(&e.file_name)
                    .to_lowercase()
            })
            .collect();

        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by(|&a, &b| {
            let by_name = keys[a].cmp(&keys[b]);
            if ascending {
                by_name
            } else {
                by_name.reverse()
            }
            .then(a.cmp(&b))
        });

        self.apply_order(&order);
        self.settings.sort.manual = true;
        // Not a drag: this order **is** the New name column's, so its header
        // keeps the arrow (D157 amending D119).
        self.settings.sort.dragged = false;
        self.settings.sort.ascending = ascending;
    }

    /// Re-lists after a run, an F2 rename or an undo, keeping a hand-set
    /// order and the Free Select set by following the rename report.
    ///
    /// Always, never a setting: a hand-set order is the run order, so losing
    /// it on the run that used it would make dragging half a feature. A
    /// relist clears `manual` because a *re-listing* has no hand-set order
    /// left to preserve — but a rename is not a re-listing, it is a set of
    /// known old→new paths, which is exactly what **D139** already uses to
    /// rekey the texture cache beside the caller. An undo's report is the same
    /// shape in the other direction, so it keeps the order too.
    ///
    /// Anything the run did not touch keeps its name, so `renamed` only has to
    /// cover what moved.
    pub fn refresh_after_run(&mut self, renamed: &[(PathBuf, PathBuf)]) {
        self.follow_renames(renamed);
        if self.settings.sort.manual {
            // A map, not `find` per entry: ten thousand dragged rows renamed
            // meant a hundred million path comparisons here, and as many
            // again in the rank below.
            let moved = moves(renamed);
            let order: Vec<PathBuf> = self
                .entries
                .iter()
                .map(|entry| followed(&moved, &entry.path))
                .collect();
            self.pending_order = Some(order);
        }
        self.request_refresh();
    }

    /// Puts a hand-set order back over a fresh listing.
    fn restore_order(&mut self, order: &[PathBuf]) {
        let position: HashMap<&Path, usize> = order
            .iter()
            .enumerate()
            .map(|(i, path)| (path.as_path(), i))
            .collect();
        let mut rank: Vec<usize> = (0..self.entries.len()).collect();
        rank.sort_by_key(|&i| {
            position
                .get(self.entries[i].path.as_path())
                .copied()
                // A file the run created, or one that appeared underneath it,
                // has no place in the old order and goes to the end rather than
                // silently to the front.
                .unwrap_or(usize::MAX)
        });
        self.apply_order(&rank);
        self.settings.sort.manual = true;
        self.settings.sort.dragged = true;
    }

    /// Moves `rows` so they land in front of the entry now at `before`.
    ///
    /// A drag in the list. The order is the run order, so this is also how a
    /// user decides which number a counter gives each file.
    ///
    /// `before` is an index into the **current** listing, and
    /// `entries().len()` means "at the end". The moved rows are taken out
    /// first, so a `before` that falls inside the moved block means the same
    /// place as the row after it.
    ///
    /// Returns false and changes nothing when that would be a no-op, so a drag
    /// that ends where it started does not dirty the preview — the same
    /// contract `CardStack::move_card` keeps for the operation stack.
    pub fn move_rows(&mut self, rows: &[usize], before: usize) -> bool {
        let moving: BTreeSet<usize> = rows
            .iter()
            .copied()
            .filter(|&i| i < self.entries.len())
            .collect();
        if moving.is_empty() {
            return false;
        }

        let mut order: Vec<usize> = Vec::with_capacity(self.entries.len());
        let mut inserted = false;
        for i in 0..self.entries.len() {
            if i == before {
                order.extend(moving.iter().copied());
                inserted = true;
            }
            if !moving.contains(&i) {
                order.push(i);
            }
        }
        if !inserted {
            order.extend(moving.iter().copied());
        }

        if order.iter().copied().eq(0..self.entries.len()) {
            return false;
        }
        self.apply_order(&order);
        // A hand-set order is not one to re-derive — the same flag the New-name
        // reorder sets (D119), which anticipated this second producer — plus
        // the bit that stops any header claiming it.
        self.settings.sort.manual = true;
        self.settings.sort.dragged = true;
        true
    }

    /// Permutes the listing, carrying the selection with it.
    ///
    /// Carried rather than cleared, unlike a relist: there the indices
    /// pointed into a listing that no longer exists, here the permutation is
    /// known exactly, so the same files stay selected.
    fn apply_order(&mut self, order: &[usize]) {
        let mut moved_to = vec![0usize; order.len()];
        for (position, &old) in order.iter().enumerate() {
            moved_to[old] = position;
        }
        // Moved, not cloned. `make_mut` already copies the whole listing
        // whenever the preview worker still holds the old `Arc` (it usually
        // does), and cloning every entry a second time on top of that made a
        // sort of ten thousand rows two full copies of the listing.
        let entries = Arc::make_mut(&mut self.entries);
        let mut slots: Vec<Option<FileEntry>> =
            std::mem::take(entries).into_iter().map(Some).collect();
        *entries = order
            .iter()
            .map(|&old| {
                slots[old]
                    .take()
                    .expect("a permutation names each row once")
            })
            .collect();
        // The whole `Selection`, in one call — the scope, the keyboard and the
        // anchor a Shift-extend measures from. Remapping the rows and forgetting
        // the other two is the exact failure this type exists to prevent.
        self.selection.remap(&moved_to);
    }

    /// Sorts the listing itself, not just the view.
    ///
    /// The order the user sees is the order operations run in, and from M3 it
    /// is what drives counter numbering — so it has to be the real order.
    fn sort_entries(&mut self) {
        let sort = self.settings.sort;
        // An order the user set by hand is not one to re-derive.
        if sort.manual {
            return;
        }

        // **Through `apply_order`, not `Vec::sort_by_key`.**
        //
        // Sorting used to permute `entries` and leave `selection` alone — and
        // the selection is *entry indices*, and the run's scope (P22). So
        // picking three files and then clicking a column header pointed the
        // scope at whatever landed in those three slots, and Rename renamed
        // three files the user had not chosen. Silently: the highlight moved
        // with the rows, so it looked correct.
        //
        // `apply_order` already carried the selection through a permutation for
        // the New-name reorder command (D119) — nothing but habit kept sorting
        // on the other path. Sorting a permutation of indices rather than the
        // entries themselves also means `sort_by_cached_key` allocates its keys
        // once per entry instead of once per comparison, and reversing the
        // order vector is exactly `entries.reverse()`, stability included.
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        let entries = &self.entries;
        match sort.column {
            // `NewName` only ever arrives here from a settings file written
            // before it stopped being a sort mode (D119). Reading it as Name is
            // what that setting always actually did.
            SortColumn::Name | SortColumn::NewName => {
                order.sort_by_cached_key(|&i| entries[i].file_name.to_lowercase());
            }
            SortColumn::Size => order.sort_by_key(|&i| entries[i].size),
            SortColumn::Modified => order.sort_by_key(|&i| entries[i].modified),
            SortColumn::Created => order.sort_by_key(|&i| entries[i].created),
            // Folded, like the name above it: `.JPG` beside `.jpg` is what a
            // user means by "sorted by extension".
            // A folder has no extension (`FileEntry::split`), so `2019.06`
            // sorts with the folders rather than among the `.06` files.
            SortColumn::Extension => order
                .sort_by_cached_key(|&i| entries[i].split().1.unwrap_or_default().to_lowercase()),
            // By folder, then by name inside it, or the rows of one folder
            // arrive in whatever order the walk found them.
            SortColumn::Folder => order.sort_by_cached_key(|&i| {
                (
                    entries[i]
                        .path
                        .parent()
                        .map(|p| p.to_string_lossy().to_lowercase())
                        .unwrap_or_default(),
                    entries[i].file_name.to_lowercase(),
                )
            }),
        }
        if !sort.ascending {
            order.reverse();
        }
        self.apply_order(&order);
    }

    /// Which rows the run applies to: the selection, or every row when
    /// nothing is selected (P22).
    pub fn scoped_indices(&self) -> Vec<usize> {
        if self.selection.is_empty() {
            (0..self.entries.len()).collect()
        } else {
            self.selection.iter().collect()
        }
    }
}

/// `(from, to)` pairs as a map from each `from`.
fn moves(renamed: &[(PathBuf, PathBuf)]) -> HashMap<&Path, &Path> {
    renamed
        .iter()
        .map(|(from, to)| (from.as_path(), to.as_path()))
        .collect()
}

/// Where `path` is after the moves.
///
/// **One prefix rewrite per depth, as many passes as it takes.** Each pass
/// walks the path's ancestors, the path itself first, and rewrites through
/// the deepest one that moved at a depth not yet rewritten; the walk stops
/// when no such ancestor is left. That covers every order a report can come
/// in:
///
/// - a run renames deepest first (`order_renames`), so a file's pair names
///   its old folders, and each folder above then carries it along, one depth
///   per pass — at any nesting;
/// - an undo reverses newest first, so the folder goes back *before* the file
///   inside it, and the file's pair names the folder's restored name: the
///   folder is followed first, then the file's own name.
///
/// Each depth once, because two folders that swapped names would otherwise
/// be chased round forever; with it the walk ends after at most as many passes
/// as the path has components.
fn followed(moved: &HashMap<&Path, &Path>, path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut rewritten: Vec<usize> = Vec::new();
    loop {
        let next = current.ancestors().find_map(|ancestor| {
            let depth = ancestor.components().count();
            if rewritten.contains(&depth) {
                return None;
            }
            moved.get(ancestor).map(|to| (depth, ancestor, *to))
        });
        let Some((depth, ancestor, to)) = next else {
            return current;
        };
        let rest = current.strip_prefix(ancestor).unwrap_or(Path::new(""));
        let joined = if rest.as_os_str().is_empty() {
            to.to_path_buf()
        } else {
            to.join(rest)
        };
        rewritten.push(depth);
        current = joined;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tree() -> TempDir {
        let dir = TempDir::new().unwrap();
        for name in ["b.txt", "a.txt", "c.mp3"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        dir
    }

    fn session_on(dir: &Path) -> Session {
        let mut session = Session {
            settings: SessionSettings {
                dir: dir.to_path_buf(),
                ..Default::default()
            },
            ..Default::default()
        };
        session.refresh_now();
        session
    }

    #[test]
    fn browser_mode_lists_the_folder_sorted() {
        let dir = tree();
        let session = session_on(dir.path());
        let names: Vec<_> = session
            .entries()
            .iter()
            .map(|e| e.file_name.clone())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt", "c.mp3"]);
    }

    #[test]
    fn the_pattern_box_narrows_the_listing() {
        let dir = tree();
        let mut session = session_on(dir.path());
        session.settings.pattern = "*.txt".into();
        session.refresh_now();
        assert_eq!(session.entries().len(), 2);
    }

    /// P63: an unreadable subfolder is a warning beside a working listing, not
    /// an empty table. Unix-only because it needs a directory the process
    /// genuinely cannot read, and `chmod 000` does not mean that to an
    /// Administrator on Windows.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_subfolder_warns_without_emptying_the_table() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tree();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("inside.txt"), b"x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut session = Session {
            settings: SessionSettings {
                dir: dir.path().to_path_buf(),
                subfolders: true,
                ..Default::default()
            },
            ..Default::default()
        };
        session.refresh_now();
        let restore = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

        assert_eq!(session.error, None, "the run is not a failure");
        assert!(!session.problems.is_empty(), "and it is not silent either");
        let names: Vec<_> = session
            .entries()
            .iter()
            .map(|e| e.file_name.clone())
            .collect();
        assert!(
            names.iter().any(|n| n == "a.txt"),
            "the readable rows survive: {names:?}"
        );
        restore.unwrap();
    }

    // --- the New name header (M8, D119) ------------------------------------

    fn plan_of(session: &Session, pipeline: &ren_core::Pipeline) -> ren_core::Plan {
        ren_core::plan(session.entries(), pipeline, ren_platform::host().as_ref())
    }

    fn names_of(session: &Session) -> Vec<String> {
        session
            .entries()
            .iter()
            .map(|e| e.file_name.clone())
            .collect()
    }

    /// The header used to sort by the *old* name, which is the one column the
    /// table already had. Ordering by the names the run is about to write is
    /// what Re-Number's worked example needs.
    #[test]
    fn the_new_name_header_orders_by_the_name_the_run_will_write() {
        let dir = tree(); // a.txt, b.txt, c.mp3
        let mut session = session_on(dir.path());
        let pipeline = ren_core::Pipeline::new().with(
            ren_core::Step::Name(Box::new(ren_core::Replace::new("a", "z"))),
            ren_core::StepConfig::scoped(ren_core::model::Scope::Name),
        );

        let plan = plan_of(&session, &pipeline);
        session.reorder_by_new_names(&plan);

        // a.txt becomes z.txt, so it goes last.
        assert_eq!(names_of(&session), ["b.txt", "c.mp3", "a.txt"]);
        assert!(session.settings.sort.manual);
    }

    /// The reason this is a command and not a sort mode (D119).
    ///
    /// A counter is a function of the *input* order (D28), so a mode that
    /// re-sorted after every plan would rewrite its own sort key: `<Counter>
    /// <Name>` descending goes `a,b,c` → `1 a, 2 b, 3 c` → sort → `c,b,a` →
    /// re-plan → `1 c, 2 b, 3 a` → sort → `a,b,c`, forever. That is the
    /// never-settling UI D26 forbids.
    ///
    /// What this asserts is the property that makes the command safe: once the
    /// order is hand-set, **nothing re-derives it**. The pipeline here is one
    /// whose new-name order differs from the old-name order, because that is
    /// the only shape in which "did not re-derive" and "re-derived by name"
    /// give different answers.
    #[test]
    fn a_hand_set_order_is_never_re_derived() {
        let dir = tree(); // a.txt, b.txt, c.mp3
        let mut session = session_on(dir.path());
        let pipeline = ren_core::Pipeline::new().with(
            ren_core::Step::Name(Box::new(ren_core::Replace::new("a", "z"))),
            ren_core::StepConfig::scoped(ren_core::model::Scope::Name),
        );

        // a.txt becomes z.txt, so by new name it goes last — where sorting by
        // the old name would have put it first.
        let plan = plan_of(&session, &pipeline);
        session.reorder_by_new_names(&plan);
        assert_eq!(names_of(&session), ["b.txt", "c.mp3", "a.txt"]);

        // Re-plan and re-sort, as the app does after any change.
        let _replanned = plan_of(&session, &pipeline);
        session.sort_entries();
        assert_eq!(
            names_of(&session),
            ["b.txt", "c.mp3", "a.txt"],
            "the order moved on its own"
        );
    }

    /// Clicking again reverses — the one part of a sort mode worth keeping.
    #[test]
    fn clicking_the_new_name_header_again_reverses() {
        let dir = tree();
        let mut session = session_on(dir.path());
        let pipeline = ren_core::Pipeline::new();

        let plan = plan_of(&session, &pipeline);
        session.reorder_by_new_names(&plan);
        assert_eq!(names_of(&session), ["a.txt", "b.txt", "c.mp3"]);

        let plan = plan_of(&session, &pipeline);
        session.reorder_by_new_names(&plan);
        assert_eq!(names_of(&session), ["c.mp3", "b.txt", "a.txt"]);
        assert!(!session.settings.sort.ascending);
    }

    /// The permutation is known exactly, so the same files stay selected —
    /// unlike a relist, where the indices point into a listing that is gone.
    #[test]
    fn a_reorder_carries_the_selection_with_it() {
        let dir = tree();
        let mut session = session_on(dir.path());
        session.selection.set([0]); // a.txt
        let pipeline = ren_core::Pipeline::new().with(
            ren_core::Step::Name(Box::new(ren_core::Replace::new("a", "z"))),
            ren_core::StepConfig::scoped(ren_core::model::Scope::Name),
        );

        let plan = plan_of(&session, &pipeline);
        session.reorder_by_new_names(&plan);

        assert_eq!(session.entries()[2].file_name, "a.txt");
        assert_eq!(
            session.selection.iter().collect::<Vec<_>>(),
            [2],
            "the selection followed the file, not the row number"
        );
    }

    /// A click replaces the selection; a modified click adds to it and clicking
    /// an already-selected row takes it back out. Moved here from `rows.rs`
    /// with the logic it tests.
    #[test]
    fn clicking_a_row_selects_it_and_a_modified_click_toggles() {
        let order: Vec<usize> = (0..5).collect();
        let mut selection = Selection::default();
        selection.click(1, false, false, &order);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [1]);

        selection.click(2, false, false, &order);
        assert_eq!(
            selection.iter().collect::<Vec<_>>(),
            [2],
            "a plain click replaces"
        );

        selection.click(3, true, false, &order);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [2, 3]);
        selection.click(3, true, false, &order);
        assert_eq!(
            selection.iter().collect::<Vec<_>>(),
            [2],
            "and takes it back out"
        );
    }

    /// **Shift did not work.** `additive = command || shift` meant a
    /// Shift-click picked the two endpoints and nothing between them — which
    /// looks like a range that lost its middle, and is a run scoped to two
    /// files when the user asked for five.
    #[test]
    fn shift_clicking_selects_every_row_between_the_anchor_and_the_one_clicked() {
        let order: Vec<usize> = (0..5).collect();
        let mut selection = Selection::default();
        selection.click(1, false, false, &order);
        selection.click(3, false, true, &order);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(selection.lead, Some(3), "the keyboard follows the click");
    }

    /// The anchor is the last row picked **without** Shift, so shrinking a
    /// range works: 1→5 then 1→3 leaves three rows, not five.
    #[test]
    fn a_second_shift_click_still_measures_from_the_last_plain_click() {
        let order: Vec<usize> = (0..6).collect();
        let mut selection = Selection::default();
        selection.click(1, false, false, &order);
        selection.click(5, false, true, &order);
        selection.click(3, false, true, &order);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [1, 2, 3]);
    }

    /// **The range walks what is on screen.** With the Changed chip narrowing
    /// the table, a range built from `min..=max` over entry indices would take
    /// in the rows the filter is hiding — and the selection is the run's scope,
    /// so those files would be renamed without ever having been visible.
    #[test]
    fn a_shift_range_skips_the_rows_the_filter_is_hiding() {
        let order = [0usize, 3, 7];
        let mut selection = Selection::default();
        selection.click(0, false, false, &order);
        selection.click(7, false, true, &order);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [0, 3, 7]);
    }

    /// The arrow keys walk the rows on screen, in the order they are drawn.
    #[test]
    fn the_down_arrow_lands_on_the_next_row_the_filter_shows() {
        // Entries 0, 3 and 7 are on screen; 1, 2, 4, 5 and 6 are hidden. A
        // walk through entry indices rather than through `order` would land on
        // a row that is not drawn.
        let order = [0usize, 3, 7];
        let mut selection = Selection::default();
        selection.click(0, false, false, &order);

        assert_eq!(selection.arrow(1, false, false, &order), Some(3));
        assert_eq!(selection.arrow(1, false, false, &order), Some(7));
        assert_eq!(selection.iter().collect::<Vec<_>>(), [7]);
    }

    /// Ctrl moves the keyboard and leaves the run alone — the only way to reach
    /// a row without re-scoping what Rename would do.
    #[test]
    fn ctrl_and_an_arrow_move_the_keyboard_and_leave_the_run_alone() {
        let order: Vec<usize> = (0..4).collect();
        let mut selection = Selection::default();
        selection.click(0, false, false, &order);

        selection.arrow(1, true, false, &order);
        assert_eq!(selection.lead, Some(1));
        assert_eq!(
            selection.iter().collect::<Vec<_>>(),
            [0],
            "the scope did not move"
        );
    }

    #[test]
    fn shift_and_an_arrow_grow_the_range_from_the_anchor() {
        let order: Vec<usize> = (0..4).collect();
        let mut selection = Selection::default();
        selection.click(1, false, false, &order);
        selection.arrow(1, false, true, &order);
        selection.arrow(1, false, true, &order);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [1, 2, 3]);
    }

    /// A list is not a carousel: holding Down past the last row stops there.
    #[test]
    fn an_arrow_at_either_end_of_the_list_stays_there() {
        let order: Vec<usize> = (0..3).collect();
        let mut selection = Selection::default();
        selection.click(2, false, false, &order);
        assert_eq!(selection.arrow(1, false, false, &order), Some(2));
        selection.click(0, false, false, &order);
        assert_eq!(selection.arrow(-1, false, false, &order), Some(0));
    }

    /// With nothing selected yet, the first press comes in from the end the
    /// user is heading away from — so Down starts at the top.
    #[test]
    fn the_first_arrow_press_comes_in_from_the_end_it_is_heading_away_from() {
        let order: Vec<usize> = (0..3).collect();
        let mut down = Selection::default();
        assert_eq!(down.arrow(1, false, false, &order), Some(0));
        let mut up = Selection::default();
        assert_eq!(up.arrow(-1, false, false, &order), Some(2));
    }

    /// An empty screen — every row filtered out — has nothing to land on.
    /// The doc says "clamped at both ends", and `clamp(0, -1)` is not a
    /// clamp, it is a panic.
    #[test]
    fn an_arrow_over_an_empty_screen_lands_nowhere() {
        let mut selection = Selection::default();
        assert_eq!(selection.arrow(1, false, false, &[]), None);
        assert_eq!(selection.arrow(-1, false, false, &[]), None);
        selection.set([4]);
        assert_eq!(selection.arrow(1, false, true, &[]), None);
        assert!(selection.contains(4), "and the selection is left alone");
    }

    #[test]
    fn home_and_end_reach_the_first_and_last_row_on_screen() {
        let order = [0usize, 3, 7];
        let mut selection = Selection::default();
        assert_eq!(
            selection.jump_to(order.len() - 1, false, false, &order),
            Some(7)
        );
        assert_eq!(selection.jump_to(0, false, false, &order), Some(0));
    }

    /// **The selection is the run's scope (P22), so a sort that leaves it on
    /// the old row numbers renames files the user did not pick.**
    ///
    /// This was shipped: `sort_entries` permuted `entries` and never touched
    /// `selection`, while its sibling `apply_order` — used only by the New-name
    /// reorder — carried it through correctly. Pick three files, click a column
    /// header, press Rename, and three different files are renamed. Silently,
    /// because the highlight moves with the rows and looks right.
    #[test]
    fn a_re_sort_carries_the_selection_with_it() {
        let dir = tree();
        let mut session = session_on(dir.path());
        assert_eq!(names_of(&session), ["a.txt", "b.txt", "c.mp3"]);

        session.selection.set([0]); // a.txt
        session.set_sort(SortColumn::Name); // same column: reverses
        assert_eq!(names_of(&session), ["c.mp3", "b.txt", "a.txt"]);

        assert_eq!(
            session
                .selection
                .iter()
                .map(|i| session.entries()[i].file_name.clone())
                .collect::<Vec<_>>(),
            ["a.txt"],
            "the scope followed the file, not the row number"
        );
    }

    /// The permutation rewrite must not change what sorting *does*, so the
    /// orders it produces are pinned — including that a descending sort is the
    /// ascending one reversed, stability and all.
    #[test]
    fn sorting_still_puts_the_rows_where_it_always_did() {
        let dir = tree();
        let mut session = session_on(dir.path());

        assert_eq!(names_of(&session), ["a.txt", "b.txt", "c.mp3"]);
        session.set_sort(SortColumn::Name);
        assert_eq!(names_of(&session), ["c.mp3", "b.txt", "a.txt"]);

        session.set_sort(SortColumn::Extension);
        let extensions: Vec<&str> = session
            .entries()
            .iter()
            .map(|e| e.split().1.unwrap_or_default())
            .collect();
        assert_eq!(extensions, ["mp3", "txt", "txt"]);

        session.set_sort(SortColumn::Size);
        assert_eq!(session.entries().len(), 3, "every row survives a sort");
    }

    /// A dragged row lands where it was dropped, and so takes that place in
    /// the run order.
    #[test]
    fn dragging_a_row_down_puts_it_where_it_was_dropped() {
        let dir = tree();
        let mut session = session_on(dir.path());
        assert_eq!(names_of(&session), ["a.txt", "b.txt", "c.mp3"]);

        // `a.txt` lands in front of whatever is at index 2 now.
        assert!(session.move_rows(&[0], 2));
        assert_eq!(names_of(&session), ["b.txt", "a.txt", "c.mp3"]);

        // And to the very end.
        assert!(session.move_rows(&[0], 3));
        assert_eq!(names_of(&session), ["a.txt", "c.mp3", "b.txt"]);
    }

    /// Dragging a multi-row selection is the gesture anyone tries first, and
    /// the rows have to arrive in the order they were in rather than the order
    /// they happened to be listed in the payload.
    #[test]
    fn dragging_a_whole_selection_keeps_them_together_and_in_order() {
        let dir = tree();
        let mut session = session_on(dir.path());
        assert!(session.move_rows(&[2, 0], 3));
        assert_eq!(names_of(&session), ["b.txt", "a.txt", "c.mp3"]);
    }

    /// A drag that ends where it started must not dirty the preview — the same
    /// contract `CardStack::move_card` keeps for the operation stack.
    #[test]
    fn a_drop_that_changes_nothing_is_not_a_change() {
        let dir = tree();
        let mut session = session_on(dir.path());
        assert!(!session.move_rows(&[0], 0));
        assert!(!session.move_rows(&[1], 1));
        assert!(!session.settings.sort.manual, "and does not claim an order");
    }

    /// The drag is the second producer of a hand-set order, which **D119**
    /// anticipated when it added the flag — and the New name header must not
    /// claim it: after a drag the listing is in nobody's column order.
    #[test]
    fn a_dragged_order_is_hand_set_but_is_not_the_new_name_columns() {
        let dir = tree();
        let mut session = session_on(dir.path());
        session.move_rows(&[0], 3);

        assert!(session.settings.sort.manual);
        assert!(
            session.settings.sort.dragged,
            "the New name header would otherwise show an arrow for an order it did not make"
        );
        assert_eq!(
            session.settings.sort.column,
            SortColumn::Name,
            "and the column the user had chosen survives, to take over on the next refresh"
        );

        // And it is not re-derived.
        session.sort_entries();
        assert_eq!(names_of(&session), ["b.txt", "c.mp3", "a.txt"]);
    }

    /// The permutation carries everything that points into the listing.
    #[test]
    fn a_dragged_order_carries_the_selection_and_the_lead() {
        let dir = tree();
        let mut session = session_on(dir.path());
        session.selection.set([0]); // a.txt

        session.move_rows(&[0], 3);
        assert_eq!(
            session
                .selection
                .iter()
                .map(|i| session.entries()[i].file_name.clone())
                .collect::<Vec<_>>(),
            ["a.txt"]
        );
        assert_eq!(session.selection.lead, Some(2));
    }

    /// A hand-set order survives a re-plan and nothing else: a real sort takes
    /// over, and so does a re-listing, which has no hand-set order left.
    #[test]
    fn a_real_sort_and_a_refresh_both_end_the_hand_set_order() {
        let dir = tree();
        let mut session = session_on(dir.path());
        let pipeline = ren_core::Pipeline::new();

        let plan = plan_of(&session, &pipeline);
        session.reorder_by_new_names(&plan);
        session.reorder_by_new_names(&plan_of(&session, &pipeline)); // descending
        assert!(session.settings.sort.manual);

        session.set_sort(SortColumn::Size);
        assert!(!session.settings.sort.manual);

        session.reorder_by_new_names(&plan_of(&session, &pipeline));
        assert!(session.settings.sort.manual);
        session.refresh_now();
        assert!(!session.settings.sort.manual);
        assert_eq!(names_of(&session), ["a.txt", "b.txt", "c.mp3"]);
    }

    /// A folder has no extension, so a dot in its name does not file it among
    /// the files that share the text after the dot.
    #[test]
    fn sorting_by_extension_puts_a_dotted_folder_with_the_folders() {
        let dir = tree(); // a.txt, b.txt, c.mp3
        std::fs::create_dir(dir.path().join("photos.txt")).unwrap();
        let mut session = Session {
            settings: SessionSettings {
                dir: dir.path().to_path_buf(),
                folders: true,
                ..Default::default()
            },
            ..Default::default()
        };
        session.refresh_now();
        session.set_sort(SortColumn::Extension);
        assert_eq!(
            names_of(&session)[0],
            "photos.txt",
            "no extension sorts first"
        );
    }

    /// With a selection the plan covers only the selected rows, and its
    /// `PlanItem::index` counts *those*. The reorder used to read it as a
    /// listing row, and filed the selected file's new name under row 0.
    #[test]
    fn the_new_name_reorder_maps_a_scoped_plan_by_file() {
        let dir = tree(); // a.txt, b.txt, c.mp3
        let mut session = session_on(dir.path());
        let pipeline = ren_core::Pipeline::new().with(
            ren_core::Step::Name(Box::new(ren_core::Replace::new("b", "zb"))),
            ren_core::StepConfig::scoped(ren_core::model::Scope::Name),
        );
        // Only b.txt is in the run.
        let scoped = [session.entries()[1].clone()];
        let plan = ren_core::plan(&scoped, &pipeline, ren_platform::host().as_ref());
        assert_eq!(plan.items[0].index, 0);

        session.reorder_by_new_names(&plan);
        assert_eq!(names_of(&session), ["a.txt", "c.mp3", "b.txt"]);
    }

    /// A stored Free Select has no files to show — the set is not stored — so
    /// a new session opens on the saved folder instead of an empty table.
    #[test]
    fn a_session_never_opens_on_an_empty_free_select() {
        let session = Session::new(SessionSettings {
            mode: SourceMode::FreeSelect,
            ..Default::default()
        });
        assert_eq!(session.settings.mode, SourceMode::Browser);
    }

    /// A relative folder is listed as an absolute one, so every path in a
    /// plan is absolute (the executor refuses anything else) and Backspace
    /// has a real parent to go to.
    #[test]
    fn a_relative_folder_is_listed_by_its_absolute_path() {
        let mut session = Session {
            settings: SessionSettings {
                dir: PathBuf::from("."),
                ..Default::default()
            },
            ..Default::default()
        };
        session.refresh_now();
        assert!(session.settings.dir.is_absolute());
        assert!(session.entries().iter().all(|e| e.path.is_absolute()));
    }

    /// Free Select follows a rename, a file inside a renamed folder, and a
    /// file renamed inside a folder that was renamed after it — the order the
    /// planner runs them in, deepest first.
    #[test]
    fn free_select_follows_its_paths_through_renames() {
        let mut session = Session {
            free_select: vec![
                PathBuf::from("/p/a.txt"),
                PathBuf::from("/p/x/kept.txt"),
                PathBuf::from("/p/x/old.txt"),
                PathBuf::from("/p/untouched.txt"),
            ],
            ..Default::default()
        };
        session.follow_renames(&[
            (PathBuf::from("/p/a.txt"), PathBuf::from("/p/b.txt")),
            (PathBuf::from("/p/x/old.txt"), PathBuf::from("/p/x/new.txt")),
            (PathBuf::from("/p/x"), PathBuf::from("/p/y")),
        ]);
        assert_eq!(
            session.free_select,
            [
                PathBuf::from("/p/b.txt"),
                "/p/y/kept.txt".into(),
                "/p/y/new.txt".into(),
                "/p/untouched.txt".into(),
            ]
        );
    }

    /// Two folders that swapped names carry their contents once each, and
    /// the walk does not chase them round.
    #[test]
    fn a_folder_swap_is_followed_once() {
        let mut session = Session {
            free_select: vec![PathBuf::from("/p/a/f.txt")],
            ..Default::default()
        };
        session.follow_renames(&[
            (PathBuf::from("/p/a"), PathBuf::from("/p/b")),
            (PathBuf::from("/p/b"), PathBuf::from("/p/a")),
        ]);
        assert_eq!(session.free_select, [PathBuf::from("/p/b/f.txt")]);
    }

    /// An undo reverses newest first, so the folder goes back *before* the
    /// file inside it, and the file's pair names the folder's restored name.
    /// The folder is followed first, then the file's own name.
    #[test]
    fn free_select_follows_an_undo_of_a_folder_and_a_file_in_it() {
        let mut session = Session {
            free_select: vec![PathBuf::from("/p/X"), PathBuf::from("/p/X/F.txt")],
            ..Default::default()
        };
        session.follow_renames(&[
            (PathBuf::from("/p/X"), PathBuf::from("/p/x")),
            (PathBuf::from("/p/x/F.txt"), PathBuf::from("/p/x/f.txt")),
        ]);
        assert_eq!(
            session.free_select,
            [PathBuf::from("/p/x"), PathBuf::from("/p/x/f.txt")]
        );
    }

    /// Two renamed folders nested in one run carry the file through both, at
    /// any depth.
    #[test]
    fn free_select_follows_a_file_through_two_nested_renamed_folders() {
        let mut session = Session {
            free_select: vec![
                PathBuf::from("/p/x"),
                PathBuf::from("/p/x/sub"),
                PathBuf::from("/p/x/sub/f.txt"),
            ],
            ..Default::default()
        };
        session.follow_renames(&[
            (
                PathBuf::from("/p/x/sub/f.txt"),
                PathBuf::from("/p/x/sub/F.txt"),
            ),
            (PathBuf::from("/p/x/sub"), PathBuf::from("/p/x/SUB")),
            (PathBuf::from("/p/x"), PathBuf::from("/p/X")),
        ]);
        assert_eq!(
            session.free_select,
            [
                PathBuf::from("/p/X"),
                PathBuf::from("/p/X/SUB"),
                PathBuf::from("/p/X/SUB/F.txt"),
            ]
        );
    }

    /// P65: Delete edits the list and nothing else.
    #[test]
    fn removing_rows_from_free_select_leaves_the_files() {
        let dir = tree();
        let mut session = session_on(dir.path());
        session.accept_dropped(vec![dir.path().join("a.txt"), dir.path().join("c.mp3")]);
        session.refresh_now();
        assert_eq!(session.remove_from_free_select(&[0]), 1);
        assert!(session.relist_wanted);
        session.refresh_now();
        assert_eq!(session.entries().len(), 1);
        assert!(dir.path().join("a.txt").exists() && dir.path().join("c.mp3").exists());
    }

    /// The row menu is not a drag: one folder added from it is added, not
    /// browsed into.
    #[test]
    fn adding_one_folder_to_free_select_adds_it() {
        let dir = tree();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let mut session = session_on(dir.path());
        session.add_to_free_select(vec![sub.clone()]);
        assert_eq!(session.settings.mode, SourceMode::FreeSelect);
        assert_eq!(session.settings.dir, dir.path());
        assert_eq!(session.free_select, [sub]);
    }

    #[test]
    fn a_missing_folder_is_reported_rather_than_panicking() {
        let mut session = Session {
            settings: SessionSettings {
                dir: PathBuf::from("/definitely/not/here"),
                ..Default::default()
            },
            ..Default::default()
        };
        session.refresh_now();
        assert!(session.error.is_some());
        assert!(session.is_empty());
    }

    #[test]
    fn sorting_toggles_direction_on_the_same_column() {
        let dir = tree();
        let mut session = session_on(dir.path());
        session.set_sort(SortColumn::Name);
        assert!(!session.settings.sort.ascending, "same column flips");
        let names: Vec<_> = session
            .entries()
            .iter()
            .map(|e| e.file_name.clone())
            .collect();
        assert_eq!(names, ["c.mp3", "b.txt", "a.txt"]);
    }

    #[test]
    fn a_refresh_drops_the_selection() {
        let dir = tree();
        let mut session = session_on(dir.path());
        session.selection.set([0]);
        session.refresh_now();
        assert!(
            session.selection.is_empty(),
            "stale indices would point at different files"
        );
    }

    /// With nothing selected, every row is in the run (P22).
    #[test]
    fn an_empty_selection_scopes_the_run_to_everything() {
        let dir = tree();
        let mut session = session_on(dir.path());
        assert_eq!(session.scoped_indices(), vec![0, 1, 2]);

        session.selection.set([1]);
        assert_eq!(session.scoped_indices(), vec![1]);
    }

    #[test]
    fn dropping_files_switches_to_free_select() {
        let dir = tree();
        let mut session = session_on(dir.path());
        session.accept_dropped(vec![dir.path().join("a.txt"), dir.path().join("c.mp3")]);
        assert!(session.relist_wanted, "a drop asks for a relist");
        session.refresh_now();

        assert_eq!(session.settings.mode, SourceMode::FreeSelect);
        assert_eq!(session.entries().len(), 2);
        assert_eq!(session.free_select_folder_count(), 1);
    }

    #[test]
    fn dropping_a_single_folder_in_browser_mode_navigates_there() {
        let dir = tree();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), b"x").unwrap();

        let mut session = session_on(dir.path());
        session.accept_dropped(vec![sub.clone()]);
        session.refresh_now();

        assert_eq!(session.settings.mode, SourceMode::Browser);
        assert_eq!(session.settings.dir, sub);
        assert_eq!(session.entries().len(), 1);
    }

    #[test]
    fn dropping_the_same_file_twice_does_not_duplicate_it() {
        let dir = tree();
        let mut session = session_on(dir.path());
        let file = dir.path().join("a.txt");
        session.accept_dropped(vec![file.clone()]);
        session.accept_dropped(vec![file]);
        assert_eq!(session.free_select.len(), 1);
    }

    #[test]
    fn clearing_free_select_empties_the_listing() {
        let dir = tree();
        let mut session = session_on(dir.path());
        session.accept_dropped(vec![dir.path().join("a.txt")]);
        session.refresh_now();
        session.clear_free_select();
        session.refresh_now();
        assert!(session.is_empty());
    }

    fn row(state: RowState, actions: Vec<ren_core::PlannedAction>) -> PlanItem {
        PlanItem {
            index: 0,
            source: "/tmp/a.txt".into(),
            new_name: "a.txt".into(),
            target: "/tmp/a.txt".into(),
            state,
            actions,
        }
    }

    fn an_action() -> Vec<ren_core::PlannedAction> {
        vec![ren_core::PlannedAction {
            step: 0,
            op: "set_attributes",
            effect: ren_core::Effect::Attributes(ren_platform::AttributeChange {
                read_only: Some(false),
                ..Default::default()
            }),
            undoability: ren_core::Undoability::Journaled,
            describe: "Write Protect off".into(),
        }]
    }

    #[test]
    fn the_row_filter_chip_selects_by_state() {
        assert!(RowFilter::All.accepts(&row(RowState::Unchanged, vec![])));
        assert!(!RowFilter::Changed.accepts(&row(RowState::Unchanged, vec![])));
        assert!(RowFilter::Changed.accepts(&row(RowState::Changed, vec![])));
        assert!(RowFilter::Conflicts.accepts(&row(RowState::Error("boom".into()), vec![])));
        assert!(!RowFilter::Conflicts.accepts(&row(RowState::Changed, vec![])));
    }

    /// The other half of the silent bug the `Cell` classifier fixes: narrowing
    /// to Changed used to hide every row a Set Date pipeline touches, so the
    /// table looked empty and the user pressed Rename anyway.
    #[test]
    fn the_changed_chip_keeps_a_row_that_is_only_acted_on() {
        let acted = row(RowState::Unchanged, an_action());
        assert!(RowFilter::Changed.accepts(&acted));
        assert!(RowFilter::All.accepts(&acted));
        assert!(!RowFilter::Conflicts.accepts(&acted));
    }
}
