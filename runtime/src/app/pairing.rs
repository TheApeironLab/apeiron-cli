use super::process::{self, Cancellation};
use super::*;
use std::{fs, path::PathBuf, process::Command};
pub fn directory(config: &Path) -> PathBuf {
    config.parent().unwrap().join("connections")
}
pub fn helper(root: &Path, args: &[String], input: &Value, cancel: &Cancellation) -> Result<Value> {
    let script = root.join("bootstrap/public_pairing.py");
    if !script.is_file() {
        return Err(fail("此宸途发行包尚未包含配对与连接管理。", 400));
    }
    let bytes = process::capture(
        Command::new("python3").arg(script).args(args),
        input.to_string().as_bytes(),
        Duration::from_secs(60),
        65536,
        cancel,
    )?;
    serde_json::from_slice(&bytes).map_err(|_| fail("入口返回了无效响应。", 502))
}
pub fn get(config: &Path, id: &str) -> Result<Value> {
    if !config::matches(r"^[a-f0-9]{32}$", id) {
        return Err(fail("连接 ID 格式不正确。", 400));
    }
    let dir = directory(config).join(id);
    if fs::symlink_metadata(&dir)
        .map_err(|_| fail("连接记录不存在。", 404))?
        .file_type()
        .is_symlink()
    {
        return Err(fail("连接目录不能是符号链接。", 400));
    }
    let value = resources::json_file(&dir.join("connection.json"), 16384)?;
    if value["id"] != id
        || !config::safe_domain(value["domain"].as_str().unwrap_or(""))
        || !config::matches(r"^apeiron-[a-f0-9]{12}$", value["identity"].as_str().unwrap_or(""))
    {
        return Err(fail("连接记录无效。", 400));
    }
    let mut result = json!({});
    for key in ["id", "identity", "domain", "host", "sshPort", "publicIp", "tunnelPort"] {
        result[key] = value[key].clone();
    }
    result["state"] = json!(if value["state"] == "revoked" {
        "revoked"
    } else {
        "paired"
    });
    Ok(result)
}
pub fn list(config: &Path) -> Result<Value> {
    let entries = match fs::read_dir(directory(config)) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!([])),
        Err(_) => return Err(fail("无法读取连接目录。", 500)),
    };
    let mut rows = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| fail("无法读取连接目录。", 500))?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if config::matches(r"^[a-f0-9]{32}$", &id) {
            rows.push(get(config, &id)?);
        }
    }
    Ok(json!(rows))
}
pub fn pair(config: &Path, code: &str, cancel: &Cancellation) -> Result<Value> {
    if code.len() > 8192 || !config::matches(r"^apeiron-pair-v1\.[A-Za-z0-9_-]+$", code) {
        return Err(fail("请粘贴 ECS 生成的完整配对码。", 400));
    }
    let dir = directory(config);
    config::location(&dir.join("guard"))?;
    process::private_directory(&dir)?;
    let root = resources::resolve(&json!({}), cancel, &|_| {})?;
    let value = helper(
        &root,
        &["client-pair".into(), dir.to_string_lossy().into_owned()],
        &json!({"code":code}),
        cancel,
    )?;
    get(config, value["id"].as_str().unwrap_or(""))
}
pub fn scope(config: &Path, id: &str, installation: &Value) -> Result<Value> {
    let c = get(config, id)?;
    let p = &installation["publicAccess"];
    if c["state"] != "paired"
        || c["domain"] != installation["domain"]
        || c["publicIp"] != p["publicIp"]
        || c["tunnelPort"] != p["tunnelPort"]
    {
        return Err(fail("配对入口已撤销或与当前平台域名不匹配。", 400));
    }
    Ok(c)
}
pub fn action(config: &Path, action: &str, id: &str, cancel: &Cancellation) -> Result<Value> {
    let mut c = get(config, id)?;
    if c["state"] == "revoked" {
        c["tunnel"] = json!(false);
        c["routes"] = json!(false);
        c["message"] = json!("此连接已撤销，需要在 ECS 重新生成配对码。");
        return Ok(c);
    }
    let root = resources::resolve(&json!({}), cancel, &|_| {})?;
    let r = helper(
        &root,
        &[
            if action == "revoke" {
                "client-revoke"
            } else {
                "client-status"
            }
            .into(),
            directory(config).to_string_lossy().into_owned(),
            id.into(),
        ],
        &json!({}),
        cancel,
    )?;
    if r["id"] != id || ![json!("paired"), json!("revoked")].contains(&r["state"]) {
        return Err(fail("入口状态无效。", 400));
    }
    c["state"] = r["state"].clone();
    c["tunnel"] = json!(r["tunnel"] == true);
    c["routes"] = json!(r["routes"] == true);
    c["checkedAt"] = json!(chrono::Utc::now().timestamp());
    if action == "revoke" {
        c["localStopped"] = json!(r["localStopped"] != false);
    }
    if action == "test" && c["state"] != "revoked" {
        let result = discovery::dns(
            &json!({"domain":c["domain"],"entryIp":c["publicIp"],"local":false}),
            true,
            cancel,
        )?;
        c["dns"] = result["passed"].clone();
        let mut passed = true;
        for name in ["apeiron", "iam"] {
            let host = format!("{name}.{}", c["domain"].as_str().unwrap());
            let ip = c["publicIp"].as_str().unwrap_or("");
            let output = process::text(
                Command::new("curl").args([
                    "--disable",
                    "--noproxy",
                    "*",
                    "--silent",
                    "--show-error",
                    "--connect-timeout",
                    "5",
                    "--max-time",
                    "10",
                    "--resolve",
                    &format!("{host}:443:{ip}"),
                    "--output",
                    "/dev/null",
                    "--write-out",
                    "%{http_code}",
                    &format!("https://{host}/"),
                ]),
                b"",
                12,
                4096,
                cancel,
            );
            passed &= output.is_ok_and(|s| config::matches(r"^[23]\d\d$", s.trim()));
        }
        c["https"] = json!(passed);
        c["message"] = json!("DNS 与 HTTPS 分别检测；尚未部署时 HTTPS 未通过是预期状态。");
    }
    Ok(c)
}
pub fn run(args: &[String]) -> Result<()> {
    let (pos, options) = parse(
        args,
        &["domain", "public-ip", "ssh-host", "ssh-port", "id", "config"],
        &["json", "help"],
    )?;
    if options.contains_key("help") || pos.len() < 2 {
        println!("{}", include_str!("../../assets/entry-help.txt"));
        return Ok(());
    }
    if pos.len() != 2 {
        return Err(fail("Unknown entry command", 2));
    }
    let cancel = Cancellation::default();
    let result = match (pos[0].as_str(), pos[1].as_str()) {
        ("connection", action @ ("list" | "status" | "test" | "revoke")) => {
            if options.keys().any(|s| !["id", "config", "json"].contains(&s.as_str())) {
                return Err(fail("Invalid connection option", 2));
            }
            let path = process::config_path(options.get("config"))?;
            if action == "list" {
                list(&path)?
            } else {
                action_result(&path, action, required(&options, "id")?, &cancel)?
            }
        }
        ("entry", action @ ("pair" | "status" | "revoke")) => {
            let allowed = if action == "pair" {
                vec!["domain", "public-ip", "ssh-host", "ssh-port", "json"]
            } else {
                vec!["domain", "json"]
            };
            if options.keys().any(|s| !allowed.contains(&s.as_str())) {
                return Err(fail("Invalid entry option", 2));
            }
            let domain = required(&options, "domain")?;
            if !config::safe_domain(domain) {
                return Err(fail("Invalid domain", 2));
            }
            #[cfg(unix)]
            let root_user = unsafe { libc::getuid() } == 0;
            #[cfg(not(unix))]
            let root_user = false;
            if !cfg!(target_os = "linux") || !root_user {
                return Err(fail("Run entry administration on the Ubuntu ECS with sudo", 2));
            }
            let root = resources::resolve(&json!({}), &cancel, &|_| {})?;
            if action == "pair" {
                let ip = required(&options, "public-ip")?;
                let port = options
                    .get("ssh-port")
                    .map(String::as_str)
                    .unwrap_or("22")
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .ok_or_else(|| fail("Invalid SSH port", 2))?;
                let mut body = json!({"domain":domain,"publicIp":ip,"sshPort":port});
                if let Some(host) = options.get("ssh-host") {
                    body["host"] = json!(host);
                }
                helper(&root, &["invite".into()], &body, &cancel)?
            } else {
                helper(&root, &[action.into(), domain.into()], &json!({}), &cancel)?
            }
        }
        _ => return Err(fail("Unknown entry command; use apeiron platform entry --help", 2)),
    };
    if options.contains_key("json") {
        println!("{result}");
    } else if result.is_array() {
        output(json!({"items":result}), &options, "apeiron.platform.entry.v1")?;
    } else {
        println!("schema=apeiron.platform.entry.v1");
        for key in [
            "code",
            "domain",
            "state",
            "tunnel",
            "routes",
            "dns",
            "https",
            "expiresAt",
        ] {
            if let Some(v) = result.get(key) {
                println!(
                    "{key}\t{}",
                    v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())
                );
            }
        }
    }
    if pos[0] == "connection" && pos[1] == "test" && ["dns", "https", "tunnel"].iter().any(|k| result[k] != true) {
        return Err(fail("Connection checks did not all pass", 9));
    }
    Ok(())
}
fn action_result(path: &Path, verb: &str, id: &str, cancel: &Cancellation) -> Result<Value> {
    action(path, verb, id, cancel)
}
