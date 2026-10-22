use std::{env, path::PathBuf, process::Command};
mod build_version;
fn git(root: &PathBuf, args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).current_dir(root).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .to_owned();
    let revision = git(&root, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let clean = git(
        &root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )
    .is_some_and(|s| s.is_empty());
    let submodules_ready = Command::new("git")
        .args(["submodule", "status", "--recursive"])
        .current_dir(&root)
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .all(|line| line.starts_with(' '))
        });
    if env::var("PROFILE").as_deref() == Ok("release") && (!clean || !submodules_ready || revision.len() != 40) {
        panic!("Release binaries require a clean committed source checkout");
    }
    if let Ok(expected) = env::var("APEIRON_RELEASE_SOURCE_REVISION") {
        assert!(
            clean && submodules_ready && expected == revision,
            "Release commit does not match the clean source checkout"
        );
    }
    let compiler = Command::new(env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("--version")
        .output()
        .expect("Cannot identify Rust compiler");
    assert!(compiler.status.success(), "Cannot identify Rust compiler");
    let version = build_version::resolve(
        env!("CARGO_PKG_VERSION"),
        env::var("APEIRON_BUILD_VERSION").ok().as_deref(),
    )
    .expect("Invalid release build version");
    println!("cargo:rustc-env=APEIRON_VERSION={version}");
    println!("cargo:rustc-env=APEIRON_SOURCE_REVISION={revision}");
    println!("cargo:rustc-env=APEIRON_SOURCE_CLEAN={clean}");
    println!(
        "cargo:rustc-env=APEIRON_BUILD_RUSTC={}",
        String::from_utf8_lossy(&compiler.stdout).trim()
    );
    println!("cargo:rerun-if-env-changed=APEIRON_RELEASE_SOURCE_REVISION");
    println!("cargo:rerun-if-env-changed=APEIRON_BUILD_VERSION");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_version.rs");
    println!("cargo:rerun-if-changed=../Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../Cargo.lock");
    println!("cargo:rerun-if-changed=../.gitmodules");
    println!("cargo:rerun-if-changed=../third_party/zygos");
    println!("cargo:rerun-if-changed=../third_party/apeiron");
    println!("cargo:rerun-if-changed=../rust-toolchain.toml");
    for name in ["HEAD", "index"] {
        if let Some(path) = git(&root, &["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={}", root.join(path).display());
        }
    }
    if let Some(reference) = git(&root, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(&root, &["rev-parse", "--git-path", &reference]) {
            println!("cargo:rerun-if-changed={}", root.join(path).display());
        }
    }
}
