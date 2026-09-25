//! The Settings window.
//!
//! `docs/DESIGN.md` S9 lists nine pages eventually. Two came first: the Batch
//! Replace list, and Appearance — which is where the theme control moves to,
//! so the left panel can be purely the pipeline.
//!
//! The shell is deliberately a page list plus a body, so M8 fills it in rather
//! than rebuilding it.

use ren_core::ops::Replace;
use ren_core::ops::music_rename::SHIPPED_STYLES;

use crate::app::Theme;
use ren_core::listing::PatternScope;
use ren_core::ops::CasingRules;

use crate::app::Startup;
use crate::viewmodel::{ColumnKind, Columns, SessionSettings};
use crate::widgets::rule_table;
use crate::widgets::string_list::StringList;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    BatchReplace,
    MusicStyles,
    /// The exception lists a **new** Set Casing card copies. Not run-wide:
    /// each card keeps its own.
    CasingExceptions,
    /// Which columns the file table shows, in what order.
    Display,
    /// What a fresh start deliberately forgets.
    Startup,
    /// The file manager's RenameIt menu.
    ShellIntegration,
    /// The three visibility switches and the system-folder guard.
    FileSystem,
    Appearance,
    /// Answers to the questions a confused user actually has, plus the
    /// shortcuts to where the app keeps its files and the reset.
    ProblemSolver,
}

impl Page {
    const ALL: [Self; 9] = [
        Self::BatchReplace,
        Self::MusicStyles,
        Self::CasingExceptions,
        Self::Display,
        Self::FileSystem,
        Self::Startup,
        Self::ShellIntegration,
        Self::Appearance,
        Self::ProblemSolver,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::BatchReplace => "Batch Replace",
            Self::MusicStyles => "Music Styles",
            Self::CasingExceptions => "Casing Exceptions",
            Self::Display => "Display",
            Self::Startup => "Startup",
            Self::ShellIntegration => "Shell Integration",
            Self::ProblemSolver => "Problem Solver",
            Self::FileSystem => "File System",
            Self::Appearance => "Appearance",
        }
    }
}

/// The app-wide default lists this window edits.
///
/// Bundled rather than passed one by one: every page added means another
/// argument at the single call site, and the reason each is here — a default a
/// *new* card copies, never something a saved pipeline depends on — is the same
/// for all of them.
pub struct Defaults<'a> {
    pub batch_replace: &'a mut Vec<Replace>,
    pub music_styles: &'a mut Vec<String>,
    /// The exception words a **new** Set Casing card copies (D35's bargain,
    /// the same one Batch Replace strikes). **Not `RunSettings`** — that holds
    /// the counter, parts, tag policy and seed, and nothing about casing. Each
    /// card owns its own `CasingRules`, so editing here never changes a
    /// pipeline already built, which `the_casing_exception_list_is_a_default_
    /// for_new_cards` asserts.
    pub casing: &'a mut CasingRules,
    pub columns: &'a mut Columns,
    pub table_style: &'a mut crate::viewmodel::TableStyle,
    pub startup: &'a mut Startup,
    /// For the Problem Solver page's folder shortcuts.
    pub platform: &'a dyn ren_platform::Platform,
    pub journal_dir: &'a std::path::Path,
    pub preset_dir: &'a std::path::Path,
    /// What the Explorer menu would show. Read fresh each frame the modal is
    /// open, the same way the preset drawer reads it — the folder is the truth,
    /// and a cached count is one more thing that can be stale on a page whose
    /// whole point is not being stale.
    pub presets: &'a [ren_core::PresetEntry],
    /// The source settings the File System page owns.
    pub session: &'a mut SessionSettings,
}

