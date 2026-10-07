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
    os == "ubuntu"
        && match architecture(arch) {
            "amd64" => config::matches(r"^22\.04(?:\.[0-9]+)?$", version),
            "arm64" => config::matches(r"^24\.04(?:\.[0-9]+)?$", version),
            _ => false,
        }
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
        cpu = if names.is_empty() {
            let data: Value = serde_json::from_str(&command("lscpu", &["--json"])).unwrap_or(Value::Null);
            data["lscpu"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter(|r| r["field"] == "Model name:")
                        .filter_map(|r| r["data"].as_str())
                        .collect::<Vec<_>>()
                        .join(" + ")
                })
                .unwrap_or_default()
        } else {
            names.join(" + ")
        };
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
pub fn probe(offline: bool, cancel: &Cancellation) -> Result<Value> {
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
            .map(|(name, host, notfound)| {
                scope.spawn(move || {
                    let start = Instant::now();
                    let result = (|| {
                        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
                            .map_err(|_| fail("unreachable", 400))?;
                        runtime.block_on(async {
                            let client = reqwest::Client::builder().timeout(Duration::from_secs(4))
                                .redirect(reqwest::redirect::Policy::none()).build()
                                .map_err(|_| fail("unreachable", 400))?;
                            tokio::select! {
                                _ = cancel.cancelled() => Err(fail("cancelled", 400)),
                                response = client.head(format!("https://{host}/"))
                                    .header("User-Agent", "apeiron-cli-connectivity").send() =>
                                    response.map_err(|e| fail(if e.is_timeout() { "timeout" } else { "unreachable" }, 400)),
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
                                check["status"] =
                                    json!(if (200..400).contains(&status) || (notfound && status == 404) {
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
                })
            })
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
pub fn dns(input: &Value, include_root: bool, cancel: &Cancellation) -> Result<Value> {
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
                scope.spawn(move || {
                    let (send, receive) = mpsc::channel();
                    let name = host.clone();
                    thread::spawn(move || {
                        let addresses = lookup(&name, !local);
                        let _ = send.send(addresses);
                    });
                    let start = Instant::now();
                    let (addresses, status) = loop {
                        if cancel.check().is_err() || start.elapsed() > Duration::from_secs(4) {
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
