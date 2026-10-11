//! One `scode acp` child and its NDJSON framing.
//!
//! apeiron never interprets the conversation: whole JSON-RPC messages go
//! out to the control plane and come back, and the only thing owned here is
//! the process lifecycle. A child dies with its channel, and a channel dies
//! with the socket — a laptop that closed its lid must not leave an agent
//! running against a control plane that has forgotten it.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use crate::protocol::Outbound;
use crate::receiver::Receiver;

/// Environment names a control plane may never set on the agent.
///
/// Everything it sends is application configuration — model credentials,
/// internal service URLs — merged over this machine's own environment so the
/// agent keeps the user's PATH, their SSH agent, their git config. These
/// names are the exception: they choose which code runs, not what it talks
/// to, and a control plane that could set them would have a shell on a
/// machine it does not own.
///
/// Config roots belong on this list for the same reason as `PATH`: scode loads
/// agents, skills, plugins and hooks there. The receiver pins the global root
/// from `--config-home`; project configuration always follows the real cwd.
/// v0.2.25 gives SCODE_GLOBAL_CONFIG_DIR precedence over the legacy spelling.
const NEVER_FROM_SERVER: &[&str] = &[
    "PATH",
    "HOME",
    "SHELL",
    "IFS",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "NODE_OPTIONS",
    "BUN_INSTALL",
    "PYTHONSTARTUP",
    "PYTHONPATH",
    "SUDO_CODE_CONFIG_HOME",
    "SCODE_GLOBAL_CONFIG_DIR",
    "SCODE_PROJECT_CONFIG_DIR",
];

fn allowed_from_server(key: &str) -> bool {
    !NEVER_FROM_SERVER.contains(&key) && !key.starts_with("DYLD_")
}

pub struct AgentChild {
    stdin: mpsc::UnboundedSender<String>,
    /// Dropping this kills the process. Closing stdin is not enough on its
    /// own: an agent that ignores EOF would outlive the conversation it was
    /// spawned for, on a machine apeiron does not own.
    _kill: KillOnDrop,
    finished: JoinHandle<std::io::Result<()>>,
    receiver: Arc<Receiver>,
}

impl AgentChild {
    pub fn send(&self, msg: &Value) -> std::io::Result<()> {
        // The pending child covers its entire lifetime, including notifications
        // and requests without a reply. Never clear it on an RPC response.
        self.receiver.check()?;
        // A closed channel means the child is already gone; its acp.exit is
        // on the way, and the control plane treats that as a dead sandbox.
        let _ = self.stdin.send(format!("{msg}\n"));
        Ok(())
    }

    pub async fn close(self) -> anyhow::Result<()> {
        let Self {
            stdin, _kill, finished, ..
        } = self;
        drop(stdin);
        drop(_kill);
        // Dropping the signal is only the request to stop. Keep this socket's
        // child registered until its supervisor has waited and closed stdio.
        finished.await??;
        Ok(())
    }
}

/// Fires once, when the `AgentChild` goes away. The waiting task holds the
/// `Child`, so this is the only way back to it.
struct KillOnDrop(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

/// Spawn the agent and wire its stdio to the socket.
pub fn spawn(
    ch: String,
    argv: &[String],
    cwd: &std::path::Path,
    config_home: &std::path::Path,
    env: HashMap<String, String>,
    out: mpsc::UnboundedSender<Outbound>,
    receiver: Arc<Receiver>,
) -> anyhow::Result<AgentChild> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("no agent command configured"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in env {
        if allowed_from_server(&key) {
            command.env(key, value);
        }
    }
    // Both spellings must name the projected global root: the current spelling
    // wins in v0.2.25, while older agents still use the legacy one. An inherited
    // project override would bypass the actual session cwd, even when the
    // control plane did not send it, so explicitly remove it from the child.
    command.env("SCODE_GLOBAL_CONFIG_DIR", config_home);
    command.env("SUDO_CODE_CONFIG_HOME", config_home);
    command.env_remove("SCODE_PROJECT_CONFIG_DIR");
    let operation = receiver.begin("acp.child")?;
    receiver.check()?;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            receiver.complete(operation)?;
            return Err(error.into());
        }
    };
    // Announced before anything the agent says, so the control plane can
    // name the process that served every message on this channel.
    let pid = child.id().ok_or_else(|| anyhow::anyhow!("spawned agent has no pid"))?;
    let _ = out.send(Outbound::AcpStarted { ch: ch.clone(), pid });
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let stdin_receiver = receiver.clone();
    let stdin_task = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            // Check at each actual write poll: a queued ACP/permission frame
            // must not get a fresh right merely because the pipe became writable.
            let written = stdin_receiver.write_checked(&mut stdin, line.as_bytes()).await;
            if written.is_err() {
                break;
            }
            let _ = stdin.flush().await;
        }
    });

    let stdout_ch = ch.clone();
    let stdout_out = out.clone();
    let mut output_tasks = JoinSet::new();
    output_tasks.spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(msg) => {
                    let _ = stdout_out.send(Outbound::AcpMsg {
                        ch: stdout_ch.clone(),
                        msg,
                    });
                }
                Err(_) => {
                    // Not JSON-RPC: the agent's own logging on stdout.
                    let _ = stdout_out.send(Outbound::AcpStderr {
                        ch: stdout_ch.clone(),
                        text: format!("[stdout] {}", truncate(trimmed)),
                    });
                }
            }
        }
    });

    let stderr_ch = ch.clone();
    let stderr_out = out.clone();
    output_tasks.spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let _ = stderr_out.send(Outbound::AcpStderr {
                    ch: stderr_ch.clone(),
                    text: truncate(trimmed).to_string(),
                });
            }
        }
    });

    let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel::<()>();
    let finished_receiver = receiver.clone();
    let finished = tokio::spawn(async move {
        let (status, stopped) = tokio::select! {
            status = child.wait() => (status, false),
            // The channel was closed (or the socket dropped): stop the agent
            // rather than waiting for it to notice its stdin ended.
            _ = &mut kill_rx => {
                let _ = child.start_kill();
                (child.wait().await, true)
            }
        };
        stdin_task.abort();
        let _ = stdin_task.await;
        if stopped {
            // Pipe reader tasks are safe to abort after the direct child was
            // waited; unlike the process owner they have no cleanup to perform.
            output_tasks.shutdown().await;
        } else {
            tokio::select! {
                _ = async { while output_tasks.join_next().await.is_some() {} } => {},
                _ = &mut kill_rx => output_tasks.shutdown().await,
            }
        }
        if let Err(error) = &status {
            finished_receiver.fail();
            eprintln!("cannot wait for agent on {ch}: {error}");
        }
        let code = status.as_ref().ok().and_then(|status| status.code());
        status?;
        finished_receiver.complete(operation)?;
        let _ = out.send(Outbound::AcpExit { ch, code });
        Ok(())
    });

    Ok(AgentChild {
        stdin: tx,
        _kill: KillOnDrop(Some(kill_tx)),
        finished,
        receiver,
    })
}

