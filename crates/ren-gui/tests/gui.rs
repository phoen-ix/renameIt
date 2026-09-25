//! The GUI, driven headlessly.
//!
//! **D24.** `egui_kittest` runs the real `RenameItApp` with no display and no
//! GPU, so M2's acceptance criteria stay things CI proves rather than things
//! anyone asserts. Interaction goes through the accessibility tree — the same
//! tree a screen reader sees — which means a test that finds a button by its
//! label is also checking the button is labelled.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use ren_core::ops::{
    AddCounter, AddRemove, CaseMode, Casing, CounterPlacement, MoveSection, NumberAction,
    NumberTarget, OpKind, ReNumber, Replace, ZeroPadding,
};
use ren_gui::RenameItApp;
use ren_gui::viewmodel::RowFilter;
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    journal: TempDir,
}

impl Fixture {
    fn new(names: &[&str]) -> Self {
        let dir = TempDir::new().expect("tempdir");
        for (i, name) in names.iter().enumerate() {
            std::fs::write(dir.path().join(name), format!("payload {i}")).expect("write");
        }
        Self {
            dir,
            journal: TempDir::new().expect("tempdir"),
        }
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path())
            .expect("readable")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A folder of real, decodable pictures — 40×20 JPEGs, built byte by byte.
    ///
    /// A name ending `.broken.jpg` gets a file that looks like a picture and is
    /// not, which is the case the tile cache has to answer rather than retry.
    fn pictures(names: &[&str]) -> Self {
        let fixture = Self::new(&[]);
        for name in names {
            let bytes = if name.ends_with(".broken.jpg") {
                b"this is not a picture".to_vec()
            } else {
                ren_core::meta::testing::image::jpeg_rotated(40, 20, 1)
            };
            std::fs::write(fixture.dir.path().join(name), bytes).expect("write");
        }
        fixture
    }

    /// A folder of real music files, built byte by byte.
    fn music(tracks: &[(&str, ren_core::meta::testing::Mp3)]) -> Self {
        let fixture = Self::new(&[]);
        for (name, mp3) in tracks {
            mp3.write(fixture.dir.path(), name);
        }
        // The reader's cache is process-wide and keyed by path, and every
        // fixture is a fresh tempdir — but two tests can be handed the same
        // path by the OS after the first one's directory is gone.
        ren_core::meta::audio::forget_all();
        fixture
    }

    fn app(&self) -> RenameItApp {
        RenameItApp::headless(
            self.dir.path().to_path_buf(),
            self.journal.path().to_path_buf(),
        )
    }
}

/// Builds a harness around the app and runs one settled frame.
/// A taller window, for the few tests that open a card's Scope expander.
///
/// Not a preference: a synthetic click is delivered at the node's screen
/// position, so a control outside the panel's clip rect is in the accessibility
/// tree and unreachable by the pointer. The pre-processor's five stages run
/// past 800 pt with a card editor above them.
fn tall_harness(app: RenameItApp) -> Harness<'static, RenameItApp> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 1800.0))
        .build_ui_state(|ui, app: &mut RenameItApp| app.show(ui), app);
    settle(&mut harness);
    harness
}

fn harness(app: RenameItApp) -> Harness<'static, RenameItApp> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_ui_state(|ui, app: &mut RenameItApp| app.show(ui), app);
    settle(&mut harness);
    harness
}

/// Lets the preview worker catch up, *then* paints.
///
/// Order matters: painting while the preview is stale shows the "updating…"
/// marker, and a frame that changes on the next pass keeps `run()` stepping.
fn settle(harness: &mut Harness<'_, RenameItApp>) {
    for _ in 0..3 {
        harness.state_mut().settle();
        harness.run();
    }
}

/// What the settled preview would write, in plan order.
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

#[test]
fn the_window_lists_the_folder_it_was_pointed_at() {
    let fixture = Fixture::new(&["a_one.txt", "b_two.txt"]);
    let harness = harness(fixture.app());

    assert_eq!(harness.state().session().entries().len(), 2);
    // The names are really on screen, not just in the model.
    harness.get_by_label_contains("a_one.txt");
    harness.get_by_label_contains("b_two.txt");
}

/// M2's headline acceptance: editing the operation updates the preview.
#[test]
fn configuring_an_operation_updates_the_preview() {
    let fixture = Fixture::new(&["my_song.mp3", "my_other.mp3"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    assert_eq!(plan.changed(), 2);
    let names: Vec<_> = plan.items.iter().map(|i| i.new_name.clone()).collect();
    assert_eq!(names, ["my other.mp3", "my song.mp3"]);
    // And the new names are painted.
    harness.get_by_label_contains("my song.mp3");
}

#[test]
fn every_general_operation_previews_what_it_should() {
    let fixture = Fixture::new(&["my_song (live).MP3"]);
    let mut harness = harness(fixture.app());

    let cases: Vec<(OpKind, &str)> = vec![
        (
            OpKind::Replace(Replace::new("_", "-")),
            "my-song (live).MP3",
        ),
        (
            OpKind::Casing(Casing::new(CaseMode::Upper)),
            "MY_SONG (LIVE).MP3",
        ),
        (
            OpKind::AddRemove(AddRemove::add("X", 0)),
            "Xmy_song (live).MP3",
        ),
        (
            // Cut "my" from the stem, leaving "_song (live)", then paste at 3.
            OpKind::MoveSection(ren_core::ops::MoveSection::new(2, 0, 3)),
            "_somyng (live).MP3",
        ),
        (
            OpKind::SpaceTrim(ren_core::ops::SpaceTrim::default()),
            "my song (live).MP3",
        ),
    ];

    for (op, expected) in cases {
        let label = op.label();
        *harness.state_mut().operation_mut() = op;
        settle(&mut harness);
        let plan = harness.state().plan().expect("a plan");
        assert_eq!(plan.items[0].new_name, expected, "operation {label}");
    }
}

/// P4 made conflicts a hard block, so the button has to explain itself.
#[test]
fn a_conflict_blocks_the_rename_button() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());

    // Delete everything but the extension: both files want the same name.
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::remove(999, 0));
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    assert!(plan.conflicts() > 0, "{:?}", plan.items);

    // Running anyway must not touch the disk.
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["one.txt", "two.txt"]);
}

#[test]
fn renaming_then_undoing_restores_the_tree() {
    let fixture = Fixture::new(&["a_1.txt", "a_2.txt"]);
    let before = fixture.names();
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["a-1.txt", "a-2.txt"]);

    harness.state_mut().undo_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), before);
}

#[test]
fn simulate_reports_the_plan_without_touching_the_disk() {
    let fixture = Fixture::new(&["a_1.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    harness.state_mut().set_simulate(true);
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);

    assert_eq!(fixture.names(), ["a_1.txt"], "nothing was written");
    assert!(!harness.state().history().can_undo());
    assert!(
        harness
            .state()
            .status()
            .is_some_and(|s| s.contains("Simulated")),
        "{:?}",
        harness.state().status()
    );
}

#[test]
fn f2_renames_a_single_row_and_undo_covers_it() {
    let fixture = Fixture::new(&["before.txt"]);
    let mut harness = harness(fixture.app());

    harness.state_mut().rename_one(0, "after.txt".to_owned());
    settle(&mut harness);
    assert_eq!(fixture.names(), ["after.txt"]);

    // A manual rename is journalled like any batch.
    harness.state_mut().undo_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["before.txt"]);
}

/// A case-only rename is not a collision with an existing file — it is the
/// same file under another spelling, and it is the single most common thing
/// anyone does by hand after a Set Casing run.
///
/// The existence check used to refuse it on a case-insensitive volume, where
/// the target path resolves to the very file being renamed. On Linux the two
/// names are genuinely different files, so this asserts the rename happens on
/// either kind of filesystem — which is the property that matters.
/// A fresh Set Date card opens on **today**.
///
/// `WallClock::default()` is a deterministic 1980-01-01 so the engine's tests
/// have a fixed instant; offering that as the starting value shows the user a
/// date nobody meant, one click from being written to every selected file.
#[test]
fn a_new_set_date_card_starts_at_today() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::SetDate(Default::default()));
    settle(&mut harness);

    let today = chrono::Local::now().date_naive();
    let card = harness
        .state()
        .stack()
        .cards()
        .iter()
        .find_map(|card| match &card.op {
            OpKind::SetDate(d) => Some(d.date),
            _ => None,
        })
        .expect("the card is on the stack");
    assert_eq!(card.year, chrono::Datelike::year(&today));
    assert_eq!(card.month, chrono::Datelike::month(&today));
    assert_eq!(card.day, chrono::Datelike::day(&today));
}

#[test]
fn f2_accepts_a_case_only_rename() {
    let fixture = Fixture::new(&["readme.txt"]);
    let mut harness = harness(fixture.app());

    harness.state_mut().rename_one(0, "README.txt".to_owned());
    settle(&mut harness);

    assert_eq!(fixture.names(), ["README.txt"]);
    assert!(
        !harness
            .state()
            .status()
            .is_some_and(|s| s.contains("already exists")),
        "{:?}",
        harness.state().status()
    );
}

#[test]
fn a_manual_rename_onto_an_existing_name_is_refused() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let mut harness = harness(fixture.app());

    harness.state_mut().rename_one(0, "b.txt".to_owned());
    settle(&mut harness);

    assert_eq!(
        fixture.names(),
        ["a.txt", "b.txt"],
        "nothing was overwritten"
    );
    assert!(
        harness
            .state()
            .status()
            .is_some_and(|s| s.contains("already exists")),
        "{:?}",
        harness.state().status()
    );
}

#[test]
fn the_include_filter_narrows_which_files_change() {
    let fixture = Fixture::new(&["live_set.mp3", "studio_take.mp3"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    harness
        .state_mut()
        .set_filter(ren_gui::widgets::filter_editor::FilterForm {
            include: "live".into(),
            ..Default::default()
        });
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    assert_eq!(plan.changed(), 1, "{:?}", plan.items);
}

#[test]
fn selecting_rows_scopes_the_run() {
    let fixture = Fixture::new(&["a_1.txt", "a_2.txt", "a_3.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    harness.state_mut().select([1]);
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    assert_eq!(plan.items.len(), 1, "only the selected row is planned");

    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["a-2.txt", "a_1.txt", "a_3.txt"]);
}

#[test]
fn the_row_filter_chip_hides_unchanged_rows() {
    let fixture = Fixture::new(&["with_underscore.txt", "plain.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    settle(&mut harness);
    harness.get_by_label_contains("plain.txt");

    harness.state_mut().set_row_filter(RowFilter::Changed);
    settle(&mut harness);
    assert!(
        harness.query_by_label_contains("plain.txt").is_none(),
        "an unchanged row must be hidden by the Changed chip"
    );
    harness.get_by_label_contains("with-underscore.txt");
}

/// F9 refreshes the file list — and a half-refresh is worse than none,
/// because it looks like a second opinion and is only the first one repeated.
///
/// The relist alone re-read names, sizes and dates, which come from the
/// directory entry. Everything read from *inside* a file — the Exif date, the
/// MP3 artist, a picture's `<Width>`, its thumbnail — is held in a process-wide
/// cache keyed on path, length and mtime, and Refresh never touched any of it.
/// So a file changed by another program in a way that kept its length and its
/// timestamp went on showing the old value, and pressing Refresh confirmed it.
///
/// An Exif date is a fixed-width string, so the two files below are the same
/// length by construction, and the mtime is put back by hand — which is exactly
/// the edit the cache key cannot see, made deliberately rather than waited for.
#[test]
fn refreshing_reads_the_files_again_rather_than_trusting_what_it_remembers() {
    use ren_core::meta::testing::image::jpeg_with_exif;

    let fixture = Fixture::new(&[]);
    let path = fixture.dir.path().join("photo.jpg");
    let stamped = |date: &str| jpeg_with_exif(Some(date), None, None);
    std::fs::write(&path, stamped("2020:01:01 00:00:00")).expect("write");
    let when = std::fs::metadata(&path)
        .expect("stat")
        .modified()
        .expect("mtime");
    // The cache is process-wide and keyed by path, and the OS hands the same
    // tempdir path back to a later test once this one's directory is gone.
    ren_core::meta::exif::forget_all();

    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() =
        OpKind::FreeFormat(ren_core::ops::FreeFormat::new("<ExifDate>"));
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["2020-01-01.jpg"]);

    std::fs::write(&path, stamped("2021:06:06 12:00:00")).expect("rewrite");
    std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("open")
        .set_modified(when)
        .expect("put the timestamp back");

    settle(&mut harness);
    assert_eq!(
        new_names(&harness),
        ["2020-01-01.jpg"],
        "nothing asked it to look again, so it remembers"
    );

    harness.state_mut().forget_and_relist();
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["2021-06-06.jpg"]);
}

/// With the Thumbnail column on, a picture shows a small preview of itself.
///
/// The picture itself cannot be asserted headlessly — there is no GPU and no
/// snapshot testing (D24) — so what is asserted is the alt text, which is the
/// only route a texture has into the accessibility tree and therefore both the
/// screen-reader label and the handle every test here has on a tile (P73).
#[test]
fn a_picture_gets_a_thumbnail_in_the_list() {
    let fixture = Fixture::pictures(&["holiday.jpg", "notes.txt"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .show_column(ren_gui::viewmodel::ColumnKind::Thumbnail, true);
    settle(&mut harness);

    harness.get_by_label_contains("thumbnail of holiday.jpg");
    // That the text file is never *asked* about is `tile::key_for`'s own test;
    // what this one adds is that nothing draws a picture for it either.
    assert!(
        harness
            .query_by_label_contains("thumbnail of notes.txt")
            .is_none(),
        "a text file has no preview to show"
    );
}

/// The headline property, and the reason a thumbnail grid can be opened on a
/// folder of ten thousand at all: demand is bounded by the viewport, not by
/// the folder.
#[test]
fn only_the_pictures_on_screen_are_decoded() {
    let names: Vec<String> = (0..60).map(|i| format!("p{i:02}.jpg")).collect();
    let fixture = Fixture::pictures(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .show_column(ren_gui::viewmodel::ColumnKind::Thumbnail, true);
    settle(&mut harness);

    let asked = harness.state().thumbs().asked();
    assert!(asked > 0, "the rows on screen did ask for their pictures");
    assert!(
        asked < names.len() / 2,
        "{asked} of {} asked for — the whole folder was read",
        names.len()
    );
    // And what was asked for is what was drawn, not a screenful more.
    assert!(!harness.state().thumbs().is_empty());
}

/// A tile already held must not be asked for again, and a file that is not a
/// picture must be answered once rather than retried on every frame.
///
/// The retry is not a performance note: a frame that asks for something on
/// every pass never reaches a still state, and `egui_kittest` panics rather
/// than warns when `run` cannot get there.
#[test]
fn a_settled_view_asks_for_nothing_more() {
    let fixture = Fixture::pictures(&["a.jpg", "b.jpg", "c.broken.jpg"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .show_column(ren_gui::viewmodel::ColumnKind::Thumbnail, true);
    settle(&mut harness);

    let requests = harness.state().thumbs().requests();
    let asked = harness.state().thumbs().asked();
    let held = harness.state().thumbs().len();
    assert_eq!(held, 3, "two pictures and one refusal, all remembered");

    for _ in 0..5 {
        harness.run();
    }
    assert_eq!(
        harness.state().thumbs().requests(),
        requests,
        "a view that has what it needs asks for nothing"
    );
    assert_eq!(harness.state().thumbs().asked(), asked);
    assert_eq!(harness.state().thumbs().len(), held);
}

/// Switches to the grid, as the source bar's List | Grid pair does.
fn use_the_grid(harness: &mut Harness<'_, RenameItApp>) {
    harness.state_mut().session_mut().settings.view = ren_gui::viewmodel::ViewMode::Grid;
}

/// The grid is the same listing, so it shows the same files.
#[test]
fn the_grid_shows_the_pictures_the_list_would() {
    let fixture = Fixture::pictures(&["holiday.jpg", "notes.txt"]);
    let mut harness = harness(fixture.app());
    use_the_grid(&mut harness);
    settle(&mut harness);

    harness.get_by_label_contains("thumbnail of holiday.jpg");
    harness.get_by_label_contains("notes.txt");
}

/// A tile carries the name the run will write, not only the name on disk.
///
/// A grid that showed only the current name would be a picture viewer: the
/// whole point of the New name column is seeing what is about to happen before
/// it happens, and a user who switched to the grid to check their photographs
/// would lose exactly that.
#[test]
fn a_tile_shows_the_name_the_run_will_write() {
    let fixture = Fixture::pictures(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    use_the_grid(&mut harness);
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    settle(&mut harness);

    // `query_all`, not `get`: the diffed new name is drawn as several runs, and
    // the operation panel names the file too.
    for text in ["my_holiday.jpg", "my-holiday"] {
        assert!(
            harness.query_all_by_label_contains(text).count() >= 1,
            "the tile does not carry {text}"
        );
    }
}

/// The exact bug `cell_of` was written to prevent, now reachable from a second
/// view: `RowState::Unchanged` is only about the *name*, so a tile that
/// classified rows for itself would say "unchanged" about a file the run is
/// about to write to.
#[test]
fn a_tile_says_what_the_run_does_rather_than_what_its_state_is_called() {
    let fixture = Fixture::pictures(&["holiday.jpg"]);
    let mut harness = harness(fixture.app());
    use_the_grid(&mut harness);
    harness
        .state_mut()
        .add_operation(OpKind::SetAttributes(ren_core::ops::SetAttributes {
            read_only: Some(true),
            ..Default::default()
        }));
    settle(&mut harness);

    assert!(
        harness
            .query_all_by_label_contains("Write Protect on")
            .count()
            >= 1,
        "the tile does not say what the run will do"
    );
    assert!(
        harness.query_by_label_contains("unchanged").is_none(),
        "the run is about to write to this file"
    );
}

/// Two properties at once, and the second is the sharpest silent bug the grid
/// could ship. The Changed chip must hide a *tile* as it hides a row — and the
/// tile that remains must be the file it is a picture of, not the file that
/// happens to sit at that position in the unfiltered listing.
#[test]
fn hiding_a_row_hides_its_tile_and_the_rest_still_point_at_their_own_files() {
    let fixture = Fixture::pictures(&["a_plain.jpg", "b_keep.jpg"]);
    let mut harness = harness(fixture.app());
    use_the_grid(&mut harness);
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("keep", "kept"));
    settle(&mut harness);
    harness.get_by_label_contains("thumbnail of a_plain.jpg");

    harness.state_mut().set_row_filter(RowFilter::Changed);
    settle(&mut harness);

    // The hidden row is gone from the tree entirely — no tile, and no caption.
    assert!(
        harness
            .query_by_label_contains("thumbnail of a_plain.jpg")
            .is_none(),
        "a row the filter hid must not come back as a tile"
    );
    // And the one tile left is the *second* entry, drawn from its own index
    // rather than from its position among the visible ones.
    harness.get_by_label_contains("thumbnail of b_keep.jpg");
    harness.get_by_label_contains("b_kept.jpg");
}

/// The grid must bound its work by the viewport, exactly as the list does — and
/// it has to do it by hand, because `egui_table` has no grid.
#[test]
fn a_grid_over_a_large_folder_asks_only_for_what_is_on_screen() {
    let names: Vec<String> = (0..200).map(|i| format!("p{i:03}.jpg")).collect();
    let fixture = Fixture::pictures(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let mut harness = harness(fixture.app());
    use_the_grid(&mut harness);
    settle(&mut harness);

    let asked = harness.state().thumbs().asked();
    assert!(asked > 0, "the tiles on screen did ask");
    assert!(
        asked < names.len() / 2,
        "{asked} of {} asked for — the whole folder was laid out",
        names.len()
    );

    // And it settles: a view that asked again on every frame would never reach
    // a still one, which `Harness::run` treats as a panic rather than a warning.
    let requests = harness.state().thumbs().requests();
    for _ in 0..5 {
        harness.run();
    }
    assert_eq!(harness.state().thumbs().requests(), requests);
}

/// The Explorer interaction itself cannot be tested here, but winit delivers
/// drops as ordinary `RawInput`, so the plumbing behind it can be.
#[test]
fn dropped_files_populate_free_select() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let other = TempDir::new().unwrap();
    std::fs::write(other.path().join("elsewhere.txt"), b"x").unwrap();

    let mut app = fixture.app();
    app.session_mut().accept_dropped(vec![
        fixture.dir.path().join("one.txt"),
        other.path().join("elsewhere.txt"),
    ]);
    let mut harness = harness(app);
    settle(&mut harness);

    let session = harness.state().session();
    assert_eq!(
        session.settings.mode,
        ren_gui::viewmodel::SourceMode::FreeSelect
    );
    assert_eq!(session.entries().len(), 2);
    assert_eq!(session.free_select_folder_count(), 2, "two source folders");
    harness.get_by_label_contains("elsewhere.txt");
}

/// M2's performance criterion: a 5 000-file listing must keep up with typing.
#[test]
fn a_five_thousand_file_preview_keeps_up_with_typing() {
    let dir = TempDir::new().unwrap();
    for i in 0..5_000 {
        std::fs::write(dir.path().join(format!("track_{i:05}.mp3")), b"").unwrap();
    }
    let journal = TempDir::new().unwrap();
    let mut app = RenameItApp::headless(dir.path().to_path_buf(), journal.path().to_path_buf());
    // The listing is a worker's answer now, not the constructor's.
    app.settle();
    assert_eq!(app.session().entries().len(), 5_000);

    // Typing "Holiday" one character at a time.
    let mut worst = std::time::Duration::ZERO;
    for n in 1..="Holiday".len() {
        let started = std::time::Instant::now();
        *app.operation_mut() = OpKind::Replace(Replace::new("_", &"Holiday"[..n]));
        app.settle();
        worst = worst.max(started.elapsed());
    }

    assert!(
        app.plan().is_some_and(|p| p.changed() == 5_000),
        "every row should have changed"
    );
    // Generous, because a debug build and a shared CI runner are both slow;
    // the real budget lives in the criterion bench and Spike B's harness.
    assert!(
        worst < std::time::Duration::from_secs(2),
        "a keystroke took {worst:?} over 5 000 files"
    );
}

// --- M3: the tag engine, the counter and the Numbers group -------------------

/// Every operation the picker offers, including the three the Numbers group
/// adds, produces the name it should.
#[test]
fn every_numbers_operation_previews_what_it_should() {
    let fixture = Fixture::new(&["Track 7 of 12.mp3"]);
    let mut harness = harness(fixture.app());

    let cases: Vec<(OpKind, &str)> = vec![
        (
            OpKind::ZeroPadding(ZeroPadding::new(3)),
            "Track 007 of 012.mp3",
        ),
        (
            OpKind::AddCounter(AddCounter::new(CounterPlacement::First, ". ")),
            "1. Track 7 of 12.mp3",
        ),
        (
            OpKind::ReNumber(
                ReNumber::new(NumberTarget::Last, NumberAction::Add).with_operand("1"),
            ),
            "Track 7 of 13.mp3",
        ),
    ];

    for (op, expected) in cases {
        let label = op.label();
        *harness.state_mut().operation_mut() = op;
        settle(&mut harness);
        let plan = harness.state().plan().expect("a plan");
        assert_eq!(plan.items[0].new_name, expected, "operation {label}");
    }
}

/// The Counter Setup panel is on screen, and what it says drives the preview.
#[test]
fn the_counter_setup_panel_drives_the_counter() {
    use ren_core::CounterSetup;

    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() =
        OpKind::AddCounter(AddCounter::new(CounterPlacement::First, "-"));
    settle(&mut harness);

    // The header is there to be opened, and the tag policy is a plain checkbox.
    harness.get_by_label_contains("Counter setup");
    harness.get_by_label_contains("Only rename if all tags are available");

    harness.state_mut().settings_mut().counter = CounterSetup {
        start: 10,
        step: 5,
        auto_pad: false,
        pad: 3,
        ..Default::default()
    };
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    let names: Vec<_> = plan.items.iter().map(|i| i.new_name.clone()).collect();
    assert_eq!(names, ["010-a.txt", "015-b.txt", "020-c.txt"]);
}

/// The Parts pattern is run-wide, and `<%n>` reads it.
#[test]
fn the_parts_pattern_feeds_the_part_tags() {
    use ren_core::PartsSpec;

    let fixture = Fixture::new(&["Metallica - Nothing Else Matters.mp3"]);
    let mut harness = harness(fixture.app());

    harness.get_by_label_contains("Setup parts");

    harness.state_mut().settings_mut().parts = PartsSpec::new("<%1> - <%2>");
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove {
        mode: ren_core::ops::AddRemoveMode::Both,
        insert: "<%2> [<%1>]".into(),
        delete: 999,
        ..Default::default()
    });
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    assert_eq!(
        plan.items[0].new_name,
        "Nothing Else Matters [Metallica].mp3"
    );
}

/// D29: a mistyped tag is reported in the editor rather than silently emptying
/// a thousand filenames.
#[test]
fn a_mistyped_tag_is_reported_in_the_editor() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::add("<Nmae>", 0));
    settle(&mut harness);

    assert!(
        harness
            .query_all_by_label_contains("<Nmae> is not a tag")
            .count()
            >= 1,
        "the card and its editor both say so"
    );
    // And nothing is renamed on the strength of it.
    let plan = harness.state().plan().expect("a plan");
    assert_eq!(plan.changed(), 0);
    assert_eq!(plan.errors(), 1);

    // A tag from a family we have not built says so, by name. Repointed as
    // each family lands: `<Artist>` was here until M6 built the music tags.
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::add("<PdfPages>", 0));
    settle(&mut harness);
    assert!(
        harness
            .query_all_by_label_contains("not supported yet")
            .count()
            >= 1
    );
}

/// The other direction, which the test below cannot check.
///
/// Proving every menu entry compiles says nothing about a tag that exists and
/// is *not* listed — and with the music family the menu is how anyone finds
/// `<TrackN>` or `<FreqS>` in the first place. Only checkable for a closed
/// family, which the audio fields are and the Exif passthrough is not.
#[test]
fn the_tag_menu_lists_every_music_tag_the_engine_has() {
    use ren_core::template::tag::AudioField;

    let listed = ren_gui::widgets::tag_field::menu_tags();
    for field in AudioField::ALL {
        let tag = format!("<{}>", field.label());
        assert!(
            listed.contains(&tag.as_str()),
            "{tag} is implemented but not in the tag menu"
        );
    }
}

/// The tag menu is on every tag-bearing field, and every tag it offers compiles
/// — the menu cannot drift away from the engine without this failing.
#[test]
fn the_tag_menu_offers_only_tags_that_compile() {
    use ren_core::Template;

    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::add("x", 0));
    settle(&mut harness);

    harness.get_by_label("<tags>");

    for tag in ren_gui::widgets::tag_field::menu_tags() {
        Template::compile(tag)
            .unwrap_or_else(|e| panic!("the menu offers {tag}, which fails: {e}"));
    }
}

/// Music Rename end to end: the palette offers it, the radios pick a style, and
/// the preview is built from what is inside the files.
#[test]
fn music_rename_names_files_from_their_tags() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::music(&[
        ("01.mp3", Mp3::tagged("Metallica", "One").frame("TRCK", "4")),
        (
            "02.mp3",
            Mp3::tagged("Metallica", "Fade to Black").frame("TRCK", "5"),
        ),
    ]);
    let mut harness = harness(fixture.app());

    // Through the palette, so the entry being live is part of the test. It
    // has to be searched for rather than scrolled to: the list is taller than
    // the modal, and a click on a row below the fold lands on nothing.
    harness.get_by_label_contains("Add operation").click();
    settle(&mut harness);
    let search = harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .expect("the search box");
    search.focus();
    harness.run();
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .unwrap()
        .type_text("music rename");
    settle(&mut harness);
    harness.get_by_label("Music Rename").click();
    settle(&mut harness);

    // The default style, showing in the preview without anything being typed.
    assert!(
        harness.query_by_label("Metallica - One.mp3").is_some(),
        "the default style should already be previewing"
    );

    // A different radio is a different pattern, and `<Track>` pads to two.
    harness.get_by_label("<Track>. <Title>").click();
    settle(&mut harness);
    harness.get_by_label("04. One.mp3");
    harness.get_by_label("05. Fade to Black.mp3");

    // And it really renames.
    harness.get_by_label("Rename").click();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["04. One.mp3", "05. Fade to Black.mp3"]);
}

/// A file with no tags at all is left alone. Without that guard every untagged
/// file in the folder collapses to the pattern's punctuation, they all collide,
/// and the run is blocked with a message about duplicates that says nothing
/// about the real problem.
#[test]
fn music_rename_leaves_an_untagged_file_alone() {
    use ren_core::meta::testing::Mp3;

    let untagged = || Mp3 {
        audio: true,
        ..Default::default()
    };
    let fixture = Fixture::music(&[
        ("a.mp3", untagged()),
        ("b.mp3", untagged()),
        ("c.mp3", Mp3::tagged("Metallica", "One")),
    ]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::MusicRename(Default::default()));
    settle(&mut harness);

    harness.get_by_label("Rename").click();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["Metallica - One.mp3", "a.mp3", "b.mp3"]);
}

