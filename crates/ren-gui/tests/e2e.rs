//! Wave 2: the controls `gui.rs` reaches through the app's own methods, now
//! reached through the widgets, plus one sweep per kind of screen.
//!
//! `docs/COVERAGE.md` maps every GUI feature and CLI flag to the test that
//! covers it; the rows that point here are the ones `gui.rs` drove by calling
//! `run_now`, `set_simulate`, `add_operation` and friends instead of clicking.
//! Everything here goes through the accessibility tree (D24), so a control
//! found by its label is also a control a screen reader can name.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use ren_core::ops::{OpKind, Replace};
use ren_gui::RenameItApp;
use tempfile::TempDir;

const MAX_STEPS: u64 = 16;

struct Fixture {
    dir: TempDir,
    journal: TempDir,
}

impl Fixture {
    fn new(names: &[&str]) -> Self {
        let dir = TempDir::new().unwrap();
        for (i, name) in names.iter().enumerate() {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("payload {i}")).unwrap();
        }
        Self {
            dir,
            journal: TempDir::new().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn app(&self) -> RenameItApp {
        RenameItApp::headless(self.path().to_path_buf(), self.journal.path().to_path_buf())
    }

    fn app_with(&self, dialogs: Picks) -> RenameItApp {
        RenameItApp::headless_with_dialogs(
            self.path().to_path_buf(),
            self.journal.path().to_path_buf(),
            Box::new(dialogs),
        )
    }
}

/// File dialogs that answer with what the test put in them.
#[derive(Debug, Default)]
struct Picks {
    folder: Option<PathBuf>,
    files: Option<Vec<PathBuf>>,
    open_preset: Option<PathBuf>,
    save_preset: Option<PathBuf>,
    csv: Option<PathBuf>,
}

impl ren_gui::dialogs::FileDialogs for Picks {
    fn pick_folder(&self, _start: &Path) -> Option<PathBuf> {
        self.folder.clone()
    }
    fn pick_files(&self) -> Option<Vec<PathBuf>> {
        self.files.clone()
    }
    fn open_preset(&self) -> Option<PathBuf> {
        self.open_preset.clone()
    }
    fn save_preset(&self, _suggested_name: &str) -> Option<PathBuf> {
        self.save_preset.clone()
    }
    fn open_csv(&self) -> Option<PathBuf> {
        self.csv.clone()
    }
}

fn harness(app: RenameItApp) -> Harness<'static, RenameItApp> {
    sized(app, 800.0)
}

fn sized(app: RenameItApp, height: f32) -> Harness<'static, RenameItApp> {
    let mut harness = Harness::builder()
        .with_max_steps(MAX_STEPS)
        .with_size(egui::vec2(1280.0, height))
        .build_ui_state(|ui, app: &mut RenameItApp| app.show(ui), app);
    settle(&mut harness);
    harness
}

fn settle(harness: &mut Harness<'_, RenameItApp>) {
    for _ in 0..3 {
        harness.state_mut().settle();
        harness.run();
    }
}

fn new_names(harness: &Harness<'_, RenameItApp>) -> Vec<String> {
    harness
        .state()
        .plan()
        .expect("a plan")
        .items
        .iter()
        .map(|item| item.new_name.clone())
        .collect()
}

fn listed(harness: &Harness<'_, RenameItApp>) -> Vec<String> {
    let mut names: Vec<String> = harness
        .state()
        .session()
        .entries()
        .iter()
        .map(|e| e.file_name.clone())
        .collect();
    names.sort();
    names
}

fn click(harness: &mut Harness<'_, RenameItApp>, label: &str) {
    harness.get_by_label(label).click();
    settle(harness);
}

/// Whether `node` is the text box a screen reader names `name`: by its label,
/// or by its hint, which egui makes the name of a box that has one.
fn named(node: &egui_kittest::Node<'_>, name: &str) -> bool {
    let ak = node.accesskit_node();
    ak.role() == Role::TextInput
        && (ak.label().as_deref() == Some(name) || ak.placeholder() == Some(name))
}

/// Types into the text box named `name` — the last one, when the source bar's
/// box and a card's share a name ("Pattern:").
fn type_into(harness: &mut Harness<'_, RenameItApp>, name: &str, text: &str) {
    let find = |harness: &Harness<'_, RenameItApp>| {
        harness
            .get_all_by_role(Role::TextInput)
            .filter(|n| named(n, name))
            .count()
    };
    assert!(find(harness) > 0, "no text box named {name:?}");
    harness
        .get_all_by_role(Role::TextInput)
        .rfind(|n| named(n, name))
        .unwrap()
        .focus();
    harness.run();
    harness
        .get_all_by_role(Role::TextInput)
        .find(|n| n.is_focused())
        .expect("the box took the keyboard")
        .type_text(text);
    settle(harness);
}

