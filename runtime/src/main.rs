//! `apeiron` — run the apeiron agent on your own machine.
//!
//! A laptop has no address the control plane can reach, so this program
//! dials out: one WebSocket to apeiron, authenticated with an ordinary
//! Personal Access Token, and the user that token belongs to gets their
//! agent here — with their files, their tools, their model credentials.
//!
//! It stays deliberately small. It spawns `scode acp` and shuttles whole
//! JSON-RPC messages, answers filesystem primitives, and runs the Plugin
//! projector's shell commands. Every rule about what those primitives mean
//! lives in the control plane (see backend/src/local/protocol.ts), so this
//! binary does not need shipping again when a rule changes.

mod agent;
mod cloud;
mod fsops;
mod local;
mod protocol;
mod receiver;
mod service;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message;

use crate::agent::AgentChild;
use crate::fsops::{OpError, Roots};
use crate::protocol::{
    decode_body, encode_body, ErrorPayload, Inbound, MachineInfo, MkdirpArgs, Outbound, ReadArgs, ReaddirArgs, RunArgs,
    StatArgs, WriteArgs, BODY_CHUNK, BODY_CHUNK_BYTES, BODY_END, CONNECT_PATH,
};

#[derive(Parser, Debug, Clone)]
#[command(name = "apeiron", version = env!("APEIRON_VERSION"), about = "Run the apeiron agent on this machine")]
struct Args {
    /// What to do. Omitting it prints help: attaching a machine is
    /// `apeiron connect`, and the app hands you that command with a token.
    #[command(subcommand)]
    command: Option<Cli>,

    /// Control plane base URL (http/https or ws/wss).
    #[arg(long, global = true, env = "APEIRON_SERVER")]
    server: Option<String>,

    /// Personal access token; its owner is the user this machine serves.
    #[arg(long, global = true, env = "APEIRON_TOKEN", hide_env_values = true)]
    token: Option<String>,

    /// Read the token from a file instead — how the background service gets
    /// it, so the credential is not in a unit file or a process listing.
    #[arg(long = "token-file", global = true, env = "APEIRON_TOKEN_FILE")]
    token_file: Option<String>,

    /// Where session directories live on this machine.
    #[arg(long, global = true, env = "APEIRON_ROOT", default_value_t = default_root())]
    root: String,

    /// Agent to spawn for the conversation. Defaults to the scode the
    /// installer put under ~/.apeiron, falling back to whatever `scode` is on
    /// PATH when that one is not there.
    #[arg(long, global = true, env = "APEIRON_AGENT", default_value_t = default_agent())]
    agent: String,

    /// scode's config home — the directory the Plugin projector writes to and
    /// that the agent is told to read, via SUDO_CODE_CONFIG_HOME.
    #[arg(long = "config-home", global = true, env = "APEIRON_CONFIG_HOME", default_value_t = default_config_home())]
    config_home: String,

    /// Log every ACP message and operation crossing this machine.
    #[arg(long, global = true, env = "APEIRON_VERBOSE")]
    verbose: bool,
}

