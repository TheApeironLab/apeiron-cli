//! Durable cleanup and local authority evidence. Request/channel identities
//! belong to one immutable socket binding; reconnect never replays old work.
//!
//! The process locks exclude another live receiver. After a crash, pending
//! work remains UNKNOWN indefinitely: a dead parent, elapsed time or a reused
//! PID cannot prove its child stopped. Recovery of such work belongs to the
//! future receiver takeover protocol, not a force/reset flag here.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use crate::protocol::{LocalHello, LocalScope};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Serialize, Deserialize)]
struct Resource {
    canonical: PathBuf,
    ancestors: Vec<Ancestor>,
}

#[derive(Serialize, Deserialize)]
struct Ancestor {
    identity: (u64, u64),
    tail: PathBuf,
}

impl Resource {
    fn resolve(path: &Path) -> io::Result<Self> {
        // Preflight also handles directories not created yet: rejecting a root
        // inside the registry must not first create an invalid registry entry.
        // Receiver::open runs after main prepares both roots, so persisted
        // claims always include the root's own physical identity.
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut existing = absolute.as_path();
        let mut tail = PathBuf::new();
        while !existing.try_exists()? {
            tail = PathBuf::from(
                existing
                    .file_name()
                    .ok_or_else(|| io::Error::other("invalid resource root"))?,
            )
            .join(tail);
            existing = existing
                .parent()
                .ok_or_else(|| io::Error::other("invalid resource root"))?;
        }
        let canonical = fs::canonicalize(existing)?.join(tail);
        let mut ancestors = Vec::new();
        for ancestor in canonical.ancestors() {
            if !ancestor.try_exists()? {
                continue;
            }
            ancestors.push(Ancestor {
                identity: identity(ancestor)?,
                tail: canonical.strip_prefix(ancestor).expect("ancestor").to_path_buf(),
            });
        }
        Ok(Self { canonical, ancestors })
    }

    fn overlaps(&self, other: &Self) -> bool {
        let prefix = |a: &Path, b: &Path| {
            #[cfg(windows)]
            let normalized = (
                PathBuf::from(a.to_string_lossy().to_lowercase()),
                PathBuf::from(b.to_string_lossy().to_lowercase()),
            );
            #[cfg(windows)]
            let (a, b) = (normalized.0.as_path(), normalized.1.as_path());
            a.starts_with(b) || b.starts_with(a)
        };
        prefix(&self.canonical, &other.canonical)
            || self.ancestors.iter().any(|a| {
                other
                    .ancestors
                    .iter()
                    .any(|b| a.identity == b.identity && prefix(&a.tail, &b.tail))
            })
    }
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    resources: Vec<Resource>,
    next: u64,
    pending: BTreeMap<u64, String>,
    #[serde(default)]
    receiver_id: Option<String>,
    #[serde(default)]
    local_scope: Option<LocalScope>,
    // A v1 journal did not track detached descendants. Missing history is
    // unknown, never evidence that its old executable could not have forked.
    #[serde(default = "legacy_process_history")]
    process_history: bool,
}

fn legacy_process_history() -> bool {
    true
}

pub struct Receiver {
    _lock: Arc<File>,
    path: PathBuf,
    state: Arc<Mutex<Snapshot>>,
    failed: watch::Sender<bool>,
    // Every local socket/cloud container keeps its own renewable deadline,
    // never a replaceable pointer to the current connection's permit.
    permit: Option<Arc<crate::cloud::Permit>>,
}

impl Receiver {
    pub fn check_roots(home: &Path, workspace: &Path, config: &Path) -> io::Result<()> {
        Self::resources(home, workspace, config).map(|_| ())
    }

    fn resources(home: &Path, workspace: &Path, config: &Path) -> io::Result<Vec<Resource>> {
        let control = Resource::resolve(&home.join(".local/state/apeiron/receivers"))?;
        let resources = vec![Resource::resolve(workspace)?, Resource::resolve(config)?];
        if resources.iter().any(|resource| resource.overlaps(&control)) {
            return Err(io::Error::other(
                "workspace/config must not cover or reside inside the receiver control state directory",
            ));
        }
        // This excludes explicit exposed roots only. Internal symlinks and
        // arbitrary proc.run commands are not a filesystem security boundary.
        Ok(resources)
    }