/// The styles are a list the user owns, and the card stores the *pattern* it
/// ended up with — so editing the list never rewrites a pipeline already built.
#[test]
fn the_music_styles_list_is_editable_and_the_card_keeps_its_pattern() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::music(&[("01.mp3", Mp3::tagged("Metallica", "One"))]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::MusicRename(Default::default()));
    settle(&mut harness);
    assert_eq!(harness.state().music_styles().len(), 3);

    // The card's own link opens Settings on the right page.
    harness.get_by_label("(edit styles)").click();
    settle(&mut harness);
    harness.get_by_label("Music Styles");
    harness.get_by_label_contains("not which row it came from");

    harness.get_all_by_label("✖").next().unwrap().click();
    settle(&mut harness);
    assert_eq!(harness.state().music_styles().len(), 2);
    harness.get_by_label("Close").click();
    settle(&mut harness);

    // The card kept the pattern it was given, even though its row is gone.
    match &harness.state().stack().cards().last().unwrap().op {
        OpKind::MusicRename(music) => assert_eq!(music.style.as_str(), "<Artist> - <Title>"),
        other => panic!("expected a Music Rename card, got {other:?}"),
    }
    harness.get_by_label("Metallica - One.mp3");
}

/// The two irreversible operations are addable, editable, and — this is the
/// point — **refused** by the engine, because the confirmation that would allow
/// them is not built yet. Failing closed is what P2 asked for.
#[test]
fn the_tag_operations_are_offered_but_refused_until_they_are_confirmed() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::music(&[("Metallica - One.mp3", Mp3::tagged("Old", "Old"))]);
    let mut harness = harness(fixture.app());
    let before = std::fs::read(fixture.dir.path().join("Metallica - One.mp3")).unwrap();

    harness
        .state_mut()
        .add_operation(OpKind::MusicTagger(Default::default()));
    settle(&mut harness);

    // A freshly added card does nothing at all (P34), and says so.
    harness.get_by_label_contains("no fields enabled");
    harness.get_by_label_contains("cannot be undone");

    // Ticking Artist enables the field — empty, so still nothing to write.
    harness.get_by_label("Artist").click();
    settle(&mut harness);
    match &harness.state().stack().cards().last().unwrap().op {
        OpKind::MusicTagger(op) => assert!(op.artist.is_some()),
        other => panic!("expected a Music Tagger card, got {other:?}"),
    }
    assert_eq!(
        harness.state().plan().map(ren_core::Plan::irreversible),
        Some(0),
        "an enabled but empty field must still write nothing"
    );

    // Give it a value, and now there is something the engine has to refuse.
    let last = harness.state().stack().len() - 1;
    match &mut harness.state_mut().stack_mut().get_mut(last).unwrap().op {
        OpKind::MusicTagger(op) => {
            op.artist = Some(ren_core::template::TextTemplate::new("Written"));
        }
        other => panic!("expected a Music Tagger card, got {other:?}"),
    }
    settle(&mut harness);
    assert_eq!(
        harness.state().plan().map(ren_core::Plan::irreversible),
        Some(1)
    );

    // P2's gate. Pressing Rename asks first, and touches nothing until asked.
    harness.get_by_label("Rename").click();
    settle(&mut harness);
    let confirm = ren_gui::panels::confirm::go_ahead_label(1);
    harness.get_by_label_contains(&confirm);
    assert_eq!(
        std::fs::read(fixture.dir.path().join("Metallica - One.mp3")).unwrap(),
        before,
        "nothing may be written before the user has agreed"
    );

    // Saying no leaves everything exactly as it was, and does not remember the
    // question being asked.
    harness.get_by_label("Leave everything alone").click();
    settle(&mut harness);
    assert!(
        harness.query_by_label_contains(&confirm).is_none(),
        "the dialog should be gone"
    );
    let status = harness.state().status().unwrap_or_default().to_owned();
    assert!(status.contains("nothing was changed"), "{status}");
    assert_eq!(
        std::fs::read(fixture.dir.path().join("Metallica - One.mp3")).unwrap(),
        before
    );

    // Saying yes writes.
    harness.get_by_label("Rename").click();
    settle(&mut harness);
    harness.get_by_label_contains(&confirm).click();
    settle(&mut harness);
    ren_core::meta::audio::forget_all();
    assert_eq!(
        ren_core::meta::audio::tags_of(&fixture.dir.path().join("Metallica - One.mp3"))
            .unwrap()
            .artist
            .as_deref(),
        Some("Written")
    );
    let status = harness.state().status().unwrap_or_default().to_owned();
    assert!(status.contains("Modified 1"), "{status}");
}

/// **The generation binding.** Consent is for the plan the user was shown, not
/// for the app. A bare bool cannot carry that: cleared synchronously it loses
/// the consent when `run()` defers, and cleared after the run it would let an
/// edited pipeline execute under an approval given for a different one.
#[test]
fn consent_does_not_survive_a_change_to_the_pipeline() {
    use ren_core::meta::testing::Mp3;
    use ren_core::template::TextTemplate;

    let fixture = Fixture::music(&[("song.mp3", Mp3::tagged("Old", "Old"))]);
    let mut harness = harness(fixture.app());
    let tagger = ren_core::ops::MusicTagger {
        artist: Some(TextTemplate::new("Written")),
        ..Default::default()
    };
    harness
        .state_mut()
        .add_operation(OpKind::MusicTagger(Box::new(tagger)));
    settle(&mut harness);

    harness.get_by_label("Rename").click();
    settle(&mut harness);
    let confirm = ren_gui::panels::confirm::go_ahead_label(1);
    harness.get_by_label_contains(&confirm);

    // Change the pipeline while the dialog is up. The plan that produced those
    // numbers is now history.
    let last = harness.state().stack().len() - 1;
    match &mut harness.state_mut().stack_mut().get_mut(last).unwrap().op {
        OpKind::MusicTagger(op) => {
            op.title = Some(TextTemplate::new("Also written"));
        }
        other => panic!("expected a Music Tagger card, got {other:?}"),
    }
    settle(&mut harness);

    // Agreeing now must not run the plan that was approved, because it is not
    // the plan any more.
    harness.get_by_label_contains(&confirm).click();
    settle(&mut harness);
    harness.get_by_label_contains(&confirm);
    ren_core::meta::audio::forget_all();
    assert_eq!(
        ren_core::meta::audio::tags_of(&fixture.dir.path().join("song.mp3"))
            .unwrap()
            .artist
            .as_deref(),
        Some("Old"),
        "a stale approval must not write"
    );
}

/// D26: pressing Run again while the dialog is up must not stack a second one.
#[test]
fn asking_again_while_the_dialog_is_up_does_not_stack_dialogs() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::music(&[("song.mp3", Mp3::tagged("Old", "Old"))]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::RemoveTags(ren_core::ops::RemoveTags {
            id3v2: true,
            ..Default::default()
        }));
    settle(&mut harness);

    harness.get_by_label("Rename").click();
    settle(&mut harness);
    harness.state_mut().run_now();
    harness.state_mut().run_now();
    settle(&mut harness);

    let confirm = ren_gui::panels::confirm::go_ahead_label(1);
    assert_eq!(
        harness.query_all_by_label_contains(&confirm).count(),
        1,
        "one dialog, however many times Run was pressed"
    );
}

/// The engine exempts simulation because it performs no syscall, and a dialog
/// headed "cannot be undone" would simply be false there.
#[test]
fn a_simulated_irreversible_run_needs_no_confirmation() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::music(&[("song.mp3", Mp3::tagged("Old", "Old"))]);
    let before = std::fs::read(fixture.dir.path().join("song.mp3")).unwrap();
    let mut harness = harness(fixture.app());
    harness.state_mut().set_simulate(true);
    harness
        .state_mut()
        .add_operation(OpKind::RemoveTags(ren_core::ops::RemoveTags {
            id3v2: true,
            ..Default::default()
        }));
    settle(&mut harness);

    harness.state_mut().run_now();
    settle(&mut harness);

    assert!(
        harness
            .query_all_by_label_contains(&ren_gui::panels::confirm::go_ahead_label(1))
            .next()
            .is_none(),
        "a simulation has nothing to consent to"
    );
    let status = harness.state().status().unwrap_or_default().to_owned();
    assert!(status.contains("nothing was written"), "{status}");
    assert_eq!(
        std::fs::read(fixture.dir.path().join("song.mp3")).unwrap(),
        before
    );
}

/// The regression guard for the user's scope decision: an ordinary rename is
/// still one click, and stays one click.
#[test]
fn an_ordinary_rename_is_still_one_click() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("a", "b"));
    settle(&mut harness);

    harness.get_by_label("Rename").click();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["b.txt"], "it should just have run");
    assert!(
        harness.query_by_label_contains("Change ").is_none(),
        "no confirmation for a run that can be undone"
    );
}

/// Quick Setup is the inverse of Music Rename: pick the shape your filenames
/// already have, and it configures Setup Parts *and* the field mappings.
#[test]
fn quick_setup_configures_the_parts_pattern_and_the_fields_together() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::music(&[("Metallica - One.mp3", Mp3::tagged("Old", "Old"))]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::MusicTagger(Default::default()));
    settle(&mut harness);

    harness.get_by_label_contains("Quick Setup").click();
    settle(&mut harness);
    harness.get_by_label("<Artist> - <Title>").click();
    settle(&mut harness);

    // The run-wide Parts pattern, which the card cannot carry on its own.
    assert_eq!(
        harness.state().run_settings().parts.pattern,
        "<%1> - <%2>",
        "Quick Setup has to set Setup Parts too, or the fields resolve to nothing"
    );
    match &harness.state().stack().cards().last().unwrap().op {
        OpKind::MusicTagger(op) => {
            assert_eq!(op.artist.as_ref().unwrap().as_str(), "<%1>");
            assert_eq!(op.title.as_ref().unwrap().as_str(), "<%2>");
            assert!(op.album.is_none());
        }
        other => panic!("expected a Music Tagger card, got {other:?}"),
    }
    // And the preview now shows what it would write.
    harness.get_by_label_contains("Artist → Metallica");
}

/// Remove Tags ships with every box clear — the only safe default for
/// something with no undo.
#[test]
fn remove_tags_starts_with_nothing_selected() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::music(&[("t.mp3", Mp3::tagged("A", "B"))]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::RemoveTags(Default::default()));
    settle(&mut harness);

    harness.get_by_label_contains("Remove these tags, if present");
    harness.get_by_label_contains("Nothing is selected");
    match &harness.state().stack().cards().last().unwrap().op {
        OpKind::RemoveTags(op) => {
            assert!(!op.id3v1 && !op.id3v2 && !op.lyrics);
        }
        other => panic!("expected a Remove Tags card, got {other:?}"),
    }

    harness.get_by_label("ID3v2").click();
    settle(&mut harness);
    harness.get_by_label_contains("cannot be undone");
}

/// The GUI half of the same thing: "Undone" on its own is a lie for a mixed
/// batch, because the renames came back and the tag writes did not.
#[test]
fn undoing_a_mixed_batch_says_what_could_not_be_taken_back() {
    use ren_core::meta::testing::Mp3;
    use ren_core::template::TextTemplate;

    let fixture = Fixture::music(&[("song.mp3", Mp3::tagged("Old", "Old"))]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("song", "tune"));
    let tagger = ren_core::ops::MusicTagger {
        artist: Some(TextTemplate::new("Written")),
        ..Default::default()
    };
    harness
        .state_mut()
        .add_operation(OpKind::MusicTagger(Box::new(tagger)));
    settle(&mut harness);

    // Consent, which step 6 will collect from a dialog.
    harness.state_mut().run_allowing_irreversible();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["tune.mp3"]);

    harness.state_mut().undo_now();
    settle(&mut harness);

    assert_eq!(
        fixture.names(),
        ["song.mp3"],
        "the rename did not come back"
    );
    let status = harness.state().status().unwrap_or_default();
    assert!(
        status.contains("could not be taken back"),
        "the status must not just say Undone: {status}"
    );
    harness.get_by_label_contains("still applied");
}

/// The three silent halves of an action-only run, all of which happened after
/// the button was pressed and none of which anything asserted.
///
/// `Set Attributes` is `Undoability::Journaled`, so this run needs no
/// confirmation — which is the point: it isolates *reporting* from *consent*.
#[test]
fn an_action_only_run_reports_what_it_did() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::SetAttributes(ren_core::ops::SetAttributes {
            read_only: Some(true),
            ..Default::default()
        }));
    settle(&mut harness);

    harness.get_by_label("Rename").click();
    settle(&mut harness);

    let status = harness.state().status().unwrap_or_default().to_owned();
    assert!(
        !status.contains("Renamed 0"),
        "a run that changed two files must not say it renamed none: {status}"
    );
    assert!(status.contains("Modified 2"), "{status}");

    // The log had no line for a metadata change at all, so it stayed empty —
    // and the Log button is gated on the log being non-empty, so the user was
    // offered no way to see what happened either.
    harness.get_by_label("Log");
    // One line per file that changed. `∆` is the log's own marker —
    // "Write Protect on" alone also matches the New name column, once per row.
    assert_eq!(
        harness
            .query_all_by_label_contains("∆  Write Protect on")
            .count(),
        2
    );
}

/// A simulation of the same run said "Simulated 0 renames — nothing was
/// written", which is wrong about the count as well as the noun.
#[test]
fn a_simulated_action_only_run_says_what_it_would_have_done() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let mut harness = harness(fixture.app());
    harness.state_mut().set_simulate(true);
    harness
        .state_mut()
        .add_operation(OpKind::SetAttributes(ren_core::ops::SetAttributes {
            read_only: Some(true),
            ..Default::default()
        }));
    settle(&mut harness);

    // Through the button, which is now possible: the toggle is "Simulate" and
    // the button it changes is "Run simulation", so the two no longer collide.
    harness.get_by_label("Run simulation").click();
    settle(&mut harness);
    let status = harness.state().status().unwrap_or_default().to_owned();
    assert!(status.contains("Modified 2"), "{status}");
    assert!(status.contains("nothing was written"), "{status}");
}

/// D31: a run can create folders on its way, and the log never said so.
#[test]
fn the_log_names_a_folder_the_run_created() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() =
        OpKind::FreeFormat(ren_core::ops::FreeFormat::new("sorted<\\><Name>"));
    settle(&mut harness);

    harness.get_by_label("Rename").click();
    settle(&mut harness);
    harness.get_by_label_contains("created folder sorted");
}

/// D28: `<Ask>` is collected once, before the run, and the answer reaches every
/// file.
#[test]
fn the_ask_modal_collects_an_answer_before_the_rename() {
    use ren_core::Answers;

    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() =
        OpKind::AddRemove(AddRemove::add("<Ask>", 0).backwards(true));
    settle(&mut harness);

    // Unanswered, the tag previews as nothing rather than blocking the preview.
    let plan = harness.state().plan().expect("a plan");
    assert_eq!(plan.changed(), 0, "nothing to add yet");

    // Pressing Rename opens the modal instead of renaming.
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["one.txt", "two.txt"],
        "nothing renamed yet"
    );
    // The modal is up, with a field for the one slot the pipeline named.
    harness.get_by_label("Enter text");
    assert!(
        harness.query_all_by_label_contains("<Ask>").count() >= 1,
        "the modal labels the slot it is asking about"
    );

    // Answering it is what the modal does on Rename.
    let mut answers = Answers::default();
    answers.asks.insert(0, " (2024)".into());
    harness.state_mut().set_answers(answers);
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    assert_eq!(plan.changed(), 2);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["one (2024).txt", "two (2024).txt"]);
}

