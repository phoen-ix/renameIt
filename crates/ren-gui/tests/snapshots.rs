//! The window, rendered (D248).
//!
//! Each test drives the real app to one screen, asserts it is there, and
//! compares a 2× render — wgpu on Mesa's lavapipe — with
//! `docs/screenshots/<name>.png`. Those files are the README's and the
//! gallery's images, so a visual regression and a stale screenshot are the
//! same failure. Linux only: it is where CI renders, and the Windows job does
//! not build this file.
//!
//! `UPDATE_SNAPSHOTS=1 cargo test -p ren-gui --test snapshots` rewrites the
//! images that changed; `scripts/docs-screenshots.sh` renders them all.
//! `RENAMEIT_SHOTS=<dir>` points the comparison at another folder.
#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use chrono::TimeZone as _;
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, SnapshotOptions};
use ren_core::meta::testing::Mp3;
use ren_core::ops::{AddRemove, CaseMode, Casing, MusicRename, MusicTagger, OpKind, Replace};
use ren_core::template::TextTemplate;
use ren_gui::RenameItApp;
use ren_gui::viewmodel::{SourceMode, ViewMode};

/// One test at a time: the demo folders share fixed paths, and lavapipe
/// crashed when several tests created and drew on their devices at once.
static SERIAL: Mutex<()> = Mutex::new(());

/// A demo folder at a fixed path, so the address bar reads the same on every
/// run. Holds [`SERIAL`] until the test ends.
struct Demo {
    dir: PathBuf,
    state: PathBuf,
    _serial: Option<MutexGuard<'static, ()>>,
}

impl Demo {
    fn new(folder: &str) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        Self::at(folder, Some(serial))
    }

    /// A second folder for a test that already holds a [`Demo`].
    fn second(folder: &str) -> Self {
        Self::at(folder, None)
    }

    fn at(folder: &str, serial: Option<MutexGuard<'static, ()>>) -> Self {
        let root = Path::new("/tmp/renameit-docs");
        let demo = Self {
            dir: root.join(folder),
            state: root.join(".state").join(folder),
            _serial: serial,
        };
        let _ = fs::remove_dir_all(&demo.dir);
        let _ = fs::remove_dir_all(&demo.state);
        fs::create_dir_all(&demo.dir).expect("demo folder");
        fs::create_dir_all(&demo.state).expect("state folder");
        demo
    }

    fn files(self, names: &[&str]) -> Self {
        for name in names {
            fs::write(self.dir.join(name), format!("{name}\n").repeat(40)).expect("write");
        }
        self.stamped()
    }

    /// Pictures with something in them, so a tile shows which file it is.
    fn pictures(self, names: &[&str]) -> Self {
        const SKIES: [([u8; 3], [u8; 3]); 4] = [
            ([255, 183, 94], [62, 92, 160]),
            ([140, 200, 240], [40, 120, 90]),
            ([250, 120, 120], [70, 50, 110]),
            ([200, 220, 235], [90, 110, 130]),
        ];
        for (i, name) in names.iter().enumerate() {
            let (top, bottom) = SKIES[i % SKIES.len()];
            image::RgbImage::from_fn(240, 160, |x, y| {
                let t = y as f32 / 159.0;
                let horizon = if y > 100 { 0.7 } else { 1.0 };
                let sun = (x as f32 - 60.0 - 30.0 * i as f32).hypot(y as f32 - 70.0) < 18.0;
                image::Rgb(std::array::from_fn(|c| {
                    let v = f32::from(top[c]) * (1.0 - t) + f32::from(bottom[c]) * t;
                    if sun { 255 } else { (v * horizon) as u8 }
                }))
            })
            .save(self.dir.join(name))
            .expect("picture");
        }
        self.stamped()
    }

    fn tracks(self, tracks: &[(&str, Mp3)]) -> Self {
        for (name, mp3) in tracks {
            mp3.write(&self.dir, name);
        }
        // The tag cache is process-wide and keyed by path.
        ren_core::meta::audio::forget_all();
        self.stamped()
    }

    /// Every file modified at a fixed *local* time, a minute apart, so the
    /// Modified column reads the same in any time zone (CI runs in
    /// Asia/Kolkata).
    fn stamped(self) -> Self {
        let mut names: Vec<_> = fs::read_dir(&self.dir)
            .expect("readable")
            .map(|e| e.expect("entry").path())
            .collect();
        names.sort();
        let base = chrono::Local
            .with_ymd_and_hms(2026, 6, 15, 10, 0, 0)
            .single()
            .expect("a plain local time");
        for (i, path) in names.iter().enumerate() {
            let when = base + chrono::TimeDelta::minutes(i as i64 * 7);
            fs::File::options()
                .write(true)
                .open(path)
                .and_then(|f| f.set_modified(when.into()))
                .expect("set mtime");
        }
        self
    }

    fn app(&self) -> RenameItApp {
        RenameItApp::headless(self.dir.clone(), self.state.clone())
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
        let _ = fs::remove_dir_all(&self.state);
    }
}

