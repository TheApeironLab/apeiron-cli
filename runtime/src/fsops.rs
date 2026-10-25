//! The filesystem primitives, and the root guard around them.
//!
//! Primitives only: resolve, list, read bytes, write bytes, make
//! directories. What counts as inside a session, what MIME a file is, how a
//! colliding upload name is suffixed — none of that is here. It is decided
//! in the control plane, against the resolved paths `stat` reports, exactly
//! as it is for a pod (see backend/src/integrations/workspace/local/store.ts).
//!
//! The guard below is not that decision. It is the promise that a control
//! plane cannot use these primitives to wander the machine: whatever it
//! names must already be under the workspace root or scode's config home.

use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::receiver::Receiver;
use serde_json::{json, Value};
use tokio::fs;

use crate::protocol::{DirEntry, ErrorPayload};

/// An operation failure that keeps its workspace error kind across the wire.
pub struct OpError(pub ErrorPayload);

pub type OpResult<T> = Result<T, OpError>;

impl OpError {
    fn not_found() -> Self {
        OpError(ErrorPayload::file("not-found", "文件不存在"))
    }
    fn forbidden(message: impl Into<String>) -> Self {
        OpError(ErrorPayload::file("forbidden", message))
    }
    fn conflict(message: impl Into<String>) -> Self {
        OpError(ErrorPayload::file("conflict", message))
    }
    fn internal(error: std::io::Error) -> Self {
        OpError(ErrorPayload::file("internal", error.to_string()))
    }
}

fn io(error: std::io::Error) -> OpError {
    match error.kind() {
        std::io::ErrorKind::NotFound => OpError::not_found(),
        std::io::ErrorKind::AlreadyExists => OpError::conflict("文件已存在"),
        std::io::ErrorKind::PermissionDenied => OpError::forbidden("权限不足"),
        _ => OpError::internal(error),
    }
}

/// The roots this machine will expose, in every spelling the filesystem uses.
///
/// Both the configured path and its canonical form are kept: on macOS the
/// workspace root is typically /var/folders/… while every resolved path
/// under it comes back as /private/var/folders/…, and the control plane
/// addresses files by those resolved paths.
#[derive(Clone, Debug)]
pub struct Roots(Vec<PathBuf>);

impl Roots {
    pub async fn prepare(workspace: &Path, config_home: &Path) -> std::io::Result<(PathBuf, Roots)> {
        fs::create_dir_all(workspace).await?;
        let workspace_real = fs::canonicalize(workspace).await?;
        let mut roots = vec![
            workspace.to_path_buf(),
            workspace_real.clone(),
            config_home.to_path_buf(),
        ];
        // scode's config home need not exist yet; the projector creates it.
        if let Ok(real) = fs::canonicalize(config_home).await {
            roots.push(real);
        }
        roots.sort();
        roots.dedup();
        Ok((workspace_real, Roots(roots)))
    }

    /// Reject a path outside the roots, lexically, before anything is opened.
    pub fn allow(&self, path: &str) -> OpResult<PathBuf> {
        let candidate = Path::new(path);
        if !candidate.is_absolute() {
            return Err(OpError::forbidden("path must be absolute"));
        }
        let clean = lexically_normal(candidate);
        if self.0.iter().any(|root| clean == *root || clean.starts_with(root)) {
            Ok(clean)
        } else {
            Err(OpError::forbidden(format!(
                "path outside this machine's apeiron roots: {}",
                clean.display()
            )))
        }
    }
}

/// Resolve `.` and `..` without touching the filesystem — the guard must not
/// be defeated by a path that only escapes after normalization.
fn lexically_normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn kind_of(meta: &std::fs::Metadata) -> &'static str {
    if meta.is_dir() {
        "dir"
    } else if meta.is_file() {
        "file"
    } else {
        "other"
    }
}

fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

// Check inside the blocking closure: checking only before spawn_blocking lets
// queued filesystem work start after its cloud permit has expired.
async fn blocking<T: Send + 'static>(
    receiver: Arc<Receiver>,
    operation: impl FnOnce(&Receiver) -> OpResult<T> + Send + 'static,
) -> OpResult<T> {
    tokio::task::spawn_blocking(move || {
        receiver.check().map_err(io)?;
        operation(&receiver)
    })
    .await
    .map_err(|e| OpError::internal(std::io::Error::other(e)))?
}

