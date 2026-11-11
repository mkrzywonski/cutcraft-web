//! Native checks do not require generated WebAssembly. Release binaries always do.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(desktop_assets)");
    let package =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    let assets = package.join("web/desktop/pkg");
    println!("cargo:rerun-if-changed={}", assets.display());
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo package version");
    let stamped =
        fs::read_to_string(assets.join("bundle-version.txt")).is_ok_and(|v| v.trim() == version);
    let complete = stamped
        && ["cutcraft_ui.js", "cutcraft_ui_bg.wasm"]
            .iter()
            .all(|name| assets.join(name).is_file());
    if complete {
        println!("cargo:rustc-cfg=desktop_assets");
    } else if env::var("PROFILE").is_ok_and(|p| p == "release") {
        panic!(
            "Release builds require the current desktop bundle. In the pinned nix-shell, run scripts/build-desktop.sh, then build the server again."
        );
    } else {
        println!(
            "cargo:warning=Desktop assets are absent or from another version. Native checks can run; run scripts/build-desktop.sh and rebuild to use the desktop layout."
        );
    }
}