#[derive(Subcommand, Debug, Clone)]
enum Cli {
    /// Attach this machine: start in the background (launchd on macOS,
    /// systemd --user on Linux) and reconnect at every login.
    Connect,
    /// Is this machine attached, and where are its logs?
    Status,
    /// Detach this machine: stop it, and forget the unit and the token.
    /// The token itself is deleted from the app (设置 → 本地电脑 → 断开).
    Disconnect,
    /// Run in this terminal, installing nothing. This is what the background
    /// service executes — and what to use where there is no launchd or
    /// systemd --user, or under a supervisor of your own.
    Serve,
    /// Internal container entrypoint. The control plane binds this PID 1 over pods/attach.
    CloudReceiver {
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
}

/// The token, from `--token` or the file the service keeps it in.
fn resolve_token(args: &Args) -> Result<String> {
    if let Some(token) = &args.token {
        return Ok(token.clone());
    }
    let path = args
        .token_file
        .as_ref()
        .ok_or_else(|| anyhow!("pass --token or --token-file"))?;
    let token = std::fs::read_to_string(path).with_context(|| format!("cannot read {path}"))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(anyhow!("{path} is empty"));
    }
    Ok(token)
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn default_root() -> String {
    home()
        .join(".apeiron")
        .join("workspaces")
        .to_string_lossy()
        .into_owned()
}

/// The scode the installer manages, under ~/.apeiron beside the workspaces.
fn managed_scode() -> PathBuf {
    home().join(".apeiron").join("bin").join("scode")
}

/// Prefer the copy apeiron installed; fall back to PATH.
///
/// The fallback is for the machine that attached before the installer managed
/// scode at all: its service unit was written with `scode acp` baked in, and
/// it keeps working until the user reruns the install command. A bare `scode`
/// is also the only thing that can work if someone builds this CLI and runs
/// `serve` by hand.
///
/// `--agent` is split on whitespace, so a HOME containing a space would break
/// the managed path. That is rare enough on macOS and Linux to prefer the
/// simpler argv over quoting rules nobody can remember; when it happens, the
/// path simply does not exist and PATH answers instead.
fn default_agent() -> String {
    let managed = managed_scode();
    if managed.exists() && !managed.to_string_lossy().contains(char::is_whitespace) {
        format!("{} acp", managed.to_string_lossy())
    } else {
        "scode acp".to_string()
    }
}

fn default_config_home() -> String {
    home().join(".apeiron").join("sudocode").to_string_lossy().into_owned()
}

macro_rules! log {
    // stdout belongs to the cloud protocol; local diagnostics use the same sink.
    ($($arg:tt)*) => { eprintln!("[apeiron] {}", format!($($arg)*)) };
}

mod app;

fn main() -> Result<()> {
    if let Some(code) = app::run() {
        std::process::exit(code);
    }
    let args = Args::parse();
    match &args.command {
        Some(Cli::Status) => service::status(),
        Some(Cli::Disconnect) => service::uninstall(),
        Some(Cli::Connect) => service::install(&service::ServiceConfig {
            server: args.server.clone().ok_or_else(|| anyhow!("--server is required"))?,
            token: resolve_token(&args)?,
            root: args.root.clone(),
            agent: args.agent.clone(),
            config_home: args.config_home.clone(),
        }),
        Some(Cli::Serve) => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(serve_local(args)),
        Some(Cli::CloudReceiver { argv }) => cloud::run(&args, argv),
        None => {
            // No subcommand: say what to do rather than starting something.
            Args::command().print_help()?;
            println!();
            Ok(())
        }
    }
}

async fn serve_local(args: Args) -> Result<()> {
    let token = resolve_token(&args)?;
    let server = args.server.clone().ok_or_else(|| anyhow!("--server is required"))?;
    if std::env::var_os("HOME").is_none() && std::env::var_os("USERPROFILE").is_none() {
        return Err(anyhow!("HOME or USERPROFILE is required for durable receiver state"));
    }
    let workspace = PathBuf::from(&args.root);
    let config_home = PathBuf::from(&args.config_home);
    receiver::Receiver::check_roots(&home(), &workspace, &config_home)?;
    // Both roots need a physical identity before admission. Deferring config
    // creation until projection would let it be created then renamed without
    // the receiver's resource claim ever learning that directory's identity.
    std::fs::create_dir_all(&config_home)
        .with_context(|| format!("cannot prepare config home {}", config_home.display()))?;
    let (workspace, roots) = Roots::prepare(&workspace, &config_home)
        .await
        .with_context(|| format!("cannot use workspace root {}", workspace.display()))?;
    let config_home = std::fs::canonicalize(&config_home)?;
    let argv: Vec<String> = args.agent.split_whitespace().map(str::to_owned).collect();
    if argv.is_empty() {
        return Err(anyhow!("--agent is empty"));
    }
    let info = MachineInfo {
        version: env!("APEIRON_VERSION").to_string(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        user_root: workspace.to_string_lossy().into_owned(),
        config_home: config_home.to_string_lossy().into_owned(),
        agent_cmd: argv.join(" "),
    };
    let receiver = Arc::new(receiver::Receiver::open(&home(), &workspace, &config_home)?);
    let session = Arc::new(Session {
        args: args.clone(),
        server,
        token,
        argv,
        workspace,
        config_home,
        roots,
        info,
        receiver,
    });

    // A laptop's link is expected to come and go: a dropped socket is
    // routine, and only a completed handshake resets the backoff. The one
    // thing that is not routine is the credential being gone — the user
    // pressed 断开 in the app, which deletes this token. Retrying then would
    // leave a dead process on their machine forever, so we stop.
    let mut backoff = Duration::from_secs(1);
    loop {
        match session.clone().connect_once().await {
            Ok(reason) => {
                if reason.starts_with(&REVOKED_CLOSE_CODE.to_string()) {
                    log!("this machine was disconnected from the app ({reason}); exiting");
                    return stop_for_good(&args);
                }
                if reason.starts_with(&REPLACED_CLOSE_CODE.to_string()) {
                    // Another connection took this user's single slot. Retrying
                    // would take it back, and the two would trade it forever —
                    // with runs landing on whichever machine happened to hold
                    // it. The newest one wins; this one stops.
                    log!("another machine is now attached for this user ({reason}); exiting");
                    return Ok(());
                }
                log!("disconnected ({reason}); retrying in {}s", backoff.as_secs());
            }
            Err(error) => {
                if error.is::<CleanupFailed>() {
                    log!("{error}; stopping without reconnecting");
                    // Deliberately exit successfully: launchd/systemd restart
                    // failures, which would bypass this unknown-child stop.
                    // Durable pending evidence also blocks manual restarts;
                    // do not remove the service or its token.
                    return Ok(());
                }
                if is_credential_rejected(&error) {
                    log!("the server rejected this token ({error}); exiting");
                    return stop_for_good(&args);
                }
                log!("connection failed ({error}); retrying in {}s", backoff.as_secs());
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// The close code the control plane sends when the token behind a live
/// connection is deleted. Mirrors `backend/src/local/gateway.ts`.
const REVOKED_CLOSE_CODE: u16 = 4006;

/// Sent to the older connection when the same user attaches another machine
/// (`LocalRegistry::attach`). One machine per user, newest wins.
const REPLACED_CLOSE_CODE: u16 = 4002;

/// The credential is gone for good, so take the background service with it —
/// otherwise every login starts a process that can only fail.
///
/// Only when this process *is* that service, though. A `serve` someone
/// started by hand with its own `--token` has no business deleting the unit
/// and token of the service running beside it: same machine, different
/// credential, and revoking one says nothing about the other.
fn stop_for_good(args: &Args) -> Result<()> {
    let ours = args
        .token_file
        .as_ref()
        .map(PathBuf::from)
        .is_some_and(|path| service::service_token_path().is_ok_and(|service| path == service));
    if !ours {
        return Ok(());
    }
    if service::forget()? {
        log!("removed the background service; run the command from the app again to reconnect");
    }
    Ok(())
}

/// 401/403 at the upgrade: the token is gone or was never valid. Anything
/// else (a 502 from a proxy, a restart) is worth retrying.
fn is_credential_rejected(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<tokio_tungstenite::tungstenite::Error>(),
        Some(tokio_tungstenite::tungstenite::Error::Http(response))
            if response.status() == 401 || response.status() == 403
    )
}

#[derive(Clone)]
struct Session {
    args: Args,
    /// Resolved once: `--server` and the token (which may have come from a
    /// file) are what every reconnect uses.
    server: String,
    token: String,
    argv: Vec<String>,
    workspace: PathBuf,
    /// Passed to every agent child as SUDO_CODE_CONFIG_HOME, so the directory
    /// the projector writes is the one the agent reads.
    config_home: PathBuf,
    roots: Roots,
    info: MachineInfo,
    receiver: Arc<receiver::Receiver>,
}

/// The socket owns every request, including one still collecting its body.
/// Reconnecting before these finish would give old work a new connection's
/// lifetime. Cancelling a task alone is not evidence its child was reaped.
struct Live {
    agents: HashMap<String, AgentChild>,
    inbound: HashMap<u32, mpsc::UnboundedSender<Option<Vec<u8>>>>,
    requests: JoinSet<Result<u32, CleanupFailed>>,
    cancel: watch::Sender<bool>,
    requests_seen: HashSet<u32>,
    channels_seen: HashSet<String>,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            agents: HashMap::new(),
            inbound: HashMap::new(),
            requests: JoinSet::new(),
            cancel: watch::channel(false).0,
            requests_seen: HashSet::new(),
            channels_seen: HashSet::new(),
        }
    }
}

impl Live {
    async fn close(mut self) -> Result<(), CleanupFailed> {
        self.cancel.send_replace(true);
        // EOF is cancellation, not BODY_END; a partial upload must not commit.
        self.inbound.clear();
        let mut agents = JoinSet::new();
        for (_, child) in self.agents.drain() {
            agents.spawn(child.close());
        }
        let mut failure = None;
        while let Some(result) = self.requests.join_next().await {
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => failure = Some(error),
                Err(error) => failure = Some(CleanupFailed(error.to_string())),
            }
        }
        while let Some(result) = agents.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failure = Some(CleanupFailed(error.to_string())),
                Err(error) => failure = Some(CleanupFailed(error.to_string())),
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

#[derive(Debug)]
struct CleanupFailed(String);

impl std::fmt::Display for CleanupFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "child cleanup was not confirmed: {}", self.0)
    }
}

impl std::error::Error for CleanupFailed {}

enum RequestError {
    Operation(OpError),
    Cleanup(CleanupFailed),
}

impl From<OpError> for RequestError {
    fn from(error: OpError) -> Self {
        Self::Operation(error)
    }
}

impl From<std::io::Error> for RequestError {
    fn from(error: std::io::Error) -> Self {
        Self::Cleanup(CleanupFailed(format!("receiver evidence: {error}")))
    }
}

impl Session {
    fn url(&self) -> String {
        let base = self.server.trim_end_matches('/');
        let base = base
            .strip_prefix("http")
            .map_or(base.to_string(), |rest| format!("ws{rest}"));
        format!("{base}{CONNECT_PATH}")
    }

