use super::*;
use super::{
    deploy::{strings, Context},
    process,
};
use std::{fs, io::Read, path::PathBuf, process::Command};
pub struct Prepared {
    pub target: Value,
    pub values: Value,
    pub(crate) plan: Value,
    nodes: Vec<Value>,
    pub(crate) bundle: PathBuf,
    cluster: String,
    arch: String,
    generated: Vec<String>,
    tool_env: BTreeMap<String, String>,
}
/// Node rules for the hosted (Nexus-first) native contract, checked after the
/// management machine has been appended as the single-k3s node. `config.rs`
/// already enforces the general shape (empty nodes for single-*, HA odd servers).
pub fn hosted_nodes(topology: &str, nodes: &[Value]) -> Result<()> {
    let servers = nodes.iter().filter(|n| n["role"] == "server").count();
    let ok = match topology {
        "multi-k3s" => nodes.len() >= 3 && servers == nodes.len() && servers % 2 == 1,
        "single-k3s" => nodes.len() == 1 && nodes[0]["host"] == "localhost" && servers == 1,
        _ => false,
    };
    if !ok {
        return Err(fail(
            match topology {
                "multi-k3s" => {
                    "原生 Nexus-first 多机部署需要至少 3 个、奇数个 server 节点且没有 agent 节点；尚未修改集群。"
                }
                "single-k3s" => "原生 Nexus-first 单机部署以管理机为唯一节点，不接受远程节点；尚未修改集群。",
                _ => "原生 Nexus-first 不支持该拓扑；尚未修改集群。",
            },
            400,
        ));
    }
    Ok(())
}
fn path(v: &Value, key: &str) -> PathBuf {
    PathBuf::from(v[key].as_str().unwrap_or(""))
}
fn archive_toolbox_image(target: &Value) -> Result<&str> {
    // Published legacy archives (including rc.5) only declare toolboxImageId.
    // New catalogs may use a tag to avoid Docker store-specific image IDs.
    target["toolboxImage"]
        .as_str()
        .filter(|image| !image.is_empty())
        .or_else(|| target["toolboxImageId"].as_str())
        .filter(|image| config::matches(r"^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*$", image))
        .ok_or_else(|| fail("安装包缺少有效的工具箱镜像名称或 ID。", 400))
}
fn archive_metadata(path: &std::path::Path, name: &str) -> std::io::Result<Vec<u8>> {
    if !resources::safe_relative(name) {
        return Err(std::io::Error::other("unsafe image metadata path"));
    }
    let mut archive = tar::Archive::new(fs::File::open(path)?);
    let mut found = None;
    for entry in archive.entries_with_seek()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() != std::path::Path::new(name) {
            continue;
        }
        if found.is_some() || !entry.header().entry_type().is_file() || entry.size() > 4 * 1024 * 1024 {
            return Err(std::io::Error::other("invalid image metadata member"));
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        found = Some(bytes);
    }
    found.ok_or_else(|| std::io::Error::other("missing image metadata"))
}
fn system_image_identity(
    archive: &std::path::Path,
    name: &str,
    expected: &str,
    observed: &str,
    architecture: &str,
) -> std::io::Result<bool> {
    if expected == observed {
        return Ok(true);
    }
    // Classic Docker reports the config digest; containerd may report an OCI
    // manifest/index digest. Prove their relationship from the already
    // checksum-verified archive, rather than accepting an arbitrary loaded tag.
    let manifest: Value = serde_json::from_slice(&archive_metadata(archive, "manifest.json")?)?;
    let rows = manifest
        .as_array()
        .ok_or_else(|| std::io::Error::other("invalid Docker manifest"))?;
    let tagged = rows
        .iter()
        .filter(|row| {
            row["RepoTags"]
                .as_array()
                .is_some_and(|tags| tags.iter().any(|tag| tag == name))
        })
        .collect::<Vec<_>>();
    if tagged.len() != 1 {
        return Ok(false);
    }
    let config_path = tagged[0]["Config"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing image config"))?;
    let bytes = archive_metadata(archive, config_path)?;
    if format!("sha256:{}", config::hash(&bytes)) != observed {
        return Ok(false);
    }
    let config: Value = serde_json::from_slice(&bytes)?;
    if config["os"] != "linux" || config["architecture"] != architecture {
        return Ok(false);
    }
    let mut digest = expected.to_owned();
    let mut size = None;
    for _ in 0..4 {
        if !config::matches(r"^sha256:[a-f0-9]{64}$", &digest) {
            return Ok(false);
        }
        let data = archive_metadata(archive, &format!("blobs/sha256/{}", &digest[7..]))?;
        if format!("sha256:{}", config::hash(&data)) != digest || size.is_some_and(|n| n != data.len() as u64) {
            return Ok(false);
        }
        let object: Value = serde_json::from_slice(&data)?;
        if object["schemaVersion"] != 2 {
            return Ok(false);
        }
        if object["config"].is_object() {
            return Ok(object["config"]["digest"] == observed && object["config"]["size"] == bytes.len() as u64);
        }
        let Some(children) = object["manifests"].as_array() else {
            return Ok(false);
        };
        let matches = children
            .iter()
            .filter(|child| {
                child["platform"]["os"] == config["os"] && child["platform"]["architecture"] == config["architecture"]
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Ok(false);
        }
        let Some(next) = matches[0]["digest"].as_str() else {
            return Ok(false);
        };
        let Some(next_size) = matches[0]["size"].as_u64() else {
            return Ok(false);
        };
        digest = next.to_owned();
        size = Some(next_size);
    }
    Ok(false)
}
fn present(tool: &str, env: &BTreeMap<String, String>) -> bool {
    let paths = env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    std::env::split_paths(&paths).any(|p| {
        let Ok(metadata) = fs::metadata(p.join(tool)) else {
            return false;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        metadata.is_file()
    })
}
fn require_tools(tools: &[&str], env: &BTreeMap<String, String>) -> Result<()> {
    let missing = tools
        .iter()
        .filter(|tool| !present(tool, env))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(fail(
            format!(
                "管理机缺少可执行工具：{}。请在 CLI 主机安装，或使用包含这些工具的离线安装包后重试；尚未创建集群。",
                missing.join("、")
            ),
            400,
        ));
    }
    Ok(())
}
fn check_offline_operator(target: &Value, c: &Context) -> Result<()> {
    let helper = path(target, "root").join("bootstrap/prepare-operator.py");
    if target["offline"] != true || !helper.is_file() {
        return Ok(());
    }
    let bundle = path(target, "bundleDir");
    if !bundle.is_absolute() {
        return Err(fail("管理机离线依赖预检需要原始安装包的绝对路径。", 400));
    }
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    c.checked("python3", strings(&[&helper.to_string_lossy(), "--bundle", &bundle.to_string_lossy(), "--check"]),
        target, &BTreeMap::new(), "检查管理机离线工具及本地供应资源（只读）")
        .map_err(|_| fail(format!("管理机离线依赖预检未通过，请查看日志。若缺少工具，可在 CLI 主机显式运行 sudo python3 {} --bundle {} --install 后重试。",
            quote(&helper.to_string_lossy()), quote(&bundle.to_string_lossy())), 400))
}
fn replace(v: &mut Value, entries: &[(&str, String)]) {
    match v {
        Value::String(s) => {
            for (k, value) in entries {
                *s = s.replace(k, value);
            }
        }
        Value::Array(a) => {
            for v in a {
                replace(v, entries)
            }
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                replace(v, entries)
            }
        }
        _ => {}
    }
}
pub fn prepare(config: &Value, c: &Context) -> Result<Prepared> {
    let mut target = config["deployment"].clone();
    let i = target["installation"].clone();
    let docker = i["topology"] == "single-k3d";
    let mut system_tools = if docker {
        vec!["bash", "docker"]
    } else {
        vec!["bash", "tar", "python3"]
    };
    if i["topology"] == "multi-k3s" {
        system_tools.push("ssh");
    }
    // These tools cannot be supplied by target-node resources. Fail before
    // resolving or downloading the deployment package.
    require_tools(&system_tools, &BTreeMap::new())?;
    if !config::safe_ip(i["entryIp"].as_str().unwrap_or("")) {
        return Err(fail("请确认平台入口 IP。", 400));
    }
    let root = resources::resolve(&target, &c.cancel, &|m| c.event(m))?;
    target["root"] = json!(root);
    let mut arch = discovery::architecture(std::env::consts::ARCH).to_owned();
    let mut nodes = i["nodes"].as_array().unwrap().clone();
    let mut facts = Vec::new();
    if !docker {
        c.event("复查节点系统、架构、权限和已有集群。");
        facts = if i["topology"] == "multi-k3s" {
            let mut input = i.clone();
            input["hosts"] = json!(nodes.iter().map(|n| n["host"].clone()).collect::<Vec<_>>());
            discovery::nodes(&input, &c.cancel)?["nodes"]
                .as_array()
                .unwrap()
                .clone()
        } else {
            vec![discovery::node("localhost", &i, true, &c.cancel)?]
        };
        for n in &facts {
            if let Some(error) = n["error"].as_str() {
                return Err(fail(format!("{}：{error}", n["host"].as_str().unwrap_or("")), 400));
            }
        }
        arch = discovery::architecture(facts[0]["architecture"].as_str().unwrap()).to_owned();
        if facts
            .iter()
            .any(|n| discovery::architecture(n["architecture"].as_str().unwrap()) != arch)
        {
            return Err(fail("请选择 CPU 架构一致的节点。", 400));
        }
        if i["topology"] == "single-k3s" {
            let addresses = facts[0]["addresses"].as_array().unwrap();
            nodes.push(json!({"host":"localhost","name":facts[0]["name"],"address":if addresses.contains(&i["entryIp"]){i["entryIp"].clone()}else{addresses[0].clone()},"role":"server"}));
        }
        for n in &nodes {
            if !facts
                .iter()
                .any(|f| f["host"] == n["host"] && f["addresses"].as_array().unwrap().contains(&n["address"]))
            {
                return Err(fail("节点内网地址已变化，请重新检测。", 400));
            }
        }
    }
    let key = format!("{}-{arch}", if docker { "k3d" } else { "k3s" });
    let catalog = resources::catalog(&root, &key)?;
    let mut plan = resources::plan(&catalog, &key, &crate::app::config::components(&config["apps"]))?;
    let bundled = !docker
        && plan["target"]["operatorTools"] == true
        && cfg!(target_os = "linux")
        && discovery::architecture(std::env::consts::ARCH) == arch;
    if bundled && key == "k3s-amd64" {
        check_offline_operator(&target, c)?;
    }
    if !docker && !bundled {
        require_tools(&["ansible-playbook", "helm", "helmfile", "curl"], &BTreeMap::new())?;
    }
    let cache = if target["bundleDir"].as_str().unwrap_or("").is_empty() {
        c.directory.parent().unwrap().join("resources")
    } else {
        path(&target, "bundleDir")
    }
    .join(catalog["resourceDirectory"].as_str().unwrap_or(""));
    distribution::prepare(
        &root,
        &cache,
        &catalog,
        &mut plan,
        &key,
        i["topology"].as_str().unwrap(),
        distribution::Preparation {
            offline: target["offline"] == true,
            cancel: &c.cancel,
            progress: &|m| c.event(m),
        },
    )?;
    if !docker && plan.get("releaseManifest").is_some() {
        hosted_nodes(i["topology"].as_str().unwrap_or(""), &nodes)?;
    }
    if !docker {
        discovery::validate_platform(&plan["target"], &facts)?;
    }
    if config["apps"].as_array().unwrap().contains(&json!("vasi"))
        && (plan["target"]["clusterOidc"] != true
            || !plan["components"]
                .as_array()
                .unwrap()
                .contains(&json!("cluster-access"))
            || !root.join("cli/src/chentu/cluster_oidc.py").is_file()
            || !root.join("bootstrap/oidc.yaml").is_file())
    {
        return Err(fail("安装包未包含完整的集群 SSO 初始化；尚未修改集群。", 400));
    }
    if docker && (i["httpPort"] != 80 || i["httpsPort"] != 443) && plan["target"]["publicPorts"] != true {
        return Err(fail("安装包不支持自定义入口端口。", 400));
    }
    if i["publicAccess"].is_object() {
        public_access("check", &target, c, &BTreeMap::new())?;
    } else if !docker {
        if !root.join("bootstrap/private_dns.py").is_file() {
            return Err(fail(
                "此安装包缺少原生内部解析准备程序，请使用新版完整包；尚未修改主机。",
                400,
            ));
        }
        c.event("私有安装将按入口 IP 配置管理机、节点和集群内部解析，无需预先配置外部 DNS。");
    }
    resources::prepare_files(&plan, &cache, target["offline"] == true, &c.cancel, &|m| c.event(m))?;
    let work = if docker {
        cluster::identity(&c.config, i["domain"].as_str().unwrap()).0
    } else {
        c.directory.join("work")
    };
    let bundle = work.join("bundle");
    process::private_directory(&bundle)?;
    if docker {
        // Even the configuration checker creates /work/state on Linux. Reserve
        // host-owned state before any toolbox process can mount this directory.
        reserve_k3d_kubeconfig(&work)?;
    }
    for f in plan["files"].as_array().unwrap() {
        c.cancel.check()?;
        let rel = f["path"].as_str().unwrap();
        let dest = resources::destination(&bundle, rel)?;
        process::private_directory(dest.parent().unwrap())?;
        fs::copy(resources::destination(&cache, rel)?, dest).map_err(|_| fail("无法暂存已校验资源。", 500))?;
    }
    c.event("正在校验安装工作目录中的资源副本。");
    resources::prepare_files(&plan, &bundle, true, &c.cancel, &|m| c.event(m))?;
    let mut generated = Vec::new();
    let images = plan["images"].as_array().unwrap();
    if !images.is_empty() {
        process::private_directory(&bundle.join("images"))?;
        let enriched = images
            .iter()
            .map(|image| {
                let mut image = image.clone();
                let file = plan["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|f| f["path"] == image["file"])
                    .unwrap();
                image["size"] = file["size"].clone();
                image["sha256"] = file["sha256"].clone();
                if plan.get("releaseManifest").is_some() {
                    image["bootstrap"] = json!(plan["bootstrapImages"]
                        .as_array()
                        .unwrap()
                        .contains(&image["reference"]));
                }
                image
            })
            .collect::<Vec<_>>();
        write_managed(
            &bundle.join("images/install-manifest.json"),
            serde_json::to_string(&enriched).unwrap().as_bytes(),
        )?;
        generated.push("images/install-manifest.json".into());
        if !docker {
            for (name, repeat) in [("images/manifest.txt", false), ("images/nexus-names.txt", true)] {
                let data = enriched
                    .iter()
                    .filter(|i| i.get("bootstrap").is_none_or(|b| b == true))
                    .map(|i| {
                        let reference = i["reference"].as_str().unwrap();
                        if repeat {
                            format!("setup {reference} {reference}\n")
                        } else {
                            format!("setup {reference}\n")
                        }
                    })
                    .collect::<String>();
                write_managed(&bundle.join(name), data.as_bytes())?;
                generated.push(name.into());
            }
        }
    }
    if let Some(selection) = plan.get("installSelection") {
        process::private_directory(&bundle.join("nexus"))?;
        write_managed(
            &bundle.join("nexus/install-selection.json"),
            serde_json::to_string(selection).unwrap().as_bytes(),
        )?;
        generated.push("nexus/install-selection.json".into());
    }
    let mut sums = plan["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| !generated.contains(&f["path"].as_str().unwrap().to_owned()))
        .map(|f| format!("{}  {}", f["sha256"].as_str().unwrap(), f["path"].as_str().unwrap()))
        .collect::<Vec<_>>();
    for name in &generated {
        sums.push(format!(
            "{}  {name}",
            config::hash(&resources::read(&bundle.join(name), 4 * 1024 * 1024)?)
        ));
    }
    write_managed(&bundle.join("SHA256SUMS"), (sums.join("\n") + "\n").as_bytes())?;
    target["workDir"] = json!(work);
    target["environment"] = json!(c.directory.join("input.yaml"));
    target["kubeconfig"] = json!(work.join("state/kubeconfig"));
    let tool_env = if bundled {
        native_tools(&plan, &target, &bundle, c)?
    } else {
        BTreeMap::new()
    };
    if let Some(kubectl) = tool_env.get("CHENTU_KUBECTL") {
        target["kubectlPath"] = json!(kubectl);
    }
    if !docker {
        require_tools(&["curl"], &tool_env)?;
        process::text(
            Command::new("curl").arg("--version").envs(&tool_env),
            b"",
            10,
            8192,
            &c.cancel,
        )
        .map_err(|_| {
            fail(
                "管理机 curl 无法运行。请检查 CLI 主机或离线安装包的 curl 及其运行库后重试；尚未创建集群。",
                400,
            )
        })?;
    }
    let cluster = format!("apeiron-{}", &config::hash(work.to_string_lossy().as_bytes())[..12]);
    let node = if docker {
        format!("k3d-{cluster}-server-0")
    } else {
        nodes.iter().find(|n| n["role"] == "server").unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let domain = i["domain"].as_str().unwrap();
    let work_value = if docker {
        "/work/state".into()
    } else {
        work.join("state").to_string_lossy().into_owned()
    };
    let bundle_value = if docker {
        "/work/bundle".into()
    } else {
        bundle.to_string_lossy().into_owned()
    };
    let mut values = plan["environment"].clone();
    replace(
        &mut values,
        &[
            ("__DOMAIN__", domain.into()),
            ("__REGISTRY__", format!("registry.{domain}")),
            ("__NODE__", node.clone()),
            ("__WORK__", work_value.clone()),
            ("__BUNDLE__", bundle_value.clone()),
            ("__ARCH__", arch.clone()),
        ],
    );
    resources::merge(
        &mut values,
        &json!({"topology":i["topology"],"domain":domain,"registry":format!("registry.{domain}"),"node":node,"work":work_value,"bundle":bundle_value,"architecture":arch,"publicHttpsPort":i["httpsPort"],"publicHttpPort":i["httpPort"]}),
    )?;
    if plan.get("releaseManifest").is_some() {
        values["nexusFirst"] = json!(true);
    } else {
        values.as_object_mut().unwrap().remove("nexusFirst");
    }
    if i["publicAccess"].is_object() {
        values["publicAccess"] = json!({"mode":i["publicAccess"]["mode"]});
    }
    if values["releases"].is_null() {
        values["releases"] = json!({});
    }
    if !docker {
        resources::merge(
            &mut values,
            &json!({"releases":{"seaweedfs":{"values":{"nodes":[node],"replicas":1,"replication":"000","peers":"seaweedfs-0.seaweedfs-peers.seaweedfs.svc.cluster.local:9333"}},"longhorn":{"enabled":false,"values":{}}},"storageClass":"local-path"}),
        )?;
    }
    Ok(Prepared {
        target,
        values,
        plan,
        nodes,
        bundle,
        cluster,
        arch,
        generated,
        tool_env,
    })
}
// Replace only generated files under this installation's private work directory.
fn write_managed(path: &Path, data: &[u8]) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| !m.is_file()) {
        return Err(fail("生成文件不能是链接或特殊文件。", 400));
    }
    let tmp = path.with_extension(format!("{:016x}.tmp", rand::random::<u64>()));
    private_write(&tmp, data)?;
    fs::rename(&tmp, path).map_err(|_| fail("无法保存生成文件。", 500))
}
fn reserve_k3d_kubeconfig(work: &Path) -> Result<()> {
    let path = resources::destination(work, "state/kubeconfig")?;
    process::private_directory(path.parent().unwrap())?;
    if path.exists() {
        // A retry must retain the original credentials; reject unreadable or
        // special files instead of replacing an existing cluster's identity.
        resources::read(&path, 4 * 1024 * 1024)?;
    } else {
        // The toolbox writes into this inode. On native Linux it would otherwise
        // create a root-owned 0600 file that the unprivileged CLI cannot read.
        private_write(&path, b"")?;
    }
    Ok(())
}
fn native_tools(plan: &Value, target: &Value, bundle: &Path, c: &Context) -> Result<BTreeMap<String, String>> {
    let files = plan["files"].as_array().unwrap();
    let paths = files.iter().map(|f| f["path"].as_str().unwrap()).collect::<Vec<_>>();
    let python = paths
        .iter()
        .filter(|p| config::matches(r"^python/cpython-3\.12[.-][a-zA-Z0-9._-]+\.tar\.gz$", p))
        .collect::<Vec<_>>();
    let mut binaries = vec!["uv", "helm", "helmfile", "age", "age-keygen", "s5cmd"];
    if plan.get("releaseManifest").is_some() {
        binaries.push("crane");
    }
    if python.len() != 1
        || binaries.iter().any(|n| !paths.contains(&format!("bin/{n}").as_str()))
        || !paths.contains(&"k3s/k3s")
        || !paths
            .iter()
            .any(|p| config::matches(r"^cli/ansible_core-[^/]+\.whl$", p))
    {
        return Err(fail("原生安装包缺少 Python、Ansible 或部署工具。", 400));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for p in binaries
            .iter()
            .map(|n| format!("bin/{n}"))
            .chain(std::iter::once("k3s/k3s".into()))
        {
            fs::set_permissions(bundle.join(p), fs::Permissions::from_mode(0o700))
                .map_err(|_| fail("无法设置工具权限。", 500))?;
        }
    }
    let runtime = path(target, "workDir").join("operator");
    process::private_directory(&runtime)?;
    let mut env = BTreeMap::from([
        (
            "CHENTU_KUBECTL".into(),
            runtime.join("venv/bin/kubectl").to_string_lossy().into_owned(),
        ),
        (
            "CHENTU_HELM".into(),
            bundle.join("bin/helm").to_string_lossy().into_owned(),
        ),
        ("UV_OFFLINE".into(), "1".into()),
        ("UV_PYTHON_DOWNLOADS".into(), "never".into()),
        (
            "PATH".into(),
            format!(
                "{}:{}:{}",
                runtime.join("venv/bin").display(),
                bundle.join("bin").display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        ),
        (
            "CHENTU_PYTHON".into(),
            runtime.join("venv/bin/python3").to_string_lossy().into_owned(),
        ),
    ]);
    c.checked(
        "tar",
        strings(&[
            "-xzf",
            &bundle.join(python[0]).to_string_lossy(),
            "-C",
            &runtime.to_string_lossy(),
        ]),
        target,
        &env,
        "准备 Python 运行环境",
    )?;
    let uv = bundle.join("bin/uv");
    c.checked(
        &uv.to_string_lossy(),
        strings(&[
            "venv",
            "--offline",
            "--python",
            &runtime.join("python/bin/python3.12").to_string_lossy(),
            &runtime.join("venv").to_string_lossy(),
        ]),
        target,
        &env,
        "准备部署工具环境",
    )?;
    c.checked(
        &uv.to_string_lossy(),
        strings(&[
            "pip",
            "install",
            "--offline",
            "--no-index",
            "--find-links",
            &bundle.join("cli").to_string_lossy(),
            "--python",
            env.get("CHENTU_PYTHON").unwrap(),
            "chentu",
            "ansible-core",
        ]),
        target,
        &env,
        "安装离线部署工具",
    )?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(bundle.join("k3s/k3s"), runtime.join("venv/bin/kubectl"))
        .map_err(|_| fail("无法配置 kubectl。", 500))?;
    for tool in ["ansible-playbook", "helmfile"] {
        c.checked(tool, strings(&["--version"]), target, &env, "检查部署工具")?;
    }
    Ok(std::mem::take(&mut env))
}
pub fn bootstrap(target: &mut Value, c: &Context, p: &Prepared) -> Result<BTreeMap<String, String>> {
    let root = path(target, "root");
    let environment = path(target, "environment");
    let i = target["installation"].clone();
    let mut env = p.tool_env.clone();
    if target["runner"] != "docker" {
        // start.sh exports K3S_DATA_DIR for the wizard's own cache; the native
        // deployment must use the standard paths that prepare-server.sh and
        // hosts.yaml pin (token fetch, airgap staging, kubeconfig symlink).
        env.insert("K3S_DATA_DIR".into(), "/var/lib/rancher/k3s".into());
    }
    for (k, v) in [
        ("CHENTU_ROOT", root.to_string_lossy().into_owned()),
        ("CHENTU_ENV", environment.to_string_lossy().into_owned()),
        ("KUBECONFIG", target["kubeconfig"].as_str().unwrap().into()),
        ("PYTHONPATH", root.join("cli/src").to_string_lossy().into_owned()),
        ("UV_OFFLINE", "1".into()),
        ("UV_PYTHON_DOWNLOADS", "never".into()),
        ("HELMFILE_NO_COLOR", "true".into()),
        ("HELMFILE_LOG_LEVEL", "info".into()),
    ] {
        env.insert(k.into(), v);
    }
    if target["runner"] == "docker" {
        let container = format!(
            "{}-setup-{}",
            p.cluster,
            &config::hash(c.directory.to_string_lossy().as_bytes())[..12]
        );
        env.insert("LAB_CONTAINER".into(), container.clone());
        let t = &p.plan["target"];
        let image = if let Some(archive) = t["toolboxArchive"].as_str() {
            let image = archive_toolbox_image(t)?;
            c.checked(
                "docker",
                strings(&["load", "-i", &p.bundle.join(archive).to_string_lossy()]),
                target,
                &env,
                "导入已校验工具箱",
            )?;
            // The archive SHA-256 and lineage (verified by the publisher) already
            // guarantee the toolbox content. Docker's image ID varies between
            // store implementations (classic returns config digest, containerd
            // returns manifest digest), so comparing IDs is unreliable.
            process::text(
                Command::new("docker").args(["image", "inspect", "--format", "{{.Id}}", image]),
                b"",
                20,
                4096,
                &c.cancel,
            )
            .map_err(|_| {
                fail(
                    format!("工具箱已导入，但 Docker 无法找到安装包声明的镜像：{image}。"),
                    400,
                )
            })?;
            if let Some(archives) = t["dockerArchives"].as_array() {
                for a in archives {
                    c.checked(
                        "docker",
                        strings(&[
                            "load",
                            "-i",
                            &p.bundle.join(a["file"].as_str().unwrap()).to_string_lossy(),
                        ]),
                        target,
                        &env,
                        "导入已校验系统镜像",
                    )?;
                    for img in a["images"].as_array().unwrap() {
                        let observed = process::text(
                            Command::new("docker").args([
                                "image",
                                "inspect",
                                "--format",
                                "{{.Id}}",
                                img["name"].as_str().unwrap(),
                            ]),
                            b"",
                            20,
                            4096,
                            &c.cancel,
                        )?;
                        if !system_image_identity(
                            &p.bundle.join(a["file"].as_str().unwrap()),
                            img["name"].as_str().unwrap(),
                            img["id"].as_str().unwrap(),
                            observed.trim(),
                            &p.arch,
                        )
                        .unwrap_or(false)
                        {
                            return Err(fail(
                                format!("系统镜像身份与已校验归档不匹配：{}。", img["name"].as_str().unwrap()),
                                400,
                            ));
                        }
                    }
                }
            }
            image.to_owned()
        } else {
            let image = t["toolboxImage"].as_str().unwrap_or("");
            if !config::matches(r"^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*@sha256:[a-f0-9]{64}$", image) {
                return Err(fail("工具箱镜像缺少固定 digest。", 400));
            }
            c.checked("docker", strings(&["pull", image]), target, &env, "下载固定版本工具箱")?;
            image.to_owned()
        };
        target["image"] = json!(image);
        c.event(&cluster::check(&path(target, "workDir"), &p.cluster, &i, &c.cancel)?);
        for warning in cluster::coexistence("single-k3d", cluster::native_k3s_present(), &instances::Host)?["warnings"]
            .as_array()
            .unwrap()
        {
            c.event(warning.as_str().unwrap_or(""));
        }
        write_managed(
            &path(target, "workDir").join("installation.json"),
            json!({"cluster":p.cluster,"domain":i["domain"]}).to_string().as_bytes(),
        )?;
        for (k, v) in [
            ("LAB_ENV", environment.to_string_lossy().into_owned()),
            ("LAB_PLATFORM", format!("linux/{}", p.arch)),
            ("LAB_EXPECT_ARCH", p.arch.clone()),
            ("LAB_WORK_DIR", target["workDir"].as_str().unwrap().into()),
            ("LAB_IMAGE", image.clone()),
            ("LAB_CLUSTER", p.cluster.clone()),
            ("LAB_REGISTRY", format!("k3d-{}-reg:5000", p.cluster)),
            ("LAB_HTTP_PORT", i["httpPort"].to_string()),
            ("LAB_HTTPS_PORT", i["httpsPort"].to_string()),
        ] {
            env.insert(k.into(), v);
        }
        c.checked(
            "docker",
            strings(&[
                "run",
                "--rm",
                "--name",
                &container,
                "--network=none",
                "--entrypoint",
                "/opt/chentu-venv/bin/python",
                "-v",
                &format!("{}:/repo:ro", root.display()),
                "-v",
                &format!("{}:/environment.yaml:ro", environment.display()),
                "-v",
                &format!("{}:/work", target["workDir"].as_str().unwrap()),
                "-e",
                "CHENTU_ENV=/environment.yaml",
                "-e",
                "PYTHONPATH=/repo/cli/src",
                &image,
                "/repo/deploy/helmfile/scripts/check.py",
            ]),
            target,
            &env,
            "校验应用配置",
        )?;
        let original = String::from_utf8(resources::read(&root.join("tests/lab/helmfile.sh"), 1024 * 1024)?)
            .map_err(|_| fail("启动入口格式无效。", 400))?;
        let root_line = "REPO=$(cd \"$(dirname \"$0\")/../..\" && pwd)";
        let env_line = "-e CHENTU_ENV=/environment.yaml -e KUBECONFIG=/work/state/kubeconfig";
        if !original.contains(root_line) || !original.contains(env_line) {
            return Err(fail("发行包 K3d 启动入口不兼容基础镜像预装。", 400));
        }
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let launcher = c.directory.join("k3d-prepare.sh");
        private_write(
            &launcher,
            original
                .replacen(root_line, &format!("REPO={}", quote(&root.to_string_lossy())), 1)
                .replacen(
                    env_line,
                    &format!(
                        "{env_line}\n  -e LAB_K3S_AIRGAP={}\n  -v {}",
                        quote(&p.bundle.join(t["k3sAirgap"].as_str().unwrap()).to_string_lossy()),
                        quote(&format!("{}:{}:ro", p.bundle.display(), p.bundle.display()))
                    ),
                    1,
                )
                .as_bytes(),
        )?;
        c.checked(
            "bash",
            strings(&[&launcher.to_string_lossy(), "prepare"]),
            target,
            &env,
            "创建或复用 K3d 测试集群",
        )?;
        c.checked(
            "docker",
            strings(&[
                "exec",
                &format!("k3d-{}-server-0", p.cluster),
                "kubectl",
                "-n",
                "kube-system",
                "rollout",
                "status",
                "deployment/traefik",
                "--timeout=600s",
            ]),
            target,
            &env,
            "等待 Traefik 就绪",
        )?;
        return Ok(env);
    }
    c.checked(
        "python3",
        strings(&[&root.join("deploy/helmfile/scripts/check.py").to_string_lossy()]),
        target,
        &env,
        "校验应用配置",
    )?;
    let multi = i["topology"] == "multi-k3s";
    let bundle = if multi {
        format!(
            "/opt/chentu/bundles/{}",
            &config::hash(c.directory.to_string_lossy().as_bytes())[..12]
        )
    } else {
        p.bundle.to_string_lossy().into_owned()
    };
    let inventory = c.directory.join("inventory.yaml");
    env.insert("CHENTU_INVENTORY".into(), inventory.to_string_lossy().into_owned());
    let hosts = |role: &str| {
        p.nodes
            .iter()
            .filter(|n| n["role"] == role)
            .map(|n| {
                let mut v = json!({"ansible_host":n["host"],"chentu_node_ip":n["address"]});
                if !multi {
                    v["ansible_connection"] = json!("local");
                }
                (n["name"].as_str().unwrap().to_owned(), v)
            })
            .collect::<serde_json::Map<_, _>>()
    };
    let mut vars = json!({"ansible_become":true,"ansible_ssh_common_args":"-o BatchMode=yes -o StrictHostKeyChecking=yes -o ConnectTimeout=8","ansible_port":i["sshPort"],"chentu_domain":i["domain"],"chentu_architecture":p.arch,"chentu_bundle":bundle,"chentu_remote_repo":"/opt/chentu/setup","chentu_kubeconfig":target["kubeconfig"]});
    for (from, to) in [("sshUser", "ansible_user"), ("sshKey", "ansible_ssh_private_key_file")] {
        if !i[from].as_str().unwrap_or("").is_empty() {
            vars[to] = i[from].clone();
        }
    }
    resources::write_yaml(
        &inventory,
        &json!({"all":{"vars":vars,"children":{"server":{"hosts":hosts("server")},"agent":{"hosts":hosts("agent")}}}}),
    )?;
    let mut tasks = Vec::new();
    let private_dns = !i["publicAccess"].is_object();
    let mut plays = Vec::new();
    if private_dns {
        let script = root.join("bootstrap/private_dns.py");
        let names_path = c.directory.join("private-dns-names.json");
        c.checked(
            "python3",
            strings(&[
                &script.to_string_lossy(),
                "plan",
                "--domain",
                i["domain"].as_str().unwrap(),
                "--address",
                i["entryIp"].as_str().unwrap(),
                "--output",
                &names_path.to_string_lossy(),
            ]),
            target,
            &env,
            "生成内部解析清单",
        )?;
        let names = private_names(&names_path, i["domain"].as_str().unwrap())?;
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let command = format!(
            "{} hosts --domain {} --address {} --names {}",
            quote(&script.to_string_lossy()),
            quote(i["domain"].as_str().unwrap()),
            quote(i["entryIp"].as_str().unwrap()),
            quote(&names.join(","))
        );
        let prepare_dns = json!({"name":"Establish and verify managed private resolution","ansible.builtin.script":{"cmd":command,"executable":"python3"},"register":"private_dns_result","changed_when":"private_dns_result.stdout | trim == 'changed'","timeout":60});
        plays.push(json!({"name":"Prepare operator private resolution","hosts":"localhost","connection":"local","become":true,"become_flags":"-n","gather_facts":false,"tasks":[prepare_dns.clone()]}));
        tasks.push(prepare_dns);
    }
    if multi {
        tasks.push(json!({"name":"Check peer SSH reachability","ansible.builtin.wait_for":{"host":"{{ item }}","port":i["sshPort"],"timeout":8,"connect_timeout":3},"loop":p.nodes.iter().map(|n|n["address"].clone()).collect::<Vec<_>>()}));
        let mut files = p.plan["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        files.extend(p.generated.clone());
        files.push("SHA256SUMS".into());
        let dirs = files
            .iter()
            .map(|f| Path::new(f).parent().unwrap().to_string_lossy().into_owned())
            .collect::<std::collections::BTreeSet<_>>();
        tasks.push(json!({"name":"Create resource directories","ansible.builtin.file":{"path":"{{ chentu_bundle }}/{{ item }}","state":"directory","mode":"0700"},"loop":dirs}));
        tasks.push(json!({"name":"Copy verified resources","ansible.builtin.copy":{"src":format!("{}/{{{{ item }}}}",p.bundle.display()),"dest":"{{ chentu_bundle }}/{{ item }}","mode":"0600"},"loop":files}));
    }
    tasks.push(json!({"name":"Verify staged bundle on each node","ansible.builtin.command":{"argv":["sha256sum","--check","SHA256SUMS"],"chdir":"{{ chentu_bundle }}"},"changed_when":false}));
    let preflight = c.directory.join("prepare-hosts.yaml");
    plays.push(json!({"name":"Prepare verified installation resources","hosts":"server:agent","become":true,"gather_facts":false,"tasks":tasks}));
    resources::write_yaml(&preflight, &json!(plays))?;
    c.checked(
        "ansible-playbook",
        strings(&["-i", &inventory.to_string_lossy(), &preflight.to_string_lossy()]),
        target,
        &env,
        "检查节点互联并准备安装包",
    )?;
    c.checked(
        "ansible-playbook",
        strings(&[
            "-i",
            &inventory.to_string_lossy(),
            &root.join("bootstrap/hosts.yaml").to_string_lossy(),
        ]),
        target,
        &env,
        "安装 K3s 并生成 kubeconfig",
    )?;
    public_access("prepare", target, c, &env)?;
    Ok(env)
}
// Runs only after the cluster identity checkpoint is durable. A transient
// Kubernetes/CoreDNS failure must be retryable without provisioning K3s again.
pub fn private_cluster_dns(target: &Value, c: &Context, env: &BTreeMap<String, String>) -> Result<()> {
    let i = &target["installation"];
    if target["runner"] != "native" || !i.is_object() || i["publicAccess"].is_object() {
        return Ok(());
    }
    c.checked(
        "python3",
        strings(&[
            &path(target, "root").join("bootstrap/private_dns.py").to_string_lossy(),
            "cluster",
            "--domain",
            i["domain"].as_str().unwrap(),
            "--address",
            i["entryIp"].as_str().unwrap(),
        ]),
        target,
        env,
        "配置集群内部解析",
    )
}
fn private_names(path: &Path, domain: &str) -> Result<Vec<String>> {
    let names = resources::json_file(path, 65536)?;
    let names = names
        .as_array()
        .filter(|names| !names.is_empty() && names.len() <= 1024)
        .ok_or_else(|| fail("内部解析清单无效。", 400))?;
    let suffix = format!(".{domain}");
    let mut unique = std::collections::BTreeSet::new();
    for name in names {
        let name = name
            .as_str()
            .filter(|name| config::safe_domain(name))
            .ok_or_else(|| fail("内部解析名称无效。", 400))?;
        let label = name
            .strip_suffix(&suffix)
            .filter(|label| !label.is_empty() && !label.contains('.'))
            .ok_or_else(|| fail("内部解析名称不属于本次安装域名。", 400))?;
        if label.is_empty() || !unique.insert(name.to_owned()) {
            return Err(fail("内部解析清单包含重复名称。", 400));
        }
    }
    for service in ["apeiron", "iam", "nexus", "registry"] {
        if !unique.contains(&format!("{service}.{domain}")) {
            return Err(fail("内部解析清单缺少必要服务。", 400));
        }
    }
    Ok(unique.into_iter().collect())
}
pub fn public_access(phase: &str, target: &Value, c: &Context, env: &BTreeMap<String, String>) -> Result<()> {
    let i = &target["installation"];
    if !i["publicAccess"].is_object() {
        return Ok(());
    }
    let root = path(target, "root");
    let script = root.join("bootstrap/public_access.py");
    if !script.is_file() {
        return Err(fail("此发行包未包含 Caddy 公网入口。", 400));
    }
    let mut access = i["publicAccess"].clone();
    if let Some(id) = access["pairingId"].as_str().map(str::to_owned) {
        pairing::scope(&c.config, &id, i)?;
        access["pairing"] = json!({"id":id,"directory":pairing::directory(&c.config)});
    }
    if phase == "check"
        && discovery::dns(
            &json!({"domain":i["domain"],"entryIp":access["publicIp"],"local":false}),
            true,
            &c.cancel,
        )?["passed"]
            != true
    {
        return Err(fail("公网 DNS 检查未通过，尚未创建集群。", 400));
    }
    let plan = c.directory.join("public-access.json");
    write_managed(&plan,json!({"topology":i["topology"],"domain":i["domain"],"access":access,"ca":if target["workDir"].as_str().unwrap_or("").is_empty(){c.directory.join("state/chentu-ca.crt")}else{path(target,"workDir").join("state/chentu-ca.crt")}}).to_string().as_bytes())?;
    let python = env.get("CHENTU_PYTHON").map(String::as_str).unwrap_or("python3");
    let args = strings(&[&script.to_string_lossy(), phase, &plan.to_string_lossy()]);
    #[cfg(unix)]
    let is_root = unsafe { libc::getuid() } == 0;
    #[cfg(not(unix))]
    let is_root = false;
    if is_root {
        c.run(python, &args, &root, env)
    } else {
        let mut sudo = strings(&[
            "-n",
            "env",
            &format!("KUBECONFIG={}", target["kubeconfig"].as_str().unwrap_or("")),
            &format!(
                "PATH={}",
                env.get("PATH")
                    .cloned()
                    .or_else(|| std::env::var("PATH").ok())
                    .unwrap_or_default()
            ),
        ]);
        if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
            sudo.push(format!("SSH_AUTH_SOCK={sock}"));
        }
        sudo.push(python.into());
        sudo.extend(args);
        c.run("sudo", &sudo, &root, env)
    }
}

#[cfg(all(test, unix))]
mod tool_contracts {
    use super::*;
    use crate::app::contracts::Temp;
    #[test]
    fn k3d_kubeconfig_is_private_host_owned_and_preserves_existing_identity() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        let temp = Temp::new();
        reserve_k3d_kubeconfig(&temp.0).unwrap();
        let path = temp.0.join("state/kubeconfig");
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(metadata.uid(), fs::metadata(&temp.0).unwrap().uid());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        fs::write(&path, b"existing cluster identity").unwrap();
        reserve_k3d_kubeconfig(&temp.0).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"existing cluster identity");
        assert_eq!(fs::metadata(&path).unwrap().ino(), metadata.ino());
        fs::remove_file(&path).unwrap();
        symlink(temp.0.join("outside"), &path).unwrap();
        assert!(reserve_k3d_kubeconfig(&temp.0).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(reserve_k3d_kubeconfig(&temp.0).is_err());
    }
    #[test]
    fn system_archives_bind_classic_and_containerd_ids_without_accepting_foreign_or_corrupt_configs() {
        let temp = Temp::new();
        for indexed in [false, true] {
            for damage in [
                "",
                "config-bytes",
                "manifest-bytes",
                "config-link",
                "config-size",
                "tag",
                "ambiguous",
            ] {
                if !indexed && damage == "ambiguous" {
                    continue;
                }
                let config =
                    br#"{"os":"linux","architecture":"arm64","rootfs":{"type":"layers","diff_ids":[]}}"#.to_vec();
                let config_digest = format!("sha256:{}", config::hash(&config));
                let config_path = format!("blobs/sha256/{}", &config_digest[7..]);
                let manifest = json!({
                    "schemaVersion":2,
                    "config":{"digest":if damage=="config-link" {format!("sha256:{}", "a".repeat(64))} else {config_digest.clone()},
                              "size":config.len()+usize::from(damage=="config-size")},
                    "layers":[]
                })
                .to_string()
                .into_bytes();
                let manifest_digest = format!("sha256:{}", config::hash(&manifest));
                let descriptor = json!({"digest":manifest_digest,"size":manifest.len(),
                    "platform":{"os":"linux","architecture":"arm64"}});
                let index = json!({"schemaVersion":2,"manifests":if damage=="ambiguous" {
                    json!([descriptor.clone(),descriptor])
                } else {json!([descriptor])}})
                .to_string()
                .into_bytes();
                let index_digest = format!("sha256:{}", config::hash(&index));
                let expected = if indexed { &index_digest } else { &manifest_digest };
                let compat = json!([{"Config":config_path,"RepoTags":[if damage=="tag" {"foreign:1"} else {"registry:2"}],"Layers":[]}])
                    .to_string().into_bytes();
                let archive_path = temp.0.join("system.tar");
                let mut archive = tar::Builder::new(fs::File::create(&archive_path).unwrap());
                for (path, mut data) in [
                    ("manifest.json".to_owned(), compat),
                    (config_path, config),
                    (format!("blobs/sha256/{}", &manifest_digest[7..]), manifest),
                    (format!("blobs/sha256/{}", &index_digest[7..]), index),
                ] {
                    if (damage == "config-bytes" && path.ends_with(&config_digest[7..]))
                        || (damage == "manifest-bytes" && path.ends_with(&manifest_digest[7..]))
                    {
                        data.push(b' ');
                    }
                    let mut header = tar::Header::new_gnu();
                    header.set_size(data.len() as u64);
                    header.set_mode(0o644);
                    header.set_cksum();
                    archive.append_data(&mut header, path, data.as_slice()).unwrap();
                }
                archive.finish().unwrap();
                assert_eq!(
                    system_image_identity(&archive_path, "registry:2", expected, &config_digest, "arm64").unwrap(),
                    damage.is_empty(),
                    "indexed={indexed} damage={damage}"
                );
                assert!(!system_image_identity(
                    &archive_path,
                    "registry:2",
                    expected,
                    &format!("sha256:{}", "f".repeat(64)),
                    "arm64"
                )
                .unwrap());
                assert!(
                    !system_image_identity(&archive_path, "registry:2", expected, &config_digest, "amd64").unwrap()
                );
                assert!(system_image_identity(&archive_path, "registry:2", expected, expected, "arm64").unwrap());
            }
        }
    }
    #[test]
    fn published_rc5_toolbox_without_tag_retains_legacy_image_reference() {
        let legacy = json!({
            "toolboxArchive": "tools/chentu-toolbox-arm64.tar",
            "toolboxImageId": "sha256:5da6f98d803beae5199eae264650c5b5998e524745a5e5322d05d318662b6028"
        });
        assert_eq!(
            archive_toolbox_image(&legacy).unwrap(),
            legacy["toolboxImageId"].as_str().unwrap()
        );
        let mut named = legacy.clone();
        named["toolboxImage"] = json!("chentu-lab:spark-534c491");
        assert_eq!(archive_toolbox_image(&named).unwrap(), "chentu-lab:spark-534c491");
        assert!(archive_toolbox_image(&json!({"toolboxArchive":"tools/toolbox.tar"})).is_err());
    }
    #[test]
    fn offline_operator_check_uses_original_bundle_and_never_installs() {
        let temp = Temp::new();
        let c = Context::fixture(&temp.0);
        let root = temp.0.join("chentu");
        temp.write("chentu/bootstrap/prepare-operator.py", "raise SystemExit(17)\n");
        let target = json!({"root":root,"offline":true,"bundleDir":temp.0});
        let error = check_offline_operator(&target, &c).unwrap_err();
        assert!(error.message.contains("sudo python3") && error.message.contains("--install 后重试"));
        let log = fs::read_to_string(temp.0.join("install.log")).unwrap();
        assert!(log.contains("退出码 17"), "{log}");
        // If this function invoked --install, this fixture would fail the test
        // through an observable side effect instead of just its read-only exit.
        temp.write("chentu/bootstrap/prepare-operator.py", format!(
            "import sys\nfrom pathlib import Path\nassert sys.argv[1:] == ['--bundle', {:?}, '--check']\nPath({:?}).write_text('checked')\n",
            temp.0.to_string_lossy(), temp.0.join("operator-checked").to_string_lossy()));
        check_offline_operator(&target, &c).unwrap();
        assert_eq!(fs::read_to_string(temp.0.join("operator-checked")).unwrap(), "checked");
    }
    #[test]
    fn private_dns_plan_rejects_foreign_nested_duplicate_and_missing_service_names() {
        let temp = Temp::new();
        let valid = json!([
            "apeiron.example.internal",
            "iam.example.internal",
            "nexus.example.internal",
            "registry.example.internal"
        ]);
        let path = temp.write("names.json", valid.to_string());
        assert_eq!(private_names(&path, "example.internal").unwrap().len(), 4);
        for extra in [
            "foreign.example.org",
            "nested.ops.example.internal",
            "apeiron.example.internal",
            "bad;command.example.internal",
        ] {
            let mut names = valid.clone();
            names.as_array_mut().unwrap().push(json!(extra));
            temp.write("names.json", names.to_string());
            assert!(private_names(&path, "example.internal").is_err(), "{extra}");
        }
        temp.write(
            "names.json",
            json!(["apeiron.example.internal", "iam.example.internal"]).to_string(),
        );
        assert!(private_names(&path, "example.internal").is_err());
    }
    #[test]
    fn management_preflight_reports_all_missing_executables_and_accepts_bundled_curl() {
        use std::os::unix::fs::PermissionsExt;
        let temp = Temp::new();
        let env = BTreeMap::from([("PATH".into(), temp.0.to_string_lossy().into_owned())]);
        let error = require_tools(&["curl", "helm"], &env).unwrap_err();
        assert!(error.message.contains("curl、helm") && error.message.contains("尚未创建集群"));
        let curl = temp.write("curl", "#!/bin/sh\nexit 0\n");
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            require_tools(&["curl"], &env).is_err(),
            "a nonexecutable file is not a tool"
        );
        fs::set_permissions(curl, fs::Permissions::from_mode(0o700)).unwrap();
        require_tools(&["curl"], &env).unwrap();
        assert!(require_tools(&["curl", "helm"], &env).is_err());
    }
    #[test]
    fn native_tools_require_complete_verified_set_and_install_offline_with_failures_propagated() {
        use std::os::unix::fs::PermissionsExt;
        let temp = Temp::new();
        let c = Context::fixture(&temp.0);
        let bundle = temp.0.join("bundle");
        let target = json!({"root":temp.0,"workDir":temp.0});
        assert!(native_tools(&json!({"files":[]}), &target, &bundle, &c)
            .unwrap_err()
            .message
            .contains("缺少"));
        let paths = [
            "bin/uv",
            "bin/helm",
            "bin/helmfile",
            "bin/age",
            "bin/age-keygen",
            "bin/s5cmd",
            "k3s/k3s",
            "python/cpython-3.12.14-aarch64.tar.gz",
            "cli/ansible_core-2.19.3-py3-none-any.whl",
        ];
        for path in paths {
            temp.write(&format!("bundle/{path}"), "fixture");
        }
        temp.write("python/bin/python3.12", "fixture");
        assert!(std::process::Command::new("tar")
            .args(["-czf"])
            .arg(bundle.join(paths[7]))
            .arg("-C")
            .arg(&temp.0)
            .arg("python")
            .status()
            .unwrap()
            .success());
        temp.write(
            "bundle/bin/uv",
            r#"#!/bin/sh
[ "$UV_OFFLINE" = 1 ] && [ "$UV_PYTHON_DOWNLOADS" = never ] || exit 42
printf '%s\n' "$*" >> "$PWD/calls"
[ ! -f "$PWD/fail" ] || exit 17
mkdir -p "$PWD/operator/venv/bin"
printf '#!/bin/sh\nexit 0\n' > "$PWD/operator/venv/bin/ansible-playbook"
chmod 700 "$PWD/operator/venv/bin/ansible-playbook"
"#,
        );
        temp.write("bundle/bin/helmfile", "#!/bin/sh\nexit 0\n");
        let plan = json!({"files":paths.map(|p|json!({"path":p}))});
        let env = native_tools(&plan, &target, &bundle, &c).unwrap();
        assert_eq!(
            env["PATH"].split(':').next().unwrap(),
            temp.0.join("operator/venv/bin").to_str().unwrap()
        );
        assert_eq!(
            env["CHENTU_PYTHON"],
            temp.0.join("operator/venv/bin/python3").to_str().unwrap()
        );
        let calls = fs::read_to_string(temp.0.join("calls")).unwrap();
        for arg in ["--no-index", "--offline", bundle.join("cli").to_str().unwrap()] {
            assert!(calls.contains(arg));
        }
        assert_eq!(
            fs::read_link(temp.0.join("operator/venv/bin/kubectl")).unwrap(),
            bundle.join("k3s/k3s")
        );
        assert_eq!(
            fs::metadata(bundle.join("bin/helm")).unwrap().permissions().mode() & 0o777,
            0o700
        );
        temp.write("fail", "");
        assert!(native_tools(&plan, &target, &bundle, &c)
            .unwrap_err()
            .message
            .contains("退出码 17"));
    }
    #[test]
    fn native_bootstrap_checks_configuration_and_copies_verified_packages_before_host_installation() {
        let temp = Temp::new();
        let c = Context::fixture(&temp.0);
        for name in ["python3", "ansible-playbook"] {
            temp.tool(
                name,
                r#"#!/bin/sh
printf '%s %s\n' "${0##*/}" "$*" >> "$CAPTURE"
[ ! -f "$PWD/fail" ] || exit 17
if [ "${2:-}" = plan ]; then
    for argument do output="$argument"; done
    printf '%s' '["apeiron.example.internal","iam.example.internal","nexus.example.internal","registry.example.internal"]' > "$output"
fi
"#,
            );
        }
        let i = json!({"topology":"multi-k3s","domain":"example.internal","entryIp":"192.0.2.10","sshPort":22,"sshKey":"","sshUser":""});
        let mut target = json!({"runner":"native","root":temp.0,"workDir":temp.0,"environment":temp.0.join("environment.yaml"),"kubeconfig":temp.0.join("kubeconfig"),"installation":i});
        let p = Prepared {
            target: Value::Null,
            values: Value::Null,
            plan: json!({"files":[{"path":"core.bin"}]}),
            nodes: vec![
                json!({"host":"node-0","name":"node-0","address":"192.0.2.10","role":"server"}),
                json!({"host":"node-1","name":"node-1","address":"192.0.2.11","role":"agent"}),
            ],
            bundle: temp.0.clone(),
            cluster: "fixture".into(),
            arch: "amd64".into(),
            generated: vec![],
            tool_env: temp.env(),
        };
        bootstrap(&mut target, &c, &p).unwrap();
        let calls = fs::read_to_string(temp.0.join("capture")).unwrap();
        let lines = calls.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("python3 ") && lines[0].ends_with("/deploy/helmfile/scripts/check.py"));
        assert!(lines[1].contains("private_dns.py plan --domain example.internal --address 192.0.2.10 --output"));
        assert!(lines[2].starts_with("ansible-playbook ") && lines[2].ends_with("/prepare-hosts.yaml"));
        assert!(lines[3].ends_with("/bootstrap/hosts.yaml"));
        assert!(
            !calls.contains("private_dns.py cluster"),
            "cluster DNS belongs after the durable identity checkpoint"
        );
        let inventory = resources::yaml(&temp.0.join("inventory.yaml")).unwrap();
        assert!(inventory["all"]["children"]["server"]["hosts"]["node-0"].is_object());
        assert!(inventory["all"]["children"]["agent"]["hosts"]["node-1"].is_object());
        assert_eq!(inventory["all"]["vars"]["chentu_architecture"], "amd64");
        assert!(inventory["all"]["vars"]["ansible_ssh_common_args"]
            .as_str()
            .unwrap()
            .contains("StrictHostKeyChecking=yes"));
        let playbook = resources::yaml(&temp.0.join("prepare-hosts.yaml")).unwrap();
        assert_eq!(playbook[0]["hosts"], "localhost");
        assert_eq!(playbook[0]["connection"], "local");
        assert_eq!(playbook[0]["become"], true);
        assert_eq!(playbook[0]["become_flags"], "-n");
        assert_eq!(playbook[1]["hosts"], "server:agent");
        let task = &playbook[1]["tasks"][0];
        assert_eq!(task["ansible.builtin.script"]["executable"], "python3");
        assert!(task["ansible.builtin.script"]["cmd"]
            .as_str()
            .unwrap()
            .contains("hosts --domain 'example.internal' --address '192.0.2.10' --names"));
        assert_eq!(
            playbook[1]["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "Establish and verify managed private resolution",
                "Check peer SSH reachability",
                "Create resource directories",
                "Copy verified resources",
                "Verify staged bundle on each node"
            ]
        );
        temp.write("capture", "");
        temp.write("fail", "");
        assert!(bootstrap(&mut target, &c, &p)
            .unwrap_err()
            .message
            .contains("退出码 17"));
        assert_eq!(fs::read_to_string(temp.0.join("capture")).unwrap().lines().count(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(host: &str, role: &str) -> Value {
        json!({"host":host,"name":host,"address":"192.0.2.1","role":role})
    }
    #[test]
    fn hosted_native_node_rules_cover_both_native_topologies() {
        // 1. multi-k3s: odd server count >= 3, no agents.
        let three = [node("a", "server"), node("b", "server"), node("c", "server")];
        hosted_nodes("multi-k3s", &three).unwrap();
        let five = [&three[..], &[node("d", "server"), node("e", "server")]].concat();
        hosted_nodes("multi-k3s", &five).unwrap();
        for nodes in [
            vec![node("a", "server")],
            vec![node("a", "server"), node("b", "agent"), node("c", "agent")],
            vec![
                node("a", "server"),
                node("b", "server"),
                node("c", "server"),
                node("d", "server"),
            ],
            vec![
                node("a", "server"),
                node("b", "server"),
                node("c", "server"),
                node("d", "agent"),
            ],
        ] {
            assert!(hosted_nodes("multi-k3s", &nodes).is_err());
        }
        // 2. single-k3s: the management machine appended as localhost is the only node.
        hosted_nodes("single-k3s", &[node("localhost", "server")]).unwrap();
        assert!(hosted_nodes("single-k3s", &three).is_err());
        assert!(hosted_nodes("single-k3s", &[node("localhost", "server"), node("b", "agent")]).is_err());
        assert!(hosted_nodes("single-k3s", &[node("remote", "server")]).is_err());
        // 3. K3d never reaches the native rule; any other topology is refused.
        assert!(hosted_nodes("single-k3d", &[]).is_err());
    }
}