/// D31, through the GUI: `<\\>` sorts files into folders, and Undo takes them
/// back out.
#[test]
fn the_subfolder_tag_sorts_files_and_undo_puts_them_back() {
    let fixture = Fixture::new(&["alpha.txt", "beta.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove {
        mode: ren_core::ops::AddRemoveMode::Add,
        insert: "<FLetter><\\>".into(),
        add_pos: 0,
        ..Default::default()
    });
    harness.state_mut().set_scope(ren_core::model::Scope::Both);
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    assert_eq!(plan.changed(), 2, "{:?}", plan.items);

    harness.state_mut().run_now();
    settle(&mut harness);
    assert!(fixture.dir.path().join("A").join("alpha.txt").is_file());
    assert!(fixture.dir.path().join("B").join("beta.txt").is_file());

    harness.state_mut().undo_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["alpha.txt", "beta.txt"]);
    assert!(!fixture.dir.path().join("A").exists());
}

/// A running counter's start moves on after each rename — and only after a
/// real one, never after a simulation.
#[test]
fn a_running_counter_picks_up_where_the_last_batch_left_off() {
    use ren_core::CounterSetup;

    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() =
        OpKind::AddCounter(AddCounter::new(CounterPlacement::First, "-"));
    harness.state_mut().settings_mut().counter = CounterSetup {
        auto_pad: false,
        running: true,
        ..Default::default()
    };
    settle(&mut harness);

    // A simulation must not move the counter on.
    harness.state_mut().set_simulate(true);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(harness.state().settings().counter.start, 1);

    harness.state_mut().set_simulate(false);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1-a.txt", "2-b.txt", "3-c.txt"]);
    assert_eq!(
        harness.state().settings().counter.start,
        4,
        "the next batch starts where this one stopped"
    );
}

// --- M4 step 0: the interaction primitives the pipeline UI is built on -------

/// Everything M4 adds — the operation palette, the preset drawer, each card's
/// Scope expander — is a popover over a button. Nothing in the suite had ever
/// opened one: the include filter is only ever driven through `set_filter`.
///
/// So this is the spike. If a `Popup` does not survive a headless click, it is
/// far cheaper to learn it now than after four panels are built on it.
#[test]
fn a_popover_opens_on_a_click_and_its_contents_reach_the_accessibility_tree() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    // Closed: the popover's contents are not in the tree at all.
    assert!(
        harness
            .query_by_label_contains("Include only files matching")
            .is_none(),
        "the popover should start closed"
    );

    harness.get_by_label_contains("Include filter").click();
    settle(&mut harness);

    harness.get_by_label_contains("Include only files matching");
    harness.get_by_label_contains("Also test the extension");
}

/// The other primitive: a menu button. The card's ⋮ overflow (duplicate,
/// delete, move up/down) is one, and it is how reordering stays reachable from
/// the keyboard and from the accessibility tree.
#[test]
fn a_menu_button_opens_on_a_click_and_its_items_reach_the_accessibility_tree() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::add("x", 0));
    settle(&mut harness);

    assert!(
        harness.query_by_label("<Name>").is_none(),
        "the tag menu should start closed"
    );
    harness.get_by_label("<tags>").click();
    settle(&mut harness);
    harness.get_by_label("<Name>");
}

/// `Ctrl+Z` in a text field means "undo that keystroke", never "put the last
/// batch of renames back". It used to mean the second one: `handle_hotkeys`
/// read the raw key state before any widget saw it, so typing in a Find box
/// could silently revert a rename that had already been written to disk.
#[test]
fn undo_and_run_hotkeys_stand_down_while_the_user_is_typing() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("one", "1"));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1.txt", "two.txt"], "renamed once");

    // Put the caret in an editor field, as a user configuring the next step is.
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .expect("a text field")
        .focus();
    harness.run();

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["1.txt", "two.txt"],
        "Ctrl+Z while typing must not undo the rename"
    );

    harness.key_press(egui::Key::F5);
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1.txt", "two.txt"], "nor F5 rename again");

    // With focus off the field, Undo works as it always did.
    harness.get_by_label_contains("Undo").click();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["one.txt", "two.txt"]);
}

/// A **number box is not a text box**, and the guard above could not tell.
///
/// `ctx.text_edit_focused()` asks "does the focused widget have a
/// `TextEditState`" — and a focused `DragValue` renders a real
/// `TextEdit::singleline(…).id(id)` under its *own* id while it is being typed
/// into, so it stores one. Every card with a position or a count has one of
/// those boxes, so clicking *Delete:* or *from pos:* silently disabled F2, F4,
/// F5, F6, F8, F9, F12, Ctrl+Z and Ctrl+K until the user clicked somewhere else.
///
/// The guard's own reasoning is what decides the fix: it exists so that a
/// keystroke the user meant for *text* is not also a command. A `DragValue` is
/// a number, no function key types a digit, and F5 on a card you have just
/// finished configuring is the single most likely moment to press it. Ctrl+Z
/// is the box's own undo, and stays down — see
/// `ctrl_z_in_a_number_box_undoes_the_digit_and_not_the_batch`.
#[test]
fn the_hotkeys_stay_live_while_a_number_box_has_focus() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::AddRemove(AddRemove::remove(1, 0)));
    settle(&mut harness);

    // The Delete: / from pos: spinners. A focused `DragValue` reports as a
    // SpinButton, not a TextInput — the role is right and the guard was wrong.
    harness
        .get_all_by_role(egui::accesskit::Role::SpinButton)
        .next()
        .expect("a position box on the Add & Remove card")
        .focus();
    harness.run();

    harness.key_press(egui::Key::F5);
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["ne.txt", "wo.txt"],
        "F5 must still run with a number box focused"
    );

    harness.key_press(egui::Key::F4);
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["one.txt", "two.txt"],
        "and F4 must still undo"
    );
}

// --- M4: the pipeline as a stack of cards ------------------------------------

/// M4's acceptance criterion, through the app: three cards, one composed
/// preview, one rename, one undo.
#[test]
fn a_three_card_pipeline_previews_composes_and_undoes_as_one() {
    use ren_core::CounterSetup;

    let fixture = Fixture::new(&["my_holiday_photo.JPG", "another_one.JPG"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    harness
        .state_mut()
        .add_operation(OpKind::Casing(Casing::new(CaseMode::Title)));
    harness
        .state_mut()
        .add_operation(OpKind::AddCounter(AddCounter::new(
            CounterPlacement::First,
            ". ",
        )));
    harness.state_mut().settings_mut().counter = CounterSetup {
        auto_pad: false,
        ..Default::default()
    };
    settle(&mut harness);

    assert_eq!(harness.state().stack().len(), 3);
    let plan = harness.state().plan().expect("a plan");
    let names: Vec<_> = plan.items.iter().map(|i| i.new_name.clone()).collect();
    assert_eq!(names, ["1. Another One.JPG", "2. My Holiday Photo.JPG"]);

    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["1. Another One.JPG", "2. My Holiday Photo.JPG"]
    );

    harness.state_mut().undo_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["another_one.JPG", "my_holiday_photo.JPG"]);
}

/// The enable checkbox is semantics, not decoration.
#[test]
fn disabling_a_card_takes_it_out_of_the_preview() {
    let fixture = Fixture::new(&["my_song.mp3"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    harness
        .state_mut()
        .add_operation(OpKind::Casing(Casing::new(CaseMode::Upper)));
    settle(&mut harness);
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "MY SONG.mp3"
    );

    harness.state_mut().set_card_enabled(1, false);
    settle(&mut harness);
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "my song.mp3"
    );

    // The card is still there, ready to be switched back on.
    assert_eq!(harness.state().stack().len(), 2);
}

/// Order is the meaning of a pipeline, so reordering must change the answer.
#[test]
fn reordering_two_cards_changes_the_result() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Casing(Casing::new(CaseMode::Title));
    harness
        .state_mut()
        .add_operation(OpKind::Replace(Replace::new("_", " and ")));
    settle(&mut harness);
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "A and B.txt"
    );

    harness.state_mut().move_card(1, 0);
    settle(&mut harness);
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "A And B.txt",
        "replacing first means the replacement is title-cased too"
    );
}

/// Reordering has to be reachable without a mouse — which is also the only way
/// the accessibility tree can drive it (D24).
#[test]
fn the_overflow_menu_reorders_the_stack() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Casing(Casing::new(CaseMode::Title));
    harness
        .state_mut()
        .add_operation(OpKind::Replace(Replace::new("_", " and ")));
    settle(&mut harness);
    assert_eq!(harness.state().stack().get(0).unwrap().op.name(), "casing");

    // Open the first card's menu and move it down.
    harness
        .get_all_by_label("Reorder, duplicate or delete")
        .next()
        .unwrap()
        .click();
    settle(&mut harness);
    harness.get_by_label("Move down").click();
    settle(&mut harness);

    assert_eq!(harness.state().stack().get(0).unwrap().op.name(), "replace");
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "A And B.txt"
    );
}

/// Duplicate is an M4 feature, so two cards of one kind is the normal case
/// rather than the edge case. Each must hold and edit its own configuration.
///
/// Driven through the accessibility tree rather than the document, so it
/// covers the editors as well: every one of them uses hard-coded widget ids,
/// and it is `push_id` around the card body that keeps egui's per-widget memory
/// — cursors, selections, open popups — from being shared between them.
#[test]
fn duplicating_a_card_gives_two_independently_editable_cards() {
    let fixture = Fixture::new(&["aXbXc.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("X", "-"));
    harness.state_mut().duplicate_card(0);
    if let OpKind::Replace(op) = &mut harness.state_mut().stack_mut().get_mut(1).unwrap().op {
        op.find = "Y".into();
    }
    harness.state_mut().expand_card(0);
    settle(&mut harness);
    assert_eq!(harness.state().stack().len(), 2);

    // The editor is an accordion — one card open at a time — so the way two
    // cards would collide is by the *second* one inheriting the first's widget
    // state when it opens. Type into each in turn and check neither moved the
    // other.
    // The source bar owns the first two text fields (folder, pattern); the open
    // card's editor follows.
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .nth(2)
        .expect("card 1's Find field")
        .focus();
    harness.run();
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .nth(2)
        .expect("card 1's Find field")
        .type_text("Z");
    settle(&mut harness);

    harness.state_mut().expand_card(1);
    settle(&mut harness);
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .nth(2)
        .expect("card 2's Find field")
        .focus();
    harness.run();
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .nth(2)
        .expect("card 2's Find field")
        .type_text("W");
    settle(&mut harness);

    let stack = harness.state().stack();
    match (&stack.get(0).unwrap().op, &stack.get(1).unwrap().op) {
        (OpKind::Replace(first), OpKind::Replace(second)) => {
            assert!(
                first.find.contains('Z'),
                "card 1 kept its own text: {:?}",
                first.find
            );
            assert!(
                !first.find.contains('W'),
                "card 2 typed into card 1: {:?}",
                first.find
            );
            assert!(
                second.find.contains('W'),
                "card 2 took its keystroke: {:?}",
                second.find
            );
            assert!(
                !second.find.contains('Z'),
                "card 2 inherited card 1's text: {:?}",
                second.find
            );
        }
        other => panic!("expected two Replace cards, got {other:?}"),
    }
}

/// An empty pipeline is reachable for the first time in M4, and the Rename
/// button owes the user a reason rather than a vague "nothing would change".
#[test]
fn deleting_the_last_card_blocks_the_run_with_a_reason() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    harness.state_mut().delete_card(0);
    settle(&mut harness);

    assert!(harness.state().stack().is_empty());
    let reason = harness.state().blocked_reason().expect("should block");
    assert!(reason.contains("empty"), "{reason}");
    harness.get_by_label_contains("No operations yet");

    // And pressing Rename does nothing.
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["a.txt"]);
}

/// D32: a card with no filter of its own uses the source bar's; a card with one
/// uses only its own.
#[test]
fn a_per_card_filter_narrows_only_that_card() {
    use ren_gui::widgets::filter_editor::FilterForm;

    let fixture = Fixture::new(&["concert one.txt", "studio two.txt"]);
    let mut harness = harness(fixture.app());

    // Card 1 only touches the concert; card 2 touches everything.
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::add("[live] ", 0));
    harness.state_mut().set_card_filter(
        0,
        Some(FilterForm {
            include: "concert".into(),
            ..Default::default()
        }),
    );
    harness
        .state_mut()
        .add_operation(OpKind::AddRemove(AddRemove::add("! ", 0)));
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    let names: Vec<_> = plan.items.iter().map(|i| i.new_name.clone()).collect();
    assert_eq!(names, ["! [live] concert one.txt", "! studio two.txt"]);
}

/// The other half of D32: the source bar's filter is what a card without one
/// inherits, which is why the M2 filter test still passes unchanged.
#[test]
fn the_source_bar_filter_is_the_default_for_a_card_that_has_none() {
    use ren_gui::widgets::filter_editor::FilterForm;

    let fixture = Fixture::new(&["concert one.txt", "studio two.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::add("! ", 0));
    harness
        .state_mut()
        .add_operation(OpKind::AddRemove(AddRemove::add("? ", 0)));
    harness.state_mut().set_filter(FilterForm {
        include: "concert".into(),
        ..Default::default()
    });
    settle(&mut harness);

    let plan = harness.state().plan().expect("a plan");
    let names: Vec<_> = plan.items.iter().map(|i| i.new_name.clone()).collect();
    assert_eq!(
        names,
        ["? ! concert one.txt", "studio two.txt"],
        "both cards inherited it"
    );

    // A card with its own filter escapes the inherited one.
    harness.state_mut().set_card_filter(
        1,
        Some(FilterForm {
            include: "studio".into(),
            ..Default::default()
        }),
    );
    settle(&mut harness);
    let plan = harness.state().plan().expect("a plan");
    let names: Vec<_> = plan.items.iter().map(|i| i.new_name.clone()).collect();
    assert_eq!(names, ["! concert one.txt", "? studio two.txt"]);
}

/// The card stack is what gets persisted, so it has to come back.
#[test]
fn the_card_stack_survives_a_restart() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let mut first = harness(fixture.app());

    *first.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    first
        .state_mut()
        .add_operation(OpKind::Casing(Casing::new(CaseMode::Upper)));
    first.state_mut().set_card_enabled(1, false);
    settle(&mut first);

    let steps = first.state().stack().to_steps();
    assert_eq!(steps.len(), 2);

    // A second app, given the steps the first one would have stored.
    let mut restarted = harness(fixture.app());
    restarted.state_mut().delete_card(0); // headless starts with one default card
    restarted
        .state_mut()
        .stack_mut()
        .append_steps(steps.clone());
    settle(&mut restarted);

    assert_eq!(restarted.state().stack().to_steps(), steps);
    assert!(!restarted.state().stack().get(1).unwrap().enabled);
}

// --- M4: the add-operation palette -------------------------------------------

/// Replaces the assertion the two "every operation previews" tests used to
/// carry — reachability proved by interaction rather than by painting.
#[test]
fn the_palette_offers_every_operation_and_adds_the_one_you_pick() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    let before = harness.state().stack().len();

    // Closed to begin with, so none of it is in the tree.
    assert!(harness.query_by_label("Add operation").is_none());

    harness.get_by_label_contains("Add operation").click();
    settle(&mut harness);

    for op in OpKind::all() {
        assert!(
            harness.query_all_by_label_contains(op.label()).count() >= 1,
            "the palette should offer {}",
            op.label()
        );
    }

    // Typed rather than scrolled to. The catalogue is twenty operations of two
    // lines each and the modal is bounded by the window, so it scrolls — and a
    // synthetic click lands at the node's screen position, which for a row
    // below the fold is nowhere. Filtering first is also what the search box is
    // *for*, so this exercises the real path rather than working around it.
    // The palette focuses its search box on open, so the text goes straight in.
    harness.event(egui::Event::Text("zero".to_owned()));
    settle(&mut harness);
    harness.get_by_label("Zero Padding").click();
    settle(&mut harness);

    assert_eq!(harness.state().stack().len(), before + 1);
    assert_eq!(
        harness.state().stack().cards().last().unwrap().op.name(),
        "zero_padding"
    );
}

/// Ctrl+K, per `docs/DESIGN.md` S2.
#[test]
fn ctrl_k_opens_the_palette() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    settle(&mut harness);
    harness.get_by_label("Add operation");
}

#[test]
fn the_palette_search_narrows_to_what_you_typed() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    harness.state_mut().open_palette();
    settle(&mut harness);

    // The search box is the palette's own field; the source bar's two are
    // behind the modal.
    let search = harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .expect("the search box");
    search.focus();
    harness.run();
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .unwrap()
        .type_text("zero");
    settle(&mut harness);

    harness.get_by_label("Zero Padding");
    assert!(
        harness.query_by_label("Set Casing").is_none(),
        "the rest should be filtered out"
    );
}

/// The palette doubled as the roadmap: what was not built yet was listed,
/// greyed, with the milestone that would bring it.
///
/// It was repointed as each one landed — Set Attributes until M5, Music Rename
/// until M6, Scripting until M7 — and Scripting was the last of them. So there
/// is nothing left to point it at, and it asserts the end state instead: no
/// entry still promises to arrive later, and the one that was a placeholder
/// until this milestone really does add.
#[test]
fn the_palette_has_no_unbuilt_entries_left() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    let before = harness.state().stack().len();

    harness.state_mut().open_palette();
    settle(&mut harness);

    assert_eq!(
        harness.query_all_by_label_contains("arrives with").count(),
        0,
        "an entry still says it is coming later"
    );

    // Search first, so the row is actually on screen. Without this the click
    // below lands on nothing — the catalogue is taller than the palette's
    // 360px scroll area, so everything past the General group is clipped while
    // still reaching the accessibility tree. This test passed for a whole
    // milestone that way, pointed at an entry that had since been built.
    let search = harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .expect("the search box");
    search.focus();
    harness.run();
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .unwrap()
        .type_text("scripting");
    settle(&mut harness);

    harness.get_by_label_contains("Scripting").click();
    settle(&mut harness);
    assert_eq!(
        harness.state().stack().len(),
        before + 1,
        "Scripting was the last placeholder and is a real operation now"
    );
}

/// The other half of that: a *ready* entry, clicked the same way, really does
/// land in the stack. Without this the test above cannot tell "the entry is
/// inert because it is not built" from "the click missed".
#[test]
fn a_ready_palette_entry_clicked_the_same_way_really_is_added() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    let before = harness.state().stack().len();

    harness.state_mut().open_palette();
    settle(&mut harness);
    let search = harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .expect("the search box");
    search.focus();
    harness.run();
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .unwrap()
        .type_text("zero padding");
    settle(&mut harness);

    harness.get_by_label_contains("Zero Padding").click();
    settle(&mut harness);
    assert_eq!(harness.state().stack().len(), before + 1);
}

/// The new card lands next to the one you were looking at, and opens.
#[test]
fn a_new_card_is_added_after_the_open_one_and_expanded() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    harness
        .state_mut()
        .add_operation(OpKind::Casing(Casing::new(CaseMode::Upper)));
    harness
        .state_mut()
        .add_operation(OpKind::SpaceTrim(Default::default()));
    settle(&mut harness);

    // Default card, then Casing, then Space Trim: each inserted after the one
    // that was open.
    let names: Vec<&str> = harness
        .state()
        .stack()
        .cards()
        .iter()
        .map(|c| c.op.name())
        .collect();
    assert_eq!(names, ["replace", "casing", "space_trim"]);

    // And the newest is the one whose editor is showing.
    harness.get_by_label_contains("Remove leading spaces");
}

/// Drag-reorder, driven through real pointer events over the card's handle.
#[test]
fn dragging_a_card_by_its_handle_reorders_the_stack() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Casing(Casing::new(CaseMode::Title));
    harness
        .state_mut()
        .add_operation(OpKind::Replace(Replace::new("_", " and ")));
    settle(&mut harness);
    assert_eq!(harness.state().stack().get(0).unwrap().op.name(), "casing");

    let handles: Vec<egui::Rect> = harness
        .get_all_by_label("Drag to reorder")
        .map(|node| node.rect())
        .collect();
    assert_eq!(handles.len(), 2, "one drag handle per card");

    // Press on the first handle, move well past egui's click threshold, drop on
    // the second card.
    let from = handles[0].center();
    let to = handles[1].center();
    harness.drag_at(from);
    harness.run();
    harness.hover_at(from + egui::vec2(0.0, 12.0));
    harness.run();
    harness.hover_at(to);
    harness.run();
    harness.drop_at(to);
    settle(&mut harness);

    assert_eq!(
        harness.state().stack().get(0).unwrap().op.name(),
        "replace",
        "the first card was dragged below the second"
    );
}

// --- M4: Settings and the pinned chips ---------------------------------------

/// The Batch Replace list is edited in Settings, and what it holds is what a
/// *new* card starts from.
#[test]
fn the_settings_window_edits_the_list_a_new_batch_replace_card_starts_from() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    let shipped = harness.state().batch_replace_rules().len();
    assert!(shipped > 1, "D27 ships a pre-loaded list");

    harness.get_by_label("Settings").click();
    settle(&mut harness);
    // Two now: the toolbar button that opened it, and the window's heading.
    assert!(
        harness.query_all_by_label_contains("Settings").count() >= 2,
        "the Settings window did not open"
    );
    harness.get_by_label_contains("rules, run top to bottom");

    // Delete one rule, then close.
    harness.get_all_by_label("✖").next().unwrap().click();
    settle(&mut harness);
    assert_eq!(harness.state().batch_replace_rules().len(), shipped - 1);

    harness.get_by_label("Close").click();
    settle(&mut harness);

    // A new card copies the edited list.
    harness
        .state_mut()
        .add_operation(OpKind::BatchReplace(Default::default()));
    settle(&mut harness);
    match &harness.state().stack().cards().last().unwrap().op {
        OpKind::BatchReplace(batch) => assert_eq!(batch.rules.len(), shipped - 1),
        other => panic!("expected a Batch Replace card, got {other:?}"),
    }
}

/// A card keeps its own rule list, so editing the default afterwards cannot
/// rewrite a pipeline that is already built — or a preset already saved.
#[test]
fn a_batch_replace_card_keeps_its_own_rules() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    harness
        .state_mut()
        .add_operation(OpKind::BatchReplace(Default::default()));
    settle(&mut harness);
    let card_rules = match &harness.state().stack().cards().last().unwrap().op {
        OpKind::BatchReplace(batch) => batch.rules.len(),
        other => panic!("expected a Batch Replace card, got {other:?}"),
    };

    // Now change the default list in Settings.
    harness.get_by_label("Settings").click();
    settle(&mut harness);
    harness.get_all_by_label("✖").next().unwrap().click();
    settle(&mut harness);
    harness.get_by_label("Close").click();
    settle(&mut harness);

    match &harness.state().stack().cards().last().unwrap().op {
        OpKind::BatchReplace(batch) => {
            assert_eq!(batch.rules.len(), card_rules, "the card is unchanged")
        }
        other => panic!("expected a Batch Replace card, got {other:?}"),
    }
}