/// Adds an operation the way a user finds one: the palette, searched, clicked.
/// The list scrolls, so an entry far down is only clickable once searched for.
fn add_from_palette(harness: &mut Harness<'_, RenameItApp>, entry: &str, by_key: bool) {
    if by_key {
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
        settle(harness);
    } else {
        click(harness, "+ Add operation");
    }
    harness.get_by_label("Add operation");
    type_into(harness, "Search…", entry);
    harness.get_by_role_and_label(Role::Button, entry).click();
    settle(harness);
}

/// Replaces what the last text box matching `pick` holds — for the boxes a
/// screen reader names by their placeholder or their contents.
fn retype_where(
    harness: &mut Harness<'_, RenameItApp>,
    pick: impl Fn(&egui_kittest::Node<'_>) -> bool,
    text: &str,
) {
    harness
        .get_all_by_role(Role::TextInput)
        .rfind(|n| pick(n))
        .expect("a matching text box")
        .focus();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness
        .get_all_by_role(Role::TextInput)
        .rfind(|n| pick(n))
        .expect("the same box, focused")
        .type_text(text);
    settle(harness);
}

fn set_number(harness: &mut Harness<'_, RenameItApp>, label: &str, value: f64) {
    harness
        .get_by_role_and_label(Role::SpinButton, label)
        .set_value_accesskit(value);
    settle(harness);
}

fn present(harness: &Harness<'_, RenameItApp>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

// --- Sweep: every screen opens, names itself, and closes ---------------------

/// Every entry in the palette, clicked, adds its card and opens its editor.
/// The marker is a line only that editor draws.
#[test]
fn sweep_every_operation_in_the_palette_opens_its_editor() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let mut harness = sized(fixture.app(), 1200.0);

    let operations = [
        ("Replace", "Replace with:"),
        ("Batch Replace", "This card's own list"),
        ("Set Casing", "Sentence case"),
        ("Add / Remove", "Overwrite instead of insert"),
        ("Move Section", "relative to original pos"),
        ("Space Trimming", "Shrink multiple spaces into one"),
        ("Add Counter", "Keep current filename."),
        ("Re-Number", "Identify minus signs"),
        ("Zero Padding", "So that 1, 10, 2 sorts as 01, 02, 10."),
        ("Music Rename", "Rename music based on tags"),
        ("Music Tagger", "Quick Setup"),
        ("Remove Tags", "Remove these tags, if present:"),
        ("Free Format", "Builds the whole name from literal text"),
        ("Set Attributes", "Grey keeps the current value"),
        ("Set Date & Time", "Source:"),
        ("CSV List Rename", "Column with new filename:"),
        ("Filename Editor", "Lines may contain <tags>."),
        ("Scripting", "Choose script:"),
    ];
    for (i, (entry, marker)) in operations.iter().enumerate() {
        let before = harness.state().stack().len();
        // The button and the hotkey both open it.
        add_from_palette(&mut harness, entry, i % 2 == 1);

        assert_eq!(harness.state().stack().len(), before + 1, "{entry}");
        assert!(
            harness.query_by_label("Add operation").is_none(),
            "{entry}: the palette closes once it has added the card"
        );
        assert!(
            present(&harness, marker),
            "{entry}: its editor shows {marker:?}"
        );
    }
}

/// Every Settings page, from the rail, with a line only that page draws.
#[test]
fn sweep_every_settings_page_is_reachable_from_the_rail() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());

    harness.get_by_label("Settings").click();
    settle(&mut harness);
    let pages = [
        ("Music Styles", "+ Add style"),
        ("Casing Exceptions", "Exception words"),
        ("Display", "Shade every other row"),
        ("File System", "Hidden files and folders"),
        ("Startup", "Uncheck Subfolders"),
        (
            "Shell Integration",
            if cfg!(windows) {
                "when I right-click files, folders and drives"
            } else {
                "no context-menu entry to install"
            },
        ),
        ("Appearance", "Interface size"),
        ("Problem Solver", "Where things are kept"),
        ("Batch Replace", "rules, run top to bottom"),
    ];
    for (page, marker) in pages {
        click(&mut harness, page);
        assert!(present(&harness, marker), "{page}: {marker:?}");
    }
    click(&mut harness, "Close");
    assert!(!present(&harness, "rules, run top to bottom"));
}

