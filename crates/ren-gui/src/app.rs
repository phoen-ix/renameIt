//! The application: layout, hotkeys, theme, persistence.
//!
//! Screen S1 of `docs/DESIGN.md` Part 2 §3 — source bar on top, operation panel
//! on the left, file table filling the rest, status and actions along the
//! bottom.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ren_core::model::{FileEntry, Scope};
use ren_core::ops::OpKind;
use ren_core::pipeline::{Pipeline, StepConfig};
use ren_core::{Answers, Plan, PlanItem, PlannedOp, RenameKind, RowState, RunSettings};
use ren_platform::Platform;
use serde::{Deserialize, Serialize};

use crate::dialogs::{FileDialogs, HostDialogs, NoDialogs};
use crate::panels::{
    AskForm, AskOutcome, FileTable, about, ask, confirm, operation, palette, presets, run_settings,
    settings, source_bar, status_bar, visual_assist,
};
use crate::thumbs::Thumbs;
use crate::viewmodel::{
    CardId, CardStack, History, ListingWorker, PreviewWorker, Session, SessionSettings, ViewMode,
};
use crate::widgets::filter_editor::FilterForm;
use crate::widgets::string_list::tidy;

const STORAGE_KEY: &str = "renameit.app";

/// Everything worth remembering between runs. Window geometry is handled by
/// eframe's own `persist_window`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Persisted {
    session: SessionSettings,
    /// What the pipeline is called, for *Save as preset*.
    pipeline_name: String,
    /// The card stack, in the same shape a preset stores (D8).
    ///
    /// `Option`, not a bare `Vec`: absent means "written by a build that had no
    /// card stack", while `Some(vec![])` means the user deleted every card and
    /// meant it. A bare `Vec` cannot tell those apart, and would regrow a card
    /// on every restart.
    steps: Option<Vec<(OpKind, StepConfig)>>,
    /// Counter setup, parts and the tag policy — run-wide, so they outlive any
    /// one operation.
    settings: RunSettings,
    /// Settings ▸ Batch Replace: the list a *new* Batch Replace card copies.
    /// A card keeps its own, so a preset stays self-contained (D35).
    batch_replace: Vec<ren_core::ops::Replace>,
    /// Settings ▸ Music Styles: the patterns Music Rename's radios offer. Same
    /// bargain — the card stores the pattern, never which row it came from.
    music_styles: Vec<String>,
    /// Settings ▸ Startup: what a fresh start deliberately forgets.
    startup: Startup,
    /// Settings ▸ Casing Exceptions: the word lists a *new* Set Casing card
    /// copies. Same bargain as the two above — the card keeps its own, so a
    /// preset stays self-contained (D35).
    casing: ren_core::ops::CasingRules,
    /// Settings ▸ Display: row striping and full-row select.
    table_style: crate::viewmodel::TableStyle,
    /// Settings ▸ Display: which columns the file table shows, in what order.
    /// Reconciled on load, so a blob from a build with fewer columns gains the
    /// rest rather than leaving the header and the body disagreeing.
    columns: crate::viewmodel::Columns,
    /// What each drop-down field has been **run** with, newest first.
    ///
    /// > *"The format field is a drop-down holding previously used format
    /// > strings"* — evidenced by the Free Format box and its siblings, and
    /// > undocumented in the prose.
    ///
    /// Keyed by the field's own id, so *"each DropDown control"* keeps its own
    /// list, capped at `tag_field::HISTORY_ITEMS`.
    field_history: std::collections::BTreeMap<String, Vec<String>>,
    theme: Theme,
    simulate: bool,

    // --- Read from older blobs, never written again. Remove at M6. ---
    /// M3 stored exactly one operation. Migrated into a one-card stack rather
    /// than dropped, so upgrading does not discard what the user had set up.
    ///
    /// Not `Option`, deliberately: eframe stores RON, and RON will not read a
    /// bare value into an `Option` — an M3 blob's `operation:(op:"casing",…)`
    /// fails with "expected option" and takes the whole blob down with it.
    /// `steps` already carries the "was this written by an older build" signal,
    /// so these two only need a default.
    #[serde(skip_serializing)]
    operation: OpKind,
    #[serde(skip_serializing)]
    step: StepConfig,
}

impl Default for Persisted {
    fn default() -> Self {
        Self {
            session: SessionSettings::default(),
            pipeline_name: String::new(),
            steps: None,
            settings: RunSettings::default(),
            batch_replace: ren_core::ops::BatchReplace::default().rules,
            music_styles: ren_core::ops::music_rename::SHIPPED_STYLES
                .iter()
                .map(|style| (*style).to_owned())
                .collect(),
            startup: Startup::default(),
            casing: ren_core::ops::CasingRules::default(),
            table_style: crate::viewmodel::TableStyle::default(),
            columns: crate::viewmodel::Columns::default(),
            field_history: std::collections::BTreeMap::new(),
            theme: Theme::System,
            simulate: false,
            operation: OpKind::default(),
            step: StepConfig::default(),
        }
    }
}

/// What a fresh start deliberately forgets — the Startup settings page.
///
/// Every one of these is *off* by default: the app remembering where you were
/// is the behaviour people expect, and each of these is a reason to override
/// it rather than an improvement on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Startup {
    /// > *"Uncheck 'Subfolders' at startup … reducing the risk of painfully
    /// > long startup times"*
    pub clear_subfolders: bool,
    /// > *"Reset pattern mask to \*.\* at startup"*
    pub clear_pattern: bool,
    /// > *"Clear function input fields on startup - … if you are concerned
    /// > about privacy"*
    ///
    /// Ours drops the whole card stack rather than blanking each field: a
    /// pipeline of eight cards with every box emptied is not privacy, it is a
    /// puzzle. A preset is how you get it back deliberately.
    pub clear_pipeline: bool,
}

impl Startup {
    fn applied_to(self, mut persisted: Persisted) -> Persisted {
        if self.clear_subfolders {
            persisted.session.subfolders = false;
        }
        if self.clear_pattern {
            persisted.session.pattern.clear();
        }
        if self.clear_pipeline {
            // `Some(vec![])` rather than `None`: absent means "written by a
            // build with no card stack" and would restore the one default card
            // (see `steps`), which is not what "cleared" means.
            persisted.steps = Some(Vec::new());
            persisted.pipeline_name.clear();
            // **And the drop-down histories.** D128 justifies this switch with
            // The documented *"if you are concerned about privacy"*, and a
            // history that survived it would still hold every pattern the user
            // had ever run — which would make that sentence untrue.
            persisted.field_history.clear();
        }
        persisted
    }
}

/// D22: egui already knows the system theme, so there is no `dark-light`
/// dependency and no D-Bus stack in the binary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl From<Theme> for egui::ThemePreference {
    fn from(theme: Theme) -> Self {
        match theme {
            Theme::System => Self::System,
            Theme::Light => Self::Light,
            Theme::Dark => Self::Dark,
        }
    }
}

/// One frame's worth of hotkeys, named — the tuple that carried them had grown
/// to nine positional fields.
#[derive(Debug, Clone, Copy)]
struct Hotkeys {
    f2: bool,
    f4: bool,
    f5: bool,
    f6: bool,
    f8: bool,
    f9: bool,
    f12: bool,
    undo: bool,
    palette: bool,
}

/// What a relist was asked for *for*, done once its rows exist.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AfterListing {
    /// F2's Enter: open the editor on the row that holds this file.
    OpenEditorOn(PathBuf),
    /// Backspace: land the keyboard on the folder just left.
    LandOn(PathBuf),
}

pub struct RenameItApp {
    platform: Arc<dyn Platform>,
    session: Session,
    preview: PreviewWorker,
    /// The folder walk, off the frame. `Session::relist_wanted` is drained
    /// into a request here once per frame, and a listing that lands is
    /// installed by `poll_listing`, which is also where everything that used
    /// to follow a synchronous relist now runs.
    listing: ListingWorker,
    /// Something to do once the next listing has landed — the row to reopen
    /// the editor on, the folder to land on — because whatever asked for it
    /// cannot see the rows yet.
    after_listing: Option<AfterListing>,
    /// The decode threads and the texture cache. Owned here rather than by the
    /// table or the grid because both views draw from the one cache, and
    /// because a run, an undo and F9 all have to reach it.
    thumbs: Thumbs,
    history: History,
    stack: CardStack,
    /// The one card whose editor is open. An accordion, keyed by identity so it
    /// survives reorder and delete.
    expanded: Option<CardId>,
    settings: RunSettings,
    filter: FilterForm,
    theme: Theme,
    simulate: bool,

    /// Answers to `<Ask>` and `<Clipboard>`, collected once before a run (D28).
    answers: Answers,
    /// The modal, while it is up.
    asking: Option<AskForm>,
    /// The add-operation palette, while it is open.
    palette: Option<palette::PaletteState>,
    /// The Settings window, while it is open.
    settings_page: Option<settings::Page>,
    /// The About window, while it is open. Session state, not persisted:
    /// nobody wants it back on the next start.
    about_open: bool,
    /// The preset drawer, while it is open.
    drawer: Option<presets::DrawerState>,
    /// The Visual Assist strip, while it is open. At most one, ever — it fills
    /// one field of one card, and two would have nothing to mean.
    visual_assist: Option<crate::panels::visual_assist::VisualAssist>,
    /// The strip's text changed underneath a selection, so it must be dropped
    /// before the next frame reads it. Applied in `show`, because the selection
    /// lives in egui's store and this is computed where there is no context.
    forget_assist_selection: bool,
    /// A selection made through the test seam rather than by dragging.
    pending_assist_selection: Option<(usize, usize)>,
    /// What the run still needs asked, computed with the preview rather than
    /// on every frame.
    pending_ask: Option<AskForm>,
    /// Where presets live.
    presets: ren_core::PresetStore,
    /// The rule list a new Batch Replace card starts from.
    batch_replace: Vec<ren_core::ops::Replace>,
    /// The styles Music Rename's radios offer.
    music_styles: Vec<String>,
    /// What a fresh start forgets, and does not.
    startup: Startup,
    /// The casing exception lists a new Set Casing card starts from.
    casing: ren_core::ops::CasingRules,
    /// Row striping and full-row select.
    table_style: crate::viewmodel::TableStyle,
    /// Which columns the file table shows, in what order.
    columns: crate::viewmodel::Columns,
    /// See `Persisted::field_history`.
    field_history: std::collections::BTreeMap<String, Vec<String>>,
    /// Anything the cards asked for while they were being drawn, read once the
    /// stack is finished with.
    requests: crate::editors::EditorRequests,
    /// Set when the answers changed and the run should start as soon as the
    /// preview has caught up with them.
    run_when_ready: bool,

    /// Entry indices the *latest request* was computed over, in plan order.
    /// The plan on screen carries its own copy (`Ready::scoped`), which is
    /// the one the table is addressed through; this one serves the things
    /// that run at request time — the assist subject, the running counter.
    scoped: Vec<usize>,
    /// Entry index → index into `plan.items`, rebuilt from `Ready::scoped`
    /// whenever a plan lands. Between a relist or a sort and the next plan it
    /// can point at rows that have moved, which is why `rows::item_of`
    /// checks the file before trusting it.
    plan_index: Vec<Option<usize>>,
    inline_rename: Option<crate::panels::rows::InlineRename>,
    show_log: bool,
    /// Set when something changed that the worker has not been told about yet.
    needs_preview: bool,
    /// Set when the preset folder may have changed under a menu that is a
    /// snapshot of it. Drained once per frame, not applied where it is set:
    /// rewriting the registry inside `apply_drawer` would do it mid-frame with
    /// the drawer's borrow still live.
    needs_menu_rewrite: bool,
    /// Settings ▸ Problem Solver asked to reset, and the confirmation is up.
    confirming_reset: bool,
    /// A row the keyboard reached that the view should bring into sight.
    /// An **entry** index; each view translates to its own row numbering.
    scroll_to: Option<usize>,
    /// A right-click whose selection came close to what Windows will carry.
    /// `None` for every other way of arriving here — see [`SelectionCaveat`].
    selection_caveat: Option<SelectionCaveat>,
    /// P2's consent, bound to the preview generation it was given for.
    ///
    /// A bool could not carry the guarantee its own comment claimed. `run()`
    /// *defers* when the preview is stale, so a synchronously-cleared bool
    /// loses the consent it was set for — and the obvious repair, clearing it
    /// after the run instead, fails the other way: the deferred run waits for a
    /// *new* preview and then applies a plan the user never saw, under an
    /// approval given for a different one. A bool has no way to tell those
    /// apart, because it does not know which plan it is about.
    ///
    /// The generation does. It already means exactly "which plan is this", it
    /// only ever increases, and every path that could invalidate consent
    /// already bumps it — so there is no invalidation list to keep in sync,
    /// which is the part that rots.
    consented_for: Option<u64>,
    /// The confirmation, while it is up.
    confirming: Option<confirm::Confirm>,
    status: Option<String>,
    /// The native file pickers, or stubs that answer nothing.
    ///
    /// Injected rather than compiled in: a `#[cfg(test)]` stub does not reach
    /// an integration test, and a real dialog in CI waits forever.
    dialogs: Box<dyn FileDialogs>,
    /// Set on the first headless frame, to paint with no animation.
    ///
    /// D26 bans animated *widgets* because a continuously repainting frame
    /// never settles. Animation is the same hazard one step down: egui_kittest
    /// gives `Harness::run` four steps to reach a still frame, and every
    /// animated transition spends some of them. Cleared once applied — the
    /// styles live in the context, not here. The real app keeps its animations;
    /// the tests do not need them to prove anything.
    instant: bool,
    /// One-shot: install `theme` on the first frame. See `show`.
    unstyled: bool,
}

impl RenameItApp {
    pub fn new(ctx: &egui::Context, storage: Option<&dyn eframe::Storage>) -> Self {
        let persisted: Persisted = storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();

        // A first run seeds the script folder with the nine worked examples.
        // Never overwrites, so an edited script survives an upgrade — and a
        // failure here is not worth refusing to start over.
        let _ = ren_core::script::ScriptStore::user().seed_defaults();

        // And the six presets, the same arrangement — except that this one
        // seeds **only into a folder that does not exist**. A script the user
        // deleted is a file they stopped using; a preset they deleted is a
        // menu item they took out of their file manager (D-preset-menu), and
        // putting it back every start is the app arguing about their own
        // right-click menu.
        let _ = ren_core::PresetStore::user().seed_defaults();

        let repaint_ctx = ctx.clone();
        let mut app = Self::build(
            persisted,
            History::default(),
            Box::new(HostDialogs),
            ren_core::PresetStore::user(),
            ren_platform::host(),
            move || repaint_ctx.request_repaint(),
        );
        app.expand_first();
        // A fresh seed per application session, overriding whatever was
        // persisted. `<Rnd*>` and `Unique Random Number` would otherwise
        // produce the same values on every run of the app forever, because
        // nothing else ever sets it — and two batches renamed a week apart
        // would collide. Per session rather than per preview, so P16's
        // "the preview and the rename that follows it agree" still holds.
        app.settings.reseed();
        ctx.set_theme(egui::ThemePreference::from(app.theme));
        app.session.request_refresh();
        app
    }

    /// An app with no window behind it, pointed at one directory.
    ///
    /// This is what the headless test suite drives (D24). It takes no
    /// `egui::Context` because there is nothing to wake: tests step frames
    /// themselves.
    pub fn headless(dir: PathBuf, journal_dir: PathBuf) -> Self {
        Self::headless_with_dialogs(dir, journal_dir, Box::new(NoDialogs))
    }

    /// The same, with the platform supplied.
    ///
    /// The Explorer menu is the only thing in the app that writes to a place
    /// the host owns and reads its own state back out of it, so it is the only
    /// thing whose behaviour cannot be seen through the real `host()` on the
    /// runner that has no such menu. A construction-time choice, like
    /// `headless_with_dialogs` — not a setter, because a platform swapped after
    /// the preview worker already holds one would be two different answers to
    /// the same question.
    pub fn headless_with_platform(
        dir: PathBuf,
        journal_dir: PathBuf,
        platform: Arc<dyn Platform>,
    ) -> Self {
        Self::headless_inner(dir, journal_dir, Box::new(NoDialogs), platform)
    }

    /// The same, with the file pickers supplied.
    ///
    /// For a test that needs one *answered* — F12 opens the folder browser, and
    /// `NoDialogs` cancels every dialog by design. Which pickers the app has is
    /// a construction-time choice already (see `dialogs`), so this is that same
    /// choice made one step earlier rather than a test-only back door.
    pub fn headless_with_dialogs(
        dir: PathBuf,
        journal_dir: PathBuf,
        dialogs: Box<dyn FileDialogs>,
    ) -> Self {
        Self::headless_inner(dir, journal_dir, dialogs, ren_platform::host())
    }