fn truncate(text: &str) -> &str {
    match text.char_indices().nth(500) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The agent must be told where scode's config home is.
    ///
    /// It used to be told nothing, and scode fell back to its own default —
    /// which happened to be the same path apeiron projected plugins into, so
    /// the two agreed by coincidence. Moving the config home turned that into
    /// a silent bug: the projector wrote plugins the agent never read.
    #[tokio::test]
    async fn the_agent_is_told_its_config_home_and_the_server_cannot_change_it() {
        // Set hostile inherited environment only on a subprocess. Mutating this
        // test runner's environment would race parallel tests.
        if std::env::var("APEIRON_TEST_CONFIG_SCOPE_CHILD").as_deref() != Ok("1") {
            let result = std::process::Command::new(std::env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    "agent::tests::the_agent_is_told_its_config_home_and_the_server_cannot_change_it",
                    "--nocapture",
                ])
                .env("APEIRON_TEST_CONFIG_SCOPE_CHILD", "1")
                .env("SUDO_CODE_CONFIG_HOME", "/tmp/host-attacker-legacy")
                .env("SCODE_GLOBAL_CONFIG_DIR", "/tmp/host-attacker-global")
                .env("SCODE_PROJECT_CONFIG_DIR", "/tmp/host-attacker-project")
                .output()
                .expect("isolated inherited environment test");
            assert!(
                result.status.success(),
                "isolated config scope test failed: {}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let (out, mut rx) = mpsc::unbounded_channel();
        let home = std::env::temp_dir().join(format!("apeiron-agent-test-{}", std::process::id()));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).expect("test workspace");
        let receiver = Arc::new(Receiver::open(&home, &workspace, &workspace).expect("receiver"));
        let child = spawn(
            "ch1".into(),
            &[
                "sh".into(),
                "-c".into(),
                r#"printf '{"seen":"%s","global":"%s","project":"%s"}\n' "$SUDO_CODE_CONFIG_HOME" "$SCODE_GLOBAL_CONFIG_DIR" "${SCODE_PROJECT_CONFIG_DIR-}""#.into(),
            ],
            std::path::Path::new("."),
            std::path::Path::new("/tmp/apeiron-test-config-home"),
            // A control plane trying to point the agent at a config home of
            // its choosing — i.e. at agents, skills and hooks of its choosing.
            HashMap::from([
                ("SUDO_CODE_CONFIG_HOME".into(), "/tmp/server-attacker-legacy".into()),
                ("SCODE_GLOBAL_CONFIG_DIR".into(), "/tmp/server-attacker-global".into()),
                ("SCODE_PROJECT_CONFIG_DIR".into(), "/tmp/server-attacker-project".into()),
            ]),
            out,
            receiver,
        )
        .expect("spawn");

        let mut seen = None;
        while let Some(message) = rx.recv().await {
            if let Outbound::AcpMsg { msg, .. } = message {
                seen = Some(msg);
                break;
            }
        }
        child.close().await.expect("child cleanup");
        std::fs::remove_dir_all(home).expect("test cleanup");
        let seen = seen.expect("observed child environment");
        assert_eq!(seen["seen"], "/tmp/apeiron-test-config-home");
        assert_eq!(seen["global"], "/tmp/apeiron-test-config-home");
        assert_eq!(seen["project"], "");
    }

    #[test]
    fn the_names_that_choose_which_code_runs_are_refused() {
        assert!(!allowed_from_server("PATH"));
        assert!(!allowed_from_server("SUDO_CODE_CONFIG_HOME"));
        assert!(!allowed_from_server("SCODE_GLOBAL_CONFIG_DIR"));
        assert!(!allowed_from_server("SCODE_PROJECT_CONFIG_DIR"));
        assert!(!allowed_from_server("DYLD_INSERT_LIBRARIES"));
        // Ordinary application configuration still comes from the server:
        // model credentials and internal service URLs are the whole point.
        assert!(allowed_from_server("ANTHROPIC_API_KEY"));
        assert!(allowed_from_server("CLAUDE_CODE_OAUTH_TOKEN"));
    }
}