/// The windows and popovers that are not Settings pages or card editors.
#[test]
fn sweep_every_window_and_popover_opens_and_closes() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());

    // About.
    click(&mut harness, "About RenameIt");
    assert!(present(&harness, "MIT licensed"));
    click(&mut harness, "Close");
    assert!(!present(&harness, "MIT licensed"));

    // The presets drawer, opened from its button and closed from its ✖.
    click(&mut harness, "Presets ▾");
    assert!(present(&harness, "No presets saved yet."));
    click(&mut harness, "✖");
    assert!(!present(&harness, "No presets saved yet."));

    // The palette, closed with Escape.
    click(&mut harness, "+ Add operation");
    harness.get_by_label("Add operation");
    harness.key_press(egui::Key::Escape);
    settle(&mut harness);
    assert!(harness.query_by_label("Add operation").is_none());

    // The two run-wide chips and the include filter open popovers.
    harness.get_by_label_contains("Counter setup").click();
    settle(&mut harness);
    assert!(present(&harness, "Reset each folder"));
    harness.key_press(egui::Key::Escape);
    settle(&mut harness);
    assert!(!present(&harness, "Reset each folder"));

    harness.get_by_label_contains("Setup parts").click();
    settle(&mut harness);
    assert!(present(&harness, "use <%1> … <%9> as tags"));
    harness.key_press(egui::Key::Escape);
    settle(&mut harness);
    assert!(!present(&harness, "use <%1> … <%9> as tags"));

    harness.get_by_label("Include filter…").click();
    settle(&mut harness);
    assert!(present(&harness, "Include only files matching"));
    harness.key_press(egui::Key::Escape);
    settle(&mut harness);
    assert!(!present(&harness, "Include only files matching"));

    // The row menu.
    harness.get_by_label_contains("one.txt").click_secondary();
    settle(&mut harness);
    assert!(present(&harness, "Show in file manager"));
    harness.key_press(egui::Key::Escape);
    settle(&mut harness);
    assert!(!present(&harness, "Show in file manager"));

    // Grid and back to List.
    click(&mut harness, "Grid");
    assert_eq!(
        harness.state().session().settings.view,
        ren_gui::viewmodel::ViewMode::Grid
    );
    click(&mut harness, "List");
    assert_eq!(
        harness.state().session().settings.view,
        ren_gui::viewmodel::ViewMode::List
    );
}

// --- The source bar ----------------------------------------------------------

/// Browser ⇄ Free Select by their switch, and Free Select's own two buttons.
#[test]
fn the_mode_switch_and_free_selects_add_and_clear() {
    let fixture = Fixture::new(&["here.txt"]);
    let elsewhere = Fixture::new(&["picked.txt"]);
    let app = fixture.app_with(Picks {
        files: Some(vec![elsewhere.path().join("picked.txt")]),
        ..Default::default()
    });
    let mut harness = harness(app);
    assert_eq!(listed(&harness), ["here.txt"]);

    click(&mut harness, "Free Select");
    assert_eq!(
        harness.state().session().settings.mode,
        ren_gui::viewmodel::SourceMode::FreeSelect
    );
    assert!(listed(&harness).is_empty());

    click(&mut harness, "Add files…");
    assert_eq!(listed(&harness), ["picked.txt"]);
    assert!(present(&harness, "1 file(s) from 1 folder(s)"));

    click(&mut harness, "Clear");
    assert!(listed(&harness).is_empty());
    assert!(
        elsewhere.path().join("picked.txt").exists(),
        "Clear empties the list, not the disk"
    );

    click(&mut harness, "Browser");
    assert_eq!(listed(&harness), ["here.txt"]);
}

/// The Files / Folders / Subfolders chips decide what the listing holds.
#[test]
fn the_include_chips_decide_what_is_listed() {
    let fixture = Fixture::new(&["top.txt", "sub/deep.txt"]);
    let mut harness = harness(fixture.app());
    assert_eq!(listed(&harness), ["top.txt"]);

    click(&mut harness, "Folders");
    assert_eq!(listed(&harness), ["sub", "top.txt"]);

    click(&mut harness, "Subfolders");
    assert_eq!(listed(&harness), ["deep.txt", "sub", "top.txt"]);

    click(&mut harness, "Files");
    assert_eq!(listed(&harness), ["sub"]);
}

