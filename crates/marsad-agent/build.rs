//! Embeds the build identifier (`<version>+<git commit>[.dirty]`) and the target triple.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let commit = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let build = format!("{version}+{commit}{}", if dirty { ".dirty" } else { "" });
    println!("cargo:rustc-env=MARSAD_BUILD={build}");
    println!(
        "cargo:rustc-env=MARSAD_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-env-changed=MARSAD_BUILD");
}