/// The theme control moved out of the pipeline panel, which is now only the
/// pipeline.
#[test]
fn the_theme_control_lives_in_settings_now() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    assert!(
        harness.query_by_label("Dark").is_none(),
        "not in the main window any more"
    );

    harness.get_by_label("Settings").click();
    settle(&mut harness);
    harness.get_by_label("Appearance").click();
    settle(&mut harness);
    assert!(
        harness
            .query_by_label_contains("run top to bottom")
            .is_none(),
        "switching pages should replace the body, not stack it"
    );
    harness.get_by_label("Dark").click();
    settle(&mut harness);
    harness.get_by_label("Close").click();
    settle(&mut harness);
}

/// The two run-wide objects are chips that summarise themselves, and open their
/// dialog on a click.
#[test]
fn the_counter_and_parts_chips_summarise_and_open() {
    use ren_core::{CounterSetup, PartsSpec};

    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    harness.state_mut().settings_mut().counter = CounterSetup {
        start: 5,
        step: 2,
        auto_pad: false,
        pad: 3,
        ..Default::default()
    };
    harness.state_mut().settings_mut().parts = PartsSpec::new("<%1> - <%2>");
    settle(&mut harness);

    harness.get_by_label_contains("Counter setup: 5, step 2, pad 3");
    harness.get_by_label_contains("Setup parts: <%1> - <%2>");

    // And the chip opens the dialog behind it.
    harness.get_by_label_contains("Counter setup").click();
    settle(&mut harness);
    harness.get_by_label_contains("Start value");
    harness.get_by_label_contains("Example:");
}

// --- M4: presets in the app ---------------------------------------------------

/// Save, clear, load — the round trip the drawer exists for.
#[test]
fn a_preset_round_trips_through_the_drawer() {
    let fixture = Fixture::new(&["my_song.mp3"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    harness
        .state_mut()
        .add_operation(OpKind::Casing(Casing::new(CaseMode::Title)));
    settle(&mut harness);
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "My Song.mp3"
    );

    harness.state_mut().save_preset("Tidy up");
    settle(&mut harness);
    assert_eq!(harness.state().preset_names(), ["Tidy up"]);

    // Throw the pipeline away.
    harness.state_mut().delete_card(1);
    harness.state_mut().delete_card(0);
    settle(&mut harness);
    assert!(harness.state().stack().is_empty());

    harness.state_mut().open_presets();
    settle(&mut harness);
    assert!(
        harness.query_all_by_label_contains("Tidy up").count() >= 1,
        "the saved preset should be listed"
    );
    harness.get_by_label("Load").click();
    settle(&mut harness);

    assert_eq!(harness.state().stack().len(), 2);
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "My Song.mp3",
        "and it renames the same way it did before"
    );
}

/// The name box is seeded from the pipeline's name when the drawer opens, and
/// then it is the user's. It used to be re-seeded on every frame it was
/// empty, so clearing it was impossible: backspace to nothing, and the next
/// frame put the name back.
#[test]
fn the_preset_name_box_can_be_cleared() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    harness.state_mut().save_preset("Tidy up");
    settle(&mut harness);

    harness.state_mut().open_presets();
    settle(&mut harness);
    assert_eq!(
        harness.state().drawer().map(|d| d.new_name.as_str()),
        Some("Tidy up"),
        "seeded from the pipeline's name on open"
    );

    // The hint is only the label while the box is empty; find it by value.
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .find(|node| node.value().as_deref() == Some("Tidy up"))
        .expect("the name box")
        .focus();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.key_press(egui::Key::Backspace);
    harness.run();
    harness.run();
    assert_eq!(
        harness.state().drawer().map(|d| d.new_name.as_str()),
        Some(""),
        "and stays empty once cleared"
    );
}

/// The acceptance criterion's second half: *"presets persist and reload across
/// restarts"*. A second app over the same folder, sharing nothing else.
#[test]
fn a_preset_survives_a_restart() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let mut first = harness(fixture.app());

    *first.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    first.state_mut().save_preset("Underscores");
    settle(&mut first);
    drop(first);

    let mut second = harness(fixture.app());
    settle(&mut second);
    assert_eq!(second.state().preset_names(), ["Underscores"]);

    second.state_mut().open_presets();
    settle(&mut second);
    second.get_by_label("Load").click();
    settle(&mut second);
    assert_eq!(second.state().plan().unwrap().items[0].new_name, "a b.txt");
}

/// D34: Load replaces the pipeline and its run settings; Append adds only the
/// operations, and leaves the counter where it was.
#[test]
fn appending_a_preset_keeps_the_current_cards_and_settings() {
    use ren_core::CounterSetup;

    let fixture = Fixture::new(&["a_b.txt"]);
    let mut harness = harness(fixture.app());

    // Save a preset carrying a counter of its own.
    *harness.state_mut().operation_mut() = OpKind::Casing(Casing::new(CaseMode::Upper));
    harness.state_mut().settings_mut().counter = CounterSetup {
        start: 50,
        ..Default::default()
    };
    harness.state_mut().save_preset("Shouty");
    settle(&mut harness);

    // Start over with a different pipeline and a different counter.
    harness.state_mut().delete_card(0);
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    harness.state_mut().settings_mut().counter = CounterSetup {
        start: 1,
        ..Default::default()
    };
    settle(&mut harness);

    harness.state_mut().open_presets();
    settle(&mut harness);
    harness.get_by_label("Append").click();
    settle(&mut harness);

    assert_eq!(harness.state().stack().len(), 2, "kept what was there");
    assert_eq!(
        harness.state().settings().counter.start,
        1,
        "append must not import someone else's counter"
    );
    assert_eq!(harness.state().plan().unwrap().items[0].new_name, "A B.txt");

    // Load, on the other hand, takes the whole thing.
    harness.get_by_label("Load").click();
    settle(&mut harness);
    assert_eq!(harness.state().stack().len(), 1);
    assert_eq!(harness.state().settings().counter.start, 50);
}

/// Run must never apply the plan the *previous* pipeline produced.
#[test]
fn running_a_preset_uses_the_preset_and_not_what_was_on_screen() {
    let fixture = Fixture::new(&["a_b.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "+"));
    harness.state_mut().save_preset("Plus");
    settle(&mut harness);

    // Something quite different on screen.
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "!"));
    settle(&mut harness);
    assert_eq!(harness.state().plan().unwrap().items[0].new_name, "a!b.txt");

    harness.state_mut().open_presets();
    settle(&mut harness);
    harness.get_by_label("Run").click();
    settle(&mut harness);

    assert_eq!(
        fixture.names(),
        ["a+b.txt"],
        "the preset ran, not the pipeline it replaced"
    );
}

/// `<Ask>` prompts at run time, and a preset that no longer asks must not reuse
/// an answer given for a question that is gone.
#[test]
fn an_ask_answer_is_forgotten_when_the_pipeline_stops_asking() {
    use ren_core::Answers;

    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() =
        OpKind::AddRemove(AddRemove::add("<Ask>", 0).backwards(true));
    let mut answers = Answers::default();
    answers.asks.insert(0, "-done".into());
    harness.state_mut().set_answers(answers);
    settle(&mut harness);
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "one-done.txt"
    );

    // Replace the card with one that does not ask.
    harness.state_mut().delete_card(0);
    harness
        .state_mut()
        .add_operation(OpKind::Casing(Casing::new(CaseMode::Upper)));
    settle(&mut harness);

    // Put an asking card back. The old answer must be gone, so pressing Rename
    // asks again instead of quietly using it.
    harness.state_mut().add_operation(OpKind::AddRemove(
        AddRemove::add("<Ask>", 0).backwards(true),
    ));
    settle(&mut harness);

    harness.state_mut().run_now();
    settle(&mut harness);
    harness.get_by_label("Enter text");
    assert_eq!(fixture.names(), ["one.txt"], "nothing renamed unasked");
}

/// A preset file that will not load is named rather than hidden, and it must
/// not stop the drawer opening.
#[test]
fn an_unreadable_preset_is_listed_with_its_error() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    harness.state_mut().save_preset("Good one");
    settle(&mut harness);
    let dir = fixture.journal.path().join("presets");
    std::fs::write(dir.join("broken.toml"), "this is not = toml [").unwrap();

    harness.state_mut().open_presets();
    settle(&mut harness);

    assert!(
        harness.query_all_by_label_contains("Good one").count() >= 1,
        "the good one still lists"
    );
    assert!(
        harness.query_all_by_label_contains("broken.toml").count() >= 1,
        "the file that will not load should be named"
    );
}

/// The per-keystroke budget with a pipeline the size M4 makes easy to build.
///
/// The existing budget test uses one operation; the CI spike shares no code
/// with the app at all. Neither would notice an eight-card stack rebuilding
/// every transform and recompiling every template on each keystroke, which is
/// precisely what M4 introduced.
#[test]
fn an_eight_card_pipeline_keeps_up_with_typing_over_five_thousand_files() {
    use ren_core::ops::{FreeFormat, SpaceTrim, ZeroPadding};

    let dir = TempDir::new().unwrap();
    for i in 0..5_000 {
        std::fs::write(dir.path().join(format!("track_{i:05} live.mp3")), b"").unwrap();
    }
    let journal = TempDir::new().unwrap();
    let mut app = RenameItApp::headless(dir.path().to_path_buf(), journal.path().to_path_buf());

    // Eight cards, including the two most expensive kinds: Batch Replace with
    // its fifty-one rules, and a template that has to be compiled.
    app.add_operation(OpKind::BatchReplace(Default::default()));
    app.add_operation(OpKind::Casing(Casing::new(CaseMode::Title)));
    app.add_operation(OpKind::Casing(Casing::new(CaseMode::Lower)));
    app.set_card_scope(3, ren_core::model::Scope::Extension);
    app.add_operation(OpKind::SpaceTrim(SpaceTrim::default()));
    app.add_operation(OpKind::ZeroPadding(ZeroPadding::new(6)));
    app.add_operation(OpKind::AddCounter(AddCounter::new(
        CounterPlacement::First,
        ". ",
    )));
    app.add_operation(OpKind::FreeFormat(FreeFormat::new("<Counter> <Name>")));
    app.settle();
    assert_eq!(app.stack().len(), 8);

    let mut worst = std::time::Duration::ZERO;
    for n in 1..="Holiday".len() {
        let started = std::time::Instant::now();
        *app.operation_mut() = OpKind::Replace(Replace::new("_", &"Holiday"[..n]));
        app.settle();
        worst = worst.max(started.elapsed());
    }

    assert!(
        app.plan().is_some_and(|p| p.changed() == 5_000),
        "every row should have changed"
    );
    assert!(
        worst < std::time::Duration::from_secs(3),
        "a keystroke took {worst:?} over 5 000 files with eight cards"
    );
}

/// D32 says a preset stores exactly what the card shows. A pre-processor can
/// arrive from a hand-written job file imported as a preset, and there is no
/// one from a hand-written job file, and until M8 there was no editor — so the
/// card had to say it was there and let it go. Both halves still hold now that
/// the editor exists: the header announces it, and unticking *Enable* takes it
/// off.
#[test]
fn a_pre_processor_that_came_from_a_file_is_shown_and_can_be_cleared() {
    use ren_core::{PreProcessor, StepConfig};

    let fixture = Fixture::new(&["Batch Renamer is fantastic!.txt"]);
    let mut harness = harness(fixture.app());

    harness.state_mut().delete_card(0);
    harness.state_mut().stack_mut().append_steps(vec![(
        OpKind::Casing(Casing::new(CaseMode::Upper)),
        StepConfig {
            preproc: Some(PreProcessor::new().skipping_first(14)),
            ..Default::default()
        },
    )]);
    harness.state_mut().expand_card(0);
    settle(&mut harness);

    // It is doing something, and it says so.
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "Batch Renamer IS FANTASTIC!.txt"
    );
    // The collapsed header carries it, because Scope is closed by default and a
    // card that silently narrows what it sees is the hardest thing to spot.
    harness.get_by_label_contains("pre-processor: skip the first 14");
    harness.get_by_label_contains("Scope").click();
    settle(&mut harness);

    harness.get_by_label("Enable Pre-Processor").click();
    settle(&mut harness);
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        "BATCH RENAMER IS FANTASTIC!.txt",
        "clearing it hands the whole name back to the operation"
    );
    // And a preset saved now would not carry it.
    assert_eq!(harness.state().stack().to_steps()[0].1.preproc, None);
}

/// The pre-processor filters out a section of the filename.
///
/// The section opens with an *Enable Pre-Processor* checkbox, and until M8 the
/// only way to get one was hand-writing a job file: `preproc_ui`
/// returned early when there was none, so the whole section was invisible.
/// Enabling one must change nothing on its own — it is the identity until a
/// stage is armed.
#[test]
fn enabling_a_pre_processor_from_the_card_starts_it_as_the_identity() {
    // A **lower-case** initial, deliberately: with `Batch…` an accidental
    // `skip_first(1)` produces the identical all-caps result, so the test would
    // pass against a pre-processor that was not the identity at all.
    let fixture = Fixture::new(&["batch renamer is fantastic!.txt"]);
    let mut harness = tall_harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Casing(Casing::new(CaseMode::Upper));
    settle(&mut harness);

    let all_caps = "BATCH RENAMER IS FANTASTIC!.txt";
    assert_eq!(harness.state().plan().unwrap().items[0].new_name, all_caps);

    harness.get_by_label_contains("Scope").click();
    settle(&mut harness);
    harness.get_by_label("Enable Pre-Processor").click();
    settle(&mut harness);

    let made = harness.state().stack().to_steps()[0].1.preproc.clone();
    assert!(
        made.as_ref()
            .is_some_and(ren_core::PreProcessor::is_identity),
        "enabling arms nothing: {made:?}"
    );
    assert_eq!(
        harness.state().plan().unwrap().items[0].new_name,
        all_caps,
        "an enabled pre-processor with no stage armed narrows nothing"
    );
    // And the fields it opened are reachable.
    harness.get_by_label("Skip the first");
    harness.get_by_label("Cut if string is found:");
}

/// An `Option` that forgets is an editor that eats your work on a mis-click —
/// and unticking a stage to see the difference is the normal way to use one.
#[test]
fn unticking_a_pre_processor_stage_and_ticking_it_again_brings_back_what_was_typed() {
    use ren_core::{PreProcessor, StepConfig};

    let fixture = Fixture::new(&["Artist - Title.txt"]);
    let mut harness = tall_harness(fixture.app());
    harness.state_mut().delete_card(0);
    harness.state_mut().stack_mut().append_steps(vec![(
        OpKind::Casing(Casing::new(CaseMode::Upper)),
        StepConfig {
            preproc: Some(
                PreProcessor::new().cutting_at(ren_core::MatchSpec::Substring(" - ".to_owned())),
            ),
            ..Default::default()
        },
    )]);
    harness.state_mut().expand_card(0);
    settle(&mut harness);

    harness.get_by_label_contains("Scope").click();
    settle(&mut harness);

    harness.get_by_label("Cut if string is found:").click();
    settle(&mut harness);
    assert_eq!(
        harness.state().stack().to_steps()[0]
            .1
            .preproc
            .as_ref()
            .unwrap()
            .cut_at,
        None,
        "unticked"
    );

    harness.get_by_label("Cut if string is found:").click();
    settle(&mut harness);
    assert_eq!(
        harness.state().stack().to_steps()[0]
            .1
            .preproc
            .as_ref()
            .unwrap()
            .cut_at
            .as_ref()
            .map(|s| s.text().to_owned()),
        Some(" - ".to_owned()),
        "and the box still holds what was in it"
    );
}

/// *Regular expression* is one switch over all three searches, as
/// `case_sensitive` already is. A mix is
/// reachable from a hand-written job file and one checkbox cannot say it, so it
/// shows grey — which egui puts into the accessibility tree as `Toggled::Mixed`
/// rather than picking an answer.
#[test]
fn a_pre_processor_that_is_regex_in_one_stage_and_not_another_shows_the_switch_grey() {
    use ren_core::{MatchSpec, PreProcessor, StepConfig};

    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    harness.state_mut().delete_card(0);
    let mut preproc = PreProcessor::new();
    preproc.cut_at = Some(MatchSpec::Substring("-".to_owned()));
    preproc.section = Some(MatchSpec::Regex("a.*".to_owned()));
    harness.state_mut().stack_mut().append_steps(vec![(
        OpKind::Casing(Casing::new(CaseMode::Upper)),
        StepConfig {
            preproc: Some(preproc),
            ..Default::default()
        },
    )]);
    harness.state_mut().expand_card(0);
    settle(&mut harness);

    harness.get_by_label_contains("Scope").click();
    settle(&mut harness);

    assert_eq!(
        harness
            .get_by_label("Regular expression")
            .accesskit_node()
            .toggled(),
        Some(egui::accesskit::Toggled::Mixed),
        "a mix shows grey rather than being normalised"
    );
}

// --- M5: Set Attributes ------------------------------------------------------

/// The grey state has to *reach* the accessibility tree, or it is a painted
/// dash that no screen reader can announce and no test can read. egui maps an
/// indeterminate checkbox to `Toggled::Mixed`, which is why the widget is built
/// on `Checkbox::indeterminate` rather than drawn by hand.
#[test]
fn a_set_attributes_card_shows_four_tri_state_boxes_that_all_start_grey() {
    use egui::accesskit::Toggled;

    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::SetAttributes(Default::default()));
    settle(&mut harness);

    for label in ["Write Protect", "Hidden", "System", "Archive"] {
        let node = harness.get_by_label(label);
        assert_eq!(
            node.accesskit_node().toggled(),
            Some(Toggled::Mixed),
            "{label} should start grey — the 'leave it unchanged' state"
        );
    }
}

/// The grid is drawn column-major so it reads like the dialog — Write Protect
/// above Hidden, System above Archive — and each box still writes through to
/// its **own** bit.
///
/// The second half is the one worth having. The grid walks `op.bits()` out of
/// order now, and `bits()` hands out `&mut` into the operation; getting that
/// wrong would leave four checkboxes that move and change nothing, which no
/// presence assertion would catch.
///
/// **It only discriminates on Windows**, and says so rather than pretending
/// otherwise: on Unix exactly one attribute is supported (D40), so every other
/// box is deliberately disabled and unclickable, and wiring all four to the
/// same bit is indistinguishable from wiring them correctly. CI's
/// `windows-latest` job is where this bites. Verified by mutation there being
/// the only thing that would prove it — locally, reverting the fix leaves this
/// green.
#[test]
fn each_attribute_box_writes_through_to_its_own_bit() {
    use egui::accesskit::Toggled;

    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::SetAttributes(Default::default()));
    settle(&mut harness);

    // Only the boxes this platform supports are clickable — on Unix that is
    // Write Protect alone (D40), and the rest are deliberately disabled. Click
    // in a different order from the draw order, which is the thing at risk.
    let clickable: Vec<&str> = ["System", "Write Protect", "Archive", "Hidden"]
        .into_iter()
        .filter(|label| !harness.get_by_label(label).accesskit_node().is_disabled())
        .collect();
    assert!(
        !clickable.is_empty(),
        "every attribute is disabled here, so this test proves nothing"
    );
    for label in &clickable {
        harness.get_by_label(label).click();
        settle(&mut harness);
    }

    for label in &clickable {
        assert_ne!(
            harness.get_by_label(label).accesskit_node().toggled(),
            Some(Toggled::Mixed),
            "{label} did not move — its box is wired to the wrong bit, or to none"
        );
    }
}

/// Clearing write protection is one click on the grey box, not a cycle
/// through every state.
#[test]
fn unchecking_write_protect_is_one_click_from_grey() {
    use egui::accesskit::Toggled;

    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::SetAttributes(Default::default()));
    settle(&mut harness);

    harness.get_by_label("Write Protect").click();
    settle(&mut harness);

    assert_eq!(
        harness
            .get_by_label("Write Protect")
            .accesskit_node()
            .toggled(),
        Some(Toggled::False),
        "one click from grey clears it"
    );
    for label in ["Hidden", "System", "Archive"] {
        assert_eq!(
            harness.get_by_label(label).accesskit_node().toggled(),
            Some(Toggled::Mixed),
            "{label} must be left alone"
        );
    }

    // By kind, not by position: the app starts with a default Replace card, so
    // an added one is never at index 0.
    let op = harness
        .state()
        .stack()
        .cards()
        .iter()
        .find_map(|card| match &card.op {
            OpKind::SetAttributes(op) => Some(*op),
            _ => None,
        })
        .expect("the card we added");
    assert_eq!(op.read_only, Some(false));
    assert_eq!(op.hidden, None, "the other three are left alone");
}

/// D40: every bit is shown on every platform, with the ones this machine
/// cannot write disabled and explained. One body, meaningful on both runners.
#[test]
fn the_attributes_this_platform_cannot_write_are_shown_greyed_with_a_reason() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::SetAttributes(Default::default()));
    settle(&mut harness);

    let platform = ren_platform::host();
    let dos = [
        ("Hidden", ren_platform::Capability::HiddenAttribute),
        ("System", ren_platform::Capability::SystemAttribute),
        ("Archive", ren_platform::Capability::ArchiveAttribute),
    ];
    for (label, capability) in dos {
        // Present either way — that is the point of showing all four.
        let node = harness.get_by_label(label);
        assert_eq!(
            node.accesskit_node().is_disabled(),
            !platform.supports(capability),
            "{label} should be enabled exactly when this platform can write it"
        );
    }

    if !platform.supports(ren_platform::Capability::HiddenAttribute) {
        assert!(
            harness
                .query_all_by_label_contains("Windows-only")
                .next()
                .is_some(),
            "a greyed box owes the user a reason"
        );
    }
}