/// A path typed into the address box is listed when Enter is pressed.
#[test]
fn a_path_typed_into_the_address_box_is_listed_on_enter() {
    let fixture = Fixture::new(&["here.txt"]);
    let there = Fixture::new(&["there.txt"]);
    let mut harness = harness(fixture.app());
    let here = fixture.path().display().to_string();

    retype_where(
        &mut harness,
        |n| n.value().as_deref() == Some(here.as_str()),
        &there.path().display().to_string(),
    );
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    assert_eq!(listed(&harness), ["there.txt"]);
}

/// The 📂 button opens the folder picker and lists what it returns (F12's
/// twin, which `gui.rs` drives by key).
#[test]
fn the_folder_button_lists_the_folder_it_returns() {
    let fixture = Fixture::new(&["here.txt"]);
    let there = Fixture::new(&["there.txt"]);
    let app = fixture.app_with(Picks {
        folder: Some(there.path().to_path_buf()),
        ..Default::default()
    });
    let mut harness = harness(app);

    click(&mut harness, "📂");
    assert_eq!(listed(&harness), ["there.txt"]);
}

// --- The status bar ----------------------------------------------------------

/// A whole run from the controls a user has: the Replace card typed into,
/// Simulate ticked and run, unticked and renamed, the log hidden, Undo.
#[test]
fn the_status_bar_simulates_renames_hides_the_log_and_undoes() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());

    // Find and Replace with are named by their hints, as egui names a box
    // that has one; the row labels name the boxes that have none.
    type_into(&mut harness, "text or * : ?", "o");
    type_into(&mut harness, "leave empty to delete", "0");
    assert_eq!(new_names(&harness), ["0ne.txt", "tw0.txt"]);

    harness
        .get_by_role_and_label(Role::CheckBox, "Simulate")
        .click();
    settle(&mut harness);
    click(&mut harness, "Run simulation");
    assert_eq!(
        fixture.names(),
        ["one.txt", "two.txt"],
        "a simulation writes nothing"
    );
    assert!(
        harness
            .state()
            .status()
            .unwrap_or_default()
            .contains("nothing was written"),
        "{:?}",
        harness.state().status()
    );

    harness
        .get_by_role_and_label(Role::CheckBox, "Simulate")
        .click();
    settle(&mut harness);
    click(&mut harness, "Rename");
    assert_eq!(fixture.names(), ["0ne.txt", "tw0.txt"]);

    // The log opens with the run, and its button folds it away.
    assert!(present(&harness, "one.txt  →  0ne.txt"));
    click(&mut harness, "Log");
    assert!(!present(&harness, "one.txt  →  0ne.txt"));
    click(&mut harness, "Log");
    assert!(present(&harness, "one.txt  →  0ne.txt"));

    click(&mut harness, "Undo");
    assert_eq!(fixture.names(), ["one.txt", "two.txt"]);
}

/// The All / Changed / Conflicts chips, clicked.
#[test]
fn the_row_filter_chips_narrow_the_rows() {
    let fixture = Fixture::new(&["a.txt", "c.txt", "z.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("z", "y"));
    settle(&mut harness);
    assert!(present(&harness, "c.txt"));

    click(&mut harness, "Changed");
    assert!(!present(&harness, "c.txt"), "c.txt does not change");
    assert!(present(&harness, "z.txt"));

    // c.txt → a.txt collides with a.txt; z.txt has nothing to do with it.
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("c", "a"));
    settle(&mut harness);
    click(&mut harness, "Conflicts");
    assert!(!present(&harness, "z.txt"));
    assert!(present(&harness, "a.txt"));

    click(&mut harness, "All");
    assert!(present(&harness, "z.txt"));
}

// --- The card stack ----------------------------------------------------------

/// A card's header checkbox, and its ⋮ menu's Duplicate, Move up and Delete.
#[test]
fn the_card_header_switches_a_card_off_and_its_menu_edits_the_stack() {
    let fixture = Fixture::new(&["ab.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("a", "x"));
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["xb.txt"]);

    harness
        .get_by_role_and_label(Role::CheckBox, "Replace")
        .click();
    settle(&mut harness);
    assert_eq!(
        new_names(&harness),
        ["ab.txt"],
        "a card switched off does nothing"
    );
    harness
        .get_by_role_and_label(Role::CheckBox, "Replace")
        .click();
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["xb.txt"]);

    harness.get_by_label("Reorder, duplicate or delete").click();
    settle(&mut harness);
    click(&mut harness, "Duplicate");
    assert_eq!(harness.state().stack().len(), 2);

    // Make the copy different, so its position can be seen.
    if let OpKind::Replace(op) = &mut harness.state_mut().stack_mut().get_mut(1).unwrap().op {
        *op = Replace::new("x", "y");
    }
    harness.state_mut().expand_card(0);
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["yb.txt"], "a→x, then x→y");

    harness
        .get_all_by_label("Reorder, duplicate or delete")
        .nth(1)
        .unwrap()
        .click();
    settle(&mut harness);
    click(&mut harness, "Move up");
    assert_eq!(new_names(&harness), ["xb.txt"], "x→y runs first, then a→x");

    harness
        .get_all_by_label("Reorder, duplicate or delete")
        .next()
        .unwrap()
        .click();
    settle(&mut harness);
    click(&mut harness, "Delete");
    assert_eq!(harness.state().stack().len(), 1);
    assert_eq!(new_names(&harness), ["xb.txt"]);
}

