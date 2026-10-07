use super::process::{self, Cancellation};
use super::*;
use std::{fs, net::ToSocketAddrs, process::Command, sync::mpsc, thread, time::Instant};
fn command(name: &str, args: &[&str]) -> String {
    process::text(Command::new(name).args(args), b"", 2, 65536, &Cancellation::default())
        .unwrap_or_default()
        .trim()
        .to_owned()
}
pub fn architecture(s: &str) -> &str {
    match s {
        "x64" | "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => s,
    }
}
pub fn supported(os: &str, version: &str, arch: &str) -> bool {
    os.eq_ignore_ascii_case("ubuntu")
        && match architecture(arch) {
            "amd64" => config::matches(r"^22\.04(?:\.[0-9]+)?$", version),
            "arm64" => config::matches(r"^24\.04(?:\.[0-9]+)?$", version),
            _ => false,
        }
}
// Some ARM kernels report only "unknown"; lscpu may nest distinct core models.
fn linux_cpu_model(names: &[&str], fallback: impl FnOnce() -> String) -> String {
    fn add(names: &mut Vec<String>, name: &str) {
        let name = name.trim();
        if !name.is_empty() && !name.eq_ignore_ascii_case("unknown") && !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    fn walk(rows: &Value, names: &mut Vec<String>) {
        if let Some(rows) = rows.as_array() {
            for row in rows {
                if row["field"] == "Model name:" {
                    add(names, row["data"].as_str().unwrap_or(""));
                }
                walk(&row["children"], names);
            }
        }
    }
    let mut result = Vec::new();
    for name in names {
        add(&mut result, name);
    }
    if result.is_empty() {
        let value: Value = serde_json::from_str(&fallback()).unwrap_or(Value::Null);
        walk(&value["lscpu"], &mut result);
    }
    result.join(" + ")
}
pub fn machine() -> Value {
    let arch = command("uname", &["-m"]);
    let kernel = command("uname", &["-r"]);
    let mut os = std::env::consts::OS.to_owned();
    let mut version = String::new();
    let cpu;
    let memory;
    if cfg!(target_os = "macos") {
        os = "macOS".into();
        version = command("/usr/bin/sw_vers", &["-productVersion"]);
        cpu = command("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"]);
        memory = command("/usr/sbin/sysctl", &["-n", "hw.memsize"])
            .parse::<f64>()
            .unwrap_or(0.0);
    } else {
        if let Ok(release) = fs::read_to_string("/etc/os-release") {
            for line in release.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    match k {
                        "NAME" => os = v.trim_matches('"').to_owned(),
                        "VERSION_ID" => version = v.trim_matches('"').to_owned(),
                        _ => {}
                    }
                }
            }
        }
        let info = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let mut names = Vec::new();
        for line in info.lines() {
            if let Some(("model name", name)) = line.split_once(':').map(|(k, v)| (k.trim(), v.trim())) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        cpu = linux_cpu_model(&names, || command("lscpu", &["--json"]));
        memory = fs::read_to_string("/proc/meminfo")
            .unwrap_or_default()
            .lines()
            .find(|s| s.starts_with("MemTotal:"))
            .and_then(|s| s.split_whitespace().nth(1))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            * 1024.0;
    }
    let arch = if cfg!(target_os = "macos") && command("/usr/sbin/sysctl", &["-n", "hw.optional.arm64"]) == "1" {
        "arm64"
    } else {
        architecture(&arch)
    };
    json!({"os":{"name":os,"version":version,"kernel":kernel},"hardware":{"architecture":arch,"runtimeArchitecture":architecture(std::env::consts::ARCH),"cpu":cpu,"cores":thread::available_parallelism().map(usize::from).unwrap_or(0),"memoryGiB":(memory/1073741824.0*10.0).round()/10.0}})
}
pub fn host() -> Value {
    let mut addresses = Vec::<String>::new();
    #[cfg(unix)]
    unsafe {
        let mut list = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) == 0 {
            let mut p = list;
            while !p.is_null() {
                let iface = &*p;
                if !iface.ifa_addr.is_null() && (*iface.ifa_addr).sa_family as i32 == libc::AF_INET {
                    let name = std::ffi::CStr::from_ptr(iface.ifa_name).to_string_lossy();
                    let addr = &*(iface.ifa_addr as *const libc::sockaddr_in);
                    let ip = std::net::Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr)).to_string();
                    if !["lo", "utun", "docker", "veth", "br-"]
                        .iter()
                        .any(|s| name.starts_with(s))
                        && !ip.starts_with("127.")
                        && config::safe_ip(&ip)
                        && !addresses.contains(&ip)
                    {
                        addresses.push(ip);
                    }
                }
                p = iface.ifa_next;
            }
            libc::freeifaddrs(list);
        }
    }
    json!({"name":command("hostname",&[]),"addresses":addresses})
}
#[derive(Default)]
struct ProbeState {
    cache: Option<(Instant, Value)>,
    active: Option<(Cancellation, std::sync::Arc<ProbeJob>)>,
}
#[derive(Default)]
struct ProbeJob {
    result: std::sync::Mutex<Option<Result<Value>>>,
    ready: std::sync::Condvar,
}
#[derive(Default)]
pub struct EnvironmentProbe(std::sync::Mutex<ProbeState>);
impl EnvironmentProbe {
    pub fn cancel(&self) {
        let mut state = self.0.lock().unwrap();
        if let Some((cancel, _)) = state.active.take() {
            cancel.cancel();
        }
        state.cache = None;
    }
    pub fn run(&self, offline: bool, refresh: bool) -> Result<Value> {
        self.run_with(offline, refresh, |cancel| probe(offline, cancel))
    }
    fn run_with(
        &self,
        offline: bool,
        refresh: bool,
        scan: impl FnOnce(&Cancellation) -> Result<Value>,
    ) -> Result<Value> {
        if offline {
            self.cancel();
            return scan(&Cancellation::default());
        }
        let (cancel, job, owner) = {
            let mut state = self.0.lock().unwrap();
            if !refresh {
                if let Some((at, value)) = &state.cache {
                    if at.elapsed() < Duration::from_secs(30) {
                        return Ok(value.clone());
                    }
                }
            }
            if let Some((cancel, job)) = &state.active {
                (cancel.clone(), job.clone(), false)
            } else {
                let cancel = Cancellation::default();
                let job = std::sync::Arc::new(ProbeJob::default());
                state.active = Some((cancel.clone(), job.clone()));
                (cancel, job, true)
            }
        };
        if owner {
            let result = scan(&cancel);
            let mut state = self.0.lock().unwrap();
            if state
                .active
                .as_ref()
                .is_some_and(|(_, active)| std::sync::Arc::ptr_eq(active, &job))
            {
                state.active = None;
                if cancel.check().is_ok() {
                    if let Ok(value) = &result {
                        state.cache = Some((Instant::now(), value.clone()));
                    }
                }
            }
            *job.result.lock().unwrap() = Some(result.clone());
            job.ready.notify_all();
            result
        } else {
            let mut result = job.result.lock().unwrap();
            while result.is_none() {
                result = job.ready.wait(result).unwrap();
            }
            result.clone().unwrap()
        }
    }
}
pub fn probe(offline: bool, cancel: &Cancellation) -> Result<Value> {
    probe_with(offline, cancel, &probe_check)
}
fn probe_with(
    offline: bool,
    cancel: &Cancellation,
    scan: &(dyn Fn(&str, &str, bool, &Cancellation) -> Value + Sync),
) -> Result<Value> {
    let machine = machine();
    let checks = if offline {
        Vec::new()
    } else {
        thread::scope(|scope| {
            let handles = [
                ("公共网站", "www.microsoft.com", false),
                ("海外访问（Google）", "www.google.com", false),
                ("GitHub API", "api.github.com", false),
                ("安装包下载域名", "release-assets.githubusercontent.com", true),
            ]
            .into_iter()
            .map(|(name, host, notfound)| scope.spawn(move || scan(name, host, notfound, cancel)))
            .collect::<Vec<_>>();
            handles.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>()
        })
    };
    let count = checks.iter().filter(|c| c["status"] == "reachable").count();
    let status = if offline {
        "skipped"
    } else if cancel.check().is_err() {
        "cancelled"
    } else if count == checks.len() {
        "reachable"
    } else if count > 0 {
        "limited"
    } else {
        "unreachable"
    };
    Ok(
        json!({"machine":machine,"checkedAt":chrono::Utc::now().to_rfc3339(),"network":{"status":status,"checks":checks}}),
    )
}

