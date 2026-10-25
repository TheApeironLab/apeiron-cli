//! Running as a background service on the user's own machine.
//!
//! The foreground process is the honest default for a first run — you see it
//! connect — but nobody wants to keep a terminal open forever, so this
//! installs this same binary under the platform's own supervisor: launchd on
//! macOS, systemd --user on Linux. Both are per-user and need no root: this
//! program runs as the person whose files it exposes, and asking for sudo to
//! attach a laptop would be the wrong trade.
//!
//! Two details matter more than the boilerplate:
//!
//! - **The token is not in the unit file.** `launchctl print` and
//!   `systemctl cat` are readable by the user's own processes and end up in
//!   pasted logs; the credential lives in a 0600 file the unit points at.
//! - **A clean exit must not be restarted.** The CLI exits 0 when its
//!   token is deleted from the app (see main.rs). `KeepAlive.SuccessfulExit
//!   = false` and `Restart=on-failure` mean "disconnect" stays disconnected,
//!   while a crash or a lost network still comes back.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

/// launchd wants reverse-DNS; systemd wants a filename. Same identity.
#[cfg(target_os = "macos")]
const LABEL: &str = "cn.apeironlab.apeiron";
#[cfg(target_os = "linux")]
const UNIT: &str = "apeiron.service";

pub struct ServiceConfig {
    pub server: String,
    pub token: String,
    pub root: String,
    pub agent: String,
    pub config_home: String,
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set"))
}

/// Where the background service keeps its token. Public so the running
/// process can tell whether it *is* that service before touching its files.
pub fn service_token_path() -> Result<PathBuf> {
    token_path()
}

fn token_path() -> Result<PathBuf> {
    Ok(home()?.join(".config").join("apeiron").join("token"))
}

fn log_path() -> Result<PathBuf> {
    Ok(home()?.join(".local").join("state").join("apeiron").join("apeiron.log"))
}