// --- The editors, through their own fields -----------------------------------

#[test]
fn the_add_remove_editor_inserts_at_the_position_typed() {
    let fixture = Fixture::new(&["ab.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_from_palette(&mut harness, "Add / Remove", false);

    type_into(&mut harness, "Insert:", "X");
    assert_eq!(new_names(&harness), ["Xab.txt"]);
    set_number(&mut harness, "at pos:", 1.0);
    assert_eq!(new_names(&harness), ["aXb.txt"]);
    harness
        .get_by_role_and_label(Role::CheckBox, "Overwrite instead of insert")
        .click();
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["aX.txt"]);
}

#[test]
fn the_casing_editor_applies_the_mode_clicked() {
    let fixture = Fixture::new(&["My File.TXT"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_from_palette(&mut harness, "Set Casing", false);

    for (mode, expected) in [
        ("UPPER CASE", "MY FILE.TXT"),
        ("lower case", "my file.TXT"),
        ("iNVERT", "mY fILE.TXT"),
    ] {
        harness
            .get_by_role_and_label(Role::RadioButton, mode)
            .click();
        settle(&mut harness);
        assert_eq!(new_names(&harness), [expected], "{mode}");
    }
}

#[test]
fn the_zero_padding_editor_pads_to_the_digits_given() {
    let fixture = Fixture::new(&["track 7.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_from_palette(&mut harness, "Zero Padding", false);
    assert_eq!(new_names(&harness), ["track 07.txt"]);

    set_number(&mut harness, "Digits:", 3.0);
    assert_eq!(new_names(&harness), ["track 007.txt"]);
}

#[test]
fn the_add_counter_editor_moves_the_counter_and_its_separator() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_from_palette(&mut harness, "Add Counter", false);
    assert_eq!(new_names(&harness), ["1-a.txt"]);

    harness
        .get_by_role_and_label(Role::RadioButton, "Last")
        .click();
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["a-1.txt"]);

    // The separator box is named by nothing a query can use; it holds "-".
    retype_where(&mut harness, |n| n.value().as_deref() == Some("-"), "_");
    assert_eq!(new_names(&harness), ["a_1.txt"]);
}

#[test]
fn the_free_format_editor_builds_the_name_from_its_pattern() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_from_palette(&mut harness, "Free Format", false);

    type_into(&mut harness, "Pattern:", "new_<Name>.md");
    assert_eq!(new_names(&harness), ["new_a.md"]);
}

/// The CSV card's folder button opens the picker, and the list it returns
/// renames.
#[test]
fn the_csv_editor_renames_from_the_list_it_was_given() {
    let fixture = Fixture::new(&["lorem.txt", "other.txt"]);
    let lists = TempDir::new().unwrap();
    let csv = lists.path().join("list.csv");
    std::fs::write(&csv, "Old,New\nlorem,Some\n").unwrap();
    let app = fixture.app_with(Picks {
        csv: Some(csv),
        ..Default::default()
    });
    let mut harness = sized(app, 1000.0);
    add_from_palette(&mut harness, "CSV List Rename", false);

    click(&mut harness, "Choose a CSV file");
    assert_eq!(new_names(&harness), ["Some.txt", "other.txt"]);
}

// --- Run-wide settings -------------------------------------------------------

/// Counter setup's own boxes, typed into, rather than `settings_mut`.
#[test]
fn the_counter_setup_popover_drives_the_counter() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_from_palette(&mut harness, "Add Counter", false);

    harness.get_by_label_contains("Counter setup").click();
    settle(&mut harness);
    set_number(&mut harness, "Start value:", 10.0);
    set_number(&mut harness, "Step:", 5.0);
    assert_eq!(new_names(&harness), ["10-a.txt", "15-b.txt", "20-c.txt"]);

    harness
        .get_by_role_and_label(Role::CheckBox, "Reset at:")
        .click();
    settle(&mut harness);
    set_number(&mut harness, "Reset at:", 15.0);
    assert_eq!(new_names(&harness), ["10-a.txt", "15-b.txt", "10-c.txt"]);
}

// --- The presets drawer, through its own buttons ------------------------------

#[test]
fn the_presets_drawer_saves_renames_duplicates_exports_deletes_and_imports() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let outbox = TempDir::new().unwrap();
    let exported = outbox.path().join("shared.toml");
    let app = fixture.app_with(Picks {
        save_preset: Some(exported.clone()),
        open_preset: Some(exported.clone()),
        ..Default::default()
    });
    let mut harness = sized(app, 1000.0);
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    settle(&mut harness);

    click(&mut harness, "Presets ▾");
    retype_where(
        &mut harness,
        |n| n.accesskit_node().placeholder() == Some("Name this pipeline"),
        "Tidy",
    );
    click(&mut harness, "Save as preset");
    assert_eq!(harness.state().preset_names(), ["Tidy"]);

    let kebab = "Rename, duplicate, export or delete this preset";
    let open_first_menu = |harness: &mut Harness<'_, RenameItApp>| {
        harness.get_all_by_label(kebab).next().unwrap().click();
        settle(harness);
    };

    open_first_menu(&mut harness);
    click(&mut harness, "Rename…");
    // The row's own box, below the name box that also still says "Tidy".
    retype_where(
        &mut harness,
        |n| n.value().as_deref() == Some("Tidy"),
        "Neat",
    );
    // Two buttons say Rename: this row's, and the window's run button below.
    harness
        .get_all_by_role_and_label(Role::Button, "Rename")
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .click();
    settle(&mut harness);
    assert_eq!(harness.state().preset_names(), ["Neat"]);

    open_first_menu(&mut harness);
    click(&mut harness, "Duplicate");
    assert_eq!(harness.state().preset_names().len(), 2);

    open_first_menu(&mut harness);
    click(&mut harness, "Export…");
    assert!(
        exported.exists(),
        "{}",
        harness.state().status().unwrap_or_default()
    );

    // Delete asks in the row: Keep leaves it, Delete removes it.
    open_first_menu(&mut harness);
    click(&mut harness, "Delete");
    click(&mut harness, "Keep");
    assert_eq!(harness.state().preset_names().len(), 2);
    open_first_menu(&mut harness);
    click(&mut harness, "Delete");
    click(&mut harness, "Delete");
    assert_eq!(harness.state().preset_names().len(), 1);

    // The exported file comes back in.
    click(&mut harness, "Import…");
    assert_eq!(
        harness.state().preset_names().len(),
        2,
        "{:?} {:?}",
        harness.state().preset_names(),
        harness.state().status()
    );
}