#[derive(Debug, Default)]
pub struct Output {
    /// A theme the user picked, if they did.
    pub theme: Option<Theme>,
    /// An interface size the user picked, as an egui zoom factor.
    ///
    /// Travels out rather than being stored, for the same reason `theme` does:
    /// it has to be pushed into the `egui::Context`, not merely remembered.
    pub zoom: Option<f32>,
    /// Set when something changed that only a fresh listing can show — the
    /// visibility switches, the pattern scope, the guard.
    ///
    /// The only thing a page asks the app to *do*. No page here feeds a plan:
    /// the rule, style and casing lists are copied by a card when it is made
    /// (D35), and Display and Startup decide how things look and what the next
    /// start forgets. There used to be a `changed` beside this that re-planned
    /// the whole listing on every edit — once per frame while the thumbnail
    /// slider was dragged — for a result that could not differ.
    pub relist: bool,
    /// The Problem Solver page's reset button was pressed.
    ///
    /// Not the reset itself: a reset puts back more than this window can
    /// reach (see `confirm::reset_ui` for the list), and the confirmation
    /// belongs **on top of** this modal rather than inside it.
    pub reset: bool,
    pub close: bool,
}

pub fn ui(ctx: &egui::Context, page: &mut Page, defaults: Defaults<'_>, theme: Theme) -> Output {
    // Read from the `Context` rather than carried in: egui owns this value and
    // persists it itself, so a copy of our own could only go stale.
    let current_zoom = ctx.zoom_factor();
    let mut out = Output::default();

    let modal = egui::Modal::new(egui::Id::new("settings")).show(ctx, |ui| {
        ui.set_width(RAIL_WIDTH + BODY_WIDTH + 3.0 * crate::theme::space::SNUG);
        ui.heading("Settings");
        ui.add_space(crate::theme::space::SNUG);

        // One height for every page.
        //
        // The window used to be `set_width(620)` with no outer scroll area and
        // no floor, so its height was whatever the open page needed: Appearance
        // came out 142 pt and Casing Exceptions 562, and the Close button moved
        // 420 pt up the screen when you changed tab. Three pages carried their
        // own inner `ScrollArea` and the longest one — Problem Solver — carried
        // none, so the only page that could not overflow was one that never
        // needed to.
        //
        // The rail is what makes a floor honest rather than arbitrary: nine
        // rows of tabs are a height the window has to find anyway.
        let body_height = crate::theme::modal_body_height(ctx, 132.0)
            .max(Page::ALL.len() as f32 * ui.spacing().interact_size.y);

        // Both halves are allocated as exact rects and drawn into with
        // `scope_builder`, rather than `allocate_ui` + a `ScrollArea` that
        // sizes itself from what is available. The latter is a feedback loop
        // inside an auto-sizing `Modal`: the scroll area asks the modal how
        // much room there is, the modal's height is whatever the scroll area
        // took last frame, and the window grew by one rail row every frame
        // until its top was off the screen. An exact rect has no opinion to
        // feed back.
        ui.horizontal_top(|ui| {
            let (rail_rect, _) =
                ui.allocate_exact_size(egui::vec2(RAIL_WIDTH, body_height), egui::Sense::hover());
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(rail_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    // A vertical rail rather than nine tabs across the top. The
                    // strip was a non-wrapping `ui.horizontal` inside 620 pt and
                    // "Problem Solver" already touched the right edge at 100 % —
                    // which is what `docs/manual-checks.md` has an open item
                    // asking somebody to look at under 125 % and 150 % scaling.
                    for candidate in Page::ALL {
                        let selected = *page == candidate;
                        if ui
                            .add_sized(
                                egui::vec2(RAIL_WIDTH, ui.spacing().interact_size.y),
                                egui::Button::selectable(selected, candidate.label()),
                            )
                            .clicked()
                        {
                            *page = candidate;
                        }
                    }
                },
            );
            // A drawn rule rather than `ui.separator()`: a separator inside a
            // horizontal layout sizes itself to the parent's height, and the
            // parent's height is what this whole block is trying to decide —
            // which is the loop that made the window grow a row per frame.
            let (rule, _) = ui.allocate_exact_size(
                egui::vec2(crate::theme::space::SNUG, body_height),
                egui::Sense::hover(),
            );
            ui.painter().vline(
                rule.center().x,
                rule.y_range(),
                ui.visuals().widgets.noninteractive.bg_stroke,
            );

            let body_width = ui.available_width();
            let (body_rect, _) =
                ui.allocate_exact_size(egui::vec2(body_width, body_height), egui::Sense::hover());
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("settings_body")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            page_ui(ui, page, defaults, theme, current_zoom, &mut out);
                        });
                },
            );
        });

        ui.add_space(crate::theme::space::SNUG);
        ui.separator();
        ui.add_space(crate::theme::space::TIGHT);
        // Bottom-right, in a footer that cannot move: it used to be
        // bottom-left with no separator, at whatever height the page ended.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                out.close = true;
            }
        });
    });

    if modal.should_close() {
        out.close = true;
    }
    out
}