/// The two silent liars, in one run: the row must not be dimmed as unchanged,
/// and the button must not claim nothing would change.
#[test]
fn an_action_only_pipeline_shows_what_it_will_do_and_does_not_block_the_run() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let mut harness = harness(fixture.app());
    harness
        .state_mut()
        .add_operation(OpKind::SetAttributes(ren_core::ops::SetAttributes {
            read_only: Some(true),
            ..Default::default()
        }));
    settle(&mut harness);

    assert_eq!(
        harness.state().blocked_reason(),
        None,
        "an action-only pipeline is perfectly runnable"
    );
    // D38: the New name column says what the run does to the row.
    assert!(
        harness
            .query_all_by_label_contains("Write Protect on")
            .next()
            .is_some(),
        "the row should say what will happen to it"
    );
    assert!(
        harness.query_by_label("unchanged").is_none(),
        "no row is untouched"
    );
}

/// A run that can put nothing back must not arm Undo. Pressing it would restore
/// nothing, report success, and push the last batch that *could* be undone out
/// of reach — so the cost is not a wasted click but a lost one.
#[test]
fn a_run_that_only_wrote_tags_does_not_arm_undo() {
    use ren_core::meta::testing::Mp3;

    let fixture = Fixture::music(&[("song.mp3", Mp3::tagged("Keep", "Me"))]);
    let mut harness = harness(fixture.app());

    // A real rename first, so there is something worth protecting.
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("song", "tune"));
    settle(&mut harness);
    harness.get_by_label("Rename").click();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["tune.mp3"]);

    // Then a tag-only run.
    *harness.state_mut().operation_mut() = OpKind::RemoveTags(ren_core::ops::RemoveTags {
        id3v2: true,
        ..Default::default()
    });
    settle(&mut harness);
    harness.state_mut().run_allowing_irreversible();
    settle(&mut harness);
    ren_core::meta::audio::forget_all();
    assert!(
        ren_core::meta::audio::tags_of(&fixture.dir.path().join("tune.mp3"))
            .unwrap()
            .artist
            .is_none(),
        "the tag really was removed"
    );

    // Undo must reach past it to the rename, not spend itself on the strip.
    harness.state_mut().undo_now();
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["song.mp3"],
        "undo should have reached the rename underneath"
    );
}

/// After a crash, the one file whose contents may be half-written is the one
/// the banner has to name — and its path was in the journal the whole time.
#[test]
fn the_recovery_banner_names_the_file_that_was_being_written() {
    use ren_core::exec::{Journal, Record};

    let fixture = Fixture::new(&["song.mp3"]);
    let mut journal = Journal::create(fixture.journal.path()).unwrap();
    journal
        .write(Record::Begin {
            platform: "test".into(),
            items: 1,
        })
        .unwrap();
    // Announced, never confirmed: the process died mid-write.
    journal
        .write(Record::PlanIrreversible {
            seq: 0,
            path: fixture.dir.path().join("song.mp3"),
            op: "music_tagger".into(),
            change: ren_core::Effect::RemoveTags {
                kinds: vec![ren_core::meta::write::TagKind::Id3v2],
            },
        })
        .unwrap();
    drop(journal);

    let harness = harness(fixture.app());
    harness.get_by_label_contains("did not finish");
    // Not "rename batch(es)" — the wrong noun for a run that wrote tags.
    assert!(
        harness.query_by_label_contains("rename batch").is_none(),
        "a tag write is not a rename"
    );
    harness.get_by_label_contains("song.mp3 — music_tagger was interrupted");
    harness.get_by_label_contains("may be half-written");
}

// --- M8: the rest of the hotkeys ---------------------------------------------

/// F8 opens the settings window.
#[test]
fn f8_opens_the_settings_window() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());

    assert!(
        harness
            .query_by_label_contains("rules, run top to bottom")
            .is_none(),
        "Settings starts closed"
    );

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label_contains("rules, run top to bottom");
}

/// F4 is a shortcut for the Undo button.
///
/// Ctrl+Z stays what it always was; F4 is the muscle memory somebody arrives
/// with. Both reach the same method, so the test that matters is that F4 does.
#[test]
fn f4_undoes_the_last_batch() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("o", "0"));
    settle(&mut harness);

    harness.key_press(egui::Key::F5);
    settle(&mut harness);
    assert_eq!(fixture.names(), ["0ne.txt", "tw0.txt"]);

    harness.key_press(egui::Key::F4);
    settle(&mut harness);
    assert_eq!(fixture.names(), ["one.txt", "two.txt"]);
}

/// F6 puts the caret in the address box, so a new path can be typed.
///
/// Asserted through focus rather than through what a keystroke lands in: the
/// point of the key is where the caret goes.
#[test]
fn f6_focuses_the_address_box() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());

    harness.key_press(egui::Key::F6);
    settle(&mut harness);

    let focused = harness.ctx.memory(|m| m.focused());
    assert_eq!(
        focused,
        Some(ren_gui::panels::source_bar::address_box()),
        "F6 did not put the caret in the address box"
    );
}

/// F12 opens the folder picker to choose a new folder (Browser mode only).
///
/// The picker is behind the `FileDialogs` trait, so this supplies one that
/// answers — `NoDialogs` cancels everything, which would make a green test
/// that proved only that nothing happened.
#[test]
fn f12_opens_the_folder_browser_and_lists_what_it_returns() {
    #[derive(Debug)]
    struct PicksThis(std::path::PathBuf);

    impl ren_gui::dialogs::FileDialogs for PicksThis {
        fn pick_folder(&self, _start: &std::path::Path) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
        fn pick_files(&self) -> Option<Vec<std::path::PathBuf>> {
            None
        }
        fn open_preset(&self) -> Option<std::path::PathBuf> {
            None
        }
        fn save_preset(&self, _suggested: &str) -> Option<std::path::PathBuf> {
            None
        }
        fn open_csv(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    let here = Fixture::new(&["one.txt"]);
    let there = Fixture::new(&["elsewhere.txt"]);
    let app = RenameItApp::headless_with_dialogs(
        here.dir.path().to_path_buf(),
        here.journal.path().to_path_buf(),
        Box::new(PicksThis(there.dir.path().to_path_buf())),
    );
    let mut harness = harness(app);
    harness.get_by_label_contains("one.txt");

    harness.key_press(egui::Key::F12);
    settle(&mut harness);
    harness.get_by_label_contains("elsewhere.txt");
}

// --- M8: the file list's right-click menu ------------------------------------

/// The row menu's Add to Free Select — through the real menu, since a
/// right-click is something the harness can actually do.
#[test]
fn the_row_menu_adds_files_to_free_select() {
    use ren_gui::viewmodel::SourceMode;

    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    assert_eq!(harness.state().session().settings.mode, SourceMode::Browser);

    harness.get_by_label_contains("one.txt").click_secondary();
    settle(&mut harness);
    harness.get_by_label("Add to Free Select").click();
    settle(&mut harness);

    let session = harness.state().session();
    assert_eq!(session.settings.mode, SourceMode::FreeSelect);
    assert_eq!(session.entries().len(), 1, "only the row that was clicked");
    assert_eq!(session.entries()[0].file_name, "one.txt");
}

/// A right-click on a row that is not selected selects it first — or "copy the
/// selection" would copy something the user was not pointing at.
#[test]
fn right_clicking_an_unselected_row_selects_it() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());

    harness.get_by_label_contains("two.txt").click_secondary();
    settle(&mut harness);
    assert_eq!(
        harness
            .state()
            .session()
            .selection
            .iter()
            .collect::<Vec<_>>(),
        [1]
    );
}

/// Copy to clipboard ▸ New names — how a listing gets out of the app and into
/// a text editor.
///
/// The text, not the clipboard: `arboard` needs a display server, and CI has
/// none. What is left to a human is `set_text`.
#[test]
fn copying_previews_produces_one_row_per_line() {
    use ren_gui::panels::rows::CopyWhat;

    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("o", "0"));
    settle(&mut harness);

    // No selection means every row, the same rule the run itself follows.
    assert_eq!(
        harness.state().rows_as_text(CopyWhat::NewNames, &[]),
        "0ne.txt\ntw0.txt\n"
    );
    assert_eq!(
        harness.state().rows_as_text(CopyWhat::Names, &[]),
        "one.txt\ntwo.txt\n"
    );
    assert_eq!(
        harness.state().rows_as_text(CopyWhat::Both, &[1]),
        "two.txt\ttw0.txt\n",
        "and a selection narrows it"
    );
    assert!(
        harness
            .state()
            .rows_as_text(CopyWhat::Paths, &[0])
            .trim_end()
            .ends_with("one.txt")
    );
}

/// About shows the version, the licence, where the code lives, and what this
/// session did.
#[test]
fn about_opens_and_reports_what_the_session_did() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());

    harness.get_by_label("About RenameIt").click();
    settle(&mut harness);
    harness.get_by_label_contains("Nothing renamed yet this session");
    harness.get_by_label_contains("MIT licensed");
    harness.get_by_label_contains(&format!("v{}", env!("CARGO_PKG_VERSION")));
    harness.get_by_label("Close").click();
    settle(&mut harness);
    assert!(harness.query_by_label_contains("MIT licensed").is_none());

    // Rename something, and it says so.
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("o", "0"));
    settle(&mut harness);
    harness.key_press(egui::Key::F5);
    settle(&mut harness);

    harness.get_by_label("About RenameIt").click();
    settle(&mut harness);
    harness.get_by_label_contains("Renamed 2 items, over 1 run this session");
}

// --- M8: the Settings pages M8 added -----------------------------------------

/// A name the listing could not read as text is shown as such, and F2 fixes it.
///
/// The engine leaves such a row alone (`ren-core`'s `non_utf8_names` suite has
/// the why). This is the half a user meets: the row must not claim to be
/// "unchanged", because that reads as "the pipeline had no effect" rather than
/// "this was never a candidate" — and the way out has to be reachable.
#[cfg(unix)]
#[test]
fn a_name_that_is_not_text_says_so_and_can_still_be_renamed() {
    use std::os::unix::ffi::OsStrExt as _;

    let fixture = Fixture::new(&["ordinary.txt"]);
    let odd = std::ffi::OsStr::from_bytes(b"caf\xE9.txt");
    std::fs::write(fixture.dir.path().join(odd), b"payload").unwrap();

    let mut harness = harness(fixture.app());
    // Within the stem, which is what the default scope covers.
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("ordinary", "renamed"));
    settle(&mut harness);

    // Said out loud, and not as "unchanged".
    harness.get_by_label_contains("name is not text");

    // The other file is untouched by the refusal — P63: its own row and no
    // others.
    harness.key_press(egui::Key::F5);
    settle(&mut harness);
    let mut names: Vec<Vec<u8>> = std::fs::read_dir(fixture.dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().as_bytes().to_vec())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![b"caf\xE9.txt".to_vec(), b"renamed.txt".to_vec()],
        "the odd name keeps its bytes; the ordinary one renamed"
    );
}

/// The columns take the width the window actually has.
///
/// Before `auto_size_mode`, `egui_table` laid the columns out once from a
/// content sizing pass on the first frame and never grew them again: at
/// 1920 pt the four default columns came out 85 / 64 / 48 / 101, using 298 pt
/// and leaving 1 230 — two thirds of the window — empty to their right, while
/// `renameit.exe` still collided with `unchanged`.
///
/// Measured through the header buttons, whose left edges are the column
/// boundaries: the header cell has no gutter of its own.
#[test]
fn the_columns_fill_the_pane_rather_than_hugging_their_contents() {
    const WINDOW: f32 = 1280.0;
    let fixture = Fixture::new(&["a-short-name.txt", "and-a-considerably-longer-one.txt"]);
    let mut harness = harness(fixture.app());
    settle(&mut harness);

    // The header row is the topmost thing in the table, which is what picks it
    // out from the cells and tooltips that carry the same words. "Name" is also
    // a substring of "New name", so ties on `top` break leftwards.
    let header_left = |h: &Harness<'_, RenameItApp>, label: &str| {
        h.get_all_by_label_contains(label)
            .map(|node| node.rect())
            .filter(|rect| rect.width() > 0.0)
            .min_by(|a, b| {
                a.top()
                    .total_cmp(&b.top())
                    .then(a.left().total_cmp(&b.left()))
            })
            .unwrap_or_else(|| panic!("no header matching {label}"))
            .left()
    };
    let name = header_left(&harness, "Name");
    let new_name = header_left(&harness, "New name");
    let size = header_left(&harness, "Size");
    let modified = header_left(&harness, "Modified");

    assert!(
        new_name - name >= 200.0,
        "the Name column is {} pt wide",
        new_name - name
    );
    assert!(
        size - new_name >= 200.0,
        "the New name column is {} pt wide",
        size - new_name
    );
    assert!(
        modified - size <= 130.0,
        "Size took {} pt, which is width the names wanted",
        modified - size
    );
    // The point of all of it: nothing is left over on the right. The bound is
    // the last column's own maximum width plus a gutter — `Modified` is capped
    // (`columns.rs::width_range`), so once it is at that cap the space "left
    // over" *is* the column. A looser number here would still catch the bug
    // this test was written for, which was 1 230 pt of genuinely empty pane.
    let last_column_cap = 196.0 + 24.0;
    assert!(
        WINDOW - modified <= last_column_cap,
        "the last column starts at {modified}, leaving {} pt of the window empty",
        WINDOW - modified
    );
}

/// The Settings window is the same size on every page, and Close stays put.
///
/// It used to be `set_width(620)` with no outer scroll area and no floor, so
/// its height was whatever the open page needed: Appearance came out 142 pt
/// and Casing Exceptions 562, and the Close button moved 420 pt up the screen
/// when you changed tab.
#[test]
fn the_settings_window_does_not_change_size_when_you_change_page() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    harness.key_press(egui::Key::F8);
    settle(&mut harness);

    let pages = [
        "Batch Replace",
        "Music Styles",
        "Casing Exceptions",
        "Display",
        "File System",
        "Startup",
        "Shell Integration",
        "Appearance",
        "Problem Solver",
    ];

    let mut seen: Vec<(&str, egui::Rect)> = Vec::new();
    for page in pages {
        // The rail entry, which is the leftmost node carrying the name — a
        // page's own body may repeat it.
        let rail = harness
            .get_all_by_label(page)
            .map(|node| node.rect())
            .min_by(|a, b| a.left().total_cmp(&b.left()))
            .expect("every page has a rail entry");
        harness
            .get_all_by_label(page)
            .find(|node| node.rect() == rail)
            .expect("the same node")
            .click();
        settle(&mut harness);
        seen.push((page, harness.get_by_label("Close").rect()));
    }

    let (first_page, first) = seen[0];
    for (page, close) in &seen[1..] {
        assert!(
            (close.min - first.min).length() < 1.0,
            "Close is at {:?} on {page} but {:?} on {first_page} — the window \
             is still resizing itself around whichever page is open",
            close.min,
            first.min
        );
    }
}

/// The interface size is a real control, and `Reset all settings` reaches it.
///
/// The reset is the half worth testing: the size lives in egui's own `Memory`
/// rather than in `Persisted`, so the reset — which walks `Persisted` field by
/// field — cannot pick it up by default the way every other setting does.
#[test]
fn the_interface_size_scales_the_window_and_the_reset_puts_it_back() {
    let fixture = Fixture::new(&["one.txt"]);
    // Tall, because the second half of this test drives the reset through the
    // Problem Solver page *while zoomed in* — and zooming shrinks the window
    // in points, so the modal body gets shorter and its buttons go below the
    // fold. A synthetic click lands at a screen position, not on a node.
    let mut harness = tall_harness(fixture.app());
    assert_eq!(harness.ctx.zoom_factor(), 1.0, "100% to begin with");

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Appearance").click();
    settle(&mut harness);
    harness.get_by_label("150%").click();
    settle(&mut harness);

    assert_eq!(harness.ctx.zoom_factor(), 1.5);

    // And the window really is laid out at the new scale: everything is
    // measured in points, and a point is now 1.5 pixels, so the *available*
    // space in points shrinks by the same factor.
    let content = harness.ctx.content_rect();
    assert!(
        content.width() < 1280.0,
        "the window is still {} pt wide, so the scale did not reach layout",
        content.width()
    );

    // Through the real path — Problem Solver, then the confirmation — rather
    // than a test hook, because the reset is the half that can silently miss.
    harness.get_by_label("Problem Solver").click();
    settle(&mut harness);
    harness.get_by_label("Reset all settings…").click();
    settle(&mut harness);
    harness.get_by_label("Restore every setting").click();
    settle(&mut harness);

    assert_eq!(
        harness.ctx.zoom_factor(),
        1.0,
        "a reset that misses the interface size leaves the user stuck at it"
    );
}

/// Settings ▸ Display edits the column list, and the table follows.
#[test]
fn the_display_page_turns_a_column_on_and_the_table_shows_it() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    assert!(
        harness.query_by_label("Ext").is_none(),
        "Ext is off to start with"
    );

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Display").click();
    settle(&mut harness);
    // Two nodes carry the label while Settings is open: the checkbox and the
    // text beside it. The first is the one that toggles.
    harness.get_all_by_label("Ext").next().unwrap().click();
    settle(&mut harness);
    harness.get_by_label("Close").click();
    settle(&mut harness);

    // The header is back, and so is a cell carrying the extension on its own —
    // "txt" beside the row's own "one.txt".
    assert!(harness.get_all_by_label("Ext").count() >= 1);
    assert!(harness.get_all_by_label("txt").count() >= 1, "the cell");
}

/// The Name column cannot be turned off: it is what identifies a row.
#[test]
fn the_name_column_cannot_be_turned_off() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Display").click();
    settle(&mut harness);

    assert!(
        harness
            .get_all_by_label("Name")
            .next()
            .unwrap()
            .accesskit_node()
            .is_disabled(),
        "the checkbox that would remove it is disabled"
    );
}

/// The system-folder guard blocks the run and says why (D127).
///
/// Driven through the setting rather than by pointing the app at `/usr`: the
/// guard is what is under test, not whether this machine has one.
#[test]
fn a_guarded_folder_blocks_the_run_with_a_reason() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("o", "0"));
    settle(&mut harness);
    assert_eq!(harness.state().blocked_reason(), None);

    // Point it at somewhere the OS owns.
    harness
        .state_mut()
        .set_dir(std::path::PathBuf::from(GUARDED_DIR));
    settle(&mut harness);
    let reason = harness.state().blocked_reason().expect("the guard blocks");
    assert!(reason.contains("operating system"), "{reason}");
    assert!(reason.contains("undo cannot fix"), "{reason}");

    // And it is a decision the user can take.
    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("File System").click();
    settle(&mut harness);
    harness
        .get_by_label_contains("Refuse to rename inside operating-system folders")
        .click();
    settle(&mut harness);
    harness.get_by_label_contains("⚠ The guard is off");
    harness.get_by_label("Close").click();
    settle(&mut harness);

    // Blocked for an ordinary reason now — there is nothing to rename there —
    // but no longer by the guard.
    let reason = harness.state().blocked_reason();
    assert!(
        !reason
            .as_deref()
            .unwrap_or_default()
            .contains("operating system"),
        "{reason:?}"
    );
}

#[cfg(unix)]
const GUARDED_DIR: &str = "/usr";
#[cfg(windows)]
const GUARDED_DIR: &str = r"C:\Windows";

/// Settings ▸ Casing Exceptions edits the list a *new* card copies, and never
/// a card that already exists (D35).
/// Exceptions are words that keep their own spelling, and the card has a link
/// to edit them.
///
/// The link edits **this card's** words. That is the whole reason it is not a
/// jump to Settings ▸ Casing Exceptions the way Music Rename's *(edit styles)*
/// is: **D66** makes that one deliberately card-independent because the card
/// keeps a *pattern*, while a Set Casing card keeps the words themselves. A
/// link to Settings would edit the copy a *new* card starts from and do nothing
/// visible to the card it was clicked on.
#[test]
fn a_set_casing_card_edits_its_own_exception_words() {
    use ren_core::ops::{CaseMode, Casing};

    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    let mut card = Casing::new(CaseMode::Title);
    assert!(
        !card.rules.exceptions.words.is_empty(),
        "a new card starts from the shipped list"
    );
    // Trimmed to two, so *Add word* is above the fold of the card panel's own
    // scroll area. With the shipped eighteen the button is in the accessibility
    // tree and outside the clip rect, so a synthetic click lands on nothing —
    // which is a fact about this harness, not about the widget.
    card.rules.exceptions.words = vec!["CD".to_owned(), "DJ".to_owned()];
    *harness.state_mut().operation_mut() = OpKind::Casing(card);
    settle(&mut harness);

    let words = |h: &Harness<'_, RenameItApp>| match &h.state().stack().to_steps()[0].0 {
        OpKind::Casing(c) => c.rules.exceptions.words.len(),
        other => panic!("{other:?}"),
    };
    let before = words(&harness);

    // Hidden until asked for: a card is a compact thing and the list is long.
    assert!(harness.query_by_label("+ Add word").is_none());
    harness.get_by_label("(edit exceptions)").click();
    settle(&mut harness);
    harness.get_by_label("+ Add word").click();
    settle(&mut harness);

    assert_eq!(words(&harness), before + 1, "the card's own list grew");

    // And it closes again, so the card does not stay tall forever.
    harness.get_by_label("(hide exceptions)").click();
    settle(&mut harness);
    assert!(harness.query_by_label("+ Add word").is_none());
}

#[test]
fn the_casing_exception_list_is_a_default_for_new_cards() {
    use ren_core::ops::{CaseMode, Casing};

    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Casing(Casing::new(CaseMode::Title));
    settle(&mut harness);

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Casing Exceptions").click();
    settle(&mut harness);
    // Two lists on the page, each with its own add button; either will do.
    harness
        .get_all_by_label("+ Add word")
        .next()
        .unwrap()
        .click();
    settle(&mut harness);
    harness.get_by_label("Close").click();
    settle(&mut harness);

    // The blank row the + added is dropped on close, so the default list is
    // unchanged — and the card that existed already never looked at it.
    let words = harness.state().casing_exception_words().len();
    assert!(words > 1, "the shipped list survives a stray click");
}

/// Settings ▸ Problem Solver has shortcuts to the folders the app keeps its
/// data in.
///
/// The shortcuts are the half that can go stale, so they are read from the same
/// functions the app uses rather than typed out. That is what this checks: the
/// journal path on the page is the journal the app is actually writing to.
#[test]
fn the_problem_solver_page_names_the_folders_the_app_really_uses() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Problem Solver").click();
    settle(&mut harness);

    harness.get_by_label("Undo journal");
    // The presets folder lives under the journal in a headless app, so the
    // journal path is a prefix of two rows. Both are real, and both being
    // there is the point.
    assert!(
        harness
            .get_all_by_label_contains(&fixture.journal.path().display().to_string())
            .count()
            >= 1
    );
    harness.get_by_label_contains("The Rename button is greyed out");
}

