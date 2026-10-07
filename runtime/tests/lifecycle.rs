//! The CLI executable speaks v2 to a real loopback peer and owns real children.
//! Barriers are files in this fixture's directory; no model or user service runs.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, WebSocketStream};

static NEXT: AtomicU64 = AtomicU64::new(0);

const OWNER_BOOT: &str = "00000000-0000-4000-8000-000000000001";

async fn receive(peer: &mut WebSocketStream<TcpStream>, kind: &str) -> Value {
    timeout(Duration::from_secs(5), async {
        loop {
            match peer.next().await.unwrap().unwrap() {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["t"] == kind {
                        return value;
                    }
                    if value["t"] == "permit.challenge" {
                        peer.send(Message::Text(json!({"t":"permit","nonce":value["nonce"]}).to_string()))
                            .await
                            .unwrap();
                    }
                }
                Message::Close(frame) => panic!("unexpected close: {frame:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("CLI response")
}

async fn announce(peer: &mut WebSocketStream<TcpStream>) -> Value {
    peer.send(Message::Text(
        json!({"t":"welcome","protocol":2,"user_id":"fixture","tenant_id":"fixture-tenant"}).to_string(),
    ))
    .await
    .unwrap();
    receive(peer, "ready").await
}

fn scope(ready: &Value, epoch: u64, boot: &str) -> Value {
    let binding = format!("{:032x}", NEXT.fetch_add(1, Ordering::Relaxed) + 1);
    let link = format!(
        "machine-{}-{}-{}-{}-{}",
        &binding[..8],
        &binding[8..12],
        &binding[12..16],
        &binding[16..20],
        &binding[20..]
    );
    json!({"binding_id":binding,"tenant_id":"fixture-tenant","user_id":"fixture","runtime":"local",
        "owner_boot_id":boot,"epoch":epoch.to_string(),"receiver_id":ready["receiver"]["receiver_id"],"link_id":link})
}

async fn handshake(peer: &mut WebSocketStream<TcpStream>, epoch: u64, boot: &str) -> Value {
    let ready = announce(peer).await;
    let binding = scope(&ready, epoch, boot);
    peer.send(Message::Text(
        json!({"t":"bind","scope":binding,"nonce":ready["nonce"]}).to_string(),
    ))
    .await
    .unwrap();
    let bound = receive(peer, "bound").await;
    assert_eq!(bound["scope"], binding);
    assert_eq!(bound["previous"], ready["receiver"]);
    receive(peer, "permit.accepted").await;
    bound
}

struct Fixture {
    root: PathBuf,
    cli: Child,
    peer: WebSocketStream<TcpStream>,
    listener: TcpListener,
}

impl Fixture {
    async fn start(mode: &str) -> Self {
        let mut fixture = Self::inert(mode).await;
        handshake(&mut fixture.peer, 1, OWNER_BOOT).await;
        fixture
    }

    async fn inert(mode: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "apeiron-cli-lifecycle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let agent = format!(
            "{} --exact process_fixture --nocapture",
            std::env::current_exe().unwrap().display()
        );
        let cli = Command::new(env!("CARGO_BIN_EXE_apeiron"))
            .args([
                "serve",
                "--server",
                &format!("http://{}", listener.local_addr().unwrap()),
            ])
            .args(["--token", "fixture-token", "--root"])
            .arg(&root)
            .arg("--config-home")
            .arg(root.join("config"))
            .args(["--agent", &agent])
            .env("APEIRON_TEST_CHILD_ROOT", &root)
            .env("APEIRON_TEST_CHILD_MODE", mode)
            // Only this subprocess uses the fixture home. Never touch the
            // developer's receiver registry or background-service evidence.
            .env("HOME", root.with_extension("home"))
            // A manual fixture must never inherit the user's service token path.
            .env_remove("APEIRON_TOKEN_FILE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (socket, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let peer = accept_async(socket).await.unwrap();
        Self {
            root,
            cli,
            peer,
            listener,
        }
    }

    async fn send(&mut self, value: Value) {
        self.peer.send(Message::Text(value.to_string())).await.unwrap();
    }

    async fn receive(&mut self, kind: &str) -> Value {
        receive(&mut self.peer, kind).await
    }

    async fn body(&mut self, id: u32, bytes: &[u8], end: bool) {
        let mut frame = vec![0];
        frame.extend_from_slice(&id.to_be_bytes());
        frame.extend_from_slice(bytes);
        self.peer.send(Message::Binary(frame)).await.unwrap();
        if end {
            let mut frame = vec![1];
            frame.extend_from_slice(&id.to_be_bytes());
            self.peer.send(Message::Binary(frame)).await.unwrap();
        }
    }

    fn process_args(&self) -> Value {
        json!([
            std::env::current_exe().unwrap(),
            "--exact",
            "process_fixture",
            "--nocapture"
        ])
    }

    async fn process(&mut self, timeout_ms: Option<u64>, stdin: bool) {
        self.send(json!({ "t": "req", "id": 7, "op": "proc.run", "stdin": stdin,
            "args": { "argv": self.process_args(), "timeout_ms": timeout_ms } }))
            .await;
    }

    async fn pid(&self) -> u32 {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(self.root.join("pid")) {
                    if let Ok(pid) = pid.parse() {
                        return pid;
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture child started")
    }

    async fn close(&mut self, code: u16) {
        self.peer
            .close(Some(CloseFrame {
                code: code.into(),
                reason: "fixture close".into(),
            }))
            .await
            .unwrap();
    }

    async fn exit(&mut self) {
        let status = timeout(Duration::from_secs(5), self.cli.wait())
            .await
            .expect("CLI cleanup completed")
            .unwrap();
        assert!(status.success(), "{status}");
    }

    async fn assert_stopped(&self, pid: u32) {
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap()
            .success();
        assert!(!alive, "owned child {pid} must be reaped before cleanup completes");
        std::fs::write(self.root.join("release"), b"release").unwrap();
        sleep(Duration::from_millis(50)).await;
        assert!(!self.root.join("late-write").exists());
    }

    fn another(&self, workspace: &std::path::Path, config: &std::path::Path) -> Child {
        Command::new(env!("CARGO_BIN_EXE_apeiron"))
            .args([
                "serve",
                "--server",
                &format!("http://{}", self.listener.local_addr().unwrap()),
                "--token",
                "fixture-token",
                "--root",
            ])
            .arg(workspace)
            .arg("--config-home")
            .arg(config)
            .env("HOME", self.root.with_extension("home"))
            .env_remove("APEIRON_TOKEN_FILE")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    async fn refused(&self, workspace: &std::path::Path, config: &std::path::Path, reason: &str) {
        let child = self.another(workspace, config);
        let output = timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .expect("refused serve exits")
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(reason),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    async fn connect_another(
        &self,
        workspace: &std::path::Path,
        config: &std::path::Path,
    ) -> (Child, WebSocketStream<TcpStream>) {
        let (child, mut peer) = self.raw_another(workspace, config).await;
        handshake(&mut peer, 1, OWNER_BOOT).await;
        (child, peer)
    }

    async fn raw_another(
        &self,
        workspace: &std::path::Path,
        config: &std::path::Path,
    ) -> (Child, WebSocketStream<TcpStream>) {
        let child = self.another(workspace, config);
        let (socket, _) = timeout(Duration::from_secs(5), self.listener.accept())
            .await
            .expect("second serve connects")
            .unwrap();
        let peer = accept_async(socket).await.unwrap();
        (child, peer)
    }

    fn block_evidence_writes(&self) {
        std::fs::create_dir(
            self.root
                .with_extension("home")
                .join(".local/state/apeiron/receivers/1/state.tmp"),
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.cli.start_kill();
        if let Ok(pid) = std::fs::read_to_string(self.root.join("descendant-pid")) {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", pid.trim()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(self.root.with_extension("home"));
    }
}

/// This test executable is also the real subprocess. Only the explicit
/// detached mode forks; its descendant PID is recorded for fixture cleanup.
#[test]
fn process_fixture() {
    let Ok(root) = std::env::var("APEIRON_TEST_CHILD_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    std::fs::write(root.join("pid"), std::process::id().to_string()).unwrap();
    if std::env::var("APEIRON_TEST_CHILD_MODE").as_deref() == Ok("detached") {
        let mut descendant = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "descendant_fixture", "--nocapture"])
            .env("APEIRON_TEST_CHILD_MODE", "descendant")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Reap if it exits early; this fixture process deliberately exits
        // before the surviving descendant, which the outer fixture cleans.
        std::thread::spawn(move || descendant.wait());
        let end = Instant::now() + Duration::from_secs(3);
        while !root.join("descendant-pid").exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(root.join("descendant-pid").exists());
        return;
    }

    if std::env::var("APEIRON_TEST_CHILD_MODE").as_deref() == Ok("reply") {
        println!("{}", json!({"jsonrpc":"2.0","id":1,"result":{"stopReason":"end_turn"}}));
        std::io::stdout().flush().unwrap();
    }
    if std::env::var("APEIRON_TEST_CHILD_MODE").as_deref() == Ok("echo") {
        // Fill stdout before reading a large stdin: both pipes must be serviced.
        std::io::stdout().write_all(b"[stdout-start]").unwrap();
        std::io::stdout().write_all(&vec![b'x'; 512 * 1024]).unwrap();
        std::io::stdout().write_all(b"[stdout-end]").unwrap();
        std::io::stdout().flush().unwrap();
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes).unwrap();
        std::fs::write(root.join("input"), &bytes).unwrap();
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut replied = false;
    while Instant::now() < deadline {
        if !replied && root.join("reply").exists() {
            println!("{}", json!({"jsonrpc":"2.0","id":1,"result":{"stopReason":"end_turn"}}));
            std::io::stdout().flush().unwrap();
            replied = true;
        }
        if root.join("release").exists() {
            std::fs::write(root.join("late-write"), b"old child wrote").unwrap();
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[tokio::test]
async fn socket_close_waits_for_request_and_agent_children_before_exit_or_reconnect() {
    for code in [4002, 4006, 1000] {
        for agent in [false, true] {
            let mut fixture = Fixture::start("wait").await;
            if agent {
                fixture.send(json!({ "t": "acp.open", "ch": "child" })).await;
                fixture.receive("acp.started").await;
            } else {
                fixture.process(None, false).await;
            }
            let pid = fixture.pid().await;
            fixture.close(code).await;
            if code != 1000 {
                fixture.exit().await;
            } else {
                let (socket, _) = timeout(Duration::from_secs(5), fixture.listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                fixture.peer = accept_async(socket).await.unwrap();
                handshake(&mut fixture.peer, 1, OWNER_BOOT).await;
            }
            fixture.assert_stopped(pid).await;
            if code == 1000 {
                fixture.close(4002).await;
                fixture.exit().await;
            }
        }
    }
}

#[tokio::test]
async fn truncated_input_never_overwrites_a_file_or_starts_a_process() {
    let mut fixture = Fixture::start("wait").await;
    let path = fixture.root.join("upload");
    std::fs::write(&path, b"original").unwrap();
    fixture
        .send(json!({ "t": "req", "id": 2, "op": "fs.write", "stdin": true,
        "args": { "path": path, "exclusive": false } }))
        .await;
    fixture.body(2, b"partial", false).await;
    fixture.process(None, true).await;
    fixture.body(7, b"partial", false).await;
    fixture.send(json!({ "t": "ping", "id": 8 })).await;
    fixture.receive("pong").await;
    fixture.close(4002).await;
    fixture.exit().await;
    assert_eq!(std::fs::read(path).unwrap(), b"original");
    assert!(!fixture.root.join("pid").exists());
}

#[tokio::test]
async fn process_timeout_reaps_the_child_even_while_stdin_is_blocked() {
    let mut fixture = Fixture::start("wait").await;
    fixture.process(Some(500), true).await;
    fixture.body(7, &vec![b'x'; 512 * 1024], true).await;
    let pid = fixture.pid().await;
    let response = fixture.receive("res").await;
    assert_eq!(response["ok"], true);
    assert_eq!(response["value"]["exit_code"], 124);
    fixture.assert_stopped(pid).await;
    fixture.close(4002).await;
    fixture.exit().await;
}

#[tokio::test]
async fn closing_an_acp_channel_waits_for_its_child_and_keeps_the_socket() {
    let mut fixture = Fixture::start("wait").await;
    fixture.send(json!({ "t": "acp.open", "ch": "child" })).await;
    fixture.receive("acp.started").await;
    let pid = fixture.pid().await;
    fixture.send(json!({ "t": "acp.close", "ch": "child" })).await;
    fixture.receive("acp.exit").await;
    fixture.assert_stopped(pid).await;
    fixture.send(json!({ "t": "ping", "id": 8 })).await;
    fixture.receive("pong").await;
    fixture.close(4002).await;
    fixture.exit().await;
}

#[tokio::test]
async fn complete_input_and_large_bidirectional_output_keep_the_v1_result_contract() {
    let mut fixture = Fixture::start("echo").await;
    let bytes = vec![b'i'; 512 * 1024];
    fixture.process(Some(3000), true).await;
    fixture.body(7, &bytes, true).await;
    // Text and binary output use separate writer queues; the response may
    // arrive before BODY_END. The contract completes only after both do.
    let (response, stdout) = timeout(Duration::from_secs(5), async {
        let mut response = None;
        let mut stdout = Vec::new();
        let mut ended = false;
        while response.is_none() || !ended {
            match fixture.peer.next().await.unwrap().unwrap() {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["t"] == "res" && value["id"] == 7 {
                        response = Some(value);
                    }
                }
                Message::Binary(frame) => {
                    assert!(frame.len() >= 5);
                    assert_eq!(u32::from_be_bytes(frame[1..5].try_into().unwrap()), 7);
                    match frame[0] {
                        0 => stdout.extend_from_slice(&frame[5..]),
                        1 => ended = true,
                        other => panic!("unknown body frame {other}"),
                    }
                }
                other => panic!("unexpected message {other:?}"),
            }
        }
        (response.unwrap(), stdout)
    })
    .await
    .expect("complete stdout and result");
    assert_eq!(response["ok"], true);
    assert_eq!(response["body"], true);
    assert_eq!(response["value"]["exit_code"], 0);
    let prefix = b"[stdout-start]";
    let suffix = b"[stdout-end]";
    let start = stdout.windows(prefix.len()).position(|part| part == prefix).unwrap() + prefix.len();
    let end = stdout.windows(suffix.len()).position(|part| part == suffix).unwrap();
    assert_eq!(&stdout[start..end], vec![b'x'; 512 * 1024]);
    assert_eq!(std::fs::read(fixture.root.join("input")).unwrap(), bytes);
    fixture.close(4002).await;
    fixture.exit().await;
}

#[tokio::test]
async fn sigkill_leaves_unknown_work_and_manual_serve_cannot_bypass_it() {
    for agent in [false, true] {
        let mut fixture = Fixture::start("wait").await;
        if agent {
            fixture.send(json!({"t":"acp.open","ch":"old"})).await;
            fixture.receive("acp.started").await;
        } else {
            fixture.process(None, false).await;
        }
        fixture.pid().await;
        fixture.cli.kill().await.unwrap();
        fixture.cli.wait().await.unwrap();
        // Parent death and socket loss do not terminate this real child.
        std::fs::write(fixture.root.join("release"), b"release").unwrap();
        timeout(Duration::from_secs(5), async {
            while !fixture.root.join("late-write").exists() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("orphan demonstrated its visible effect");
        fixture
            .refused(&fixture.root, &fixture.root.join("config"), "UNKNOWN")
            .await;
        // A second manual attempt or a changed config spelling is no escape.
        fixture
            .refused(
                &fixture.root.join("nested"),
                &fixture.root.join("new-config"),
                "UNKNOWN",
            )
            .await;
    }
}

#[tokio::test]
async fn resource_claims_cover_shared_config_nested_roots_aliases_and_renames() {
    let mut fixture = Fixture::start("wait").await;
    fixture
        .refused(&fixture.root, &fixture.root.join("config"), "already in use")
        .await;
    fixture
        .refused(
            &fixture.root.join("nested"),
            &fixture.root.join("other-config"),
            "already in use",
        )
        .await;
    // The same HOME owns two independent resource sets. Neither pair hashing
    // nor one lock for all serve processes satisfies both boundaries.
    let sibling = fixture.root.with_extension("sibling");
    let isolated_config = sibling.join("config");
    std::fs::create_dir_all(&sibling).unwrap();
    assert!(
        fixture.root.join("config").is_dir(),
        "receiver prepares an initially absent config root"
    );
    let moved_config = sibling.join("moved-config");
    std::fs::rename(fixture.root.join("config"), &moved_config).unwrap();
    fixture.refused(&sibling, &moved_config, "already in use").await;
    std::fs::rename(&moved_config, fixture.root.join("config")).unwrap();
    fixture
        .refused(&sibling, &fixture.root.join("config"), "already in use")
        .await;
    #[cfg(unix)]
    {
        let alias = sibling.join("alias");
        std::os::unix::fs::symlink(&fixture.root, &alias).unwrap();
        fixture.refused(&alias, &isolated_config, "already in use").await;
    }
    let (mut independent, mut peer) = fixture.connect_another(&sibling, &isolated_config).await;
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "done".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), independent.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    let moved = sibling.join("renamed");
    std::fs::rename(&fixture.root, &moved).unwrap();
    fixture
        .refused(&moved.join("child"), &isolated_config, "already in use")
        .await;
    std::fs::rename(&moved, &fixture.root).unwrap();
    fixture.close(4002).await;
    fixture.exit().await;
    std::fs::remove_dir_all(sibling).unwrap();
}

#[tokio::test]
async fn exposed_roots_cannot_cover_or_create_entries_inside_receiver_control_state() {
    let mut fixture = Fixture::start("wait").await;
    let home = fixture.root.with_extension("home");
    let registry = home.join(".local/state/apeiron/receivers");
    let reason = "workspace/config must not cover or reside inside the receiver control state directory";
    fixture.refused(&home, &fixture.root.join("config"), reason).await;
    fixture.refused(&fixture.root, &home, reason).await;
    let nested = registry.join("must-not-create");
    fixture.refused(&nested, &fixture.root.join("config"), reason).await;
    fixture.refused(&fixture.root, &nested, reason).await;
    assert!(!nested.exists(), "preflight must not corrupt the receiver registry");
    #[cfg(unix)]
    {
        let alias = fixture.root.join("control-alias");
        std::os::unix::fs::symlink(&registry, &alias).unwrap();
        fixture.refused(&alias, &fixture.root.join("config"), reason).await;
    }
    fixture.send(json!({"t":"ping","id":8})).await;
    fixture.receive("pong").await;
    fixture.close(4002).await;
    fixture.exit().await;
    let (mut restarted, mut peer) = fixture
        .connect_another(&fixture.root, &fixture.root.join("config"))
        .await;
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "done".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), restarted.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn failed_admission_evidence_stops_before_any_process_side_effect() {
    let mut fixture = Fixture::start("wait").await;
    fixture.block_evidence_writes();
    fixture.process(None, false).await;
    fixture.exit().await;
    assert!(!fixture.root.join("pid").exists());
}

#[tokio::test]
async fn failed_completion_evidence_never_returns_success_or_unlocks_unknown_work() {
    let mut fixture = Fixture::start("wait").await;
    fixture.process(None, false).await;
    fixture.pid().await;
    fixture.block_evidence_writes();
    std::fs::write(fixture.root.join("release"), b"release").unwrap();
    timeout(Duration::from_secs(5), async {
        while let Some(Ok(message)) = fixture.peer.next().await {
            if let Message::Text(text) = message {
                let value: Value = serde_json::from_str(&text).unwrap();
                assert_ne!(value["t"], "res", "no success/error result can claim saved completion");
            }
        }
    })
    .await
    .expect("receiver closes on evidence failure");
    fixture.exit().await;
    assert!(fixture.root.join("late-write").exists());
    // Repairing storage does not manufacture the lost cleanup observation.
    std::fs::remove_dir(
        fixture
            .root
            .with_extension("home")
            .join(".local/state/apeiron/receivers/1/state.tmp"),
    )
    .unwrap();
    fixture
        .refused(&fixture.root, &fixture.root.join("config"), "UNKNOWN")
        .await;
}

#[tokio::test]
async fn an_acp_reply_is_not_child_termination_and_normal_close_allows_restart() {
    let mut fixture = Fixture::start("reply").await;
    fixture.send(json!({"t":"acp.open","ch":"child"})).await;
    fixture.receive("acp.started").await;
    let response = fixture.receive("acp.msg").await;
    assert_eq!(response["msg"]["result"]["stopReason"], "end_turn");
    fixture
        .refused(&fixture.root, &fixture.root.join("config"), "already in use")
        .await;
    let pid = fixture.pid().await;
    fixture.close(4002).await;
    fixture.exit().await;
    fixture.assert_stopped(pid).await;
    let (mut restarted, mut peer) = fixture
        .connect_another(&fixture.root, &fixture.root.join("config"))
        .await;
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "done".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), restarted.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn received_acp_result_still_blocks_restart_when_the_child_was_never_waited() {
    let mut fixture = Fixture::start("reply").await;
    fixture.send(json!({"t":"acp.open","ch":"child"})).await;
    fixture.receive("acp.started").await;
    fixture.receive("acp.msg").await;
    fixture.cli.kill().await.unwrap();
    fixture.cli.wait().await.unwrap();
    std::fs::write(fixture.root.join("release"), b"release").unwrap();
    timeout(Duration::from_secs(5), async {
        while !fixture.root.join("late-write").exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ACP response did not stop its child");
    fixture
        .refused(&fixture.root, &fixture.root.join("config"), "UNKNOWN")
        .await;
}

#[tokio::test]
async fn losing_the_reply_does_not_lose_cleanup_or_replay_a_v1_request() {
    let mut fixture = Fixture::start("wait").await;
    fixture.process(None, false).await;
    let pid = fixture.pid().await;
    // Close the peer without consuming any result. Cleanup still must wait
    // the child and persist the terminal fact before this receiver exits.
    fixture.close(4002).await;
    fixture.exit().await;
    fixture.assert_stopped(pid).await;
    let (mut restarted, mut peer) = fixture
        .connect_another(&fixture.root, &fixture.root.join("config"))
        .await;
    peer.send(Message::Text(json!({"t":"ping","id":7}).to_string()))
        .await
        .unwrap();
    let message = timeout(Duration::from_secs(5), peer.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let message: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    assert_eq!(message, json!({"t":"pong","id":7}));
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "done".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), restarted.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn local_business_before_a_bound_grant_has_no_side_effect() {
    let mut fixture = Fixture::inert("wait").await;
    announce(&mut fixture.peer).await;
    fixture.process(None, false).await;
    assert!(timeout(Duration::from_secs(5), fixture.cli.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    assert!(!fixture.root.join("pid").exists());
}

#[tokio::test]
async fn replayed_permit_closes_admission_and_queued_body_cannot_borrow_a_new_socket() {
    let mut fixture = Fixture::inert("wait").await;
    let ready = announce(&mut fixture.peer).await;
    fixture
        .send(json!({"t":"bind","scope":scope(&ready,1,OWNER_BOOT),"nonce":ready["nonce"]}))
        .await;
    fixture.receive("bound").await;
    let challenge = fixture.receive("permit.challenge").await;
    fixture.send(json!({"t":"permit","nonce":challenge["nonce"]})).await;
    fixture.receive("permit.accepted").await;
    let file = fixture.root.join("queued-write");
    std::fs::write(&file, b"original").unwrap();
    fixture
        .send(json!({"t":"req","id":98,"op":"fs.write","stdin":true,"args":{"path":file,"exclusive":false}}))
        .await;
    fixture.body(98, b"stale", false).await;
    fixture.send(json!({"t":"permit","nonce":challenge["nonce"]})).await;
    assert!(timeout(Duration::from_secs(5), fixture.cli.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    assert_eq!(std::fs::read(&file).unwrap(), b"original");
    let (mut next, mut peer) = fixture
        .connect_another(&fixture.root, &fixture.root.join("config"))
        .await;
    let mut end = vec![1];
    end.extend_from_slice(&98u32.to_be_bytes());
    peer.send(Message::Binary(end)).await.unwrap();
    peer.send(Message::Text(json!({"t":"ping","id":3}).to_string()))
        .await
        .unwrap();
    receive(&mut peer, "pong").await;
    assert_eq!(std::fs::read(&file).unwrap(), b"original");
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "test complete".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), next.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn delayed_permit_after_a_paused_receiver_deadline_never_starts_queued_work() {
    let mut fixture = Fixture::inert("wait").await;
    let ready = announce(&mut fixture.peer).await;
    fixture
        .send(json!({"t":"bind","scope":scope(&ready,1,OWNER_BOOT),"nonce":ready["nonce"]}))
        .await;
    fixture.receive("bound").await;
    let challenge = fixture.receive("permit.challenge").await;
    let pid = fixture.cli.id().unwrap();
    assert!(Command::new("kill")
        .args(["-STOP", &pid.to_string()])
        .status()
        .await
        .unwrap()
        .success());
    fixture.send(json!({"t":"permit","nonce":challenge["nonce"]})).await;
    fixture.process(None, false).await;
    sleep(Duration::from_millis(10_200)).await;
    assert!(Command::new("kill")
        .args(["-CONT", &pid.to_string()])
        .status()
        .await
        .unwrap()
        .success());
    sleep(Duration::from_millis(300)).await;
    assert!(!fixture.root.join("pid").exists());
}

#[tokio::test]
async fn local_scope_high_water_survives_clean_restart_and_rejects_wrong_boot_or_lower_epoch() {
    let mut fixture = Fixture::start("wait").await;
    fixture.close(4002).await;
    fixture.exit().await;
    let (mut child, mut peer) = fixture.raw_another(&fixture.root, &fixture.root.join("config")).await;
    let boot_b = "00000000-0000-4000-8000-000000000002";
    let bound = handshake(&mut peer, 2, boot_b).await;
    assert_eq!(bound["previous"]["process_history"], false);
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "idle turnover".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    for (epoch, boot) in [(2, OWNER_BOOT), (1, boot_b)] {
        let (child, mut peer) = fixture.raw_another(&fixture.root, &fixture.root.join("config")).await;
        let ready = announce(&mut peer).await;
        assert_eq!(ready["receiver"]["prior_scope"]["epoch"], "2");
        peer.send(Message::Text(
            json!({"t":"bind","scope":scope(&ready,epoch,boot),"nonce":ready["nonce"]}).to_string(),
        ))
        .await
        .unwrap();
        let output = timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("stale or mismatched receiver scope"));
    }
}

#[tokio::test]
async fn completed_process_history_reconnects_to_a_new_backend_without_replaying_work() {
    let mut fixture = Fixture::start("echo").await;
    fixture.process(None, true).await;
    fixture.body(7, b"", true).await;
    assert_eq!(fixture.receive("res").await["ok"], true);
    fixture.close(4002).await;
    fixture.exit().await;
    let (mut next, mut peer) = fixture.raw_another(&fixture.root, &fixture.root.join("config")).await;
    let bound = handshake(&mut peer, 2, "00000000-0000-4000-8000-000000000002").await;
    assert_eq!(bound["previous"]["cleanup"], "idle");
    assert_eq!(bound["previous"]["process_history"], true);
    std::fs::remove_file(fixture.root.join("pid")).unwrap();
    peer.send(Message::Text(json!({"t":"ping","id":7}).to_string()))
        .await
        .unwrap();
    receive(&mut peer, "pong").await;
    assert!(!fixture.root.join("pid").exists(), "the old operation was not replayed");
    peer.send(Message::Text(
        json!({"t":"req","id":7,"op":"proc.run","stdin":false,"args":{"argv":fixture.process_args()}}).to_string(),
    ))
    .await
    .unwrap();
    assert_eq!(receive(&mut peer, "res").await["ok"], true);
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "new owner done".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), next.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn duplicate_request_identity_cannot_replay_a_completed_process() {
    let mut fixture = Fixture::start("echo").await;
    fixture.process(None, true).await;
    fixture.body(7, b"", true).await;
    assert_eq!(fixture.receive("res").await["ok"], true);
    std::fs::remove_file(fixture.root.join("pid")).unwrap();
    fixture.process(None, false).await;
    assert!(timeout(Duration::from_secs(5), fixture.cli.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    assert!(!fixture.root.join("pid").exists());
}

#[test]
fn descendant_fixture() {
    if std::env::var("APEIRON_TEST_CHILD_MODE").as_deref() != Ok("descendant") {
        return;
    }
    let root = PathBuf::from(std::env::var("APEIRON_TEST_CHILD_ROOT").unwrap());
    std::fs::write(root.join("descendant-pid"), std::process::id().to_string()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && !root.join("descendant-release").exists() {
        std::fs::write(root.join("descendant-tick"), format!("{:?}", Instant::now())).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[tokio::test]
async fn socket_retirement_preserves_history_without_claiming_live_descendants_stopped() {
    let mut fixture = Fixture::start("detached").await;
    fixture.process(None, false).await;
    assert_eq!(fixture.receive("res").await["ok"], true);
    let descendant = std::fs::read_to_string(fixture.root.join("descendant-pid")).unwrap();
    fixture.close(4002).await;
    fixture.exit().await;
    let before = std::fs::read(fixture.root.join("descendant-tick")).unwrap();
    let (mut other, mut peer) = fixture.raw_another(&fixture.root, &fixture.root.join("config")).await;
    let bound = handshake(&mut peer, 2, "00000000-0000-4000-8000-000000000002").await;
    assert_eq!(bound["previous"]["cleanup"], "idle");
    assert_eq!(bound["previous"]["process_history"], true);
    sleep(Duration::from_millis(50)).await;
    assert_ne!(std::fs::read(fixture.root.join("descendant-tick")).unwrap(), before);
    assert!(Command::new("kill")
        .args(["-0", descendant.trim()])
        .status()
        .await
        .unwrap()
        .success());
    std::fs::write(fixture.root.join("descendant-release"), b"release").unwrap();
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "done".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), other.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn a_legacy_journal_without_process_history_is_unknown_not_pristine() {
    let mut fixture = Fixture::start("wait").await;
    fixture.close(4002).await;
    fixture.exit().await;
    let path = fixture
        .root
        .with_extension("home")
        .join(".local/state/apeiron/receivers/1/state.json");
    let mut legacy: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for field in ["receiver_id", "local_scope", "process_history"] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let (other, mut peer) = fixture.raw_another(&fixture.root, &fixture.root.join("config")).await;
    let ready = announce(&mut peer).await;
    assert_eq!(ready["receiver"]["process_history"], true);
    peer.send(Message::Text(
        json!({"t":"bind","scope":scope(&ready,1,OWNER_BOOT),"nonce":ready["nonce"]}).to_string(),
    ))
    .await
    .unwrap();
    let output = timeout(Duration::from_secs(5), other.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("legacy receiver descendants are UNKNOWN"));
}

#[tokio::test]
async fn an_empty_registry_slot_cannot_erase_another_slots_process_history() {
    let mut fixture = Fixture::start("wait").await;
    let workspace = fixture.root.with_extension("history-workspace");
    let config = fixture.root.with_extension("history-config");
    let (mut independent, mut peer) = fixture.raw_another(&workspace, &config).await;
    let bound = handshake(&mut peer, 1, OWNER_BOOT).await;
    peer.send(Message::Text(
        json!({"t":"req","id":71,"op":"proc.run","stdin":false,"args":{"argv":["/usr/bin/true"]}}).to_string(),
    ))
    .await
    .unwrap();
    assert_eq!(receive(&mut peer, "res").await["ok"], true);
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "history recorded".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), independent.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    fixture.close(4002).await;
    fixture.exit().await;
    let (mut reopened, mut peer) = fixture.raw_another(&workspace, &config).await;
    let ready = announce(&mut peer).await;
    assert_eq!(ready["receiver"]["process_history"], true);
    assert_eq!(ready["receiver"]["receiver_id"], bound["scope"]["receiver_id"]);
    peer.close(Some(CloseFrame {
        code: 4002.into(),
        reason: "test complete".into(),
    }))
    .await
    .unwrap();
    assert!(timeout(Duration::from_secs(5), reopened.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    std::fs::remove_dir_all(workspace).unwrap();
    std::fs::remove_dir_all(config).unwrap();
}