/// The rail: the widest label in it, plus room to breathe.
///
/// 150 fitted "Shell Integration" at the old `Button` size of 13. The type
/// scale moved, so this did too — a rail label that ellipsizes is a navigation
/// control that cannot say where it goes.
const RAIL_WIDTH: f32 = 168.0;
/// The page beside it — wider than the old whole window, because the rail
/// takes the width the tab strip used to spend. Only the modal's own width is
/// set from this; the body itself takes what is left.
const BODY_WIDTH: f32 = 560.0;

fn page_ui(
    ui: &mut egui::Ui,
    page: &mut Page,
    defaults: Defaults<'_>,
    theme: Theme,
    current_zoom: f32,
    out: &mut Output,
) {
    match *page {
        Page::BatchReplace => batch_replace_ui(ui, defaults.batch_replace),
        Page::MusicStyles => music_styles_ui(ui, defaults.music_styles),
        Page::CasingExceptions => casing_ui(ui, defaults.casing),
        Page::Display => display_ui(ui, defaults.columns, defaults.table_style, defaults.session),
        Page::Startup => startup_ui(ui, defaults.startup),
        Page::ShellIntegration => shell_ui(ui, defaults.platform, defaults.presets),
        Page::ProblemSolver => {
            out.reset = problem_solver_ui(
                ui,
                defaults.platform,
                defaults.journal_dir,
                defaults.preset_dir,
            )
        }
        Page::FileSystem => out.relist |= file_system_ui(ui, defaults.session),
        Page::Appearance => {
            let (picked, zoom) = appearance_ui(ui, theme, current_zoom);
            out.theme = picked;
            out.zoom = zoom;
        }
    }
}

fn batch_replace_ui(ui: &mut egui::Ui, rules: &mut Vec<Replace>) {
    ui.label(
        egui::RichText::new(
            "The list a new Batch Replace operation starts from. A card keeps its own \
             copy, so editing here never changes a pipeline you have already built — or \
             a preset you have already saved.",
        )
        .weak()
        .small(),
    );
    ui.add_space(6.0);
    rule_table::ui(ui, rules);
}

/// The list Music Rename's radios offer.
///
/// A plain editable list, not `rule_table`: a style is one string, and the
/// order is what the radios are read in.
fn music_styles_ui(ui: &mut egui::Ui, styles: &mut Vec<String>) {
    ui.label(
        egui::RichText::new(
            "The styles Music Rename offers. A card stores the pattern it ended up with, \
             not which row it came from, so editing here never changes a pipeline you have \
             already built — or a preset you have already saved.",
        )
        .weak()
        .small(),
    );
    ui.add_space(6.0);

    StringList::new("music_style_row")
        .hint("<Artist> - <Title>")
        .add_label("+ Add style")
        .defaults(
            "The three shipped styles",
            SHIPPED_STYLES.iter().map(|s| (*s).to_owned()).collect(),
        )
        .show(ui, styles);
}

/// The sizes offered, as egui zoom factors.
///
/// Steps rather than a slider, and deliberately: `zoom_factor` changes
/// `pixels_per_point`, so every glyph is re-rasterised at the new scale and the
/// old rasterisations stay in the atlas until it passes its rebuild threshold.
/// Dragging a continuous control does that once per frame. Seven steps also
/// make the setting something a user can *choose* rather than dial in.
const SIZES: [(f32, &str); 7] = [
    (0.9, "90%"),
    (1.0, "100%"),
    (1.1, "110%"),
    (1.25, "125%"),
    (1.5, "150%"),
    (1.75, "175%"),
    (2.0, "200%"),
];

/// The narrowest and widest the app will set itself to.
///
/// `Context::set_zoom_factor` does no clamping of its own — only
/// `gui_zoom::zoom_in`/`out` apply egui's 0.2–5.0 bounds — and a stray `0.0`
/// trips a debug assert in the glyph cache and renders garbage in release.
pub const MIN_ZOOM: f32 = 0.5;
pub const MAX_ZOOM: f32 = 3.0;