fn probe_check(name: &str, host: &str, notfound: bool, cancel: &Cancellation) -> Value {
    let start = Instant::now();
    let result = (|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| fail("unreachable", 400))?;
        runtime.block_on(async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(4))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| fail("unreachable", 400))?;
            tokio::select! {
                _ = cancel.cancelled() => Err(fail("cancelled", 400)),
                response = client.head(format!("https://{host}/"))
                    .header("User-Agent", "apeiron-cli-connectivity").send() =>
                    response.map_err(|e| fail(network_error(&e), 400)),
            }
        })
    })();
    let mut check = json!({"name":name,"host":host,"elapsedMs":start.elapsed().as_millis()});
    if cancel.check().is_err() {
        check["status"] = json!("cancelled");
    } else {
        match result {
            Ok(r) => {
                let status = r.status().as_u16();
                check["status"] = json!(if (200..400).contains(&status) || (notfound && status == 404) {
                    "reachable"
                } else {
                    "http-error"
                });
                check["httpStatus"] = json!(status);
            }
            Err(e) => check["status"] = json!(e.message),
        }
    }
    check
}
fn network_error(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        return "timeout";
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    let mut text = String::new();
    while let Some(e) = source {
        text.push_str(&e.to_string().to_lowercase());
        source = e.source();
    }
    classify_network_error(&text)
}
fn classify_network_error(message: &str) -> &'static str {
    if [
        "dns",
        "name or service not known",
        "nodename nor servname",
        "failed to lookup",
        "enotfound",
    ]
    .iter()
    .any(|s| message.contains(s))
    {
        "dns-error"
    } else if ["certificate", "cert_", "tls", "ssl"]
        .iter()
        .any(|s| message.contains(s))
    {
        "tls-error"
    } else {
        "unreachable"
    }
}
pub fn dns(input: &Value, include_root: bool, cancel: &Cancellation) -> Result<Value> {
    dns_with(
        input,
        include_root,
        cancel,
        Duration::from_secs(4),
        std::sync::Arc::new(lookup),
    )
}
type DnsLookup = dyn Fn(&str, bool) -> std::io::Result<std::collections::BTreeSet<String>> + Send + Sync;
fn dns_with(
    input: &Value,
    include_root: bool,
    cancel: &Cancellation,
    timeout: Duration,
    lookup: std::sync::Arc<DnsLookup>,
) -> Result<Value> {
    let domain = input["domain"].as_str().unwrap_or("");
    let ip = input["entryIp"].as_str().unwrap_or("");
    let local = input["local"]
        .as_bool()
        .ok_or_else(|| fail("请填写有效的域名和入口 IPv4 地址。", 400))?;
    if !config::safe_domain(domain) || !config::safe_ip(ip) || (local && ip != "127.0.0.1") {
        return Err(fail("请填写有效的域名和入口 IPv4 地址。", 400));
    }
    let mut hosts = vec![format!("apeiron.{domain}"), format!("iam.{domain}")];
    if include_root {
        hosts.push(domain.into());
    }
    if !local {
        hosts.push(format!(
            "apeiron-check-{:012x}.{domain}",
            rand::random::<u64>() & 0xffffffffffff
        ));
    }
    let checks = thread::scope(|scope| {
        let handles = hosts
            .into_iter()
            .map(|host| {
                let lookup = lookup.clone();
                scope.spawn(move || {
                    let (send, receive) = mpsc::channel();
                    let name = host.clone();
                    thread::spawn(move || {
                        let addresses = lookup(&name, !local);
                        let _ = send.send(addresses);
                    });
                    let start = Instant::now();
                    let (addresses, status) = loop {
                        if cancel.check().is_err() || start.elapsed() > timeout {
                            break (vec![], "timeout");
                        }
                        match receive.recv_timeout(Duration::from_millis(50)) {
                            Ok(Ok(a)) => {
                                let a = a.into_iter().collect::<Vec<_>>();
                                let status = if !a.is_empty() && a.iter().all(|a| a == ip) {
                                    "matched"
                                } else {
                                    "mismatch"
                                };
                                break (a, status);
                            }
                            Ok(Err(_)) => break (vec![], "unresolved"),
                            Err(mpsc::RecvTimeoutError::Disconnected) => break (vec![], "unresolved"),
                            Err(_) => {}
                        }
                    };
                    json!({"host":host,"addresses":addresses,"status":status})
                })
            })
            .collect::<Vec<_>>();
        handles.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>()
    });
    Ok(
        json!({"checkedFrom":host()["name"],"checkedAt":chrono::Utc::now().to_rfc3339(),"passed":checks.iter().all(|c|c["status"]=="matched"),"wildcard":!local,"checks":checks}),
    )
}
pub fn aliases() -> Result<Value> {
    let source = fs::read_to_string(process::home()?.join(".ssh/config")).unwrap_or_default();
    if source.len() > 512 * 1024 {
        return Err(fail("SSH 配置过大，请手动填写地址。", 400));
    }
    let mut names = Vec::new();
    for line in source.lines() {
        let mut words = line.split_whitespace();
        if words.next().is_some_and(|s| s.eq_ignore_ascii_case("host")) {
            for name in words.filter(|s| config::safe_host(s)) {
                if !names.contains(&name) && names.len() < 32 {
                    names.push(name);
                }
            }
        }
    }
    Ok(json!({"aliases":names}))
}
pub fn node(host: &str, connection: &Value, local: bool, cancel: &Cancellation) -> Result<Value> {
    let mut cmd = Command::new(if local { "python3" } else { "ssh" });
    if local {
        cmd.arg("-");
    } else {
        if !config::safe_host(host) {
            return Err(fail("节点地址格式不正确。", 400));
        }
        cmd.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=8",
            "-p",
        ])
        .arg(connection["sshPort"].as_u64().unwrap().to_string());
        if let Some(user) = connection["sshUser"].as_str().filter(|s| !s.is_empty()) {
            cmd.args(["-l", user]);
        }
        if let Some(key) = connection["sshKey"].as_str().filter(|s| !s.is_empty()) {
            cmd.args(["-i", key, "-o", "IdentitiesOnly=yes"]);
        }
        cmd.args([host, "python3 -"]);
    }
    let result = process::capture(
        &mut cmd,
        include_bytes!("../../assets/node-probe.py"),
        Duration::from_secs(20),
        128 * 1024,
        cancel,
    )
    .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|_| fail("节点响应格式不正确。", 400)));
    cancel.check()?;
    if let Ok(mut facts) = result {
        if config::safe_name(facts["name"].as_str().unwrap_or(""))
            && facts["addresses"].is_array()
            && ["cores", "memoryGiB", "diskGiB"].iter().all(|k| facts[k].is_number())
        {
            let supported = supported(
                facts["os"].as_str().unwrap_or(""),
                facts["version"].as_str().unwrap_or(""),
                facts["architecture"].as_str().unwrap_or(""),
            );
            facts["host"] = json!(host);
            facts["supported"] = json!(supported);
            let error = if !supported {
                Some("K3s 主机需要 Ubuntu 22.04 / AMD64 或 Ubuntu 24.04 / ARM64。")
            } else if facts["sudo"] != true {
                Some("需要 root 或免密 sudo。")
            } else if facts["existingCluster"] == true {
                Some("检测到已有 K3s 数据，请使用未安装集群的机器。")
            } else if facts["addresses"].as_array().unwrap().is_empty() {
                Some("未发现可用的内网 IPv4。")
            } else {
                None
            };
            if let Some(error) = error {
                facts["error"] = json!(error);
            }
            return Ok(facts);
        }
    }
    Ok(
        json!({"host":host,"name":"","os":"","version":"","architecture":"","cores":0,"memoryGiB":0,"diskGiB":0,"addresses":[],"sudo":false,"existingCluster":false,"supported":false,"error":if local{"无法检测本机，请检查 Python 3。"}else{"SSH 检测失败：检查地址、用户、密钥和 Python 3；首次连接请先用 ssh 确认主机指纹。"}}),
    )
}
pub fn nodes(input: &Value, cancel: &Cancellation) -> Result<Value> {
    let connection = config::connection(input)?;
    let hosts = input["hosts"]
        .as_array()
        .ok_or_else(|| fail("节点检测参数不正确。", 400))?;
    let mut seen = std::collections::HashSet::new();
    if hosts.is_empty()
        || hosts.len() > 32
        || hosts
            .iter()
            .any(|h| !config::safe_host(h.as_str().unwrap_or("")) || !seen.insert(h.as_str().unwrap_or("")))
    {
        return Err(fail("请填写 1–32 个不重复的 IP 或 SSH 别名。", 400));
    }
    let mut nodes = Vec::new();
    for chunk in hosts.chunks(4) {
        let batch = thread::scope(|scope| {
            let handles = chunk
                .iter()
                .map(|h| scope.spawn(|| node(h.as_str().unwrap(), &connection, false, cancel)))
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Result<Vec<_>>>()
        })?;
        nodes.extend(batch);
    }
    Ok(json!({"nodes":nodes}))
}

