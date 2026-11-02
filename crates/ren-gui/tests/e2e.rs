//! Wave 2: the controls `gui.rs` reaches through the app's own methods, now
//! reached through the widgets, plus one sweep per kind of screen.
//!
//! `docs/COVERAGE.md` maps every GUI feature and CLI flag to the test that
//! covers it; the rows that point here are the ones `gui.rs` drove by calling
//! `run_now`, `set_simulate`, `add_operation` and friends instead of clicking.
//! Everything here goes through the accessibility tree (D24), so a control
//! found by its label is also a control a screen reader can name.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use ren_core::ops::{OpKind, Replace};
use ren_gui::RenameItApp;
use ren_gui::viewmodel::update::{self, Version};
use ren_platform::update::{HttpHead, InstallRecord};
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

/// The run button's labels (D251): "Rename 3 files", "Run on 1 file", or a
/// bare "Rename" or "Run" — and not the row menu's "Rename…" or the presets
/// drawer's own Rename.
fn is_run_label(label: &str) -> bool {
    let counted = |verb: &str| {
        label
            .strip_prefix(verb)
            .and_then(|rest| rest.split(' ').next())
            .is_some_and(|n| n.parse::<usize>().is_ok())
    };
    label == "Rename" || label == "Run" || counted("Rename ") || counted("Run on ")
}

/// Settings, from the ⚙ at the window's top right (D255), on rail entry
/// `page` unless that is "Settings" itself.
fn menu(harness: &mut Harness<'_, RenameItApp>, page: &str) {
    click(harness, "Settings");
    if page != "Settings" {
        click(harness, page);
    }
}