    async fn connect_once(self: Arc<Self>) -> Result<String> {
        let mut authority = local::Authority::new()?;
        let self_ = Arc::new(Self {
            receiver: Arc::new(self.receiver.with_permit(authority.permit.clone())),
            ..(*self).clone()
        });
        let url = self.url();
        log!("connecting to {url}");
        let mut request = url.into_client_request()?;
        request
            .headers_mut()
            .insert(AUTHORIZATION, format!("Bearer {}", self.token).parse()?);
        let (socket, _) = tokio_tungstenite::connect_async(request).await?;
        let (mut sink, mut stream) = socket.split();

        // One writer task owns the sink: operations, agent output and body
        // frames all interleave on this socket and must not race each other.
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Outbound>();
        let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let mut writer = tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    text = out_rx.recv() => match text {
                        Some(value) => Message::Text(serde_json::to_string(&value).unwrap_or_default()),
                        None => break,
                    },
                    frame = raw_rx.recv() => match frame {
                        Some(bytes) => Message::Binary(bytes),
                        None => break,
                    },
                };
                if sink.send(message).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        let mut live = Live::default();
        let mut writer_finished = false;
        let mut cleanup_error = None;
        let evidence_failed = self_.receiver.failure();
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        let reason = loop {
            let message = tokio::select! {
                _ = tick.tick() => {
                    match authority.tick() {
                        Ok(Some(value)) => { let _ = out_tx.send(value); }
                        Ok(None) => {},
                        Err(error) => break error.to_string(),
                    }
                    continue;
                }
                _ = cancelled(evidence_failed.clone()) => {
                    cleanup_error = Some(CleanupFailed("receiver evidence persistence failed".into()));
                    break "receiver evidence failed".to_string();
                }
                _ = &mut writer => {
                    writer_finished = true;
                    break "socket writer ended".to_string();
                }
                completed = live.requests.join_next(), if !live.requests.is_empty() => {
                    match completed {
                        Some(Ok(Ok(id))) => { live.inbound.remove(&id); }
                        Some(Ok(Err(error))) => {
                            cleanup_error = Some(error);
                            break "request cleanup failed".to_string();
                        }
                        Some(Err(error)) => {
                            cleanup_error = Some(CleanupFailed(error.to_string()));
                            break "request task failed".to_string();
                        }
                        None => {},
                    }
                    continue;
                }
                message = stream.next() => message,
            };
            let Some(message) = message else {
                break "stream ended".to_string();
            };
            match message {
                Ok(Message::Text(text)) => {
                    let result = async {
                        let message = serde_json::from_str::<Inbound>(&text)?;
                        if let Some(replies) = authority.control(&message, &self_.receiver, &self_.info)? {
                            for reply in replies {
                                let _ = out_tx.send(reply);
                            }
                            return Ok(());
                        }
                        self_.clone().on_text(&text, &mut live, &out_tx, &raw_tx).await
                    }
                    .await;
                    if let Err(error) = result {
                        cleanup_error = Some(CleanupFailed(error.to_string()));
                        break "agent cleanup failed".to_string();
                    }
                }
                Ok(Message::Binary(bytes)) => {
                    if let Err(error) = authority.check() {
                        break error.to_string();
                    }
                    let Some(frame) = decode_body(&bytes) else { continue };
                    if frame.kind == BODY_CHUNK {
                        if let Some(sender) = live.inbound.get(&frame.id) {
                            let _ = sender.send(Some(frame.payload.to_vec()));
                        }
                    } else if let Some(sender) = live.inbound.remove(&frame.id) {
                        let _ = sender.send(None);
                    }
                }
                Ok(Message::Close(frame)) => {
                    break frame
                        .map(|f| format!("{} {}", f.code, f.reason))
                        .unwrap_or_else(|| "closed".to_string())
                }
                Ok(_) => {}
                Err(error) => break error.to_string(),
            }
        };
        // Wait for process owners to kill AND reap. Do not abort these futures:
        // kill_on_drop is only a fallback and its reaping is best effort.
        authority.permit.retire();
        let cleanup = live.close().await;
        drop(out_tx);
        drop(raw_tx);
        if !writer_finished {
            // The peer is gone; flushing a blocked socket must not hold cleanup.
            writer.abort();
            let _ = writer.await;
        }
        if let Some(error) = cleanup_error {
            return Err(error.into());
        }
        cleanup?;
        Ok(reason)
    }

