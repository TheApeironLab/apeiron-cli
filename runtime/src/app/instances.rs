//! Host-level installation instances. An instance is one config path; the
//! registry `~/.apeiron/instances.json` only indexes paths, names, topologies
//! and reserved entry ports. Port allocation and cluster creation run under the
//! host scheduler lock `~/.apeiron/scheduler.lock` so concurrent wizards cannot
//! reserve the same loopback ports. Nothing here prunes Docker state.
use super::process::{self, Cancellation};
use super::*;
use std::{collections::BTreeSet, fs, net::TcpListener, path::PathBuf, process::Command, time::Instant};

pub const FIRST_PORT: u16 = 54320;
pub const LAST_PORT: u16 = 60000;
pub const TOPOLOGIES: [&str; 3] = ["single-k3d", "single-k3s", "multi-k3s"];

/// External tool access. Production runs the host binaries; tests inject fakes.
pub trait Runner: Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<String>;
}
pub struct Host;
impl Runner for Host {
    fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        process::text(
            Command::new(program).args(args),
            b"",
            120,
            1024 * 1024,
            &Cancellation::default(),
        )
    }
}
#[cfg(test)]
type Handler = Box<dyn Fn(&str, &[&str]) -> Result<String> + Send + Sync>;
#[cfg(test)]
pub struct Fake {
    pub handler: Handler,
    pub calls: std::sync::Mutex<Vec<String>>,
}
#[cfg(test)]
impl Fake {
    pub fn new(handler: impl Fn(&str, &[&str]) -> Result<String> + Send + Sync + 'static) -> Self {
        Self {
            handler: Box::new(handler),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}
#[cfg(test)]
impl Runner for Fake {
    fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        self.calls.lock().unwrap().push(format!("{program} {}", args.join(" ")));
        (self.handler)(program, args)
    }
}

pub fn base() -> Result<PathBuf> {
    Ok(process::home()?.join(".apeiron"))
}
pub fn registry_path(base: &Path) -> PathBuf {
    base.join("instances.json")
}
pub fn scheduler_path(base: &Path) -> PathBuf {
    base.join("scheduler.lock")
}
pub fn id_for(config: &Path) -> String {
    config::hash(config.to_string_lossy().as_bytes())[..12].to_owned()
}
/// `<root>/<name>/config.json` names an instance; returns `(root, name)`.
pub fn located(config: &Path) -> Option<(PathBuf, String)> {
    let dir = config.parent()?;
    let root = dir.parent()?;
    let name = dir.file_name()?.to_str()?;
    if config.file_name()? != "config.json" || root.file_name()? != "instances" || !config::safe_name(name) {
        return None;
    }
    Some((root.to_path_buf(), name.to_owned()))
}
pub fn root(current: &Path, base: &Path) -> PathBuf {
    located(current).map_or_else(|| base.join("instances"), |(root, _)| root)
}
pub fn path_for(base: &Path, name: &str) -> Result<PathBuf> {
    if !config::safe_name(name) {
        return Err(fail("实例名称须为小写字母、数字和连字符。", 2));
    }
    Ok(base.join("instances").join(name).join("config.json"))
}

#[derive(Debug)]
pub struct Scheduler(#[allow(dead_code)] fs::File);
pub fn scheduler(base: &Path, wait: Duration, cancel: &Cancellation) -> Result<Scheduler> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        process::private_directory(base)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(scheduler_path(base))
            .map_err(|_| fail("无法打开宿主调度锁。", 500))?;
        let meta = file.metadata().map_err(|_| fail("无法检查宿主调度锁。", 500))?;
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
            return Err(fail("宿主调度锁必须是当前用户的私有普通文件。", 409));
        }
        let start = Instant::now();
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Scheduler(file));
            }
            if start.elapsed() >= wait {
                return Err(fail("另一个安装实例正在分配端口或创建集群，请稍后重试。", 409));
            }
            cancel.check()?;
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (base, wait, cancel);
        Err(fail("此平台不支持宿主调度锁。", 409))
    }
}