// Public ingress must be verified through DNS, never a workstation's hosts file.
// Local/private ingress deliberately uses the OS resolver so installed hosts work.
pub fn lookup(name: &str, dns_only: bool) -> std::io::Result<std::collections::BTreeSet<String>> {
    if !dns_only {
        return (name, 443)
            .to_socket_addrs()
            .map(|a| a.map(|v| v.ip().to_string()).collect());
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        use hickory_resolver::{
            config::{LookupIpStrategy, ResolveHosts},
            TokioResolver,
        };
        let mut builder = TokioResolver::builder_tokio().map_err(std::io::Error::other)?;
        builder.options_mut().use_hosts_file = ResolveHosts::Never;
        builder.options_mut().ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        let resolver = builder.build();
        let result = tokio::time::timeout(Duration::from_secs(4), resolver.lookup_ip(format!("{name}.")))
            .await
            .map_err(std::io::Error::other)?
            .map_err(std::io::Error::other)?;
        Ok(result.iter().map(|ip| ip.to_string()).collect())
    })
}

pub fn validate_platform(target: &Value, nodes: &[Value]) -> Result<()> {
    let p = &target["hostPlatform"];
    if p["os"] != "ubuntu"
        || nodes.is_empty()
        || nodes.iter().any(|n| {
            !supported(
                n["os"].as_str().unwrap_or(""),
                n["version"].as_str().unwrap_or(""),
                n["architecture"].as_str().unwrap_or(""),
            ) || n["version"]
                .as_str()
                .unwrap_or("")
                .split('.')
                .take(2)
                .collect::<Vec<_>>()
                .join(".")
                != p["version"].as_str().unwrap_or("")
                || architecture(n["architecture"].as_str().unwrap_or("")) != p["architecture"].as_str().unwrap_or("")
        })
    {
        return Err(fail("原生安装包 hostPlatform 与节点系统或架构不匹配。", 400));
    }
    Ok(())
}

