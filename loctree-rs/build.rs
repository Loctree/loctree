mod build_support;

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support.rs");
    for name in [
        "LOCTREE_GIT_COMMIT",
        "LOCTREE_GIT_DIRTY",
        "LOCTREE_BUILD_VERSION",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
    }
    if let Some(common_dir) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={common_dir}/packed-refs");
        if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={common_dir}/{head_ref}");
        }
    }
    let commit = std::env::var("LOCTREE_GIT_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git(&["rev-parse", "--short=8", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_owned());
    let dirty = std::env::var("LOCTREE_GIT_DIRTY")
        .ok()
        .map(|value| matches!(value.trim(), "1" | "true" | "yes" | "dirty"))
        .unwrap_or_else(|| {
            git(&["status", "--porcelain"]).is_some_and(|status| !status.is_empty())
        });
    let package_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    let build_version = std::env::var("LOCTREE_BUILD_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| build_support::format_build_version(&package_version, &commit, dirty));
    println!("cargo:rustc-env=LOCTREE_GIT_COMMIT={commit}");
    println!(
        "cargo:rustc-env=LOCTREE_GIT_DIRTY={}",
        if dirty { "1" } else { "0" }
    );
    println!("cargo:rustc-env=LOCTREE_BUILD_VERSION={build_version}");
}