/// Append and Run, clicked.
#[test]
fn the_presets_drawer_appends_and_runs() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    harness.state_mut().save_preset("Spaces");
    settle(&mut harness);

    click(&mut harness, "Presets ▾");
    click(&mut harness, "Append");
    assert_eq!(
        harness.state().stack().len(),
        2,
        "its card follows the one there"
    );

    click(&mut harness, "Run");
    assert_eq!(fixture.names(), ["a b.txt"]);
}

// --- Recovery, the row menu's reveal, Settings switches -----------------------

/// "Leave as is" folds the recovery banner away and touches nothing.
#[test]
fn the_recovery_banner_can_be_left_as_it_is() {
    use ren_core::exec::{Journal, Record};

    let fixture = Fixture::new(&["b.txt"]);
    let mut journal = Journal::create(fixture.journal.path()).unwrap();
    journal
        .write(Record::Begin {
            platform: "test".into(),
            items: 1,
        })
        .unwrap();
    journal
        .write(Record::PlanRename {
            seq: 0,
            from: fixture.path().join("a.txt"),
            to: fixture.path().join("b.txt"),
        })
        .unwrap();
    drop(journal);

    let mut harness = harness(fixture.app());
    assert!(present(&harness, "did not finish"));
    click(&mut harness, "Leave as is");
    assert!(!present(&harness, "did not finish"));
    assert_eq!(fixture.names(), ["b.txt"]);
}