#[cfg(test)]
mod cpu_contracts {
    use super::*;
    #[test]
    fn arm_core_models_are_deduplicated_including_nested_lscpu_and_unknown_names() {
        assert_eq!(
            linux_cpu_model(&["unknown", "unknown"], || {
                json!({"lscpu":[{"field":"Vendor ID:","data":"ARM"},{"field":"Model name:","data":"Cortex-X925"},{"field":"Model name:","data":"Cortex-A725"},{"field":"Model name:","data":"Cortex-X925"}]}).to_string()
            }),
            "Cortex-X925 + Cortex-A725"
        );
        assert_eq!(
            linux_cpu_model(&["unknown"], || {
                json!({"lscpu":[{"field":"Vendor ID:","data":"ARM","children":[{"field":"Model name:","data":"Cortex-X925"}]}]}).to_string()
            }),
            "Cortex-X925"
        );
        for response in [
            "",
            "invalid json",
            "{}",
            r#"{"lscpu":[{"field":"Model name:","data":"unknown"}]}"#,
        ] {
            assert_eq!(linux_cpu_model(&["unknown"], || response.into()), "");
        }
        assert_eq!(
            linux_cpu_model(&["known", "unknown"], || panic!("must not call lscpu")),
            "known"
        );
        assert_eq!(
            linux_cpu_model(&["AMD processor", "AMD processor"], || panic!("must not call lscpu")),
            "AMD processor"
        );
    }
}