pub async fn stat(roots: &Roots, path: &str, receiver: Arc<Receiver>) -> OpResult<Value> {
    let target = roots.allow(path)?;
    blocking(receiver, move |receiver| {
        let real = std::fs::canonicalize(&target).map_err(io)?;
        receiver.check().map_err(io)?;
        let meta = std::fs::metadata(&real).map_err(io)?;
        Ok(json!({ "real": real.to_string_lossy(), "kind": kind_of(&meta), "size": meta.len(), "mtime_ms": mtime_ms(&meta) }))
    }).await
}

pub async fn readdir(roots: &Roots, path: &str, receiver: Arc<Receiver>) -> OpResult<Value> {
    let dir = roots.allow(path)?;
    blocking(receiver, move |receiver| {
        let mut entries: Vec<DirEntry> = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(io)? {
            receiver.check().map_err(io)?;
            let entry = entry.map_err(io)?;
            let Ok(meta) = entry.metadata() else { continue };
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                kind: kind_of(&meta),
                size: meta.len(),
                mtime_ms: mtime_ms(&meta),
            });
        }
        Ok(json!({ "entries": entries }))
    })
    .await
}

pub type ReadFile = Arc<Mutex<std::fs::File>>;

pub async fn open_read(
    roots: &Roots,
    path: &str,
    offset: u64,
    length: Option<u64>,
    receiver: Arc<Receiver>,
) -> OpResult<(ReadFile, u64)> {
    let target = roots.allow(path)?;
    blocking(receiver, move |receiver| {
        let mut file = std::fs::File::open(&target).map_err(io)?;
        receiver.check().map_err(io)?;
        let available = file.metadata().map_err(io)?.len().saturating_sub(offset);
        let total = length.map_or(available, |want| want.min(available));
        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset)).map_err(io)?;
        }
        Ok((Arc::new(Mutex::new(file)), total))
    })
    .await
}

pub async fn read_chunk(file: &ReadFile, buffer: &mut [u8], receiver: Arc<Receiver>) -> OpResult<usize> {
    let file = file.clone();
    let length = buffer.len();
    let bytes = blocking(receiver, move |receiver| {
        let mut file = file
            .lock()
            .map_err(|_| OpError::internal(std::io::Error::other("read lock poisoned")))?;
        receiver.check().map_err(io)?;
        let mut bytes = vec![0; length];
        let count = file.read(&mut bytes).map_err(io)?;
        bytes.truncate(count);
        Ok(bytes)
    })
    .await?;
    buffer[..bytes.len()].copy_from_slice(&bytes);
    Ok(bytes.len())
}

pub async fn write(
    roots: &Roots,
    path: &str,
    exclusive: bool,
    bytes: &[u8],
    receiver: Arc<Receiver>,
) -> OpResult<Value> {
    let target = roots.allow(path)?;
    let bytes = bytes.to_vec();
    blocking(receiver, move |receiver| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        if exclusive {
            options.create_new(true);
        } else {
            options.create(true).truncate(true);
        }
        receiver.check().map_err(io)?;
        let mut file = options.open(&target).map_err(io)?;
        let mut written = 0;
        while written < bytes.len() {
            receiver.check().map_err(io)?;
            let count = file.write(&bytes[written..]).map_err(io)?;
            if count == 0 {
                return Err(io(std::io::ErrorKind::WriteZero.into()));
            }
            written += count;
        }
        file.flush().map_err(io)?;
        Ok(json!({ "size": bytes.len() }))
    })
    .await
}

pub async fn mkdirp(roots: &Roots, paths: &[String], receiver: Arc<Receiver>) -> OpResult<Value> {
    let targets = paths.iter().map(|p| roots.allow(p)).collect::<OpResult<Vec<_>>>()?;
    blocking(receiver, move |receiver| {
        for target in targets {
            receiver.check().map_err(io)?;
            std::fs::create_dir_all(&target).map_err(io)?;
        }
        Ok(json!({}))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_happens_before_the_guard() {
        let roots = Roots(vec![PathBuf::from("/tmp/ws")]);
        assert!(roots.allow("/tmp/ws/a/b").is_ok());
        assert!(roots.allow("/tmp/ws/a/../b").is_ok());
        // Escapes that only appear after resolving `..` are still refused.
        assert!(roots.allow("/tmp/ws/../etc/passwd").is_err());
        assert!(roots.allow("/etc/passwd").is_err());
        // A sibling directory that merely shares the prefix is not inside.
        assert!(roots.allow("/tmp/wsx/file").is_err());
        assert!(roots.allow("relative/path").is_err());
    }
}
