//! Stamps the Windows executable with its icon and version metadata.
//!
//! Only Windows has an executable resource section, so everywhere else this is
//! a no-op. The check is on `CARGO_CFG_TARGET_OS` — the **target** — rather
//! than `cfg(windows)`, which in a build script means the host and would
//! silently drop the icon when cross-compiling to Windows from Linux.
//!
//! The icon is ours — `assets/icon.py` draws it.

fn main() {
    println!("cargo:rerun-if-changed=assets/renameit.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/renameit.ico");
    // What the Properties ▸ Details tab shows. `set` takes the raw VERSIONINFO
    // key names; the version string itself comes from Cargo, so there is one
    // place to change it.
    res.set("ProductName", "RenameIt");
    res.set("FileDescription", "RenameIt — batch file renamer");
    res.set("LegalCopyright", "MIT licensed. See LICENSE.");
    res.set("OriginalFilename", "renameit.exe");

    if let Err(error) = res.compile() {
        // Never fatal: an unbranded executable that works beats a build that
        // refuses to finish. But it is always said out loud, because a release
        // that quietly lost its icon is worse than one that failed to build.
        //
        // The two reasons differ, so the message does. Cross-compiling to
        // Windows from Linux has no resource compiler to find and never will;
        // *on* Windows it means the SDK is missing, which is fixable. Release
        // builds run on `windows-latest` (see `.github/workflows/release.yml`),
        // which is the path that has to work.
        if cfg!(windows) {
            println!(
                "cargo:warning=icon and version metadata not embedded — is the Windows SDK \
                 installed? ({error})"
            );
        } else {
            println!(
                "cargo:warning=icon and version metadata need a Windows host; this cross-build \
                 has none, so the executable is unbranded. Release builds run on windows-latest."
            );
        }
    }
}