fn appearance_ui(ui: &mut egui::Ui, theme: Theme, zoom: f32) -> (Option<Theme>, Option<f32>) {
    let mut picked = None;
    ui.label(egui::RichText::new("Theme").strong());
    ui.horizontal(|ui| {
        for (candidate, label) in [
            (Theme::System, "System"),
            (Theme::Light, "Light"),
            (Theme::Dark, "Dark"),
        ] {
            if ui.selectable_label(theme == candidate, label).clicked() {
                picked = Some(candidate);
            }
        }
    });

    ui.add_space(crate::theme::space::SECTION);
    ui.label(egui::RichText::new("Interface size").strong());
    ui.label(
        egui::RichText::new(
            "Scales the whole window — text, spacing, icons and rows together, so nothing \
             is left behind at its old size.",
        )
        .weak()
        .small(),
    );
    ui.add_space(crate::theme::space::TIGHT);
    let mut chosen = None;
    ui.horizontal(|ui| {
        for (factor, label) in SIZES {
            // Compared with a tolerance: the keyboard steps in 0.1 from
            // wherever it is, so the live value need not be one of ours.
            let current = (zoom - factor).abs() < 0.01;
            if ui.selectable_label(current, label).clicked() {
                chosen = Some(factor);
            }
        }
    });
    ui.add_space(crate::theme::space::TIGHT);
    ui.label(
        egui::RichText::new(
            "Ctrl and + or − do the same thing from anywhere in the app, in smaller steps; \
             Ctrl and 0 puts it back to 100%.",
        )
        .weak()
        .small(),
    );

    (picked, chosen)
}

/// Set Casing's two exception lists.
///
/// Both live on `CasingRules`, which is **per card**: this page edits the copy
/// a *new* card starts from, and a card that already exists never looks at it
/// again (D35). Editing here therefore cannot change a pipeline you have built
/// or a preset you have saved — which is the whole point, and the opposite of
/// what this comment claimed until M8.
fn casing_ui(ui: &mut egui::Ui, rules: &mut CasingRules) {
    ui.label(egui::RichText::new("Exception words").strong());
    ui.label(
        egui::RichText::new(
            "Words Set Casing writes exactly as they appear here, whatever the mode — \
             acronyms and the like.",
        )
        .weak()
        .small(),
    );
    ui.add_space(4.0);
    StringList::new("casing_exception_row")
        .hint("CD")
        .add_label("+ Add word")
        .width(200.0)
        .defaults(
            "The list shipped with the app",
            CasingRules::default().exceptions.words,
        )
        .show(ui, &mut rules.exceptions.words);

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    ui.label(egui::RichText::new("Title Case lowercase exceptions").strong());
    ui.label(
        egui::RichText::new(
            "Words Title Case leaves lowercase unless they start the name — short \
             prepositions, articles and conjunctions, typically.",
        )
        .weak()
        .small(),
    );
    ui.add_space(4.0);
    StringList::new("casing_lowercase_row")
        .hint("the")
        .add_label("+ Add word")
        .width(200.0)
        .defaults(
            "The list shipped with the app",
            CasingRules::default().title_case.lowercase_exceptions,
        )
        .show(ui, &mut rules.title_case.lowercase_exceptions);
}