const PHOTOS: [&str; 9] = [
    "beach_at_sunset.jpg",
    "dinner_with_friends.jpg",
    "harbour_boats.jpg",
    "mountain_view_01.jpg",
    "mountain_view_02.jpg",
    "old_town_walk.jpg",
    "packing_list.txt",
    "train_tickets.pdf",
    "trip_notes.txt",
];

fn holiday() -> Demo {
    let pictures: Vec<_> = PHOTOS
        .iter()
        .copied()
        .filter(|n| n.ends_with(".jpg"))
        .collect();
    let others: Vec<_> = PHOTOS
        .iter()
        .copied()
        .filter(|n| !n.ends_with(".jpg"))
        .collect();
    Demo::new("Holiday 2026").pictures(&pictures).files(&others)
}

fn window(app: RenameItApp, theme: egui::Theme) -> Harness<'static, RenameItApp> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_pixels_per_point(2.0)
        .with_theme(theme)
        .with_max_steps(16)
        .with_render_cursor(false)
        .build_ui_state(|ui, app: &mut RenameItApp| app.show(ui), app);
    settle(&mut harness);
    harness
}

fn light(app: RenameItApp) -> Harness<'static, RenameItApp> {
    window(app, egui::Theme::Light)
}

/// The workers first, then a paint — as `tests/gui.rs` does.
fn settle(harness: &mut Harness<'_, RenameItApp>) {
    for _ in 0..3 {
        harness.state_mut().settle();
        harness.run();
    }
}

/// Waits out any "updating…" (a stale preview), then compares the frame.
fn shoot(harness: &mut Harness<'_, RenameItApp>, name: &str) {
    for _ in 0..10 {
        settle(harness);
        if harness.query_by_label_contains("updating…").is_none() {
            break;
        }
    }
    assert!(
        harness.query_by_label_contains("updating…").is_none(),
        "{name}: the preview never settled"
    );
    let dir = std::env::var_os("RENAMEIT_SHOTS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots"),
        PathBuf::from,
    );
    harness.snapshot_options(name, &SnapshotOptions::new().output_path(dir));
}

/// Find & Replace `_` → space, then Title Case: the hero's two cards.
fn tidy_up(harness: &mut Harness<'_, RenameItApp>) {
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    harness
        .state_mut()
        .add_operation(OpKind::Casing(Casing::new(CaseMode::Title)));
    settle(harness);
}

fn open_settings_page(harness: &mut Harness<'_, RenameItApp>, page: &str) {
    harness.key_press(egui::Key::F8);
    settle(harness);
    // The rail entry is the leftmost node with the page's name.
    let rail = harness
        .get_all_by_label(page)
        .map(|node| node.rect())
        .min_by(|a, b| a.left().total_cmp(&b.left()))
        .expect("a rail entry");
    harness
        .get_all_by_label(page)
        .find(|node| node.rect() == rail)
        .expect("the same node")
        .click();
    settle(harness);
}

fn hero(theme: egui::Theme, name: &str) {
    let demo = holiday();
    let mut harness = window(demo.app(), theme);
    tidy_up(&mut harness);
    assert_eq!(harness.state().plan().expect("a plan").changed(), 9);
    harness.get_by_label("Train Tickets.pdf");
    shoot(&mut harness, name);
}

#[test]
fn main_preview() {
    hero(egui::Theme::Light, "main-preview");
}

#[test]
fn main_preview_dark() {
    hero(egui::Theme::Dark, "main-preview.dark");
}

#[test]
fn main_empty() {
    let demo = Demo::new("Empty folder");
    let mut harness = light(demo.app());
    assert!(harness.state().session().entries().is_empty());
    harness.get_by_label("No files listed");
    shoot(&mut harness, "main-empty");
}

#[test]
fn main_conflict() {
    let demo = Demo::new("Reports").files(&["report_draft.txt", "report_final.txt"]);
    let mut harness = light(demo.app());
    // Cut the stems down to nothing: both files want the same name.
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::remove(999, 0));
    settle(&mut harness);
    assert!(harness.state().plan().expect("a plan").conflicts() > 0);
    assert!(harness.state().blocked_reason().is_some());
    shoot(&mut harness, "main-conflict");
}