/// The host platform, recording what it was asked to show.
#[derive(Debug)]
struct RevealSpy {
    inner: Arc<dyn ren_platform::Platform>,
    revealed: Mutex<Vec<PathBuf>>,
}

impl ren_platform::Platform for RevealSpy {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn capabilities(&self) -> &'static [ren_platform::Capability] {
        self.inner.capabilities()
    }
    fn rename(&self, from: &Path, to: &Path) -> ren_platform::Result<()> {
        self.inner.rename(from, to)
    }
    fn replace_file(&self, temp: &Path, target: &Path) -> ren_platform::Result<()> {
        self.inner.replace_file(temp, target)
    }
    fn get_attributes(&self, path: &Path) -> ren_platform::Result<ren_platform::FileAttributes> {
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
    fn set_times(&self, path: &Path, change: ren_platform::TimeChange) -> ren_platform::Result<()> {
        self.inner.set_times(path, change)
    }
    fn naming_rules(&self, path: &Path) -> &'static ren_platform::NamingRules {
        self.inner.naming_rules(path)
    }
    fn is_system_folder(&self, path: &Path) -> bool {
        self.inner.is_system_folder(path)
    }
    fn case_sensitivity(&self, dir: &Path) -> ren_platform::CaseSensitivity {
        self.inner.case_sensitivity(dir)
    }
    fn reveal_in_file_manager(&self, path: &Path) -> ren_platform::Result<()> {
        self.revealed.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
    fn notify_shell_changed(&self, path: &Path) {
        self.inner.notify_shell_changed(path);
    }
}

/// The row menu's Show in file manager and the Problem Solver's 📂 both ask the
/// platform to show a path — the file, and the journal folder.
#[test]
fn show_in_file_manager_reveals_the_row_and_the_problem_solver_its_folders() {
    let fixture = Fixture::new(&["one.txt"]);
    let spy = Arc::new(RevealSpy {
        inner: ren_platform::host(),
        revealed: Mutex::new(Vec::new()),
    });
    let app = RenameItApp::headless_with_platform(
        fixture.path().to_path_buf(),
        fixture.journal.path().to_path_buf(),
        spy.clone(),
    );
    let mut harness = harness(app);

    harness.get_by_label_contains("one.txt").click_secondary();
    settle(&mut harness);
    click(&mut harness, "Show in file manager");
    assert_eq!(
        spy.revealed.lock().unwrap().as_slice(),
        [fixture.path().join("one.txt")]
    );

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    click(&mut harness, "Problem Solver");
    // Two kinds of 📂: the source bar's folder picker at the top, then the
    // page's three, the journal's first.
    let mut folders: Vec<_> = harness.get_all_by_label("📂").collect();
    folders.sort_by(|a, b| a.rect().top().total_cmp(&b.rect().top()));
    folders[1].click();
    drop(folders);
    settle(&mut harness);
    assert_eq!(
        spy.revealed.lock().unwrap().last(),
        Some(&fixture.journal.path().to_path_buf())
    );
}

/// Settings ▸ File System: the hidden-files switch and the pattern scope.
#[test]
fn the_file_system_page_changes_what_is_listed() {
    let fixture = Fixture::new(&["seen.txt", ".hidden.dat", "locked.txt", "folder/inner.txt"]);
    let locked = fixture.path().join("locked.txt");
    let set_readonly = |on: bool| {
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_readonly(on);
        std::fs::set_permissions(&locked, perms).unwrap();
    };
    set_readonly(true);
    let mut harness = harness(fixture.app());
    // Everything is shown by default (D126).
    assert_eq!(listed(&harness), [".hidden.dat", "locked.txt", "seen.txt"]);

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    click(&mut harness, "File System");
    harness
        .get_by_role_and_label(Role::CheckBox, "Write-protected files and folders")
        .click();
    settle(&mut harness);
    assert_eq!(listed(&harness), [".hidden.dat", "seen.txt"]);
    harness
        .get_by_role_and_label(Role::CheckBox, "Hidden files and folders")
        .click();
    settle(&mut harness);
    if cfg!(unix) {
        // A leading dot is what hidden means on Unix; on Windows an attribute.
        assert_eq!(listed(&harness), ["seen.txt"]);
    }

    // The pattern applies to Files only, so a folder that does not match it
    // stays listed beside the files that do. Two chips say "Files": the
    // source bar's, at the top, and this page's.
    harness
        .get_all_by_label("Files")
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .click();
    settle(&mut harness);
    click(&mut harness, "Close");
    click(&mut harness, "Folders");
    type_into(&mut harness, "Pattern:", "*.txt");
    assert_eq!(listed(&harness), ["folder", "seen.txt"]);
    set_readonly(false);
}