    pub fn open(home: &Path, workspace: &Path, config: &Path) -> io::Result<Self> {
        let resources = Self::resources(home, workspace, config)?;
        let directory = home.join(".local/state/apeiron/receivers");
        fs::create_dir_all(&directory)?;
        for ancestor in directory.ancestors() {
            sync_directory(ancestor)?;
            if ancestor == home {
                break;
            }
        }
        // Registry and lifecycle locks are stable files: unlinking an owned
        // lock would permit a second inode/handle to claim the same resource.
        let _registry = loop {
            if let Some(held) = lock(&directory.join("registry.lock"))? {
                break held;
            }
            // Only startup holds this lock. Disjoint manual receivers may
            // start concurrently; contention must not reject either one.
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let mut reusable = None;
        let mut maximum = 0u64;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let number = entry
                .file_name()
                .to_string_lossy()
                .parse::<u64>()
                .map_err(|_| io::Error::other("invalid receiver slot"))?;
            maximum = maximum.max(number);
            let path = entry.path();
            let held = lock(&path.join("live.lock"))?;
            let snapshot: Snapshot = serde_json::from_slice(&fs::read(path.join("state.json"))?)?;
            if snapshot.version != 1 {
                return Err(io::Error::other("unsupported receiver evidence version"));
            }
            let overlap = resources
                .iter()
                .any(|a| snapshot.resources.iter().any(|b| a.overlaps(b)));
            if overlap && (held.is_none() || !snapshot.pending.is_empty()) {
                return Err(io::Error::other(if held.is_none() {
                    "receiver resources are already in use"
                } else {
                    "receiver cleanup is UNKNOWN; refusing to reuse its resources"
                }));
            }
            let same_resources = serde_json::to_vec(&resources)? == serde_json::to_vec(&snapshot.resources)?;
            if overlap && snapshot.process_history && !same_resources {
                return Err(io::Error::other(
                    "receiver cleanup is UNKNOWN for changed resource roots",
                ));
            }
            // At most the peak concurrent/unknown receiver count is retained.
            // Reuse only after the old lifecycle lock is released AND every
            // admitted operation has a durable terminal observation.
            // Exact resource history wins over an earlier reusable empty slot.
            // Directory iteration order must never turn an old process-bearing
            // workspace into a fresh receiver identity.
            if snapshot.pending.is_empty()
                && (reusable.is_none() || (overlap && snapshot.process_history))
                && (!snapshot.process_history || same_resources)
            {
                if let Some(held) = held {
                    reusable = Some((
                        path,
                        held,
                        snapshot.next,
                        snapshot.receiver_id,
                        snapshot.local_scope,
                        snapshot.process_history,
                    ));
                }
            }
        }
        let (slot, held, sequence, receiver_id, local_scope, process_history) = match reusable {
            Some(value) => value,
            None => {
                let slot = directory.join(
                    maximum
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("receiver slots exhausted"))?
                        .to_string(),
                );
                fs::create_dir(&slot)?;
                let held = lock(&slot.join("live.lock"))?.ok_or_else(|| io::Error::other("new receiver lock busy"))?;
                (slot, held, 0, None, None, false)
            }
        };
        let snapshot = Snapshot {
            version: 1,
            resources,
            // A reused slot never reuses its durable operation identity,
            // including after a clean restart with the same v1 request ID.
            next: sequence,
            pending: BTreeMap::new(),
            receiver_id: Some(match receiver_id {
                Some(id) => id,
                None => crate::cloud::fresh()?,
            }),
            local_scope,
            process_history,
        };
        let path = slot.join("state.json");
        persist(&path, &snapshot)?;
        sync_directory(&directory)?;
        Ok(Self {
            _lock: Arc::new(held),
            path,
            state: Arc::new(Mutex::new(snapshot)),
            failed: watch::channel(false).0,
            permit: None,
        })
    }

    /// Each socket gets a fixed permit. Work queued by its predecessor keeps
    /// that predecessor's expired permit even after another socket binds.
    pub fn with_permit(&self, permit: Arc<crate::cloud::Permit>) -> Self {
        Self {
            _lock: self._lock.clone(),
            path: self.path.clone(),
            state: self.state.clone(),
            failed: watch::channel(false).0,
            permit: Some(permit),
        }
    }

    pub fn local_hello(&self) -> io::Result<LocalHello> {
        self.update(|state| {
            Ok(LocalHello {
                receiver_id: state
                    .receiver_id
                    .clone()
                    .ok_or_else(|| io::Error::other("receiver identity unavailable"))?,
                prior_scope: state.local_scope.clone(),
                cleanup: if state.pending.is_empty() { "idle" } else { "unknown" },
                process_history: state.process_history,
            })
        })
    }

    pub fn bind_local(&self, scope: &LocalScope) -> io::Result<LocalHello> {
        let previous = self.local_hello()?;
        let epoch = scope
            .epoch
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0 && *value <= i64::MAX as u64);
        let uuid = |value: &str| {
            value.len() == 36
                && value.bytes().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
        };
        let link_id = scope.link_id.strip_prefix("machine-");
        if epoch.is_none()
            || epoch.map(|value| value.to_string()).as_ref() != Some(&scope.epoch)
            || scope.runtime != "local"
            || scope.tenant_id.is_empty()
            || scope.user_id.is_empty()
            || !uuid(&scope.owner_boot_id)
            || !link_id.is_some_and(uuid)
            || link_id.map(|value| value.replace('-', "")).as_ref() != Some(&scope.binding_id)
            || scope.binding_id.len() != 32
            || !scope
                .binding_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || scope.receiver_id != previous.receiver_id
        {
            return Err(io::Error::other("invalid local receiver scope"));
        }
        self.update(|state| {
            if !state.pending.is_empty() {
                return Err(io::Error::other("receiver cleanup is UNKNOWN"));
            }
            if let Some(prior) = &state.local_scope {
                let old_epoch = prior.epoch.parse::<u64>().map_err(io::Error::other)?;
                if prior.tenant_id != scope.tenant_id
                    || prior.user_id != scope.user_id
                    || epoch.unwrap() < old_epoch
                    || (epoch.unwrap() == old_epoch && prior.owner_boot_id != scope.owner_boot_id)
                {
                    return Err(io::Error::other("stale or mismatched receiver scope"));
                }
                // The same receiver may follow a new Backend boot after its
                // old socket's admitted operations have drained. Keep process
                // history: retiring command admission does not prove arbitrary
                // descendants stopped or settle an unknown Run on the server.
            } else if state.process_history {
                return Err(io::Error::other("legacy receiver descendants are UNKNOWN"));
            }
            state.local_scope = Some(scope.clone());
            Ok(())
        })?;
        Ok(previous)
    }

    #[cfg(target_os = "linux")]
    pub fn open_cloud(
        home: &Path,
        workspace: &Path,
        config: &Path,
        permit: Arc<crate::cloud::Permit>,
    ) -> io::Result<Self> {
        let mut receiver = Self::open(home, workspace, config)?;
        receiver.permit = Some(permit);
        Ok(receiver)
    }

    pub fn check(&self) -> io::Result<()> {
        if *self.failed.borrow() {
            return Err(io::Error::other("receiver admission is closed"));
        }
        if let Some(permit) = &self.permit {
            if !fs::symlink_metadata(&self.path).is_ok_and(|meta| meta.is_file()) {
                self.fail();
                return Err(io::Error::other("cloud receiver journal disappeared"));
            }
            // Revoked authority stops side effects; terminal cleanup facts
            // may still be persisted. A journal failure is different and
            // leaves this receiver permanently closed via failed above.
            permit.check()?;
        }
        Ok(())
    }

    /// Each poll can become writable after a scheduling pause. Gate the actual
    /// syscall, for ACP messages and proc stdin alike, not just queue admission.
    pub async fn write_checked(
        &self,
        writer: &mut (impl tokio::io::AsyncWrite + Unpin),
        bytes: &[u8],
    ) -> io::Result<()> {
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let count = std::future::poll_fn(|cx| {
                if let Err(error) = self.check() {
                    return std::task::Poll::Ready(Err(error));
                }
                std::pin::Pin::new(&mut *writer).poll_write(cx, remaining)
            })
            .await?;
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            remaining = &remaining[count..];
        }
        Ok(())
    }

    pub fn failure(&self) -> watch::Receiver<bool> {
        self.failed.subscribe()
    }

    pub fn fail(&self) {
        self.failed.send_replace(true);
    }

    fn update<T>(&self, change: impl FnOnce(&mut Snapshot) -> io::Result<T>) -> io::Result<T> {
        let mut state = self.state.lock().map_err(|_| {
            self.fail();
            io::Error::other("receiver evidence lock poisoned")
        })?;
        if *self.failed.borrow() {
            return Err(io::Error::other("receiver evidence failed; admission closed"));
        }
        // A removed/corrupt journal cannot be silently reconstructed by the
        // next atomic replace. Compare under the same mutex as the write.
        let intact = (|| {
            if !fs::symlink_metadata(&self.path)?.is_file() || fs::read(&self.path)? != serde_json::to_vec(&*state)? {
                return Err(io::Error::other("receiver evidence changed or disappeared"));
            }
            Ok(())
        })();
        let result = intact.and_then(|()| change(&mut state)).and_then(|value| {
            persist(&self.path, &state)?;
            Ok(value)
        });
        if result.is_err() {
            self.failed.send_replace(true);
        }
        result
    }

    pub fn begin(&self, kind: &str) -> io::Result<u64> {
        self.check()?;
        self.update(|state| {
            state.next = state
                .next
                .checked_add(1)
                .ok_or_else(|| io::Error::other("receiver sequence exhausted"))?;
            state.pending.insert(state.next, kind.to_string());
            if kind == "acp.child" || kind == "proc.run" {
                state.process_history = true;
            }
            Ok(state.next)
        })
    }

    /// Persist only lifecycle completion. ACP replies do not complete a child,
    /// and socket cleanup is never evidence of arbitrary descendant termination.
    pub fn complete(&self, operation: u64) -> io::Result<()> {
        self.update(|state| {
            if state.pending.remove(&operation).is_none() {
                return Err(io::Error::other("unknown receiver operation"));
            }
            Ok(())
        })
    }
}