/// Three files, not the whole holiday folder: the run log opens at a
/// scroll area's minimum height, which a fourth line would overflow.
#[test]
fn main_after_run() {
    let demo = Demo::new("Holiday 2026")
        .pictures(&["beach_at_sunset.jpg", "harbour_boats.jpg"])
        .files(&["train_tickets.pdf"]);
    let mut harness = light(demo.app());
    tidy_up(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    assert!(demo.dir.join("Train Tickets.pdf").exists());
    let status = harness.state().status().unwrap_or_default().to_owned();
    assert!(status.contains("Renamed 3 items"), "{status}");
    shoot(&mut harness, "main-after-run");
}

/// The right-click menu over a row, which selects that row first.
#[test]
fn main_row_menu() {
    let demo = holiday();
    let mut harness = light(demo.app());
    tidy_up(&mut harness);
    harness
        .get_by_label_contains("harbour_boats.jpg")
        .click_secondary();
    settle(&mut harness);
    harness.get_by_label("Rename…");
    harness.get_by_label("Copy to clipboard");
    shoot(&mut harness, "main-row-menu");
}

#[test]
fn main_free_select() {
    let demo = holiday();
    let other = Demo::second("Scans").files(&["scan_0001.pdf", "scan_0002.pdf"]);
    let mut app = demo.app();
    app.start_at(vec![
        demo.dir.join("beach_at_sunset.jpg"),
        demo.dir.join("harbour_boats.jpg"),
        other.dir.join("scan_0001.pdf"),
        other.dir.join("scan_0002.pdf"),
    ]);
    let mut harness = light(app);
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("_", " "));
    settle(&mut harness);
    let session = harness.state().session();
    assert_eq!(session.settings.mode, SourceMode::FreeSelect);
    assert_eq!(session.free_select_folder_count(), 2);
    shoot(&mut harness, "main-free-select");
}

/// Also `docs/manual-checks.md`'s by-eye rows: a long name is clipped rather
/// than widening its tile, and a file that is not a picture gets ⊘.
#[test]
fn main_grid() {
    let demo = holiday()
        .pictures(&["camera_import_2026_06_15_harbour_at_dawn_from_the_ferry.jpg"])
        .files(&["not_a_picture.jpg"]);
    let mut harness = light(demo.app());
    harness.state_mut().session_mut().settings.view = ViewMode::Grid;
    tidy_up(&mut harness);
    harness.get_by_label_contains("thumbnail of beach_at_sunset.jpg");
    shoot(&mut harness, "main-grid");
}

/// The bundled fonts, drawn: no boxes, joined Arabic, tone marks in place
/// (`docs/manual-checks.md`, non-Latin filenames).
#[test]
fn main_writing_systems() {
    let demo = Demo::new("Writing systems").files(&[
        "日本語のファイル.txt",
        "中文文件.txt",
        "한국어.txt",
        "ไฟล์ภาษาไทย.txt",
        "हिन्दी फ़ाइल.txt",
        "مرحبا بالعالم.txt",
        "שלום עולם.txt",
        "Ελληνικά.txt",
        "Русский файл.txt",
        "café ☕.txt",
    ]);
    let mut harness = light(demo.app());
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::add("✓ ", 0));
    settle(&mut harness);
    harness.get_by_label_contains("✓ 中文文件.txt");
    shoot(&mut harness, "main-writing-systems");
}

#[test]
fn main_recovery_banner() {
    use ren_core::exec::{Journal, Record};

    let demo = Demo::new("Interrupted").files(&["holiday_video.mp4", "notes.txt"]);
    let mut journal = Journal::create(&demo.state).expect("journal");
    journal
        .write(Record::Begin {
            platform: "test".into(),
            items: 1,
        })
        .expect("begin");
    // Announced, never confirmed: the process died mid-rename.
    journal
        .write(Record::PlanRename {
            seq: 0,
            from: demo.dir.join("holiday_video.mp4"),
            to: demo.dir.join("Holiday Video.mp4"),
        })
        .expect("plan");
    drop(journal);

    let mut harness = light(demo.app());
    harness.get_by_label_contains("did not finish");
    shoot(&mut harness, "main-recovery-banner");
}

#[test]
fn palette_add_operation() {
    let demo = holiday();
    let mut harness = light(demo.app());
    harness.state_mut().open_palette();
    settle(&mut harness);
    // Narrowed, so the list ends on a whole entry: the full catalogue
    // scrolls. The search box has the focus from the moment it opens.
    harness.event(egui::Event::Text("tags".to_owned()));
    settle(&mut harness);
    harness.get_by_label("Free Format");
    harness.get_by_label("Remove Tags");
    assert!(harness.query_by_label("Set Casing").is_none());
    shoot(&mut harness, "palette-add-operation");
}

#[test]
fn presets_drawer() {
    let demo = holiday();
    ren_core::PresetStore::new(demo.state.join("presets"))
        .seed_defaults()
        .expect("default presets");
    let mut harness = light(demo.app());
    tidy_up(&mut harness);
    harness.state_mut().open_presets();
    settle(&mut harness);
    assert!(
        harness
            .query_all_by_label_contains("Add prefix to filename")
            .count()
            >= 1
    );
    shoot(&mut harness, "presets-drawer");
}