/// Settings ▸ Display: the thumbnail border and row stripes are stored.
#[test]
fn the_display_page_stores_its_switches() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    click(&mut harness, "Display");

    let stripes = harness.state().table_style().stripes;
    harness
        .get_by_role_and_label(Role::CheckBox, "Shade every other row")
        .click();
    settle(&mut harness);
    assert_eq!(harness.state().table_style().stripes, !stripes);

    let border = harness.state().session().settings.thumb_border;
    harness
        .get_by_role_and_label(Role::CheckBox, "Draw a border around thumbnails")
        .click();
    settle(&mut harness);
    assert_eq!(harness.state().session().settings.thumb_border, !border);

    // A column turned on, then Restore defaults: the table is back to four.
    harness.get_all_by_label("Ext").next().unwrap().click();
    settle(&mut harness);
    click(&mut harness, "Restore defaults");
    click(&mut harness, "Close");
    assert!(harness.query_by_label("Ext").is_none(), "Ext is off again");
}

/// A folder the walk cannot read is counted beside the path, not hidden.
#[cfg(unix)]
#[test]
fn an_unreadable_subfolder_is_counted_beside_the_path() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new(&["top.txt", "locked/inner.txt"]);
    let locked = fixture.path().join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads it anyway; there is nothing to report then.
    let unreadable = std::fs::read_dir(&locked).is_err();

    let mut harness = harness(fixture.app());
    click(&mut harness, "Subfolders");
    let counted = present(&harness, "1 item(s) could not be read");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(counted, unreadable);
}

/// Home and End jump to the ends of the list.
#[test]
fn home_and_end_jump_to_the_ends_of_the_list() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = harness(fixture.app());
    harness.key_press(egui::Key::End);
    settle(&mut harness);
    assert_eq!(harness.state().session().selection.lead, Some(2));
    harness.key_press(egui::Key::Home);
    settle(&mut harness);
    assert_eq!(harness.state().session().selection.lead, Some(0));
}

#[test]
fn the_space_trimming_switches_reach_the_preview() {
    let fixture = Fixture::new(&["  a  b.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_from_palette(&mut harness, "Space Trimming", false);
    assert_eq!(new_names(&harness), ["a b.txt"]);

    harness
        .get_by_role_and_label(Role::CheckBox, "Shrink multiple spaces into one")
        .click();
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["a  b.txt"]);

    harness
        .get_by_role_and_label(Role::CheckBox, "Remove leading spaces")
        .click();
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["  a  b.txt"]);
}

/// The command-line-limit banner's Dismiss keeps the selection it was given.
#[test]
fn the_selection_limit_banner_can_be_dismissed() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut app = fixture.app();
    app.start_from(&ren_gui::launch::Launch {
        paths: vec![fixture.path().join("one.txt")],
        from_shell: true,
        command_line_chars: Some(1_900),
        ..Default::default()
    });
    let mut harness = harness(app);
    assert!(present(&harness, "2000 characters"));

    click(&mut harness, "Dismiss");
    assert!(!present(&harness, "2000 characters"));
    assert_eq!(listed(&harness), ["one.txt"], "still the selection");
}

/// The row menu's Rename… opens the same inline editor F2 does.
#[test]
fn the_row_menus_rename_opens_the_inline_editor() {
    let fixture = Fixture::new(&["song.mp3", "other.mp3"]);
    let mut harness = harness(fixture.app());

    harness.get_by_label_contains("song.mp3").click_secondary();
    settle(&mut harness);
    click(&mut harness, "Rename…");
    harness
        .get_all_by_role(Role::TextInput)
        .find(|n| n.value().as_deref() == Some("song.mp3"))
        .expect("the inline editor")
        .type_text("track");
    settle(&mut harness);
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    assert_eq!(fixture.names(), ["other.mp3", "track.mp3"]);
}