fn validate_registry(v: &Value) -> Result<()> {
    let broken = || fail("实例注册表格式不正确，请检查 ~/.apeiron/instances.json。", 409);
    let list = v["instances"].as_array().ok_or_else(broken)?;
    if v["schemaVersion"] != 1 || v.as_object().is_some_and(|o| o.len() != 2) {
        return Err(broken());
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for i in list {
        let id = i["id"].as_str().unwrap_or("");
        let name = i["name"].as_str().unwrap_or("");
        let path = i["config"].as_str().unwrap_or("");
        if !config::matches(r"^[a-f0-9]{12}$", id)
            || !config::safe_name(name)
            || !TOPOLOGIES.contains(&i["topology"].as_str().unwrap_or(""))
            || !Path::new(path).is_absolute()
            || path.chars().any(char::is_control)
            || id != id_for(Path::new(path))
            || ["httpPort", "httpsPort"]
                .iter()
                .any(|k| !i[k].as_u64().is_some_and(|p| (1..=65535).contains(&p)))
            || !ids.insert(id)
            || !names.insert(name)
        {
            return Err(broken());
        }
    }
    Ok(())
}
pub fn read_registry(base: &Path) -> Result<Value> {
    let path = registry_path(base);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"schemaVersion":1,"instances":[]}));
        }
        Err(_) => return Err(fail("无法读取实例注册表。", 500)),
        Ok(m) if !m.is_file() || m.len() > 1024 * 1024 => {
            return Err(fail("实例注册表必须是普通文件。", 409));
        }
        Ok(_) => {}
    }
    let value: Value = serde_json::from_slice(&fs::read(&path).map_err(|_| fail("无法读取实例注册表。", 500))?)
        .map_err(|_| fail("实例注册表格式不正确，请检查 ~/.apeiron/instances.json。", 409))?;
    validate_registry(&value)?;
    Ok(value)
}
pub fn write_registry(base: &Path, value: &Value) -> Result<()> {
    validate_registry(value)?;
    process::private_directory(base)?;
    let temp = base.join(format!("instances.json.tmp-{:016x}", rand::random::<u64>()));
    private_write(&temp, (serde_json::to_string_pretty(value).unwrap() + "\n").as_bytes())?;
    let result = fs::File::open(&temp)
        .and_then(|f| f.sync_all())
        .and_then(|_| fs::rename(&temp, registry_path(base)))
        .map_err(|_| fail("无法保存实例注册表。", 500));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
/// Resolves `{id}` or `{name}` to a registry ID; names are unique in the registry.
pub fn resolve_id(registry: &Value, input: &Value) -> Result<String> {
    if let Some(id) = input.get("id") {
        return id
            .as_str()
            .filter(|id| config::matches(r"^[a-f0-9]{12}$", id))
            .map(str::to_owned)
            .ok_or_else(|| fail("实例 ID 格式不正确。", 400));
    }
    let name = input["name"].as_str().unwrap_or("");
    if !config::safe_name(name) {
        return Err(fail("实例名称格式不正确。", 400));
    }
    registry["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["name"] == name)
        .and_then(|i| i["id"].as_str().map(str::to_owned))
        .ok_or_else(|| fail("实例不存在。", 404))
}
pub fn entry<'a>(registry: &'a Value, id: &str) -> Result<&'a Value> {
    registry["instances"]
        .as_array()
        .and_then(|l| l.iter().find(|i| i["id"] == id))
        .ok_or_else(|| fail("实例不存在。", 404))
}

