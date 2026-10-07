use super::process::{self, Cancellation};
use super::*;
use std::{
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::Command,
};
pub fn identity(config: &Path, domain: &str) -> (PathBuf, String) {
    let hash = config::hash(format!("{}|{domain}", config.display()).as_bytes());
    let work = config
        .parent()
        .unwrap()
        .join("deployments")
        .join(format!("k3d-{}", &hash[..12]));
    let hash = config::hash(work.to_string_lossy().as_bytes());
    (work, format!("apeiron-{}", &hash[..12]))
}
pub fn preflight(config: &Path, installation: &Value, cancel: &Cancellation) -> Result<String> {
    if installation["topology"] != "single-k3d" {
        return Ok(String::new());
    }
    let (work, cluster) = identity(config, installation["domain"].as_str().unwrap_or(""));
    check(&work, &cluster, installation, cancel)
}
pub fn check(work: &Path, cluster: &str, i: &Value, cancel: &Cancellation) -> Result<String> {
    let name = format!("k3d-{cluster}-serverlb");
    let observed=process::text(Command::new("docker").args(["inspect","--format",r#"{"cluster":{{json (index .Config.Labels "k3d.cluster")}},"bindings":{{json .HostConfig.PortBindings}},"running":{{json .State.Running}}}"#,&name]),b"",10,65536,cancel);
    if let Ok(output) = observed {
        let c: Value = serde_json::from_str(&output).map_err(|_| fail("无法读取集群端口配置。", 400))?;
        return validate_owned(work, cluster, i, &c);
    }
    // An inspect failure alone does not prove absence: confirm the daemon can list containers.
    let names = process::text(
        Command::new("docker").args([
            "ps",
            "-a",
            "--filter",
            &format!("name=^/{name}$"),
            "--format",
            "{{.Names}}",
        ]),
        b"",
        10,
        4096,
        cancel,
    )
    .map_err(|_| fail("无法检查 Docker，请确认 Docker 已启动。", 400))?;
    if !names.trim().is_empty() {
        return Err(fail("无法读取现有集群的端口配置，请检查 Docker 后重试。", 400));
    }
    for key in ["httpPort", "httpsPort"] {
        cancel.check()?;
        let port = i[key].as_u64().unwrap() as u16;
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        match TcpStream::connect_timeout(&address, Duration::from_millis(1500)) {
            Ok(_) => return Err(fail(format!("本机 {port} 端口已被占用，请修改入口端口。"), 409)),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
            Err(_) => return Err(fail("无法检查本机端口。", 400)),
        }
    }
    Ok("入口端口可用，将创建新的 K3d 测试集群。".into())
}
fn validate_owned(work: &Path, cluster: &str, i: &Value, c: &Value) -> Result<String> {
    let marker = resources::json_file(&work.join("installation.json"), 8192).unwrap_or(Value::Null);
    if marker["cluster"] != cluster || marker["domain"] != i["domain"] || c["cluster"] != cluster {
        return Err(fail(
            "同名集群不属于本次安装，请使用其他组织域名，避免接管其他集群。",
            409,
        ));
    }
    for (inner, outer) in [
        (i["httpPort"].as_u64().unwrap(), i["httpPort"].as_u64().unwrap()),
        (443, i["httpsPort"].as_u64().unwrap()),
    ] {
        if c["bindings"][format!("{inner}/tcp")].as_array().is_none_or(|bindings| {
            !bindings.iter().any(|b| {
                b["HostPort"].as_str().and_then(|s| s.parse::<u64>().ok()) == Some(outer)
                    && ["127.0.0.1", "0.0.0.0", ""].contains(&b["HostIp"].as_str().unwrap_or(""))
            })
        }) {
            return Err(fail(
                "已找到本次安装的集群，但入口端口与当前配置不同。请填回创建集群时的端口，再重新部署。",
                409,
            ));
        }
    }
    if c["running"] != true {
        return Err(fail("本次安装的 K3d 集群已停止，请先启动后重新部署。", 409));
    }
    Ok("已识别本次安装的 K3d 集群，将复用集群重新部署，保留数据和凭据。".into())
}
pub fn toolbox_command(target: &Value, env: &BTreeMap<String, String>, tool: &str) -> Command {
    let mut cmd;
    if target["runner"] == "native" {
        cmd = Command::new(tool);
        cmd.args(["--kubeconfig", target["kubeconfig"].as_str().unwrap()]);
    } else {
        cmd = Command::new("docker");
        cmd.args([
            "run",
            "--rm",
            "-i",
            "--pull=never",
            "--name",
            env.get("LAB_CONTAINER").map(String::as_str).unwrap_or("apeiron-setup"),
            "--network",
        ]);
        cmd.arg(env.get("LAB_NETWORK").cloned().unwrap_or_else(|| {
            format!(
                "k3d-{}",
                env.get("LAB_CLUSTER").map(String::as_str).unwrap_or("chentu-helmfile")
            )
        }));
        cmd.args(["--entrypoint", tool, "-v"]);
        cmd.arg(format!(
            "{}/state/kubeconfig:/kubeconfig:ro",
            target["workDir"].as_str().unwrap()
        ));
        cmd.arg(target["image"].as_str().unwrap());
        cmd.args(["--kubeconfig", "/kubeconfig"]);
    }
    cmd.envs(env);
    cmd
}
pub fn helm_state(target: &Value, env: &BTreeMap<String, String>, cancel: &Cancellation) -> Result<()> {
    let data = process::capture(
        toolbox_command(target, env, "helm").args(["list", "--pending", "--all-namespaces", "--output", "json"]),
        b"",
        Duration::from_secs(20),
        262144,
        cancel,
    )?;
    let data: Value = serde_json::from_slice(&data).map_err(|_| fail("无法确认 Helm 状态，尚未运行 sync。", 400))?;
    let rows = data.as_array().ok_or_else(|| fail("Helm 状态格式不正确。", 400))?;
    for row in rows {
        if !config::matches(r"^[a-z0-9][a-z0-9.-]{0,252}$", row["name"].as_str().unwrap_or(""))
            || !config::matches(r"^[a-z0-9][a-z0-9-]{0,62}$", row["namespace"].as_str().unwrap_or(""))
            || ![
                Some("pending-install"),
                Some("pending-upgrade"),
                Some("pending-rollback"),
            ]
            .contains(&row["status"].as_str())
        {
            return Err(fail("无法确认 Helm 状态，尚未运行 sync。", 400));
        }
    }
    if !rows.is_empty() {
        return Err(fail(
            format!(
                "检测到未完成的 Helm 操作：{}。请确认并处理上次操作后重新部署，不会自动回滚。",
                rows.iter()
                    .take(12)
                    .map(|r| format!(
                        "{}/{}（{}）",
                        r["namespace"].as_str().unwrap(),
                        r["name"].as_str().unwrap(),
                        r["status"].as_str().unwrap()
                    ))
                    .collect::<Vec<_>>()
                    .join("、")
            ),
            409,
        ));
    }
    Ok(())
}
pub fn remove_toolbox(name: &str) -> bool {
    let cancel = Cancellation::default();
    let _ = process::capture(
        Command::new("docker").args(["rm", "-f", name]),
        b"",
        Duration::from_secs(15),
        4096,
        &cancel,
    );
    process::text(
        Command::new("docker").args([
            "ps",
            "-a",
            "--filter",
            &format!("name=^/{name}$"),
            "--format",
            "{{.Names}}",
        ]),
        b"",
        10,
        4096,
        &cancel,
    )
    .is_ok_and(|s| s.trim().is_empty())
}
pub fn model_key(model: &Value, target: &Value, env: &BTreeMap<String, String>, cancel: &Cancellation) -> Result<()> {
    use base64::Engine;
    let key = model["apiKey"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("not-required");
    let key = base64::engine::general_purpose::STANDARD.encode(key);
    let body = json!({"apiVersion":"v1","kind":"List","items":[{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"apeiron"}},{"apiVersion":"v1","kind":"Secret","metadata":{"name":"apeiron-model-keys","namespace":"apeiron"},"type":"Opaque","data":{"APEIRON_MODEL_API_KEY_SETUP":key}},{"apiVersion":"v1","kind":"Secret","metadata":{"name":"apeiron-scode-creds","namespace":"apeiron"},"type":"Opaque","data":{"MODEL_API_KEY_SETUP":key}}]});
    process::capture(
        toolbox_command(target, env, "kubectl").args([
            "apply",
            "--server-side",
            "--field-manager=apeiron-setup-model",
            "-f",
            "-",
        ]),
        body.to_string().as_bytes(),
        Duration::from_secs(30),
        65536,
        cancel,
    )
    .map(|_| ())
    .map_err(|_| fail("无法写入模型 Secret，已停止部署。", 400))
}

#[cfg(test)]
mod contracts {
    use super::*;
    use crate::app::contracts::Temp;
    #[test]
    fn owned_clusters_keep_ports_and_identity_and_never_adopt_unrelated_or_stopped_clusters() {
        let temp = Temp::new();
        let i = json!({"domain":"team.internal","httpPort":54320,"httpsPort":54321});
        let mut observed = json!({"cluster":"apeiron-test","running":true,"bindings":{"54320/tcp":[{"HostIp":"127.0.0.1","HostPort":"54320"}],"443/tcp":[{"HostIp":"127.0.0.1","HostPort":"54321"}]}});
        let check = |i: &Value, c: &Value| validate_owned(&temp.0, "apeiron-test", i, c);
        assert!(check(&i, &observed).unwrap_err().message.contains("不属于本次安装"));
        temp.write(
            "installation.json",
            json!({"cluster":"apeiron-test","domain":"team.internal"}).to_string(),
        );
        assert!(check(&i, &observed).unwrap().contains("复用集群"));
        let mut changed = i.clone();
        changed["httpsPort"] = json!(54323);
        assert!(check(&changed, &observed)
            .unwrap_err()
            .message
            .contains("端口与当前配置不同"));
        changed = i.clone();
        changed["domain"] = json!("other.internal");
        assert!(check(&changed, &observed)
            .unwrap_err()
            .message
            .contains("不属于本次安装"));
        observed["running"] = json!(false);
        assert!(check(&i, &observed).unwrap_err().message.contains("已停止"));
        observed["running"] = json!(true);
        observed["bindings"]["54320/tcp"][0]["HostPort"] = json!("54321");
        observed["bindings"]["443/tcp"][0]["HostPort"] = json!("54320");
        assert!(check(&i, &observed).unwrap_err().message.contains("端口与当前配置不同"));
    }
}
