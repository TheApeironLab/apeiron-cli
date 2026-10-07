use super::*;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::{fs, net::Ipv4Addr, path::PathBuf};
pub fn matches(pattern: &str, s: &str) -> bool {
    Regex::new(pattern).unwrap().is_match(s)
}
pub fn text(v: &Value, key: &str, max: usize, optional: bool) -> Result<String> {
    let s = v[key]
        .as_str()
        .ok_or_else(|| fail(format!("{key} 格式不正确。"), 400))?;
    if s.len() > max || s.chars().any(char::is_control) || (!optional && s.trim().is_empty()) {
        return Err(fail(format!("{key} 格式不正确。"), 400));
    }
    Ok(s.trim().to_owned())
}
pub fn safe_name(s: &str) -> bool {
    matches(r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$", s)
}
pub fn safe_host(s: &str) -> bool {
    s.len() <= 253 && matches(r"^[a-zA-Z0-9][a-zA-Z0-9._-]*$", s)
}
pub fn safe_domain(s: &str) -> bool {
    s.len() <= 220 && s.contains('.') && s.parse::<Ipv4Addr>().is_err() && s.split('.').all(safe_name)
}
pub fn safe_ip(s: &str) -> bool {
    s.parse::<Ipv4Addr>()
        .map(|ip| {
            let b = ip.octets();
            b[0] != 0 && b[0] < 224 && !(b[0] == 169 && b[1] == 254)
        })
        .unwrap_or(false)
}
pub fn apps() -> Value {
    serde_json::from_str(include_str!("../../assets/apps.json")).unwrap()
}
pub fn defaults() -> Value {
    let mac = cfg!(target_os = "macos");
    json!({"installation":{"topology":if mac{"single-k3d"}else{"single-k3s"},"httpPort":if mac{54320}else{80},"httpsPort":if mac{54321}else{443},"domain":"","entryIp":if mac{"127.0.0.1"}else{""},"ha":false,"sshUser":"","sshKey":"","sshPort":22,"nodes":[]},"runner":if mac{"docker"}else{"native"},"root":"","environment":std::env::var("CHENTU_ENV").or_else(|_|std::env::var("LAB_ENV")).unwrap_or_default(),"kubeconfig":std::env::var("KUBECONFIG").unwrap_or_default(),"workDir":std::env::var("LAB_WORK_DIR").unwrap_or_default(),"image":std::env::var("LAB_IMAGE").unwrap_or_else(|_|"chentu-lab".into()),"offline":false,"bundleDir":""})
}
pub fn connection(v: &Value) -> Result<Value> {
    let user = v["sshUser"].as_str().unwrap_or("");
    let key = v["sshKey"].as_str().unwrap_or("");
    let port = v
        .get("sshPort")
        .unwrap_or(&json!(22))
        .as_u64()
        .ok_or_else(|| fail("SSH 端口不正确。", 400))?;
    if !user.is_empty() && !matches(r"^[a-z_][a-z0-9_-]{0,63}$", user) {
        return Err(fail("SSH 用户名格式不正确。", 400));
    }
    if key.len() > 4096
        || key.chars().any(|c| c.is_control() || "{}".contains(c))
        || (!key.is_empty() && !Path::new(key).is_absolute())
        || !(1..=65535).contains(&port)
    {
        return Err(fail("SSH 路径或端口不正确。", 400));
    }
    Ok(json!({"sshUser":user,"sshKey":key,"sshPort":port}))
}
pub fn installation(v: &Value) -> Result<Value> {
    let topology = text(v, "topology", 32, false)?;
    if !["single-k3d", "single-k3s", "multi-k3s"].contains(&topology.as_str()) {
        return Err(fail("请选择安装方式。", 400));
    }
    let local = topology == "single-k3d";
    let domain = text(v, "domain", 220, false)?;
    if !safe_domain(&domain) {
        return Err(fail("请填写有效的平台域名。", 400));
    }
    let ip = v["entryIp"].as_str().unwrap_or(if local { "127.0.0.1" } else { "" });
    if (local && ip != "127.0.0.1") || (!ip.is_empty() && (!safe_ip(ip) || (!local && ip.starts_with("127.")))) {
        return Err(fail("请填写可访问的入口 IPv4 地址。", 400));
    }
    let ha = v["ha"].as_bool().ok_or_else(|| fail("高可用选项格式不正确。", 400))?;
    if ha && topology != "multi-k3s" {
        return Err(fail("高可用仅适用于多机 K3s。", 400));
    }
    let http = v
        .get("httpPort")
        .map(|n| n.as_u64())
        .unwrap_or(Some(if local { 54320 } else { 80 }))
        .ok_or_else(|| fail("端口不正确。", 400))?;
    let https = v
        .get("httpsPort")
        .map(|n| n.as_u64())
        .unwrap_or(Some(if local { 54321 } else { 443 }))
        .ok_or_else(|| fail("端口不正确。", 400))?;
    if !(1..=65535).contains(&http)
        || !(1..=65535).contains(&https)
        || http == https
        || (!local && (http != 80 || https != 443))
        || (local && (http == 443 || https == 80))
    {
        return Err(fail("HTTP / HTTPS 端口不正确。", 400));
    }
    let nodes = v["nodes"].as_array().ok_or_else(|| fail("节点列表不正确。", 400))?;
    if nodes.len() > 32 {
        return Err(fail("最多支持 32 个节点。", 400));
    }
    for n in nodes {
        if !safe_host(n["host"].as_str().unwrap_or(""))
            || !safe_name(n["name"].as_str().unwrap_or(""))
            || n["address"].as_str().unwrap_or("").parse::<Ipv4Addr>().is_err()
            || ![Some("server"), Some("agent")].contains(&n["role"].as_str())
        {
            return Err(fail("请检测节点，并确认节点名称、地址和角色。", 400));
        }
    }
    for key in ["host", "name", "address"] {
        let mut seen = std::collections::HashSet::new();
        if nodes.iter().any(|n| !seen.insert(n[key].as_str().unwrap_or(""))) {
            return Err(fail("节点地址和名称不能重复。", 400));
        }
    }
    let servers = nodes.iter().filter(|n| n["role"] == "server").count();
    if topology == "multi-k3s" {
        if nodes.len() < 2
            || if ha {
                servers < 3 || servers % 2 == 0
            } else {
                servers != 1
            }
        {
            return Err(fail("多机部署需要有效的控制节点数量。", 400));
        }
    } else if !nodes.is_empty() {
        return Err(fail("单机部署不接受远程节点。", 400));
    }
    let mut out = connection(v)?;
    for (k,value) in json!({"topology":topology,"domain":domain,"entryIp":ip,"httpPort":http,"httpsPort":https,"ha":ha,"nodes":nodes}).as_object().unwrap(){out[k]=value.clone();}
    if let Some(p) = v.get("publicAccess") {
        let mode = p["mode"].as_str().unwrap_or("");
        let public_ip = p["publicIp"].as_str().unwrap_or("");
        let port = p
            .get("tunnelPort")
            .map(|n| n.as_u64())
            .unwrap_or(Some(19444))
            .unwrap_or(0);
        if topology != "single-k3s"
            || !["direct", "relay"].contains(&mode)
            || !safe_ip(public_ip)
            || matches(
                r"^(10|127|192\.168|172\.(1[6-9]|2\d|3[01])|100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7]))\.",
                public_ip,
            )
            || !(1024..=65535).contains(&port)
            || matches(r"\.(internal|local|localhost|test|invalid|example)$", &domain)
        {
            return Err(fail("公网入口配置不正确。", 400));
        }
        let mut public = json!({"mode":mode,"publicIp":public_ip,"tunnelPort":port});
        if let Some(id) = p.get("pairingId") {
            if mode != "relay" || p.get("gateway").is_some() || !matches(r"^[a-f0-9]{32}$", id.as_str().unwrap_or("")) {
                return Err(fail("配对连接格式不正确。", 400));
            }
            public["pairingId"] = id.clone();
        } else if mode == "relay" {
            let g = &p["gateway"];
            let host = g["host"].as_str().unwrap_or("");
            if !safe_host(host) {
                return Err(fail("请填写 ECS SSH 地址。", 400));
            }
            let mut gateway = connection(g)?;
            gateway["host"] = json!(host);
            public["gateway"] = gateway;
        }
        out["publicAccess"] = public;
    }
    Ok(out)
}
pub fn models(v: &Value, previous: &Value) -> Result<Value> {
    let provider = text(v, "provider", 64, false)?;
    if !matches(r"^[a-z0-9][a-z0-9._-]{0,63}$", &provider) {
        return Err(fail("Provider 格式不正确。", 400));
    }
    let base = text(v, "baseUrl", 2048, false)?.trim_end_matches('/').to_owned();
    let url = reqwest::Url::parse(&base).map_err(|_| fail("模型 API 地址不正确。", 400))?;
    if !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(fail("模型 API 地址不正确。", 400));
    }
    let key = if v.get("apiKey").is_none() && previous["baseUrl"] == base {
        previous["apiKey"].as_str().unwrap_or("").to_owned()
    } else {
        text(v, "apiKey", 4096, true)?
    };
    Ok(
        json!({"provider":provider,"baseUrl":base,"apiKey":key,"fast":text(v,"fast",256,false)?,"deep":text(v,"deep",256,false)?}),
    )
}
fn absolute(v: &Value, key: &str) -> Result<String> {
    let p = text(v, key, 4096, false)?;
    if !Path::new(&p).is_absolute() {
        return Err(fail(format!("{key} 必须是绝对路径。"), 400));
    }
    Ok(p)
}
pub fn validate(v: &Value, current: &Value) -> Result<Value> {
    let slug = text(v, "slug", 63, false)?;
    if !safe_name(&slug) {
        return Err(fail("Slug 格式不正确。", 400));
    }
    let list = v["apps"].as_array().ok_or_else(|| fail("请选择应用。", 400))?;
    let apps = apps();
    let catalog = apps.as_array().unwrap();
    let mut seen = std::collections::HashSet::new();
    if list.is_empty()
        || list.len() > catalog.len()
        || list
            .iter()
            .any(|x| !seen.insert(x.as_str().unwrap_or("")) || !catalog.iter().any(|a| a["id"] == *x))
    {
        return Err(fail("应用列表不正确或重复。", 400));
    }
    let mut out = json!({"schemaVersion":2,"slug":slug,"apps":catalog.iter().filter(|a|list.contains(&a["id"])).map(|a|a["id"].clone()).collect::<Vec<_>>()});
    if let Some(m) = v.get("models") {
        if !m.is_null() {
            out["models"] = models(m, &current["models"])?;
        }
    } else if current.get("models").is_some() {
        out["models"] = current["models"].clone();
    }
    if current.get("llm").is_some() {
        out["llm"] = current["llm"].clone();
    }
    if let Some(d) = v.get("deployment") {
        if d.get("profile").is_some() || std::env::var_os("CHENTU_PROFILE").is_some() {
            return Err(fail("profile 已移除，请选择 installation.topology。", 400));
        }
        let offline = d["offline"].as_bool().unwrap_or(false);
        let bundle = if d["bundleDir"].as_str().unwrap_or("").is_empty() {
            String::new()
        } else {
            absolute(d, "bundleDir")?
        };
        if offline && bundle.is_empty() {
            return Err(fail("离线部署请选择安装包目录。", 400));
        }
        if let Some(i) = d.get("installation") {
            let i = installation(i)?;
            if !d["offline"].is_boolean() || (offline && i.get("publicAccess").is_some()) {
                return Err(fail("在线/离线部署配置不正确。", 400));
            }
            out["deployment"] = json!({"runner":if i["topology"]=="single-k3d"{"docker"}else{"native"},"installation":i,"root":"","environment":"","kubeconfig":"","workDir":"","image":"chentu-lab","offline":offline,"bundleDir":bundle});
        } else {
            let runner = text(d, "runner", 10, false)?;
            if !["native", "docker"].contains(&runner.as_str()) {
                return Err(fail("请选择部署方式。", 400));
            }
            let docker = runner == "docker";
            let image = if docker {
                text(d, "image", 256, false)?
            } else {
                String::new()
            };
            if docker && !matches(r"^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*$", &image) {
                return Err(fail("工具箱镜像名称不正确。", 400));
            }
            out["deployment"] = json!({"runner":runner,"root":if d["root"].as_str().unwrap_or("").is_empty(){String::new()}else{absolute(d,"root")?},"environment":absolute(d,"environment")?,"kubeconfig":if docker{String::new()}else{absolute(d,"kubeconfig")?},"workDir":if docker{absolute(d,"workDir")?}else{String::new()},"image":image,"offline":offline,"bundleDir":bundle});
        }
    }
    Ok(out)
}
pub fn hash(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
pub fn location(path: &Path) -> Result<()> {
    let mut ancestor = path
        .parent()
        .ok_or_else(|| fail("配置路径不正确。", 400))?
        .to_path_buf();
    while !ancestor.exists() {
        if !ancestor.pop() {
            return Err(fail("配置路径不正确。", 400));
        }
    }
    let mut p = fs::canonicalize(ancestor).map_err(|_| fail("无法检查配置目录。", 400))?;
    loop {
        if fs::symlink_metadata(p.join(".git")).is_ok() {
            return Err(fail("配置包含密钥，请把配置文件放在 Git 工作目录之外。", 400));
        }
        if !p.pop() {
            break;
        }
    }
    Ok(())
}
pub fn read(path: &Path) -> Result<Value> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({"revision":null})),
        Err(_) => return Err(fail("无法读取配置。", 500)),
    };
    if !meta.is_file() || meta.len() > 16384 {
        return Err(fail("配置路径必须是普通文件且不超过 16 KiB。", 409));
    }
    let bytes = fs::read(path).map_err(|_| fail("无法读取配置。", 500))?;
    let body: Value =
        serde_json::from_slice(&bytes).map_err(|_| fail("现有配置格式或版本不支持，已保留原文件。", 409))?;
    if body["schemaVersion"] != 1 && body["schemaVersion"] != 2 {
        return Err(fail("现有配置版本不支持。", 409));
    }
    let mut c = validate(&body, &Value::Null)?;
    if let Some(llm) = body.get("llm") {
        c["llm"] = json!({"baseUrl":text(llm,"baseUrl",2048,false)?,"modelId":text(llm,"modelId",256,false)?,"apiKey":text(llm,"apiKey",4096,true)?});
    }
    Ok(json!({"config":c,"revision":hash(&bytes)}))
}
pub fn public(s: &Value) -> Value {
    let c = &s["config"];
    if c.is_null() {
        return json!({"revision":s["revision"],"config":null});
    }
    let mut result = json!({"slug":c["slug"],"apps":c["apps"],"deployment":c["deployment"],"models":null});
    if let Some(m) = c.get("models") {
        result["models"] = json!({"provider":m["provider"],"baseUrl":m["baseUrl"],"fast":m["fast"],"deep":m["deep"],"hasApiKey":!m["apiKey"].as_str().unwrap_or("").is_empty()});
    }
    json!({"revision":s["revision"],"config":result})
}
pub fn save(path: &Path, input: &Value) -> Result<Value> {
    location(path)?;
    let old = read(path)?;
    if input.get("revision") != old.get("revision") {
        return Err(fail("配置已更新，请刷新后重试。", 409));
    }
    let c = validate(input, &old["config"])?;
    if c.get("deployment").is_none() {
        return Err(fail("请填写部署环境。", 400));
    }
    for a in apps().as_array().unwrap() {
        if a["required"] == true && !c["apps"].as_array().unwrap().contains(&a["id"]) {
            return Err(fail("请保留必选应用。", 400));
        }
    }
    super::process::private_directory(path.parent().unwrap())?;
    let lock = PathBuf::from(format!("{}.lock", path.display()));
    private_write(&lock, b"").map_err(|_| fail("配置正在被其他进程使用，请检查 .lock 文件。", 409))?;
    let temp = PathBuf::from(format!("{}.{}.tmp", path.display(), rand::random::<u64>()));
    let result = (|| {
        if read(path)?["revision"] != old["revision"] {
            return Err(fail("配置已更新，请刷新后重试。", 409));
        }
        let raw = serde_json::to_string_pretty(&c).unwrap() + "\n";
        private_write(&temp, raw.as_bytes())?;
        OpenOptions::new()
            .read(true)
            .open(&temp)
            .and_then(|f| f.sync_all())
            .map_err(|_| fail("无法持久化配置。", 500))?;
        fs::rename(&temp, path).map_err(|_| fail("无法保存配置。", 500))?;
        Ok(json!({"config":c,"revision":hash(raw.as_bytes())}))
    })();
    let _ = fs::remove_file(temp);
    let _ = fs::remove_file(lock);
    result
}