/// Host ports from `docker ps -a --format '{{.Ports}}'`, including stopped containers.
pub fn bound_ports(docker_ps: &str) -> BTreeSet<u16> {
    let mut out = BTreeSet::new();
    for item in docker_ps.split([',', '\n']) {
        let Some((host, _)) = item.trim().split_once("->") else {
            continue;
        };
        let range = host.rsplit(':').next().unwrap_or("");
        let (a, b) = range.split_once('-').unwrap_or((range, range));
        if let (Ok(a), Ok(b)) = (a.parse::<u16>(), b.parse::<u16>()) {
            if a <= b && b - a < 1024 {
                out.extend(a..=b);
            }
        }
    }
    out
}
pub fn bindable(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}
pub fn allocate_ports(registry: &Value, bind: &dyn Fn(u16) -> bool, docker_ps: &str) -> Result<(u16, u16)> {
    let mut taken = bound_ports(docker_ps);
    for i in registry["instances"].as_array().into_iter().flatten() {
        for k in ["httpPort", "httpsPort"] {
            if let Some(p) = i[k].as_u64() {
                taken.insert(p as u16);
            }
        }
    }
    let mut port = FIRST_PORT;
    while port < LAST_PORT {
        if [port, port + 1].iter().all(|p| !taken.contains(p) && bind(*p)) {
            return Ok((port, port + 1));
        }
        port += 2;
    }
    Err(fail("没有可用的本机入口端口。", 409))
}