    fn headless_inner(
        dir: PathBuf,
        journal_dir: PathBuf,
        dialogs: Box<dyn FileDialogs>,
        platform: Arc<dyn Platform>,
    ) -> Self {
        let persisted = Persisted {
            session: SessionSettings {
                dir,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut app = Self::build(
            persisted,
            History::in_dir(journal_dir.clone()),
            dialogs,
            // Beside the journal, so every headless test gets its own.
            ren_core::PresetStore::new(journal_dir.join("presets")),
            platform,
            || {},
        );
        app.expand_first();
        app.instant = true;
        // Through the worker, like the real window — `settle` waits for it.
        app.session.request_refresh();
        app.drain_listing_request();
        app
    }

    /// The card stack a stored blob describes, migrating M3's single operation.
    fn restore(persisted: &Persisted) -> CardStack {
        let name = persisted.pipeline_name.clone();
        match &persisted.steps {
            Some(steps) => CardStack::from_steps(name, steps.clone()),
            // An M3 blob, or none at all — either way, one card.
            None => CardStack::from_steps(
                name,
                vec![(persisted.operation.clone(), persisted.step.clone())],
            ),
        }
    }

    fn build(
        persisted: Persisted,
        history: History,
        dialogs: Box<dyn FileDialogs>,
        preset_store: ren_core::PresetStore,
        platform: Arc<dyn Platform>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        // Before anything is built from it, because these decide what it is
        // built *from*. Applying them to the finished app would list the old
        // folder first — which for "uncheck Subfolders" is the very walk the
        // setting exists to avoid (D128). Here rather than in `new` because
        // this is the one constructor both entry points go through, and a
        // startup setting that only worked in the real binary would be a
        // setting nothing could test.
        let persisted = persisted.startup.applied_to(persisted);
        // Shared rather than duplicated: both workers wake the same window, and
        // `request_repaint` is idempotent within a frame, so two of them
        // finishing at once still costs one frame.
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(repaint);
        Self {
            dialogs,
            preview: PreviewWorker::spawn(platform.clone(), {
                let repaint = repaint.clone();
                move || repaint()
            }),
            listing: ListingWorker::spawn({
                let repaint = repaint.clone();
                move || repaint()
            }),
            after_listing: None,
            thumbs: Thumbs::new(move || repaint()),
            platform,
            stack: Self::restore(&persisted),
            session: Session::new(persisted.session),
            history,
            expanded: None,
            settings: persisted.settings,
            filter: FilterForm::default(),
            answers: Answers::default(),
            asking: None,
            palette: None,
            settings_page: None,
            about_open: false,
            drawer: None,
            visual_assist: None,
            forget_assist_selection: false,
            pending_assist_selection: None,
            pending_ask: None,
            presets: preset_store,
            batch_replace: persisted.batch_replace,
            music_styles: persisted.music_styles,
            startup: persisted.startup,
            casing: persisted.casing,
            table_style: persisted.table_style,
            field_history: persisted.field_history,
            columns: {
                // A blob from a build with fewer columns is missing the rest,
                // and one from a build with more carries columns this one
                // cannot render. Either would leave the header and the body
                // indexing different lists.
                let mut columns = persisted.columns;
                columns.reconcile();
                columns
            },
            requests: Default::default(),
            run_when_ready: false,
            theme: persisted.theme,
            simulate: persisted.simulate,
            scoped: Vec::new(),
            plan_index: Vec::new(),
            inline_rename: None,
            show_log: false,
            needs_preview: true,
            needs_menu_rewrite: false,
            confirming_reset: false,
            scroll_to: None,
            selection_caveat: None,
            consented_for: None,
            confirming: None,
            status: None,
            instant: false,
            unstyled: true,
        }
    }

    /// Blocks until both workers have caught up with what the last frame asked
    /// for.
    ///
    /// The UI itself must never do this — it exists so a test can assert on a
    /// settled plan and a drawn tile instead of racing them.
    ///
    /// Two loops sharing one deadline. The thumbnails are second because they
    /// are only ever requested by a frame that has already been drawn, so
    /// waiting for them first would wait for a set nothing asked for yet.
    pub fn settle(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        // The listing first: it is what the preview is computed over, and a
        // listing landing asks for a preview, so waiting for the preview
        // first would wait for one that is about to be superseded.
        self.drain_listing_request();
        // Until the *latest* request is answered, not until anything lands: a
        // listing for the folder before this one can arrive first, and
        // stopping there would settle on the wrong rows.
        while self.listing.is_listing() && std::time::Instant::now() < deadline {
            self.poll_listing();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.poll_listing();

        if self.needs_preview {
            self.request_preview();
        }
        while self.preview.is_stale() && std::time::Instant::now() < deadline {
            self.preview.poll();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.preview.poll();
        self.rebuild_plan_index();

        // A decode that never answers would spend this whole deadline on every
        // call, which is why a refusal is an answer (D136) and why `pending`
        // drains when the threads go away.
        while self.thumbs.pending() > 0 && std::time::Instant::now() < deadline {
            self.thumbs.poll();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.thumbs.poll();
    }

    /// Turns the session's "a relist is wanted" into a request to the worker.
    ///
    /// Once per frame, after everything that could have asked: the session
    /// cannot send the request itself (it has no worker and no thread), and
    /// several things in one frame asking for one relist should cost one.
    fn drain_listing_request(&mut self) {
        if std::mem::take(&mut self.session.relist_wanted) {
            self.listing.request(self.session.source());
        }
    }

    /// Installs a listing the worker has finished, if there is one.
    ///
    /// Returns true if one landed. Everything that used to follow a
    /// synchronous relist runs here: the preview is asked for over the new
    /// rows, the index into the old plan is dropped, and whatever was waiting
    /// for the rows to exist gets them.
    fn poll_listing(&mut self) -> bool {
        let Some(listed) = self.listing.poll() else {
            return false;
        };
        self.session.install_listing(listed.outcome);
        self.needs_preview = true;
        // The plan on screen is over rows that no longer exist; `item_of`
        // checks each row's file before trusting the index, so nothing wrong
        // is drawn, but an empty index is the honest state until the next
        // plan lands.
        self.plan_index.clear();
        match self.after_listing.take() {
            Some(AfterListing::OpenEditorOn(path)) => {
                self.open_editor_on_the_next_row(Some(path));
            }
            Some(AfterListing::LandOn(path)) => {
                let back = self
                    .session
                    .entries()
                    .iter()
                    .position(|entry| entry.path == path);
                self.session.selection.set_lead(back);
                self.scroll_to = back;
            }
            None => {}
        }
        true
    }

    /// The thumbnail cache and its decode threads.
    pub fn thumbs(&self) -> &Thumbs {
        &self.thumbs
    }

    /// Sets the source-bar include filter, as the popover would.
    pub fn set_filter(&mut self, filter: FilterForm) {
        self.filter = filter;
        self.needs_preview = true;
    }

    /// Simulate mode, as the status-bar checkbox would set it.
    pub fn set_simulate(&mut self, simulate: bool) {
        self.simulate = simulate;
    }

    pub fn set_row_filter(&mut self, filter: crate::viewmodel::RowFilter) {
        self.session.settings.row_filter = filter;
    }

    /// Runs the current plan, as the Rename button would.
    pub fn run_now(&mut self) {
        self.run();
    }

    /// Runs it with P2's consent already given, skipping the dialog.
    ///
    /// The door the confirmation goes through, exposed for tests that are about
    /// something else. It binds to the plan currently delivered, so it must be
    /// called on a settled preview — the same requirement the dialog satisfies
    /// by construction, since it can only open once one exists.
    pub fn run_allowing_irreversible(&mut self) {
        self.consented_for = self.preview.ready().map(|r| r.generation);
        self.run();
    }

    /// Undoes the last batch, as the Undo button would.
    pub fn undo_now(&mut self) {
        self.undo();
    }

    pub fn select(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.session.selection.set(indices);
        self.needs_preview = true;
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Mutable access for tests and for the drag-drop path.
    pub fn session_mut(&mut self) -> &mut Session {
        self.needs_preview = true;
        &mut self.session
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Opens on what the command line named, if it named anything.
    ///
    /// With no switches, a bare command line is a list of files to load into
    /// free select mode, or a single folder to start in.
    ///
    /// One folder browses there; anything else goes to Free Select — which is
    /// exactly what a drag from the file manager already does, so this reuses
    /// `accept_dropped` rather than inventing a second reading of the same
    /// question. Paths that do not exist are dropped here rather than reported:
    /// the shell can hand us a file that was deleted since the menu opened, and
    /// an error dialog before the window has been seen is not a good greeting.
    ///
    /// **The mode is forced first, and that is not what `accept_dropped` does.**
    /// It navigates only when the mode is *already* Browser, because a drag of
    /// a folder into Free Select genuinely means "add this to the list". A
    /// command line naming one folder has never meant that — but `mode` is
    /// persisted and nothing resets it, so a user whose last session ended in
    /// Free Select got the folder as a single row instead of a listing of what
    /// is inside it.
    ///
    /// The condition deliberately mirrors `accept_dropped`'s own, which
    /// re-checks it — so widening this to every path set changes nothing and
    /// no test catches that. What is load-bearing here is the *mode*, not the
    /// decision; the condition is written out so the next reader can see which
    /// case is being overridden rather than infer it.
    pub fn start_at(&mut self, paths: Vec<std::path::PathBuf>) {
        let real: Vec<_> = paths.into_iter().filter(|p| p.exists()).collect();
        if real.is_empty() {
            return;
        }
        if real.len() == 1 && real[0].is_dir() {
            self.session.settings.mode = crate::viewmodel::SourceMode::Browser;
        }
        self.session.accept_dropped(real);
        self.needs_preview = true;
    }

    /// Opens on whatever the command line asked for.
    ///
    /// One place rather than four, because the order matters and is easy to get
    /// wrong: the source is set **first**, so the preset loads against a
    /// listing that already exists and the status line the user reads is the
    /// preset's rather than the listing's.
    ///
    /// It never renames. A preset chosen from the Explorer menu loads, lists
    /// the selection and shows the preview — **P4** blocks a conflicting plan
    /// and **P2** gates an irreversible one, and a right-click has nowhere to
    /// answer either.
    pub fn start_from(&mut self, launch: &crate::launch::Launch) {
        if launch.start_in {
            // "Start from this folder": the folder the first path is *in*, or
            // the path itself when it is already a folder. Resolved here rather
            // than by asking Windows for `%W`, whose behaviour for a static
            // verb no test of ours can reach.
            if let Some(first) = launch.paths.iter().find(|p| p.exists()) {
                let dir = if first.is_dir() {
                    first.clone()
                } else {
                    first.parent().unwrap_or(first).to_path_buf()
                };
                self.session.settings.mode = crate::viewmodel::SourceMode::Browser;
                self.set_dir(dir);
            }
        } else {
            self.start_at(launch.paths.clone());
        }

        if let Some(file) = &launch.preset {
            self.load_preset(file);
        }
        if let Some(complaint) = &launch.complaint {
            self.status = Some(complaint.clone());
        }

        self.selection_caveat = SelectionCaveat::for_launch(launch);

        // **Startup repair.** The menu is a snapshot of a folder that anything
        // can change while the program is not running: a preset dropped in by
        // hand, one deleted in Explorer, a second install of RenameIt, or a
        // portable copy that moved and left every command line pointing at
        // where it used to be. One rewrite here fixes all four, and retires the
        // manual-check row that told the user to tick the box again.
        //
        // Here and not in `new`, deliberately: `start_from` is reached only
        // from `main`, so no test — and no headless run on a developer's own
        // machine — can write to the real registry.
        self.rewrite_menu();
    }

    /// The second half of *"jump to the next item in the file list"*.
    ///
    /// **Moves the lead, never the selection.** A rename is not the user
    /// choosing which files a run covers; silently narrowing the scope to one
    /// file would empty the New-name column for every other row.
    fn open_editor_on_the_next_row(&mut self, successor: Option<std::path::PathBuf>) {
        let Some(path) = successor else { return };
        let Some(next) = self
            .session
            .entries()
            .iter()
            .position(|entry| entry.path == path)
        else {
            return;
        };
        let name = self.session.entries()[next].file_name.clone();
        self.session.selection.set_lead(Some(next));
        self.inline_rename = Some(crate::panels::rows::InlineRename::opening(next, &name));
        self.scroll_to = Some(next);
    }

    /// The strings each drop-down field remembers, newest first.
    pub fn field_history(&self, id: &str) -> &[String] {
        self.field_history.get(id).map_or(&[], Vec::as_slice)
    }

    /// Records what this run's pattern fields were set to.
    ///
    /// **When the run is committed, not on every keystroke.** The combo holds
    /// *previously used* strings; recording as you type fills it with
    /// the prefixes of one string. A simulation counts — the user still said
    /// they meant it.
    ///
    /// Walks the stack rather than the widgets, so a pattern on a *collapsed*
    /// card is remembered too: the card is in the run whether or not it is on
    /// screen.
    fn remember_fields(&mut self) {
        for (op, _) in self.stack.to_steps() {
            for (id, text) in history_entries(&op) {
                if text.trim().is_empty() {
                    continue;
                }
                let list = self.field_history.entry(id.to_owned()).or_default();
                list.retain(|previous| *previous != text);
                list.insert(0, text);
                list.truncate(crate::widgets::tag_field::HISTORY_ITEMS);
            }
        }
    }

    /// Points the browser somewhere else and relists, as the address box does.
    pub fn set_dir(&mut self, dir: std::path::PathBuf) {
        self.session.settings.dir = dir;
        self.session.request_refresh();
    }

    /// F9: *"Refresh the file list."* — reads the folder again, and forgets
    /// everything that was read from the files themselves.
    ///
    /// The relist alone was a half-refresh. `Session::refresh` re-reads names,
    /// sizes and dates, because those come from the directory entry; every
    /// *metadata* tag — the Exif date, the MP3 artist, the `<Width>` of a
    /// picture, the thumbnail — comes from a process-wide cache keyed on path,
    /// length and mtime, and none of those was touched. So a file edited in
    /// another program by a tool that preserves its size and mtime kept showing
    /// its old tags, and pressing Refresh confirmed the stale value rather than
    /// correcting it.
    ///
    /// The listing's own `refresh()` deliberately does **not** do this. It runs
    /// after every rename and every undo, where forgetting would undo the
    /// thumbnail cache's rekey and re-read the tags of every file in the folder
    /// on the app's commonest workflow (D140).
    pub fn forget_and_relist(&mut self) {
        ren_core::meta::audio::forget_all();
        ren_core::meta::exif::forget_all();
        ren_core::meta::folder::forget_all();
        ren_core::meta::html::forget_all();
        ren_core::meta::image::forget_all();
        self.thumbs.clear();
        self.session.request_refresh();
    }

    /// Row striping and full-row select (Settings ▸ Display).
    pub fn table_style(&self) -> crate::viewmodel::TableStyle {
        self.table_style
    }

    /// Turns a column on or off, as Settings ▸ Display would.
    ///
    /// The settings page reaches `columns` through `Defaults`; this is the same
    /// change made from outside, for a caller that wants one column rather than
    /// the whole page.
    pub fn show_column(&mut self, kind: crate::viewmodel::ColumnKind, visible: bool) {
        if let Some(column) = self.columns.all_mut().iter_mut().find(|c| c.kind == kind) {
            column.visible = visible;
        }
    }

    /// What a fresh start forgets (Settings ▸ Startup).
    pub fn startup(&self) -> Startup {
        self.startup
    }

    /// The exception words a *new* Set Casing card copies (Settings ▸ Casing
    /// Exceptions).
    pub fn casing_exception_words(&self) -> &[String] {
        &self.casing.exceptions.words
    }

    /// The first card's operation — what M2's single-operation panel edited.
    ///
    /// An empty pipeline grows a card, so this is always valid.
    pub fn operation_mut(&mut self) -> &mut OpKind {
        self.needs_preview = true;
        if self.stack.is_empty() {
            let id = self.stack.push(OpKind::default());
            self.expanded = Some(id);
        }
        &mut self.stack.get_mut(0).expect("just ensured").op
    }

    /// Process Name / Process Extension for the first card, as its Scope
    /// expander would set it.
    pub fn set_scope(&mut self, scope: Scope) {
        self.set_card_scope(0, scope);
    }

    pub fn stack(&self) -> &CardStack {
        &self.stack
    }

    /// The card stack, for tests and for the panel. Marks the preview stale.
    pub fn stack_mut(&mut self) -> &mut CardStack {
        self.needs_preview = true;
        &mut self.stack
    }

    /// Adds an operation and opens its editor, as the palette does.
    pub fn add_operation(&mut self, mut op: OpKind) -> CardId {
        // A fresh Batch Replace starts from the list in Settings. It then keeps
        // its own copy, so editing the default later never rewrites a pipeline
        // that has already been built — or a preset already saved (D35).
        if let OpKind::BatchReplace(batch) = &mut op {
            batch.rules = self.batch_replace.clone();
        }
        // A fresh Set Date card opens on *today*. `WallClock::default()` is a
        // deterministic 1980-01-01 so the engine's tests have a fixed instant;
        // showing that to a user as the starting value is a date nobody meant,
        // one click away from being written to every selected file.
        // And a fresh Set Casing starts from the exception lists in Settings,
        // for the same reason and with the same promise.
        if let OpKind::Casing(casing) = &mut op {
            casing.rules = self.casing.clone();
        }
        if let OpKind::SetDate(set_date) = &mut op {
            set_date.date =
                ren_core::ops::WallClock::from_naive(chrono::Local::now().naive_local());
        }
        let after = self.expanded.and_then(|id| self.stack.index_of(id));
        let id = self.stack.insert_after(after, op);
        self.expanded = Some(id);
        self.needs_preview = true;
        id
    }

    pub fn set_card_enabled(&mut self, index: usize, enabled: bool) {
        if let Some(card) = self.stack.get_mut(index) {
            card.enabled = enabled;
            self.needs_preview = true;
        }
    }

    pub fn set_card_scope(&mut self, index: usize, scope: Scope) {
        if let Some(card) = self.stack.get_mut(index) {
            card.scope = scope;
            self.needs_preview = true;
        }
    }

    pub fn set_card_filter(&mut self, index: usize, filter: Option<FilterForm>) {
        if let Some(card) = self.stack.get_mut(index) {
            card.filter = filter;
            self.needs_preview = true;
        }
    }

    pub fn move_card(&mut self, from: usize, to: usize) {
        if self.stack.move_card(from, to) {
            self.needs_preview = true;
        }
    }

    pub fn duplicate_card(&mut self, index: usize) {
        if let Some(id) = self.stack.duplicate(index) {
            self.expanded = Some(id);
            self.needs_preview = true;
        }
    }

    pub fn delete_card(&mut self, index: usize) {
        if self.stack.remove(index).is_some() {
            if self
                .expanded
                .is_some_and(|id| self.stack.index_of(id).is_none())
            {
                self.expanded = self.stack.cards().first().map(|c| c.id);
            }
            self.needs_preview = true;
        }
    }

    /// Opens one card's editor, closing whichever was open.
    pub fn expand_card(&mut self, index: usize) {
        self.expanded = self.stack.get(index).map(|c| c.id);
    }

    /// Opens the add-operation palette, as `+ Add operation` and Ctrl+K do.
    pub fn open_palette(&mut self) {
        self.palette = Some(palette::PaletteState::default());
    }

    /// Opens the Settings window, as the ⚙ button does.
    pub fn open_settings(&mut self) {
        self.settings_page = Some(settings::Page::default());
    }

    /// Opens the preset drawer.
    pub fn open_presets(&mut self) {
        self.drawer = Some(presets::DrawerState::opened(&self.stack.name));
    }

    /// The drawer's state while it is open, for tests.
    pub fn drawer(&self) -> Option<&presets::DrawerState> {
        self.drawer.as_ref()
    }

    /// The plan item the table shows on row `index`, if the current plan has
    /// one for the file that is there — the same lookup every cell makes.
    pub fn item_for_row(&self, index: usize) -> Option<&ren_core::PlanItem> {
        crate::panels::rows::item_of(
            self.session.entries(),
            self.preview.plan(),
            &self.plan_index,
            index,
        )
    }

    fn apply_drawer(&mut self, out: presets::DrawerOutput) {
        let asked = out.asked_for_something();
        if let Some(name) = out.save_as {
            self.save_preset(&name);
        }
        if let Some(path) = out.load {
            self.load_preset(&path);
            // The name box follows the pipeline it would save: after a load
            // that is the loaded preset's name, whatever was typed before.
            if let Some(drawer) = &mut self.drawer {
                drawer.new_name = self.stack.name.clone();
            }
        }
        if let Some(path) = out.append {
            self.append_preset(&path);
        }
        if let Some(path) = out.run {
            self.run_preset(&path);
        }
        if let Some((path, name)) = out.rename {
            self.rename_preset(&path, &name);
        }
        if let Some(path) = out.duplicate {
            self.duplicate_preset(&path);
        }
        if let Some(path) = out.delete {
            self.delete_preset(&path);
        }
        if let Some(path) = out.import {
            self.import_preset_from(&path);
        }
        if let Some((path, to)) = out.export {
            self.export_preset_to(&path, &to);
        }
        // The one funnel every preset mutation already goes through. Setting
        // the flag here rather than in each of the five that change the folder
        // is what stops a tenth drawer action forgetting — see
        // `DrawerOutput::asked_for_something`.
        if asked {
            self.needs_menu_rewrite = true;
        }
    }

    /// Everything worth remembering, as `save` writes it.
    fn persisted(&self) -> Persisted {
        Persisted {
            session: self.session.settings.clone(),
            pipeline_name: self.stack.name.clone(),
            steps: Some(self.stack.to_steps()),
            settings: self.settings.clone(),
            batch_replace: self.batch_replace.clone(),
            music_styles: self.music_styles.clone(),
            startup: self.startup,
            casing: self.casing.clone(),
            table_style: self.table_style,
            columns: self.columns.clone(),
            field_history: self.field_history.clone(),
            theme: self.theme,
            simulate: self.simulate,
            operation: OpKind::default(),
            step: StepConfig::default(),
        }
    }

    /// > *"Probably to most useful tool is the reset to default settings
    /// > button, which usually fixes any problems you might have!"*
    ///
    /// **The rule that makes the split decidable: it puts back exactly what a
    /// Settings page owns, and does not touch what the source bar and the table
    /// header own.** `SessionSettings` mixes the two — `dir` and `pattern` are
    /// where you are, `thumb_size` and `show_hidden` are how it looks — so this
    /// cannot be a wholesale `Persisted::default()`.
    ///
    /// **Deliberately narrow (D156).** A reset that emptied the whole settings
    /// folder would take presets and scripts with it. Ours spares the undo
    /// journal — a **safety** exclusion rather than a convenience one, because
    /// it is the only way back from the last run — and spares presets and
    /// scripts, which the page above
    /// this button already treats as the user's own files.
    fn reset_settings(&mut self, ctx: &egui::Context) {
        let fresh = Persisted::default();

        // What a Settings page owns.
        self.batch_replace = fresh.batch_replace.clone();
        self.music_styles = fresh.music_styles.clone();
        self.casing = fresh.casing.clone();
        self.startup = fresh.startup;
        self.table_style = fresh.table_style;
        self.columns = fresh.columns.clone();
        self.field_history = fresh.field_history.clone();
        self.theme = fresh.theme;
        self.simulate = fresh.simulate;
        self.settings = fresh.settings.clone();
        // `RunSettings::seed` documents zero as "nobody has chosen one", and
        // only `new` ever replaced it. Adopting the default verbatim would leave
        // every `<Rnd>` in the session producing the same value forever, and two
        // batches renamed a week apart colliding.
        self.settings.reseed();

        // The File System and Display pages' half of `SessionSettings`, field by
        // field. `guard_system_folders` coming back **on** is the point rather
        // than a side effect: it is the one switch whose default is the safe one
        // (D127).
        let live = &mut self.session.settings;
        live.guard_system_folders = fresh.session.guard_system_folders;
        live.show_hidden = fresh.session.show_hidden;
        live.show_system = fresh.session.show_system;
        live.show_read_only = fresh.session.show_read_only;
        live.pattern_applies = fresh.session.pattern_applies;
        live.view = fresh.session.view;
        live.thumb_size = fresh.session.thumb_size;
        live.thumb_border = fresh.session.thumb_border;

        // Not in `Persisted` and never saved, but a stray one is exactly the
        // *"some files are missing from the list"* the page above answers.
        self.filter = FilterForm::default();

        ctx.set_theme(egui::ThemePreference::from(self.theme));
        // The interface size lives in egui's own `Memory`, not in `Persisted`
        // — which is why it is easy to leave out of a reset that walks
        // `Persisted` field by field, and why it is put back by hand here.
        ctx.set_zoom_factor(1.0);
        // The visibility switches decide what is *in* the list.
        self.session.request_refresh();
        self.status = Some("Settings restored to their defaults".to_owned());
    }

    /// Rewrites the Explorer menu from the presets that are on disk *now*.
    ///
    /// **Only when it is already installed.** This is a repair, not an install:
    /// saving a preset must never be the thing that puts an entry in someone's
    /// context menu, and a user who has never ticked the box must not acquire
    /// one by using the program.
    ///
    /// Silent on success and on "nothing to do", because it runs as a side
    /// effect of unrelated actions. A failure is worth the status line: the
    /// menu is now out of step with the folder, and only saying so gives the
    /// user anything to do about it.
    fn rewrite_menu(&mut self) {
        if self.platform.context_menu_installed() != Some(true) {
            return;
        }
        let (entries, _) = self.presets.list();
        let menu = menu_presets(&entries);
        if let Err(error) = self.platform.set_context_menu(true, &menu) {
            self.status = Some(format!("Could not update the file manager menu: {error}"));
        }
    }

    /// The presets on disk, as the drawer lists them.
    pub fn preset_names(&self) -> Vec<String> {
        self.presets.list().0.into_iter().map(|e| e.name).collect()
    }

    /// Saves the current pipeline under a name.
    pub fn save_preset(&mut self, name: &str) {
        let preset = ren_core::Preset {
            name: name.to_owned(),
            description: String::new(),
            steps: self.stack.to_steps(),
            settings: self.settings.clone(),
        };
        match self.presets.save(&preset) {
            Ok(_) => {
                self.stack.name = name.to_owned();
                self.status = Some(format!("Saved preset “{name}”"));
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Replaces the pipeline, and the run-wide settings that came with it.
    ///
    /// **D34.** Load takes the whole saved pipeline: its cards *and* its
    /// counter, parts and tag policy, because those are part of what was saved.
    /// Append takes only the cards — see [`RenameItApp::append_preset`].
    pub fn load_preset(&mut self, path: &Path) {
        match self.presets.load(path) {
            Ok((preset, notes)) => {
                self.stack = CardStack::from_steps(preset.name.clone(), preset.steps);
                self.settings = preset.settings;
                // Whatever the preset carried, this session's seed wins. A
                // preset written by an older build may still hold one, and a
                // frozen seed makes every `<Rnd*>` in the pipeline produce the
                // same names on every run of it.
                self.settings.reseed();
                // A different pipeline asks different questions.
                self.answers = Answers::default();
                self.expand_first();
                self.needs_preview = true;
                self.status = Some(match notes.dropped_source {
                    Some(_) => format!(
                        "Loaded “{}” — it named a folder of its own, which presets ignore",
                        preset.name
                    ),
                    None => format!("Loaded “{}”", preset.name),
                });
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Adds a preset's operations to the end of this pipeline.
    ///
    /// Its run-wide settings are *not* taken: inheriting someone else's counter
    /// start when you meant to add two operations is a nasty surprise (D34).
    pub fn append_preset(&mut self, path: &Path) {
        match self.presets.load(path) {
            Ok((preset, _)) => {
                let added = preset.steps.len();
                self.stack.append_steps(preset.steps);
                self.answers = Answers::default();
                self.needs_preview = true;
                self.status = Some(format!("Appended {}", ren_core::plural(added, "operation")));
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Loads a preset and renames with it.
    ///
    /// The run waits for the preview to catch up: pressing Run must never apply
    /// the plan the *previous* pipeline produced.
    pub fn run_preset(&mut self, path: &Path) {
        self.load_preset(path);
        if self
            .status
            .as_deref()
            .is_some_and(|s| s.starts_with("Loaded"))
        {
            self.run_when_ready = true;
        }
    }

    pub fn rename_preset(&mut self, path: &Path, new_name: &str) {
        if let Err(e) = self.presets.rename(path, new_name) {
            self.status = Some(e.to_string());
        }
    }

    pub fn duplicate_preset(&mut self, path: &Path) {
        if let Err(e) = self.presets.duplicate(path) {
            self.status = Some(e.to_string());
        }
    }

    pub fn delete_preset(&mut self, path: &Path) {
        if let Err(e) = self.presets.delete(path) {
            self.status = Some(e.to_string());
        }
    }

    /// Copies an outside file into the preset folder.
    pub fn import_preset_from(&mut self, path: &Path) {
        match self.presets.import(path) {
            Ok((_, notes)) => {
                self.status = Some(if notes.dropped_source.is_some() {
                    "Imported — the folder it named was dropped, as presets have none".to_owned()
                } else {
                    "Imported".to_owned()
                })
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Writes a preset out to a file the user can share.
    pub fn export_preset_to(&mut self, path: &Path, to: &Path) {
        let result = self
            .presets
            .load(path)
            .and_then(|(preset, _)| self.presets.export(&preset, to));
        match result {
            Ok(()) => self.status = Some(format!("Exported to {}", to.display())),
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// The rule list a new Batch Replace card starts from.
    pub fn batch_replace_rules(&self) -> &[ren_core::ops::Replace] {
        &self.batch_replace
    }

    /// The styles Music Rename's radios offer.
    pub fn music_styles(&self) -> &[String] {
        &self.music_styles
    }

    /// The run-wide settings — the counter, Setup Parts, the tag policy.
    pub fn run_settings(&self) -> &RunSettings {
        &self.settings
    }

    /// Opens the first card, which is what a fresh start and a freshly loaded
    /// preset both want.
    fn expand_first(&mut self) {
        self.expanded = self.stack.cards().first().map(|c| c.id);
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.preview.plan()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Whether the pipeline would do nothing at all.
    ///
    /// Its own method because M4 makes an empty pipeline reachable — until now
    /// there was always exactly one operation.
    fn pipeline_is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Why the Rename button is disabled, if it is.
    ///
    /// Exposed because a disabled button's hover text does not reach the
    /// accessibility tree, so a headless test cannot read it any other way.
    pub fn blocked_reason(&self) -> Option<String> {
        status_bar::blocked_reason_for(
            self.preview.plan(),
            self.pipeline_is_empty(),
            self.session.guarded.as_deref(),
            self.preview.failure(),
        )
    }

    /// The pipeline the card stack describes.
    fn pipeline(&self) -> Pipeline {
        let inherited = self.filter.is_active().then_some(&self.filter);
        self.stack
            .to_pipeline(inherited, &self.settings, &self.answers)
    }

    pub fn settings(&self) -> &RunSettings {
        &self.settings
    }

    /// Run-wide settings, for tests and for the setup panels.
    pub fn settings_mut(&mut self) -> &mut RunSettings {
        self.needs_preview = true;
        &mut self.settings
    }

    /// The answers the `<Ask>` modal would collect.
    pub fn set_answers(&mut self, answers: Answers) {
        self.answers = answers;
        self.needs_preview = true;
    }

    /// Whether the pipeline still needs something from the user before it can
    /// run — every `<Ask>` slot answered, and the clipboard read if wanted.
    fn unanswered(&self) -> Option<AskForm> {
        self.pending_ask.clone()
    }

    fn unanswered_in(pipeline: &Pipeline, answers: &Answers) -> Option<AskForm> {
        let wants_clipboard = pipeline
            .needs()
            .contains(ren_core::template::TagNeeds::CLIPBOARD);
        let missing: Vec<_> = pipeline
            .asks()
            .into_iter()
            .filter(|spec| answers.ask(spec.slot).is_none())
            .collect();
        let clipboard = wants_clipboard && answers.clipboard.is_none();
        let form = AskForm::new(&missing, clipboard);
        (!form.is_empty()).then_some(form)
    }

    fn request_preview(&mut self) {
        self.needs_preview = false;
        self.scoped = self.session.scoped_indices();

        // No selection means the whole listing, and the `Arc` can be shared
        // rather than copied — the common case, and the big one.
        let entries: Arc<Vec<FileEntry>> = if self.scoped.len() == self.session.entries().len() {
            self.session.entries().clone()
        } else {
            Arc::new(
                self.scoped
                    .iter()
                    .map(|&i| self.session.entries()[i].clone())
                    .collect(),
            )
        };
        let pipeline = self.pipeline();
        // Both of these used to happen once per *frame*, in `unanswered()`,
        // which rebuilt the whole pipeline — every operation cloned and every
        // template recompiled — just to ask whether a modal was needed. Doing
        // it here costs nothing extra: the pipeline is being built anyway.
        self.forget_stale_answers(&pipeline);
        self.pending_ask = Self::unanswered_in(&pipeline, &self.answers);
        // The same reasoning, one line further: Visual Assist's text needs a
        // `RunContext`, which is D28's serial pre-pass, so computing it per
        // frame would be that mistake a third time.
        self.recompute_assist_subject(&pipeline, &entries);
        self.preview
            .request(entries, Arc::new(pipeline), self.scoped.clone());
    }

    /// Works out the text the strip shows, once per preview generation.
    ///
    /// The `RunContext` and the row index are the **real** ones — built over
    /// the scoped entries, in their order — because `<Counter>` and `<Rnd>` in
    /// an earlier card are functions of exactly those. A synthetic context is
    /// how the Filename Editor once came to show a permanent, wrong error on
    /// every collapsed card.
    fn recompute_assist_subject(&mut self, pipeline: &ren_core::Pipeline, entries: &[FileEntry]) {
        let Some(state) = &self.visual_assist else {
            return;
        };
        let Some(index) = self.stack.index_of(state.card) else {
            self.visual_assist = None; // The card went away.
            return;
        };
        let (choices, capped) = visual_assist::choices(self.session.entries(), &self.scoped);
        let file = visual_assist::opening_file(&choices, Some(&state.file));
        let row = file
            .as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path));

        let subject = match row {
            Some(row) => {
                let run = pipeline.run_context(entries);
                let cx = ren_core::ops::EvalCx::new(&entries[row], row, entries.len(), &run);
                match pipeline.subject_at(index, &cx) {
                    Ok(answer) => answer.map(|subject| {
                        // The Add half of a *Both* card is measured on the name
                        // the Remove half has already shortened — "Removes
                        // first, then adds", and the order in that label is
                        // load-bearing. `add_subject` is the engine's own
                        // answer, so the GUI never re-derives the shortening.
                        match (&self.stack.get(index).map(|c| &c.op), state.target) {
                            (
                                Some(ren_core::ops::OpKind::AddRemove(op)),
                                crate::editors::assist::AssistTarget::AddPos,
                            ) => op.add_subject(subject.active()).into_owned(),
                            _ => subject.active().to_owned(),
                        }
                    }),
                    // A step above this one failed for this file, so there is
                    // no defined input. The card that owns the error says so
                    // itself; here it is enough that there is nothing to mark.
                    Err(_) => Err(ren_core::NoSubject::NoSuchStep),
                }
            }
            None => Err(ren_core::NoSubject::NoSuchStep),
        };

        let state = self.visual_assist.as_mut().expect("checked above");
        if state.text() != subject.as_deref().ok() {
            self.forget_assist_selection = true;
        }
        if let Some(file) = file {
            state.file = file;
        }
        state.choices = choices;
        state.capped = capped;
        state.subject = subject;
        state.unreplayed_script = pipeline.unreplayed_script_before(index);
    }

    /// Carries out whatever a ⌖, or the strip it opened, asked for.
    fn apply_assist_request(&mut self, request: crate::editors::AssistRequest) {
        use crate::editors::AssistRequest;
        match request {
            AssistRequest::Point { card, target } => self.point_visual_assist(card, target),
            AssistRequest::Show(path) => {
                if let Some(state) = &mut self.visual_assist {
                    state.show(path);
                    self.forget_assist_selection = true;
                    self.needs_preview = true;
                }
            }
            AssistRequest::Close => {
                self.visual_assist = None;
                self.forget_assist_selection = true;
            }
            AssistRequest::Commit(span) => self.commit_visual_assist(span, false),
            AssistRequest::AnchorToEnd(span) => self.commit_visual_assist(span, true),
        }
    }

    /// Opens the strip, moves it to another target, or closes it.
    ///
    /// One strip ever: a second would have nothing to mean, since it fills one
    /// field of one card. Clicking the lit ⌖ again closes, which is the same
    /// idiom the card summary already is.
    fn point_visual_assist(&mut self, card: CardId, target: crate::editors::assist::AssistTarget) {
        match &mut self.visual_assist {
            Some(state) if state.card == card && state.target == target => {
                self.visual_assist = None;
                self.forget_assist_selection = true;
                return;
            }
            Some(state) if state.card == card => {
                state.point_at(target);
                // The prompt changed, so what a span would mean changed too.
                self.forget_assist_selection = true;
                self.needs_preview = true;
                return;
            }
            _ => {}
        }
        let (choices, _) = visual_assist::choices(self.session.entries(), &self.scoped);
        let Some(file) = visual_assist::opening_file(&choices, None) else {
            self.status = Some("There are no files to mark up".to_owned());
            return;
        };
        let seed = self
            .stack
            .index_of(card)
            .and_then(|index| self.stack.get(index))
            .and_then(|card| seed_selection(&card.op, target));
        self.visual_assist = Some(visual_assist::VisualAssist::new(card, target, file));
        self.forget_assist_selection = true;
        self.pending_assist_selection = seed;
        self.needs_preview = true;
    }

    /// **Select**, and its one variant. Writes, then closes.
    ///
    /// Closing is the point of the word: a strip left open keeps moving a
    /// readout under a field that is no longer listening, and a second Select
    /// would silently overwrite the first.
    fn commit_visual_assist(&mut self, span: crate::editors::assist::AssistSpan, anchor: bool) {
        self.forget_assist_selection = true;
        let Some(state) = self.visual_assist.take() else {
            return;
        };
        let Some(index) = self.stack.index_of(state.card) else {
            return;
        };
        let chars = state.text().map_or(0, |text| text.chars().count());
        let Some(card) = self.stack.get_mut(index) else {
            return;
        };
        if !crate::editors::assist::apply_assist(&mut card.op, state.target, &span) {
            self.status = Some("That card is not the one the selection was made for".to_owned());
            return;
        }
        if anchor {
            crate::editors::assist::anchor_to_end(&mut card.op, state.target, chars);
        }
        self.needs_preview = true;
    }

    /// Closes the strip, wherever from.
    ///
    /// The selection lives in egui's store and most callers have no context, so
    /// the forgetting is deferred to the next frame — see
    /// `forget_assist_selection`.
    fn close_visual_assist(&mut self) {
        if self.visual_assist.take().is_some() {
            self.forget_assist_selection = true;
        }
    }

    /// F3: *"Opens the Visual Assist window where available."*
    ///
    /// Cycles the targets the expanded card offers, wrapping to closed — so on
    /// Replace or Move Section it is a toggle, and on an Add & Remove card in
    /// *Both* mode it goes Remove → Add → closed. One rule covers both, and the
    /// lit marker moves so the cycle is visible.
    ///
    /// *"Where available"* is the documented phrase, and where it is not, this
    /// says so: a silent no-op on a documented key is what a user reports as a
    /// bug. It does **not** expand a card — moving the accordion from a key
    /// someone may have hit by accident is worse than a sentence.
    fn cycle_visual_assist(&mut self) {
        use crate::editors::assist::AssistTarget;

        let Some(card) = self.expanded else {
            self.status = Some(
                "Open an operation first — Visual Assist fills in a position from a filename"
                    .to_owned(),
            );
            return;
        };
        let Some(index) = self.stack.index_of(card) else {
            return;
        };
        let Some(op) = self.stack.get(index).map(|c| &c.op) else {
            return;
        };
        let offered = AssistTarget::offered_by(op);
        if offered.is_empty() {
            self.status = Some(format!("Visual Assist is not available for {}", op.name()));
            return;
        }

        let open = self
            .visual_assist
            .as_ref()
            .filter(|state| state.card == card)
            .map(|state| state.target);
        let next = match open {
            None => Some(offered[0]),
            Some(current) => offered
                .iter()
                .position(|t| *t == current)
                .and_then(|at| offered.get(at + 1))
                .copied(),
        };
        match next {
            Some(target) => self.point_visual_assist(card, target),
            // Past the last one: closed.
            None => {
                self.visual_assist = None;
                self.forget_assist_selection = true;
            }
        }
    }

    /// Opens Visual Assist on a card, as its ⌖ would.
    pub fn open_visual_assist(
        &mut self,
        card: CardId,
        target: crate::editors::assist::AssistTarget,
    ) {
        self.point_visual_assist(card, target);
    }

    /// Points the strip's picker at a file, as its dropdown would.
    pub fn visual_assist_show(&mut self, path: &Path) {
        if let Some(state) = &mut self.visual_assist {
            state.show(path.to_path_buf());
            self.forget_assist_selection = true;
            self.needs_preview = true;
        }
    }

    /// Makes a selection, as a drag or Shift+arrow in the field would.
    ///
    /// The mouse path needs galley hit-testing at real coordinates, which the
    /// accessibility tree does not usefully expose — so the keyboard drives it
    /// in CI and this drives everything the keyboard cannot reach.
    pub fn visual_assist_select(&mut self, start: usize, len: usize) {
        self.pending_assist_selection = Some((start, len));
    }

    /// The text the strip is showing, or why there is none.
    pub fn visual_assist_subject(&self) -> Option<&Result<String, ren_core::NoSubject>> {
        self.visual_assist.as_ref().map(|state| &state.subject)
    }

    /// Which field the open strip fills, if one is open.
    pub fn visual_assist_target(&self) -> Option<crate::editors::assist::AssistTarget> {
        self.visual_assist.as_ref().map(|state| state.target)
    }

    /// The card whose editor is open.
    pub fn expanded_card(&self) -> Option<CardId> {
        self.expanded
    }

    /// Drops answers for `<Ask>` slots the pipeline no longer has.
    ///
    /// Otherwise deleting the card that asked and adding another leaves the
    /// answer behind, and the next run proceeds **without asking** — using an
    /// answer given for a question that is no longer on screen.
    fn forget_stale_answers(&mut self, pipeline: &Pipeline) {
        let wanted: std::collections::BTreeSet<u8> =
            pipeline.asks().iter().map(|spec| spec.slot).collect();
        self.answers.asks.retain(|slot, _| wanted.contains(slot));
        if !pipeline
            .needs()
            .contains(ren_core::template::TagNeeds::CLIPBOARD)
        {
            self.answers.clipboard = None;
        }
    }

    /// Entry index → plan item, from the plan's own record of what it covers.
    ///
    /// From `Ready::scoped`, never from `self.scoped`: a plan can land after
    /// the selection changed underneath it, and indexing a whole-listing plan
    /// through a one-row scope showed another file's preview on that row
    /// until the next plan arrived. The plan says which rows it is about.
    ///
    /// If the listing has moved since the plan was requested — a sort, a
    /// relist — the positional record no longer applies, and the plan is
    /// mapped by file instead. That costs a hash per row, so it is only paid
    /// when the cheap check says the rows moved.
    fn rebuild_plan_index(&mut self) {
        let entries = self.session.entries();
        self.plan_index = vec![None; entries.len()];
        let Some(ready) = self.preview.ready() else {
            return;
        };
        let by_position = ready
            .scoped
            .iter()
            .zip(&ready.plan.items)
            .all(|(&entry_index, item)| {
                entries
                    .get(entry_index)
                    .is_some_and(|entry| entry.path == item.source)
            });
        if by_position {
            for (position, &entry_index) in ready.scoped.iter().enumerate() {
                if let Some(slot) = self.plan_index.get_mut(entry_index) {
                    *slot = Some(position);
                }
            }
        } else {
            let by_source: std::collections::HashMap<&Path, usize> = ready
                .plan
                .items
                .iter()
                .enumerate()
                .map(|(position, item)| (item.source.as_path(), position))
                .collect();
            for (slot, entry) in self.plan_index.iter_mut().zip(entries.iter()) {
                *slot = by_source.get(entry.path.as_path()).copied();
            }
        }
    }

    fn run(&mut self) {
        // One dialog at a time. Without this, a held F5 or a stuck
        // `run_when_ready` rebuilds the confirmation every frame — cheap and
        // invisible in the paint, and exactly the never-settling loop D26
        // forbids, with duplicate nodes in the accessibility tree to match.
        if self.confirming.is_some() {
            return;
        }
        // Never against a plan older than the pipeline on screen. Pressing Run
        // while the preview is still in flight — which loading a preset and
        // running it does by construction — would otherwise apply the *previous*
        // pipeline's plan.
        if self.needs_preview || self.preview.is_stale() {
            self.run_when_ready = true;
            return;
        }

        // The engine never blocks on a user, so anything the pipeline wants
        // asked is asked here, before a plan is built from the answers.
        if let Some(form) = self.unanswered() {
            self.asking = Some(form);
            return;
        }
        // One read, so the plan and the generation that produced it cannot
        // disagree about which run this is.
        let Some((generation, plan)) = self
            .preview
            .ready()
            .map(|ready| (ready.generation, ready.plan.clone()))
        else {
            return;
        };

        // P2's gate. Only for a run that really cannot be taken back, so an
        // ordinary rename stays one click — a confirmation on every run is one
        // the user learns to click through, which is the only way it can fail.
        // Simulation is exempt for the reason the engine gives: it performs no
        // syscall, so there is nothing to consent to, and a dialog headed
        // "cannot be undone" would be false there.
        let consented = self.consented_for == Some(generation);
        if !self.simulate && !consented && plan.irreversible() > 0 {
            self.confirming = Some(confirm::Confirm::new(&plan, generation));
            return;
        }
        // One-shot, whatever happens below.
        self.consented_for = None;

        match self
            .history
            .run(&plan, self.platform.as_ref(), self.simulate, consented)
        {
            Ok(report) => {
                // What happened, not what was planned. `apply` stops at the
                // first failure, so those two numbers part company exactly when
                // the user most needs to be told.
                // Renames and metadata changes counted separately, in the
                // status bar's own vocabulary. Saying "Renamed 0 item(s)" after
                // a run that rewrote two files' tags is the same lie
                // `blocked_reason` was fixed for a milestone ago.
                let what =
                    crate::viewmodel::describe_counts(report.renamed.len(), report.modified());
                self.status = Some(if self.simulate {
                    format!("Simulated: {what} — nothing was written")
                } else if report.is_success() {
                    what
                } else {
                    format!(
                        "{what} of {} planned — {} failed. Undo reverts what did happen.",
                        plan.affected(),
                        report.failed.len()
                    )
                });
                self.show_log = true;
                // Outside the `!simulate` branch below: a simulation is still
                // the user saying they meant that pattern.
                self.remember_fields();
                if !self.simulate {
                    // A running counter that advanced over renames which never
                    // happened would leave a gap in the next batch.
                    if report.is_success() {
                        self.advance_running_counter();
                    }
                    // A rename is the commit point. The name the strip's
                    // selection was measured against is gone, and `refresh()`
                    // below invalidates the file it was pointing at — leaving
                    // it open is how a position measured against a dead name
                    // gets written into a card and run a second time. A
                    // simulation renames nothing, so it does not close (this
                    // branch is already inside `if !self.simulate`).
                    self.close_visual_assist();
                    // Before the relist, so the pictures are already under
                    // their new names when the next frame asks for them.
                    // Renaming four hundred photographs must not look like
                    // opening the folder for the first time (D139).
                    self.thumbs.renamed(&report.renamed);
                    // Through the rename report, so a hand-set row order
                    // survives the run that used it — the same trick D139
                    // plays for the pictures one line above.
                    self.session.refresh_after_run(&report.renamed);
                }
            }
            // The journal died with files already moved. The listing on
            // screen is wrong now, and the strip and the pictures are keyed
            // to names that may be gone — everything a completed run does
            // afterwards, this has to do too, minus the counter.
            Err(e @ ren_core::exec::ExecError::Interrupted { .. }) => {
                self.status = Some(e.to_string());
                self.show_log = true;
                self.close_visual_assist();
                self.session.request_refresh();
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// *"the start value is updated after each rename operation"* to *"the
    /// number that would have been next in line if the counter had continued"*.
    ///
    /// Only after a real rename: a simulation that moved the counter on would
    /// make Simulate a destructive button.
    fn advance_running_counter(&mut self) {
        if !self.settings.counter.running {
            return;
        }
        let entries: Vec<FileEntry> = self
            .scoped
            .iter()
            .filter_map(|&i| self.session.entries().get(i).cloned())
            .collect();
        let run = ren_core::RunContext::build(&entries, &self.settings, self.answers.clone());
        self.settings.counter.start = run.next_start(&self.settings.counter);
    }

    fn undo(&mut self) {
        match self.history.undo(self.platform.as_ref()) {
            Ok(()) => {
                // "Undone" on its own is a lie for a mixed batch: the renames
                // came back and the tag writes did not, and the one the user
                // cannot fix is the one that has to be said (D54).
                self.status = Some(match self.history.last_irreversible() {
                    0 => "Undone".to_owned(),
                    1 => "Undone — 1 change could not be taken back".to_owned(),
                    n => format!("Undone — {n} changes could not be taken back"),
                });
                self.show_log = true;
                // The same as a run, for the same reason.
                self.close_visual_assist();
                // The same move a run makes, in reverse; the pictures follow.
                self.thumbs.renamed(self.history.last_restored());
                self.session.request_refresh();
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// F2's one-off rename. Journalled like any other batch, so Undo covers it.
    pub fn rename_one(&mut self, index: usize, new_name: String) {
        self.inline_rename = None;
        let Some(entry) = self.session.entries().get(index).cloned() else {
            return;
        };
        // Enter renames the file and jumps to the next item in the list.
        //
        // Captured **by path**, before the rename, and looked up again after.
        // `refresh()` re-lists and re-sorts, so the row that was next may be
        // anywhere afterwards — and if the new name sorts past it, it is. An
        // index taken now would open the editor on whatever landed there.
        //
        // The next row **on screen**: with the Changed chip on, the next entry
        // may be hidden, and "the next item in the file list" means the list
        // the user is looking at.
        let successor = self
            .shown_rows()
            .iter()
            .skip_while(|&&i| i != index)
            .nth(1)
            .and_then(|&i| self.session.entries().get(i))
            .map(|entry| entry.path.clone());
        if new_name == entry.file_name || new_name.is_empty() {
            return;
        }

        let rules = self.platform.naming_rules(&entry.path);
        if let Err(problem) = rules.validate_component(&new_name) {
            self.status = Some(format!("Cannot rename: {problem}"));
            return;
        }
        let target = entry.parent().join(&new_name);
        // A case-only rename lands on *itself*. On a case-insensitive volume
        // `readme.txt` → `README.txt` resolves to the very file being renamed,
        // so a bare existence check refuses the single most common thing Set
        // Casing does — and the engine and the platform both handle it (the
        // planner's own folded-target detection treats it as one file, and
        // `Platform::rename` is where the two-step dance lives if a filesystem
        // needs one).
        let same_file_new_spelling = rules.fold(&new_name) == rules.fold(&entry.file_name);
        if !same_file_new_spelling && target.symlink_metadata().is_ok() {
            self.status = Some(format!("Cannot rename: {new_name} already exists"));
            return;
        }

        let plan = Plan {
            items: vec![PlanItem {
                index: 0,
                source: entry.path.clone(),
                new_name: new_name.clone(),
                target: target.clone(),
                state: RowState::Changed,
                actions: Vec::new(),
            }],
            ops: vec![PlannedOp::Rename {
                from: entry.path,
                to: target,
                kind: RenameKind::Direct,
            }],
            // A single in-place rename runs no pipeline, so there is nothing to
            // have produced a note.
            notes: Vec::new(),
        };
        // An inline rename of one file: a rename is always reversible.
        match self
            .history
            .run(&plan, self.platform.as_ref(), false, false)
        {
            Ok(report) => {
                self.status = Some(format!("Renamed to {new_name}"));
                self.thumbs.renamed(&report.renamed);
                self.session.request_refresh();
                // Once the rows exist again: the successor is looked up by
                // path in the listing that has not landed yet.
                self.after_listing = successor.map(AfterListing::OpenEditorOn);
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Files dropped from the file manager. winit delivers these identically on
    /// every platform, so the plumbing is testable even though the real
    /// Explorer interaction is not.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.session.accept_dropped(dropped);
            self.needs_preview = true;
        }
    }

    /// The keys the **file list** owns: the arrows, Home, End and `Ctrl+A`.
    ///
    /// > *"Navigate the file structure with these two and the arrow keys."*
    ///
    /// Split out from `handle_hotkeys` rather than added to it, because the
    /// guard has to be **stricter in two ways**.
    ///
    /// **P81's number-box exemption does not reach here.** That exemption is
    /// safe because nothing on a spinner is spelled F5 — but a focused
    /// `DragValue` in text-entry mode *is* the arrows, Home, End and Backspace.
    /// Letting these through would make every position box on every card
    /// unusable the moment it had the keyboard.
    ///
    /// **And egui moves focus with the arrows before any widget runs.**
    /// `Memory::begin_pass` latches a `FocusDirection` out of the raw events,
    /// so `consume_key` inside the frame is already too late to stop it. The
    /// rule that works is therefore *the list takes the arrows only when
    /// nothing holds the keyboard* — which is the state the app starts in and
    /// returns to after every click on a row, because a `Label` is not
    /// focusable.
    fn handle_list_keys(&mut self, ctx: &egui::Context) {
        if ctx.text_edit_focused() || self.modal_is_up() {
            return;
        }

        // Above the focus check: selecting everything is meaningful whatever
        // has the keyboard, and `Ctrl+A` collides with nothing in a list.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A)) {
            self.select_all_shown();
        }

        if ctx.memory(|m| m.focused()).is_some() {
            return;
        }

        // *"(Browser file mode only)"* — Free Select has no working path to
        // change, which is why F6 and F12 are gated the same way.
        let browsing = self.session.settings.mode == crate::viewmodel::SourceMode::Browser;
        if browsing {
            self.walk_the_folder_tree(ctx);
        }

        // The keys first, the row list second: the list is a walk over the
        // whole listing, and on the frames with no key down — almost all of
        // them — there is nothing to walk it for.
        let (up, down, home, end, ctrl, shift) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
                i.modifiers.command,
                i.modifiers.shift,
            )
        });
        if !(up || down || home || end) {
            return;
        }
        let shown = self.shown_rows();
        if shown.is_empty() {
            return;
        }

        // Left and Right are deliberately not taken. In the table they would
        // have to mean "next column", which is nothing; in the grid they would
        // mean ±1, which Down already means. The listing is one-dimensional and
        // it is the run order.
        let landed = if up {
            self.session.selection.arrow(-1, ctrl, shift, &shown)
        } else if down {
            self.session.selection.arrow(1, ctrl, shift, &shown)
        } else if home {
            self.session.selection.jump_to(0, ctrl, shift, &shown)
        } else if end {
            self.session
                .selection
                .jump_to(shown.len() - 1, ctrl, shift, &shown)
        } else {
            return;
        };

        if let Some(entry) = landed {
            self.scroll_to = Some(entry);
            if !ctrl {
                // Plain and Shift both change the scope; Ctrl moves the
                // keyboard and deliberately leaves the run alone.
                self.needs_preview = true;
            }
        }
    }

    /// > *"`Enter` & `Backspace` — Navigate the file structure with these two
    /// > and the arrow keys. (Browser file mode only)"*
    ///
    /// **P77 is not reopened.** That policy refused *double-click* meaning two
    /// things depending on which view is lit; these are keys, they mean the
    /// same thing in the list and the grid, and the pointer stays unoverloaded
    /// — which is the limb of P77 that was load-bearing.
    ///
    /// The two directions are deliberately asymmetric in what they need:
    /// **Backspace works whatever the Folders chip says**, because a parent
    /// folder is not a row; **Enter needs a folder row to act on**, so it needs
    /// the chip. That asymmetry is why the *"always show folder icons"* row is
    /// waived rather than blocking this one.
    fn walk_the_folder_tree(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            // A **folder** row descends into it. A file row does nothing, and
            // silently: Enter navigates the file structure, this app has no
            // capability for opening a file, and a status line on every
            // stray Enter is noise. The `is_dir` filter is load-bearing — the
            // path of a *file* in `settings.dir` makes the next listing fail.
            if let Some(dir) = self
                .session
                .selection
                .lead
                .and_then(|i| self.session.entries().get(i))
                .filter(|entry| entry.is_dir)
                .map(|entry| entry.path.clone())
            {
                self.set_dir(dir);
            }
        }

        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
            && let Some(parent) = self.session.settings.dir.parent().map(Path::to_path_buf)
        {
            // Nothing above a drive root, where `parent()` is `None`.
            let leaving = std::mem::replace(&mut self.session.settings.dir, parent);
            self.session.request_refresh();
            // Land on the folder just left, as Explorer does — when it is
            // listed at all, which is when the Folders chip is on. Once the
            // parent's rows exist, which is not yet.
            self.after_listing = Some(AfterListing::LandOn(leaving));
        }
    }

    /// True while something in front of the list owns the keyboard.
    fn modal_is_up(&self) -> bool {
        self.palette.is_some()
            || self.asking.is_some()
            || self.confirming.is_some()
            || self.settings_page.is_some()
            || self.about_open
    }

    /// The entry indices on screen, in display order.
    ///
    /// The same list the table and the grid build, from the same function, so a
    /// row the filter is hiding is hidden from the keyboard too.
    fn shown_rows(&self) -> Vec<usize> {
        crate::panels::rows::visible_rows(
            self.session.entries(),
            self.preview.plan(),
            &self.plan_index,
            self.session.settings.row_filter,
        )
    }

    /// `Ctrl+A`, which the hotkey pass promised and nothing built.
    ///
    /// **The rows on screen**, not every entry. The row filter's promise is
    /// that a hidden row is gone; selecting one would scope a rename to a file
    /// the user cannot see.
    fn select_all_shown(&mut self) {
        self.session.selection.set(self.shown_rows());
        self.needs_preview = true;
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        // **F3 sits above the guard below, and it is the only key that does.**
        //
        // The field Visual Assist puts on screen *is* a text edit — read-only,
        // so there is nothing to type into it and no keystroke of the user's to
        // steal — and `text_edit_focused()` cannot tell that from a Find box.
        // Left below the guard, the key that opens the strip would stop working
        // the moment the user clicked into what it opened, which is the one
        // place it is most obviously wanted.
        //
        // Nothing else moves. F5 and Ctrl+Z keep the reasoning in the comment
        // below: a run started mid-word, or an undo that reaches past the text
        // the user is editing, are exactly what that guard is for.
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            self.cycle_visual_assist();
        }

        // Not while the user is typing. `Ctrl+Z` in a Find box means "undo that
        // keystroke"; without this it undid the last *rename batch on disk*,
        // and F5 renamed mid-word. Focus on a button or on nothing still counts
        // as the app having the keyboard, so the shortcuts stay live where they
        // are useful.
        //
        // **A number box is not typing.** `text_edit_focused()` asks whether the
        // focused widget has a `TextEditState`, and a focused `DragValue` builds
        // a real `TextEdit` under its own id — so every position and count box
        // on every card answered yes, and clicking *from pos:* disabled all nine
        // shortcuts until the user clicked elsewhere. The reasoning above is
        // about prose the user is composing: a spinner holds a handful of digits
        // it commits on every keystroke, none of these keys types one, and a
        // card you have just finished configuring is the likeliest moment to
        // press F5. See `widgets::number` for why egui cannot tell them apart.
        if ctx.text_edit_focused() && !crate::widgets::number::has_focus(ctx) {
            return;
        }

        // The hotkeys, in order: F2 rename, F4 undo,
        // F5 run, F6 focus the address box, F8 settings, F9 refresh, F12 folder
        // browser. F3 is Visual Assist and is handled above, outside this
        // guard. F1 is P64 and Delete is P65.
        // Ctrl+Z and Ctrl+K are ours.
        let keys = ctx.input(|i| Hotkeys {
            f2: i.key_pressed(egui::Key::F2),
            f4: i.key_pressed(egui::Key::F4),
            f5: i.key_pressed(egui::Key::F5),
            f6: i.key_pressed(egui::Key::F6),
            f8: i.key_pressed(egui::Key::F8),
            f9: i.key_pressed(egui::Key::F9),
            f12: i.key_pressed(egui::Key::F12),
            undo: i.modifiers.command && i.key_pressed(egui::Key::Z),
            palette: i.modifiers.command && i.key_pressed(egui::Key::K),
        });
        let Hotkeys {
            f2,
            f4,
            f5,
            f6,
            f8,
            f9,
            f12,
            undo,
            palette,
        } = keys;

        if palette {
            self.open_palette();
        }
        if f8 {
            self.open_settings();
        }
        // *"(Browser file mode only)"* for both: in Free Select there is no
        // address box to focus and no working path to change.
        let browsing = self.session.settings.mode == crate::viewmodel::SourceMode::Browser;
        if f6 && browsing {
            ctx.memory_mut(|m| m.request_focus(crate::panels::source_bar::address_box()));
        }
        if f12
            && browsing
            && let Some(dir) = self.dialogs.pick_folder(&self.session.settings.dir.clone())
        {
            self.session.settings.dir = dir;
            self.session.request_refresh();
        }

        if f2
            && let Some(index) = self
                .session
                .selection
                .lead
                .or_else(|| self.session.selection.iter().next())
            && let Some(entry) = self.session.entries().get(index)
        {
            self.inline_rename = Some(crate::panels::rows::InlineRename::opening(
                index,
                &entry.file_name,
            ));
        }
        if f5 {
            self.run();
        }
        if f9 {
            self.forget_and_relist();
        }
        // Both are shortcuts for the Undo button: Ctrl+Z is the modern
        // spelling, F4 the one long-time renaming tools trained people on.
        if undo || f4 {
            self.undo();
        }
    }

    /// Carries out what the file list's right-click menu asked for.
    ///
    /// Here rather than in the table because every one of these needs something
    /// the table does not have: the platform, the session, or the clipboard.
    pub fn row_action(&mut self, action: crate::panels::rows::RowAction) {
        use crate::panels::rows::RowAction;

        match action {
            RowAction::Rename(index) => {
                if let Some(entry) = self.session.entries().get(index) {
                    self.inline_rename = Some(crate::panels::rows::InlineRename::opening(
                        index,
                        &entry.file_name,
                    ));
                }
            }
            RowAction::Reveal(index) => {
                if let Some(entry) = self.session.entries().get(index) {
                    // Best effort: there may be no file manager, and failing to
                    // open one is not something to interrupt a rename over.
                    let _ = self.platform.reveal_in_file_manager(&entry.path);
                }
            }
            RowAction::AddToFreeSelect(rows) => {
                let paths: Vec<_> = self
                    .rows_or_all(&rows)
                    .filter_map(|i| self.session.entries().get(i).map(|e| e.path.clone()))
                    .collect();
                self.session.accept_dropped(paths);
                self.needs_preview = true;
            }
            RowAction::Copy { what, rows } => {
                // The same rule `run()` keeps (P35): a plan older than the
                // pipeline on screen is not what the user is looking at, and
                // copying its names would paste a previous keystroke's
                // preview somewhere it will be believed.
                if what.needs_plan() && (self.needs_preview || self.preview.is_stale()) {
                    self.status =
                        Some("The preview is still updating — copy again in a moment".to_owned());
                    return;
                }
                let text = self.rows_as_text(what, &rows);
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    let _ = clipboard.set_text(text);
                }
            }
        }
    }

    /// The rows a menu item applies to.
    ///
    /// *"However, if no items are selected, all items will be renamed"* — the
    /// same rule `scoped_indices` follows, so *Copy* covers exactly what
    /// *Rename* would.
    fn rows_or_all(&self, rows: &[usize]) -> impl Iterator<Item = usize> + use<'_> {
        let all: Vec<usize> = if rows.is_empty() {
            (0..self.session.entries().len()).collect()
        } else {
            rows.to_vec()
        };
        all.into_iter()
    }

    /// *"Copy to Clipboard ▸ All Previews"*, and the three neighbours worth
    /// having: one row per line, in the order the table shows them.
    /// Public because the clipboard is not: `arboard` needs a display server,
    /// so a headless test drives the text and leaves `set_text` — the one line
    /// with nothing in it to get wrong — to a human.
    pub fn rows_as_text(&self, what: crate::panels::rows::CopyWhat, rows: &[usize]) -> String {
        use crate::panels::rows::CopyWhat;

        let mut out = String::new();
        for index in self.rows_or_all(rows) {
            let Some(entry) = self.session.entries().get(index) else {
                continue;
            };
            // A row outside the run has no new name; it keeps the one it has,
            // which is what the table shows for it too.
            let new_name = crate::panels::rows::item_of(
                self.session.entries(),
                self.preview.plan(),
                &self.plan_index,
                index,
            )
            .map_or(entry.file_name.as_str(), |item| item.new_name.as_str());

            match what {
                CopyWhat::Names => out.push_str(&entry.file_name),
                CopyWhat::NewNames => out.push_str(new_name),
                CopyWhat::Paths => out.push_str(&entry.path.to_string_lossy()),
                CopyWhat::Both => {
                    out.push_str(&entry.file_name);
                    out.push('\t');
                    out.push_str(new_name);
                }
            }
            out.push('\n');
        }
        out
    }

    /// What Windows will not hand over, said once and dismissibly.
    ///
    /// A banner rather than the status line, which the next preset load
    /// overwrites — and this has to survive the preset that the same click
    /// loaded a moment ago.
    ///
    /// It offers the repair rather than only the explanation: *List the whole
    /// folder* is the thing the user would otherwise have to work out for
    /// themselves, and it is the one path with no limit at all.
    fn selection_banner(&mut self, ui: &mut egui::Ui) {
        let Some(caveat) = self.selection_caveat.clone() else {
            return;
        };
        egui::Frame::new()
            .fill(ui.visuals().warn_fg_color.gamma_multiply(0.15))
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    // The number we were given and the ceiling, and no verb
                    // about what happened in between: Windows hides an entry
                    // rather than shortening it, so "some files were dropped"
                    // would be a claim nothing here can support (P83).
                    ui.label(format!(
                        "⚠ Windows passes a right-click selection as a command line and stops \
                         at about {limit} characters; this one was {chars}. If files are \
                         missing from this list, that is the reason.",
                        limit = ren_platform::SHELL_COMMAND_LINE_LIMIT,
                        chars = caveat.chars,
                    ));
                    if let Some(dir) = &caveat.dir
                        && ui
                            .button("List the whole folder")
                            .on_hover_text(format!("{}", dir.display()))
                            .clicked()
                    {
                        self.session.settings.mode = crate::viewmodel::SourceMode::Browser;
                        self.set_dir(dir.clone());
                        self.selection_caveat = None;
                    }
                    if ui.button("Dismiss").clicked() {
                        self.selection_caveat = None;
                    }
                });
            });
    }

    fn recovery_banner(&mut self, ui: &mut egui::Ui) {
        if self.history.unfinished.is_empty() {
            return;
        }
        let count = self.history.unfinished.len();
        egui::Frame::new()
            .fill(ui.visuals().warn_fg_color.gamma_multiply(0.15))
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // "rename" was the wrong noun for a batch that wrote
                    // tags, and a count on its own says nothing about the one
                    // thing that matters: which file might be damaged.
                    ui.label(format!(
                        "⚠ {count} batch(es) did not finish — the app or the machine \
                         stopped part-way."
                    ));
                    if ui.button("Roll back").clicked() {
                        if let Err(e) = self.history.recover(self.platform.as_ref()) {
                            self.status = Some(e.to_string());
                        }
                        self.session.request_refresh();
                    }
                    if ui.button("Leave as is").clicked() {
                        self.history.dismiss_recovery();
                    }
                });
                // The files that were being written when it stopped. A tag
                // write is rewritten in place, so one of these may be
                // half-written — and until now it was the only thing the
                // banner did not say, while the path was in the journal all
                // along.
                for item in &self.history.unfinished {
                    for flight in &item.in_flight {
                        let name = flight
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        ui.label(
                            egui::RichText::new(if flight.rewrote_contents {
                                format!(
                                    "{name} — {} was interrupted, so it may be half-written",
                                    flight.op
                                )
                            } else {
                                format!(
                                    "{name} — {} was interrupted, so it may be under either name",
                                    flight.op
                                )
                            })
                            .small(),
                        );
                    }
                }
            });
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        use crate::viewmodel::LogLine;
        egui::ScrollArea::vertical()
            .max_height(140.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for line in &self.history.log {
                    match line {
                        LogLine::Renamed { from, to } => ui.label(format!("{from}  →  {to}")),
                        LogLine::Restored { from, to } => ui.label(format!("{from}  ↺  {to}")),
                        LogLine::Failed { path, error } => ui
                            .colored_label(ui.visuals().error_fg_color, format!("{path}: {error}")),
                        LogLine::Skipped { path, reason } => ui
                            .colored_label(ui.visuals().warn_fg_color, format!("{path}: {reason}")),
                        LogLine::Irreversible { path, what } => ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("{path}: {what}, so it is still applied"),
                        ),
                        // Default colour, not `warn`: this one succeeded.
                        // Warn is for what was skipped or could not be undone.
                        LogLine::Modified { path, what } => ui.label(format!("{path}  ∆  {what}")),
                        LogLine::CreatedDir { path } => {
                            ui.label(egui::RichText::new(format!("created folder {path}")).weak())
                        }
                        // Replacing is warned about; creating is not. The
                        // confirmation already happened — this is the record of
                        // which of the two it turned out to be.
                        LogLine::Wrote {
                            path,
                            replaced: true,
                        } => ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("replaced {path}, which cannot be undone"),
                        ),
                        LogLine::Wrote { path, .. } => ui.label(format!("wrote {path}")),
                        LogLine::Note(text) => ui.label(egui::RichText::new(text).strong()),
                    };
                }
            });
    }
}

/// A selection that arrived from the Explorer menu close to Windows' ceiling.
///
/// **It never claims anything was dropped, because we cannot know.** Windows
/// hides a menu entry whose command line would exceed the cap rather than
/// shortening it, so a truncated selection does not reach this program at all —
/// it reaches a user who right-clicked and found the entry missing. What this
/// records is the other case: a selection that got through, near enough to the
/// limit that the user has a reason to wonder, and a mechanism nothing else in
/// the app would ever explain (**P83**).
#[derive(Debug, Clone)]
pub struct SelectionCaveat {
    /// What Windows measured, not what we parsed out of it.
    pub chars: usize,
    /// The folder the selection came from, for the repair.
    pub dir: Option<PathBuf>,
}

impl SelectionCaveat {
    /// Whether this launch deserves the banner, and what it should say.
    ///
    /// Three conditions, and each of them is a way of not saying something
    /// untrue:
    ///
    /// * **`from_shell`** — a drag of two hundred files onto the window went
    ///   through no command line at all, and telling that user about a
    ///   2000-character cap would be inventing a limit that did not apply.
    /// * **not `start_in`** — *Start from this folder* passes one path however
    ///   many are selected, so it has no limit to explain.
    /// * **near the ceiling** — four fifths of it. Not a guess at where
    ///   truncation began (there is no such point to guess at) but the width of
    ///   the band in which "this is how many files a right-click can carry"
    ///   stops being trivia and becomes the answer to what the user is looking
    ///   at.
    pub fn for_launch(launch: &crate::launch::Launch) -> Option<Self> {
        let chars = launch.command_line_chars?;
        if !launch.from_shell
            || launch.start_in
            || chars * 5 < ren_platform::SHELL_COMMAND_LINE_LIMIT * 4
        {
            return None;
        }
        Some(Self {
            chars,
            dir: launch.paths.first().and_then(|path| {
                if path.is_dir() {
                    Some(path.clone())
                } else {
                    path.parent().map(Path::to_path_buf)
                }
            }),
        })
    }
}

/// The drop-down fields an operation carries, as `(field id, text)`.
///
/// The four fields that hold a *pattern*: Free Format's box, Find and
/// *…and replace with*, and Add's Insert. The CSV file is deliberately not one
/// of them — it is a path with a picker beside it, and a recent-files list is
/// a different feature.
///
/// The wildcard is deliberate and the ids match the editors': a new operation
/// with a pattern box opts in by adding an arm, and one without needs no
/// thought at all.
fn history_entries(op: &OpKind) -> Vec<(&'static str, String)> {
    match op {
        OpKind::FreeFormat(inner) => {
            vec![("free_format_pattern", inner.pattern.as_str().to_owned())]
        }
        OpKind::Replace(inner) => vec![
            ("replace_find", inner.find.clone()),
            ("replace_with", inner.replace.as_str().to_owned()),
        ],
        OpKind::AddRemove(inner) => vec![("add_insert", inner.insert.as_str().to_owned())],
        _ => Vec::new(),
    }
}

/// The presets on disk, as the Explorer menu needs them.
///
/// The caption is the display name and the command carries the **path**,
/// because names are not unique — `PresetStore::load_named` returns `Ambiguous`
/// rather than guessing, and a menu keyed on names would ship that ambiguity to
/// every right-click.
///
/// A free function so the three callers that need it — the drawer's funnel, the
/// startup repair and the Settings page — cannot each convert it slightly
/// differently.
pub fn menu_presets(entries: &[ren_core::PresetEntry]) -> Vec<ren_platform::shell::MenuPreset<'_>> {
    entries
        .iter()
        .map(|entry| ren_platform::shell::MenuPreset {
            name: &entry.name,
            file: &entry.path,
        })
        .collect()
}

impl eframe::App for RenameItApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // One list of fields, in `persisted`, so a field added later cannot be
        // saved and then forgotten by the reset — or the other way round.
        eframe::set_value(storage, STORAGE_KEY, &self.persisted());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

impl RenameItApp {
    /// The whole window, independent of eframe so tests can drive it.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.unstyled {
            self.unstyled = false;
            // Here rather than in `new`, because `headless` has no `Context` to
            // style and a test should render what the window renders.
            crate::theme::install_fonts(&ctx);
            crate::theme::install(&ctx);
        }
        if self.instant {
            self.instant = false;
            ctx.all_styles_mut(|style| style.animation_time = 0.0);
        }
        self.handle_drops(&ctx);
        self.handle_hotkeys(&ctx);
        self.handle_list_keys(&ctx);

        // Last frame's drawer action, applied now that its borrow is gone.
        if std::mem::take(&mut self.needs_menu_rewrite) {
            self.rewrite_menu();
        }

        // A listing that landed since the last frame, then anything drawn
        // last frame that asked for one. The order matters: a request made
        // after the poll is the newest wish, and a listing installed after
        // the request would be answered by a stale generation.
        self.poll_listing();
        self.drain_listing_request();

        if self.preview.poll() {
            self.rebuild_plan_index();
        }
        // Before anything draws: what is on screen now must not be evictable,
        // and a tile that arrived since the last frame must reach the cache
        // before the visible set is worked out again — or it counts as missing
        // and the same picture is decoded twice.
        self.thumbs.begin_frame();
        self.thumbs.poll();

        // Visual Assist's selection lives in egui's store, so the two places
        // that decide it must change — a subject that moved underneath it, and
        // the test seam — leave a note for here, where there is a context.
        if std::mem::take(&mut self.forget_assist_selection) {
            visual_assist::forget_selection(&ctx);
        }
        if let Some((start, len)) = self.pending_assist_selection.take() {
            visual_assist::set_selection(&ctx, start, len);
        }
        // A strip whose card has collapsed has no way out: the ⌖ that would
        // close it is not on screen. `expanded` is the authority.
        if let Some(state) = &self.visual_assist
            && self.expanded != Some(state.card)
        {
            self.visual_assist = None;
            visual_assist::forget_selection(&ctx);
        }
        // A run that was waiting for the preview to catch up. Checked every
        // frame rather than only when the cache changed: the plan may well have
        // landed before this frame started, and then there is nothing to notice.
        if self.run_when_ready && !self.needs_preview && !self.preview.is_stale() {
            self.run_when_ready = false;
            self.run();
        }

        if let Some(page) = &mut self.settings_page {
            let mut page = *page;
            // Read here rather than inside the page, so the borrow of
            // `self.presets` ends before `defaults` takes `&mut self.session`.
            let (preset_entries, _) = self.presets.list();
            let defaults = settings::Defaults {
                batch_replace: &mut self.batch_replace,
                music_styles: &mut self.music_styles,
                casing: &mut self.casing,
                columns: &mut self.columns,
                table_style: &mut self.table_style,
                startup: &mut self.startup,
                session: &mut self.session.settings,
                platform: self.platform.as_ref(),
                journal_dir: &self.history.journal_dir,
                preset_dir: self.presets.dir(),
                presets: &preset_entries,
            };
            let out = settings::ui(&ctx, &mut page, defaults, self.theme);
            if let Some(theme) = out.theme {
                self.theme = theme;
                ctx.set_theme(egui::ThemePreference::from(theme));
            }
            if let Some(zoom) = out.zoom {
                // Clamped here because `set_zoom_factor` does not clamp at all
                // — egui's own 0.2-5.0 bounds live in `gui_zoom::zoom_in`, and
                // a zero would trip a debug assert in the glyph cache and draw
                // garbage in release.
                ctx.set_zoom_factor(zoom.clamp(settings::MIN_ZOOM, settings::MAX_ZOOM));
            }
            if out.reset {
                // Asked, not done: the confirmation belongs **on top of** this
                // modal, so it is drawn after the settings block below.
                self.confirming_reset = true;
            }
            if out.relist {
                // The File System page decides what is *in* the list, so its
                // switches take effect on a fresh walk rather than at the next
                // thing that happens to trigger one.
                self.session.request_refresh();
            }
            if out.changed {
                self.needs_preview = true;
            }
            if out.close {
                // Blank rows are dropped when the window closes, not while the
                // user is typing — see `string_list::tidy`.
                tidy(&mut self.music_styles);
                tidy(&mut self.casing.exceptions.words);
                tidy(&mut self.casing.title_case.lowercase_exceptions);
            }
            self.settings_page = (!out.close).then_some(page);
        }

        // After the settings block above, so it is the top modal: Escape and a
        // click on the backdrop then close *this* rather than the window behind
        // it, and the user lands back on the page showing the restored lists.
        if self.confirming_reset {
            match confirm::reset_ui(&ctx) {
                confirm::Outcome::Confirmed => {
                    self.confirming_reset = false;
                    self.reset_settings(&ctx);
                }
                confirm::Outcome::Cancelled => self.confirming_reset = false,
                confirm::Outcome::Open => {}
            }
        }

        if self.about_open && about::ui(&ctx, &self.history.batches) {
            self.about_open = false;
        }

        if let Some(state) = &mut self.palette {
            let mut state = std::mem::take(state);
            match palette::ui(&ctx, &mut state) {
                palette::Outcome::Open => self.palette = Some(state),
                palette::Outcome::Add(op) => {
                    self.palette = None;
                    self.add_operation(op);
                }
                palette::Outcome::Cancelled => self.palette = None,
            }
        }

        if let Some(form) = &mut self.asking {
            let mut form = std::mem::take(form);
            match ask::ui(&ctx, &mut form) {
                AskOutcome::Open => self.asking = Some(form),
                AskOutcome::Confirmed => {
                    self.asking = None;
                    let answers = form.answers();
                    // Merge, so a second run does not lose the first's answers.
                    self.answers.asks.extend(answers.asks);
                    if answers.clipboard.is_some() {
                        self.answers.clipboard = answers.clipboard;
                    }
                    self.needs_preview = true;
                    self.run_when_ready = true;
                }
                AskOutcome::Cancelled => {
                    self.asking = None;
                    // Not "Rename cancelled": for a tag-write run that is the
                    // same wrong noun the post-run status used to carry.
                    self.status = Some("Cancelled — nothing was changed".to_owned());
                }
            }
        }

        // Last, so it is the topmost thing if anything else is somehow open.
        if let Some(state) = self.confirming.take() {
            match confirm::ui(&ctx, &state) {
                confirm::Outcome::Open => self.confirming = Some(state),
                confirm::Outcome::Confirmed => {
                    self.consented_for = Some(state.generation);
                    // Straight back into `run()` rather than through
                    // `run_when_ready`: the deferred-run block sits *above*
                    // this one, so routing through it would cost a whole frame
                    // in which the user has consented and the app looks idle.
                    // Nothing about the plan changed, so nothing needs
                    // recomputing — unlike the `<Ask>` path, whose answers do.
                    self.run();
                }
                confirm::Outcome::Cancelled => {
                    self.consented_for = None;
                    self.run_when_ready = false;
                    self.status = Some("Cancelled — nothing was changed".to_owned());
                }
            }
        }

        egui::Panel::top(egui::Id::new("source")).show(ui, |ui| {
            self.recovery_banner(ui);
            self.selection_banner(ui);
            let out = source_bar::ui(
                ui,
                &mut self.session,
                &mut self.filter,
                self.dialogs.as_ref(),
                self.listing.is_listing(),
            );
            if out.relist {
                self.session.request_refresh();
            }
            if out.refilter {
                self.needs_preview = true;
            }
            if out.open_settings {
                self.open_settings();
            }
            if out.open_about {
                self.about_open = true;
            }
        });

        egui::Panel::bottom(egui::Id::new("status")).show(ui, |ui| {
            let pipeline_empty = self.pipeline_is_empty();
            let out = status_bar::ui(
                ui,
                &self.session,
                status_bar::PreviewState {
                    plan: self.preview.plan(),
                    stale: self.preview.is_stale(),
                    failure: self.preview.failure(),
                },
                &self.history,
                &mut self.simulate,
                pipeline_empty,
            );
            if let Some(filter) = out.row_filter {
                self.session.settings.row_filter = filter;
            }
            if out.toggle_log {
                self.show_log = !self.show_log;
            }
            if out.run {
                self.run();
            }
            if out.undo {
                self.undo();
            }

            if let Some(status) = &self.status {
                ui.label(egui::RichText::new(status).weak());
            }
            if self.show_log && !self.history.log.is_empty() {
                ui.separator();
                self.log_panel(ui);
            }
        });

        egui::Panel::left(egui::Id::new("operation"))
            .resizable(true)
            .default_size(320.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Pipeline").heading());
                    // Settings and About used to be here too. They are the
                    // window's commands rather than the pipeline's, so they
                    // moved to the window's top-right; what is left in this
                    // header belongs to the card stack underneath it.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Presets ▾")
                            .on_hover_text("Save this pipeline, or load one you saved")
                            .clicked()
                        {
                            if self.drawer.is_some() {
                                self.drawer = None;
                            } else {
                                self.open_presets();
                            }
                        }
                    });
                });
                ui.add_space(4.0);

                let mut expanded = self.expanded;
                {
                    // Disjoint field borrows: the stack is taken mutably while
                    // the session and dialogs are only read.
                    let cx = crate::editors::EditorCx {
                        entries: self.session.entries(),
                        scoped: &self.scoped,
                        dialogs: self.dialogs.as_ref(),
                        platform: self.platform.as_ref(),
                        scope: ren_core::Scope::Name,
                        music_styles: &self.music_styles,
                        field_history: &self.field_history,
                        requests: &self.requests,
                        // Overwritten per card by `for_card`; this is only what
                        // a card that never asks would see.
                        card: crate::viewmodel::CardId::none(),
                        assist: None,
                    };
                    if operation::ui(
                        ui,
                        &mut self.stack,
                        &mut expanded,
                        &cx,
                        self.visual_assist.as_ref(),
                    ) {
                        self.needs_preview = true;
                    }
                }
                self.expanded = expanded;
                // A card asked for a window. Opening it here rather than
                // inside the editor means the modal is drawn on the next
                // frame, from the top, instead of nested inside the panel that
                // spawned it.
                if self.requests.edit_music_styles.replace(false) {
                    self.settings_page = Some(settings::Page::MusicStyles);
                }
                // Quick Setup configures Setup Parts as well as its own card,
                // which is what makes it "quick" — the two halves are useless
                // apart.
                if let Some(parts) = self.requests.set_parts.borrow_mut().take() {
                    self.settings.parts = ren_core::PartsSpec::new(parts);
                    self.needs_preview = true;
                }
                // *"You can also add the current Replace function settings to
                // the list."* The defaults list, so nothing already built
                // changes under the user (D35).
                // Visual Assist. Drained here, after the stack is drawn, so a
                // click never mutates a card mid-frame — and so a Select can
                // safely close the strip that raised it.
                let assist_request = self.requests.assist.borrow_mut().take();
                if let Some(request) = assist_request {
                    self.apply_assist_request(request);
                }
                if let Some(rule) = self.requests.add_batch_rule.borrow_mut().take() {
                    self.batch_replace.push(rule);
                }

                ui.add_space(4.0);
                if ui
                    .button("+ Add operation")
                    .on_hover_text("Ctrl+K")
                    .clicked()
                {
                    self.open_palette();
                }

                ui.add_space(8.0);
                ui.separator();
                // Borrowed, not cloned: `settings` and `session` are disjoint
                // fields, and the popup this feeds is closed on almost every
                // frame.
                let sample = self
                    .session
                    .selection
                    .iter()
                    .next()
                    .and_then(|i| self.session.entries().get(i))
                    .or_else(|| self.session.entries().first());
                if run_settings::ui(ui, &mut self.settings, sample) {
                    self.needs_preview = true;
                }
                if self.pending_ask.is_some() {
                    ui.label(
                        egui::RichText::new(
                            "The preview leaves <Ask> empty — you are asked for it when you \
                             press Rename.",
                        )
                        .weak()
                        .small(),
                    );
                }
            });

        if self.drawer.is_some() {
            let (entries, problems) = self.presets.list();
            let mut state = self.drawer.take().unwrap_or_default();
            let mut out = presets::DrawerOutput::default();
            egui::Panel::right(egui::Id::new("presets"))
                .resizable(true)
                .default_size(300.0)
                .show(ui, |ui| {
                    out = presets::ui(ui, &mut state, &entries, &problems, self.dialogs.as_ref());
                });
            self.drawer = (!out.close).then_some(state);
            self.apply_drawer(out);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if self.session.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(egui::RichText::new("No files listed").weak());
                });
                return;
            }

            // The view needs the listing and the selection at once, and both
            // live in `session`. Lending the selection out for the frame is
            // cheaper than cloning it, and it goes straight back.
            let mut selection = std::mem::take(&mut self.session.selection);
            // Drained here: it is a one-shot request from the last keypress,
            // and leaving it set would re-scroll on every frame after.
            let scroll_to = self.scroll_to.take();
            let entries = self.session.entries().clone();
            let look = crate::panels::tile::Look {
                size: self.session.settings.thumb_size,
                border: self.session.settings.thumb_border,
            };

            let mut sort_request = None;
            let mut reorder_request = false;
            let mut move_request = None;
            let row_action;
            let rename_confirmed;

            match self.session.settings.view {
                ViewMode::List => {
                    let mut table = FileTable::new(
                        &entries,
                        self.preview.plan(),
                        &self.plan_index,
                        &mut selection,
                        self.session.settings.row_filter,
                        self.session.settings.sort,
                        &self.columns,
                        self.table_style,
                        &mut self.inline_rename,
                        &mut self.thumbs,
                        look,
                    );
                    table.scroll_to = scroll_to;
                    table.show(ui);
                    sort_request = table.sort_request;
                    reorder_request = table.reorder_request;
                    move_request = table.move_request.take();
                    row_action = table.row_action.take();
                    rename_confirmed = table.rename_confirmed.take();
                }
                // No sort headers and no reorder command: those belong to
                // column headings, and a grid has none. The order is still the
                // listing's, so the run order is the same in both views.
                ViewMode::Grid => {
                    let mut grid = crate::panels::grid::Grid::new(
                        &entries,
                        self.preview.plan(),
                        &self.plan_index,
                        &mut selection,
                        self.session.settings.row_filter,
                        &mut self.inline_rename,
                        &mut self.thumbs,
                        look,
                    );
                    grid.scroll_to = scroll_to;
                    grid.show(ui);
                    row_action = grid.row_action.take();
                    rename_confirmed = grid.rename_confirmed.take();
                }
            }
            // **`rows`, not the whole `Selection`.** Moving the lead with Ctrl
            // and an arrow key is not a change to what the run covers, and
            // re-planning on every keypress would make the list stutter.
            let selection_changed = selection.rows != self.session.selection.rows;
            self.session.selection = selection;
            if selection_changed {
                self.needs_preview = true;
            }

            if let Some(column) = sort_request {
                self.session.set_sort(column);
                self.needs_preview = true;
            }
            // A command, not a mode (D119): it needs the plan whose names it
            // orders by, and does nothing without one.
            // A drag that landed. The order is the run order, so a moved row
            // is numbered where it was dropped.
            if let Some((rows, before)) = move_request
                && self.session.move_rows(&rows, before)
            {
                self.needs_preview = true;
            }
            if reorder_request && let Some(plan) = self.preview.plan() {
                self.session.reorder_by_new_names(plan);
                self.needs_preview = true;
            }
            if let Some(action) = row_action {
                self.row_action(action);
            }
            if let Some((index, name)) = rename_confirmed {
                self.rename_one(index, name);
            }
        });

        if self.needs_preview {
            self.request_preview();
        }
    }
}