    async fn on_text(
        self: Arc<Self>,
        text: &str,
        live: &mut Live,
        out: &mpsc::UnboundedSender<Outbound>,
        raw: &mpsc::UnboundedSender<Vec<u8>>,
    ) -> Result<()> {
        let Ok(message) = serde_json::from_str::<Inbound>(text) else {
            return Ok(());
        };
        match message {
            Inbound::Welcome { .. } | Inbound::Bind { .. } | Inbound::Permit { .. } => {
                return Err(anyhow!("control frame at business boundary"))
            }
            Inbound::AcpOpen { ch, env } => {
                if !live.channels_seen.insert(ch.clone()) {
                    return Err(anyhow!("replayed ACP channel"));
                }
                match agent::spawn(
                    ch.clone(),
                    &self.argv,
                    &self.workspace,
                    &self.config_home,
                    env,
                    out.clone(),
                    self.receiver.clone(),
                ) {
                    Ok(child) => {
                        live.agents.insert(ch.clone(), child);
                        log!("agent started on {ch}: {}", self.info.agent_cmd);
                    }
                    Err(error) => {
                        if *self.receiver.failure().borrow() {
                            return Err(error);
                        }
                        log!("agent failed to start: {error}");
                        // Say why on the wire, not just in this log: the person
                        // waiting is looking at the app, and "scode is not
                        // installed" is the most likely first-run answer.
                        let _ = out.send(Outbound::AcpStderr {
                            ch: ch.clone(),
                            text: format!("cannot start `{}`: {error}", self.info.agent_cmd),
                        });
                        let _ = out.send(Outbound::AcpExit { ch, code: None });
                    }
                }
            }
            Inbound::AcpSend { ch, msg } => {
                if self.args.verbose {
                    log!("→ agent {}", describe(&msg));
                }
                if let Some(child) = live.agents.get(&ch) {
                    child.send(&msg)?;
                }
            }
            Inbound::AcpClose { ch } => {
                if let Some(child) = live.agents.remove(&ch) {
                    child.close().await?;
                }
            }
            Inbound::Ping { id } => {
                let _ = out.send(Outbound::Pong { id });
            }
            Inbound::Request { id, op, args, stdin } => {
                if !live.requests_seen.insert(id) {
                    return Err(anyhow!("replayed operation id"));
                }
                let body = if stdin {
                    let (tx, rx) = mpsc::unbounded_channel();
                    live.inbound.insert(id, tx);
                    Some(rx)
                } else {
                    None
                };
                if self.args.verbose {
                    log!("op {op} (req#{id}{})", if stdin { ", stdin" } else { "" });
                }
                let session = self.clone();
                let out = out.clone();
                let raw = raw.clone();
                let cancel = live.cancel.subscribe();
                live.requests.spawn(async move {
                    match session.serve(id, &op, args, body, (&out, &raw), cancel).await {
                        Ok(()) => {}
                        Err(RequestError::Operation(OpError(payload))) => {
                            let _ = out.send(Outbound::err(id, payload));
                        }
                        Err(RequestError::Cleanup(error)) => return Err(error),
                    }
                    Ok(id)
                });
            }
            Inbound::Unknown => {}
        }
        Ok(())
    }

