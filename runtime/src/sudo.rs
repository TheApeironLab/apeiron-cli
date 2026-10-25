//! Native Sudocode launcher. Model and permission semantics remain owned by scode.
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scode.tar.gz"));
const MANIFEST: &str = include_str!("../../release/sudocode-runtime.json");
const TOOL_DOC: &str = include_str!("../../docs/sudo-tools.md");

#[derive(Deserialize)]
struct Manifest {
    version: String,
    commit: String,
    platforms: BTreeMap<String, Pin>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pin {
    archive_sha256: String,
    archive_size: u64,
    member: String,
    binary_sha256: String,
    binary_size: u64,
}

fn manifest() -> Manifest {
    serde_json::from_str(MANIFEST).expect("checked-in Sudocode runtime manifest")
}

fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("{os}-{arch}")
}

pub(crate) fn build_info() -> serde_json::Value {
    let manifest = manifest();
    serde_json::json!({"version":manifest.version,"commit":manifest.commit,"embedded":!PAYLOAD.is_empty()})
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        bail!("Sudocode state directory must be a directory, not a symlink");
    }
    Ok(())
}

fn verify_binary(path: &Path, pin: &Pin) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() != pin.binary_size {
        bail!(
            "Cached Sudocode runtime is invalid; remove {} and retry",
            path.display()
        );
    }
    if digest(&fs::read(path)?) != pin.binary_sha256 {
        bail!("Cached Sudocode checksum mismatch; remove {} and retry", path.display());
    }
    Ok(())
}

fn unpack_runtime(payload: &[u8], directory: &Path, pin: &Pin) -> Result<PathBuf> {
    if payload.len() as u64 != pin.archive_size || digest(payload) != pin.archive_sha256 {
        bail!("Embedded Sudocode archive checksum mismatch");
    }
    private_dir(directory)?;
    let path = directory.join("scode");
    if path.symlink_metadata().is_ok() {
        verify_binary(&path, pin)?;
        return Ok(path);
    }
    let mut archive = tar::Archive::new(GzDecoder::new(payload));
    let mut binary = None;
    for entry in archive.entries()? {
        let entry = entry?;
        if entry.path()?.as_ref() != Path::new(&pin.member) {
            continue;
        }
        if binary.is_some() || !entry.header().entry_type().is_file() || entry.size() != pin.binary_size {
            bail!("Invalid Sudocode archive member");
        }
        let mut bytes = Vec::new();
        entry.take(pin.binary_size + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != pin.binary_size || digest(&bytes) != pin.binary_sha256 {
            bail!("Sudocode executable checksum mismatch");
        }
        binary = Some(bytes);
    }
    let bytes = binary.context("Sudocode executable missing from archive")?;
    let temporary = directory.join(format!(".scode-{}-{:x}", std::process::id(), rand::random::<u64>()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o700);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        // Publishing a complete inode avoids executing a partially extracted binary.
        // Concurrent launchers may have already installed this exact digest.
        match fs::hard_link(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => verify_binary(&path, pin),
            Err(error) => Err(error.into()),
        }
    })();
    let _ = fs::remove_file(&temporary);
    result?;
    Ok(path)
}

fn runtime(home: &Path) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("APEIRON_SCODE_BIN") {
        let path = PathBuf::from(path);
        if !path.is_absolute()
            || !path.is_file()
            || fs::canonicalize(&path)? == fs::canonicalize(std::env::current_exe()?)?
        {
            bail!("APEIRON_SCODE_BIN must name an absolute scode executable, not apeiron");
        }
        return Ok(path);
    }
    if PAYLOAD.is_empty() {
        bail!("This development build has no Sudocode payload. Set APEIRON_SCODE_BIN to an absolute scode path, or rebuild with APEIRON_SCODE_ARCHIVE. Release builds bundle it offline.");
    }
    let manifest = manifest();
    let pin = manifest
        .platforms
        .get(&platform())
        .context("Unsupported Sudocode platform")?;
    let directory = home.join(".apeiron/runtimes/sudocode").join(&pin.binary_sha256);
    unpack_runtime(PAYLOAD, &directory, pin)
}