/// The binary the unit will point at. Resolved now, because `connect` is
/// normally run by the install script from a path that will still be there
/// ($HOME/.local/bin/apeiron), and a unit pointing at a temporary directory
/// is a unit that breaks on the next reboot.
fn binary() -> Result<PathBuf> {
    let path = std::env::current_exe().context("cannot find this binary's own path")?;
    Ok(path.canonicalize().unwrap_or(path))
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("cannot write {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot restrict {}", path.display()))?;
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("cannot run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The arguments the supervised process runs with — the token comes from the
/// file, never from here.
fn arguments(config: &ServiceConfig, binary: &Path, token_file: &Path) -> Vec<String> {
    vec![
        binary.to_string_lossy().into_owned(),
        "serve".into(),
        "--server".into(),
        config.server.clone(),
        "--token-file".into(),
        token_file.to_string_lossy().into_owned(),
        "--root".into(),
        config.root.clone(),
        "--agent".into(),
        config.agent.clone(),
        "--config-home".into(),
        config.config_home.clone(),
    ]
}

/// launchd and systemd both start with a minimal PATH, and the agent this
/// program spawns runs the user's own tools — git, node, whatever a session
/// shells out to. Freeze the PATH of the shell that installed the service:
/// that is the one where those commands work.
///
/// `~/.apeiron/bin` goes in front so the managed scode is what a subprocess
/// finds too. The agent itself is launched by absolute path and does not
/// depend on this; a session that shells out to `scode` is what does.
fn path_env() -> String {
    let inherited = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
    match std::env::var_os("HOME") {
        Some(home) => {
            let managed = Path::new(&home).join(".apeiron").join("bin");
            format!("{}:{inherited}", managed.display())
        }
        None => inherited,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn install(_config: &ServiceConfig) -> Result<()> {
    bail!("apeiron has no background service for this platform; run apeiron connect --foreground")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn install(config: &ServiceConfig) -> Result<()> {
    let token_file = token_path()?;
    write_private(&token_file, &format!("{}\n", config.token))?;
    let log = log_path()?;
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    }
    if let Err(error) = install_unit(config, &binary()?, &token_file, &log) {
        // Leave nothing behind. A token written for a service that does not
        // exist is a credential with no owner, sitting on someone's disk.
        let _ = forget();
        return Err(error.context(
            "could not register the background service (a machine without launchd or systemd --user, \
             such as a container or WSL1, is the usual reason) — `apeiron connect --foreground` runs \
             it in this terminal instead",
        ));
    }
    println!("[apeiron] installed as a background service; logs: {}", log.display());
    println!("[apeiron] stop it from the app (设置 → 本地电脑 → 断开), or run: apeiron disconnect");
    Ok(())
}

/// Delete the unit and the token without asking the supervisor anything.
///
/// This is what the running service itself calls when the app disconnected it: it is
/// running *as* the service, so `launchctl bootout` / `systemctl stop` would
/// kill it mid-cleanup. Removing the files is enough — the clean exit that
/// follows is not restarted, and at the next login there is no unit to load.
/// Returns whether there was a service to forget.
pub fn forget() -> Result<bool> {
    let mut removed = false;
    // `.ok()`: a platform with no unit at all has nothing to forget.
    for path in [unit_path().ok(), token_path().ok()].into_iter().flatten() {
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
            removed = true;
        }
    }
    Ok(removed)
}

pub fn uninstall() -> Result<()> {
    uninstall_unit()?;
    // The credential goes with it: a token file left behind outlives the
    // reason it existed.
    let token_file = token_path()?;
    if token_file.exists() {
        fs::remove_file(&token_file).with_context(|| format!("cannot remove {}", token_file.display()))?;
    }
    println!("[apeiron] the background service is gone");
    Ok(())
}

#[cfg(target_os = "macos")]
fn plist_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(target_os = "macos")]
fn unit_path() -> Result<PathBuf> {
    Ok(home()?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn domain() -> String {
    // std has no uid; `id -u` is always there and this runs once per command.
    let uid: u32 = run("id", &["-u"])
        .ok()
        .and_then(|out| out.trim().parse().ok())
        .unwrap_or(0);
    format!("gui/{uid}")
}

#[cfg(target_os = "macos")]
fn install_unit(config: &ServiceConfig, binary: &Path, token_file: &Path, log: &Path) -> Result<()> {
    let arguments = arguments(config, binary, token_file)
        .iter()
        .map(|value| format!("    <string>{}</string>", plist_escape(value)))
        .collect::<Vec<_>>()
        .join("\n");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <!-- Without the bundle association, Login Items groups this under the certificate owner's name. -->
  <key>AssociatedBundleIdentifiers</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{arguments}
  </array>
  <key>RunAtLoad</key><true/>
  <!-- Restart a crash, but not the clean exit that "disconnect" causes. -->
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Background</string>
  <key>EnvironmentVariables</key><dict>
    <key>PATH</key><string>{path}</string>
  </dict>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        arguments = arguments,
        path = plist_escape(&path_env()),
        log = plist_escape(&log.to_string_lossy()),
    );
    let unit = unit_path()?;
    if let Some(parent) = unit.parent() {
        fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::write(&unit, plist).with_context(|| format!("cannot write {}", unit.display()))?;
    // Reinstalling is the normal path (a new token from the app), so boot the
    // old one out first and ignore the error when there is none.
    let _ = run("launchctl", &["bootout", &format!("{}/{LABEL}", domain())]);
    run("launchctl", &["bootstrap", &domain(), &unit.to_string_lossy()])?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_unit() -> Result<()> {
    let _ = run("launchctl", &["bootout", &format!("{}/{LABEL}", domain())]);
    let unit = unit_path()?;
    if unit.exists() {
        fs::remove_file(&unit).with_context(|| format!("cannot remove {}", unit.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn status() -> Result<()> {
    let unit = unit_path()?;
    if !unit.exists() {
        println!("[apeiron] no background service installed");
        return Ok(());
    }
    match run("launchctl", &["list", LABEL]) {
        Ok(info) => {
            let pid = info
                .lines()
                .find(|line| line.contains("\"PID\""))
                .and_then(|line| line.split('=').nth(1))
                .map(|value| value.trim().trim_end_matches(';').to_string());
            match pid {
                Some(pid) => println!("[apeiron] running (pid {pid}); logs: {}", log_path()?.display()),
                None => println!("[apeiron] installed but not running; logs: {}", log_path()?.display()),
            }
        }
        Err(_) => println!("[apeiron] installed but not loaded"),
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn unit_path() -> Result<PathBuf> {
    Ok(home()?.join(".config").join("systemd").join("user").join(UNIT))
}

#[cfg(target_os = "linux")]
fn install_unit(config: &ServiceConfig, binary: &Path, token_file: &Path, log: &Path) -> Result<()> {
    let command = arguments(config, binary, token_file)
        .iter()
        .map(|value| format!("'{}'", value.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ");
    let unit_body = format!(
        "[Unit]\n\
         Description=apeiron — runs the apeiron agent on this machine\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={command}\n\
         # Restart a crash, but not the clean exit that \"disconnect\" causes.\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         Environment=PATH={path}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        command = command,
        path = path_env(),
    );
    let unit = unit_path()?;
    if let Some(parent) = unit.parent() {
        fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::write(&unit, unit_body).with_context(|| format!("cannot write {}", unit.display()))?;
    run("systemctl", &["--user", "daemon-reload"])?;
    run("systemctl", &["--user", "enable", "--now", UNIT])?;
    run("systemctl", &["--user", "restart", UNIT])?;
    // Without lingering the service dies at logout, which is not what someone
    // installing a background service expects; it needs root, so say so.
    println!(
        "[apeiron] journal: journalctl --user -u {UNIT} -f  (logs also mirrored to {})",
        log.display()
    );
    println!("[apeiron] to keep it running while you are logged out: sudo loginctl enable-linger $USER");
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_unit() -> Result<()> {
    let _ = run("systemctl", &["--user", "disable", "--now", UNIT]);
    let unit = unit_path()?;
    if unit.exists() {
        fs::remove_file(&unit).with_context(|| format!("cannot remove {}", unit.display()))?;
    }
    let _ = run("systemctl", &["--user", "daemon-reload"]);
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn status() -> Result<()> {
    if !unit_path()?.exists() {
        println!("[apeiron] no background service installed");
        return Ok(());
    }
    let state = run("systemctl", &["--user", "is-active", UNIT]).unwrap_or_else(|_| "inactive".into());
    println!("[apeiron] {}", state.trim());
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn unit_path() -> Result<PathBuf> {
    bail!("apeiron has no background service for this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_unit(_: &ServiceConfig, _: &Path, _: &Path, _: &Path) -> Result<()> {
    bail!("apeiron has no background service for this platform; run it in the foreground")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn uninstall_unit() -> Result<()> {
    bail!("apeiron has no background service for this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn status() -> Result<()> {
    bail!("apeiron has no background service for this platform")
}