    async fn serve(
        &self,
        id: u32,
        op: &str,
        args: serde_json::Value,
        body: Option<mpsc::UnboundedReceiver<Option<Vec<u8>>>>,
        output: (&mpsc::UnboundedSender<Outbound>, &mpsc::UnboundedSender<Vec<u8>>),
        cancel: watch::Receiver<bool>,
    ) -> Result<(), RequestError> {
        let (out, raw) = output;
        self.receiver.check()?;
        if *cancel.borrow() {
            return Err(cancelled_error().into());
        }
        match op {
            "fs.stat" => {
                let args: StatArgs = parse(args)?;
                let value = fsops::stat(&self.roots, &args.path, self.receiver.clone()).await?;
                let _ = out.send(Outbound::ok(id, value, false));
            }
            "fs.readdir" => {
                let args: ReaddirArgs = parse(args)?;
                let value = fsops::readdir(&self.roots, &args.path, self.receiver.clone()).await?;
                let _ = out.send(Outbound::ok(id, value, false));
            }
            "fs.read" => {
                let args: ReadArgs = parse(args)?;
                let (file, total) = fsops::open_read(
                    &self.roots,
                    &args.path,
                    args.offset.unwrap_or(0),
                    args.length,
                    self.receiver.clone(),
                )
                .await?;
                // Metadata first, then the bytes: the control plane starts
                // its HTTP response as soon as it knows the size.
                let _ = out.send(Outbound::ok(id, json!({ "size": total }), true));
                let mut buffer = vec![0u8; BODY_CHUNK_BYTES];
                let mut sent = 0u64;
                while sent < total {
                    if *cancel.borrow() {
                        return Err(cancelled_error().into());
                    }
                    let want = ((total - sent) as usize).min(buffer.len());
                    let read = fsops::read_chunk(&file, &mut buffer[..want], self.receiver.clone()).await?;
                    if read == 0 {
                        break;
                    }
                    let _ = raw.send(encode_body(BODY_CHUNK, id, &buffer[..read]));
                    sent += read as u64;
                }
                let _ = raw.send(encode_body(BODY_END, id, &[]));
            }
            "fs.write" => {
                let args: WriteArgs = parse(args)?;
                let bytes = collect(body, cancel).await?;
                let operation = self.receiver.begin("fs.write")?;
                // Once a filesystem write starts, await it through flush. Tokio
                // filesystem work may outlive a dropped future on its IO pool.
                let result = fsops::write(&self.roots, &args.path, args.exclusive, &bytes, self.receiver.clone()).await;
                self.receiver.complete(operation)?;
                let value = result?;
                let _ = out.send(Outbound::ok(id, value, false));
            }
            "fs.mkdirp" => {
                let args: MkdirpArgs = parse(args)?;
                let operation = self.receiver.begin("fs.mkdirp")?;
                let result = fsops::mkdirp(&self.roots, &args.paths, self.receiver.clone()).await;
                self.receiver.complete(operation)?;
                let value = result?;
                let _ = out.send(Outbound::ok(id, value, false));
            }
            "proc.run" => {
                let args: RunArgs = parse(args)?;
                let stdin = collect(body, cancel.clone()).await?;
                let operation = self.receiver.begin("proc.run")?;
                let result = run_process(&args, stdin, cancel, self.receiver.clone()).await;
                // run_process only returns an ordinary error after no child
                // was spawned or the exact direct child was waited. Unknown
                // cleanup deliberately leaves its durable pending entry.
                if !result.as_ref().is_err_and(|error| error.is::<CleanupFailed>()) {
                    self.receiver.complete(operation)?;
                }
                let (code, stdout, stderr) = result.map_err(|error| match error.downcast::<CleanupFailed>() {
                    Ok(error) => RequestError::Cleanup(error),
                    Err(error) => RequestError::Operation(to_op_error(error)),
                })?;
                // stdout goes out as body frames before the response closes
                // the operation: the projector reads tar streams out of it.
                for chunk in stdout.chunks(BODY_CHUNK_BYTES) {
                    let _ = raw.send(encode_body(BODY_CHUNK, id, chunk));
                }
                let _ = raw.send(encode_body(BODY_END, id, &[]));
                let _ = out.send(Outbound::ok(id, json!({ "exit_code": code, "stderr": stderr }), true));
            }
            other => {
                let _ = out.send(Outbound::err(
                    id,
                    ErrorPayload::plain(format!("unknown operation {other}")),
                ));
            }
        }
        Ok(())
    }
}