/// The run button under the pipeline, whose label carries the count
/// ("Rename 3 files", D251).
fn rename_button<'h>(harness: &'h Harness<'_, RenameItApp>) -> egui_kittest::Node<'h> {
    harness
        .get_all_by_role(Role::Button)
        .find(|n| n.accesskit_node().label().is_some_and(|l| is_run_label(&l)))
        .expect("the Rename button")
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

/// Adds an operation the way a user does: `+ Add operation` (or Ctrl+K) makes
/// a default card, and its type drop-down turns it into `entry`.
fn add_operation_of(harness: &mut Harness<'_, RenameItApp>, entry: &str, by_key: bool) {
    if by_key {
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
        settle(harness);
    } else {
        click(harness, "+ Add operation");
    }
    // The new card is the last, and it is the default operation's.
    harness
        .get_all_by_role(Role::ComboBox)
        .rfind(|n| n.value().as_deref() == Some("Replace"))
        .expect("the new card's type drop-down")
        .click();
    settle(harness);
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

/// Every operation, added and picked from the card's drop-down, opens its
/// editor.
/// The marker is a line only that editor draws.
#[test]
fn sweep_every_operation_in_the_drop_down_opens_its_editor() {
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
        add_operation_of(&mut harness, entry, i % 2 == 1);

        assert_eq!(harness.state().stack().len(), before + 1, "{entry}");
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

    menu(&mut harness, "Settings");
    let pages = [
        ("Music styles", "+ Add style"),
        ("Casing words", "Exception words"),
        ("Columns & rows", "Shade every other row"),
        ("Thumbnails", "Draw a border around thumbnails"),
        ("Which files show", "Hidden files and folders"),
        ("When RenameIt starts", "Uncheck Subfolders"),
        ("Default operation", "New cards start as:"),
        (
            "Right-click menu",
            if cfg!(windows) {
                "when I right-click files, folders and drives"
            } else {
                "no context-menu entry to install"
            },
        ),
        ("Appearance", "Interface size"),
        ("Updates", "ask GitHub whether there is a newer release"),
        ("Where files are kept", "Reset all settings…"),
        ("Help", "If something looks wrong"),
        ("About", "MIT licensed"),
        ("Batch Replace list", "rules, run top to bottom"),
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

    // About, the last page of Settings.
    menu(&mut harness, "About");
    assert!(present(&harness, "MIT licensed"));
    click(&mut harness, "Close");
    assert!(!present(&harness, "MIT licensed"));

    // The presets drawer, opened from its button and closed from its ✖.
    click(&mut harness, "Presets ▾");
    assert!(present(&harness, "No presets saved yet."));
    click(&mut harness, "✖");
    assert!(!present(&harness, "No presets saved yet."));

    // The include filter opens a popover.
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
fn the_run_controls_simulate_rename_fold_the_log_and_undo() {
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
    // The button says what it will do, count included (D251).
    click(&mut harness, "Simulate 2 files");
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
    click(&mut harness, "Rename 2 files");
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
    add_operation_of(&mut harness, "Add / Remove", false);

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
    add_operation_of(&mut harness, "Set Casing", false);

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
    add_operation_of(&mut harness, "Zero Padding", false);
    assert_eq!(new_names(&harness), ["track 07.txt"]);

    set_number(&mut harness, "Digits:", 3.0);
    assert_eq!(new_names(&harness), ["track 007.txt"]);
}

#[test]
fn the_add_counter_editor_moves_the_counter_and_its_separator() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_operation_of(&mut harness, "Add Counter", false);
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
    add_operation_of(&mut harness, "Free Format", false);

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
    add_operation_of(&mut harness, "CSV List Rename", false);

    click(&mut harness, "Choose a CSV file");
    assert_eq!(new_names(&harness), ["Some.txt", "other.txt"]);
}

// --- Run-wide settings -------------------------------------------------------

/// The counter's own boxes in the Add Counter card (D254), typed into, rather
/// than `settings_mut`.
#[test]
fn the_add_counter_cards_counter_boxes_drive_the_counter() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_operation_of(&mut harness, "Add Counter", false);

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

/// "Only rename if all tags are available", ticked: a file whose tags cannot
/// all be filled in keeps its name rather than one with a gap in it.
#[test]
fn only_if_all_tags_are_available_leaves_an_untagged_file_alone() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    add_operation_of(&mut harness, "Free Format", false);
    type_into(&mut harness, "Pattern:", "<Name> by <Artist>.txt");
    assert_eq!(new_names(&harness), ["a by .txt"]);

    // In the Free Format card, because <Artist> reads the file.
    harness
        .get_by_role_and_label(Role::CheckBox, "Only rename if all tags are available")
        .click();
    settle(&mut harness);
    assert!(harness.state().settings().require_all_tags);
    assert_eq!(new_names(&harness), ["a.txt"]);
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

/// `(open, reached)`: the first rename sets `reached` and waits for `open`.
type Gate = Arc<(Mutex<(bool, bool)>, Condvar)>;

/// The host platform, recording what it was asked to show and, given a gate,
/// holding its first rename there.
#[derive(Debug)]
struct Spy {
    inner: Arc<dyn ren_platform::Platform>,
    revealed: Mutex<Vec<PathBuf>>,
    gate: Option<Gate>,
    /// Serves a release one patch newer than this build, as GitHub would:
    /// the `releases/latest` redirect, the checksum list and the installer.
    /// `Some(None)` is a copy that was not installed.
    release: Option<Option<InstallRecord>>,
    /// Every installer started, with its arguments.
    launched: Mutex<Vec<Vec<String>>>,
    /// Every file opened with its default program.
    opened: Mutex<Vec<PathBuf>>,
}

impl Spy {
    fn new(gate: Option<Gate>) -> Arc<Self> {
        Arc::new(Self {
            inner: ren_platform::host(),
            revealed: Mutex::default(),
            gate,
            release: None,
            launched: Mutex::default(),
            opened: Mutex::default(),
        })
    }

    fn with_release(install: Option<InstallRecord>) -> Arc<Self> {
        Arc::new(Self {
            release: Some(install),
            ..Arc::into_inner(Self::new(None)).unwrap()
        })
    }

    fn newer() -> Version {
        let Version(major, minor, patch) = Version::current();
        Version(major, minor, patch + 1)
    }
}

impl ren_platform::Platform for Spy {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn capabilities(&self) -> &'static [ren_platform::Capability] {
        if self.release.is_some() {
            &ren_platform::Capability::ALL
        } else {
            self.inner.capabilities()
        }
    }
    fn http_get(
        &self,
        url: &str,
        _follow_redirects: bool,
        body: &mut dyn std::io::Write,
    ) -> ren_platform::Result<HttpHead> {
        let newer = Self::newer();
        let (status, location) = if url == update::latest_url() {
            (302, Some(update::release_page(newer)))
        } else if url == update::asset_url(newer, update::SUMS) {
            let line = format!("{}  {}\n", "0".repeat(64), update::installer_asset(newer));
            body.write_all(line.as_bytes()).unwrap();
            (200, None)
        } else if url == update::asset_url(newer, &update::installer_asset(newer)) {
            body.write_all(b"MZ").unwrap();
            (200, None)
        } else {
            (404, None)
        };
        Ok(HttpHead { status, location })
    }
    /// The digest the checksum list above claims; whether a mismatch is
    /// refused is `viewmodel::update`'s own test.
    fn sha256_file(&self, _path: &Path) -> ren_platform::Result<[u8; 32]> {
        Ok([0; 32])
    }
    fn install_record(&self) -> Option<InstallRecord> {
        self.release.flatten()
    }
    fn launch_installer(
        &self,
        _installer: &Path,
        args: &[&str],
        _elevate: bool,
    ) -> ren_platform::Result<()> {
        let args = args.iter().map(|a| (*a).to_owned()).collect();
        self.launched.lock().unwrap().push(args);
        Ok(())
    }
    fn rename(&self, from: &Path, to: &Path) -> ren_platform::Result<()> {
        if let Some(gate) = &self.gate {
            let (state, signal) = &**gate;
            let mut state = state.lock().unwrap();
            if !state.1 {
                state.1 = true;
                signal.notify_all();
                drop(signal.wait_while(state, |s| !s.0).unwrap());
            }
        }
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
    fn open_file(&self, path: &Path) -> ren_platform::Result<()> {
        self.opened.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
    fn notify_shell_changed(&self, path: &Path) {
        self.inner.notify_shell_changed(path);
    }
}

/// The row menu's Show in file manager and Where files are kept's 📂 both ask the
/// platform to show a path — the file, and the journal folder.
#[test]
fn show_in_file_manager_reveals_the_row_and_the_problem_solver_its_folders() {
    let fixture = Fixture::new(&["one.txt"]);
    let spy = Spy::new(None);
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
    click(&mut harness, "Where files are kept");
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

// --- The pointer on a row, Explorer's way (P115) -----------------------------

/// Frames 50 ms apart, so two clicks queued back to back land inside the
/// double-click time. At kittest's default 250 ms a frame, every event is its
/// own frame and two clicks are always a slow click.
fn quick(app: RenameItApp) -> Harness<'static, RenameItApp> {
    let mut harness = Harness::builder()
        .with_max_steps(MAX_STEPS)
        .with_step_dt(0.05)
        .with_size(egui::vec2(1280.0, 800.0))
        .build_ui_state(|ui, app: &mut RenameItApp| app.show(ui), app);
    settle(&mut harness);
    harness
}

fn double_click_at(harness: &mut Harness<'_, RenameItApp>, pos: egui::Pos2) {
    harness.hover_at(pos);
    for pressed in [true, false, true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
    }
    settle(harness);
}

fn double_click(harness: &mut Harness<'_, RenameItApp>, label: &str) {
    let at = harness.get_by_label(label).rect().center();
    double_click_at(harness, at);
}

/// The inline editor, if it is open on `name`.
fn editing(harness: &Harness<'_, RenameItApp>, name: &str) -> bool {
    harness
        .query_all_by_role(Role::TextInput)
        .any(|n| n.value().as_deref() == Some(name))
}

/// Opens the drop-down a screen reader names `combo` and picks `choice`.
fn pick(harness: &mut Harness<'_, RenameItApp>, combo: &str, choice: &str) {
    harness.get_by_role_and_label(Role::ComboBox, combo).click();
    settle(harness);
    click(harness, choice);
}

fn with_a_folder(fixture: &Fixture) -> RenameItApp {
    let mut app = fixture.app();
    app.session_mut().settings.folders = true;
    app.session_mut().request_refresh();
    app
}

#[test]
fn a_quick_double_click_opens_a_folder_in_the_list_and_the_grid() {
    let fixture = Fixture::new(&["a.txt", "sub/inside.txt"]);
    let mut harness = quick(with_a_folder(&fixture));
    double_click(&mut harness, "📁 sub");
    assert!(harness.state().session().settings.dir.ends_with("sub"));
    harness.get_by_label("inside.txt");
    assert!(!editing(&harness, "sub"), "opened, not renamed");

    let mut harness = quick(with_a_folder(&fixture));
    click(&mut harness, "Grid");
    // The picture carries no name of its own (P72): aim above the caption.
    let caption = harness.get_by_label("sub").rect();
    double_click_at(
        &mut harness,
        egui::pos2(caption.center().x, caption.top() - 30.0),
    );
    assert!(harness.state().session().settings.dir.ends_with("sub"));
}

/// `..` above the rows goes up a folder on a double-click, in the list and in
/// the grid, and lands on the folder it left — Backspace's behaviour.
#[test]
fn a_double_click_on_dot_dot_goes_up_in_the_list_and_the_grid() {
    let fixture = Fixture::new(&["a.txt", "sub/inside.txt"]);
    for grid in [false, true] {
        let mut app = with_a_folder(&fixture);
        app.set_dir(fixture.dir.path().join("sub"));
        let mut harness = quick(app);
        if grid {
            click(&mut harness, "Grid");
        }
        double_click(&mut harness, "📁 ..");
        assert_eq!(
            harness.state().session().settings.dir,
            fixture.dir.path(),
            "grid: {grid}"
        );
        let session = harness.state().session();
        let landed = session
            .selection
            .lead
            .map(|i| &session.entries()[i].file_name);
        assert_eq!(landed.map(String::as_str), Some("sub"), "grid: {grid}");
    }
}

/// No `..` where there is nowhere to go: Free Select has no folder.
#[test]
fn free_select_has_no_dot_dot() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut app = fixture.app();
    app.session_mut().settings.mode = ren_gui::viewmodel::SourceMode::FreeSelect;
    let harness = quick(app);
    assert!(harness.query_by_label("📁 ..").is_none());
}

/// Explorer's slow second click: a click on the row that is already the only
/// one selected opens the editor once the double-click time has passed.
#[test]
fn a_slow_second_click_on_the_selected_row_renames_it() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let mut first = harness(fixture.app());
    click(&mut first, "a.txt");
    assert!(!editing(&first, "a.txt"), "the first click only selects");
    click(&mut first, "a.txt");
    assert!(editing(&first, "a.txt"));

    // A Ctrl-click is a selection edit, never a rename.
    let mut again = harness(fixture.app());
    click(&mut again, "b.txt");
    again
        .get_by_label("b.txt")
        .click_modifiers(egui::Modifiers::COMMAND);
    settle(&mut again);
    assert!(!editing(&again, "b.txt"));
}

/// A quick double-click on a file renames it by default; Settings ▸ Display
/// can make it open the file with its default program instead.
#[test]
fn a_quick_double_click_on_a_file_does_what_the_display_page_says() {
    let fixture = Fixture::new(&["a.txt"]);
    let spy = Spy::new(None);
    let app = RenameItApp::headless_with_platform(
        fixture.path().to_path_buf(),
        fixture.journal.path().to_path_buf(),
        spy.clone(),
    );
    let mut harness = quick(app);
    double_click(&mut harness, "a.txt");
    assert!(editing(&harness, "a.txt"), "Rename it, the default");
    harness.key_press(egui::Key::Escape);
    settle(&mut harness);

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    click(&mut harness, "Columns & rows");
    pick(&mut harness, "Double-click on a file", "Open it");
    click(&mut harness, "Close");

    double_click(&mut harness, "a.txt");
    assert_eq!(
        spy.opened.lock().unwrap().as_slice(),
        [fixture.path().join("a.txt")]
    );
    assert!(!editing(&harness, "a.txt"));
}

/// Free Select has no folder of its own: a double-click on a folder there does
/// nothing, unless the Display page says to browse into it.
#[test]
fn a_folder_in_free_select_opens_only_if_the_display_page_says_so() {
    let fixture = Fixture::new(&["a.txt", "sub/inside.txt"]);
    let mut app = fixture.app();
    app.session_mut().add_to_free_select(vec![
        fixture.path().join("sub"),
        fixture.path().join("a.txt"),
    ]);
    let mut harness = quick(app);
    double_click(&mut harness, "📁 sub");
    assert_eq!(
        harness.state().session().settings.mode,
        ren_gui::viewmodel::SourceMode::FreeSelect,
        "Do nothing, the default"
    );

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    click(&mut harness, "Columns & rows");
    pick(
        &mut harness,
        "Double-click on a folder in Free Select",
        "Browse into it",
    );
    click(&mut harness, "Close");

    double_click(&mut harness, "📁 sub");
    let session = harness.state().session();
    assert_eq!(
        session.settings.mode,
        ren_gui::viewmodel::SourceMode::Browser
    );
    assert!(session.settings.dir.ends_with("sub"));
}

/// D252: the folder being browsed opens in the file manager from the source
/// bar — the folder itself, which only a row's menu reached before, one file
/// at a time.
#[test]
fn the_folder_being_browsed_opens_in_the_file_manager() {
    let fixture = Fixture::new(&["one.txt"]);
    let spy = Spy::new(None);
    let app = RenameItApp::headless_with_platform(
        fixture.path().to_path_buf(),
        fixture.journal.path().to_path_buf(),
        spy.clone(),
    );
    let mut harness = harness(app);
    click(&mut harness, "Open in file manager");
    assert_eq!(
        spy.revealed.lock().unwrap().as_slice(),
        [fixture.path().to_path_buf()]
    );
}

/// D255: the ⚙ opens Settings directly. There is no ⋯ menu any more, and
/// About is the rail's last entry.
#[test]
fn the_gear_opens_settings_and_about_is_its_last_page() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    assert!(harness.query_by_label("Menu").is_none());

    click(&mut harness, "Settings");
    assert!(present(&harness, "Interface size"), "opens on Appearance");
    let last = harness
        .get_all_by_role(Role::Button)
        .filter(|n| {
            n.accesskit_node()
                .label()
                .is_some_and(|l| l == "About" || l == "Help" || l == "Appearance")
        })
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .and_then(|n| n.accesskit_node().label());
    assert_eq!(last.as_deref(), Some("About"));
}

/// Settings ▸ Updates on an installed copy: Check now finds the newer release,
/// and Update to X downloads it, checks it, and starts the installer silently
/// in the installed scope with /RESTART — after which the window closes.
#[test]
fn update_to_a_newer_release_hands_over_to_the_installer() {
    let fixture = Fixture::new(&["one.txt"]);
    let spy = Spy::with_release(Some(InstallRecord {
        per_machine: false,
        context_menu: false,
    }));
    let app = RenameItApp::headless_with_platform(
        fixture.path().to_path_buf(),
        fixture.journal.path().to_path_buf(),
        spy.clone(),
    );
    let mut harness = harness(app);
    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    click(&mut harness, "Updates");
    assert!(present(&harness, "Only when I ask"));
    assert!(spy.launched.lock().unwrap().is_empty(), "nothing by itself");

    click(&mut harness, "Check now");
    let newer = Spy::newer();
    assert!(present(&harness, &format!("RenameIt {newer} is available")));
    click(&mut harness, &format!("Update to {newer}"));
    assert_eq!(
        spy.launched.lock().unwrap().as_slice(),
        [vec!["/S", "/CurrentUser", "/RESTART"]]
    );
}

/// A copy the installer did not put there is told about the release and
/// pointed at its page, and offered no Update button and no Automatic mode.
#[test]
fn a_portable_copy_is_pointed_at_the_download_page() {
    let fixture = Fixture::new(&["one.txt"]);
    let spy = Spy::with_release(None);
    let app = RenameItApp::headless_with_platform(
        fixture.path().to_path_buf(),
        fixture.journal.path().to_path_buf(),
        spy.clone(),
    );
    let mut harness = harness(app);
    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    click(&mut harness, "Updates");
    click(&mut harness, "Check now");
    assert!(present(&harness, "Open download page"));
    assert!(!present(&harness, "Update to"));
    assert!(
        harness
            .get_by_role_and_label(
                Role::RadioButton,
                "Once a day — install it when I close RenameIt"
            )
            .accesskit_node()
            .is_disabled(),
        "Automatic needs an installed copy"
    );
    assert!(spy.launched.lock().unwrap().is_empty());
}

/// The run controls' Cancel, clicked while the first rename is held: that file
/// is renamed, the rest are not, and the status says the run stopped.
#[test]
fn cancel_stops_a_running_batch_after_the_file_under_way() {
    let fixture = Fixture::new(&["a_1.txt", "a_2.txt", "a_3.txt"]);
    let gate = Gate::default();
    let app = RenameItApp::headless_with_platform(
        fixture.path().to_path_buf(),
        fixture.journal.path().to_path_buf(),
        Spy::new(Some(gate.clone())),
    );
    let mut harness = harness(app);
    type_into(&mut harness, "text or * : ?", "_");
    type_into(&mut harness, "leave empty to delete", "-");

    // Frames only while the gate is shut: `settle` waits for the run to end.
    rename_button(&harness).click();
    harness.step();
    let (state, signal) = &*gate;
    let (guard, wait) = signal
        .wait_timeout_while(
            state.lock().unwrap(),
            std::time::Duration::from_secs(10),
            |s| !s.1,
        )
        .unwrap();
    assert!(!wait.timed_out(), "the run reached its first rename");
    drop(guard);
    harness.step();
    harness
        .get_by_role_and_label(Role::Button, "Cancel")
        .click();
    // One frame takes the click, the next draws what it did.
    harness.step();
    harness.step();
    assert!(present(&harness, "stopping…"));

    state.lock().unwrap().0 = true;
    signal.notify_all();
    settle(&mut harness);
    assert!(
        harness
            .state()
            .status()
            .is_some_and(|s| s.starts_with("Stopped")),
        "{:?}",
        harness.state().status()
    );
    assert_eq!(fixture.names(), ["a-1.txt", "a_2.txt", "a_3.txt"]);
}

/// Settings ▸ File System: the visibility switches and the pattern scope.
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
    click(&mut harness, "Which files show");
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
    // No file carries the system attribute on Linux (`ren-platform`'s
    // visibility.rs), so only the switch itself can be checked here.
    let system = harness.state().session().settings.show_system;
    harness
        .get_by_role_and_label(Role::CheckBox, "System files and folders")
        .click();
    settle(&mut harness);
    assert_eq!(harness.state().session().settings.show_system, !system);

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

/// Settings ▸ Columns & rows and Thumbnails: the row stripes and the
/// thumbnail border are stored.
#[test]
fn the_display_page_stores_its_switches() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = sized(fixture.app(), 1000.0);
    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    click(&mut harness, "Columns & rows");

    let stripes = harness.state().table_style().stripes;
    harness
        .get_by_role_and_label(Role::CheckBox, "Shade every other row")
        .click();
    settle(&mut harness);
    assert_eq!(harness.state().table_style().stripes, !stripes);

    click(&mut harness, "Thumbnails");
    let border = harness.state().session().settings.thumb_border;
    harness
        .get_by_role_and_label(Role::CheckBox, "Draw a border around thumbnails")
        .click();
    settle(&mut harness);
    assert_eq!(harness.state().session().settings.thumb_border, !border);

    // A column turned on, then Restore defaults: the table is back to four.
    click(&mut harness, "Columns & rows");
    harness.get_by_role_and_label(Role::CheckBox, "Ext").click();
    settle(&mut harness);
    assert_eq!(harness.get_all_by_label("Ext").count(), 2, "box and header");
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
    add_operation_of(&mut harness, "Space Trimming", false);
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

// --- Drag to select (P117) ---------------------------------------------------

/// Which files are selected, by name.
fn selected(harness: &Harness<'_, RenameItApp>) -> Vec<String> {
    let session = harness.state().session();
    session
        .selection
        .iter()
        .map(|i| session.entries()[i].file_name.clone())
        .collect()
}

fn row_at(harness: &Harness<'_, RenameItApp>, name: &str) -> egui::Pos2 {
    harness.get_by_label(name).rect().center()
}

/// Press on a row, sweep down three rows: four selected. Back up one: three.
/// Release, then a plain click selects only the row clicked.
#[test]
fn dragging_over_rows_selects_them_and_moving_back_deselects() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);
    let mut harness = harness(fixture.app());
    let (a, c, d) = (
        row_at(&harness, "a.txt"),
        row_at(&harness, "c.txt"),
        row_at(&harness, "d.txt"),
    );

    harness.drag_at(a);
    harness.run();
    harness.hover_at(a + egui::vec2(0.0, 6.0));
    harness.run();
    harness.hover_at(d);
    harness.run();
    assert_eq!(selected(&harness), ["a.txt", "b.txt", "c.txt", "d.txt"]);

    harness.hover_at(c);
    harness.run();
    assert_eq!(selected(&harness), ["a.txt", "b.txt", "c.txt"]);
    harness.drop_at(c);
    settle(&mut harness);
    assert_eq!(
        selected(&harness),
        ["a.txt", "b.txt", "c.txt"],
        "release keeps it"
    );

    click(&mut harness, "e.txt");
    assert_eq!(selected(&harness), ["e.txt"]);
}

