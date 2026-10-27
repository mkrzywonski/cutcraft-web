//! Stamps the build with the commit it came from, so "1.2.3 (abc1234)" says exactly which build
//! is running. Without git (a source download), the version stands alone.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !text.trim().is_empty()).then(|| text.trim().to_owned())
}

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    // A build without the git history (Nix) passes its commit in instead.
    let commit = std::env::var("CUTCRAFT_COMMIT")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| git(&["rev-parse", "--short=7", "HEAD"]));
    let build = match commit {
        Some(commit) => format!("{version} ({commit})"),
        None => version.to_owned(),
    };
    println!("cargo:rustc-env=CUTCRAFT_BUILD={build}");
    // Stamp again when HEAD moves: a commit, checkout or reset.
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
    }
    // Branch refs live in the shared directory, which differs from the above in a worktree.
    if let Some(common) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={common}/packed-refs");
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={common}/{head}");
        }
    }
    println!("cargo:rerun-if-env-changed=CUTCRAFT_COMMIT");
    println!("cargo:rerun-if-changed=build.rs");
}