/// The selection the strip opens showing.
///
/// What the card already says, so opening Visual Assist starts from the current
/// answer rather than from a bare caret — and so the readout means something on
/// the first frame. `None` for Replace, whose box holds text rather than a
/// position, and where a seeded selection would be a guess.
fn seed_selection(
    op: &ren_core::ops::OpKind,
    target: crate::editors::assist::AssistTarget,
) -> Option<(usize, usize)> {
    use crate::editors::assist::AssistTarget;
    use ren_core::ops::OpKind;
    match (op, target) {
        (OpKind::AddRemove(op), AssistTarget::AddPos) if !op.add_backwards => Some((op.add_pos, 0)),
        (OpKind::AddRemove(op), AssistTarget::RemoveSection) if !op.remove_backwards => {
            Some((op.remove_pos, op.delete))
        }
        (OpKind::MoveSection(op), AssistTarget::MoveCut) if !op.from_backwards => {
            Some((op.from_pos, op.cut))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ren_core::ops::{CaseMode, Casing};

    fn listing(names: &[&str]) -> (tempfile::TempDir, RenameItApp) {
        let dir = tempfile::TempDir::new().unwrap();
        for name in names {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let journal = tempfile::TempDir::new().unwrap();
        let mut app = RenameItApp::headless(dir.path().to_path_buf(), journal.path().to_path_buf());
        *app.operation_mut() = OpKind::Replace(ren_core::ops::Replace::new("_", " "));
        app.settle();
        (dir, app)
    }

    /// A plan says which rows it is about; the app used to remember that
    /// separately and could disagree with it. Here a whole-listing plan lands
    /// *after* the selection narrowed to one row — the race — and every row
    /// still shows its own file's preview, because the index is rebuilt from
    /// the plan's own scope.
    #[test]
    fn a_plan_is_addressed_through_the_scope_it_was_built_over() {
        let (_dir, mut app) = listing(&["a_1.txt", "b_2.txt", "c_3.txt"]);
        let whole = app.plan().cloned().unwrap();
        assert_eq!(whole.items.len(), 3);

        // The selection narrows, the request goes out for one row…
        app.session.selection.set([1]);
        app.request_preview();
        assert_eq!(app.scoped, [1]);
        // …and the *previous* whole-listing answer arrives.
        app.preview.install_ready(whole, vec![0, 1, 2]);
        app.rebuild_plan_index();

        for (row, expected) in [(0, "a 1.txt"), (1, "b 2.txt"), (2, "c 3.txt")] {
            assert_eq!(
                app.item_for_row(row).map(|item| item.new_name.as_str()),
                Some(expected),
                "row {row}"
            );
        }
    }

    /// The folder changes while the previous folder's walk is still out. The
    /// second request must not be lost when the first walk lands — it was,
    /// once: installing a listing cleared the session's "relist wanted" flag,
    /// which the second change had set and the frame had not yet drained.
    #[test]
    fn a_folder_changed_while_the_last_walk_was_out_still_gets_listed() {
        let first = tempfile::TempDir::new().unwrap();
        let second = tempfile::TempDir::new().unwrap();
        std::fs::write(first.path().join("first.txt"), b"x").unwrap();
        std::fs::write(second.path().join("second.txt"), b"x").unwrap();
        let journal = tempfile::TempDir::new().unwrap();

        // `headless` has already asked for `first`; ask for `second` before
        // that answer is taken, then let the first answer land by itself.
        let mut app =
            RenameItApp::headless(first.path().to_path_buf(), journal.path().to_path_buf());
        app.set_dir(second.path().to_path_buf());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.poll_listing() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(app.session.relist_wanted, "the second folder is still owed");

        app.settle();
        let names: Vec<&str> = app
            .session
            .entries()
            .iter()
            .map(|e| e.file_name.as_str())
            .collect();
        assert_eq!(names, ["second.txt"]);
        assert!(!app.listing.is_listing());
    }

    /// Between a sort and the next plan the positional index points at rows
    /// that have moved. A row whose file is not the plan item's file shows no
    /// preview rather than the previous occupant's; and when a plan built over
    /// the old order lands, it is mapped back by file.
    #[test]
    fn a_moved_listing_never_shows_another_rows_preview() {
        let (_dir, mut app) = listing(&["a_1.txt", "b_2.txt"]);
        let old_order = app.plan().cloned().unwrap();
        assert_eq!(app.item_for_row(0).unwrap().new_name, "a 1.txt");

        // Reverse the listing under the plan, and do not let a new one land.
        app.session.set_sort(crate::viewmodel::SortColumn::Name);
        assert_eq!(app.session.entries()[0].file_name, "b_2.txt");
        assert_eq!(
            app.item_for_row(0),
            None,
            "row 0 now holds b_2.txt, and the index still says a_1's item"
        );

        // The plan for the old order arrives anyway (it was in flight): the
        // positional record no longer applies, so it is mapped by file.
        app.preview.install_ready(old_order, vec![0, 1]);
        app.rebuild_plan_index();
        assert_eq!(app.item_for_row(0).unwrap().new_name, "b 2.txt");
        assert_eq!(app.item_for_row(1).unwrap().new_name, "a 1.txt");
    }

    /// A stand-in for eframe's real storage, so the round trip goes through
    /// the codec the app actually uses.
    #[derive(Default)]
    struct MapStorage(std::collections::HashMap<String, String>);

    impl eframe::Storage for MapStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    /// Everything the app remembers has to survive a round trip through
    /// eframe's storage, or window/layout state silently stops persisting.
    ///
    /// Through `eframe::set_value`/`get_value` specifically, which serialise as
    /// **RON** — not the JSON this test used to use. `OpKind` is an internally
    /// tagged enum, the one shape RON handles through a special case, and a
    /// failure here is silent: `get_value` returns `None` and the whole blob
    /// resets to defaults.
    #[test]
    fn the_persisted_state_round_trips_through_eframe_storage() {
        let original = Persisted {
            session: SessionSettings {
                dir: PathBuf::from("/music"),
                pattern: "*.mp3".into(),
                subfolders: true,
                // A user who left the app in the grid must find it there.
                view: ViewMode::Grid,
                thumb_size: 160,
                thumb_border: true,
                ..Default::default()
            },
            pipeline_name: "Photo cleanup".to_owned(),
            field_history: std::collections::BTreeMap::from([(
                "free_format_pattern".to_owned(),
                vec!["<Parent>_<FullName>".to_owned()],
            )]),
            steps: Some(vec![
                (
                    OpKind::Casing(Casing::new(CaseMode::Title)),
                    StepConfig::scoped(Scope::Extension),
                ),
                (OpKind::default(), StepConfig::default()),
            ]),
            settings: RunSettings {
                counter: ren_core::CounterSetup {
                    start: 5,
                    step: 2,
                    ..Default::default()
                },
                parts: ren_core::PartsSpec::new("<%1> - <%2>"),
                require_all_tags: true,
                seed: 7,
                ..Default::default()
            },
            batch_replace: vec![ren_core::Replace::new("a", "b")],
            music_styles: vec!["<Album> - <Track>".to_owned()],
            startup: Startup {
                clear_subfolders: true,
                clear_pattern: false,
                clear_pipeline: true,
            },
            casing: ren_core::ops::CasingRules {
                exceptions: ren_core::ops::ExceptionRules {
                    words: vec!["CD".to_owned()],
                },
                ..Default::default()
            },
            table_style: crate::viewmodel::TableStyle {
                stripes: true,
                full_row_select: true,
            },
            columns: {
                let mut columns = crate::viewmodel::Columns::default();
                columns.all_mut()[0].width = 321.0;
                columns.move_down(0);
                columns
            },
            theme: Theme::Dark,
            simulate: true,
            operation: OpKind::default(),
            step: StepConfig::default(),
        };

        let mut storage = MapStorage::default();
        eframe::set_value(&mut storage, STORAGE_KEY, &original);
        let back: Persisted =
            eframe::get_value(&storage, STORAGE_KEY).expect("stored state must read back");

        assert_eq!(back.session, original.session);
        assert_eq!(back.pipeline_name, original.pipeline_name);
        assert_eq!(back.steps, original.steps);
        assert_eq!(back.settings, original.settings);
        assert_eq!(back.batch_replace, original.batch_replace);
        assert_eq!(back.music_styles, original.music_styles);
        // Order and width both, so a reordered table survives a restart.
        assert_eq!(back.columns, original.columns);
        assert_eq!(back.table_style, original.table_style);
        assert_eq!(back.casing, original.casing);
        assert_eq!(back.startup, original.startup);
        assert_eq!(back.theme, original.theme);
        assert_eq!(back.simulate, original.simulate);
    }

    /// The column list is reconciled *when the app is built*, not only when
    /// something calls `reconcile`.
    ///
    /// This is the wiring, and it is the half that bites: a blob written by a
    /// build with four columns leaves `visible()` shorter than the header
    /// expects, so every cell after the gap renders the wrong column's content.
    #[test]
    fn a_blob_from_a_build_with_fewer_columns_gains_the_rest_on_load() {
        let short: Persisted =
            serde_json::from_str(r#"{"columns":[{"kind":"Name","width":99.0,"visible":true}]}"#)
                .expect("an older blob still parses");
        assert_eq!(short.columns.all().len(), 1, "as stored");

        let journal = tempfile::TempDir::new().unwrap();
        let app = RenameItApp::build(
            short,
            History::in_dir(journal.path().to_path_buf()),
            Box::new(NoDialogs),
            ren_core::PresetStore::new(journal.path().join("presets")),
            ren_platform::host(),
            || {},
        );
        assert_eq!(app.columns.all().len(), 8, "and complete once loaded");
        assert_eq!(app.columns.all()[0].width, 99.0, "what was stored is kept");
    }

    /// The Startup switches decide what the app is built *from*.
    ///
    /// Applied to the blob before `build`, not to the app afterwards: for
    /// "uncheck Subfolders" the difference is the whole point, because the
    /// walk it exists to avoid would already have happened.
    #[test]
    fn the_startup_switches_are_applied_before_anything_is_listed() {
        let stored = Persisted {
            session: SessionSettings {
                subfolders: true,
                pattern: "*.mp3".to_owned(),
                ..Default::default()
            },
            pipeline_name: "Photo cleanup".to_owned(),
            steps: Some(vec![(OpKind::default(), StepConfig::default())]),
            ..Default::default()
        };

        // Off by default: the app remembering where you were is what people
        // expect, and each switch is a reason to override that.
        let kept = Startup::default().applied_to(stored.clone());
        assert!(kept.session.subfolders);
        assert_eq!(kept.session.pattern, "*.mp3");
        assert_eq!(kept.steps.as_deref().map(<[_]>::len), Some(1));

        let cleared = Startup {
            clear_subfolders: true,
            clear_pattern: true,
            clear_pipeline: true,
        }
        .applied_to(stored);
        assert!(!cleared.session.subfolders);
        assert!(cleared.session.pattern.is_empty());
        // `Some(vec![])`, not `None`: absent means "an older build wrote this"
        // and restores the one default card, which is not what cleared means.
        assert_eq!(cleared.steps, Some(Vec::new()));
        assert_eq!(RenameItApp::restore(&cleared).len(), 0);
        assert!(cleared.pipeline_name.is_empty());
    }

    /// And the wiring: `build` applies them, so both entry points do.
    #[test]
    fn a_built_app_honours_the_startup_switches() {
        let journal = tempfile::TempDir::new().unwrap();
        let app = RenameItApp::build(
            Persisted {
                session: SessionSettings {
                    subfolders: true,
                    pattern: "*.mp3".to_owned(),
                    ..Default::default()
                },
                steps: Some(vec![(OpKind::default(), StepConfig::default())]),
                field_history: std::collections::BTreeMap::from([(
                    "free_format_pattern".to_owned(),
                    vec!["<Parent>_<FullName>".to_owned()],
                )]),
                startup: Startup {
                    clear_subfolders: true,
                    clear_pattern: true,
                    clear_pipeline: true,
                },
                ..Default::default()
            },
            History::in_dir(journal.path().to_path_buf()),
            Box::new(NoDialogs),
            ren_core::PresetStore::new(journal.path().join("presets")),
            ren_platform::host(),
            || {},
        );
        assert!(!app.session.settings.subfolders);
        assert!(app.session.settings.pattern.is_empty());
        assert!(app.stack.is_empty());
        // > *"Clear function input fields on startup — … if you are concerned
        // > about privacy"*
        //
        // **D128** answers that with the whole card stack, and justifies it with
        // that sentence — so a history holding every pattern the user has run
        // has to go with it, or the sentence stops being true.
        assert!(
            app.field_history.is_empty(),
            "privacy means the drop-down histories too"
        );
    }

    /// `RunSettings::seed` documents zero as *"nobody has chosen one"*, and only
    /// `RenameItApp::new` ever replaced it.
    ///
    /// A reset that adopted the default blob verbatim would leave every `<Rnd>`
    /// in the session producing the same value forever (P16) — and two batches
    /// renamed a week apart colliding.
    #[test]
    fn a_reset_chooses_a_fresh_seed_rather_than_the_unset_zero() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = tempfile::TempDir::new().unwrap();
        let mut app = RenameItApp::headless(dir.path().to_path_buf(), journal.path().to_path_buf());
        let ctx = egui::Context::default();

        app.reset_settings(&ctx);
        assert_ne!(
            app.settings.seed, 0,
            "zero means nobody has chosen one, and nothing else would ever fix it"
        );
    }

    /// The survivor list lives in one place, so a field added to `Persisted`
    /// later is **reset by default** rather than silently kept — the safe
    /// direction for a button whose whole job is putting things back.
    #[test]
    fn a_reset_keeps_the_folder_and_the_pipeline_and_puts_the_rest_back() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = tempfile::TempDir::new().unwrap();
        let mut app = RenameItApp::headless(dir.path().to_path_buf(), journal.path().to_path_buf());
        let ctx = egui::Context::default();

        app.music_styles.clear();
        app.batch_replace.clear();
        app.casing.exceptions.words.clear();
        app.theme = Theme::Dark;
        app.session.settings.guard_system_folders = false;
        app.session.settings.thumb_size = 200;
        app.session.settings.pattern = "*.mp3".to_owned();
        app.stack.name = "mine".to_owned();

        app.reset_settings(&ctx);

        assert!(!app.music_styles.is_empty());
        assert!(!app.batch_replace.is_empty());
        assert!(!app.casing.exceptions.words.is_empty());
        assert_eq!(app.theme, Theme::System);
        assert!(
            app.session.settings.guard_system_folders,
            "the one switch whose default is the safe one (D127)"
        );
        assert_eq!(
            app.session.settings.thumb_size,
            crate::viewmodel::THUMB_DEFAULT
        );

        assert_eq!(
            app.session.settings.dir,
            dir.path(),
            "where you are is not a setting"
        );
        assert_eq!(
            app.session.settings.pattern, "*.mp3",
            "nor is what you filtered to"
        );
        assert_eq!(app.stack.name, "mine", "nor is the pipeline you built");
    }

    /// A stored blob from an older version must not stop the app starting.
    #[test]
    fn a_missing_or_partial_blob_falls_back_to_defaults() {
        let partial: Persisted =
            serde_json::from_str(r#"{"theme":"Light"}"#).expect("defaults fill the gaps");
        assert_eq!(partial.theme, Theme::Light);
        assert_eq!(partial.steps, None);
        assert!(!partial.simulate);
        // Which restores as the one default card M2 and M3 always had.
        assert_eq!(RenameItApp::restore(&partial).len(), 1);

        // And an empty store is simply a first run.
        let empty = MapStorage::default();
        assert!(eframe::get_value::<Persisted>(&empty, STORAGE_KEY).is_none());
    }

    /// A Re-Number saved before M4 renamed its wire tag still loads.
    ///
    /// Without the `re_number` alias on the variant, this blob fails to parse
    /// and eframe silently resets *everything* — folder, theme, counter.
    #[test]
    fn a_blob_holding_the_old_re_number_spelling_still_loads() {
        let mut storage = MapStorage::default();
        let original = Persisted {
            steps: Some(vec![(
                OpKind::ReNumber(ren_core::ReNumber::default()),
                StepConfig::default(),
            )]),
            ..Default::default()
        };
        eframe::set_value(&mut storage, STORAGE_KEY, &original);

        // Rewrite the tag the way a pre-M4 build would have stored it.
        let stored = eframe::Storage::get_string(&storage, STORAGE_KEY).expect("just stored");
        assert!(
            stored.contains("renumber"),
            "expected the new spelling: {stored}"
        );
        eframe::Storage::set_string(
            &mut storage,
            STORAGE_KEY,
            stored.replace("\"renumber\"", "\"re_number\""),
        );

        let back: Persisted =
            eframe::get_value(&storage, STORAGE_KEY).expect("an older blob must still load");
        assert_eq!(back.steps, original.steps);
    }

    /// An M3 blob stored one operation and one step config. Upgrading must
    /// carry that into a one-card stack rather than discard what the user had
    /// configured.
    #[test]
    fn a_blob_from_before_the_card_stack_becomes_one_card() {
        /// `Persisted` as M3 defined it: one operation, one step config, and
        /// no card stack at all.
        #[derive(Serialize)]
        struct M3Persisted {
            session: SessionSettings,
            operation: OpKind,
            step: StepConfig,
            settings: RunSettings,
            theme: Theme,
            simulate: bool,
        }

        let mut storage = MapStorage::default();
        eframe::Storage::set_string(
            &mut storage,
            STORAGE_KEY,
            ron::ser::to_string(&M3Persisted {
                session: SessionSettings::default(),
                operation: OpKind::Casing(Casing::new(CaseMode::Upper)),
                step: StepConfig::scoped(Scope::Extension),
                settings: RunSettings::default(),
                theme: Theme::Dark,
                simulate: false,
            })
            .expect("serialisable"),
        );

        let text = eframe::Storage::get_string(&storage, STORAGE_KEY).unwrap();
        let persisted: Persisted = ron::from_str(&text)
            .unwrap_or_else(|e| panic!("an M3 blob must still load: {e}\n{text}"));
        let stack = RenameItApp::restore(&persisted);

        assert_eq!(stack.len(), 1, "the operation survived the upgrade");
        assert_eq!(stack.get(0).unwrap().op.name(), "casing");
        assert_eq!(stack.get(0).unwrap().scope, Scope::Extension);
        assert_eq!(persisted.theme, Theme::Dark);
    }

    /// A pipeline the user deliberately emptied must come back empty, which is
    /// why `steps` is an `Option` rather than a bare `Vec`.
    #[test]
    fn an_empty_pipeline_does_not_regrow_a_card_on_restart() {
        let mut storage = MapStorage::default();
        eframe::set_value(
            &mut storage,
            STORAGE_KEY,
            &Persisted {
                steps: Some(Vec::new()),
                ..Default::default()
            },
        );
        let back: Persisted = eframe::get_value(&storage, STORAGE_KEY).unwrap();
        assert!(RenameItApp::restore(&back).is_empty());
    }

    #[test]
    fn every_theme_maps_onto_an_egui_preference() {
        for theme in [Theme::System, Theme::Light, Theme::Dark] {
            let _: egui::ThemePreference = theme.into();
        }
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;
    use ren_platform::shell::MenuPreset;
    use std::sync::Mutex;

    /// The host, with the Explorer menu answered by this test instead.
    ///
    /// A wrapper rather than a hand-written platform: the ten other trait
    /// methods have real behaviour that the app leans on to list a folder at
    /// all, and a stub for each would be ten chances to make this test pass
    /// against something the app does not do.
    struct MenuSpy {
        inner: Arc<dyn Platform>,
        installed: Mutex<Option<bool>>,
        /// One entry per call, each holding the item captions it was given.
        writes: Mutex<Vec<Vec<String>>>,
    }

    impl MenuSpy {
        fn new(installed: Option<bool>) -> Arc<Self> {
            Arc::new(Self {
                inner: ren_platform::host(),
                installed: Mutex::new(installed),
                writes: Mutex::new(Vec::new()),
            })
        }

        fn writes(&self) -> Vec<Vec<String>> {
            self.writes.lock().unwrap().clone()
        }
    }

    /// `Platform: Debug`, and an `Arc<dyn Platform>` is not — so this prints
    /// the half that is this test's own.
    impl std::fmt::Debug for MenuSpy {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MenuSpy")
                .field("installed", &self.installed)
                .field("writes", &self.writes)
                .finish_non_exhaustive()
        }
    }

    impl Platform for MenuSpy {
        fn name(&self) -> &'static str {
            self.inner.name()
        }
        fn capabilities(&self) -> &'static [ren_platform::Capability] {
            self.inner.capabilities()
        }
        fn rename(&self, from: &Path, to: &Path) -> ren_platform::Result<()> {
            self.inner.rename(from, to)
        }
        fn get_attributes(
            &self,
            path: &Path,
        ) -> ren_platform::Result<ren_platform::FileAttributes> {
            self.inner.get_attributes(path)
        }
        fn set_attributes(
            &self,
            path: &Path,
            change: ren_platform::AttributeChange,
        ) -> ren_platform::Result<()> {
            self.inner.set_attributes(path, change)
        }
        fn get_times(&self, path: &Path) -> ren_platform::Result<ren_platform::FileTimes> {
            self.inner.get_times(path)
        }
        fn set_times(
            &self,
            path: &Path,
            change: ren_platform::TimeChange,
        ) -> ren_platform::Result<()> {
            self.inner.set_times(path, change)
        }
        fn naming_rules(&self, path: &Path) -> &'static ren_platform::NamingRules {
            self.inner.naming_rules(path)
        }
        fn case_sensitivity(&self, dir: &Path) -> ren_platform::CaseSensitivity {
            self.inner.case_sensitivity(dir)
        }
        fn reveal_in_file_manager(&self, path: &Path) -> ren_platform::Result<()> {
            self.inner.reveal_in_file_manager(path)
        }
        fn notify_shell_changed(&self, path: &Path) {
            self.inner.notify_shell_changed(path);
        }

        fn context_menu_installed(&self) -> Option<bool> {
            *self.installed.lock().unwrap()
        }

        fn set_context_menu(
            &self,
            installed: bool,
            presets: &[MenuPreset<'_>],
        ) -> ren_platform::Result<()> {
            *self.installed.lock().unwrap() = Some(installed);
            self.writes
                .lock()
                .unwrap()
                .push(presets.iter().map(|p| p.name.to_owned()).collect());
            Ok(())
        }
    }

    fn app_with(spy: &Arc<MenuSpy>) -> (RenameItApp, tempfile::TempDir, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("one.txt"), b"x").unwrap();
        let app = RenameItApp::headless_with_platform(
            dir.path().to_path_buf(),
            journal.path().to_path_buf(),
            spy.clone(),
        );
        (app, dir, journal)
    }

    /// The menu is a snapshot of a folder, so saving a preset has to write it
    /// again — nothing in the registry can enumerate a folder at right-click
    /// time.
    #[test]
    fn saving_a_preset_puts_it_in_the_menu() {
        let spy = MenuSpy::new(Some(true));
        let (mut app, _dir, _journal) = app_with(&spy);

        app.save_preset("Tidy up");
        app.rewrite_menu();

        assert_eq!(spy.writes(), [vec!["Tidy up".to_owned()]]);
    }

    /// **The guard.** Saving a preset must never be the thing that puts an
    /// entry in someone's context menu: this is a repair, not an install, and a
    /// user who has never ticked the box must not acquire one by using the
    /// program.
    #[test]
    fn a_menu_that_is_not_installed_is_not_quietly_installed() {
        for state in [Some(false), None] {
            let spy = MenuSpy::new(state);
            let (mut app, _dir, _journal) = app_with(&spy);

            app.save_preset("Tidy up");
            app.rewrite_menu();

            assert!(spy.writes().is_empty(), "{state:?}");
        }
    }

    /// A preset deleted while the program was closed, a preset dropped in by
    /// hand, and a portable copy that moved are all the same repair — and it
    /// happens without the user being told to tick anything again.
    #[test]
    fn starting_up_rewrites_the_menu_from_the_folder_as_it_is_now() {
        let spy = MenuSpy::new(Some(true));
        let (mut app, dir, _journal) = app_with(&spy);
        app.save_preset("Kept");
        app.save_preset("Also kept");

        app.start_from(&crate::launch::Launch {
            paths: vec![dir.path().to_path_buf()],
            start_in: true,
            from_shell: true,
            ..Default::default()
        });

        let last = spy.writes().pop().expect("one rewrite at startup");
        let mut names = last;
        names.sort();
        assert_eq!(names, ["Also kept", "Kept"]);
    }

    /// The funnel. Every preset mutation already passes through `apply_drawer`,
    /// so the flag is set there rather than in each of the five actions that
    /// change the folder — and a frame in which the drawer merely sat open
    /// leaves it alone, or the registry would be rewritten sixty times a
    /// second.
    #[test]
    fn a_drawer_action_is_what_marks_the_menu_stale() {
        let spy = MenuSpy::new(Some(true));
        let (mut app, _dir, _journal) = app_with(&spy);
        app.save_preset("Doomed");
        let path = app.presets.list().0[0].path.clone();

        app.apply_drawer(presets::DrawerOutput::default());
        assert!(!app.needs_menu_rewrite, "the drawer only sat there");

        app.apply_drawer(presets::DrawerOutput {
            delete: Some(path),
            ..Default::default()
        });
        assert!(app.needs_menu_rewrite);

        // What the next frame does with it.
        app.needs_menu_rewrite = false;
        app.rewrite_menu();
        assert_eq!(
            spy.writes(),
            [Vec::<String>::new()],
            "the folder as it is now, which is empty"
        );
    }

    /// The caveat is about a **command line**, so nothing that did not arrive on
    /// one may raise it.
    ///
    /// A drag of two hundred files onto the window is the case this is really
    /// guarding: it has no limit at all, and a banner about 2000 characters
    /// would be inventing one.
    #[test]
    fn only_a_right_click_selection_can_raise_the_caveat() {
        let shell = crate::launch::Launch {
            paths: vec![PathBuf::from("/tmp/a.jpg")],
            from_shell: true,
            command_line_chars: Some(1900),
            ..Default::default()
        };
        assert!(SelectionCaveat::for_launch(&shell).is_some());

        assert!(
            SelectionCaveat::for_launch(&crate::launch::Launch {
                from_shell: false,
                ..shell.clone()
            })
            .is_none(),
            "a drag, which went through no command line"
        );
        assert!(
            SelectionCaveat::for_launch(&crate::launch::Launch {
                start_in: true,
                ..shell.clone()
            })
            .is_none(),
            "one path however many are selected"
        );
        assert!(
            SelectionCaveat::for_launch(&crate::launch::Launch {
                command_line_chars: None,
                ..shell.clone()
            })
            .is_none(),
            "a platform that cannot answer says nothing"
        );
    }

    /// A handful of files is the overwhelming majority of right-clicks, and
    /// every one of them would see this banner if the threshold were absent.
    #[test]
    fn an_ordinary_selection_says_nothing_at_all() {
        for chars in [0, 120, 1_400, 1_599] {
            assert!(
                SelectionCaveat::for_launch(&crate::launch::Launch {
                    paths: vec![PathBuf::from("/tmp/a.jpg")],
                    from_shell: true,
                    command_line_chars: Some(chars),
                    ..Default::default()
                })
                .is_none(),
                "{chars}"
            );
        }
        // 1600 is four fifths of the documented 2000.
        assert!(
            SelectionCaveat::for_launch(&crate::launch::Launch {
                paths: vec![PathBuf::from("/tmp/a.jpg")],
                from_shell: true,
                command_line_chars: Some(1_600),
                ..Default::default()
            })
            .is_some()
        );
    }

    /// The repair needs somewhere to go, and Explorer sends files rather than
    /// the folder they are in.
    #[test]
    fn the_caveat_carries_the_folder_the_selection_came_from() {
        let caveat = SelectionCaveat::for_launch(&crate::launch::Launch {
            paths: vec![
                PathBuf::from("/tmp/photos/a.jpg"),
                "/tmp/photos/b.jpg".into(),
            ],
            from_shell: true,
            command_line_chars: Some(1_900),
            ..Default::default()
        })
        .expect("near the limit");
        assert_eq!(caveat.dir.as_deref(), Some(Path::new("/tmp/photos")));
    }

    /// The caption is the name and the command carries the path, because two
    /// presets may share a name — `PresetStore::load_named` returns `Ambiguous`
    /// rather than guessing, and a menu keyed on names would ship that
    /// ambiguity to every right-click.
    #[test]
    fn the_menu_takes_the_name_for_the_caption_and_the_path_for_the_command() {
        let entries = [
            ren_core::PresetEntry {
                name: "Tidy".to_owned(),
                description: String::new(),
                path: PathBuf::from("/p/a.toml"),
                steps: 1,
            },
            ren_core::PresetEntry {
                name: "Tidy".to_owned(),
                description: String::new(),
                path: PathBuf::from("/p/b.toml"),
                steps: 1,
            },
        ];
        let menu = menu_presets(&entries);
        assert_eq!(menu.len(), 2, "two items, not one");
        assert_eq!(menu[0].name, "Tidy");
        assert_eq!(menu[0].file, Path::new("/p/a.toml"));
        assert_eq!(menu[1].file, Path::new("/p/b.toml"));
    }
}