fn parse<T: serde::de::DeserializeOwned>(args: serde_json::Value) -> Result<T, OpError> {
    serde_json::from_value(args)
        .map_err(|error| OpError(ErrorPayload::file("invalid-input", format!("bad arguments: {error}"))))
}

fn to_op_error(error: anyhow::Error) -> OpError {
    OpError(ErrorPayload::plain(error.to_string()))
}

fn cancelled_error() -> OpError {
    OpError(ErrorPayload::plain("request cancelled before its input completed"))
}

async fn cancelled(mut cancel: watch::Receiver<bool>) {
    while !*cancel.borrow_and_update() {
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

async fn collect(
    body: Option<mpsc::UnboundedReceiver<Option<Vec<u8>>>>,
    cancel: watch::Receiver<bool>,
) -> Result<Vec<u8>, OpError> {
    if *cancel.borrow() {
        return Err(cancelled_error());
    }
    let Some(mut rx) = body else { return Ok(Vec::new()) };
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancelled(cancel.clone()) => return Err(cancelled_error()),
            chunk = rx.recv() => chunk,
        };
        match chunk {
            Some(Some(part)) => bytes.extend_from_slice(&part),
            Some(None) => return Ok(bytes), // Only an explicit BODY_END completes input.
            None => return Err(cancelled_error()),
        }
    }
}