/// Settings ▸ Startup, end to end: the switch is stored, and a fresh app built
/// from that state honours it.
#[test]
fn the_startup_page_stores_a_switch_that_a_later_start_obeys() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Startup").click();
    settle(&mut harness);
    harness.get_by_label_contains("Clear the pipeline").click();
    settle(&mut harness);
    harness.get_by_label("Close").click();
    settle(&mut harness);

    // It is off by default and on now — the page changed real state rather
    // than drawing a checkbox bound to nothing.
    assert!(harness.state().startup().clear_pipeline);
}

/// The command line is a list of files, or a folder to start in — which is
/// how the Explorer entry invokes the app, and how a drop on the executable
/// arrives. A folder on the command line browses **whatever mode the session
/// is in**.
///
/// `Session::accept_dropped` only navigates when the mode is *already*
/// Browser — so with a session in Free Select the folder was added as a
/// single **row** instead of a listing of what is in it. A fresh session
/// starts in Browser now (`Session::new`), but `start_at` is also reached with
/// one already in Free Select; the existing tests miss it only because
/// `RenameItApp::headless` always starts in the default mode.
///
/// The rule belongs to `start_at`, not to `accept_dropped`: a *drag* of a
/// folder into Free Select is genuinely "add this to the list", and that
/// reading is the one the session doc records. A command line naming one
/// folder has never meant that.
#[test]
fn a_folder_on_the_command_line_browses_even_from_free_select() {
    use ren_gui::viewmodel::SourceMode;

    let here = Fixture::new(&["one.txt"]);
    let there = Fixture::new(&["elsewhere.txt", "another.txt"]);

    let mut app = here.app();
    // End the previous session in Free Select, as anyone who used it would.
    app.session_mut()
        .accept_dropped(vec![here.dir.path().join("one.txt")]);
    assert_eq!(
        app.session().settings.mode,
        SourceMode::FreeSelect,
        "the session this starts from"
    );

    app.start_at(vec![there.dir.path().to_path_buf()]);
    let browsing = harness(app);

    assert_eq!(
        browsing.state().session().settings.mode,
        SourceMode::Browser,
        "one folder on the command line is a folder to browse"
    );
    browsing.get_by_label_contains("elsewhere.txt");
    assert!(
        browsing.query_by_label_contains("one.txt").is_none(),
        "and the folder itself is not a row"
    );
}

#[test]
fn a_folder_on_the_command_line_is_where_the_app_opens() {
    use ren_gui::viewmodel::SourceMode;

    let here = Fixture::new(&["one.txt"]);
    let there = Fixture::new(&["elsewhere.txt", "another.txt"]);

    let mut app = here.app();
    app.start_at(vec![there.dir.path().to_path_buf()]);
    let browsing = harness(app);

    // One folder browses there, rather than putting the folder itself in a list.
    assert_eq!(
        browsing.state().session().settings.mode,
        SourceMode::Browser
    );
    browsing.get_by_label_contains("elsewhere.txt");

    // Several paths are Free Select, the same reading a drag already gets.
    let mut app = here.app();
    app.start_at(vec![
        there.dir.path().join("elsewhere.txt"),
        there.dir.path().join("another.txt"),
    ]);
    let free = harness(app);
    assert_eq!(free.state().session().settings.mode, SourceMode::FreeSelect);
    assert_eq!(free.state().session().entries().len(), 2);
}

/// A path the shell hands over that no longer exists is dropped, not reported.
/// An error dialog before the window has been seen is not a good greeting.
#[test]
fn a_path_that_is_gone_does_not_stop_the_app_starting() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut app = fixture.app();
    app.start_at(vec![std::path::PathBuf::from("/definitely/not/here")]);
    let harness = harness(app);

    harness.get_by_label_contains("one.txt");
    assert_eq!(harness.state().session().error, None);
}

/// The banner that explains what Windows would not hand over, and repairs it.
///
/// Driven through a `Launch` rather than a real command line, which is what
/// makes it testable at all: `Launch` carries the character count Windows
/// measured, so the Linux runner can pose the question Windows would have.
#[test]
fn a_selection_near_the_command_line_limit_offers_the_whole_folder() {
    let fixture = Fixture::new(&["one.txt", "two.txt", "three.txt"]);
    let mut app = fixture.app();
    // One file listed, as a right-click on one file would give — and the
    // folder holds three, so "list the whole folder" is visibly different.
    app.start_from(&ren_gui::launch::Launch {
        paths: vec![fixture.dir.path().join("one.txt")],
        from_shell: true,
        command_line_chars: Some(1_900),
        ..Default::default()
    });
    let mut harness = harness(app);

    harness.get_by_label_contains("2000 characters");
    // The number we were given, and no verb about what happened to it.
    harness.get_by_label_contains("1900");

    harness.get_by_label("List the whole folder").click();
    settle(&mut harness);

    let listed = harness.state().session().entries().len();
    assert_eq!(listed, 3, "the folder, not the selection");
    assert!(
        harness.query_by_label("List the whole folder").is_none(),
        "the repair takes the banner with it"
    );
}

/// A drag has no command line, so it must not be told about a command-line
/// limit — the banner's whole risk is being shown to people it does not apply
/// to.
#[test]
fn a_drag_of_many_files_is_told_nothing_about_a_limit() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut app = fixture.app();
    app.start_from(&ren_gui::launch::Launch {
        paths: vec![fixture.dir.path().join("one.txt")],
        from_shell: false,
        command_line_chars: Some(1_900),
        ..Default::default()
    });
    let harness = harness(app);

    assert!(harness.query_by_label_contains("2000 characters").is_none());
}

/// The arrow keys walk the file list.
#[test]
fn the_arrow_keys_walk_the_file_list() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = harness(fixture.app());

    harness.key_press(egui::Key::ArrowDown);
    settle(&mut harness);
    harness.key_press(egui::Key::ArrowDown);
    settle(&mut harness);

    let selection = &harness.state().session().selection;
    assert_eq!(selection.lead, Some(1));
    assert_eq!(
        selection.iter().collect::<Vec<_>>(),
        [1],
        "arrowing scopes the run, as a listview does"
    );
}

/// **The sharpest test in this commit.** P81 exempts a focused number box from
/// the hotkey guard, and that is safe because nothing on a spinner is spelled
/// F5 — but a spinner in text-entry mode *is* the arrows, Home, End and
/// Backspace. Folding the list keys into `handle_hotkeys` under that exemption
/// would make every position box on every card unusable.
#[test]
fn the_arrow_keys_leave_a_focused_spinner_alone() {
    use ren_core::ops::AddRemove;

    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::default());
    settle(&mut harness);

    harness.key_press(egui::Key::ArrowDown);
    settle(&mut harness);
    let before = harness.state().session().selection.lead;

    let spinner = harness
        .get_all_by_role(egui::accesskit::Role::SpinButton)
        .next()
        .expect("a position box");
    spinner.focus();
    settle(&mut harness);

    harness.key_press(egui::Key::ArrowDown);
    settle(&mut harness);
    assert_eq!(
        harness.state().session().selection.lead,
        before,
        "the spinner has the keyboard, so the list must not take the arrow"
    );

    // **`Ctrl+A` is the one the P81 exemption really endangers.** The arrows
    // are caught a second time by the focus check below it, so they survive
    // either guard; select-all runs *above* that check, so only the stricter
    // guard stops the list eating a spinner's own select-all.
    let scoped = harness.state().session().selection.len();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    settle(&mut harness);
    assert_eq!(
        harness.state().session().selection.len(),
        scoped,
        "Ctrl+A belongs to whatever has the keyboard"
    );
}

/// `Ctrl+A`, which the hotkey pass promised and nothing built —
/// scoped to **the rows on screen**, because the row filter's promise is that a
/// hidden row is gone and selecting one would rename a file the user cannot
/// see.
#[test]
fn ctrl_a_selects_every_row_the_list_is_showing() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = harness(fixture.app());

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    settle(&mut harness);
    assert_eq!(harness.state().session().selection.len(), 3);

    // Narrow to Changed with a pipeline that touches exactly one file. The
    // default scope is Name, so the subject is "a" rather than "a.txt".
    *harness.state_mut().operation_mut() = OpKind::Replace(ren_core::ops::Replace::new("a", "z"));
    harness.state_mut().session_mut().settings.row_filter = ren_gui::viewmodel::RowFilter::Changed;
    harness.state_mut().select(vec![]);
    settle(&mut harness);

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    settle(&mut harness);
    assert_eq!(
        harness.state().session().selection.len(),
        1,
        "the rows on screen, not every entry"
    );
}

/// Enter and Backspace walk the folder tree in Browser mode.
#[test]
fn enter_walks_into_the_folder_under_the_keyboard() {
    let fixture = Fixture::new(&["a.txt"]);
    std::fs::create_dir(fixture.dir.path().join("sub")).unwrap();
    std::fs::write(fixture.dir.path().join("sub/inside.txt"), b"x").unwrap();

    let mut app = fixture.app();
    app.session_mut().settings.folders = true;
    app.session_mut().request_refresh();
    let mut harness = harness(app);

    // Walk down to the folder row, then in.
    harness.key_press(egui::Key::ArrowDown);
    settle(&mut harness);
    while harness
        .state()
        .session()
        .selection
        .lead
        .and_then(|i| harness.state().session().entries().get(i))
        .is_some_and(|e| !e.is_dir)
    {
        harness.key_press(egui::Key::ArrowDown);
        settle(&mut harness);
    }

    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    assert!(harness.state().session().settings.dir.ends_with("sub"));
    harness.get_by_label_contains("inside.txt");
}

/// Backspace goes up, and lands on the folder it left — as Explorer does, and
/// **whatever the Folders chip says**, because a parent folder is not a row.
#[test]
fn backspace_goes_up_one_level_and_lands_on_the_folder_it_left() {
    let fixture = Fixture::new(&["a.txt"]);
    std::fs::create_dir(fixture.dir.path().join("sub")).unwrap();
    std::fs::write(fixture.dir.path().join("sub/inside.txt"), b"x").unwrap();

    let mut app = fixture.app();
    app.session_mut().settings.folders = true;
    app.set_dir(fixture.dir.path().join("sub"));
    let mut harness = harness(app);
    harness.get_by_label_contains("inside.txt");

    harness.key_press(egui::Key::Backspace);
    settle(&mut harness);

    assert_eq!(
        harness.state().session().settings.dir,
        fixture.dir.path(),
        "up one level"
    );
    let landed = harness
        .state()
        .session()
        .selection
        .lead
        .map(|i| harness.state().session().entries()[i].file_name.clone());
    assert_eq!(
        landed.as_deref(),
        Some("sub"),
        "on the folder it came out of"
    );
}

/// **The `is_dir` filter is load-bearing.** Without it `settings.dir` becomes
/// the path of a *file*, the next listing fails, and the table empties — from
/// one stray Enter.
#[test]
fn enter_on_a_file_does_not_go_anywhere() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let mut harness = harness(fixture.app());
    let before = harness.state().session().settings.dir.clone();

    harness.key_press(egui::Key::ArrowDown);
    settle(&mut harness);
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);

    assert_eq!(harness.state().session().settings.dir, before);
    assert_eq!(harness.state().session().entries().len(), 2);
}

/// Browser mode only — Free Select is a list of files from anywhere and has
/// no working path to change.
#[test]
fn neither_key_navigates_in_free_select() {
    let fixture = Fixture::new(&["a.txt"]);
    std::fs::create_dir(fixture.dir.path().join("sub")).unwrap();

    let mut app = fixture.app();
    app.session_mut().settings.folders = true;
    app.session_mut().request_refresh();
    app.session_mut().settings.mode = ren_gui::viewmodel::SourceMode::FreeSelect;
    let mut harness = harness(app);
    let before = harness.state().session().settings.dir.clone();

    harness.key_press(egui::Key::Backspace);
    settle(&mut harness);
    assert_eq!(harness.state().session().settings.dir, before);
}

/// The palette reads Enter without consuming it, and `handle_list_keys` runs
/// first — so one Enter must not both add an operation and change folder.
///
/// This one passes on `text_edit_focused()` alone, because the palette's search
/// box has the keyboard. The modal guard is what covers a window with **no**
/// focused field — see the Settings test below, which is the one that
/// discriminates.
#[test]
fn the_palette_keeps_its_own_enter() {
    let fixture = Fixture::new(&["a.txt"]);
    std::fs::create_dir(fixture.dir.path().join("sub")).unwrap();
    let mut app = fixture.app();
    app.session_mut().settings.folders = true;
    app.session_mut().request_refresh();
    let mut harness = harness(app);

    let before = harness.state().session().settings.dir.clone();
    let cards = harness.state().stack().to_steps().len();

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    settle(&mut harness);
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);

    assert_eq!(harness.state().stack().to_steps().len(), cards + 1);
    assert_eq!(
        harness.state().session().settings.dir,
        before,
        "the palette had the keyboard"
    );
}

/// **The test the modal guard is actually for.** A window can own the keyboard
/// without any text box holding it — Settings opens on a page whose only field
/// is unfocused — and `text_edit_focused()` says nothing about that. Without
/// `modal_is_up()`, Backspace typed at a settings window walks the folder tree
/// behind it.
#[test]
fn a_settings_window_with_nothing_focused_still_keeps_backspace() {
    let fixture = Fixture::new(&["a.txt"]);
    std::fs::create_dir(fixture.dir.path().join("sub")).unwrap();
    let mut app = fixture.app();
    app.set_dir(fixture.dir.path().join("sub"));
    let mut harness = harness(app);
    let before = harness.state().session().settings.dir.clone();

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.key_press(egui::Key::Backspace);
    settle(&mut harness);

    assert_eq!(
        harness.state().session().settings.dir,
        before,
        "the settings window had the keyboard"
    );
}

/// F2 finds where the name ends and the extension begins.
///
/// The stem is selected, so typing replaces the name and keeps the extension —
/// what Explorer does. `CCursorRange::two(0, boundary)` puts the primary
/// cursor *at* the boundary, so the caret is there too.
#[test]
fn f2_selects_the_name_so_typing_replaces_it_and_keeps_the_extension() {
    let fixture = Fixture::new(&["song.mp3"]);
    let mut harness = harness(fixture.app());
    harness.state_mut().select(vec![0]);
    settle(&mut harness);

    harness.key_press(egui::Key::F2);
    settle(&mut harness);
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| n.value().as_deref() == Some("song.mp3"))
        .expect("the inline editor")
        .type_text("track");
    settle(&mut harness);
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    assert_eq!(fixture.names(), ["track.mp3"]);
}

/// Enter renames the file and jumps to the next item in the list.
///
/// **By path, not by index.** `refresh()` re-lists and re-sorts, so renaming
/// `a.txt` to `zz.txt` moves it to the end and everything else up — an index
/// captured before the rename would open the editor on the wrong file.
#[test]
fn the_jump_follows_the_file_the_sort_puts_next_and_not_the_row_number() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = harness(fixture.app());

    harness.state_mut().rename_one(0, "zz.txt".to_owned());
    settle(&mut harness);

    let lead = harness.state().session().selection.lead.expect("a lead");
    assert_eq!(
        harness.state().session().entries()[lead].file_name,
        "b.txt",
        "the file that was next, wherever the re-sort put it"
    );
}

/// A rename is not the user choosing which files a run covers. Narrowing the
/// scope to one file would empty the New-name column for every other row.
#[test]
fn the_jump_does_not_narrow_what_the_run_covers() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Casing(Casing::new(CaseMode::Upper));
    settle(&mut harness);

    harness.state_mut().rename_one(0, "zz.txt".to_owned());
    settle(&mut harness);

    assert!(
        harness.state().session().selection.is_empty(),
        "the scope is untouched, so every row is still in the run"
    );
    assert_eq!(harness.state().plan().unwrap().items.len(), 3);
}

/// The last row has nowhere to jump to, and must not reopen on itself.
#[test]
fn the_last_row_has_nowhere_to_jump_to() {
    let fixture = Fixture::new(&["only.txt"]);
    let mut harness = harness(fixture.app());

    harness.state_mut().rename_one(0, "renamed.txt".to_owned());
    settle(&mut harness);

    // Other boxes exist — the address bar, the pattern field — so the question
    // is whether one of them is the inline editor holding this name.
    assert!(
        harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .all(|n| n.value().as_deref() != Some("renamed.txt")),
        "nothing to jump to, so no editor reopened"
    );
}

/// Dragging a file up or down the list changes the number a counter gives it,
/// end to end: the order **is** the run order, so a dragged row is numbered
/// where it was dropped.
#[test]
fn dragging_a_file_up_the_list_changes_the_number_the_counter_gives_it() {
    use ren_core::ops::AddCounter;

    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut app = fixture.app();
    *app.operation_mut() = OpKind::AddCounter(AddCounter::default());
    let mut harness = harness(app);

    assert_eq!(
        new_names(&harness),
        ["1-a.txt", "2-b.txt", "3-c.txt"],
        "list order is the numbering"
    );

    // The drop index is an **entry** index: put `c.txt` in front of `a.txt`.
    harness.state_mut().session_mut().move_rows(&[2], 0);
    settle(&mut harness);

    assert_eq!(new_names(&harness), ["1-c.txt", "2-a.txt", "3-b.txt"]);
}

/// **The only test that can catch a drop index taken from the visible row.**
///
/// With the Changed chip narrowing the table, the entry index and the row
/// number are different — and with the chip off they are equal, so every other
/// test passes against the wrong one.
#[test]
fn a_row_dropped_while_the_filter_hides_others_lands_where_it_looks_like_it_lands() {
    let fixture = Fixture::new(&["a.txt", "skip.txt", "c.txt"]);
    let mut app = fixture.app();
    // Touches `a.txt` and `c.txt`, leaves `skip.txt` alone.
    *app.operation_mut() = OpKind::Replace(ren_core::ops::Replace::new(".", "_"));
    let mut harness = harness(app);

    // Entries are a, c, skip in name order: a=0, c=1, skip=2.
    let names: Vec<String> = harness
        .state()
        .session()
        .entries()
        .iter()
        .map(|e| e.file_name.clone())
        .collect();
    let c_at = names.iter().position(|n| n == "c.txt").unwrap();
    let a_at = names.iter().position(|n| n == "a.txt").unwrap();

    harness.state_mut().session_mut().settings.row_filter = ren_gui::viewmodel::RowFilter::Changed;
    settle(&mut harness);

    harness.state_mut().session_mut().move_rows(&[c_at], a_at);
    settle(&mut harness);

    let after: Vec<String> = harness
        .state()
        .session()
        .entries()
        .iter()
        .map(|e| e.file_name.clone())
        .collect();
    assert_eq!(
        after[0], "c.txt",
        "the file the user dropped, not its neighbour"
    );
}

/// After a drag the listing is in nobody's column order, so no header may claim
/// it — the New name header's arrow is for the order **it** made.
#[test]
fn the_new_name_header_keeps_its_arrow_to_itself() {
    let fixture = Fixture::new(&["a.txt", "b.txt"]);
    let mut app = fixture.app();
    *app.operation_mut() = OpKind::Replace(ren_core::ops::Replace::new("a", "z"));
    let mut harness = harness(app);

    harness
        .query_all_by_label_contains("New name")
        .next()
        .expect("the column header")
        .click();
    settle(&mut harness);
    let arrows = |h: &Harness<'_, RenameItApp>| {
        h.query_all_by_label_contains("New name ▲").count()
            + h.query_all_by_label_contains("New name ▼").count()
    };
    assert!(arrows(&harness) > 0, "its own order gets the arrow");

    harness.state_mut().session_mut().move_rows(&[1], 0);
    settle(&mut harness);
    assert_eq!(
        arrows(&harness),
        0,
        "a dragged order is not the New name column's"
    );
}

/// A hand-set order survives the run that used it, always — not a setting:
/// the hand-set order **is** the run order, so losing it on
/// the run that used it would make dragging half a feature. A rename is not a
/// re-listing — it is a set of known old→new paths, which is what **D139**
/// already uses to rekey the pictures one line earlier.
#[test]
fn a_dragged_order_survives_the_run_that_used_it() {
    use ren_core::ops::AddCounter;

    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut app = fixture.app();
    *app.operation_mut() = OpKind::AddCounter(AddCounter::default());
    let mut harness = harness(app);

    // Put `c.txt` first by hand.
    harness.state_mut().session_mut().move_rows(&[2], 0);
    settle(&mut harness);
    assert_eq!(new_names(&harness), ["1-c.txt", "2-a.txt", "3-b.txt"]);

    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1-c.txt", "2-a.txt", "3-b.txt"]);

    let order: Vec<String> = harness
        .state()
        .session()
        .entries()
        .iter()
        .map(|e| e.file_name.clone())
        .collect();
    assert_eq!(
        order,
        ["1-c.txt", "2-a.txt", "3-b.txt"],
        "the order the user set, not the name order a relist would give"
    );
    assert!(harness.state().session().settings.sort.manual);
}

/// The Free Format box is a drop-down holding the patterns run before.
///
/// **Recorded when a run is committed, not per keystroke.** A history filled as
/// you type holds the prefixes of one string and nothing you would ever pick.
#[test]
fn a_pattern_reaches_the_history_when_it_is_run() {
    use ren_core::ops::FreeFormat;

    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() =
        OpKind::FreeFormat(FreeFormat::new("<Parent>_<FullName>"));
    settle(&mut harness);

    assert!(
        harness
            .state()
            .field_history("free_format_pattern")
            .is_empty(),
        "typing is not using"
    );

    harness.state_mut().run_now();
    settle(&mut harness);

    assert_eq!(
        harness.state().field_history("free_format_pattern"),
        ["<Parent>_<FullName>"]
    );
}

/// Newest first, no repeats, and it stops — a list that grew forever would stop
/// being a shortcut.
#[test]
fn the_history_holds_no_duplicates_and_stops_at_twelve() {
    use ren_core::ops::FreeFormat;

    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    for i in 0..15 {
        *harness.state_mut().operation_mut() =
            OpKind::FreeFormat(FreeFormat::new(format!("name {i}")));
        settle(&mut harness);
        harness.state_mut().run_now();
        settle(&mut harness);
    }
    // And once more with something already in the list.
    *harness.state_mut().operation_mut() = OpKind::FreeFormat(FreeFormat::new("name 14"));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);

    let history = harness.state().field_history("free_format_pattern");
    assert_eq!(history.len(), ren_gui::widgets::tag_field::HISTORY_ITEMS);
    assert_eq!(history[0], "name 14", "newest first");
    assert_eq!(
        history.iter().filter(|h| *h == "name 14").count(),
        1,
        "and it moved rather than being added twice"
    );
}

