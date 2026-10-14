//! The formal release contract is validated before creating or modifying a cluster.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

const MANIFEST: &str = "setup/release-manifest.json";
fn invalid(detail: &str) -> Error {
    fail(format!("Nexus 发布清单不完整：{detail}；尚未修改集群。"), 400)
}
fn names(value: &Value, detail: &str) -> Result<BTreeSet<String>> {
    let mut result = BTreeSet::new();
    for name in value.as_array().ok_or_else(|| invalid(detail))? {
        let name = name.as_str().ok_or_else(|| invalid(detail))?;
        if !config::matches(r"^[A-Za-z0-9][A-Za-z0-9._/-]*$", name) || !result.insert(name.into()) {
            return Err(invalid(detail));
        }
    }
    Ok(result)
}
fn closure(components: &Value, roots: &BTreeSet<String>) -> Result<BTreeSet<String>> {
    fn visit(c: &Value, name: &str, active: &mut BTreeSet<String>, done: &mut BTreeSet<String>) -> Result<()> {
        if done.contains(name) {
            return Ok(());
        }
        if !active.insert(name.into()) {
            return Err(invalid("组件包含循环依赖"));
        }
        let component = c.get(name).ok_or_else(|| invalid("组件依赖不存在"))?;
        for dependency in names(&component["requires"], "组件依赖声明无效")? {
            visit(c, &dependency, active, done)?;
        }
        active.remove(name);
        done.insert(name.into());
        Ok(())
    }
    let mut done = BTreeSet::new();
    for name in roots {
        visit(components, name, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(done)
}
fn digest(value: &Value) -> bool {
    config::matches(r"^sha256:[a-f0-9]{64}$", value.as_str().unwrap_or(""))
}
fn text(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| !s.trim().is_empty() && s.len() < 512 && !s.contains(['\n', '\r']))
}
fn target_path(value: &Value, image: bool) -> bool {
    let Some(path) = value.as_str() else {
        return false;
    };
    let repository = if image {
        path.split_once(':').map_or(path, |(p, _)| p)
    } else {
        path
    };
    resources::safe_relative(repository)
        && (!image || !repository.split('/').next().unwrap_or("").contains('.'))
        && !path.contains("//")
        && !path.contains('@')
        && (!image || config::matches(r"^[a-z0-9][a-z0-9._/-]*(?::[A-Za-z0-9._-]+)?$", path))
}

fn native_package_path(artifact: &Value, architecture: &str) -> bool {
    let name = artifact["name"].as_str().unwrap_or("");
    let version = artifact["version"].as_str().unwrap_or("");
    if artifact["transport"] != "native"
        || !config::matches(r"^[a-z0-9][a-z0-9.+-]*$", name)
        || !config::matches(r"^[A-Za-z0-9][A-Za-z0-9.+:!~_-]*$", version)
    {
        return false;
    }
    let expected = if artifact["kind"] == "apt" {
        let arch = artifact["packageArchitecture"].as_str().unwrap_or("");
        if arch != architecture && arch != "all" {
            return false;
        }
        format!("pool/{}/{name}/{name}_{version}_{arch}.deb", &name[..1])
    } else {
        let filename = artifact["file"].as_str().unwrap_or("").rsplit('/').next().unwrap_or("");
        if !config::matches(r"^[A-Za-z0-9][A-Za-z0-9_.+-]*\.whl$", filename) {
            return false;
        }
        format!("packages/{name}/{version}/{filename}")
    };
    artifact["targetPath"] == expected
}

pub fn validate(manifest: &Value, catalog: &Value, plan: &mut Value, key: &str, topology: &str) -> Result<()> {
    let capability = &plan["target"]["nexusFirst"];
    if capability["schemaVersion"] != 1 || capability["manifest"] != MANIFEST {
        return Err(invalid("安装目标未声明受支持的正式协议"));
    }
    if manifest["schemaVersion"] != 1
        || manifest["kind"] != "chentu-release"
        || !text(&manifest["release"])
        || manifest["toolchain"].as_object().is_none_or(|m| m.is_empty())
    {
        return Err(invalid("版本、类型或构建工具链缺失"));
    }
    for source in ["chentu", "apeironCli"] {
        if !config::matches(
            r"^[a-f0-9]{40}$",
            manifest["sources"][source]["commit"].as_str().unwrap_or(""),
        ) {
            return Err(invalid("源码必须声明确切已提交的 Git revision"));
        }
    }
    let profile = &manifest["profiles"][key];
    let arch = key.rsplit('-').next().unwrap_or("");
    let supported = names(&profile["supportedTopologies"], "profile 拓扑声明无效")?;
    if profile["architecture"] != arch
        || supported != names(&capability["supportedTopologies"], "目标拓扑声明无效")?
        || !supported.contains(topology)
        || !matches!(
            (key, topology),
            ("k3d-arm64", "single-k3d") | ("k3s-amd64", "multi-k3s")
        )
    {
        return Err(invalid(
            "此包不支持所选架构或部署拓扑；当前 Nexus 分发支持 K3d ARM64 或 Ubuntu 22.04 AMD64 三节点",
        ));
    }
    if key == "k3s-amd64"
        && plan["target"]["hostPlatform"]
            != json!({
                "os":"ubuntu", "version":"22.04", "architecture":"amd64"
            })
    {
        return Err(invalid("原生 Nexus 分发必须声明 Ubuntu 22.04 AMD64"));
    }
    let components = manifest["components"]
        .as_object()
        .ok_or_else(|| invalid("缺少组件发布闭包"))?;
    let available: BTreeSet<String> = catalog["components"].as_object().unwrap().keys().cloned().collect();
    if names(&profile["components"], "profile 组件声明无效")? != available
        || components.keys().cloned().collect::<BTreeSet<_>>() != available
    {
        return Err(invalid("profile 与安装目录的组件集合不一致"));
    }
    closure(&manifest["components"], &available)?;
    let bootstrap = names(&profile["bootstrapClosure"], "基础组件闭包无效")?;
    if !bootstrap.contains("nexus")
        || bootstrap != names(&capability["bootstrapComponents"], "目标基础组件无效")?
        || closure(&manifest["components"], &bootstrap)? != bootstrap
        || !bootstrap.is_subset(&names(&plan["components"], "所选组件无效")?)
    {
        return Err(invalid("Nexus 基础闭包必须完整并属于所选应用闭包"));
    }
    let mut repositories = BTreeMap::new();
    for repository in manifest["repositories"]
        .as_array()
        .ok_or_else(|| invalid("缺少仓库声明"))?
    {
        let name = repository["name"].as_str().unwrap_or("");
        let format = repository["format"].as_str().unwrap_or("");
        let kind = repository["type"].as_str().unwrap_or("");
        if !config::matches(r"^[a-z][a-z0-9-]*$", name)
            || name.ends_with("-internal")
            || !["docker", "helm", "raw", "npm", "pypi", "apt"].contains(&format)
            || !["hosted", "proxy"].contains(&kind)
            || repository["online"] != true
            || (kind == "proxy" && repository["autoBlocking"] != true)
            || repositories.insert(name, (format, kind)).is_some()
        {
            return Err(invalid(
                "仓库只允许 ONLINE hosted/proxy，proxy 必须 autoBlocking；不允许 group 或 -internal",
            ));
        }
    }
    let files: BTreeMap<&str, &Value> = catalog["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| (file["path"].as_str().unwrap(), file))
        .collect();
    let mut artifacts = BTreeMap::new();
    for artifact in manifest["artifacts"]
        .as_array()
        .ok_or_else(|| invalid("缺少统一资源发布清单"))?
    {
        let id = artifact["id"].as_str().unwrap_or("");
        let file = files.get(id).ok_or_else(|| invalid("发布资源未列入安装文件目录"))?;
        let kind = artifact["kind"].as_str().unwrap_or("");
        let format = match kind {
            "image" => "docker",
            "chart" => "helm",
            "resource" | "tool" => "raw",
            "npm" => "npm",
            "pypi" => "pypi",
            "apt" => "apt",
            _ => return Err(invalid("未知资源类型")),
        };
        if id == MANIFEST
            || artifact["file"] != id
            || artifact["size"] != file["size"]
            || artifact["sha256"] != file["sha256"]
            || !text(&artifact["name"])
            || !text(&artifact["version"])
            || !artifact["bootstrap"].is_boolean()
            || if ["apt", "pypi"].contains(&kind) {
                !native_package_path(artifact, arch)
            } else {
                !target_path(&artifact["targetPath"], kind == "image")
            }
            || repositories.get(artifact["repository"].as_str().unwrap_or("")) != Some(&(format, "hosted"))
            || artifacts.insert(id, artifact).is_some()
        {
            return Err(invalid("资源名称、版本、校验值、仓库或目标路径不完整/冲突"));
        }
        let platforms = names(&artifact["platforms"], "资源架构声明无效")?;
        if platforms.is_empty() || !platforms.iter().all(|p| p == "any" || p == &format!("linux/{arch}")) {
            return Err(invalid("资源架构与所选 profile 不一致"));
        }
        if kind == "image"
            && (!text(&artifact["image"]["sourceReference"])
                || !digest(&artifact["image"]["sourceDigest"])
                || !digest(&artifact["image"]["targetDigest"])
                || artifact["image"]["architecture"] != arch
                || artifact["image"]["targetPath"] != artifact["targetPath"])
        {
            return Err(invalid("镜像 source/target digest 或架构映射不完整"));
        }
        if kind == "chart"
            && (artifact["chart"]["name"] != artifact["name"]
                || artifact["chart"]["version"] != artifact["version"]
                || names(&artifact["chart"]["releases"], "Chart release 归属无效")?.is_empty())
        {
            return Err(invalid("Chart 名称、版本或 release 归属缺失"));
        }
    }
    if artifacts.len() + 1 != files.len() || !files.contains_key(MANIFEST) {
        return Err(invalid("统一发布清单必须覆盖每个安装文件，且清单自身必须校验"));
    }
    let mut bootstrap_ids = BTreeSet::new();
    let mut selected_ids = BTreeSet::new();
    let selected = names(&plan["components"], "所选组件无效")?;
    for (name, component) in components {
        if names(&component["requires"], "组件依赖声明无效")?
            != names(&catalog["components"][name]["requires"], "目录依赖无效")?
        {
            return Err(invalid("发布闭包与安装目录依赖不一致"));
        }
        let ids = names(&component["artifacts"], "组件资源声明无效")?;
        let mut expected = names(&catalog["components"][name]["files"], "目录资源无效")?;
        expected.remove(MANIFEST);
        if ids != expected || ids.iter().any(|id| !artifacts.contains_key(id.as_str())) {
            return Err(invalid("组件发布资源与安装目录不一致"));
        }
        if bootstrap.contains(name) {
            bootstrap_ids.extend(ids.clone());
        }
        if selected.contains(name) {
            selected_ids.extend(ids);
        }
    }
    for (id, artifact) in &artifacts {
        if artifact["bootstrap"] != bootstrap_ids.contains(*id) {
            return Err(invalid("资源 bootstrap 标记与基础闭包不一致"));
        }
    }
    let mut bootstrap_images = Vec::new();
    let mut mapped_images = BTreeSet::new();
    let all_images = catalog["components"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|component| component["images"].as_array())
        .flatten();
    for image in all_images {
        let id = image["file"].as_str().unwrap();
        mapped_images.insert(id);
        let artifact = artifacts.get(id).ok_or_else(|| invalid("镜像未列入发布清单"))?;
        if artifact["kind"] != "image"
            || artifact["image"]["targetPath"] != image["reference"]
            || artifact["image"]["targetDigest"] != image["digest"]
        {
            return Err(invalid("镜像归档与发布 source/target 映射不一致"));
        }
        if bootstrap_ids.contains(id) && !bootstrap_images.contains(&image["reference"]) {
            bootstrap_images.push(image["reference"].clone());
        }
    }
    if artifacts
        .iter()
        .any(|(id, artifact)| artifact["kind"] == "image" && !mapped_images.contains(*id))
    {
        return Err(invalid("发布镜像缺少安装目录 reference/digest 映射"));
    }
    let mut selected_files = plan["files"].as_array().unwrap().clone();
    if !selected_files.iter().any(|f| f["path"] == MANIFEST) {
        selected_files.push((*files[MANIFEST]).clone());
    }
    for file in &mut selected_files {
        if let Some(artifact) = artifacts.get(file["path"].as_str().unwrap()) {
            for field in ["name", "version", "repository", "platforms"] {
                file[field] = artifact[field].clone();
            }
        }
    }
    bootstrap_images.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    plan["files"] = json!(selected_files);
    plan["bootstrapImages"] = json!(bootstrap_images);
    plan["releaseManifest"] = manifest.clone();
    plan["installSelection"] =
        json!({"release":manifest["release"],"profile":key,"components":selected,"artifacts":selected_ids});
    Ok(())
}

fn verify_source(root: &Path, manifest: &Value, cancel: &process::Cancellation) -> Result<()> {
    let package = resources::json_file(&root.join("chentu-package.json"), 4 * 1024 * 1024)
        .map_err(|_| invalid("缺少正式 Chentu source package provenance"))?;
    if package["schemaVersion"] != 1
        || package["kind"] != "chentu-deployment"
        || package["version"] != manifest["release"]
        || package["revision"] != manifest["sources"]["chentu"]["commit"]
        || package["sourceOverlay"] != json!({})
    {
        return Err(invalid(
            "发布清单与正式 Chentu source revision 不一致，或仍包含临时源码 overlay",
        ));
    }
    let source = package["files"]
        .as_object()
        .ok_or_else(|| invalid("源码文件校验清单缺失"))?;
    let generated = package["generatedFiles"]
        .as_object()
        .ok_or_else(|| invalid("生成的安装目录校验清单缺失"))?;
    if !source.contains_key("deploy/helmfile/scripts/nexus_first.py")
        || !generated.contains_key("setup/install.json")
        || source.contains_key("setup/install.json")
    {
        return Err(invalid("正式 driver 或生成安装目录的 provenance 不完整"));
    }
    for (relative, digest) in source.iter().chain(generated) {
        cancel.check()?;
        if !config::matches(r"^[a-f0-9]{64}$", digest.as_str().unwrap_or("")) {
            return Err(invalid("源码 SHA-256 格式无效"));
        }
        let bytes = resources::read(&resources::source_destination(root, relative)?, 64 * 1024 * 1024)
            .map_err(|_| invalid(&format!("源码文件缺失：{relative}")))?;
        if digest != &config::hash(&bytes) {
            return Err(invalid(&format!("源码文件 SHA-256 校验失败：{relative}")));
        }
    }
    Ok(())
}

// Installation profiles describe target nodes; management machines must also
// match the operator tools shipped by the same release. Use discovered facts,
// including the runtime architecture, rather than inferring an OS from a profile.
fn validate_management_platform(
    plan: &Value,
    key: &str,
    topology: &str,
    runtime_os: &str,
    machine: &Value,
) -> Result<()> {
    let hardware = &machine["hardware"];
    let arch = discovery::architecture(hardware["architecture"].as_str().unwrap_or(""));
    let runtime_arch = discovery::architecture(hardware["runtimeArchitecture"].as_str().unwrap_or(""));
    let supported = match (runtime_os, arch, runtime_arch, key, topology) {
        ("macos", "arm64", "arm64", "k3d-arm64", "single-k3d") => machine["os"]["name"] == "macOS",
        ("linux", "amd64", "amd64", "k3s-amd64", "multi-k3s") => {
            plan["target"]["hostPlatform"] == json!({"os":"ubuntu","version":"22.04","architecture":"amd64"})
                && discovery::validate_platform(
                    &plan["target"],
                    &[json!({"os":machine["os"]["name"],"version":machine["os"]["version"],"architecture":arch})],
                )
                .is_ok()
        }
        _ => false,
    };
    if !supported {
        return Err(invalid("管理机平台与安装目标不匹配；正式 Nexus 分发要求 macOS ARM64 / single-k3d，或 Ubuntu 22.04 AMD64 / multi-k3s"));
    }
    Ok(())
}

pub struct Preparation<'a> {
    pub offline: bool,
    pub cancel: &'a process::Cancellation,
    pub progress: &'a dyn Fn(&str),
}
pub fn prepare(
    root: &Path,
    cache: &Path,
    catalog: &Value,
    plan: &mut Value,
    key: &str,
    topology: &str,
    options: Preparation<'_>,
) -> Result<()> {
    if plan["target"].get("nexusFirst").is_none() {
        return Ok(());
    }
    validate_management_platform(plan, key, topology, std::env::consts::OS, &discovery::machine())?;
    resources::read(&root.join("deploy/helmfile/scripts/nexus_first.py"), 4 * 1024 * 1024)
        .map_err(|_| invalid("缺少 Chentu 正式 nexus_first.py 部署入口"))?;
    let entry = catalog["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == MANIFEST)
        .ok_or_else(|| invalid("发布清单自身未声明大小和 SHA-256"))?;
    resources::prepare_files(
        &json!({"files":[entry]}),
        cache,
        options.offline,
        options.cancel,
        options.progress,
    )?;
    let bytes = resources::read(&resources::destination(cache, MANIFEST)?, 4 * 1024 * 1024)
        .map_err(|_| invalid("缺少 setup/release-manifest.json"))?;
    if entry["size"] != bytes.len() as u64 || entry["sha256"] != config::hash(&bytes) {
        return Err(invalid("发布清单文件大小或 SHA-256 校验失败"));
    }
    let manifest: Value = serde_json::from_slice(&bytes).map_err(|_| invalid("发布清单 JSON 格式无效"))?;
    verify_source(root, &manifest, options.cancel)?;
    validate(&manifest, catalog, plan, key, topology)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value, Value) {
        let mut components = json!({});
        let mut release_components = json!({});
        let mut artifacts = Vec::new();
        let mut files = Vec::new();
        for (name, requires, bootstrap) in [
            ("cert-manager", vec![], true),
            ("nexus", vec!["cert-manager"], true),
            ("apeiron", vec!["nexus"], false),
            ("optional", vec!["nexus"], false),
        ] {
            let file = format!("images/{name}.tar");
            let reference = format!("library/{name}:1.0");
            let digest = format!("sha256:{}", "a".repeat(64));
            files.push(json!({"path":file,"size":1,"sha256":"b".repeat(64)}));
            artifacts.push(json!({"id":file,"file":file,"kind":"image","name":name,"version":"1.0",
                "size":1,"sha256":"b".repeat(64),"repository":"docker-hosted","targetPath":reference,
                "platforms":["linux/arm64"],"bootstrap":bootstrap,"image":{"sourceReference":format!("docker.io/{reference}"),
                "sourceDigest":digest,"targetPath":reference,"targetDigest":digest,"architecture":"arm64"}}));
            components[name] = json!({"requires":requires,"files":[file],"images":[{"file":file,"reference":reference,"digest":digest}]});
            release_components[name] = json!({"requires":requires,"artifacts":[file]});
        }
        let chart = json!({"id":"charts/nexus-1.0.tgz","file":"charts/nexus-1.0.tgz","kind":"chart","name":"nexus","version":"1.0",
            "size":2,"sha256":"c".repeat(64),"repository":"helm-hosted","targetPath":"nexus-1.0.tgz","platforms":["any"],"bootstrap":true,
            "chart":{"name":"nexus","version":"1.0","releases":["nexus"]}});
        files.push(json!({"path":chart["file"],"size":chart["size"],"sha256":chart["sha256"]}));
        components["nexus"]["files"]
            .as_array_mut()
            .unwrap()
            .push(chart["file"].clone());
        release_components["nexus"]["artifacts"]
            .as_array_mut()
            .unwrap()
            .push(chart["file"].clone());
        artifacts.push(chart);
        let manifest = json!({"schemaVersion":1,"kind":"chentu-release","release":"0.2.0-nexus.1",
            "sources":{"chentu":{"commit":"a".repeat(40)},"apeironCli":{"commit":"b".repeat(40)}},"toolchain":{"rust":"1.98.0"},
            "repositories":[{"name":"docker-hosted","format":"docker","type":"hosted","online":true},
                {"name":"helm-hosted","format":"helm","type":"hosted","online":true},
                {"name":"docker-proxy","format":"docker","type":"proxy","online":true,"autoBlocking":true}],
            "profiles":{"k3d-arm64":{"architecture":"arm64","supportedTopologies":["single-k3d"],
                "components":["cert-manager","nexus","apeiron","optional"],"bootstrapClosure":["cert-manager","nexus"]}},
            "components":release_components,"artifacts":artifacts});
        files.push(json!({"path":MANIFEST,"size":manifest.to_string().len(),"sha256":config::hash(manifest.to_string().as_bytes())}));
        let catalog = json!({"schemaVersion":1,"components":components,"files":files,"targets":{"k3d-arm64":{
            "base":["nexus"],"deploymentTopology":true,"environment":{},"nexusFirst":{"schemaVersion":1,"manifest":MANIFEST,
            "supportedTopologies":["single-k3d"],"bootstrapComponents":["cert-manager","nexus"]}}}});
        let plan = resources::plan(&catalog, "k3d-arm64", &json!(["apeiron"])).unwrap();
        (manifest, catalog, plan)
    }
    fn native_fixture() -> (Value, Value, Value) {
        let (mut manifest, mut catalog, _) = fixture();
        let mut profile = manifest["profiles"]["k3d-arm64"].take();
        profile["architecture"] = json!("amd64");
        profile["supportedTopologies"] = json!(["multi-k3s"]);
        manifest["profiles"] = json!({"k3s-amd64":profile});
        for artifact in manifest["artifacts"].as_array_mut().unwrap() {
            if artifact["kind"] == "image" {
                artifact["image"]["architecture"] = json!("amd64");
            }
            artifact["platforms"] = json!(["linux/amd64"]);
        }
        let mut target = catalog["targets"]["k3d-arm64"].take();
        target["nexusFirst"]["supportedTopologies"] = json!(["multi-k3s"]);
        target["hostPlatform"] = json!({"os":"ubuntu","version":"22.04","architecture":"amd64"});
        catalog["targets"] = json!({"k3s-amd64":target});
        let plan = resources::plan(&catalog, "k3s-amd64", &json!(["apeiron"])).unwrap();
        (manifest, catalog, plan)
    }
    fn management_machine(os: &str, version: &str, arch: &str, runtime_arch: &str) -> Value {
        json!({"os":{"name":os,"version":version},"hardware":{"architecture":arch,"runtimeArchitecture":runtime_arch}})
    }
    #[test]
    fn management_machine_must_match_the_exact_distribution_profile() {
        let (_, _, native) = native_fixture();
        let (_, _, k3d) = fixture();
        for version in ["22.04", "22.04.5"] {
            for arch in ["amd64", "x86_64"] {
                let linux = management_machine("Ubuntu", version, arch, "x86_64");
                validate_management_platform(&native, "k3s-amd64", "multi-k3s", "linux", &linux).unwrap();
                assert!(validate_management_platform(&k3d, "k3d-arm64", "single-k3d", "linux", &linux).is_err());
            }
        }
        let mac = management_machine("macOS", "26.0", "arm64", "aarch64");
        validate_management_platform(&k3d, "k3d-arm64", "single-k3d", "macos", &mac).unwrap();
        assert!(validate_management_platform(&native, "k3s-amd64", "multi-k3s", "macos", &mac).is_err());
        for (os, version, arch, runtime_arch) in [
            ("Ubuntu", "24.04", "amd64", "amd64"),
            ("Ubuntu", "20.04", "amd64", "amd64"),
            ("Ubuntu", "22.040", "amd64", "amd64"),
            ("Debian GNU/Linux", "22.04", "amd64", "amd64"),
            ("", "22.04", "amd64", "amd64"),
            ("Ubuntu", "", "amd64", "amd64"),
            ("Ubuntu", "22.04", "arm64", "arm64"),
            ("Ubuntu", "22.04", "amd64", "arm64"),
            ("Ubuntu", "22.04", "arm64", "amd64"),
            ("Ubuntu", "22.04", "amd64", ""),
        ] {
            let machine = management_machine(os, version, arch, runtime_arch);
            assert!(validate_management_platform(&native, "k3s-amd64", "multi-k3s", "linux", &machine).is_err());
        }
        let linux = management_machine("Ubuntu", "22.04", "amd64", "amd64");
        for (runtime_os, key, topology) in [
            ("windows", "k3s-amd64", "multi-k3s"),
            ("linux", "k3s-amd64", "single-k3s"),
            ("linux", "k3s-arm64", "multi-k3s"),
            ("linux", "k3d-amd64", "single-k3d"),
            ("linux", "unknown-amd64", "multi-k3s"),
        ] {
            assert!(validate_management_platform(&native, key, topology, runtime_os, &linux).is_err());
        }
        for platform in [
            Value::Null,
            json!({"os":"ubuntu","version":"24.04","architecture":"amd64"}),
            json!({"os":"ubuntu","version":"22.04","architecture":"arm64"}),
        ] {
            let mut wrong = native.clone();
            wrong["target"]["hostPlatform"] = platform;
            assert!(validate_management_platform(&wrong, "k3s-amd64", "multi-k3s", "linux", &linux).is_err());
        }
        let translated = management_machine("macOS", "26.0", "arm64", "x86_64");
        assert!(validate_management_platform(&k3d, "k3d-arm64", "single-k3d", "macos", &translated).is_err());
    }
    fn preparation_fixture(
        native: bool,
    ) -> (
        crate::app::contracts::Temp,
        crate::app::contracts::Temp,
        Value,
        Value,
        Value,
    ) {
        let (mut manifest, mut catalog, _) = if native { native_fixture() } else { fixture() };
        let (key, topology) = if native {
            ("k3s-amd64", "multi-k3s")
        } else {
            ("k3d-arm64", "single-k3d")
        };
        let root = crate::app::contracts::Temp::new();
        let cache = crate::app::contracts::Temp::new();
        let driver = b"official driver";
        root.write("deploy/helmfile/scripts/nexus_first.py", driver);
        for artifact in manifest["artifacts"].as_array_mut().unwrap() {
            let path = artifact["file"].as_str().unwrap().to_owned();
            let bytes = format!("fixture bytes for {path}");
            cache.write(&path, &bytes);
            artifact["size"] = json!(bytes.len());
            artifact["sha256"] = json!(config::hash(bytes.as_bytes()));
            let file = catalog["files"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|f| f["path"] == path)
                .unwrap();
            file["size"] = artifact["size"].clone();
            file["sha256"] = artifact["sha256"].clone();
        }
        let manifest_bytes = manifest.to_string();
        cache.write(MANIFEST, &manifest_bytes);
        let entry = catalog["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|f| f["path"] == MANIFEST)
            .unwrap();
        entry["size"] = json!(manifest_bytes.len());
        entry["sha256"] = json!(config::hash(manifest_bytes.as_bytes()));
        // Keep K3d's ordinary catalog reader on its declared airgap path.
        if !native {
            catalog["targets"][key]["k3sAirgap"] = json!("images/nexus.tar");
        }
        let catalog_bytes = catalog.to_string();
        root.write("setup/install.json", &catalog_bytes);
        let package = json!({"schemaVersion":1,"kind":"chentu-deployment","version":manifest["release"],
            "revision":manifest["sources"]["chentu"]["commit"],"sourceOverlay":{},
            "files":{"deploy/helmfile/scripts/nexus_first.py":config::hash(driver)},
            "generatedFiles":{"setup/install.json":config::hash(catalog_bytes.as_bytes())}});
        root.write("chentu-package.json", package.to_string());
        let catalog = resources::catalog(&root.0, key).unwrap();
        let mut plan = resources::plan(&catalog, key, &json!(["apeiron"])).unwrap();
        validate(&manifest, &catalog, &mut plan, key, topology).unwrap();
        (root, cache, manifest, catalog, plan)
    }
    // Exercises the public prepare entry with real machine discovery and real
    // manifest/provenance/resource bytes. On other hosts it verifies rejection.
    #[test]
    fn actual_prepare_checks_management_machine_and_preserves_integrity() {
        let native_host = cfg!(target_os = "linux");
        let (root, cache, manifest, catalog, mut plan) = preparation_fixture(native_host);
        let (key, topology) = if native_host {
            ("k3s-amd64", "multi-k3s")
        } else {
            ("k3d-arm64", "single-k3d")
        };
        let manifest_bytes = manifest.to_string();
        let driver = b"official driver";
        // Another otherwise valid profile must fail at the public entry, not
        // merely in the helper. This catches accidental removal of the gate.
        let (other_root, other_cache, _, other_catalog, mut other_plan) = preparation_fixture(!native_host);
        let (other_key, other_topology) = if native_host {
            ("k3d-arm64", "single-k3d")
        } else {
            ("k3s-amd64", "multi-k3s")
        };
        let cancel = process::Cancellation::default();
        assert!(prepare(
            &other_root.0,
            &other_cache.0,
            &other_catalog,
            &mut other_plan,
            other_key,
            other_topology,
            Preparation {
                offline: true,
                cancel: &cancel,
                progress: &|_| {}
            }
        )
        .unwrap_err()
        .message
        .contains("管理机平台"));
        let machine = discovery::machine();
        let os = machine["os"]["name"].as_str().unwrap_or("");
        let version = machine["os"]["version"].as_str().unwrap_or("");
        let arch = machine["hardware"]["architecture"].as_str().unwrap_or("");
        // Decide the expected outcome from independent host facts, so a broken
        // management helper cannot turn a supported-host regression into a skip.
        let supported = if native_host {
            discovery::architecture(std::env::consts::ARCH) == "amd64"
                && arch == "amd64"
                && version.split('.').take(2).collect::<Vec<_>>() == ["22", "04"]
                && discovery::supported(os, version, arch)
        } else {
            cfg!(target_os = "macos")
                && discovery::architecture(std::env::consts::ARCH) == "arm64"
                && arch == "arm64"
                && os == "macOS"
        };
        let cancel = process::Cancellation::default();
        let run = |plan: &mut Value| {
            prepare(
                &root.0,
                &cache.0,
                &catalog,
                plan,
                key,
                topology,
                Preparation {
                    offline: true,
                    cancel: &cancel,
                    progress: &|_| {},
                },
            )
        };
        let result = run(&mut plan);
        if !supported {
            assert!(result.unwrap_err().message.contains("管理机平台"));
            return;
        }
        result.unwrap();
        eprintln!("real prepare passed on {os} {version} {arch} for {key} / {topology}");
        assert_eq!(plan["installSelection"]["profile"], key);
        assert_eq!(plan["installSelection"]["release"], manifest["release"]);
        resources::prepare_files(&plan, &cache.0, true, &cancel, &|_| {}).unwrap();
        cache.write(MANIFEST, "tampered manifest");
        assert!(run(&mut plan).unwrap_err().message.contains("校验失败"));
        cache.write(MANIFEST, &manifest_bytes);
        root.write("deploy/helmfile/scripts/nexus_first.py", "tampered driver");
        assert!(run(&mut plan).unwrap_err().message.contains("SHA-256"));
        root.write("deploy/helmfile/scripts/nexus_first.py", driver);
        cache.write("images/nexus.tar", "tampered resource");
        assert!(resources::prepare_files(&plan, &cache.0, true, &cancel, &|_| {})
            .unwrap_err()
            .message
            .contains("校验失败"));
    }
    #[test]
    fn formal_release_selection_keeps_release_fixed_and_stages_manifest_outside_app_closure() {
        let (manifest, catalog, mut plan) = fixture();
        validate(&manifest, &catalog, &mut plan, "k3d-arm64", "single-k3d").unwrap();
        assert_eq!(plan["installSelection"]["release"], manifest["release"]);
        assert_eq!(
            plan["installSelection"]["components"],
            json!(["apeiron", "cert-manager", "nexus"])
        );
        assert!(!plan["installSelection"]["artifacts"]
            .as_array()
            .unwrap()
            .contains(&json!("images/optional.tar")));
        assert!(plan["files"].as_array().unwrap().iter().any(|f| f["path"] == MANIFEST));
        assert_eq!(
            plan["bootstrapImages"],
            json!(["library/cert-manager:1.0", "library/nexus:1.0"])
        );
        assert_eq!(plan["files"][0]["repository"], "docker-hosted");
    }
    #[test]
    fn native_hosted_requires_exact_platform_and_topology() {
        let (manifest, catalog, mut plan) = native_fixture();
        validate(&manifest, &catalog, &mut plan, "k3s-amd64", "multi-k3s").unwrap();
        assert!(validate(&manifest, &catalog, &mut plan, "k3s-amd64", "single-k3s").is_err());
        plan["target"]["hostPlatform"]["version"] = json!("24.04");
        assert!(validate(&manifest, &catalog, &mut plan, "k3s-amd64", "multi-k3s").is_err());
    }
    #[test]
    fn unsupported_topology_architecture_and_incomplete_bootstrap_fail_closed() {
        let (mut manifest, mut catalog, mut plan) = fixture();
        assert!(validate(&manifest, &catalog, &mut plan, "k3d-arm64", "single-k3s").is_err());
        assert!(validate(&manifest, &catalog, &mut plan, "k3d-amd64", "single-k3d").is_err());
        manifest["profiles"]["k3d-arm64"]["bootstrapClosure"] = json!(["nexus"]);
        catalog["targets"]["k3d-arm64"]["nexusFirst"]["bootstrapComponents"] = json!(["nexus"]);
        plan["target"] = catalog["targets"]["k3d-arm64"].clone();
        assert!(validate(&manifest, &catalog, &mut plan, "k3d-arm64", "single-k3d")
            .unwrap_err()
            .message
            .contains("基础闭包"));
    }
    #[test]
    fn hosted_proxy_only_and_offline_online_policy_are_mandatory() {
        for (field, value) in [
            ("type", json!("group")),
            ("online", json!(false)),
            ("name", json!("docker-internal")),
        ] {
            let (mut manifest, catalog, mut plan) = fixture();
            manifest["repositories"][0][field] = value;
            assert!(validate(&manifest, &catalog, &mut plan, "k3d-arm64", "single-k3d").is_err());
        }
        let (mut manifest, catalog, mut plan) = fixture();
        manifest["repositories"][2]["autoBlocking"] = json!(false);
        assert!(validate(&manifest, &catalog, &mut plan, "k3d-arm64", "single-k3d").is_err());
    }
    #[test]
    fn source_chart_digest_platform_and_optional_resource_conflicts_are_rejected() {
        for case in 0..8 {
            let (mut manifest, catalog, mut plan) = fixture();
            match case {
                0 => manifest["sources"]["chentu"]["commit"] = json!("HEAD"),
                1 => manifest["artifacts"][4]["chart"]["version"] = json!("latest"),
                2 => manifest["artifacts"][0]["image"]["sourceDigest"] = json!("missing"),
                3 => manifest["artifacts"][0]["image"]["targetDigest"] = json!(format!("sha256:{}", "d".repeat(64))),
                4 => manifest["artifacts"][3]["sha256"] = json!("d".repeat(64)),
                5 => manifest["artifacts"][0]["platforms"] = json!(["linux/amd64"]),
                6 => manifest["artifacts"][3]["image"]["targetDigest"] = json!(format!("sha256:{}", "d".repeat(64))),
                _ => manifest["artifacts"].as_array_mut().unwrap().pop().map(|_| ()).unwrap(),
            }
            assert!(
                validate(&manifest, &catalog, &mut plan, "k3d-arm64", "single-k3d").is_err(),
                "case {case}"
            );
        }
    }
    #[test]
    fn missing_capability_driver_or_manifest_is_rejected_before_resource_staging() {
        let (manifest, catalog, mut plan) = fixture();
        plan["target"]["nexusFirst"] = json!({"bootstrapComponents":["nexus"]});
        assert!(validate(&manifest, &catalog, &mut plan, "k3d-arm64", "single-k3d").is_err());
        let temp = crate::app::contracts::Temp::new();
        let (_, catalog, mut plan) = fixture();
        assert!(prepare(
            &temp.0,
            &temp.0,
            &catalog,
            &mut plan,
            "k3d-arm64",
            "single-k3d",
            Preparation {
                offline: true,
                cancel: &process::Cancellation::default(),
                progress: &|_| {}
            }
        )
        .is_err());
        plan["target"].as_object_mut().unwrap().remove("nexusFirst");
        assert!(prepare(
            &temp.0,
            &temp.0,
            &catalog,
            &mut plan,
            "k3d-arm64",
            "single-k3d",
            Preparation {
                offline: true,
                cancel: &process::Cancellation::default(),
                progress: &|_| {}
            }
        )
        .is_ok());
    }
    #[test]
    fn committed_source_provenance_verifies_actual_files_and_rejects_overlay_or_tampering() {
        let (manifest, _, _) = fixture();
        let temp = crate::app::contracts::Temp::new();
        temp.write("deploy/helmfile/scripts/nexus_first.py", "official driver");
        temp.write("setup/install.json", "{}");
        temp.write(".gitignore", "target/");
        let mut package = json!({"schemaVersion":1,"kind":"chentu-deployment","version":manifest["release"],
            "revision":manifest["sources"]["chentu"]["commit"],"sourceOverlay":{},
            "files":{"deploy/helmfile/scripts/nexus_first.py":config::hash(b"official driver"),".gitignore":config::hash(b"target/")},
            "generatedFiles":{"setup/install.json":config::hash(b"{}")}});
        temp.write("chentu-package.json", package.to_string());
        let cancel = process::Cancellation::default();
        verify_source(&temp.0, &manifest, &cancel).unwrap();
        temp.write("deploy/helmfile/scripts/nexus_first.py", "tampered driver");
        assert!(verify_source(&temp.0, &manifest, &cancel)
            .unwrap_err()
            .message
            .contains("SHA-256"));
        temp.write("deploy/helmfile/scripts/nexus_first.py", "official driver");
        package["sourceOverlay"] = json!({"deploy/helmfile/scripts/nexus_first.py":"overlay"});
        temp.write("chentu-package.json", package.to_string());
        assert!(verify_source(&temp.0, &manifest, &cancel).is_err());
        package["sourceOverlay"] = json!({});
        package["revision"] = json!("d".repeat(40));
        temp.write("chentu-package.json", package.to_string());
        assert!(verify_source(&temp.0, &manifest, &cancel).is_err());
    }
    #[test]
    fn native_packages_keep_epoch_coordinates_without_relaxing_resource_paths() {
        let mut apt = json!({"kind":"apt","transport":"native","name":"libaudit1",
            "version":"1:3.0.7-1ubuntu0.1","packageArchitecture":"amd64",
            "targetPath":"pool/l/libaudit1/libaudit1_1:3.0.7-1ubuntu0.1_amd64.deb"});
        assert!(native_package_path(&apt, "amd64"));
        assert!(!native_package_path(&apt, "arm64"));
        assert!(!target_path(&apt["targetPath"], false));
        apt["transport"] = json!("tar-v1");
        assert!(!native_package_path(&apt, "amd64"));
        let mut wheel = json!({"kind":"pypi","transport":"native","name":"annotated-types",
            "version":"0.8.0","file":"cli/annotated_types-0.8.0-py3-none-any.whl",
            "targetPath":"packages/annotated-types/0.8.0/annotated_types-0.8.0-py3-none-any.whl"});
        assert!(native_package_path(&wheel, "amd64"));
        for path in [
            "../escape",
            "packages/other/0.8.0/annotated_types-0.8.0-py3-none-any.whl",
        ] {
            wheel["targetPath"] = json!(path);
            assert!(!native_package_path(&wheel, "amd64"));
        }
    }
}