async fn run_process(
    args: &RunArgs,
    input: Vec<u8>,
    cancel: watch::Receiver<bool>,
    receiver: Arc<receiver::Receiver>,
) -> Result<(i32, Vec<u8>, String)> {
    if *cancel.borrow() {
        return Err(anyhow!("request cancelled"));
    }
    let (program, rest) = args.argv.split_first().ok_or_else(|| anyhow!("empty argv"))?;
    receiver.check()?;
    let mut child = Command::new(program)
        .args(rest)
        .stdin(if input.is_empty() {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut output = Vec::new();
    let mut errors = Vec::new();
    // Borrow Child instead of consuming it in wait_with_output: cancellation
    // must leave an owned handle available for an explicit kill followed by wait.
    // Drain both output pipes while writing stdin to avoid pipe-buffer deadlock.
    let result = {
        let work = async {
            let (status, _, _, _) = tokio::try_join!(
                child.wait(),
                async {
                    if let Some(mut sink) = stdin.take() {
                        receiver.write_checked(&mut sink, &input).await?;
                        sink.shutdown().await?;
                    }
                    Ok::<_, std::io::Error>(())
                },
                stdout.read_to_end(&mut output),
                stderr.read_to_end(&mut errors),
            )?;
            Ok::<_, std::io::Error>(status)
        };
        tokio::select! {
            biased;
            _ = cancelled(cancel) => None,
            _ = async {
                match args.timeout_ms {
                    Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                    None => std::future::pending().await,
                }
            } => Some(Err(None)),
            result = work => Some(result.map_err(Some)),
        }
    };
    if let Some(Ok(status)) = result {
        return Ok((
            status.code().unwrap_or(-1),
            output,
            String::from_utf8_lossy(&errors).into_owned(),
        ));
    }
    // A timeout is a caller result, not proof of termination. Even if sending
    // the signal races natural exit, always wait on this exact child handle.
    let kill_error = child.start_kill().err();
    child
        .wait()
        .await
        .map_err(|error| CleanupFailed(format!("cannot reap request child: {error}")))?;
    if let Some(error) = kill_error {
        return Err(error.into());
    }
    match result {
        None => Err(anyhow!("request cancelled")),
        Some(Err(None)) => Ok((
            124,
            Vec::new(),
            format!("timed out after {}ms", args.timeout_ms.unwrap()),
        )),
        Some(Err(Some(error))) => Err(error.into()),
        Some(Ok(_)) => unreachable!(),
    }
}

fn describe(msg: &serde_json::Value) -> String {
    if let Some(method) = msg.get("method").and_then(|value| value.as_str()) {
        return method.to_string();
    }
    match msg.get("id") {
        Some(id) if msg.get("error").is_some() => format!("error#{id}"),
        Some(id) => format!("result#{id}"),
        None => "?".to_string(),
    }
}
