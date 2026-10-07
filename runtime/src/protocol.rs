//! Wire protocol, mirroring `backend/src/local/protocol.ts`.
//!
//! The TypeScript file is the specification; this is the second
//! implementation of it, so every rename here is a breaking change there.
//! Keep the two in step and bump `PROTOCOL_VERSION` on both sides when the
//! shape changes — the gateway refuses a mismatch with a close reason that
//! names both numbers, which is how a user learns to upgrade.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 2;
pub const CONNECT_PATH: &str = "/local/connect";

/// Chunk size for outbound bodies; matches BODY_CHUNK_BYTES in the TS side.
pub const BODY_CHUNK_BYTES: usize = 256 * 1024;

// ── text frames ─────────────────────────────────────────────────────────────

/// Control plane → machine.
#[derive(Debug, Deserialize)]
#[serde(tag = "t")]
pub enum Inbound {
    #[serde(rename = "welcome")]
    Welcome {
        protocol: u32,
        user_id: String,
        tenant_id: String,
    },
    #[serde(rename = "bind")]
    Bind { scope: LocalScope, nonce: String },
    #[serde(rename = "permit")]
    Permit { nonce: String },
    #[serde(rename = "acp.open")]
    AcpOpen {
        ch: String,
        #[serde(default)]
        env: std::collections::HashMap<String, String>,
    },
    #[serde(rename = "acp.send")]
    AcpSend { ch: String, msg: Value },
    #[serde(rename = "acp.close")]
    AcpClose { ch: String },
    #[serde(rename = "req")]
    Request {
        id: u32,
        op: String,
        args: Value,
        #[serde(default)]
        stdin: bool,
    },
    #[serde(rename = "ping")]
    Ping { id: u32 },
    /// A newer control plane may send message types this binary predates;
    /// ignoring them keeps additive changes from needing a version bump.
    #[serde(other)]
    Unknown,
}

/// Machine → control plane.
#[derive(Debug, Serialize)]
#[serde(tag = "t")]
pub enum Outbound {
    #[serde(rename = "ready")]
    Ready {
        protocol: u32,
        info: MachineInfo,
        #[serde(flatten)]
        authority: Option<LocalAnnouncement>,
    },
    #[serde(rename = "bound")]
    Bound { scope: LocalScope, previous: LocalHello },
    #[serde(rename = "permit.challenge")]
    PermitChallenge { nonce: String, window_ms: u64 },
    #[serde(rename = "permit.accepted")]
    PermitAccepted { nonce: String },
    /// The agent process exists. Its pid and the channel id together are the
    /// execution subject the control plane records for the run: a pid alone
    /// is reusable, the channel is unique to this spawn.
    #[serde(rename = "acp.started")]
    AcpStarted { ch: String, pid: u32 },
    #[serde(rename = "acp.msg")]
    AcpMsg { ch: String, msg: Value },
    #[serde(rename = "acp.exit")]
    AcpExit { ch: String, code: Option<i32> },
    #[serde(rename = "acp.stderr")]
    AcpStderr { ch: String, text: String },
    #[serde(rename = "res")]
    Response {
        id: u32,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorPayload>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        body: bool,
    },
    #[serde(rename = "pong")]
    Pong { id: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalScope {
    pub binding_id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub runtime: String,
    pub owner_boot_id: String,
    pub epoch: String,
    pub receiver_id: String,
    pub link_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalHello {
    pub receiver_id: String,
    pub prior_scope: Option<LocalScope>,
    pub cleanup: &'static str,
    pub process_history: bool,
}

#[derive(Debug, Serialize)]
pub struct LocalAnnouncement {
    pub receiver: LocalHello,
    pub nonce: String,
    pub window_ms: u64,
}

impl Outbound {
    pub fn ok(id: u32, value: Value, body: bool) -> Self {
        Outbound::Response {
            id,
            ok: true,
            value: Some(value),
            error: None,
            body,
        }
    }

    pub fn err(id: u32, error: ErrorPayload) -> Self {
        Outbound::Response {
            id,
            ok: false,
            value: None,
            error: Some(error),
            body: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MachineInfo {
    pub version: String,
    pub platform: String,
    pub user_root: String,
    pub config_home: String,
    pub agent_cmd: String,
}

/// A workspace error the control plane re-raises as its own `FileError`, so
/// the files API keeps answering 404 / 403 / 409 exactly as it does for a
/// local or pod-backed session.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorPayload {
    /// "file" carries a FileErrorKind in `kind`; "error" is anything else.
    #[serde(rename = "type")]
    pub kind_tag: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    pub message: String,
}

impl ErrorPayload {
    pub fn file(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind_tag: "file",
            kind: Some(kind),
            message: message.into(),
        }
    }

    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            kind_tag: "error",
            kind: None,
            message: message.into(),
        }
    }
}

// ── operation arguments ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct StatArgs {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct ReaddirArgs {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct ReadArgs {
    pub path: String,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub length: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct WriteArgs {
    pub path: String,
    #[serde(default)]
    pub exclusive: bool,
}

#[derive(Debug, Deserialize)]
pub struct MkdirpArgs {
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct RunArgs {
    pub argv: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct DirEntry {
    pub name: String,
    /// After following symlinks: "file", "dir" or "other".
    pub kind: &'static str,
    pub size: u64,
    pub mtime_ms: u64,
}

// ── binary frames ───────────────────────────────────────────────────────────

pub const BODY_CHUNK: u8 = 0;
pub const BODY_END: u8 = 1;
const HEADER: usize = 5;

pub fn encode_body(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(HEADER + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&id.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

pub struct BodyFrame<'a> {
    pub kind: u8,
    pub id: u32,
    pub payload: &'a [u8],
}

pub fn decode_body(frame: &[u8]) -> Option<BodyFrame<'_>> {
    if frame.len() < HEADER {
        return None;
    }
    let kind = frame[0];
    if kind != BODY_CHUNK && kind != BODY_END {
        return None;
    }
    let id = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]);
    Some(BodyFrame {
        kind,
        id,
        payload: &frame[HEADER..],
    })
}
