use super::*;
use super::{
    deploy::{strings, Context},
    process,
};
use std::{fs, path::PathBuf, process::Command};
pub struct Prepared {
    pub target: Value,
    pub values: Value,
    plan: Value,
    nodes: Vec<Value>,
    bundle: PathBuf,
    cluster: String,
    arch: String,
    generated: Vec<String>,
    tool_env: BTreeMap<String, String>,
}
fn path(v: &Value, key: &str) -> PathBuf {
    PathBuf::from(v[key].as_str().unwrap_or(""))
}
fn present(tool: &str, env: &BTreeMap<String, String>) -> bool {
    let paths = env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    std::env::split_paths(&paths).any(|p| p.join(tool).is_file())
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
    let plan = resources::plan(&catalog, &key, &config["apps"])?;
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
    let bundled = !docker
        && plan["target"]["operatorTools"] == true
        && cfg!(target_os = "linux")
        && discovery::architecture(std::env::consts::ARCH) == arch;
    let mut tools = if docker {
        vec!["docker"]
    } else if bundled {
        vec!["tar", "python3"]
    } else {
        vec!["ansible-playbook", "helm", "helmfile", "python3"]
    };
    if i["topology"] == "multi-k3s" {
        tools.push("ssh");
    }
    for tool in tools {
        if !present(tool, &BTreeMap::new()) {
            return Err(fail(format!("管理机缺少 {tool}，请安装后重新检测。"), 400));
        }
    }
    if i["publicAccess"].is_object() {
        public_access("check", &target, c, &BTreeMap::new())?;
    } else if !docker
        && discovery::dns(
            &json!({"domain":i["domain"],"entryIp":i["entryIp"],"local":false}),
            false,
            &c.cancel,
        )?["passed"]
            != true
    {
        return Err(fail("管理机 DNS 检查未通过，尚未创建集群。", 400));
    }
    let cache = if target["bundleDir"].as_str().unwrap_or("").is_empty() {
        c.directory.parent().unwrap().join("resources")
    } else {
        path(&target, "bundleDir")
    }
    .join(catalog["resourceDirectory"].as_str().unwrap_or(""));
    resources::prepare_files(&plan, &cache, target["offline"] == true, &c.cancel, &|m| c.event(m))?;
    let work = if docker {
        cluster::identity(&c.config, i["domain"].as_str().unwrap()).0
    } else {
        c.directory.join("work")
    };
    let bundle = work.join("bundle");
    process::private_directory(&bundle)?;
    for f in plan["files"].as_array().unwrap() {
        c.cancel.check()?;
        let rel = f["path"].as_str().unwrap();
        let dest = resources::destination(&bundle, rel)?;
        process::private_directory(dest.parent().unwrap())?;
        fs::copy(resources::destination(&cache, rel)?, dest).map_err(|_| fail("无法暂存已校验资源。", 500))?;
    }
    resources::prepare_files(&plan, &bundle, true, &c.cancel, &|_| {})?;
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
                let data = images
                    .iter()
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
fn native_tools(plan: &Value, target: &Value, bundle: &Path, c: &Context) -> Result<BTreeMap<String, String>> {
    let files = plan["files"].as_array().unwrap();
    let paths = files.iter().map(|f| f["path"].as_str().unwrap()).collect::<Vec<_>>();
    let python = paths
        .iter()
        .filter(|p| config::matches(r"^python/cpython-3\.12[.-][a-zA-Z0-9._-]+\.tar\.gz$", p))
        .collect::<Vec<_>>();
    let binaries = ["uv", "helm", "helmfile", "age", "age-keygen", "s5cmd"];
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
            c.checked(
                "docker",
                strings(&["load", "-i", &p.bundle.join(archive).to_string_lossy()]),
                target,
                &env,
                "导入已校验工具箱",
            )?;
            let image = t["toolboxImageId"].as_str().unwrap();
            let observed = process::text(
                Command::new("docker").args(["image", "inspect", "--format", "{{.Id}}", image]),
                b"",
                20,
                4096,
                &c.cancel,
            )?;
            if observed.trim() != image {
                return Err(fail("工具箱镜像 ID 不匹配。", 400));
            }
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
                        if observed.trim() != img["id"].as_str().unwrap() {
                            return Err(fail("系统镜像 ID 不匹配。", 400));
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
    if !i["publicAccess"].is_object() {
        tasks.push(json!({"name":"Verify platform DNS on each node","ansible.builtin.command":{"argv":["python3","-c","import socket,sys; domain,ip=sys.argv[1:]; hosts=['apeiron.'+domain,'iam.'+domain]; assert all({a[4][0] for a in socket.getaddrinfo(h,443,type=socket.SOCK_STREAM)}=={ip} for h in hosts)",i["domain"],i["entryIp"]]},"changed_when":false,"async":15,"poll":1}));
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
    resources::write_yaml(
        &preflight,
        &json!([{"name":"Prepare verified installation resources","hosts":"server:agent","become":true,"gather_facts":false,"tasks":tasks}]),
    )?;
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
                "#!/bin/sh\nprintf '%s %s\\n' \"${0##*/}\" \"$*\" >> \"$CAPTURE\"\n[ ! -f \"$PWD/fail\" ] || exit 17\n",
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
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("python3 ") && lines[0].ends_with("/deploy/helmfile/scripts/check.py"));
        assert!(lines[1].starts_with("ansible-playbook ") && lines[1].ends_with("/prepare-hosts.yaml"));
        assert!(lines[2].ends_with("/bootstrap/hosts.yaml"));
        let inventory = resources::yaml(&temp.0.join("inventory.yaml")).unwrap();
        assert!(inventory["all"]["children"]["server"]["hosts"]["node-0"].is_object());
        assert!(inventory["all"]["children"]["agent"]["hosts"]["node-1"].is_object());
        assert_eq!(inventory["all"]["vars"]["chentu_architecture"], "amd64");
        assert!(inventory["all"]["vars"]["ansible_ssh_common_args"]
            .as_str()
            .unwrap()
            .contains("StrictHostKeyChecking=yes"));
        let playbook = resources::yaml(&temp.0.join("prepare-hosts.yaml")).unwrap();
        assert_eq!(
            playbook[0]["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "Verify platform DNS on each node",
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