fn persist(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec(snapshot)?)?;
    file.sync_all()?;
    drop(file);
    replace(&temporary, path)?;
    sync_directory(path.parent().expect("state parent"))
}

#[cfg(unix)]
fn lock(path: &Path) -> io::Result<Option<File>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    unsafe extern "C" {
        fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    // LOCK_EX | LOCK_NB on all supported Unix release targets.
    if unsafe { flock(file.as_raw_fd(), 2 | 4) } == 0 {
        return Ok(Some(file));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(None)
    } else {
        Err(error)
    }
}

#[cfg(windows)]
fn lock(path: &Path) -> io::Result<Option<File>> {
    use std::os::windows::fs::OpenOptionsExt;
    match OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.raw_os_error() == Some(32) => Ok(None), // ERROR_SHARING_VIOLATION
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(path)?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    #[repr(C)]
    #[derive(Default)]
    struct Information {
        attributes: u32,
        creation: [u32; 2],
        access: [u32; 2],
        write: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Information) -> i32;
    }
    let file = OpenOptions::new()
        .access_mode(0x80) // FILE_READ_ATTRIBUTES; directory listing is not required.
        .share_mode(7)
        .custom_flags(0x02000000)
        .open(path)?;
    let mut info = Information::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        u64::from(info.volume),
        u64::from(info.index_high) << 32 | u64::from(info.index_low),
    ))
}

#[cfg(unix)]
fn replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // REPLACE_EXISTING | WRITE_THROUGH; std::fs::rename has no durability flag.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> io::Result<()> {
    // Windows does not expose POSIX directory fsync. Each replacement uses
    // MoveFileExW WRITE_THROUGH after flushing the new file instead.
    Ok(())
}
