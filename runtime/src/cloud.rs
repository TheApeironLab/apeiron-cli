//! Cloud control protocol. One container incarnation has one immutable binding.
//! The DB/Kubernetes owner proves whole-container termination before replacing
//! it; this process never converts EOF or a missing reply into completion.
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(crate) const WINDOW_MS: u64 = 10_000;
#[cfg(any(target_os = "linux", test))]
const MAX_RECORD_BYTES: usize = 1024 * 1024;
#[cfg(any(target_os = "linux", test))]
const HEADER_BYTES: usize = 25;

pub struct Permit {
    deadline: AtomicU64,
    retired: AtomicBool,
}

impl Permit {
    pub(crate) fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
    }
    pub(crate) fn new() -> Self {
        Self {
            deadline: AtomicU64::new(0),
            retired: AtomicBool::new(false),
        }
    }

    pub fn check(&self) -> io::Result<()> {
        self.check_at(now_ms()?)
    }

    fn check_at(&self, now: u64) -> io::Result<()> {
        if self.retired.load(Ordering::SeqCst) || now >= self.deadline.load(Ordering::SeqCst) {
            self.retired.store(true, Ordering::SeqCst);
            return Err(io::Error::other("cloud permit expired; receiver retired"));
        }
        Ok(())
    }

    pub(crate) fn grant(&self, issued: u64, now: u64) -> io::Result<()> {
        let previous = self.deadline.load(Ordering::SeqCst);
        if self.retired.load(Ordering::SeqCst) || (previous != 0 && now >= previous) || now >= issued + WINDOW_MS {
            self.retired.store(true, Ordering::SeqCst);
            return Err(io::Error::other("late cloud permit cannot revive receiver"));
        }
        self.deadline.store(issued + WINDOW_MS, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn now_ms() -> io::Result<u64> {
    #[repr(C)]
    struct Timespec {
        seconds: std::ffi::c_long,
        nanos: std::ffi::c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    let mut time = Timespec { seconds: 0, nanos: 0 };
    // CLOCK_BOOTTIME includes suspend: pausing the machine cannot extend authority.
    if unsafe { clock_gettime(7, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(time.seconds as u64 * 1000 + time.nanos as u64 / 1_000_000)
}

#[cfg(target_os = "macos")]
pub(crate) fn now_ms() -> io::Result<u64> {
    #[repr(C)]
    struct Timebase {
        numerator: u32,
        denominator: u32,
    }
    unsafe extern "C" {
        fn mach_continuous_time() -> u64;
        fn mach_timebase_info(value: *mut Timebase) -> i32;
    }
    let mut scale = Timebase {
        numerator: 0,
        denominator: 0,
    };
    if unsafe { mach_timebase_info(&mut scale) } != 0 || scale.denominator == 0 {
        return Err(io::Error::other("monotonic clock unavailable"));
    }
    // Unlike mach_absolute_time, continuous time includes system sleep.
    Ok(
        (u128::from(unsafe { mach_continuous_time() }) * u128::from(scale.numerator)
            / u128::from(scale.denominator)
            / 1_000_000) as u64,
    )
}

#[cfg(windows)]
pub(crate) fn now_ms() -> io::Result<u64> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetTickCount64() -> u64;
    }
    // GetTickCount64 includes time spent suspended; wall-clock edits cannot renew it.
    Ok(unsafe { GetTickCount64() })
}

pub(crate) fn fresh() -> io::Result<String> {
    let mut bytes = [0; 16];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| io::Error::other("receiver random source failed"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(target_os = "linux")]
struct Scope {
    binding_id: String,
    tenant_id: String,
    user_id: String,
    runtime: String,
    owner_boot_id: String,
    epoch: String,
    receiver_boot_id: String,
    subject: Subject,
    workspace: Workspace,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(target_os = "linux")]
struct Subject {
    kind: String,
    namespace: String,
    pod_name: String,
    pod_uid: String,
    container_name: String,
    container_id: String,
    restart_count: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(target_os = "linux")]
struct Workspace {
    name: String,
    uid: String,
}

#[cfg(any(target_os = "linux", test))]
fn hex_id(value: &str) -> io::Result<[u8; 16]> {
    if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(io::Error::other("invalid receiver identifier"));
    }
    let mut bytes = [0; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(io::Error::other)?;
    }
    Ok(bytes)
}

#[cfg(any(target_os = "linux", test))]
struct Record {
    kind: u8,
    binding: [u8; 16],
    sequence: u64,
    payload: Vec<u8>,
}
#[cfg(any(target_os = "linux", test))]
impl Record {
    fn encode(&self) -> io::Result<Vec<u8>> {
        let length = HEADER_BYTES + self.payload.len();
        if length > MAX_RECORD_BYTES || self.kind > 1 {
            return Err(io::Error::other("invalid receiver record"));
        }
        let mut bytes = Vec::with_capacity(length + 4);
        bytes.extend_from_slice(&(length as u32).to_be_bytes());
        bytes.push(self.kind);
        bytes.extend_from_slice(&self.binding);
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        if !(HEADER_BYTES..=MAX_RECORD_BYTES).contains(&bytes.len()) || bytes[0] > 1 {
            return Err(io::Error::other("invalid receiver record"));
        }
        Ok(Self {
            kind: bytes[0],
            binding: bytes[1..17].try_into().unwrap(),
            sequence: u64::from_be_bytes(bytes[17..25].try_into().unwrap()),
            payload: bytes[25..].to_vec(),
        })
    }
}

#[cfg(any(target_os = "linux", test))]
struct Sequence {
    binding: [u8; 16],
    next: u64,
}
#[cfg(any(target_os = "linux", test))]
impl Sequence {
    fn accept(&mut self, record: &Record) -> io::Result<bool> {
        if record.binding != self.binding || record.sequence < self.next {
            return Ok(false);
        }
        if record.sequence != self.next {
            return Err(io::Error::other("receiver sequence gap"));
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| io::Error::other("receiver sequence exhausted"))?;
        Ok(true)
    }
}

#[cfg(not(target_os = "linux"))]
pub fn run(_: &crate::Args, _: &[String]) -> anyhow::Result<()> {
    anyhow::bail!("cloud-receiver requires Linux container PID 1")
}

#[cfg(target_os = "linux")]
pub use linux::run;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{
        fsops::Roots,
        protocol::{decode_body, Inbound, MachineInfo, Outbound, BODY_CHUNK, PROTOCOL_VERSION},
        receiver::Receiver,
        Args, Live, Session,
    };
    use anyhow::{anyhow, bail, Result};
    use serde_json::{json, Value};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::{atomic::AtomicPtr, Arc};
    use std::time::Duration;
    use tokio::sync::mpsc;

    unsafe extern "C" {
        fn getpid() -> i32;
        fn getuid() -> u32;
        fn getgid() -> u32;
        fn setgroups(size: usize, list: *const u32) -> i32;
        fn setresuid(real: u32, effective: u32, saved: u32) -> i32;
        fn setresgid(real: u32, effective: u32, saved: u32) -> i32;
        fn chown(path: *const std::ffi::c_char, uid: u32, gid: u32) -> i32;
        fn prctl(option: i32, ...) -> i32;
        fn capset(header: *const CapHeader, data: *const [CapData; 2]) -> i32;
        fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32;
        fn signal(number: i32, handler: usize) -> usize;
        fn timer_create(clock: i32, event: *const std::ffi::c_void, timer: *mut *mut std::ffi::c_void) -> i32;
        fn timer_settime(timer: *mut std::ffi::c_void, flags: i32, value: *const Timer, old: *mut Timer) -> i32;
        fn _exit(code: i32) -> !;
        fn clock_gettime(clock: i32, time: *mut Time) -> i32;
        fn fcntl(fd: i32, command: i32, ...) -> i32;
        fn read(fd: i32, buffer: *mut std::ffi::c_void, length: usize) -> isize;
        fn write(fd: i32, buffer: *const std::ffi::c_void, length: usize) -> isize;
    }
    #[repr(C)]
    struct CapHeader {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CapData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    #[repr(C)]
    struct Time {
        seconds: std::ffi::c_long,
        nanos: std::ffi::c_long,
    }
    #[repr(C)]
    struct Timer {
        interval: Time,
        value: Time,
    }
    static WATCHED_PERMIT: AtomicPtr<Permit> = AtomicPtr::new(std::ptr::null_mut());

    extern "C" fn expire(_: i32) {
        let permit = WATCHED_PERMIT.load(Ordering::SeqCst);
        if permit.is_null() {
            return;
        }
        let mut time = Time { seconds: 0, nanos: 0 };
        // Only async-signal-safe libc calls and lock-free atomics in this
        // handler. A kernel BOOTTIME timer also covers blocked root bootstrap,
        // without creating a root thread that setresuid could leave behind.
        let valid_clock = unsafe { clock_gettime(7, &mut time) } == 0;
        let now = time.seconds as u64 * 1000 + time.nanos as u64 / 1_000_000;
        let permit = unsafe { &*permit };
        if !valid_clock || permit.retired.load(Ordering::SeqCst) || now >= permit.deadline.load(Ordering::SeqCst) {
            unsafe { _exit(1) }
        }
    }
    fn watch(permit: &Arc<Permit>) -> io::Result<()> {
        // PID 1 owns this allocation until exit; leaking one Arc makes the
        // signal handler safe even while an error unwinds the normal stack.
        WATCHED_PERMIT.store(Arc::into_raw(permit.clone()).cast_mut(), Ordering::SeqCst);
        if unsafe { signal(14, expire as *const () as usize) } == usize::MAX {
            return Err(io::Error::last_os_error());
        }
        let mut timer = std::ptr::null_mut();
        syscall(unsafe { timer_create(7, std::ptr::null(), &mut timer) })?;
        let periodic = Timer {
            interval: Time {
                seconds: 0,
                nanos: 50_000_000,
            },
            value: Time {
                seconds: 0,
                nanos: 50_000_000,
            },
        };
        syscall(unsafe { timer_settime(timer, 0, &periodic, std::ptr::null_mut()) })
    }

    fn syscall(result: i32) -> io::Result<()> {
        if result == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn wait(fd: i32, events: i16, deadline: Option<u64>) -> io::Result<()> {
        loop {
            if deadline.is_some_and(|end| now_ms().map_or(true, |now| now >= end)) {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "receiver handshake expired"));
            }
            let mut descriptor = PollFd { fd, events, revents: 0 };
            let result = unsafe { poll(&mut descriptor, 1, 50) };
            if result > 0 {
                return Ok(());
            }
            if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return Err(io::Error::last_os_error());
            }
        }
    }
    fn read_exact(bytes: &mut [u8], deadline: Option<u64>) -> io::Result<()> {
        let mut position = 0;
        while position < bytes.len() {
            wait(0, 1, deadline)?;
            // std::io::stdin buffers ahead; polling fd 0 while its buffer
            // already holds the next record would deadlock a coalesced stream.
            let count = unsafe { read(0, bytes[position..].as_mut_ptr().cast(), bytes.len() - position) };
            if count == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "receiver stdin closed"));
            }
            if count > 0 {
                position += count as usize;
                continue;
            }
            let error = io::Error::last_os_error();
            if !matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) {
                return Err(error);
            }
        }
        Ok(())
    }
    fn read_record(deadline: Option<u64>) -> io::Result<Record> {
        let mut prefix = [0; 4];
        read_exact(&mut prefix, deadline)?;
        let length = u32::from_be_bytes(prefix) as usize;
        if !(HEADER_BYTES..=MAX_RECORD_BYTES).contains(&length) {
            return Err(io::Error::other("invalid receiver record length"));
        }
        let mut bytes = vec![0; length];
        read_exact(&mut bytes, deadline)?;
        Record::decode(&bytes)
    }
    fn write_record(record: &Record, deadline: Option<u64>) -> io::Result<()> {
        let bytes = record.encode()?;
        let mut position = 0;
        while position < bytes.len() {
            wait(1, 4, deadline)?;
            let count = unsafe { write(1, bytes[position..].as_ptr().cast(), bytes.len() - position) };
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            if count > 0 {
                position += count as usize;
                continue;
            }
            let error = io::Error::last_os_error();
            if !matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) {
                return Err(error);
            }
        }
        Ok(())
    }
    struct Writer {
        sequence: Sequence,
    }
    impl Writer {
        fn send(&mut self, kind: u8, payload: Vec<u8>, deadline: Option<u64>) -> io::Result<()> {
            write_record(
                &Record {
                    kind,
                    binding: self.sequence.binding,
                    sequence: self.sequence.next,
                    payload,
                },
                deadline,
            )?;
            self.sequence.next = self
                .sequence
                .next
                .checked_add(1)
                .ok_or_else(|| io::Error::other("receiver sequence exhausted"))?;
            Ok(())
        }
        fn json(&mut self, value: Value, deadline: Option<u64>) -> io::Result<()> {
            self.send(0, serde_json::to_vec(&value)?, deadline)
        }
    }
    struct Challenge {
        nonce: String,
        issued: u64,
    }
    impl Challenge {
        fn new() -> io::Result<Self> {
            Ok(Self {
                nonce: fresh()?,
                issued: now_ms()?,
            })
        }
        fn value(&self) -> Value {
            json!({"t":"permit.challenge", "nonce":self.nonce, "window_ms":WINDOW_MS})
        }
        fn consume(self, value: &Value, permit: &Permit) -> io::Result<()> {
            if value.get("nonce").and_then(Value::as_str) != Some(&self.nonce) {
                return Err(io::Error::other("unknown permit nonce"));
            }
            permit.grant(self.issued, now_ms()?)
        }
    }

    fn own(path: &Path) -> io::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        syscall(unsafe { chown(path.as_ptr(), 1000, 1000) })
    }
    fn directory(path: &Path, permit: &Permit) -> io::Result<()> {
        permit.check()?;
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(io::Error::other("cloud initialization path is not a real directory")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(path)?;
            }
            Err(error) => return Err(error),
        }
        permit.check()?;
        own(path)?;
        let permissions = fs::metadata(path)?.permissions().mode() | 0o700;
        fs::set_permissions(path, fs::Permissions::from_mode(permissions))
    }
    fn own_journal(path: &Path, permit: &Permit) -> io::Result<()> {
        permit.check()?;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other("journal symlink"));
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                own_journal(&entry?.path(), permit)?;
            }
        }
        own(path)
    }
    fn initialize(args: &Args, receiver: &Receiver, permit: &Permit, journal: &Path) -> Result<()> {
        let operation = receiver.begin("initialize")?;
        // Only fixed top-level directories are repaired; user data is never
        // recursively chowned. No worker thread or user subprocess exists yet.
        for path in [&args.root, "/home/user/.nexus", &args.config_home] {
            directory(Path::new(path), permit)?;
        }
        own_journal(journal, permit)?;
        permit.check()?;
        syscall(unsafe { prctl(38, 1usize, 0usize, 0usize, 0usize) })?; // PR_SET_NO_NEW_PRIVS
        syscall(unsafe { setgroups(0, std::ptr::null()) })?;
        syscall(unsafe { setresgid(1000, 1000, 1000) })?;
        syscall(unsafe { setresuid(1000, 1000, 1000) })?;
        let empty = CapData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        };
        syscall(unsafe {
            capset(
                &CapHeader {
                    version: 0x20080522,
                    pid: 0,
                },
                &[empty; 2],
            )
        })?;
        if unsafe { getuid() } != 1000 || unsafe { getgid() } != 1000 {
            bail!("cloud identity drop failed");
        }
        for name in ["sudocode.json", "settings.json"] {
            permit.check()?;
            let destination = Path::new(&args.config_home).join(name);
            if fs::symlink_metadata(&destination).is_ok() {
                continue;
            }
            let bytes = fs::read(Path::new("/opt/apeiron/defaults").join(name))?;
            let temporary = destination.with_file_name(format!(
                ".{name}.{}.tmp",
                journal.file_name().unwrap().to_string_lossy()
            ));
            permit.check()?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            permit.check()?;
            // Publish only a complete file and never replace an existing user
            // config. Death before link leaves only an unreferenced temp file;
            // a later incarnation never mistakes it for an initialized config.
            let published = fs::hard_link(&temporary, &destination);
            fs::remove_file(&temporary)?;
            match published {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            fs::File::open(&args.config_home)?.sync_all()?;
        }
        receiver.complete(operation)?;
        Ok(())
    }

    pub fn run(args: &Args, argv: &[String]) -> Result<()> {
        if unsafe { getpid() } != 1 || unsafe { getuid() } != 0 {
            bail!("cloud-receiver requires root container PID 1");
        }
        if args.root != "/home/user" || args.config_home != "/home/user/.nexus/sudocode" || argv.is_empty() {
            bail!("cloud receiver requires fixed workspace/config roots and agent argv");
        }
        let pod_uid = std::env::var("APEIRON_POD_UID")?;
        if pod_uid.is_empty() {
            bail!("cloud Pod UID is missing");
        }
        // Nonblocking descriptors keep even a partial attach record subject to
        // the boottime deadline; no unbounded read_exact is allowed after bind.
        for fd in [0, 1] {
            let flags = unsafe { fcntl(fd, 3) };
            syscall(flags)?;
            syscall(unsafe { fcntl(fd, 4, flags | 0o4000) })?;
        }
        let boot = fresh()?;
        let mut hello: Option<Challenge> = None;
        let scope = loop {
            let record = read_record(hello.as_ref().map(|value| value.issued + WINDOW_MS))?;
            if record.kind != 0 {
                bail!("body before cloud bind");
            }
            let value: Value = serde_json::from_slice(&record.payload)?;
            match value.get("t").and_then(Value::as_str) {
                Some("probe") if record.binding == [0; 16] && record.sequence == 0 && value["protocol"] == 2 => {
                    // A duplicate probe reuses its original nonce/deadline.
                    if hello.is_none() {
                        hello = Some(Challenge::new()?);
                    }
                    let challenge = hello.as_ref().unwrap();
                    write_record(
                        &Record {
                            kind: 0,
                            binding: [0; 16],
                            sequence: 0,
                            payload: serde_json::to_vec(
                                &json!({"t":"hello", "protocol":2, "receiver_boot_id":boot, "pod_uid":pod_uid, "nonce":challenge.nonce, "window_ms":WINDOW_MS}),
                            )?,
                        },
                        Some(challenge.issued + WINDOW_MS),
                    )?;
                }
                Some("bind") if value["protocol"] == 2 && record.sequence == 0 => {
                    let challenge = hello.as_ref().ok_or_else(|| anyhow!("bind before probe"))?;
                    let scope: Scope = serde_json::from_value(value["scope"].clone())?;
                    if value["nonce"] != challenge.nonce
                        || now_ms()? >= challenge.issued + WINDOW_MS
                        || record.binding != hex_id(&scope.binding_id)?
                        || record.binding == [0; 16]
                        || scope.receiver_boot_id != boot
                        || scope.subject.pod_uid != pod_uid
                        || scope.runtime != "cloud"
                        || scope.subject.kind != "k8s"
                        || [
                            &scope.tenant_id,
                            &scope.user_id,
                            &scope.owner_boot_id,
                            &scope.subject.namespace,
                            &scope.subject.pod_name,
                            &scope.subject.container_name,
                            &scope.subject.container_id,
                            &scope.workspace.name,
                            &scope.workspace.uid,
                        ]
                        .iter()
                        .any(|value| value.is_empty())
                        || scope.epoch.is_empty()
                        || !scope.epoch.bytes().all(|byte| byte.is_ascii_digit())
                    {
                        bail!("cloud binding does not match receiver");
                    }
                    break scope;
                }
                _ => bail!("invalid pre-bind cloud control"),
            }
        };
        let binding = hex_id(&scope.binding_id)?;
        let mut input = Sequence { binding, next: 1 };
        let mut writer = Writer {
            sequence: Sequence { binding, next: 0 },
        };
        let permit = Arc::new(Permit::new());
        let mut challenge = Some(Challenge::new()?);
        let initial_deadline = challenge.as_ref().unwrap().issued + WINDOW_MS;
        writer.json(json!({"t":"bound", "scope":scope}), Some(initial_deadline))?;
        writer.json(challenge.as_ref().unwrap().value(), Some(initial_deadline))?;
        // Container-layer control state is independent of the mounted user PVC.
        let journal = PathBuf::from("/run/apeiron-receiver").join(&boot);
        fs::create_dir_all(journal.parent().unwrap())?;
        fs::create_dir(&journal)?;
        fs::set_permissions(&journal, fs::Permissions::from_mode(0o700))?;
        let receiver = Arc::new(Receiver::open_cloud(
            &journal,
            Path::new(&args.root),
            Path::new(&args.config_home),
            permit.clone(),
        )?);
        loop {
            let deadline = permit.deadline.load(Ordering::SeqCst).max(initial_deadline);
            let record = read_record(Some(deadline))?;
            if !input.accept(&record)? {
                continue;
            }
            if record.kind != 0 {
                bail!("body before initialization");
            }
            let value: Value = serde_json::from_slice(&record.payload)?;
            match value.get("t").and_then(Value::as_str) {
                Some("bind") if value["scope"] == serde_json::to_value(&scope)? => {
                    writer.json(json!({"t":"bound","scope":scope}), Some(deadline))?;
                }
                Some("permit") => {
                    challenge
                        .take()
                        .ok_or_else(|| anyhow!("permit nonce already consumed"))?
                        .consume(&value, &permit)?;
                    writer.json(
                        json!({"t":"permit.accepted","nonce":value["nonce"]}),
                        Some(permit.deadline.load(Ordering::SeqCst)),
                    )?;
                }
                Some("initialize") => {
                    permit.check()?;
                    watch(&permit)?;
                    initialize(args, &receiver, &permit, &journal)?;
                    break;
                }
                _ => bail!("ordinary operation before cloud initialization"),
            }
        }
        permit.check()?;
        writer.json(
            json!({"t":"initialized", "uid":1000, "gid":1000}),
            Some(permit.deadline.load(Ordering::SeqCst)),
        )?;
        // No threads were created while root. The kernel timer already bounds
        // admission and exit, including blocked filesystem and child cleanup.
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
        let result = runtime.block_on(engine(
            args.clone(),
            argv.to_vec(),
            scope,
            input,
            writer,
            receiver,
            permit.clone(),
        ));
        permit.retired.store(true, Ordering::SeqCst);
        // PID 1 exit is the boundary, not an indefinite wait on live children or
        // Tokio's stdin thread. The Backend still requires kubelet evidence.
        if let Err(error) = result {
            eprintln!("[apeiron] cloud receiver retired: {error}");
        }
        std::process::exit(1)
    }

    async fn engine(
        args: Args,
        argv: Vec<String>,
        scope: Scope,
        mut input: Sequence,
        mut writer: Writer,
        receiver: Arc<Receiver>,
        permit: Arc<Permit>,
    ) -> Result<()> {
        let (workspace, roots) = Roots::prepare(Path::new(&args.root), Path::new(&args.config_home)).await?;
        let config_home = fs::canonicalize(&args.config_home)?;
        let info = MachineInfo {
            version: env!("APEIRON_VERSION").into(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            user_root: workspace.to_string_lossy().into_owned(),
            config_home: config_home.to_string_lossy().into_owned(),
            agent_cmd: argv.join(" "),
        };
        let session = Arc::new(Session {
            args,
            server: String::new(),
            token: String::new(),
            argv,
            workspace,
            config_home,
            roots,
            info: info.clone(),
            receiver: receiver.clone(),
        });
        let (records, mut record_rx) = mpsc::unbounded_channel();
        std::thread::spawn(move || loop {
            let record = read_record(None);
            let failed = record.is_err();
            if records.send(record).is_err() || failed {
                break;
            }
        });
        let (wire, mut wire_rx) = mpsc::unbounded_channel::<(u8, Vec<u8>)>();
        let writer_permit = permit.clone();
        let writer_receiver = receiver.clone();
        std::thread::spawn(move || {
            while let Some((kind, bytes)) = wire_rx.blocking_recv() {
                if writer_permit.check().is_err()
                    || writer
                        .send(kind, bytes, Some(writer_permit.deadline.load(Ordering::SeqCst)))
                        .is_err()
                {
                    break;
                }
            }
            writer_receiver.fail();
        });
        let send = |value: Value| -> Result<()> {
            wire.send((0, serde_json::to_vec(&value)?))
                .map_err(|_| anyhow!("cloud writer closed"))
        };
        send(serde_json::to_value(Outbound::Ready {
            protocol: PROTOCOL_VERSION,
            info,
            authority: None,
        })?)?;
        let (out, mut out_rx) = mpsc::unbounded_channel();
        let (raw, mut raw_rx) = mpsc::unbounded_channel();
        let mut live = Live::default();
        let mut pending: Option<Challenge> = None;
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        loop {
            permit.check()?;
            tokio::select! {
                _ = tick.tick() => {
                    receiver.check()?;
                    if pending.is_none() && permit.deadline.load(Ordering::SeqCst).saturating_sub(now_ms()?) <= WINDOW_MS / 2 {
                        let next = Challenge::new()?;
                        send(next.value())?;
                        pending = Some(next);
                    }
                },
                Some(value) = out_rx.recv() => { send(serde_json::to_value(value)?)?; },
                Some(bytes) = raw_rx.recv() => { wire.send((1, bytes)).map_err(|_| anyhow!("cloud writer closed"))?; },
                completed = live.requests.join_next(), if !live.requests.is_empty() => {
                    if let Some(value) = completed { live.inbound.remove(&value??); }
                },
                record = record_rx.recv() => {
                    let record = record.ok_or_else(|| anyhow!("cloud input ended"))??;
                    if !input.accept(&record)? { continue; }
                    receiver.check()?;
                    if record.kind == 1 {
                        let frame = decode_body(&record.payload).ok_or_else(|| anyhow!("invalid cloud body"))?;
                        if frame.kind == BODY_CHUNK {
                            if let Some(sender) = live.inbound.get(&frame.id) { let _ = sender.send(Some(frame.payload.to_vec())); }
                        } else if let Some(sender) = live.inbound.remove(&frame.id) { let _ = sender.send(None); }
                        continue;
                    }
                    let value: Value = serde_json::from_slice(&record.payload)?;
                    match value.get("t").and_then(Value::as_str) {
                        Some("permit") => {
                            pending.take().ok_or_else(|| anyhow!("unknown permit nonce"))?.consume(&value, &permit)?;
                            send(json!({"t":"permit.accepted", "nonce":value["nonce"]}))?;
                        },
                        Some("initialize") => send(json!({"t":"initialized", "uid":1000, "gid":1000}))?,
                        Some("bind") if value["scope"] == serde_json::to_value(&scope)? => send(json!({"t":"bound", "scope":scope}))?,
                        _ => {
                            let text = String::from_utf8(record.payload)?;
                            if matches!(serde_json::from_str::<Inbound>(&text)?, Inbound::Welcome {..} | Inbound::Unknown) { bail!("invalid cloud business frame"); }
                            session.clone().on_text(&text, &mut live, &out, &raw).await?;
                        },
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_golden_and_sequence_isolation() {
        let record = Record {
            kind: 1,
            binding: [0x12; 16],
            sequence: 7,
            payload: vec![1, 2, 3],
        };
        let bytes = record.encode().unwrap();
        assert_eq!(&bytes[..5], &[0, 0, 0, 28, 1]);
        assert_eq!(&bytes[21..29], &[0, 0, 0, 0, 0, 0, 0, 7]);
        let decoded = Record::decode(&bytes[4..]).unwrap();
        let mut sequence = Sequence {
            binding: [0x12; 16],
            next: 7,
        };
        let wrong = Record {
            binding: [0x34; 16],
            ..Record::decode(&bytes[4..]).unwrap()
        };
        assert!(!sequence.accept(&wrong).unwrap());
        assert!(sequence.accept(&decoded).unwrap());
        assert!(!sequence.accept(&decoded).unwrap());
        let gap = Record { sequence: 9, ..decoded };
        assert!(sequence.accept(&gap).is_err());
        assert!(Record::decode(&[0; 24]).is_err());
        assert!(hex_id("ABCDEFABCDEFABCDEFABCDEFABCDEFABCD").is_err());
    }
    #[test]
    fn deadline_belongs_to_challenge_not_delivery_and_never_revives() {
        let permit = Permit::new();
        permit.grant(100, 9_999).unwrap();
        permit.check_at(10_099).unwrap();
        assert!(permit.check_at(10_100).is_err());
        assert!(permit.grant(10_000, 10_101).is_err());
        let late = Permit::new();
        assert!(late.grant(100, 10_100).is_err());
        assert!(late.grant(20_000, 20_001).is_err());
        let renewed = Permit::new();
        renewed.grant(100, 101).unwrap();
        renewed.grant(5_100, 5_101).unwrap();
        renewed.check_at(15_099).unwrap();
        assert!(renewed.check_at(15_100).is_err());
    }
}