/// The Display page: which columns the file table shows and in what order,
/// the two row switches, and the thumbnails.
fn display_ui(
    ui: &mut egui::Ui,
    columns: &mut Columns,
    style: &mut crate::viewmodel::TableStyle,
    session: &mut SessionSettings,
) {
    ui.label(
        egui::RichText::new(
            "Columns in the file list. Drag a column edge in the table itself to resize it.",
        )
        .weak()
        .small(),
    );
    ui.add_space(6.0);

    let mut swap: Option<(usize, bool)> = None;
    let count = columns.all().len();
    for (index, column) in columns.all_mut().iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let is_name = column.kind == ColumnKind::Name;
            // Name is the row's identity; a table without it is a list of
            // sizes, so the box that would remove it is disabled rather than
            // silently put back.
            ui.add_enabled(
                !is_name,
                egui::Checkbox::new(&mut column.visible, column.kind.label()),
            )
            .on_disabled_hover_text("The name is what identifies a row");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        index + 1 < count,
                        crate::widgets::icons::IconButton::new(
                            crate::widgets::icons::Icon::CaretDown,
                            "Move down",
                        ),
                    )
                    .clicked()
                {
                    swap = Some((index, false));
                }
                if ui
                    .add_enabled(
                        index > 0,
                        crate::widgets::icons::IconButton::new(
                            crate::widgets::icons::Icon::CaretUp,
                            "Move up",
                        ),
                    )
                    .clicked()
                {
                    swap = Some((index, true));
                }
            });
        });
    }

    if let Some((index, up)) = swap {
        if up {
            columns.move_up(index);
        } else {
            columns.move_down(index);
        }
    }

    ui.add_space(8.0);
    if ui
        .button("Restore defaults")
        .on_hover_text("The four the app starts with, in the order they start in")
        .clicked()
    {
        *columns = Columns::default();
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    ui.checkbox(&mut style.stripes, "Shade every other row");
    ui.checkbox(
        &mut style.full_row_select,
        "Click anywhere on a row to select it",
    )
    .on_hover_text(
        "Off, you must click the name — which is what this app has always done. \
         The name still takes the double-click that renames it either way.",
    );

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    thumbnails_ui(ui, session);
}

/// How big thumbnails are drawn, and whether they get a border.
///
/// One slider rather than "small, medium or large" (D142): there was never a
/// reason three fixed sizes should be the only three.
///
/// Not a listing change — nothing here decides what is *in* the list, only how
/// big the picture beside it is, so no relist follows. Measured in points,
/// like every other size in the window: a 2× display decodes twice the pixels
/// (`tile::key_for`).
fn thumbnails_ui(ui: &mut egui::Ui, session: &mut SessionSettings) {
    ui.label(egui::RichText::new("Thumbnails").strong());
    ui.label(
        egui::RichText::new(
            "Used by the Thumb column and by the grid. Sizes in between the marks are drawn \
             from the next size up, so dragging this does not re-read every picture.",
        )
        .weak()
        .small(),
    );
    ui.add_space(4.0);

    ui.add(
        egui::Slider::new(
            &mut session.thumb_size,
            crate::viewmodel::THUMB_MIN..=crate::viewmodel::THUMB_MAX,
        )
        .text("Size")
        .suffix(" pt"),
    );
    ui.checkbox(&mut session.thumb_border, "Draw a border around thumbnails");
}