#[test]
fn card_music_rename() {
    let demo = Demo::new("Night Market").tracks(&[
        (
            "01 track.mp3",
            Mp3::tagged("The Paper Lanterns", "Lantern Light").frame("TRCK", "1"),
        ),
        (
            "02 track.mp3",
            Mp3::tagged("The Paper Lanterns", "Harbour Song").frame("TRCK", "2"),
        ),
        (
            "quiet_hours_FINAL.mp3",
            Mp3::tagged("The Paper Lanterns", "Quiet Hours").frame("TRCK", "3"),
        ),
        (
            "track04 (copy).mp3",
            Mp3::tagged("The Paper Lanterns", "Copper Moon").frame("TRCK", "4"),
        ),
    ]);
    let mut harness = light(demo.app());
    *harness.state_mut().operation_mut() = OpKind::MusicRename(MusicRename::default());
    settle(&mut harness);
    harness.get_by_label("The Paper Lanterns - Copper Moon.mp3");
    shoot(&mut harness, "card-music-rename");
}

#[test]
fn dialog_irreversible() {
    let demo = Demo::new("Night Market").tracks(&[
        (
            "The Paper Lanterns - Lantern Light.mp3",
            Mp3::tagged("Unknown", "Track 1"),
        ),
        (
            "The Paper Lanterns - Harbour Song.mp3",
            Mp3::tagged("Unknown", "Track 2"),
        ),
    ]);
    let mut harness = light(demo.app());
    *harness.state_mut().operation_mut() = OpKind::MusicTagger(Box::new(MusicTagger {
        artist: Some(TextTemplate::new("The Paper Lanterns")),
        ..Default::default()
    }));
    settle(&mut harness);
    harness.get_by_label("Rename").click();
    settle(&mut harness);
    harness.get_by_label_contains(&ren_gui::panels::confirm::go_ahead_label(2));
    shoot(&mut harness, "dialog-irreversible");
}

#[test]
fn dialog_ask() {
    let demo = holiday();
    let mut harness = light(demo.app());
    *harness.state_mut().operation_mut() = OpKind::AddRemove(AddRemove::add("<Ask> - ", 0));
    settle(&mut harness);
    harness.state_mut().run_now();
    settle(&mut harness);
    harness.get_by_label("Enter text");
    shoot(&mut harness, "dialog-ask");
}

#[test]
fn visual_assist() {
    let demo = holiday();
    let mut harness = light(demo.app());
    // Taller, so the strip fits on the card stack without scrolling.
    harness.set_size(egui::vec2(1280.0, 1000.0));
    *harness.state_mut().operation_mut() = OpKind::Replace(Replace::new("", "-"));
    settle(&mut harness);
    harness.get_by_label_contains("⌖ find").click();
    settle(&mut harness);
    // `_at_`, characters 5..9 of `beach_at_sunset`.
    harness.state_mut().visual_assist_select(5, 4);
    settle(&mut harness);
    harness.get_by_label_contains("Selection length: 4");
    harness.get_by_label("Select");
    shoot(&mut harness, "visual-assist");
}

/// `expect` is a label only that page shows, so a missed rail click fails
/// here rather than only in the pixel diff.
fn settings_page(page: &str, expect: &str, name: &str) {
    let demo = holiday();
    let mut harness = light(demo.app());
    open_settings_page(&mut harness, page);
    harness.get_by_label_contains(expect);
    shoot(&mut harness, name);
}

#[test]
fn settings_batch_replace() {
    settings_page(
        "Batch Replace",
        "a new Batch Replace operation starts from",
        "settings-batch-replace",
    );
}

#[test]
fn settings_music_styles() {
    settings_page("Music Styles", "+ Add style", "settings-music-styles");
}

#[test]
fn settings_casing_exceptions() {
    settings_page(
        "Casing Exceptions",
        "Exception words",
        "settings-casing-exceptions",
    );
}

#[test]
fn settings_display() {
    settings_page("Display", "Shade every other row", "settings-display");
}

#[test]
fn settings_file_system() {
    settings_page(
        "File System",
        "Hidden files and folders",
        "settings-file-system",
    );
}

#[test]
fn settings_startup() {
    settings_page("Startup", "Uncheck Subfolders", "settings-startup");
}

#[test]
fn settings_appearance() {
    settings_page("Appearance", "Interface size", "settings-appearance");
}

#[test]
fn dialog_reset() {
    let demo = holiday();
    let mut harness = light(demo.app());
    open_settings_page(&mut harness, "Problem Solver");
    harness.get_by_label("Reset all settings…").click();
    settle(&mut harness);
    harness.get_by_label("Restore every setting");
    shoot(&mut harness, "dialog-reset");
}