/// Settings ▸ Problem Solver's reset puts every setting back.
///
/// It asks first, and it asks because nothing puts a settings file back —
/// **D156**, not P2, which is about tag writes and never reaches a settings
/// action.
#[test]
fn the_reset_button_asks_first_then_puts_every_setting_back() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut app = fixture.app();
    *app.operation_mut() = OpKind::Casing(Casing::new(CaseMode::Upper));
    let mut harness = harness(app);
    let dir = harness.state().session().settings.dir.clone();

    // Move something a Settings page owns.
    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Music Styles").click();
    settle(&mut harness);
    harness.get_all_by_label("✖").next().unwrap().click();
    settle(&mut harness);
    let trimmed = harness.state().music_styles().len();

    harness.get_by_label("Problem Solver").click();
    settle(&mut harness);
    harness.get_by_label("Reset all settings…").click();
    settle(&mut harness);

    // Nothing has happened yet, and it says what it will not touch.
    assert_eq!(
        harness.state().music_styles().len(),
        trimmed,
        "asked, not done"
    );
    harness.get_by_label_contains("cannot be undone");
    assert!(
        harness
            .query_all_by_label_contains("does not touch your files")
            .count()
            >= 1,
        "it names what it spares — a reset button ten lines under three folder          shortcuts invites exactly the wrong guess"
    );

    harness.get_by_label("Restore every setting").click();
    settle(&mut harness);

    assert!(harness.state().music_styles().len() > trimmed, "put back");
    assert_eq!(
        harness.state().stack().to_steps().len(),
        1,
        "the pipeline is work, not a setting"
    );
    assert_eq!(
        harness.state().session().settings.dir,
        dir,
        "and so is the folder you are looking at"
    );
}

/// Backing out leaves everything alone — the safe answer is on the left and it
/// has to mean it.
#[test]
fn declining_the_reset_changes_nothing() {
    let fixture = Fixture::new(&["a.txt"]);
    let mut harness = harness(fixture.app());

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Music Styles").click();
    settle(&mut harness);
    harness.get_all_by_label("✖").next().unwrap().click();
    settle(&mut harness);
    let trimmed = harness.state().music_styles().len();

    harness.get_by_label("Problem Solver").click();
    settle(&mut harness);
    harness.get_by_label("Reset all settings…").click();
    settle(&mut harness);
    harness.get_by_label("Leave my settings alone").click();
    settle(&mut harness);

    assert_eq!(harness.state().music_styles().len(), trimmed);
}

/// Settings ▸ Shell Integration says what it can do on this platform.
#[test]
fn the_shell_page_says_what_this_platform_can_do() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Shell Integration").click();
    settle(&mut harness);

    if cfg!(windows) {
        harness.get_by_label_contains("when I right-click files, folders and drives");
        // The two facts the menu itself cannot tell anyone. Windows-only,
        // because the page returns before them where there is no menu to
        // install — which is the whole of what the Linux runner can see here.
        harness.get_by_label_contains("Show more options");
        harness.get_by_label_contains("2000 characters");
    } else {
        harness.get_by_label_contains("no context-menu entry to install");
    }
}

/// *Full row select* — click anywhere on the row — and *draw a grey background
/// on every other row*.
///
/// What this covers is that the page changes real state and the table is built
/// from it.
///
/// **What it does not cover**, and cannot: the pointer behaviour, and the two
/// lines that hand the style to the table. This harness drives widgets through
/// the accessibility tree, and both halves of this setting are about pixels —
/// a click on a plain size label with the row's target behind it, and a shaded
/// rectangle. Neither has a node to aim at or to read. `docs/manual-checks.md`
/// walks the whole path instead, beside the file dialogs and the clipboard,
/// which are out of reach for the same kind of reason.
#[test]
fn full_row_select_makes_the_whole_row_a_click_target() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    assert!(!harness.state().table_style().full_row_select);
    // Shading ships on (P91), so this one is checked by switching it off and
    // back on rather than only on.
    assert!(harness.state().table_style().stripes);

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Display").click();
    settle(&mut harness);
    harness
        .get_by_label_contains("Click anywhere on a row to select it")
        .click();
    settle(&mut harness);
    harness
        .get_by_label_contains("Shade every other row")
        .click();
    settle(&mut harness);
    assert!(
        !harness.state().table_style().stripes,
        "the switch turns it off"
    );
    harness
        .get_by_label_contains("Shade every other row")
        .click();
    settle(&mut harness);
    harness.get_by_label("Close").click();
    settle(&mut harness);

    assert!(harness.state().table_style().full_row_select);
    assert!(harness.state().table_style().stripes);

    // The name cell still takes its own clicks with both switched on, which is
    // what keeps double-click-to-rename working — and is the half of this the
    // harness *can* see.
    harness.get_by_label_contains("one.txt").click();
    settle(&mut harness);
    assert_eq!(
        harness
            .state()
            .session()
            .selection
            .iter()
            .collect::<Vec<_>>(),
        [0]
    );
}

// --- M8: the two Batch Replace additions -------------------------------------

/// A Replace card can add its settings to the Batch Replace list with one
/// button.
///
/// The **defaults** list, which is what Settings edits and what a *new* card
/// copies (D35) — adding to a card that already exists would edit somebody's
/// built pipeline from another card's button.
#[test]
fn a_replace_card_can_add_itself_to_the_batch_replace_list() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    let before = harness.state().batch_replace_rules().len();

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    settle(&mut harness);
    harness.get_by_label("Add to Batch Replace").click();
    settle(&mut harness);

    let rules = harness.state().batch_replace_rules();
    assert_eq!(rules.len(), before + 1);
    let added = rules.last().unwrap();
    assert_eq!(added.find, "_");
    assert_eq!(added.replace.as_str(), " ");
}

/// Its **settings**, plural — a rule carries seven of them. The
/// button has always sent all seven (it clones the whole operation) and nothing
/// checked more than two of them, so a version that rebuilt the rule from the
/// find and replace boxes would have passed the test above unchanged.
#[test]
fn add_to_batch_replace_carries_all_seven_settings_not_just_the_two_boxes() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() = OpKind::Replace(
        Replace::new("_", " ")
            .case_sensitive(true)
            .swap(true)
            .skip(3)
            .max(2)
            .regex(false),
    );
    settle(&mut harness);
    harness.get_by_label("Add to Batch Replace").click();
    settle(&mut harness);

    let rules = harness.state().batch_replace_rules();
    let added = rules.last().unwrap();
    assert!(added.case_sensitive, "case sensitivity");
    assert!(added.swap, "swap mode");
    assert_eq!(added.skip, 3, "skip");
    assert_eq!(added.max, 2, "max");
}

/// Fifty-one rules on one page, and four of a rule's seven settings behind a
/// popover — so a rule that is not at its defaults has to say so **on the row**.
/// In the label, not the tooltip: egui only puts a tooltip into the
/// accessibility tree while it is hovered, so a tooltip marker would be
/// invisible to a screen reader and to this harness alike.
#[test]
fn a_batch_rule_whose_hidden_settings_are_not_default_says_so_on_its_own_row() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());

    *harness.state_mut().operation_mut() =
        OpKind::Replace(Replace::new("zzz", "y").skip(3).case_sensitive(true));
    settle(&mut harness);
    harness.get_by_label("Add to Batch Replace").click();
    settle(&mut harness);

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Batch Replace").click();
    settle(&mut harness);

    // One row in the whole list carries a summary; the shipped fifty are plain.
    harness.get_by_label_contains("case sensitive, skip 3");
    assert_eq!(
        harness.get_all_by_label("⋯").count(),
        harness.state().batch_replace_rules().len() - 1,
        "every other rule is at its defaults and stays quiet"
    );
}

/// The list's own add button, with an empty box, which is what the button
/// always did.
///
/// The colon-separated half is `rule_table::added_by`'s own test: this harness
/// drives the accessibility tree and a hint is a placeholder rather than a
/// name, so there is no box here to type into. `docs/manual-checks.md` walks
/// it.
#[test]
fn the_batch_replace_list_adds_a_blank_rule() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    let before = harness.state().batch_replace_rules().len();

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label("Add rule").click();
    settle(&mut harness);

    assert_eq!(harness.state().batch_replace_rules().len(), before + 1);
}

// --- M8: Visual Assist -------------------------------------------------------

use ren_gui::editors::assist::AssistTarget;

/// The card at `index`, for a test that needs its identity rather than its
/// position.
fn card_id(harness: &Harness<'_, RenameItApp>, index: usize) -> ren_gui::viewmodel::CardId {
    harness.state().stack().cards()[index].id
}

/// A card's ⌖ button (or F3) opens Visual Assist, to select the text to find
/// by eye.
///
/// No separate window: a ⌖ on the card opens a strip inside it, carrying all
/// five controls.
#[test]
fn the_marker_button_opens_the_strip_on_its_own_card() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("x", "y"));
    settle(&mut harness);
    harness.state_mut().expand_card(0);
    settle(&mut harness);

    assert!(harness.state().visual_assist_target().is_none());
    harness.get_by_label_contains("⌖ find").click();
    settle(&mut harness);

    assert_eq!(
        harness.state().visual_assist_target(),
        Some(AssistTarget::ReplaceFind)
    );
    harness.get_by_label_contains("Select text:");
    // A caret at the start, because a Replace box holds text rather than a
    // position and there is nothing to seed from.
    harness.get_by_label_contains("Position: 0");
}

/// The strip shows the text the operation is **handed**, not the file name.
///
/// A card scoped to the stem is handed the stem; a card three deep is handed
/// what the two above it produced. Getting this wrong is the whole reason
/// `Pipeline::subject_at` exists — a position picked against the name on disk
/// is wrong by exactly what the earlier cards changed.
#[test]
fn the_strip_shows_what_this_card_is_handed_and_not_the_file_name() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    let second = harness
        .state_mut()
        .add_operation(OpKind::Replace(Replace::new("nothing", "")));
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(second, AssistTarget::ReplaceFind);
    settle(&mut harness);

    assert_eq!(
        harness.state().visual_assist_subject().unwrap().as_deref(),
        Ok("my-holiday"),
        "the first card's output, and the stem — not `my_holiday.jpg`"
    );
}

/// Select writes and closes, and Cancel does neither.
#[test]
fn select_lifts_the_chosen_text_into_the_find_box() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("keep", "me"));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);
    // `holiday`, characters 3..10 of `my_holiday`.
    harness.state_mut().visual_assist_select(3, 7);
    settle(&mut harness);
    harness.get_by_label_contains("Selection length: 7");

    harness.get_by_label("Select").click();
    settle(&mut harness);

    match &harness.state().stack().cards()[0].op {
        OpKind::Replace(op) => assert_eq!(op.find, "holiday"),
        other => panic!("{other:?}"),
    }
    assert!(
        harness.state().visual_assist_target().is_none(),
        "Select closes the strip it was pressed in"
    );
}

/// Cancel leaves the card exactly as it was.
#[test]
fn cancel_writes_nothing() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("keep", "me"));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);
    harness.state_mut().visual_assist_select(3, 7);
    settle(&mut harness);

    harness.get_by_label("Cancel").click();
    settle(&mut harness);

    match &harness.state().stack().cards()[0].op {
        OpKind::Replace(op) => assert_eq!(op.find, "keep", "untouched"),
        other => panic!("{other:?}"),
    }
    assert!(harness.state().visual_assist_target().is_none());
}

/// Nothing selected is not something to write. All three span targets have an
/// early-return no-op for an empty selection, so Select would appear to work
/// and change nothing.
#[test]
fn select_refuses_until_something_is_selected() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("keep", "me"));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);

    harness.get_by_label("Select").click();
    settle(&mut harness);
    match &harness.state().stack().cards()[0].op {
        OpKind::Replace(op) => assert_eq!(op.find, "keep"),
        other => panic!("{other:?}"),
    }
    assert!(
        harness.state().visual_assist_target().is_some(),
        "a refused Select leaves the strip open to try again"
    );
}

/// A card that is switched off is handed nothing, and the strip says which of
/// the reasons it is rather than showing an empty box.
#[test]
fn a_card_that_is_handed_nothing_says_why() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("x", "y"));
    let card = card_id(&harness, 0);
    harness.state_mut().stack_mut().get_mut(0).unwrap().enabled = false;
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);

    harness.get_by_label_contains("switched off");
    assert!(harness.state().visual_assist_subject().unwrap().is_err());
}

/// The strip is not on screen until it is asked for — which is what keeps the
/// `Role::TextInput` ordinals the rest of this file indexes by from shifting.
#[test]
fn the_strip_is_absent_until_the_button_is_clicked() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("x", "y"));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    let before = harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .count();
    assert!(
        harness.query_by_label_contains("Select text:").is_none(),
        "closed"
    );

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);
    harness.get_by_label_contains("Select text:");

    assert_eq!(
        harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .count(),
        before,
        "the strip's field must not renumber the ordinals this file indexes by"
    );
}

/// Collapsing the card takes the strip's only way out off screen, so it closes
/// with it.
#[test]
fn collapsing_the_card_closes_the_strip() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("x", "y"));
    let card = card_id(&harness, 0);
    harness
        .state_mut()
        .add_operation(OpKind::Casing(Default::default()));
    settle(&mut harness);

    harness.state_mut().expand_card(0);
    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);
    assert!(harness.state().visual_assist_target().is_some());

    harness.state_mut().expand_card(1);
    settle(&mut harness);
    assert!(
        harness.state().visual_assist_target().is_none(),
        "a mode whose only control is off screen has no way out"
    );
}

/// Remove sets the position **and** the length from one selection.
#[test]
fn the_section_marker_sets_both_the_position_and_the_length() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() =
        OpKind::AddRemove(AddRemove::remove(0, 0).backwards(true));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::RemoveSection);
    settle(&mut harness);
    harness.state_mut().visual_assist_select(3, 7);
    settle(&mut harness);

    harness.get_by_label("Select").click();
    settle(&mut harness);

    match &harness.state().stack().cards()[0].op {
        OpKind::AddRemove(op) => {
            assert_eq!(op.remove_pos, 3);
            assert_eq!(op.delete, 7);
            assert!(
                !op.remove_backwards,
                "the position reads forward, so the box that would read it \
                 backwards has to come off"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(new_names(&harness), ["my_.jpg"]);
}

/// Move Section's cut, and its paste position left alone — *paste at* has no
/// ⌖ button, and it is measured after the cut anyway.
#[test]
fn the_cut_marker_sets_the_cut_and_leaves_the_paste_position() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::MoveSection(MoveSection::new(0, 0, 2));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::MoveCut);
    settle(&mut harness);
    harness.state_mut().visual_assist_select(0, 3);
    settle(&mut harness);
    harness.get_by_label("Select").click();
    settle(&mut harness);

    match &harness.state().stack().cards()[0].op {
        OpKind::MoveSection(op) => {
            assert_eq!(op.from_pos, 0);
            assert_eq!(op.cut, 3);
            assert_eq!(op.to_pos, 2, "untouched");
        }
        other => panic!("{other:?}"),
    }
}

/// A card in *Both* mode has two markers, and the Add one shows the name the
/// Remove half has **already shortened** — because that is what `apply_add` is
/// measured on. Showing the pre-Remove text would put every caret in the wrong
/// place by however many characters Remove takes out.
#[test]
fn the_add_marker_on_a_both_card_shows_the_shortened_name() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove {
        mode: ren_core::ops::AddRemoveMode::Both,
        delete: 3,
        remove_pos: 0,
        insert: "-".into(),
        ..Default::default()
    });
    let card = card_id(&harness, 0);
    settle(&mut harness);

    // Both markers are on the card, and they are distinguishable — `get_by_label`
    // panics on two matches, so one shared "⌖" would have been unqueryable.
    harness.get_by_label_contains("⌖ section");
    harness.get_by_label_contains("⌖ position");

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::RemoveSection);
    settle(&mut harness);
    assert_eq!(
        harness.state().visual_assist_subject().unwrap().as_deref(),
        Ok("my_holiday"),
        "the Remove half sees the whole stem"
    );

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::AddPos);
    settle(&mut harness);
    assert_eq!(
        harness.state().visual_assist_subject().unwrap().as_deref(),
        Ok("holiday"),
        "and the Add half sees what is left after Remove has run"
    );
}

/// A caret at the very end is `pos 0` from the end of **every** name — which is
/// what the shipped "Add suffix to end of filename" preset does, and what a
/// forward position cannot express for a listing of mixed lengths.
#[test]
fn a_caret_at_the_end_can_be_anchored_to_it() {
    let fixture = Fixture::new(&["short.txt", "a_much_longer_name.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::add("!", 0));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::AddPos);
    settle(&mut harness);
    // The end of "a_much_longer_name", the first file in the listing.
    harness.state_mut().visual_assist_select(18, 0);
    settle(&mut harness);

    harness.get_by_label_contains("Anchor to the end").click();
    settle(&mut harness);

    match &harness.state().stack().cards()[0].op {
        OpKind::AddRemove(op) => {
            assert_eq!(op.add_pos, 0);
            assert!(op.add_backwards);
        }
        other => panic!("{other:?}"),
    }
    // And it lands at the end of both names, which a forward position could not.
    assert_eq!(
        new_names(&harness),
        ["a_much_longer_name!.txt", "short!.txt"]
    );
}

/// F3 opens Visual Assist where the card has one.
///
/// And **while a position box has focus**, which is the whole difficulty:
/// `ctx.text_edit_focused()` is true for a focused `DragValue`, and true again
/// for the strip's own read-only field. F3 is the one key exempt from that
/// guard, because it is the only one whose purpose is the mode the guard is
/// blocking.
#[test]
fn f3_opens_visual_assist_even_while_a_field_has_focus() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::remove(1, 0));
    settle(&mut harness);
    harness.state_mut().expand_card(0);
    settle(&mut harness);

    // The Delete: spinner — a text edit as far as the guard can tell.
    harness
        .get_all_by_role(egui::accesskit::Role::SpinButton)
        .next()
        .expect("a position box")
        .focus();
    harness.run();

    harness.key_press(egui::Key::F3);
    settle(&mut harness);
    assert_eq!(
        harness.state().visual_assist_target(),
        Some(AssistTarget::RemoveSection)
    );

    // And again from inside the strip's own field, which is the case that
    // would break if F3 ever moved back below the guard.
    harness
        .get_all_by_role(egui::accesskit::Role::MultilineTextInput)
        .next()
        .expect("the strip's field")
        .focus();
    harness.run();
    harness.key_press(egui::Key::F3);
    settle(&mut harness);
    assert!(
        harness.state().visual_assist_target().is_none(),
        "one target, so F3 is a toggle"
    );
}

/// On a card with two markers F3 cycles them, which a single window has no
/// equivalent of.
#[test]
fn f3_cycles_the_two_targets_of_a_both_card() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove {
        mode: ren_core::ops::AddRemoveMode::Both,
        delete: 1,
        insert: "-".into(),
        ..Default::default()
    });
    settle(&mut harness);
    harness.state_mut().expand_card(0);
    settle(&mut harness);

    for expected in [
        Some(AssistTarget::RemoveSection),
        Some(AssistTarget::AddPos),
        None,
        Some(AssistTarget::RemoveSection),
    ] {
        harness.key_press(egui::Key::F3);
        settle(&mut harness);
        assert_eq!(harness.state().visual_assist_target(), expected);
    }
}

/// Where the card has no Visual Assist, F3 says so. A key the user was told
/// about that silently does nothing is what gets reported as a bug.
#[test]
fn f3_says_so_when_the_card_has_no_visual_assist() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Casing(Casing::new(CaseMode::Upper));
    settle(&mut harness);
    harness.state_mut().expand_card(0);
    settle(&mut harness);

    harness.key_press(egui::Key::F3);
    settle(&mut harness);

    assert!(harness.state().visual_assist_target().is_none());
    let status = harness.state().status().unwrap_or_default().to_owned();
    assert!(status.contains("not available"), "{status}");
}

/// F5 and Ctrl+Z keep standing down inside a real text field. Exempting F3 must
/// not have exempted anything else.
#[test]
fn exempting_f3_did_not_exempt_the_rest() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("one", "1"));
    settle(&mut harness);

    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .expect("the folder box")
        .focus();
    harness.run();

    harness.key_press(egui::Key::F5);
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["one.txt", "two.txt"],
        "F5 still stands down while a real text field has the keyboard"
    );
}

/// The state survives a re-sort, and that is the point of holding a **path**
/// rather than a row. Sorting renumbers every row underneath the strip; the
/// file it is showing does not move.
#[test]
fn the_strip_survives_a_re_sort() {
    let fixture = Fixture::new(&["b_second.txt", "a_first.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("x", "y"));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);
    harness
        .state_mut()
        .visual_assist_show(&fixture.dir.path().join("b_second.txt"));
    settle(&mut harness);
    assert_eq!(
        harness.state().visual_assist_subject().unwrap().as_deref(),
        Ok("b_second")
    );

    // Reverse the order. Row 0 is now a different file.
    harness
        .state_mut()
        .session_mut()
        .set_sort(ren_gui::viewmodel::SortColumn::Name);
    settle(&mut harness);

    assert_eq!(
        harness.state().visual_assist_subject().unwrap().as_deref(),
        Ok("b_second"),
        "the strip follows its file, not its row"
    );
}

/// A run is the commit point: the name the selection was measured against is
/// gone, so the strip goes with it. A simulation renames nothing, so it stays.
#[test]
fn a_run_closes_the_strip_and_a_simulation_does_not() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("one", "1"));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness.state_mut().set_simulate(true);
    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert!(
        harness.state().visual_assist_target().is_some(),
        "a simulation renames nothing, so nothing the strip is measuring moved"
    );

    harness.state_mut().set_simulate(false);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1.txt"]);
    assert!(
        harness.state().visual_assist_target().is_none(),
        "a rename is the commit point"
    );
}

/// An undo moves the names back, so it ends the strip for the same reason.
#[test]
fn an_undo_closes_the_strip() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("one", "1"));
    let card = card_id(&harness, 0);
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);
    assert!(harness.state().visual_assist_target().is_some());

    harness.state_mut().undo_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["one.txt"]);
    assert!(harness.state().visual_assist_target().is_none());
}

