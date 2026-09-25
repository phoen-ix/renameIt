//! RenameIt — a batch file renamer.

// A release build must not pop a console window behind the app. A *debug* build
// must, or `dbg!`, a panic message and every `cargo run` lose their output —
// which is why this is conditional rather than unconditional.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// The window and taskbar icon, 64x64 raw RGBA.
///
/// Windows takes its Explorer icon from the executable's resource section
/// (`build.rs`), but that is not the window icon and it does not exist at all
/// on Linux. Raw RGBA rather than a PNG so no image decoder is needed to show
/// it; `assets/icon.py` writes both from the same drawing.
const ICON_RGBA: &[u8] = include_bytes!("../assets/renameit-64.rgba");
const ICON_SIDE: u32 = 64;

fn main() -> eframe::Result {
    // Before `run_native`, because eframe picks its storage location *inside*
    // it — so "am I portable?" cannot be answered by the app once it is
    // running (D131). Both halves are decided here: our own data root, and
    // eframe's window/state blob.
    let portable = ren_platform::portable_root();
    if let Some(root) = &portable {
        // Ignoring the error is right: `set` only fails if something already
        // set it, and nothing in this binary can have.
        let _ = ren_platform::use_portable_root(root.clone());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([720.0, 420.0])
            .with_icon(egui::IconData {
                rgba: ICON_RGBA.to_vec(),
                width: ICON_SIDE,
                height: ICON_SIDE,
            })
            .with_title("RenameIt"),
        // Window geometry persistence comes free; the app's own state is saved
        // in `RenameItApp::save`.
        persist_window: true,
        // A portable install keeps its window state with everything else. The
        // whole point is a folder you can copy to a stick and take with you,
        // and a copy that left its layout behind on the machine it was set up
        // on would only be half of one.
        persistence_path: portable.as_ref().map(|root| root.join("app-state.ron")),
        ..Default::default()
    };

    // A bare argument list is a list of files to load, or a single folder to
    // start in — a file dropped on the executable, or a menu entry from before
    // the flags existed. `parse` never fails; see `launch`.
    //
    // A line the Explorer menu wrote is split again without the C runtime's
    // escaping: a drive root arrives quoted as `"E:\"`, which `args_os` reads
    // as `E:"` with the quote still open (see `ren_platform::split_verbatim`).
    let mut argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if argv.iter().any(|arg| arg == "--from-shell")
        && let Some(line) = ren_platform::raw_command_line()
    {
        argv = ren_platform::split_verbatim(&line);
    }
    let mut launch = ren_gui::launch::parse(argv);

    // "Copy filenames to clipboard" — before `run_native`, so nothing is drawn
    // at all. See `launch::copy_names` for why the text survives the process.
    if launch.copy_names {
        match ren_gui::launch::copy_names(&launch.paths) {
            Ok(()) => return Ok(()),
            // There is no stderr in a release build, so a failure opens the
            // window it was trying to avoid — with the files loaded and the
            // reason on screen. The row menu's Copy ▸ Names is then one
            // right-click away, which is a better answer than nothing at all.
            Err(error) => {
                launch.complaint =
                    Some(format!("Could not put the names on the clipboard: {error}"));
            }
        }
    }

    eframe::run_native(
        "RenameIt",
        options,
        Box::new(move |cc| {
            let mut app = ren_gui::RenameItApp::new(&cc.egui_ctx, cc.storage);
            app.start_from(&launch);
            Ok(Box::new(app))
        }),
    )
}
