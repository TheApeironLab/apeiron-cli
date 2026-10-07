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
        let mut cmd = credential_command(target)?;
        let raw = process::capture(&mut cmd, b"", Duration::from_secs(20), 65536, cancel)?;
        decode_credentials(&raw)
    })();
    result.map_err(|_| fail("暂时无法读取初始管理员凭据，请确认集群可连接后重试。", 400))
}
fn credential_command(target: &Value) -> Result<Command> {
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
    Ok(cmd)
}
fn decode_credentials(raw: &[u8]) -> Result<Value> {
    let value: Value = serde_json::from_slice(raw).map_err(|_| fail("凭据响应无效。", 400))?;
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
}
pub fn verify(info: &Value, cancel: &Cancellation) -> Result<Value> {
    verify_with(
        info,
        cancel,
        |host, dns_only| {
            let (sender, receiver) = std::sync::mpsc::channel();
            let host = host.to_owned();
            thread::spawn(move || {
                let _ = sender.send(discovery::lookup(&host, dns_only));
            });
            receiver
                .recv_timeout(Duration::from_secs(4))
                .map_err(std::io::Error::other)?
        },
        |host, ip, port, url, cancel| {
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
                        url,
                    ]),
                b"",
                12,
                4096,
                cancel,
            );
            output?.trim().parse::<u16>().map_err(|_| fail("HTTPS 响应无效。", 400))
        },
    )
}
fn verify_with(
    info: &Value,
    cancel: &Cancellation,
    resolve: impl Fn(&str, bool) -> std::io::Result<std::collections::BTreeSet<String>>,
    https: impl Fn(&str, &str, u16, &str, &Cancellation) -> Result<u16>,
) -> Result<Value> {
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
        if resolve(&host, info["public"] == true).is_ok_and(|a| !a.is_empty() && a.iter().all(|s| s == ip)) {
            check["dns"] = json!("passed");
            let code = https(&host, ip, port as u16, &url, cancel).ok();
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
fn mac_script(current: &str, a: &Value) -> Result<String> {
    let merged = merge_hosts(current, a)?;
    let ca = public_ca(a["ca"].as_str().unwrap())?;
    let delimiter = format!("APEIRON_{:016x}{:016x}", rand::random::<u64>(), rand::random::<u64>());
    let script = include_str!("../../assets/mac-access.sh")
        .replace("APEIRON_HASH_PLACEHOLDER", &config::hash(current.as_bytes()))
        .replace("APEIRON_DELIMITER_PLACEHOLDER", &delimiter)
        .replace("APEIRON_HOSTS_PLACEHOLDER", &merged)
        .replace("APEIRON_CA_PLACEHOLDER", ca["pem"].as_str().unwrap());
    Ok(script)
}
fn authorization_script(script: &str) -> String {
    format!("try\nreturn do shell script {} with administrator privileges with prompt \"Apeiron 将配置本机 hosts，并在系统钥匙串中信任部署 CA（HTTPS）。\"\non error message number code\nif code is -128 then return \"APEIRON_AUTH_CANCELLED\"\nerror message number code\nend try", serde_json::to_string(script).unwrap())
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
        self.start_with(a, capability(), Self::install)
    }
    pub(super) fn start_with(
        &self,
        a: Value,
        capability: Value,
        install: impl FnOnce(&Value) -> Result<Value> + Send + 'static,
    ) -> Result<()> {
        if self.active() {
            return Err(fail("本机访问配置正在进行。", 409));
        }
        hostnames(&a)?;
        if capability["available"] != true {
            return Err(fail("一键安装需要桌面 Mac，请使用手动指引。", 400));
        }
        self.wait();
        *self.status.lock().unwrap() =
            json!({"phase":"installing","message":"请在 macOS 系统弹窗中授权配置 hosts 与 CA 信任。"});
        let instance = self.clone();
        *self.task.lock().unwrap() = Some(thread::spawn(move || {
            *instance.status.lock().unwrap() = match install(&a) {
                Ok(status) => status,
                Err(e) => json!({"phase":if e.code == 499 {"cancelled"} else {"failed"},"message":e.message}),
            };
        }));
        Ok(())
    }
    fn install(a: &Value) -> Result<Value> {
        let current = String::from_utf8(resources::read(Path::new("/private/etc/hosts"), 1048576)?)
            .map_err(|_| fail("hosts 文件格式异常。", 400))?;
        let script = mac_script(&current, a)?;
        let apple = authorization_script(&script);
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
        if out.trim() == "APEIRON_AUTH_CANCELLED" {
            return Err(fail("已取消系统授权。", 499));
        }
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
        Ok(
            json!({"phase":if passed{"succeeded"}else{"failed"},"message":if passed{"本机解析与 HTTPS 检查通过。"}else{"配置已安装，但访问检查未全部通过。"},"backup":backup,"checks":checks}),
        )
    }
}

#[cfg(test)]
pub(super) mod contracts {
    use super::*;
    use crate::app::contracts::Temp;
    #[test]
    fn credential_decoder_projects_only_expected_secret_and_rejects_malformed_output() {
        let encode = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let credentials = json!({"username":"fixture-admin","password":"not-a-real-password<&\""});
        let secret = json!({"metadata":{"name":"keycloak-bootstrap","namespace":"keycloak"},"data":{"username":encode("fixture-admin"),"password":encode("not-a-real-password<&\""),"extra":encode("must-not-return")}});
        assert_eq!(decode_credentials(secret.to_string().as_bytes()).unwrap(), credentials);
        let mut wrong_namespace = secret.clone();
        wrong_namespace["metadata"]["namespace"] = json!("elsewhere");
        let mut invalid_base64 = secret.clone();
        invalid_base64["data"]["password"] = json!("invalid base64");
        let mut control = secret.clone();
        control["data"]["username"] = json!(encode("bad\nuser"));
        for bad in [
            credentials["password"].to_string(),
            "{}".into(),
            wrong_namespace.to_string(),
            invalid_base64.to_string(),
            control.to_string(),
        ] {
            let e = decode_credentials(bad.as_bytes()).unwrap_err();
            assert!(!e.message.contains("not-a-real-password"));
        }
    }
    #[test]
    fn credential_command_pins_secret_and_kubeconfig_without_socket_writes_or_credentials() {
        let temp = Temp::new();
        temp.write(
            "installation.json",
            json!({"cluster":"apeiron-fixture","domain":"example.internal"}).to_string(),
        );
        let target = json!({"runner":"docker","workDir":temp.0,"image":"fixture-toolbox","installation":{"domain":"example.internal"}});
        let cmd = credential_command(&target).unwrap();
        assert_eq!(cmd.get_program(), "docker");
        let args = cmd.get_args().map(|s| s.to_str().unwrap()).collect::<Vec<_>>();
        for value in [
            "--pull=never",
            "k3d-apeiron-fixture",
            &format!("{}:/kubeconfig:ro", temp.0.join("state/kubeconfig").display()),
        ] {
            assert!(args.contains(&value));
        }
        assert_eq!(
            &args[args.len() - 7..],
            ["-n", "keycloak", "get", "secret", "keycloak-bootstrap", "-o", "json"]
        );
        assert!(!args.join(" ").contains("docker.sock"));
        assert!(!args.join(" ").contains("password"));
        temp.write(
            "installation.json",
            json!({"cluster":"apeiron-fixture","domain":"wrong.internal"}).to_string(),
        );
        assert!(credential_command(&target).is_err());
        let cmd = credential_command(&json!({"runner":"native","kubeconfig":"/generated/kubeconfig"})).unwrap();
        assert_eq!(cmd.get_program(), "kubectl");
        assert_eq!(
            cmd.get_args().take(2).collect::<Vec<_>>(),
            ["--kubeconfig", "/generated/kubeconfig"]
        );
    }
    pub fn artifacts(temp: &Temp) -> Value {
        let cert = temp.0.join("fixture.crt");
        assert!(Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:prime256v1",
                "-nodes",
                "-days",
                "1",
                "-subj",
                "/CN=fixture",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
                "-keyout"
            ])
            .arg(temp.0.join("fixture.key"))
            .arg("-out")
            .arg(&cert)
            .output()
            .unwrap()
            .status
            .success());
        let ca = public_ca(&std::fs::read_to_string(&cert).unwrap()).unwrap();
        json!({"info":{"domain":"team.internal","entryIp":"127.0.0.1","local":true,"ca":ca},"ca":ca["pem"],"hosts":"# generated\n127.0.0.1 apeiron.team.internal\n127.0.0.1 iam.team.internal\n127.0.0.1 task.team.internal\n"})
    }
    #[test]
    fn public_ca_and_host_merge_reject_private_keys_leaf_certificates_symlinks_and_conflicts() {
        let temp = Temp::new();
        let a = artifacts(&temp);
        let pem = a["ca"].as_str().unwrap();
        let key = std::fs::read_to_string(temp.0.join("fixture.key")).unwrap();
        assert!(config::matches(
            r"^(?:[A-F0-9]{2}:){31}[A-F0-9]{2}$",
            a["info"]["ca"]["fingerprint"].as_str().unwrap()
        ));
        for invalid in [
            key.clone(),
            format!("{pem}{key}"),
            format!("{pem}{pem}"),
            "invalid".into(),
        ] {
            assert!(public_ca(&invalid).is_err());
        }
        let before="## system\n127.0.0.1 localhost localalias # do not remove\n::1 localhost\n192.0.2.2 unrelated.internal\n# BEGIN APEIRON other.internal\n127.0.0.1 apeiron.other.internal\n# END APEIRON other.internal\n";
        let first = merge_hosts(before, &a).unwrap();
        assert!(first.contains(before));
        assert!(first.contains("# BEGIN APEIRON team.internal\n"));
        assert_eq!(merge_hosts(&first, &a).unwrap(), first);
        assert_eq!(first.matches("127.0.0.1 apeiron.team.internal").count(), 1);
        assert_eq!(
            merge_hosts(&first.replace("task.team.internal", "removed.team.internal"), &a).unwrap(),
            first
        );
        for malformed in [
            "192.0.2.7 custom APEIRON.TEAM.INTERNAL # conflict\n",
            "::1 apeiron.team.internal\n",
            "# BEGIN APEIRON team.internal\n",
            "# END APEIRON team.internal\n",
            &format!("{first}{first}"),
            "# BEGIN APEIRON team.internal\n192.0.2.9 unrelated.internal\n# END APEIRON team.internal\n",
        ] {
            assert!(merge_hosts(malformed, &a).is_err());
        }
        for hosts in [
            "127.0.0.1 unrelated.internal\n".into(),
            format!("{}127.0.0.1 foo.team.internal extra\n", a["hosts"].as_str().unwrap()),
            format!("{}192.0.2.1 bad.team.internal\n", a["hosts"].as_str().unwrap()),
        ] {
            let mut invalid = a.clone();
            invalid["hosts"] = json!(hosts);
            assert!(merge_hosts("", &invalid).is_err());
        }
        let mut invalid = a.clone();
        invalid["info"]["ca"]["fingerprint"] = json!("wrong");
        assert!(merge_hosts("", &invalid).unwrap_err().message.contains("指纹"));
        invalid = a.clone();
        invalid["info"]["domain"] = json!("foo;touch /tmp/x");
        assert!(merge_hosts("", &invalid).is_err());
        invalid = a.clone();
        invalid["ca"] = json!(format!("{pem}\n-----BEGIN PRIVATE KEY-----\n"));
        assert!(merge_hosts("", &invalid).is_err());
        let leaf = temp.0.join("leaf.crt");
        assert!(Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:prime256v1",
                "-nodes",
                "-days",
                "1",
                "-subj",
                "/CN=leaf",
                "-addext",
                "basicConstraints=critical,CA:FALSE",
                "-keyout"
            ])
            .arg(temp.0.join("leaf.key"))
            .arg("-out")
            .arg(&leaf)
            .output()
            .unwrap()
            .status
            .success());
        assert!(public_ca(&std::fs::read_to_string(leaf).unwrap()).is_err());
        #[cfg(unix)]
        {
            std::fs::create_dir_all(temp.0.join("state")).unwrap();
            std::os::unix::fs::symlink(temp.0.join("fixture.key"), temp.0.join("state/chentu-ca.crt")).unwrap();
            let result=collect(&json!({"workDir":temp.0,"root":temp.0,"installation":{"domain":"example.internal","entryIp":"127.0.0.1","topology":"single-k3d"}}),&temp.0,&Cancellation::default()).unwrap();
            assert!(result["ca"].is_null());
            assert!(result["info"]["ca"].is_null());
            assert!(!result.to_string().contains(&key));
        }
    }
    #[test]
    fn verification_requires_exact_dns_and_valid_https_and_preserves_public_ports() {
        use std::cell::RefCell;
        let input = json!({"domain":"example.internal","entryIp":"127.0.0.1"});
        let contacted = RefCell::new(Vec::new());
        let good = |_: &str, _: bool| Ok(std::collections::BTreeSet::from(["127.0.0.1".into()]));
        let https = |host: &str, ip: &str, port: u16, _: &str, _: &Cancellation| {
            assert_eq!(ip, "127.0.0.1");
            assert_eq!(port, 443);
            contacted.borrow_mut().push(host.to_owned());
            Ok(302)
        };
        assert_eq!(
            verify_with(&input, &Cancellation::default(), good, https).unwrap()["passed"],
            true
        );
        assert_eq!(
            *contacted.borrow(),
            [
                "apeiron.example.internal",
                "ops.example.internal",
                "iam.example.internal"
            ]
        );
        let bad = verify_with(
            &input,
            &Cancellation::default(),
            |_, _| {
                Ok(std::collections::BTreeSet::from([
                    "127.0.0.1".into(),
                    "192.0.2.1".into(),
                ]))
            },
            https,
        )
        .unwrap();
        assert_eq!(bad["passed"], false);
        assert!(bad["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["https"] == "skipped"));
        assert_eq!(contacted.borrow().len(), 3);
        let bad = verify_with(
            &input,
            &Cancellation::default(),
            |h, _| {
                if h.starts_with("iam.") {
                    Err(std::io::Error::other("private DNS diagnostic"))
                } else {
                    good(h, false)
                }
            },
            |h, _, _, _, _| {
                if h.starts_with("apeiron.") {
                    Err(fail("private diagnostic", 400))
                } else {
                    Ok(404)
                }
            },
        )
        .unwrap();
        assert_eq!(bad["passed"], false);
        assert_eq!(bad["checks"][1]["httpStatus"], 404);
        assert!(!bad.to_string().contains("private"));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(verify_with(&input, &cancelled, good, https).is_err());
        let mut chosen = input.clone();
        chosen["httpsPort"] = json!(54321);
        let ports = RefCell::new(Vec::new());
        let result = verify_with(
            &chosen,
            &Cancellation::default(),
            |h, _| {
                assert!(!h.contains(':'));
                good(h, false)
            },
            |_, _, p, url, _| {
                ports.borrow_mut().push(p);
                assert_eq!(reqwest::Url::parse(url).unwrap().port(), Some(54321));
                Ok(200)
            },
        )
        .unwrap();
        assert_eq!(*ports.borrow(), [54321, 54321, 54321]);
        assert_eq!(result["passed"], true);
        chosen["httpsPort"] = json!(65536);
        assert!(verify_with(&chosen, &Cancellation::default(), good, https).is_err());
    }
    #[test]
    #[cfg(unix)]
    fn privileged_script_preserves_hosts_permissions_and_never_overwrites_after_ca_failure_or_edit() {
        use std::os::unix::fs::PermissionsExt;
        let temp = Temp::new();
        let a = artifacts(&temp);
        let script = mac_script("# quotes: \" ' \\ $(touch /tmp/no) `whoami`\n", &a).unwrap();
        let path = temp.write("syntax.sh", &script);
        assert!(Command::new("/bin/sh").arg("-n").arg(&path).status().unwrap().success());
        assert!(script.contains("add-trusted-cert -d -r trustRoot -p ssl"));
        assert!(!script.contains("PRIVATE KEY"));
        #[cfg(target_os = "macos")]
        {
            let apple = temp.write("authorization.applescript", authorization_script(&script));
            assert!(Command::new("/usr/bin/osacompile")
                .arg("-o")
                .arg(temp.0.join("authorization.scpt"))
                .arg(apple)
                .output()
                .unwrap()
                .status
                .success());
        }
        for mode in ["ok", "ca-fails", "hosts-changed"] {
            let dir = Temp::new();
            let before = "127.0.0.1 localhost\n# $(touch SHOULD_NOT_EXECUTE)\n";
            let hosts = dir.write("hosts", before);
            std::fs::set_permissions(&hosts, std::fs::Permissions::from_mode(0o644)).unwrap();
            let security = dir.tool(
                "security",
                &format!(
                    "#!/bin/sh\n{}\n",
                    match mode {
                        "ca-fails" => "exit 1".into(),
                        "hosts-changed" => format!("printf '%s\\n' '# concurrent edit' >> '{}'", hosts.display()),
                        _ => "exit 0".into(),
                    }
                ),
            );
            let script = mac_script(before, &a)
                .unwrap()
                .replace("/private/etc", dir.0.to_str().unwrap())
                .replace("/usr/bin/security", &format!("'{}'", security.display()))
                .replace("/usr/bin/dscacheutil -flushcache", ":")
                .replace("/usr/bin/killall -HUP mDNSResponder", ":");
            let output = Command::new("/bin/sh")
                .arg(dir.write("install.sh", script))
                .output()
                .unwrap();
            if mode == "ok" {
                assert!(output.status.success());
                let stdout = String::from_utf8(output.stdout).unwrap();
                let backup = stdout.trim().split("APEIRON_BACKUP=").nth(1).unwrap();
                assert_eq!(std::fs::read_to_string(backup).unwrap(), before);
                assert_eq!(
                    std::fs::read_to_string(&hosts).unwrap(),
                    merge_hosts(before, &a).unwrap()
                );
                assert_eq!(std::fs::metadata(&hosts).unwrap().permissions().mode() & 0o777, 0o644);
            } else {
                assert!(!output.status.success());
                assert_eq!(
                    std::fs::read_to_string(&hosts).unwrap(),
                    format!(
                        "{before}{}",
                        if mode == "hosts-changed" {
                            "# concurrent edit\n"
                        } else {
                            ""
                        }
                    )
                );
            }
            assert!(!dir.0.join("SHOULD_NOT_EXECUTE").exists());
        }
    }
    #[test]
    fn local_access_reserves_authorization_and_supports_cancel_retry_and_failed_https() {
        let temp = Temp::new();
        let a = artifacts(&temp);
        let installer = LocalAccess::new();
        let (send, receive) = std::sync::mpsc::channel();
        installer
            .start_with(a.clone(), json!({"available":true}), move |_| {
                receive.recv().unwrap();
                Err(fail("取消", 499))
            })
            .unwrap();
        assert!(installer.active());
        assert_eq!(
            installer
                .start_with(a.clone(), json!({"available":true}), |_| panic!(
                    "duplicate authorization"
                ))
                .unwrap_err()
                .code,
            409
        );
        send.send(()).unwrap();
        installer.wait();
        assert_eq!(installer.snapshot()["phase"], "cancelled");
        for passed in [true, false] {
            installer.start_with(a.clone(),json!({"available":true}),move |_|Ok(json!({"phase":if passed{"succeeded"}else{"failed"},"message":if passed{"通过"}else{"配置已安装，但访问检查未全部通过。"},"backup":"/private/etc/apeiron-access.abc123/hosts.before","checks":[{"host":"apeiron.team.internal","passed":passed},{"host":"iam.team.internal","passed":passed}]}))).unwrap();
            installer.wait();
            assert_eq!(
                installer.snapshot()["phase"],
                if passed { "succeeded" } else { "failed" }
            );
            assert_eq!(installer.snapshot()["checks"].as_array().unwrap().len(), 2);
            assert!(installer.snapshot()["backup"]
                .as_str()
                .unwrap()
                .contains("hosts.before"));
        }
    }
}