fn template(name: &str, topology: &str, http: u16, https: u16, source: &Value) -> Value {
    let local = topology == "single-k3d";
    // The package chosen in the wizard supplies the application catalog and its required apps.
    let apps = source["apps"].as_array().cloned().unwrap_or_default();
    let d = &source["deployment"];
    let i = &d["installation"];
    let same = i["topology"] == topology;
    let nodes = if topology == "multi-k3s" {
        if same {
            i["nodes"].clone()
        } else {
            json!([
                {"host":"192.0.2.10","name":"node-0","address":"192.0.2.10","role":"server"},
                {"host":"192.0.2.11","name":"node-1","address":"192.0.2.11","role":"agent"}
            ])
        }
    } else {
        json!([])
    };
    let text = |k: &str| i[k].as_str().filter(|_| same).unwrap_or("");
    json!({"schemaVersion":2,"slug":name,"apps":apps,"deployment":{
        "runner":if local{"docker"}else{"native"},
        "installation":{"topology":topology,"domain":format!("{name}.apeironlab.internal"),
            "entryIp":if local{"127.0.0.1"}else{""},"httpPort":http,"httpsPort":https,"ha":false,
            "sshUser":text("sshUser"),"sshKey":text("sshKey"),"sshPort":i["sshPort"].as_u64().filter(|_|same).unwrap_or(22),"nodes":nodes},
        "root":"","environment":"","kubeconfig":"","workDir":"","image":"chentu-lab",
        "offline":d["offline"].as_bool().unwrap_or(false),"bundleDir":d["bundleDir"].as_str().unwrap_or("")}})
}
fn record(config: &Path, name: &str, topology: &str, http: u16, https: u16) -> Value {
    json!({"id":id_for(config),"name":name,"topology":topology,"config":config,"httpPort":http,"httpsPort":https,"createdAt":chrono::Utc::now().to_rfc3339()})
}
pub struct Creation<'a> {
    pub base: &'a Path,
    pub root: &'a Path,
    pub source: &'a Value,
    pub bind: &'a dyn Fn(u16) -> bool,
    pub runner: &'a dyn Runner,
    pub cancel: &'a Cancellation,
}
pub fn create(c: &Creation, name: &str, topology: &str) -> Result<Value> {
    if !config::safe_name(name) {
        return Err(fail("实例名称须为小写字母、数字和连字符。", 400));
    }
    if !TOPOLOGIES.contains(&topology) {
        return Err(fail("请选择安装方式。", 400));
    }
    let _lock = scheduler(c.base, Duration::from_secs(30), c.cancel)?;
    let mut registry = read_registry(c.base)?;
    let directory = c.root.join(name);
    let config = directory.join("config.json");
    if registry["instances"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["name"] == name || i["config"] == json!(config))
        || fs::symlink_metadata(&directory).is_ok()
    {
        return Err(fail("实例名称或目录已存在。", 409));
    }
    config::location(&config)?;
    let (http, https) = if topology == "single-k3d" {
        let ps = c
            .runner
            .run("docker", &["ps", "-a", "--format", "{{.Ports}}"])
            .map_err(|_| fail("无法检查 Docker，请确认 Docker 已启动。", 400))?;
        allocate_ports(&registry, c.bind, &ps)?
    } else {
        (80, 443)
    };
    process::private_directory(&directory)?;
    private_write(
        &config,
        (serde_json::to_string_pretty(&template(name, topology, http, https, c.source)).unwrap() + "\n").as_bytes(),
    )?;
    config::read(&config)?;
    let entry = record(&config, name, topology, http, https);
    registry["instances"].as_array_mut().unwrap().push(entry.clone());
    write_registry(c.base, &registry)?;
    Ok(entry)
}
/// Registers an existing `<root>/<name>/config.json` (for example the one a
/// packaged start.sh selected) so the registry reflects its reserved ports.
pub fn adopt(base: &Path, config: &Path, cancel: &Cancellation) -> Result<Option<Value>> {
    let Some((_, name)) = located(config) else {
        return Ok(None);
    };
    let _lock = scheduler(base, Duration::from_secs(2), cancel)?;
    let mut registry = read_registry(base)?;
    if let Ok(existing) = entry(&registry, &id_for(config)) {
        return Ok(Some(existing.clone()));
    }
    let saved = config::read(config)?;
    let i = &saved["config"]["deployment"]["installation"];
    let topology = i["topology"].as_str().unwrap_or("single-k3d").to_owned();
    let local = topology == "single-k3d";
    let http = i["httpPort"].as_u64().unwrap_or(if local { 54320 } else { 80 }) as u16;
    let https = i["httpsPort"].as_u64().unwrap_or(if local { 54321 } else { 443 }) as u16;
    if registry["instances"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["name"] == name)
    {
        return Err(fail("同名实例已注册到其他配置路径。", 409));
    }
    let entry = record(config, &name, &topology, http, https);
    registry["instances"].as_array_mut().unwrap().push(entry.clone());
    write_registry(base, &registry)?;
    Ok(Some(entry))
}
fn locked(config: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let Ok(file) = fs::OpenOptions::new()
            .read(true)
            .open(format!("{}.installation-owner", config.display()))
        else {
            return false;
        };
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) != 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        false
    }
}
pub fn cluster_name(entry: &Value) -> Option<String> {
    if entry["topology"] != "single-k3d" {
        return None;
    }
    let config = Path::new(entry["config"].as_str()?);
    let saved = config::read(config).ok()?;
    let domain = saved["config"]["deployment"]["installation"]["domain"].as_str()?;
    Some(cluster::identity(config, domain).1)
}
pub fn k3d_state(cluster: &str, runner: &dyn Runner) -> &'static str {
    let name = format!("k3d-{cluster}-serverlb");
    match runner.run("docker", &["inspect", "--format", "{{json .State.Running}}", &name]) {
        Ok(s) if s.trim() == "true" => "running",
        Ok(s) if s.trim() == "false" => "stopped",
        Ok(_) => "unknown",
        Err(_) => match runner.run(
            "docker",
            &[
                "ps",
                "-a",
                "--filter",
                &format!("name=^/{name}$"),
                "--format",
                "{{.Names}}",
            ],
        ) {
            Ok(s) if s.trim().is_empty() => "absent",
            _ => "unknown",
        },
    }
}
pub fn status(entry: &Value, runner: &dyn Runner) -> Value {
    let config = Path::new(entry["config"].as_str().unwrap_or(""));
    let mut out = entry.clone();
    let exists = fs::symlink_metadata(config).is_ok_and(|m| m.is_file());
    out["exists"] = json!(exists);
    out["phase"] = json!(match resume::recovery(config) {
        Ok(Some(r)) if r["succeeded"] == true => "succeeded",
        Ok(Some(_)) => "interrupted",
        Ok(None) if exists => "idle",
        Ok(None) => "new",
        Err(_) => "blocked",
    });
    out["locked"] = json!(locked(config));
    // Display only: the saved domain, never trusted for identity.
    out["domain"] = config::read(config)
        .ok()
        .and_then(|saved| {
            saved["config"]["deployment"]["installation"]["domain"]
                .as_str()
                .map(str::to_owned)
        })
        .map_or(Value::Null, Value::String);
    let cluster = cluster_name(entry);
    out["k3d"] = json!(cluster.as_deref().map(|c| k3d_state(c, runner)));
    out["cluster"] = json!(cluster);
    out
}
pub fn list(base: &Path, current: &Path, runner: &dyn Runner) -> Result<Value> {
    let registry = read_registry(base)?;
    let id = id_for(current);
    let instances = registry["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| status(e, runner))
        .collect::<Vec<_>>();
    Ok(json!({"schemaVersion":1,"registry":registry_path(base),"path":current,
        "current":if instances.iter().any(|i|i["id"]==id){json!(id)}else{Value::Null},"instances":instances}))
}
pub fn delete(base: &Path, id: &str, current: &Path, runner: &dyn Runner, cancel: &Cancellation) -> Result<Value> {
    let _lock = scheduler(base, Duration::from_secs(30), cancel)?;
    let mut registry = read_registry(base)?;
    let entry = entry(&registry, id)?.clone();
    if entry["topology"] != "single-k3d" {
        return Err(fail("仅支持删除 K3d 实例；原生 K3s 实例请按宸途流程卸载。", 409));
    }
    let config = PathBuf::from(entry["config"].as_str().unwrap());
    if config == current {
        return Err(fail("不能删除当前实例，请先切换到其他实例。", 409));
    }
    let directory = config.parent().unwrap().to_path_buf();
    let _guard = if fs::symlink_metadata(&directory).is_ok() {
        Some(resume::guard(&config).map_err(|_| fail("另一个进程正在使用该实例，不能删除。", 409))?)
    } else {
        None
    };
    let state = status(&entry, runner);
    if state["phase"] == "blocked" {
        return Err(fail("该实例的恢复记录损坏或权限不安全，请先人工处理后再删除。", 409));
    }
    let cluster = state["cluster"].as_str().map(str::to_owned);
    let mut cluster_deleted = false;
    if let Some(cluster) = &cluster {
        match state["k3d"].as_str().unwrap_or("unknown") {
            "running" => return Err(fail("实例的 K3d 集群仍在运行，请先停止集群后再删除。", 409)),
            "stopped" => {
                runner
                    .run("k3d", &["cluster", "delete", cluster])
                    .map_err(|_| fail("k3d 未能删除集群，实例保留。", 502))?;
                cluster_deleted = true;
            }
            "absent" => {}
            _ => return Err(fail("无法确认集群状态，请检查 Docker 后重试。", 400)),
        }
        // The registry container is detached from the cluster; absence is fine.
        let _ = runner.run("k3d", &["registry", "delete", &format!("k3d-{cluster}-reg")]);
    }
    let removable = located(&config).is_some();
    drop(_guard);
    if removable && fs::symlink_metadata(&directory).is_ok() {
        fs::remove_dir_all(&directory).map_err(|_| fail("无法删除实例目录。", 500))?;
    }
    registry["instances"].as_array_mut().unwrap().retain(|i| i["id"] != id);
    write_registry(base, &registry)?;
    Ok(json!({"deleted":entry,"clusterDeleted":cluster_deleted,"removedDirectory":removable}))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::app::contracts::Temp;

    fn registry_with(ports: &[(u16, u16)]) -> Value {
        json!({"schemaVersion":1,"instances":ports.iter().enumerate().map(|(n,(h,s))|{
            let config = PathBuf::from(format!("/tmp/apeiron-test/instances/inst{n}/config.json"));
            json!({"id":id_for(&config),"name":format!("inst{n}"),"topology":"single-k3d","config":config,"httpPort":h,"httpsPort":s})
        }).collect::<Vec<_>>()})
    }

    #[test]
    fn port_allocation_skips_registry_bind_failures_and_docker_bindings() {
        let free = |_: u16| true;
        assert_eq!(allocate_ports(&registry_with(&[]), &free, "").unwrap(), (54320, 54321));
        // Registry reservations are skipped even when nothing listens yet.
        assert_eq!(
            allocate_ports(&registry_with(&[(54320, 54321)]), &free, "").unwrap(),
            (54322, 54323)
        );
        // A listener (or any bind failure) on either port skips the pair.
        let busy = |p: u16| p != 54323;
        assert_eq!(
            allocate_ports(&registry_with(&[(54320, 54321)]), &busy, "").unwrap(),
            (54324, 54325)
        );
        // Stopped k3d load balancers still reserve their published ports.
        let ps = "0.0.0.0:54324->80/tcp, [::]:54324->80/tcp, 127.0.0.1:54325->443/tcp\n\n127.0.0.1:5000-5001->5000-5001/tcp\n";
        assert_eq!(bound_ports(ps), BTreeSet::from([5000, 5001, 54324, 54325]));
        assert_eq!(
            allocate_ports(&registry_with(&[(54320, 54321)]), &busy, ps).unwrap(),
            (54326, 54327)
        );
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        assert!(!bindable(listener.local_addr().unwrap().port()));
    }

    #[test]
    fn registry_writes_are_atomic_private_and_validated() {
        let temp = Temp::new();
        let base = temp.0.join("apeiron");
        assert_eq!(read_registry(&base).unwrap()["instances"], json!([]));
        let registry = registry_with(&[(54320, 54321)]);
        write_registry(&base, &registry).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(registry_path(&base)).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(read_registry(&base).unwrap(), registry);
        assert!(fs::read_dir(&base).unwrap().count() == 1, "no temp files left behind");
        let mut broken = registry.clone();
        broken["instances"][0]["id"] = json!("000000000000");
        assert!(write_registry(&base, &broken).is_err());
        assert_eq!(
            read_registry(&base).unwrap(),
            registry,
            "failed write keeps the old registry"
        );
        fs::write(
            registry_path(&base),
            b"{\"schemaVersion\":1,\"instances\":[{\"id\":\"x\"}]}",
        )
        .unwrap();
        assert!(read_registry(&base).unwrap_err().message.contains("注册表"));
    }

    #[test]
    fn scheduler_lock_serializes_hosts_and_rejects_waiting_past_deadline() {
        let temp = Temp::new();
        let base = temp.0.join("apeiron");
        let cancel = Cancellation::default();
        let held = scheduler(&base, Duration::from_millis(10), &cancel).unwrap();
        let start = Instant::now();
        let second = scheduler(&base, Duration::from_millis(200), &cancel);
        assert!(second.unwrap_err().message.contains("另一个安装实例"));
        assert!(start.elapsed() >= Duration::from_millis(200));
        let (send, receive) = std::sync::mpsc::channel();
        let b = base.clone();
        let worker = std::thread::spawn(move || {
            let lock = scheduler(&b, Duration::from_secs(5), &Cancellation::default());
            send.send(lock.is_ok()).unwrap();
        });
        assert!(
            receive.recv_timeout(Duration::from_millis(300)).is_err(),
            "waiter blocks while held"
        );
        drop(held);
        assert!(receive.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn create_allocates_ports_registers_and_select_delete_follow_registry_state() {
        let temp = Temp::new();
        let base = temp.0.join("apeiron");
        let root = base.join("instances");
        let runner = Fake::new(|program, args| match (program, args.first().copied()) {
            ("docker", Some("ps")) => Ok("127.0.0.1:54320->80/tcp, 127.0.0.1:54321->443/tcp\n".into()),
            ("docker", Some("inspect")) => Ok("true\n".into()),
            _ => Err(fail("unexpected", 502)),
        });
        let cancel = Cancellation::default();
        let source = json!({"apps":["apeiron","nexus","ontology","vasi"],"deployment":{"offline":true,"bundleDir":"/opt/bundle"}});
        let creation = Creation {
            base: &base,
            root: &root,
            source: &source,
            bind: &|_| true,
            runner: &runner,
            cancel: &cancel,
        };
        assert!(create(&creation, "Bad Name", "single-k3d").is_err());
        assert!(create(&creation, "lab", "lxc").is_err());
        let a = create(&creation, "lab", "single-k3d").unwrap();
        assert_eq!(
            (a["httpPort"].as_u64(), a["httpsPort"].as_u64()),
            (Some(54322), Some(54323))
        );
        let config = PathBuf::from(a["config"].as_str().unwrap());
        assert_eq!(config, root.join("lab/config.json"));
        let saved = config::read(&config).unwrap()["config"].clone();
        assert_eq!(saved["deployment"]["installation"]["domain"], "lab.apeironlab.internal");
        assert_eq!(saved["deployment"]["bundleDir"], "/opt/bundle");
        assert_eq!(saved["apps"], source["apps"]);
        assert!(create(&creation, "lab", "single-k3d")
            .unwrap_err()
            .message
            .contains("已存在"));
        let b = create(&creation, "team", "multi-k3s").unwrap();
        assert_eq!(b["httpPort"], 80);
        assert!(config::read(Path::new(b["config"].as_str().unwrap())).is_ok());
        let listed = list(&base, &config, &runner).unwrap();
        assert_eq!(listed["current"], a["id"]);
        assert_eq!(listed["instances"][0]["phase"], "idle");
        assert_eq!(listed["instances"][0]["k3d"], "running");
        assert_eq!(listed["instances"][1]["k3d"], Value::Null);
        // Running clusters, native instances and the current instance are never deleted.
        let other = root.join("other/config.json");
        assert!(delete(&base, a["id"].as_str().unwrap(), &other, &runner, &cancel)
            .unwrap_err()
            .message
            .contains("仍在运行"));
        assert!(delete(&base, b["id"].as_str().unwrap(), &other, &runner, &cancel)
            .unwrap_err()
            .message
            .contains("K3d"));
        assert!(delete(&base, a["id"].as_str().unwrap(), &config, &runner, &cancel)
            .unwrap_err()
            .message
            .contains("当前实例"));
        assert!(config.is_file());
        assert!(!runner.calls().iter().any(|c| c.starts_with("k3d")));
        let stopped = Fake::new(|program, args| match (program, args.first().copied()) {
            ("docker", Some("inspect")) => Ok("false\n".into()),
            ("k3d", _) => Ok(String::new()),
            _ => Err(fail("unexpected", 502)),
        });
        let result = delete(&base, a["id"].as_str().unwrap(), &other, &stopped, &cancel).unwrap();
        assert_eq!(result["clusterDeleted"], true);
        let cluster = cluster::identity(&config, "lab.apeironlab.internal").1;
        assert_eq!(
            stopped.calls(),
            vec![
                format!("docker inspect --format {{{{json .State.Running}}}} k3d-{cluster}-serverlb"),
                format!("k3d cluster delete {cluster}"),
                format!("k3d registry delete k3d-{cluster}-reg"),
            ]
        );
        assert!(!root.join("lab").exists());
        assert_eq!(read_registry(&base).unwrap()["instances"].as_array().unwrap().len(), 1);
        assert_eq!(
            delete(&base, "0123456789ab", &other, &stopped, &cancel)
                .unwrap_err()
                .code,
            404
        );
    }

    #[test]
    fn adopt_registers_packaged_instance_paths_once_and_ignores_other_layouts() {
        let temp = Temp::new();
        let base = temp.0.join("apeiron");
        let cancel = Cancellation::default();
        let loose = temp.write("config.json", "{}");
        assert_eq!(adopt(&base, &loose, &cancel).unwrap(), None);
        let config = base.join("installers/cand/instances/default/config.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        let mut value = template("default", "single-k3d", 54330, 54331, &Value::Null);
        value["deployment"]["installation"]["domain"] = json!("team.internal");
        fs::write(&config, value.to_string()).unwrap();
        let first = adopt(&base, &config, &cancel).unwrap().unwrap();
        assert_eq!(
            (first["httpPort"].as_u64(), first["name"].as_str()),
            (Some(54330), Some("default"))
        );
        assert_eq!(adopt(&base, &config, &cancel).unwrap().unwrap()["id"], first["id"]);
        assert_eq!(read_registry(&base).unwrap()["instances"].as_array().unwrap().len(), 1);
        assert_eq!(root(&config, &base), base.join("installers/cand/instances"));
        assert_eq!(root(&loose, &base), base.join("instances"));
    }
}