/// Ctrl-drag adds the swept rows to what is already selected.
#[test]
fn a_ctrl_drag_adds_to_the_selection() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);
    let mut harness = harness(fixture.app());
    click(&mut harness, "a.txt");
    let (c, d) = (row_at(&harness, "c.txt"), row_at(&harness, "d.txt"));

    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
    harness.drag_at(c);
    harness.run();
    harness.hover_at(c + egui::vec2(0.0, 6.0));
    harness.run();
    harness.hover_at(d);
    harness.run();
    harness.drop_at(d);
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    settle(&mut harness);

    assert_eq!(selected(&harness), ["a.txt", "c.txt", "d.txt"]);
}

/// After a sweep, a click on a blank part of another row — not on its name —
/// selects only that row (P118).
#[test]
fn a_click_anywhere_on_a_row_after_a_sweep_selects_only_that_row() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);
    let mut harness = harness(fixture.app());
    let (a, c) = (row_at(&harness, "a.txt"), row_at(&harness, "c.txt"));
    harness.drag_at(a);
    harness.run();
    harness.hover_at(a + egui::vec2(0.0, 12.0));
    harness.run();
    harness.hover_at(c);
    harness.run();
    harness.drop_at(c);
    settle(&mut harness);
    assert_eq!(selected(&harness), ["a.txt", "b.txt", "c.txt"]);

    // Well to the right of the name, in the same row.
    let e = row_at(&harness, "e.txt") + egui::vec2(300.0, 0.0);
    harness.hover_at(e);
    harness.run();
    harness.event(egui::Event::PointerButton {
        pos: e,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.event(egui::Event::PointerButton {
        pos: e,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    settle(&mut harness);
    assert_eq!(selected(&harness), ["e.txt"]);

    // And just right of a short name, still inside the Name column.
    let d = row_at(&harness, "d.txt") + egui::vec2(60.0, 0.0);
    harness.hover_at(d);
    harness.run();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: d,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    settle(&mut harness);
    assert_eq!(selected(&harness), ["d.txt"]);
}