fn register_cli(config: &Path, executable: &Path) -> Result<()> {
    let tools = config.join("cli-tools/apeiron/tools");
    private_dir(&tools)?;
    let launcher = tools.join("apeiron");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        match fs::symlink_metadata(&launcher) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if fs::read_link(&launcher)? != executable {
                    // Never replace a user supplied executable or directory.
                    let tmp = tools.join(format!(".apeiron-{:x}", rand::random::<u64>()));
                    symlink(executable, &tmp)?;
                    if let Err(error) = fs::rename(&tmp, &launcher) {
                        let _ = fs::remove_file(&tmp);
                        return Err(error.into());
                    }
                }
            }
            Ok(_) => bail!("CLI package entry already exists: {}", launcher.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = symlink(executable, &launcher) {
                    if error.kind() != std::io::ErrorKind::AlreadyExists || fs::read_link(&launcher)? != executable {
                        return Err(error.into());
                    }
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    // Versioned documentation is immutable so concurrent sessions can read it.
    let doc = tools.join("APEIRON-CLI.md");
    match OpenOptions::new().write(true).create_new(true).open(&doc) {
        Ok(mut file) => file.write_all(TOOL_DOC.as_bytes())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn launch(args: &[OsString]) -> Result<i32> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is required for Sudocode")?);
    let binary = runtime(&home)?;
    let config = std::env::var_os("SCODE_GLOBAL_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("SUDO_CODE_CONFIG_HOME").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".apeiron/sudo"));
    let executable = fs::canonicalize(std::env::current_exe()?)?;
    let informational = args.len() == 1 && ["--help", "-h", "--version", "-V"].iter().any(|s| args[0] == *s);
    if !informational {
        register_cli(&config, &executable)?;
    }
    let mut paths = vec![executable
        .parent()
        .context("Cannot locate Apeiron directory")?
        .to_owned()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let mut command = Command::new(binary);
    command
        .args(args)
        .env("SCODE_GLOBAL_CONFIG_DIR", config)
        .env("SCODE_CHECK_UPDATES", "0")
        .env("PATH", std::env::join_paths(paths)?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("Cannot start Sudocode")
    }
    #[cfg(not(unix))]
    Ok(command.status()?.code().unwrap_or(1))
}

pub(crate) fn run(args: &[OsString]) -> i32 {
    match launch(args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("apeiron sudo: {error:#}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_the_pinned_member_and_rejects_modified_cache() {
        let bytes = b"#!/bin/sh\nexit 0\n";
        let mut archive = tar::Builder::new(Vec::new());
        for (name, content) in [
            ("package/scode", bytes.as_slice()),
            ("unrelated", b"ignored".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, name, content).unwrap();
        }
        let tar = archive.into_inner().unwrap();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&tar).unwrap();
        let payload = gzip.finish().unwrap();
        let pin = Pin {
            archive_sha256: digest(&payload),
            archive_size: payload.len() as u64,
            member: "package/scode".into(),
            binary_sha256: digest(bytes),
            binary_size: bytes.len() as u64,
        };
        let directory = std::env::temp_dir().join(format!("sudo-extract-{:x}", rand::random::<u64>()));
        let path = unpack_runtime(&payload, &directory, &pin).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!directory.join("unrelated").exists());
        assert_eq!(unpack_runtime(&payload, &directory, &pin).unwrap(), path);
        fs::write(&path, b"tampered").unwrap();
        assert!(unpack_runtime(&payload, &directory, &pin).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"tampered");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn refuses_corrupt_payload_before_creating_runtime() {
        let manifest = manifest();
        let pin = manifest.platforms.get("linux-x64").unwrap();
        let destination = std::env::temp_dir().join(format!("sudo-corrupt-{:x}", rand::random::<u64>()));
        assert!(unpack_runtime(b"corrupt", &destination, pin).is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn pinned_recipe_and_supported_platforms_match() {
        let pins = manifest();
        let recipe: serde_json::Value = serde_json::from_str(include_str!("../../release/recipe.json")).unwrap();
        assert_eq!(pins.version, recipe["sudocode"]["version"]);
        assert_eq!(pins.commit, recipe["sudocode"]["commit"]);
        assert_eq!(pins.platforms.len(), 4);
        for target in ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64"] {
            let pin = &pins.platforms[target];
            assert_eq!(pin.binary_sha256.len(), 64);
            assert_eq!(pin.archive_sha256.len(), 64);
        }
    }
}
