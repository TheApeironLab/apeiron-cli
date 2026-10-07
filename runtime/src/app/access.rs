use super::process::{self, Cancellation};
use super::*;
use base64::Engine;
#[cfg(target_os = "macos")]
use std::fs;
use std::{
    process::Command,
    sync::{Arc, Mutex},
    thread,
};
pub fn public_ca(pem: &str) -> Result<Value> {
    if !config::matches(
        r"^\s*-----BEGIN CERTIFICATE-----\s+[A-Za-z0-9+/=\s]+-----END CERTIFICATE-----\s*$",
        pem,
    ) {
        return Err(fail("需要单一公开 CA 证书。", 400));
    }
    let (_, parsed) =
        x509_parser::pem::parse_x509_pem(pem.trim().as_bytes()).map_err(|_| fail("CA 证书格式无效。", 400))?;
    let cert = parsed.parse_x509().map_err(|_| fail("CA 证书格式无效。", 400))?;
    if !cert.is_ca() || !cert.validity().is_valid() {
        return Err(fail("CA 证书尚未生效或已过期。", 400));
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(&parsed.contents);
    let body = encoded
        .as_bytes()
        .chunks(64)
        .map(|b| std::str::from_utf8(b).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let fingerprint = config::hash(&parsed.contents)
        .as_bytes()
        .chunks(2)
        .map(|b| std::str::from_utf8(b).unwrap().to_uppercase())
        .collect::<Vec<_>>()
        .join(":");
    let expires = chrono::DateTime::from_timestamp(cert.validity().not_after.timestamp(), 0)
        .ok_or_else(|| fail("CA 到期时间无效。", 400))?
        .to_rfc3339();
    Ok(
        json!({"pem":format!("-----BEGIN CERTIFICATE-----\n{body}\n-----END CERTIFICATE-----\n"),"fingerprint":fingerprint,"expiresAt":expires}),
    )
}
pub fn collect(target: &Value, dir: &Path, cancel: &Cancellation) -> Result<Value> {
    let i = &target["installation"];
    if i["publicAccess"].is_object() {
        return Ok(
            json!({"info":{"domain":i["domain"],"entryIp":i["publicAccess"]["publicIp"],"local":false,"public":true,"httpsPort":443,"notes":["公网 DNS 与 HTTPS 已通过检查，证书由 Caddy 自动续期。"]}}),
        );
    }
    let mut result = json!({"info":{"domain":i["domain"],"entryIp":i["entryIp"],"local":i["topology"]=="single-k3d","httpsPort":i["httpsPort"],"notes":[]}});
    let ca = (|| {
        let raw = resources::read(
            &Path::new(target["workDir"].as_str().unwrap()).join("state/chentu-ca.crt"),
            32768,
        )?;
        let ca = public_ca(std::str::from_utf8(&raw).map_err(|_| fail("无效 CA。", 400))?)?;
        let path = dir.join("chentu-ca.crt");
        private_write(&path, ca["pem"].as_str().unwrap().as_bytes())?;
        Ok::<_, Error>((path, ca))
    })();
    match ca {
        Ok((path, ca)) => {
            result["ca"] = ca["pem"].clone();
            result["info"]["ca"] = json!({"path":path,"fingerprint":ca["fingerprint"],"expiresAt":ca["expiresAt"]});
        }
        Err(_) => result["info"]["notes"]
            .as_array_mut()
            .unwrap()
            .push(json!("尚未读取到有效的公开 CA 证书，请检查 ingress 初始化与安装日志。")),
    }
    let hosts = (|| {
        let root = Path::new(target["root"].as_str().unwrap());
        let domain = i["domain"].as_str().unwrap();
        let ip = i["entryIp"].as_str().unwrap();
        let out = process::text(
            Command::new("python3")
                .args(["-m", "chentu.lab.ingresshosts", &root.to_string_lossy(), domain, ip])
                .current_dir(root)
                .env("PYTHONPATH", root.join("cli/src"))
                .env("PYTHONDONTWRITEBYTECODE", "1"),
            b"",
            10,
            65536,
            cancel,
        )?;
        let suffix = format!(".{domain}:{ip}");
        let mut names = std::collections::BTreeSet::new();
        for pin in out.split_whitespace() {
            let name = pin
                .strip_suffix(&suffix)
                .filter(|s| config::safe_name(s))
                .ok_or_else(|| fail("主机名清单无效。", 400))?;
            names.insert(format!("{name}.{domain}"));
        }
        if !names.contains(&format!("apeiron.{domain}")) || !names.contains(&format!("iam.{domain}")) {
            return Err(fail("主机名清单缺少必需入口。", 400));
        }
        let text = format!(
            "# Apeiron {domain}\n# Add these entries to this workstation's hosts file. Do not replace the file.\n{}",
            names.iter().map(|n| format!("{ip} {n}\n")).collect::<String>()
        );
        private_write(&dir.join("apeiron-hosts.txt"), text.as_bytes())?;
        Ok::<_, Error>(text)
    })();
    match hosts {
        Ok(hosts) => {
            result["hosts"] = json!(hosts);
            result["info"]["hostsPath"] = json!(dir.join("apeiron-hosts.txt"));
        }
        Err(_) => result["info"]["notes"]
            .as_array_mut()
            .unwrap()
            .push(json!("完整 hosts 清单未生成，请检查发行包中的主机名清单工具。")),
    }
    cancel.check()?;
    Ok(result)
}
pub fn credentials(target: &Value, cancel: &Cancellation) -> Result<Value> {
    let result = (|| {
        let args = [
            "--request-timeout=8s",
            "-n",
            "keycloak",
            "get",
            "secret",
            "keycloak-bootstrap",
            "-o",
            "json",
        ];
        let mut cmd;
        if target["runner"] == "native" {
            cmd = Command::new("kubectl");
            cmd.args(["--kubeconfig", target["kubeconfig"].as_str().unwrap()]);
        } else {
            let work = Path::new(target["workDir"].as_str().unwrap());
            let marker = resources::json_file(&work.join("installation.json"), 8192)?;
            let cluster = marker["cluster"].as_str().unwrap_or("");
            let image = target["image"].as_str().unwrap_or("");
            if marker["domain"] != target["installation"]["domain"]
                || !config::safe_name(cluster)
                || !config::matches(r"^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*$", image)
            {
                return Err(fail("部署标记不匹配。", 400));
            }
            cmd = Command::new("docker");
            cmd.args([
                "run",
                "--rm",
                "--pull=never",
                "--network",
                &format!("k3d-{cluster}"),
                "--entrypoint",
                "kubectl",
                "-v",
                &format!("{}:/kubeconfig:ro", work.join("state/kubeconfig").display()),
                image,
                "--kubeconfig",
                "/kubeconfig",
            ]);
        }
        cmd.args(args);
        let raw = process::capture(&mut cmd, b"", Duration::from_secs(20), 65536, cancel)?;
        let value: Value = serde_json::from_slice(&raw).map_err(|_| fail("凭据响应无效。", 400))?;
        if value["metadata"]["name"] != "keycloak-bootstrap" || value["metadata"]["namespace"] != "keycloak" {
            return Err(fail("凭据响应无效。", 400));
        }
        let mut output = json!({});
        for (key, max) in [("username", 256), ("password", 4096)] {
            let encoded = value["data"][key]
                .as_str()
                .filter(|s| s.len() <= max * 2)
                .ok_or_else(|| fail("凭据响应无效。", 400))?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| fail("凭据响应无效。", 400))?;
            let text = String::from_utf8(bytes).map_err(|_| fail("凭据响应无效。", 400))?;
            if text.is_empty() || text.len() > max || text.chars().any(char::is_control) {
                return Err(fail("凭据响应无效。", 400));
            }
            output[key] = json!(text);
        }
        Ok(output)
    })();
    result.map_err(|_| fail("暂时无法读取初始管理员凭据，请确认集群可连接后重试。", 400))
}
pub fn verify(info: &Value, cancel: &Cancellation) -> Result<Value> {
    let domain = info["domain"].as_str().unwrap_or("");
    let ip = info["entryIp"].as_str().unwrap_or("");
    let port = info["httpsPort"].as_u64().unwrap_or(443);
    if !config::safe_domain(domain) || !config::safe_ip(ip) || !(1..=65535).contains(&port) {
        return Err(fail("部署访问地址无效。", 400));
    }
    let mut checks = Vec::new();
    for (id, name) in [("apeiron", "Apeiron"), ("ops", "Apeiron Ops"), ("iam", "IAM")] {
        cancel.check()?;
        let host = format!("{id}.{domain}");
        let url = format!(
            "https://{host}{}/",
            if port == 443 { String::new() } else { format!(":{port}") }
        );
        let mut check = json!({"name":name,"host":host,"url":url,"dns":"failed","https":"skipped","message":"解析失败或地址与部署入口不一致，请检查 hosts / DNS。"});
        let (sender, receiver) = std::sync::mpsc::channel();
        let host_copy = host.clone();
        let dns_only = info["public"] == true;
        thread::spawn(move || {
            let _ = sender.send(discovery::lookup(&host_copy, dns_only));
        });
        if receiver
            .recv_timeout(Duration::from_secs(4))
            .is_ok_and(|r| r.is_ok_and(|a| !a.is_empty() && a.iter().all(|s| s == ip)))
        {
            check["dns"] = json!("passed");
            let command = if cfg!(target_os = "macos") {
                "/usr/bin/curl"
            } else {
                "curl"
            };
            let output = process::text(
                Command::new(command)
                    .env_clear()
                    .env("PATH", std::env::var("PATH").unwrap_or_default())
                    .args([
                        "--disable",
                        "--silent",
                        "--show-error",
                        "--head",
                        "--noproxy",
                        "*",
                        "--max-time",
                        "10",
                        "--proto",
                        "=https",
                        "--resolve",
                        &format!("{host}:{port}:{ip}"),
                        "--output",
                        "/dev/null",
                        "--write-out",
                        "%{http_code}",
                        &url,
                    ]),
                b"",
                12,
                4096,
                cancel,
            );
            let code = output.ok().and_then(|s| s.trim().parse::<u16>().ok());
            check["https"] = json!(if code.is_some_and(|n| (200..400).contains(&n)) {
                "passed"
            } else {
                "failed"
            });
            check["message"] = json!(if check["https"] == "passed" {
                "HTTPS 可访问。"
            } else {
                "HTTPS 检查失败，请检查 CA 信任和应用状态。"
            });
            if let Some(code) = code {
                check["httpStatus"] = json!(code);
            }
        }
        checks.push(check);
    }
    cancel.check()?;
    Ok(
        json!({"checkedFrom":discovery::host()["name"],"checkedAt":chrono::Utc::now().to_rfc3339(),"passed":checks.iter().all(|c|c["dns"]=="passed"&&c["https"]=="passed"),"checks":checks}),
    )
}
pub fn capability() -> Value {
    #[cfg(not(target_os = "macos"))]
    let available = false;
    #[cfg(target_os = "macos")]
    let available = {
        use std::os::unix::fs::MetadataExt;
        std::env::var_os("SSH_CONNECTION").is_none()
            && std::env::var_os("SSH_TTY").is_none()
            && fs::metadata("/dev/console").is_ok_and(|m| m.uid() == unsafe { libc::getuid() })
    };
    json!({"available":available,"host":discovery::host()["name"],"reason":if available{""}else{"一键安装需要在有桌面会话的 Mac 上运行 CLI。请按手动指引配置。"}})
}
fn hostnames(a: &Value) -> Result<Vec<String>> {
    let info = &a["info"];
    let domain = info["domain"].as_str().unwrap_or("");
    let ip = info["entryIp"].as_str().unwrap_or("");
    if !config::safe_domain(domain) || !config::safe_ip(ip) || (info["local"] == true && ip != "127.0.0.1") {
        return Err(fail("部署访问地址无效。", 400));
    }
    let ca = public_ca(a["ca"].as_str().unwrap_or(""))?;
    if ca["fingerprint"] != info["ca"]["fingerprint"] {
        return Err(fail("部署 CA 指纹不匹配。", 400));
    }
    let hosts = a["hosts"]
        .as_str()
        .filter(|s| s.len() <= 65536)
        .ok_or_else(|| fail("缺少完整 CA 和 hosts 文件。", 400))?;
    let mut names = std::collections::BTreeSet::new();
    for line in hosts
        .lines()
        .filter(|s| !s.trim().is_empty() && !s.trim().starts_with('#'))
    {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 2
            || parts[0] != ip
            || !config::safe_domain(parts[1])
            || !parts[1].ends_with(&format!(".{domain}"))
        {
            return Err(fail("部署 hosts 清单无效。", 400));
        }
        names.insert(parts[1].to_owned());
    }
    if !names.contains(&format!("apeiron.{domain}")) || !names.contains(&format!("iam.{domain}")) {
        return Err(fail("hosts 清单缺少必需入口。", 400));
    }
    Ok(names.into_iter().collect())
}
fn merge_hosts(current: &str, a: &Value) -> Result<String> {
    if current.len() > 1048576 || current.contains('\0') {
        return Err(fail("本机 hosts 文件格式异常。", 400));
    }
    let names = hostnames(a)?;
    let domain = a["info"]["domain"].as_str().unwrap();
    let ip = a["info"]["entryIp"].as_str().unwrap();
    let begin = format!("# BEGIN APEIRON {domain}");
    let end = format!("# END APEIRON {domain}");
    let mut keep = Vec::new();
    let (mut inside, mut seen) = (false, false);
    for line in current.split('\n') {
        let clean = line.trim_end_matches('\r');
        if clean == begin {
            if seen || inside {
                return Err(fail("hosts 配置标记重复。", 400));
            }
            seen = true;
            inside = true;
            continue;
        }
        if clean == end {
            if !inside {
                return Err(fail("hosts 配置标记不完整。", 400));
            }
            inside = false;
            continue;
        }
        let parts = clean.split('#').next().unwrap().split_whitespace().collect::<Vec<_>>();
        if inside {
            if parts
                .iter()
                .skip(1)
                .any(|n| !n.to_lowercase().ends_with(&format!(".{domain}")))
            {
                return Err(fail("hosts 区块包含其他域名。", 400));
            }
            continue;
        }
        if parts.first().is_some_and(|address| *address != ip)
            && parts.iter().skip(1).any(|n| names.contains(&n.to_lowercase()))
        {
            return Err(fail("hosts 已有指向其他 IP 的平台域名，请先检查冲突。", 400));
        }
        keep.push(line);
    }
    if inside {
        return Err(fail("hosts 配置标记不完整。", 400));
    }
    while keep.last() == Some(&"") {
        keep.pop();
    }
    Ok(format!(
        "{}\n\n{begin}\n{}{end}\n",
        keep.join("\n"),
        names.iter().map(|name| format!("{ip} {name}\n")).collect::<String>()
    ))
}
#[derive(Clone)]
pub struct LocalAccess {
    status: Arc<Mutex<Value>>,
    task: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}
impl LocalAccess {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(json!({"phase":"idle","message":""}))),
            task: Arc::new(Mutex::new(None)),
        }
    }
    pub fn snapshot(&self) -> Value {
        self.status.lock().unwrap().clone()
    }
    pub fn active(&self) -> bool {
        self.snapshot()["phase"] == "installing"
    }
    pub fn reset(&self) {
        if !self.active() {
            *self.status.lock().unwrap() = json!({"phase":"idle","message":""});
        }
    }
    pub fn wait(&self) {
        if let Some(t) = self.task.lock().unwrap().take() {
            let _ = t.join();
        }
    }
    pub fn start(&self, a: Value) -> Result<()> {
        if self.active() {
            return Err(fail("本机访问配置正在进行。", 409));
        }
        hostnames(&a)?;
        if capability()["available"] != true {
            return Err(fail("一键安装需要桌面 Mac，请使用手动指引。", 400));
        }
        self.wait();
        *self.status.lock().unwrap() =
            json!({"phase":"installing","message":"请在 macOS 系统弹窗中授权配置 hosts 与 CA 信任。"});
        let instance = self.clone();
        *self.task.lock().unwrap() = Some(thread::spawn(move || {
            let result = instance.install(&a);
            if let Err(e) = result {
                *instance.status.lock().unwrap() = json!({"phase":"failed","message":e.message});
            }
        }));
        Ok(())
    }
    fn install(&self, a: &Value) -> Result<()> {
        let current = String::from_utf8(resources::read(Path::new("/private/etc/hosts"), 1048576)?)
            .map_err(|_| fail("hosts 文件格式异常。", 400))?;
        let merged = merge_hosts(&current, a)?;
        let ca = public_ca(a["ca"].as_str().unwrap())?;
        let delimiter = format!("APEIRON_{:016x}{:016x}", rand::random::<u64>(), rand::random::<u64>());
        let script = include_str!("../../assets/mac-access.sh")
            .replace("APEIRON_HASH_PLACEHOLDER", &config::hash(current.as_bytes()))
            .replace("APEIRON_DELIMITER_PLACEHOLDER", &delimiter)
            .replace("APEIRON_HOSTS_PLACEHOLDER", &merged)
            .replace("APEIRON_CA_PLACEHOLDER", ca["pem"].as_str().unwrap());
        let apple=format!("do shell script {} with administrator privileges with prompt \"Apeiron 将配置本机 hosts，并在系统钥匙串中信任部署 CA（HTTPS）。\"",serde_json::to_string(&script).unwrap());
        let out = process::text(
            Command::new("/usr/bin/osascript").args(["-e", &apple]),
            b"",
            180,
            8192,
            &Cancellation::default(),
        )
        .map_err(|_| {
            fail(
                "本机访问配置未完成，请检查系统授权。部分配置可能已安装，重试会合并现有记录。",
                400,
            )
        })?;
        let backup = regex::Regex::new(r"APEIRON_BACKUP=(/private/etc/apeiron-access\.[A-Za-z0-9]+/hosts\.before)")
            .unwrap()
            .captures(&out)
            .map(|c| c[1].to_owned());
        let verification = verify(&a["info"], &Cancellation::default())?;
        let checks = verification["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["name"] != "Apeiron Ops")
            .map(|v| json!({"host":v["host"],"passed":v["dns"]=="passed"&&v["https"]=="passed"}))
            .collect::<Vec<_>>();
        let passed = checks.iter().all(|c| c["passed"] == true);
        *self.status.lock().unwrap() = json!({"phase":if passed{"succeeded"}else{"failed"},"message":if passed{"本机解析与 HTTPS 检查通过。"}else{"配置已安装，但访问检查未全部通过。"},"backup":backup,"checks":checks});
        Ok(())
    }
}