/// The three visibility switches and the system-folder guard.
///
/// Returns true when the listing has to be rebuilt, which is every change on
/// this page — all four decide what is *in* the list.
fn file_system_ui(ui: &mut egui::Ui, session: &mut SessionSettings) -> bool {
    let mut changed = false;

    ui.label(egui::RichText::new("Show").strong());
    ui.label(
        egui::RichText::new(
            "Everything is shown by default and you narrow it: a renamer that quietly leaves \
             rows out is the more dangerous of the two, because you cannot see what is not \
             there.",
        )
        .weak()
        .small(),
    );
    ui.add_space(4.0);
    for (label, flag) in [
        ("Hidden files and folders", &mut session.show_hidden),
        ("System files and folders", &mut session.show_system),
        (
            "Write-protected files and folders",
            &mut session.show_read_only,
        ),
    ] {
        changed |= ui.checkbox(flag, label).changed();
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    ui.label(egui::RichText::new("Pattern box applies to").strong());
    ui.horizontal(|ui| {
        for (scope, label) in [
            (PatternScope::Both, "Both"),
            (PatternScope::Files, "Files"),
            (PatternScope::Folders, "Folders"),
        ] {
            if ui
                .selectable_label(session.pattern_applies == scope, label)
                .clicked()
                && session.pattern_applies != scope
            {
                session.pattern_applies = scope;
                changed = true;
            }
        }
    });
    ui.label(
        egui::RichText::new("Only matters with both Files and Folders switched on.")
            .weak()
            .small(),
    );

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    ui.label(egui::RichText::new("System folders").strong());
    changed |= ui
        .checkbox(
            &mut session.guard_system_folders,
            "Refuse to rename inside operating-system folders",
        )
        .on_hover_text(
            "C:\\Windows, Program Files, /usr, /etc and the like. This is the one mistake undo \
             cannot take back: the rename succeeds, the journal records it, and the machine \
             stops booting before anybody presses Ctrl+Z.",
        )
        .changed();
    if !session.guard_system_folders {
        ui.label(
            egui::RichText::new(
                "⚠ The guard is off. Renaming inside an operating-system folder can break this \
                 machine in a way undo does not fix.",
            )
            .color(ui.visuals().warn_fg_color)
            .small(),
        );
    }

    changed
}

/// What a fresh start deliberately forgets — the Startup page. Every switch is
/// off by default.
fn startup_ui(ui: &mut egui::Ui, startup: &mut Startup) {
    ui.label(
        egui::RichText::new(
            "The app remembers where you were. These are the parts you can tell it to forget.",
        )
        .weak()
        .small(),
    );
    ui.add_space(6.0);

    ui.checkbox(&mut startup.clear_subfolders, "Uncheck Subfolders")
        .on_hover_text(
            "So a start is never slow: a deep tree left switched on is walked before the \
             window appears.",
        );
    ui.checkbox(&mut startup.clear_pattern, "Clear the pattern box")
        .on_hover_text("So a forgotten *.mp3 does not silently hide everything else");
    ui.checkbox(&mut startup.clear_pipeline, "Clear the pipeline")
        .on_hover_text(
            "So the next person at this machine does not see what you were renaming. The \
             whole card stack goes, rather than each box being blanked: eight cards with \
             every field empty is not privacy, it is a puzzle. Save a preset to get it back \
             deliberately.",
        );

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("These take effect the next time the app starts.")
            .weak()
            .small(),
    );
}

/// Answers to the problems people actually hit, and shortcuts to the folders
/// the app keeps its files in.
///
/// The shortcuts are the useful half and the only half that can go stale, so
/// they are read from the same functions the app uses rather than typed out.
fn problem_solver_ui(
    ui: &mut egui::Ui,
    platform: &dyn ren_platform::Platform,
    journal_dir: &std::path::Path,
    preset_dir: &std::path::Path,
) -> bool {
    ui.label(egui::RichText::new("Where things are kept").strong());
    if ren_platform::is_portable() {
        ui.label(
            egui::RichText::new(
                "This is a portable install: everything below lives beside the program, and \
                 copying that folder takes your presets, scripts and settings with it.",
            )
            .weak()
            .small(),
        );
    } else {
        ui.label(
            egui::RichText::new(format!(
                "A normal install, keeping its files in the per-user folder. Put a file called \
                 {} beside the program to make it portable.",
                ren_platform::PORTABLE_MARKER
            ))
            .weak()
            .small(),
        );
    }
    ui.add_space(4.0);

    let script_dir = ren_core::script::ScriptStore::user().dir().to_path_buf();
    for (label, path, what) in [
        (
            "Undo journal",
            journal_dir.to_path_buf(),
            "One folder per rename. Undo reads these, and so does crash recovery at startup.",
        ),
        (
            "Presets",
            preset_dir.to_path_buf(),
            "One file per preset. Copy them between machines.",
        ),
        (
            "Scripts",
            script_dir,
            "The .koto files a Script card can run, including the nine shipped examples.",
        ),
    ] {
        ui.horizontal(|ui| {
            if ui.button("📂").on_hover_text("Open this folder").clicked() {
                let _ = platform.reveal_in_file_manager(&path);
            }
            ui.label(egui::RichText::new(label).strong());
            ui.label(
                egui::RichText::new(path.display().to_string())
                    .weak()
                    .monospace()
                    .small(),
            );
        })
        .response
        .on_hover_text(what);
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);

    ui.label(egui::RichText::new("If something looks wrong").strong());
    ui.add_space(4.0);
    for (problem, answer) in [
        (
            "The Rename button is greyed out",
            "It always says why, just above itself. A conflict blocks the whole run rather \
             than skipping a file, because the alternative is renaming some of your files \
             and not others.",
        ),
        (
            "A tag came out as literal text",
            "It did not: a tag we do not recognise is an error, not empty text. The editor \
             names it while you type. Check the spelling against the <tags> menu.",
        ),
        (
            "Some files are missing from the list",
            "Check the pattern box, the Files/Folders switches, and Settings ▸ File System. \
             Anything the walk could not read is counted beside the folder path.",
        ),
        (
            "A rename finished but left files behind",
            "A run stops at a file it cannot rename, or at Cancel; Undo reverts it. Only a \
             crash is rolled back at the next start.",
        ),
        (
            "Undo is greyed out after tagging music",
            "Writing tags and stripping them change what is inside a file, and nothing can \
             put that back. Both ask before they run and name the files.",
        ),
    ] {
        ui.label(egui::RichText::new(problem).strong().small());
        ui.label(egui::RichText::new(answer).weak().small());
        ui.add_space(4.0);
    }

    ui.separator();
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Start again").strong());
    ui.label(
        egui::RichText::new(
            "Puts every setting in this window back to what the app ships with. Your \
             files, presets, scripts and undo history are not settings and are left \
             alone — the folders they live in are listed above.",
        )
        .weak()
        .small(),
    );
    ui.add_space(4.0);
    // The ellipsis is the promise that it asks first.
    ui.button("Reset all settings…")
        .on_hover_text("Asks before it does anything")
        .clicked()
}

