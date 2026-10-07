// apeiron's version is the repository VERSION file (../VERSION from cli/),
// not Cargo.toml: the same number the backend reports and the release tag
// carries (`v<VERSION>`). Cargo.toml keeps a placeholder because Cargo needs
// one; the binary never shows it.
use std::{env, fs, path::Path};

fn main() {
    let version_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../VERSION");
    println!("cargo:rerun-if-changed={}", version_file.display());
    let version =
        fs::read_to_string(&version_file).unwrap_or_else(|e| panic!("cannot read {}: {e}", version_file.display()));
    let version = version.trim();
    let strict_semver = version.split('.').count() == 3
        && version
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    assert!(strict_semver, "VERSION must be X.Y.Z, got {version:?}");
    println!("cargo:rustc-env=APEIRON_VERSION={version}");
}
