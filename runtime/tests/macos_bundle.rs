//! The macOS service unit must point *inside* the .app bundle.
//!
//! `apeiron` installs to `~/.apeiron/Apeiron.app/Contents/MacOS/apeiron` and
//! leaves `~/.apeiron/bin/apeiron` as a symlink to it, so that macOS has a
//! bundle to take an icon from — without one, 系统设置 → 通用 → 登录项与扩展
//! shows the background service as a generic grey "exec" block.
//!
//! That only holds because `service::binary()` canonicalises `current_exe()`.
//! Run through the symlink, launchd would otherwise be handed the symlink's
//! own path; macOS would see a bare executable again and the icon would go
//! back to being generic — with nothing failing and no test complaining.
//! This tests the actual CLI-generated service registration.

#![cfg(target_os = "macos")]

use std::{fs, os::unix::fs as unix_fs, path::Path, process::Command};

#[test]
fn service_binary_resolves_a_symlink_into_the_app_bundle() {
    let root = std::env::temp_dir().join(format!("apeiron-bundle-test-{}", std::process::id()));
    let macos = root.join("Apeiron.app/Contents/MacOS");
    let bin = root.join("bin");
    fs::create_dir_all(&macos).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let executable = macos.join("apeiron");
    fs::copy(env!("CARGO_BIN_EXE_apeiron"), &executable).unwrap();
    let link = bin.join("apeiron");
    unix_fs::symlink(&executable, &link).unwrap();
    // Exercise the shipped CLI, but never register a test service in the user's
    // launchd domain. Only the OS registration call is replaced.
    let launchctl = bin.join("launchctl");
    fs::write(&launchctl, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&launchctl, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(&link)
        .args(["connect", "--server", "http://127.0.0.1:1", "--token", "fixture"])
        .env("HOME", &root)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let unit = root.join("Library/LaunchAgents/cn.apeironlab.apeiron.plist");
    let parsed = Command::new("plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(unit)
        .output()
        .unwrap();
    assert!(parsed.status.success());
    let plist: serde_json::Value = serde_json::from_slice(&parsed.stdout).unwrap();
    assert_eq!(
        plist["ProgramArguments"][0],
        executable.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(plist["ProgramArguments"][1], "serve");
    let info = Command::new("plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/Info.plist"))
        .output()
        .unwrap();
    let info: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(plist["AssociatedBundleIdentifiers"], info["CFBundleIdentifier"]);
    assert_eq!(info["CFBundleName"], "Apeiron");
    assert_eq!(info["LSUIElement"], true);
    fs::remove_dir_all(root).unwrap();
}

/// The icon the bundle carries has to exist and be a real .icns; a truncated
/// or missing file silently produces the generic icon again.
#[test]
fn committed_icon_is_a_usable_icns() {
    let icon = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/apeiron.icns");
    let bytes = fs::read(&icon).unwrap_or_else(|e| panic!("cannot read {}: {e}", icon.display()));

    // `icns` magic, then the total length the file claims to be.
    assert_eq!(&bytes[..4], b"icns", "{} is not an icns file", icon.display());
    let declared = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    assert_eq!(
        declared,
        bytes.len(),
        "{} claims to be {declared} bytes but is {}; it is truncated",
        icon.display(),
        bytes.len()
    );
}