#[cfg(test)]
mod dns_contracts {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[test]
    fn dns_checks_every_address_wildcard_and_matrix_root_with_bounded_cancellation() {
        let input = json!({"domain":"team.apeironlab.internal","entryIp":"192.0.2.10","local":false});
        let seen = Arc::new(Mutex::new(Vec::new()));
        let copy = seen.clone();
        let good = Arc::new(move |host: &str, dns_only: bool| {
            assert!(dns_only);
            copy.lock().unwrap().push(host.to_owned());
            Ok(std::collections::BTreeSet::from(["192.0.2.10".into()]))
        });
        let run = |input: &Value, root, lookup| {
            dns_with(input, root, &Cancellation::default(), Duration::from_millis(10), lookup).unwrap()
        };
        assert_eq!(run(&input, false, good.clone())["passed"], true);
        let names = seen.lock().unwrap();
        assert_eq!(names.len(), 3);
        assert!(names
            .iter()
            .any(|n| config::matches(r"^apeiron-check-[a-f0-9]+\.team\.apeironlab\.internal$", n)));
        drop(names);
        seen.lock().unwrap().clear();
        assert_eq!(run(&input, true, good)["passed"], true);
        assert!(seen.lock().unwrap().contains(&"team.apeironlab.internal".to_owned()));
        assert_eq!(seen.lock().unwrap().len(), 4);
        let wildcard = run(
            &input,
            false,
            Arc::new(|h, _| {
                if h.starts_with("apeiron-check-") {
                    Err(std::io::Error::other("NXDOMAIN"))
                } else {
                    Ok(std::collections::BTreeSet::from(["192.0.2.10".into()]))
                }
            }),
        );
        assert_eq!(wildcard["passed"], false);
        assert_eq!(wildcard["checks"][2]["status"], "unresolved");
        for addresses in [vec![], vec!["192.0.2.10", "192.0.2.11"]] {
            assert_eq!(
                run(
                    &input,
                    false,
                    Arc::new(move |_, _| Ok(addresses.iter().map(|s| s.to_string()).collect()))
                )["passed"],
                false
            );
        }
        let mut local = input.clone();
        local["local"] = json!(true);
        local["entryIp"] = json!("127.0.0.1");
        let checked = run(
            &local,
            false,
            Arc::new(|_, dns_only| {
                assert!(!dns_only);
                Ok(std::collections::BTreeSet::from(["127.0.0.1".into()]))
            }),
        );
        assert_eq!(checked["passed"], true);
        assert_eq!(checked["wildcard"], false);
        assert_eq!(checked["checks"].as_array().unwrap().len(), 2);
        for (i, root) in [(&local, false), (&input, true)] {
            let timed = run(
                i,
                root,
                Arc::new(|_, _| {
                    thread::sleep(Duration::from_millis(100));
                    Ok(Default::default())
                }),
            );
            assert!(timed["checks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["status"] == "timeout"));
        }
        let cancel = Cancellation::default();
        cancel.cancel();
        assert_eq!(
            dns_with(
                &local,
                false,
                &cancel,
                Duration::from_millis(10),
                Arc::new(|_, _| Ok(Default::default()))
            )
            .unwrap()["passed"],
            false
        );
        for domain in [
            "bad;command",
            "127.0.0.1",
            "wrong..internal",
            "UPPER.internal",
            &format!("{}.internal", "x".repeat(64)),
        ] {
            local["domain"] = json!(domain);
            assert!(dns(&local, false, &Cancellation::default()).is_err());
        }
    }
}

#[cfg(test)]
mod probe_contracts {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    };
    #[test]
    fn offline_and_limited_network_results_never_include_configuration_or_private_diagnostics() {
        let result = probe_with(true, &Cancellation::default(), &|_, _, _, _| {
            panic!("offline network request")
        })
        .unwrap();
        assert_eq!(result["network"], json!({"status":"skipped","checks":[]}));
        assert!(!result["machine"]["os"]["name"].as_str().unwrap().is_empty());
        assert!(!result["machine"]["hardware"]["architecture"]
            .as_str()
            .unwrap()
            .is_empty());
        assert!(result["machine"]["hardware"]["cores"].as_u64().unwrap() > 0);
        assert!(result["machine"]["hardware"]["memoryGiB"].as_f64().unwrap() > 0.0);
        let probe = EnvironmentProbe::default();
        let calls = AtomicUsize::new(0);
        let scan = |cancel: &Cancellation| {
            probe_with(false, cancel, &|name, host, notfound, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                let status = match host {
                    "www.google.com" => "timeout",
                    "api.github.com" => classify_network_error("enotfound sensitive proxy diagnostic"),
                    _ => "reachable",
                };
                json!({"name":name,"host":host,"status":status,"httpStatus":if notfound{404}else{200}})
            })
        };
        let result = probe.run_with(false, false, scan).unwrap();
        assert_eq!(result["network"]["status"], "limited");
        assert_eq!(
            result["network"]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["status"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["reachable", "timeout", "dns-error", "reachable"]
        );
        assert_eq!(result["network"]["checks"][1]["host"], "www.google.com");
        assert_eq!(result["network"]["checks"][3]["httpStatus"], 404);
        assert!(!result.to_string().contains("sensitive"));
        probe.run_with(false, false, scan).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        probe.run_with(false, true, scan).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 8);
        let result=probe_with(false,&Cancellation::default(),&|name,host,_,_|json!({"name":name,"host":host,"status":if host=="api.github.com"{classify_network_error("cert_has_expired not-for-browser")}else{"http-error"},"httpStatus":403})).unwrap();
        assert_eq!(result["network"]["status"], "unreachable");
        assert_eq!(result["network"]["checks"][2]["status"], "tls-error");
        assert_eq!(result["network"]["checks"][0]["httpStatus"], 403);
        assert!(!result.to_string().contains("not-for-browser"));
    }
    #[test]
    fn concurrent_clients_share_scan_and_offline_cancels_all_checks_without_reusing_online_cache() {
        let probe = Arc::new(EnvironmentProbe::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicUsize::new(0));
        let (started, receive) = mpsc::channel();
        thread::scope(|scope| {
            let owner = probe.clone();
            let calls = calls.clone();
            let cancelled = cancelled.clone();
            let first = scope.spawn(move || {
                owner
                    .run_with(false, false, |cancel| {
                        probe_with(false, cancel, &|_, _, _, cancel| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            started.send(()).unwrap();
                            while cancel.check().is_ok() {
                                thread::sleep(Duration::from_millis(1));
                            }
                            cancelled.fetch_add(1, Ordering::SeqCst);
                            json!({"status":"cancelled"})
                        })
                    })
                    .unwrap()
            });
            for _ in 0..4 {
                receive.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            let second = scope.spawn(|| {
                probe
                    .run_with(false, true, |_| panic!("duplicate concurrent scan"))
                    .unwrap()
            });
            // Wait until the second request holds a reference to the active job.
            let start = Instant::now();
            while probe
                .0
                .lock()
                .unwrap()
                .active
                .as_ref()
                .map(|(_, job)| Arc::strong_count(job))
                .unwrap_or(0)
                < 3
            {
                assert!(start.elapsed() < Duration::from_secs(2));
                thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(probe.run(true, false).unwrap()["network"]["status"], "skipped");
            assert_eq!(first.join().unwrap()["network"]["status"], "cancelled");
            assert_eq!(second.join().unwrap()["network"]["status"], "cancelled");
        });
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(cancelled.load(Ordering::SeqCst), 4);
        let result = probe
            .run_with(false, false, |cancel| {
                probe_with(false, cancel, &|_, _, _, _| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    json!({"status":"timeout"})
                })
            })
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 8);
        assert_eq!(result["network"]["status"], "unreachable");
        probe.cancel();
        assert!(probe.0.lock().unwrap().cache.is_none());
    }
}