/// Duplicating the card expands the copy, which collapses the original — so the
/// strip closes rather than following the duplicate or hanging on a card that
/// is no longer drawn. Both of those are the tempting bug, and the second
/// writes into a card the user has never looked at.
#[test]
fn duplicating_the_card_closes_the_strip() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("x", "y"));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness.state_mut().expand_card(0);
    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);
    assert!(harness.state().visual_assist_target().is_some());

    harness.state_mut().duplicate_card(0);
    settle(&mut harness);

    assert_eq!(harness.state().stack().len(), 2);
    assert!(
        harness.state().visual_assist_target().is_none(),
        "it must neither follow the copy nor sit on a card that is not drawn"
    );
}

/// Moving the card changes what is handed to it, so the strip stays open and
/// recomputes. A strip still showing the old position's text while writing a
/// position measured against it is the likeliest bug in the whole feature.
#[test]
fn moving_the_card_keeps_the_strip_and_recomputes_it() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    let second = harness
        .state_mut()
        .add_operation(OpKind::Replace(Replace::new("nothing", "")));
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(second, AssistTarget::ReplaceFind);
    settle(&mut harness);
    assert_eq!(
        harness.state().visual_assist_subject().unwrap().as_deref(),
        Ok("my-holiday"),
        "second in the stack, so it sees the first card's output"
    );

    // Move it to the front: it is now handed the untouched name.
    harness.state_mut().move_card(1, 0);
    settle(&mut harness);

    assert!(
        harness.state().visual_assist_target().is_some(),
        "still open"
    );
    assert_eq!(
        harness.state().visual_assist_subject().unwrap().as_deref(),
        Ok("my_holiday"),
        "and showing what it is handed now, not what it was handed before"
    );
}

/// Switching the card off leaves the strip open and says why, rather than
/// vanishing on a checkbox the user may have hit by accident.
#[test]
fn disabling_the_card_keeps_the_strip_and_explains() {
    let fixture = Fixture::new(&["my_holiday.jpg"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("x", "y"));
    let card = card_id(&harness, 0);
    settle(&mut harness);

    harness
        .state_mut()
        .open_visual_assist(card, AssistTarget::ReplaceFind);
    settle(&mut harness);
    assert!(harness.state().visual_assist_subject().unwrap().is_ok());

    harness.state_mut().stack_mut().get_mut(0).unwrap().enabled = false;
    harness.state_mut().session_mut();
    settle(&mut harness);

    assert!(
        harness.state().visual_assist_target().is_some(),
        "still open"
    );
    harness.get_by_label_contains("switched off");
}

// --- M8: the Explorer preset menu --------------------------------------------

/// Start from this folder, on a **file**, browses the folder it is in.
///
/// Resolved in our code rather than by asking Windows for `%W`, whose behaviour
/// for a static registry verb no test of ours can reach — which is why the
/// menu entry is a flag over the paths rather than a substitution we would be
/// trusting blind.
#[test]
fn start_in_browses_the_folder_a_file_lives_in() {
    use ren_gui::viewmodel::SourceMode;

    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut app = fixture.app();
    app.start_from(&ren_gui::launch::Launch {
        paths: vec![fixture.dir.path().join("one.txt")],
        start_in: true,
        from_shell: true,
        ..Default::default()
    });
    let harness = harness(app);

    assert_eq!(harness.state().session().settings.mode, SourceMode::Browser);
    // Both files, because it listed the folder rather than the one file.
    harness.get_by_label_contains("one.txt");
    harness.get_by_label_contains("two.txt");
}

/// A preset from the menu **loads and previews**; it does not rename.
///
/// Renaming straight from the click is the tempting alternative. P4 blocks a
/// conflicting plan and P2 gates an irreversible one, and a right-click has
/// nowhere to answer either — so the
/// files are still on disk under their old names when the window opens, with
/// the new ones in the preview beside them.
#[test]
fn a_preset_from_the_menu_previews_rather_than_renaming() {
    let fixture = Fixture::new(&["my_song.txt", "my_other.txt"]);
    let presets = tempfile::TempDir::new().expect("tempdir");
    let file = presets.path().join("tidy.toml");
    std::fs::write(
        &file,
        "version = 1\n\n[preset]\nname = \"Tidy\"\n\n[[step]]\nop = \"replace\"\nfind = \"_\"\nreplace = \"-\"\n",
    )
    .expect("write");

    let mut app = fixture.app();
    app.start_from(&ren_gui::launch::Launch {
        paths: vec![
            fixture.dir.path().join("my_song.txt"),
            fixture.dir.path().join("my_other.txt"),
        ],
        preset: Some(file),
        from_shell: true,
        ..Default::default()
    });
    let mut harness = harness(app);
    settle(&mut harness);

    assert_eq!(
        new_names(&harness),
        ["my-other.txt", "my-song.txt"],
        "the preset is loaded and previewed"
    );
    assert_eq!(
        fixture.names(),
        ["my_other.txt", "my_song.txt"],
        "and nothing on disk has moved"
    );
}

/// A preset file that has been deleted since the menu was written still opens
/// the app, and says what happened.
///
/// `PresetStore::rename` renames the file as well as the name inside it, so a
/// menu entry dangles on every rename until the menu is rewritten. The window
/// opening with the files listed and a sentence on screen is the right failure;
/// nothing opening at all is not.
#[test]
fn a_preset_that_is_gone_still_opens_the_app_and_says_so() {
    let fixture = Fixture::new(&["one.txt"]);
    let mut app = fixture.app();
    app.start_from(&ren_gui::launch::Launch {
        paths: vec![fixture.dir.path().join("one.txt")],
        preset: Some(fixture.dir.path().join("no-such-preset.toml")),
        from_shell: true,
        ..Default::default()
    });
    let harness = harness(app);

    harness.get_by_label_contains("one.txt");
    assert!(
        harness.state().status().is_some_and(|s| !s.is_empty()),
        "a dangling menu entry has to say something"
    );
}

/// A command line the parser could not read still opens the app, with the
/// reason on screen. In a release build there is no stderr for clap to print
/// to and no console to read it in, so exiting would be a program that
/// silently fails to appear.
#[test]
fn a_command_line_we_could_not_read_still_opens_and_explains() {
    let fixture = Fixture::new(&["one.txt"]);
    let launch = ren_gui::launch::parse(
        [
            "renameit",
            "--presett",
            fixture.dir.path().join("one.txt").to_str().unwrap(),
        ]
        .into_iter()
        .map(std::ffi::OsString::from),
    );

    let mut app = fixture.app();
    app.start_from(&launch);
    let harness = harness(app);

    harness.get_by_label_contains("one.txt");
    let status = harness.state().status().unwrap_or_default().to_owned();
    assert!(status.contains("command line"), "{status}");
}

/// The menu's *Copy filenames to clipboard* and the row menu's *Copy ▸ Names*
/// must agree about what a filename list is.
///
/// They are two functions because one has no `Session` — a context-menu click
/// copies and exits without ever listing a folder — so nothing but this test
/// stops them drifting into two different answers for the same words.
#[test]
fn the_menu_and_the_row_menu_copy_the_same_list() {
    let fixture = Fixture::new(&["b_two.txt", "a_one.txt", "c_three.txt"]);
    let harness = harness(fixture.app());

    let from_the_table = harness
        .state()
        .rows_as_text(ren_gui::panels::rows::CopyWhat::Names, &[]);
    // Explorer hands over the file you right-clicked first, whatever the order
    // on screen — so this is deliberately not in listing order.
    let from_the_menu = ren_gui::launch::names_text(&[
        fixture.dir.path().join("c_three.txt"),
        fixture.dir.path().join("a_one.txt"),
        fixture.dir.path().join("b_two.txt"),
    ]);

    assert_eq!(from_the_menu, from_the_table);
    assert_eq!(from_the_menu, "a_one.txt\nb_two.txt\nc_three.txt\n");
}

// --- The file list's state across runs, relists and modals --------------------

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

/// **Free Select follows its files through a run.** The set is a list of
/// paths, and a run renames them: relisting the old paths used to fail on the
/// first one and empty the table with a bare OS error, after every run, F2 and
/// undo in Free Select.
#[test]
fn a_free_select_run_keeps_its_rows() {
    use ren_gui::viewmodel::SourceMode;

    let fixture = Fixture::new(&["a_1.txt", "a_2.txt"]);
    let mut app = fixture.app();
    app.start_at(vec![
        fixture.dir.path().join("a_1.txt"),
        fixture.dir.path().join("a_2.txt"),
    ]);
    *app.operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    let mut harness = harness(app);
    assert_eq!(
        harness.state().session().settings.mode,
        SourceMode::FreeSelect
    );

    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["a-1.txt", "a-2.txt"]);
    assert_eq!(harness.state().session().error, None);
    assert_eq!(
        listed(&harness),
        ["a-1.txt", "a-2.txt"],
        "the rows followed"
    );

    // And back again, through the undo report.
    harness.state_mut().undo_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["a_1.txt", "a_2.txt"]);
    assert_eq!(listed(&harness), ["a_1.txt", "a_2.txt"]);

    // F2 is a run of one.
    harness.state_mut().rename_one(0, "solo.txt".to_owned());
    settle(&mut harness);
    assert_eq!(harness.state().session().error, None);
    assert_eq!(listed(&harness), ["a_2.txt", "solo.txt"]);
}

/// P63 in Free Select: a file deleted outside the app costs its own row and
/// no others.
#[test]
fn a_free_select_file_that_is_gone_costs_only_its_own_row() {
    let fixture = Fixture::new(&["keep.txt", "gone.txt"]);
    let mut app = fixture.app();
    app.start_at(vec![
        fixture.dir.path().join("keep.txt"),
        fixture.dir.path().join("gone.txt"),
    ]);
    let mut harness = harness(app);
    assert_eq!(listed(&harness), ["gone.txt", "keep.txt"]);

    std::fs::remove_file(fixture.dir.path().join("gone.txt")).unwrap();
    harness.state_mut().forget_and_relist();
    settle(&mut harness);

    let session = harness.state().session();
    assert_eq!(
        session.error, None,
        "one missing file is not a failed listing"
    );
    assert_eq!(listed(&harness), ["keep.txt"]);
    assert_eq!(
        session.problems.len(),
        1,
        "and it is named rather than hidden"
    );
}

/// **F2's editor belongs to a file, not a row number.** Opened on `a.txt`, then
/// the list re-sorted underneath it: Enter must rename `a.txt`, not whatever
/// the sort put in row 0.
#[test]
fn the_inline_editor_follows_its_file_through_a_sort() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut harness = harness(fixture.app());
    harness.state_mut().select(vec![0]);
    settle(&mut harness);

    harness.key_press(egui::Key::F2);
    settle(&mut harness);
    // Descending: c.txt is row 0 now.
    harness
        .state_mut()
        .session_mut()
        .set_sort(ren_gui::viewmodel::SortColumn::Name);
    settle(&mut harness);

    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| n.value().as_deref() == Some("a.txt"))
        .expect("the inline editor, still holding a.txt")
        .type_text("x");
    settle(&mut harness);
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    assert_eq!(fixture.names(), ["b.txt", "c.txt", "x.txt"]);
}

/// Ctrl+Shift+Z is redo almost everywhere. This app has no redo, and the key
/// must not reach the batch undo instead.
#[test]
fn ctrl_shift_z_is_not_undo() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("one", "1"));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1.txt", "two.txt"]);

    harness.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Z,
    );
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1.txt", "two.txt"], "no batch undo");
}

/// A number box being typed into is a `TextEdit`, and Ctrl+Z there undoes the
/// digit. It must not also undo the last batch on disk.
#[test]
fn ctrl_z_in_a_number_box_undoes_the_digit_and_not_the_batch() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("one", "1"));
    harness
        .state_mut()
        .add_operation(OpKind::AddRemove(AddRemove::remove(0, 0)));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1.txt", "two.txt"]);

    harness
        .get_all_by_role(egui::accesskit::Role::SpinButton)
        .next()
        .expect("a position box on the Add & Remove card")
        .focus();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1.txt", "two.txt"], "no batch undo");
}

/// A modal owns the keyboard even when nothing inside it has focus: Ctrl+Z
/// in Settings must not revert the last batch behind it, and F5 must not run.
#[test]
fn the_hotkeys_stand_down_behind_a_modal() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("one", "1"));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1.txt", "two.txt"]);

    harness.key_press(egui::Key::F8);
    settle(&mut harness);
    harness.get_by_label_contains("rules, run top to bottom");

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["1.txt", "two.txt"],
        "no undo behind Settings"
    );

    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("two", "2"));
    settle(&mut harness);
    harness.key_press(egui::Key::F5);
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["1.txt", "two.txt"],
        "no run behind Settings"
    );
}

/// P65: Delete takes rows out of Free Select, and never touches the disk.
#[test]
fn delete_removes_rows_from_free_select_and_leaves_the_files() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut app = fixture.app();
    app.start_at(vec![
        fixture.dir.path().join("a.txt"),
        fixture.dir.path().join("b.txt"),
        fixture.dir.path().join("c.txt"),
    ]);
    let mut harness = harness(app);
    let b = harness
        .state()
        .session()
        .entries()
        .iter()
        .position(|e| e.file_name == "b.txt")
        .unwrap();
    harness.state_mut().select(vec![b]);
    settle(&mut harness);

    harness.key_press(egui::Key::Delete);
    settle(&mut harness);
    assert_eq!(listed(&harness), ["a.txt", "c.txt"]);
    assert_eq!(
        fixture.names(),
        ["a.txt", "b.txt", "c.txt"],
        "nothing deleted"
    );
}

/// "Add to Free Select" on one folder row adds that folder. The drag rule —
/// one folder dropped in Browser mode navigates there — is for drags.
#[test]
fn adding_one_folder_row_to_free_select_does_not_browse_into_it() {
    use ren_gui::viewmodel::SourceMode;

    let fixture = Fixture::new(&["one.txt"]);
    std::fs::create_dir(fixture.dir.path().join("2019")).unwrap();
    std::fs::write(fixture.dir.path().join("2019").join("inside.txt"), b"x").unwrap();
    let mut harness = harness(fixture.app());
    harness.state_mut().session_mut().settings.folders = true;
    harness.state_mut().session_mut().request_refresh();
    settle(&mut harness);

    harness.get_by_label_contains("2019").click_secondary();
    settle(&mut harness);
    harness.get_by_label("Add to Free Select").click();
    settle(&mut harness);

    let session = harness.state().session();
    assert_eq!(session.settings.mode, SourceMode::FreeSelect);
    assert_eq!(session.settings.dir, fixture.dir.path(), "did not browse");
    assert_eq!(listed(&harness), ["2019"]);
}

/// Simulate means nothing touches the disk, and F2 is a run of one.
#[test]
fn f2_honours_simulate() {
    let fixture = Fixture::new(&["before.txt"]);
    let mut harness = harness(fixture.app());
    harness.state_mut().set_simulate(true);

    harness.state_mut().rename_one(0, "after.txt".to_owned());
    settle(&mut harness);
    assert_eq!(fixture.names(), ["before.txt"], "nothing was written");
    assert!(
        harness
            .state()
            .status()
            .is_some_and(|s| s.contains("Simulated")),
        "{:?}",
        harness.state().status()
    );
}

/// A run still going in another window holds its journal. It is not a batch
/// that did not finish, and it must never be offered for rollback.
#[test]
fn a_run_in_another_window_is_named_and_not_offered_for_rollback() {
    let fixture = Fixture::new(&["a.txt"]);
    let live = ren_core::exec::Journal::create(fixture.journal.path()).unwrap();

    let harness = harness(fixture.app());
    harness.get_by_label_contains("another RenameIt window");
    assert!(
        harness.query_by_label("Roll back").is_none(),
        "nothing here can be rolled back"
    );
    drop(live);
}

/// One journal that cannot be read used to hide every unfinished batch, and
/// itself, without a word.
#[test]
fn an_unreadable_journal_is_named_in_the_banner() {
    let fixture = Fixture::new(&["a.txt"]);
    std::fs::write(
        fixture.journal.path().join("broken.jsonl"),
        "this is not json\n{}\n",
    )
    .unwrap();

    let harness = harness(fixture.app());
    harness.get_by_label_contains("could not be read");
    harness.get_by_label_contains("broken.jsonl");
}

/// A folder named on the command line relative to where it was started is
/// made absolute once, on the way in. A relative listing gave relative plan
/// paths, which the executor refuses, and a guard that compared prefixes.
#[cfg(unix)]
#[test]
fn a_relative_folder_on_the_command_line_is_made_absolute() {
    let fixture = Fixture::new(&["a_1.txt"]);
    let cwd = std::env::current_dir().unwrap();
    let mut relative = std::path::PathBuf::new();
    for _ in cwd.components().skip(1) {
        relative.push("..");
    }
    relative.push(fixture.dir.path().strip_prefix("/").unwrap());
    assert!(relative.is_relative() && relative.is_dir(), "{relative:?}");

    let mut app = fixture.app();
    app.start_at(vec![relative]);
    *app.operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    let mut harness = harness(app);
    assert!(harness.state().session().settings.dir.is_absolute());

    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["a-1.txt"],
        "{:?}",
        harness.state().status()
    );
}

/// Roll back is a batch undo and runs off the frame like one: a rollback of a
/// big batch on a share must not freeze the window.
#[test]
fn roll_back_runs_off_the_frame() {
    let fixture = Fixture::new(&["a_1.txt", "a_2.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", "-"));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["a-1.txt", "a-2.txt"]);

    // Make it look crashed: no Commit.
    let journal = harness.state().history().batches[0].journal.clone();
    let text = std::fs::read_to_string(&journal).unwrap();
    let crashed: String = text
        .lines()
        .filter(|l| !l.contains("\"commit\""))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&journal, crashed).unwrap();

    let mut harness = harness_from(fixture.app());
    harness.get_by_label_contains("1 batch did not finish");
    harness.get_by_label("Roll back").click();
    // One frame per queued event and no more: the click lands on the last,
    // and nothing after it could have taken the job's delivery yet.
    harness.step();
    assert!(
        harness.state().is_running(),
        "the rollback is on the worker"
    );
    settle(&mut harness);
    assert_eq!(fixture.names(), ["a_1.txt", "a_2.txt"]);
    assert!(
        harness
            .state()
            .status()
            .is_some_and(|s| s.starts_with("Rolled back")),
        "{:?}",
        harness.state().status()
    );
    assert!(harness.query_by_label("Roll back").is_none(), "banner gone");
}

fn harness_from(app: RenameItApp) -> Harness<'static, RenameItApp> {
    harness(app)
}

/// A script's write is shown before it happens, create or overwrite, and the
/// status line counts it.
#[test]
fn a_script_write_is_confirmed_before_it_happens() {
    let fixture = Fixture::new(&["a.txt"]);
    let scripts = TempDir::new().unwrap();
    let target = fixture.dir.path().join("list.m3u");
    std::fs::write(
        scripts.path().join("Writer.koto"),
        format!(
            "rename = || ''\ndone = ||\n  {{path: '{}', contents: 'x', log: 'wrote it'}}\n",
            target.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    ren_core::script::store::forget_all();

    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() =
        OpKind::Script(ren_core::ops::Script::new("Writer").in_dir(scripts.path()));
    settle(&mut harness);
    harness.get_by_label_contains("1 file will be written");

    harness.state_mut().run_now();
    settle(&mut harness);
    assert!(
        !target.exists(),
        "not before the user says so: {:?}",
        harness.state().status()
    );
    harness.get_by_label_contains("list.m3u  —  create this file");

    harness.get_by_label("Write 1 file").click();
    settle(&mut harness);
    assert!(target.exists());
}

/// A batch undone somewhere else — `ren-cli undo`, a second window — is gone
/// from the Undo stack rather than jamming it.
#[test]
fn a_batch_undone_elsewhere_leaves_the_undo_stack() {
    let fixture = Fixture::new(&["one.txt", "two.txt"]);
    let mut harness = harness(fixture.app());
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("one", "1"));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("two", "2"));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert_eq!(fixture.names(), ["1.txt", "2.txt"]);

    // What `ren-cli undo` does: the newest batch.
    ren_core::exec::undo_last(ren_platform::host().as_ref(), fixture.journal.path()).unwrap();
    assert_eq!(fixture.names(), ["1.txt", "two.txt"]);

    harness.state_mut().undo_now();
    settle(&mut harness);
    assert!(
        harness
            .state()
            .status()
            .is_some_and(|s| s.contains("already undone")),
        "{:?}",
        harness.state().status()
    );
    harness.state_mut().undo_now();
    settle(&mut harness);
    assert_eq!(
        fixture.names(),
        ["one.txt", "two.txt"],
        "the older batch is reachable"
    );
}

/// A hand-set order survives a run whose renames form a cycle — the files
/// swap names through a temporary one — and survives the undo of it.
#[test]
fn a_dragged_order_survives_a_swap_and_its_undo() {
    let fixture = Fixture::new(&["a.txt", "b.txt", "c.txt"]);
    let mut app = fixture.app();
    *app.operation_mut() = OpKind::Replace(Replace::new("a", "X"));
    app.add_operation(OpKind::Replace(Replace::new("b", "a")));
    app.add_operation(OpKind::Replace(Replace::new("X", "b")));
    let mut harness = harness(app);
    assert_eq!(new_names(&harness), ["b.txt", "a.txt", "c.txt"]);

    // c first by hand.
    harness.state_mut().session_mut().move_rows(&[2], 0);
    settle(&mut harness);
    let order = |h: &Harness<'_, RenameItApp>| -> Vec<String> {
        h.state()
            .session()
            .entries()
            .iter()
            .map(|e| e.file_name.clone())
            .collect()
    };
    assert_eq!(order(&harness), ["c.txt", "a.txt", "b.txt"]);

    harness.state_mut().run_now();
    settle(&mut harness);
    // The file that was a.txt is b.txt now, and it kept its slot.
    assert_eq!(order(&harness), ["c.txt", "b.txt", "a.txt"]);
    assert!(harness.state().session().settings.sort.manual);

    harness.state_mut().undo_now();
    settle(&mut harness);
    assert_eq!(order(&harness), ["c.txt", "a.txt", "b.txt"], "and back");
    assert!(harness.state().session().settings.sort.manual);
}