/// The file manager's RenameIt menu — D7, widened by the preset menu.
///
/// Everything on this page is **read back from the registry** rather than
/// remembered. A menu can be removed by another install, a registry cleaner, or
/// a user with `regedit` open, and a remembered `true` would be a page
/// confidently describing something that is not there.
fn shell_ui(
    ui: &mut egui::Ui,
    platform: &dyn ren_platform::Platform,
    presets: &[ren_core::PresetEntry],
) {
    ui.label(
        egui::RichText::new(
            "Open RenameIt from your file manager's right-click menu, on the files you \
             have selected there — or with one of your presets already loaded.",
        )
        .weak()
        .small(),
    );
    ui.add_space(8.0);

    let Some(installed) = platform.context_menu_installed() else {
        ui.label(format!(
            "There is no context-menu entry to install on {}. Windows is the platform this \
             ships for; the Linux desktop entry is M9's job.",
            platform.name()
        ));
        return;
    };

    let menu = crate::app::menu_presets(presets);
    // Kept across frames in egui's temp store, not in a local: the page is
    // redrawn on the next mouse move, and a local set only on the click's
    // frame showed a failed registry write for one frame — after which the
    // box simply refused to stay ticked, with nothing saying why. Cleared by
    // the next write that succeeds.
    let failure_id = ui.id().with("shell_menu_failure");
    let mut failure: Option<String> = ui.data(|d| d.get_temp(failure_id));
    let mut wrote = |result: ren_platform::Result<()>| {
        failure = result.err().map(|error| error.to_string());
    };

    let mut want = installed;
    if ui
        .checkbox(
            &mut want,
            "Show a RenameIt menu when I right-click files, folders and drives",
        )
        .on_hover_text(
            "Written under HKEY_CURRENT_USER, so this needs no administrator and removing it \
             is deleting what it wrote.",
        )
        .changed()
    {
        wrote(platform.set_context_menu(want, &menu));
    }

    ui.add_space(6.0);
    if installed {
        ui.horizontal(|ui| {
            // What the menu holds, not what the folder holds: the plan stops
            // at `MAX_PRESETS`, and a count above it would describe a menu
            // that does not exist.
            let shown = menu.len().min(ren_platform::shell::plan::MAX_PRESETS);
            ui.label(
                egui::RichText::new(format!(
                    "Installed, with {shown} preset{} in it.",
                    if shown == 1 { "" } else { "s" }
                ))
                .small(),
            );
            if ui
                .button("Refresh")
                .on_hover_text(
                    "Writes the menu again from the presets that are here now. The menu is a \
                     snapshot — RenameIt refreshes it whenever you add, rename or delete a \
                     preset, and when it starts.",
                )
                .clicked()
            {
                wrote(platform.set_context_menu(true, &menu));
            }
        });
    }

    if menu.len() > ren_platform::shell::plan::MAX_PRESETS {
        // Said here because nothing else says it: the plan truncates the
        // list silently, and a user with forty-five presets would otherwise
        // find five missing from the menu with no clue why.
        ui.label(
            egui::RichText::new(format!(
                "The menu holds the first {} presets by name; {} more are here but not in it.",
                ren_platform::shell::plan::MAX_PRESETS,
                menu.len() - ren_platform::shell::plan::MAX_PRESETS
            ))
            .color(ui.visuals().warn_fg_color)
            .small(),
        );
    }

    match &failure {
        Some(error) => {
            // Shown rather than swallowed: the one thing worse than a failed
            // registry write is one the user thinks succeeded.
            ui.label(
                egui::RichText::new(format!("Could not change it: {error}"))
                    .color(ui.visuals().error_fg_color)
                    .small(),
            );
            ui.data_mut(|d| d.insert_temp(failure_id, error.clone()));
        }
        None => ui.data_mut(|d| d.remove::<String>(failure_id)),
    }

    ui.add_space(10.0);
    // The two facts a user cannot discover from the menu itself. Both are
    // consequences of shipping registry entries rather than a COM handler
    // (D2), and neither has anywhere else to be said.
    ui.label(
        egui::RichText::new(
            "On Windows 11 this lives under \"Show more options\", or Shift+F10 — the short \
             menu is reserved for entries built with a component this program deliberately \
             does not ship.",
        )
        .weak()
        .small(),
    );
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Windows passes a selection to a menu entry as a command line, and caps it at \
             2000 characters — roughly 30 files with ordinary names. Past that Windows hides \
             the entry rather than shortening it. \"Start from this folder\" has no such \
             limit, and neither does opening the folder and selecting inside RenameIt.",
        )
        .weak()
        .small(),
    );
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Choosing a preset here loads it and shows the preview. It does not rename: a \
             right-click has nowhere to warn about a conflict or ask before something that \
             cannot be undone.",
        )
        .weak()
        .small(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use std::path::Path;

    /// A machine whose menu is not installed and cannot be: a policy has
    /// locked `HKCU\Software\Classes`. Only the three calls the page makes
    /// are answered.
    #[derive(Debug)]
    struct LockedMenu;

    impl ren_platform::Platform for LockedMenu {
        fn name(&self) -> &'static str {
            "Locked"
        }
        fn capabilities(&self) -> &'static [ren_platform::Capability] {
            &[]
        }
        fn rename(&self, _: &Path, _: &Path) -> ren_platform::Result<()> {
            unreachable!()
        }
        fn replace_file(&self, _: &Path, _: &Path) -> ren_platform::Result<()> {
            unreachable!()
        }
        fn get_attributes(&self, _: &Path) -> ren_platform::Result<ren_platform::FileAttributes> {
            unreachable!()
        }
        fn set_attributes(
            &self,
            _: &Path,
            _: ren_platform::AttributeChange,
        ) -> ren_platform::Result<()> {
            unreachable!()
        }
        fn get_times(&self, _: &Path) -> ren_platform::Result<ren_platform::FileTimes> {
            unreachable!()
        }
        fn set_times(&self, _: &Path, _: ren_platform::TimeChange) -> ren_platform::Result<()> {
            unreachable!()
        }
        fn naming_rules(&self, _: &Path) -> &'static ren_platform::NamingRules {
            &ren_platform::WINDOWS
        }
        fn case_sensitivity(&self, _: &Path) -> ren_platform::CaseSensitivity {
            unreachable!()
        }
        fn reveal_in_file_manager(&self, _: &Path) -> ren_platform::Result<()> {
            unreachable!()
        }
        fn notify_shell_changed(&self, _: &Path) {}
        fn context_menu_installed(&self) -> Option<bool> {
            Some(false)
        }
    }

    /// The failure outlives the frame of the click. It used to be a local, so
    /// the next repaint dropped it and the box just refused to stay ticked.
    #[test]
    fn a_menu_that_could_not_be_written_keeps_saying_so() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| shell_ui(ui, &LockedMenu, &[]));
        harness.run();
        harness
            .get_by_label("Show a RenameIt menu when I right-click files, folders and drives")
            .click();
        for _ in 0..3 {
            harness.run();
        }
        harness.get_by_label_contains("Could not change it:");
    }
}
