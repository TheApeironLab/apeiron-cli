use super::process::{private_directory, Cancellation};
use super::*;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::BufReader,
    path::{Component, PathBuf},
};
const VERSION: &str = "0.1.0-rc.5";
const REVISION: &str = "f109f8567b9df0cf1eeb1b6d6cbd7092be5f99c3";
const DIGEST: &str = "f45a9a370a15e01697ba82d8ad1101853c040a3b1bbf1c0e1f557164d4706021";
const HOST: &str = "apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com";
const ENTRIES: [&str; 5] = [
    "deploy/helmfile/run.sh",
    "deploy/helmfile/helmfile.yaml.gotmpl",
    "deploy/helmfile/scripts/check.py",
    "cli/src/chentu/environment.py",
    "tests/lab/helmfile.sh",
];
pub fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = opts.open(path).map_err(|_| fail("无法读取普通文件。", 400))?;
    let stat = file.metadata().map_err(|_| fail("无法读取文件属性。", 400))?;
    if !stat.is_file() || stat.len() > limit {
        return Err(fail("文件格式或大小不正确。", 400));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| fail("无法读取文件。", 400))?;
    if bytes.len() as u64 > limit {
        return Err(fail("文件超过大小限制。", 400));
    }
    Ok(bytes)
}
pub fn json_file(path: &Path, limit: u64) -> Result<Value> {
    serde_json::from_slice(&read(path, limit)?).map_err(|_| fail("JSON 文件格式不正确。", 400))
}
pub fn yaml(path: &Path) -> Result<Value> {
    serde_yaml_ng::from_slice(&read(path, 2 * 1024 * 1024)?).map_err(|_| fail("环境文件必须是有效的普通 YAML。", 400))
}
pub fn write_yaml(path: &Path, value: &Value) -> Result<()> {
    private_write(
        path,
        serde_yaml_ng::to_string(value)
            .map_err(|_| fail("无法生成 YAML 配置。", 400))?
            .as_bytes(),
    )
}
pub fn safe_relative(path: &str) -> bool {
    config::matches(r"^[a-zA-Z0-9][a-zA-Z0-9._/-]*$", path)
        && path.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}
pub fn destination(base: &Path, relative: &str) -> Result<PathBuf> {
    if !safe_relative(relative) {
        return Err(fail("安装资源路径不正确。", 400));
    }
    let mut path = base.to_path_buf();
    for part in std::iter::once("").chain(relative.split('/')) {
        if !part.is_empty() {
            path.push(part);
        }
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => return Err(fail("安装包目录不能包含符号链接。", 400)),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(fail("无法检查资源路径。", 400)),
            _ => {}
        }
    }
    Ok(path)
}
fn download(
    url: &str,
    seconds: u64,
    trusted: bool,
    cancel: &Cancellation,
    mut consume: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| fail("无法启动下载。", 500))?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(seconds))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| fail("无法启动下载。", 500))?;
        let mut url = reqwest::Url::parse(url).map_err(|_| fail("资源下载地址不正确。", 400))?;
        for _ in 0..6 {
            cancel.check()?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || (trusted
                    && (url.host_str() != Some(HOST)
                        || url.port().is_some()
                        || url.query().is_some()
                        || url.fragment().is_some()
                        || !url.path().starts_with("/chentu/releases/")))
            {
                return Err(fail("资源下载地址不受信任。", 400));
            }
            let mut response = tokio::select! {
                _ = cancel.cancelled() => return Err(fail("操作已取消。", 499)),
                response = client.get(url.clone()).header("User-Agent", "apeiron-cli").send() =>
                    response.map_err(|_| fail("资源下载失败，请检查网络或离线安装包。", 400))?,
            };
            if [301, 302, 303, 307, 308].contains(&response.status().as_u16()) {
                url = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| url.join(v).ok())
                    .ok_or_else(|| fail("资源重定向无效。", 400))?;
                continue;
            }
            if response.status().as_u16() != 200 {
                return Err(fail(
                    format!("资源下载失败（HTTP {}）。", response.status().as_u16()),
                    400,
                ));
            }
            loop {
                let chunk = tokio::select! {
                    _ = cancel.cancelled() => return Err(fail("操作已取消。", 499)),
                    chunk = response.chunk() => chunk.map_err(|_| fail("资源下载中断。", 400))?,
                };
                match chunk {
                    Some(bytes) => consume(&bytes)?,
                    None => return Ok(()),
                }
            }
        }
        Err(fail("资源下载重定向次数过多。", 400))
    })
}
pub fn resolve(target: &Value, cancel: &Cancellation, progress: &dyn Fn(&str)) -> Result<PathBuf> {
    let bundle = target["bundleDir"].as_str().unwrap_or("");
    if !bundle.is_empty() {
        let root = Path::new(bundle).join("chentu");
        if root.join("setup/install.json").is_file() {
            if ENTRIES.iter().any(|p| !root.join(p).is_file()) {
                return Err(fail("离线安装包缺少部署入口。", 400));
            }
            progress("使用所选安装包中的宸途部署程序。");
            return Ok(root);
        }
    }
    if target["offline"] == true {
        return Err(fail(
            "离线安装包缺少 chentu/ 部署程序或 setup/install.json 资源清单；未发起网络请求。",
            400,
        ));
    }
    if let Some(root) = target["root"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| std::env::var("APEIRON_CHENTU_ROOT").ok().filter(|s| !s.is_empty()))
    {
        progress("使用显式配置的开发部署程序。");
        return Ok(root.into());
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or(super::process::home()?.join(".cache"));
    if !base.is_absolute() {
        return Err(fail("XDG_CACHE_HOME 必须是绝对路径。", 400));
    }
    let cache = base.join("apeiron/chentu");
    config::location(&cache.join("guard"))?;
    private_directory(&cache)?;
    let id = format!("chentu-{VERSION}");
    let archive = cache.join(format!("{id}.tar.gz"));
    let dest = cache.join(&id);
    let lock = cache.join(format!("{id}.lock"));
    let deadline = std::time::Instant::now();
    loop {
        cancel.check()?;
        match private_write(&lock, b"") {
            Ok(()) => break,
            Err(_) if lock.exists() && deadline.elapsed() < Duration::from_secs(180) => {
                std::thread::sleep(Duration::from_millis(200))
            }
            Err(_) => return Err(fail("资源正被其他进程准备，请检查缓存锁。", 409)),
        }
    }
    let stage = cache.join(format!(".prepare-{:016x}", rand::random::<u64>()));
    let result = (|| {
        private_directory(&stage)?;
        let mut bytes = read(&archive, 64 * 1024 * 1024)
            .ok()
            .filter(|b| config::hash(b) == DIGEST);
        if bytes.is_none() && !bundle.is_empty() {
            let supplied = Path::new(bundle).join(format!("{id}.tar.gz"));
            if supplied.exists() {
                let b = read(&supplied, 64 * 1024 * 1024)?;
                if config::hash(&b) != DIGEST {
                    return Err(fail("本地宸途安装包校验失败。", 400));
                }
                bytes = Some(b);
            }
        }
        let bytes = match bytes {
            Some(b) => b,
            None => {
                progress("正在下载并校验宸途安装包…");
                let mut b = Vec::new();
                download(
                    &format!("https://{HOST}/chentu/releases/{VERSION}/{id}.tar.gz"),
                    120,
                    true,
                    cancel,
                    |chunk| {
                        if b.len() + chunk.len() > 64 * 1024 * 1024 {
                            return Err(fail("安装包超过大小限制。", 400));
                        }
                        b.extend_from_slice(chunk);
                        Ok(())
                    },
                )?;
                cancel.check()?;
                if b.len() > 64 * 1024 * 1024 || config::hash(&b) != DIGEST {
                    return Err(fail("宸途资源包校验失败。", 400));
                }
                let tmp = stage.join("archive.tar.gz");
                private_write(&tmp, &b)?;
                fs::rename(tmp, &archive).map_err(|_| fail("无法保存已校验安装包。", 500))?;
                b
            }
        };
        let extracted = stage.join("extracted");
        private_directory(&extracted)?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
        let entries = archive.entries().map_err(|_| fail("安装包无法解压。", 400))?;
        let mut intact = dest.is_dir();
        let mut total = 0u64;
        for entry in entries {
            cancel.check()?;
            let mut entry = entry.map_err(|_| fail("安装包目录结构不正确。", 400))?;
            let path = entry.path().map_err(|_| fail("安装包路径不正确。", 400))?.into_owned();
            let pathstr = path.to_str().ok_or_else(|| fail("安装包路径不正确。", 400))?;
            if !pathstr.starts_with(&(id.clone() + "/"))
                || path.components().any(|c| !matches!(c, Component::Normal(_)))
                || pathstr.contains('\\')
            {
                return Err(fail("安装包目录结构不正确。", 400));
            }
            if entry.header().entry_type().is_dir() {
                private_directory(&extracted.join(path))?;
                continue;
            }
            if !entry.header().entry_type().is_file() {
                return Err(fail("安装包不能包含链接或特殊文件。", 400));
            }
            total = total
                .checked_add(entry.size())
                .ok_or_else(|| fail("安装包过大。", 400))?;
            if total > 512 * 1024 * 1024 {
                return Err(fail("安装包解压大小超过限制。", 400));
            }
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|_| fail("安装包解压失败。", 400))?;
            if intact {
                intact = destination(&dest, pathstr)
                    .and_then(|p| read(&p, data.len() as u64))
                    .map(|old| old == data)
                    .unwrap_or(false);
            }
            let output = extracted.join(path);
            private_directory(output.parent().unwrap())?;
            private_write(&output, &data)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    &output,
                    fs::Permissions::from_mode(if entry.header().mode().unwrap_or(0) & 0o111 != 0 {
                        0o700
                    } else {
                        0o600
                    }),
                )
                .map_err(|_| fail("无法设置资源权限。", 500))?;
            }
        }
        let root = extracted.join(&id);
        if ENTRIES.iter().any(|p| !root.join(p).is_file()) {
            return Err(fail("宸途安装包缺少部署入口。", 400));
        }
        let manifest = json_file(&root.join("chentu-package.json"), 16384)?;
        if manifest["schemaVersion"] != 1
            || manifest["kind"] != "chentu-deployment"
            || manifest["version"] != VERSION
            || manifest["revision"] != REVISION
        {
            return Err(fail("安装包版本与 CLI 要求不符。", 400));
        }
        if !intact {
            if dest.exists() {
                fs::rename(&dest, stage.join("previous")).map_err(|_| fail("无法替换资源缓存。", 500))?;
            }
            fs::rename(extracted, &dest).map_err(|_| fail("无法安装资源缓存。", 500))?;
        }
        progress("宸途部署资源已校验。");
        Ok(dest.join(&id))
    })();
    let _ = fs::remove_dir_all(stage);
    let _ = fs::remove_file(lock);
    result
}
pub fn catalog(root: &Path, target: &str) -> Result<Value> {
    let release = json_file(&root.join("setup/install.json"), 4 * 1024 * 1024)?;
    let mut catalog = if release["schemaVersion"] == 2 {
        if !config::matches(r"^(k3d|k3s)-(arm64|amd64)$", target) {
            return Err(fail("安装目标格式不正确。", 400));
        }
        let mut c = release["catalogs"][target].clone();
        if c["targets"]
            .as_object()
            .is_none_or(|m| m.len() != 1 || !m.contains_key(target))
        {
            return Err(fail("发行包未提供匹配的目标清单。", 400));
        }
        c["resourceDirectory"] = json!(format!("targets/{target}"));
        c
    } else {
        release
    };
    if catalog["schemaVersion"] != 1 || !catalog["targets"].is_object() || !catalog["components"].is_object() {
        return Err(fail("安装资源清单格式不正确。", 400));
    }
    let files = catalog["files"]
        .as_array()
        .ok_or_else(|| fail("资源清单缺少 files。", 400))?;
    let mut paths = std::collections::HashSet::new();
    for f in files {
        let p = f["path"].as_str().unwrap_or("");
        if !safe_relative(p)
            || p == "SHA256SUMS"
            || !paths.insert(p)
            || f["size"].as_u64().unwrap_or(0) == 0
            || !config::matches(r"^[a-f0-9]{64}$", f["sha256"].as_str().unwrap_or(""))
        {
            return Err(fail("资源必须声明唯一安全路径、大小和 SHA-256。", 400));
        }
        if let Some(url) = f.get("url") {
            let u = reqwest::Url::parse(url.as_str().unwrap_or("")).map_err(|_| fail("资源下载地址不正确。", 400))?;
            if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
                return Err(fail("资源必须使用 HTTPS。", 400));
            }
        }
    }
    for (name, c) in catalog["components"].as_object().unwrap() {
        if !config::matches(r"^[a-z0-9-]+$", name)
            || c["requires"].as_array().is_none_or(|a| {
                a.iter()
                    .any(|x| x.as_str().is_none_or(|s| catalog["components"].get(s).is_none()))
            })
            || c["files"]
                .as_array()
                .is_none_or(|a| a.iter().any(|x| !paths.contains(x.as_str().unwrap_or(""))))
            || c.get("values").is_some_and(|v| !v.is_object())
        {
            return Err(fail("应用依赖或资源声明不完整。", 400));
        }
        if let Some(images) = c.get("images") {
            for image in images.as_array().ok_or_else(|| fail("镜像声明不完整。", 400))? {
                if !c["files"].as_array().unwrap().contains(&image["file"])
                    || !config::matches(
                        r"^[a-z0-9][a-z0-9._/-]*:[A-Za-z0-9._-]+$",
                        image["reference"].as_str().unwrap_or(""),
                    )
                    || !config::matches(r"^sha256:[a-f0-9]{64}$", image["digest"].as_str().unwrap_or(""))
                {
                    return Err(fail("镜像声明不完整。", 400));
                }
            }
        }
    }
    if target.starts_with("k3d-") {
        airgap(&mut catalog, root, target)?;
    }
    Ok(catalog)
}
fn airgap(c: &mut Value, root: &Path, target: &str) -> Result<()> {
    let extra = json!({"path":"k3s/k3s-airgap-images-arm64.tar.zst","size":241933732u64,"sha256":"856feb047453cd697c2bc4dcf20127448554c8366be992a211558a13d830a038","url":format!("https://{HOST}/chentu/releases/{VERSION}/k3s/k3s-airgap-images-arm64.tar.zst")});
    if c["targets"][target]["k3sAirgap"].is_string() {
        if let Some(f) = c["files"].as_array_mut().unwrap().iter_mut().find(|f| {
            f["path"] == extra["path"]
                && f["sha256"] == extra["sha256"]
                && f["size"] == extra["size"]
                && f.get("url").is_none()
        }) {
            f["url"] = extra["url"].clone();
        }
        return Ok(());
    }
    // Remove this compatibility supplement when the pinned release declares airgap resources for every supported target.
    if target != "k3d-arm64" || yaml(&root.join("stack/k3s/artifacts.yaml"))?["version"] != "v1.36.3+k3s1" {
        return Err(fail("K3s 版本或架构与固定基础镜像包不匹配。", 400));
    }
    let files = c["files"].as_array_mut().unwrap();
    if let Some(old) = files.iter().find(|f| f["path"] == extra["path"]) {
        if old["size"] != extra["size"] || old["sha256"] != extra["sha256"] {
            return Err(fail("基础镜像包声明冲突。", 400));
        }
    } else {
        files.push(extra.clone());
    }
    c["components"]["k3d-airgap"] = json!({"requires":[],"files":[extra["path"]]});
    let base = c["targets"][target]["base"]
        .as_array_mut()
        .ok_or_else(|| fail("目标缺少基础组件。", 400))?;
    if !base.contains(&json!("k3d-airgap")) {
        base.push(json!("k3d-airgap"));
    }
    c["targets"][target]["k3sAirgap"] = extra["path"].clone();
    Ok(())
}
pub fn merge(base: &mut Value, addition: &Value) -> Result<()> {
    for (k, v) in addition.as_object().ok_or_else(|| fail("配置必须是对象。", 400))? {
        if ["__proto__", "prototype", "constructor"].contains(&k.as_str()) {
            return Err(fail("安装包配置包含无效字段。", 400));
        }
        if v.is_object() && base[k].is_object() {
            merge(&mut base[k], v)?;
        } else {
            base[k] = v.clone();
        }
    }
    Ok(())
}
pub fn plan(c: &Value, target: &str, apps: &Value) -> Result<Value> {
    let t = &c["targets"][target];
    if t["deploymentTopology"] != true || !t["environment"].is_object() || t["environment"].get("profile").is_some() {
        return Err(fail("安装包不支持目标或仍使用旧 profile。", 400));
    }
    fn visit(c: &Value, name: &str, visiting: &mut Vec<String>, done: &mut Vec<String>) -> Result<()> {
        if done.iter().any(|x| x == name) {
            return Ok(());
        }
        if visiting.iter().any(|x| x == name) {
            return Err(fail("安装包包含循环依赖。", 400));
        }
        let component = c["components"]
            .get(name)
            .ok_or_else(|| fail(format!("安装包缺少组件：{name}"), 400))?;
        visiting.push(name.into());
        for dep in component["requires"].as_array().unwrap() {
            visit(c, dep.as_str().unwrap(), visiting, done)?;
        }
        visiting.pop();
        done.push(name.into());
        Ok(())
    }
    let mut done = Vec::new();
    let base = t["base"].as_array().ok_or_else(|| fail("安装包缺少基础组件。", 400))?;
    for name in base
        .iter()
        .chain(apps.as_array().ok_or_else(|| fail("应用列表无效。", 400))?)
    {
        visit(
            c,
            name.as_str().ok_or_else(|| fail("组件名称无效。", 400))?,
            &mut vec![],
            &mut done,
        )?;
    }
    let mut env = t["environment"].clone();
    let mut paths = std::collections::HashSet::new();
    let mut images = BTreeMap::new();
    for name in &done {
        let part = &c["components"][name];
        for p in part["files"].as_array().unwrap() {
            paths.insert(p.as_str().unwrap());
        }
        if let Some(v) = part.get("values") {
            merge(&mut env, v)?;
        }
        if let Some(rows) = part["images"].as_array() {
            for image in rows {
                let key = image["reference"].as_str().unwrap();
                if images
                    .get(key)
                    .is_some_and(|old: &Value| old["file"] != image["file"] || old["digest"] != image["digest"])
                {
                    return Err(fail("安装包包含冲突的镜像版本。", 400));
                }
                images.insert(key, image.clone());
            }
        }
    }
    let arch = target.rsplit('-').next().unwrap();
    if env.get("profile").is_some() || env.get("architecture").is_some_and(|a| a != "__ARCH__" && a != arch) {
        return Err(fail("资源架构或 profile 配置不匹配。", 400));
    }
    for field in ["k3sAirgap", "toolboxArchive"] {
        if let Some(path) = t[field].as_str() {
            if !safe_relative(path) || !paths.contains(path) {
                return Err(fail("基础归档未列入资源清单。", 400));
            }
        }
    }
    if t["toolboxArchive"].is_string()
        && !config::matches(r"^sha256:[a-f0-9]{64}$", t["toolboxImageId"].as_str().unwrap_or(""))
    {
        return Err(fail("工具箱归档缺少固定镜像 ID。", 400));
    }
    if let Some(archives) = t.get("dockerArchives") {
        for a in archives
            .as_array()
            .ok_or_else(|| fail("系统镜像归档声明不完整。", 400))?
        {
            if !paths.contains(a["file"].as_str().unwrap_or(""))
                || a["images"].as_array().is_none_or(|rows| {
                    rows.is_empty()
                        || rows.iter().any(|i| {
                            !config::matches(r"^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*$", i["name"].as_str().unwrap_or(""))
                                || !config::matches(r"^sha256:[a-f0-9]{64}$", i["id"].as_str().unwrap_or(""))
                        })
                })
            {
                return Err(fail("系统镜像归档声明不完整。", 400));
            }
        }
    }
    let files = c["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| paths.contains(f["path"].as_str().unwrap()))
        .cloned()
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Err(fail("安装包没有可校验资源。", 400));
    }
    Ok(
        json!({"components":done,"files":files,"target":t,"environment":env,"images":images.values().collect::<Vec<_>>()}),
    )
}
fn valid_file(path: &Path, f: &Value, cancel: &Cancellation) -> bool {
    (move || -> Result<bool> {
        let file = fs::File::open(path).map_err(|_| fail("文件不可读。", 400))?;
        let stat = file.metadata().map_err(|_| fail("文件属性不可读。", 400))?;
        if !stat.is_file() || stat.len() != f["size"].as_u64().unwrap() {
            return Ok(false);
        }
        let mut reader = BufReader::new(file);
        let mut digest = Sha256::new();
        let mut buf = [0u8; 65536];
        loop {
            cancel.check()?;
            let n = reader.read(&mut buf).map_err(|_| fail("资源不可读。", 400))?;
            if n == 0 {
                break;
            }
            digest.update(&buf[..n]);
        }
        Ok(format!("{:x}", digest.finalize()) == f["sha256"].as_str().unwrap())
    })()
    .unwrap_or(false)
}
pub fn prepare_files(
    plan: &Value,
    base: &Path,
    offline: bool,
    cancel: &Cancellation,
    progress: &dyn Fn(&str),
) -> Result<()> {
    let mut missing = Vec::new();
    for f in plan["files"].as_array().unwrap() {
        cancel.check()?;
        if !valid_file(&destination(base, f["path"].as_str().unwrap())?, f, cancel) {
            missing.push(f);
        }
    }
    if offline && !missing.is_empty() {
        return Err(fail(
            format!(
                "离线资源缺少或校验失败：{}；未发起网络请求。",
                missing
                    .iter()
                    .take(8)
                    .map(|f| f["path"].as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("、")
            ),
            400,
        ));
    }
    for f in missing {
        cancel.check()?;
        let name = f["path"].as_str().unwrap();
        progress(&format!("准备资源：{name}"));
        let dest = destination(base, name)?;
        private_directory(dest.parent().unwrap())?;
        let tmp = dest.with_file_name(format!(
            "{}.{:016x}.part",
            dest.file_name().unwrap().to_string_lossy(),
            rand::random::<u64>()
        ));
        let result = (|| {
            let url = f["url"]
                .as_str()
                .ok_or_else(|| fail(format!("资源 {name} 缺少下载地址。"), 400))?;
            private_write(&tmp, b"")?;
            let mut file = OpenOptions::new()
                .write(true)
                .open(&tmp)
                .map_err(|_| fail("无法写入资源。", 500))?;
            let mut sha = Sha256::new();
            let mut size = 0;
            download(url, 1800, false, cancel, |chunk| {
                size += chunk.len() as u64;
                if size > f["size"].as_u64().unwrap() {
                    return Err(fail("资源大小不符。", 400));
                }
                sha.update(chunk);
                file.write_all(chunk).map_err(|_| fail("无法写入资源。", 500))?;
                Ok(())
            })?;
            if size != f["size"].as_u64().unwrap() || format!("{:x}", sha.finalize()) != f["sha256"].as_str().unwrap() {
                return Err(fail("资源完整性校验失败。", 400));
            }
            file.sync_all().map_err(|_| fail("无法持久化资源。", 500))?;
            drop(file);
            fs::rename(&tmp, &dest).map_err(|_| fail("无法安装资源文件。", 500))?;
            Ok(())
        })();
        let _ = fs::remove_file(tmp);
        result?;
    }
    progress("所选组件的资源文件均已完整校验。");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_interrupts_stalled_tls_download() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("https://{}/archive", listener.local_addr().unwrap());
        let cancel = Cancellation::default();
        let trigger = cancel.clone();
        let server = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let socket = loop {
                if let Ok((socket, _)) = listener.accept() {
                    break Some(socket);
                }
                if start.elapsed() > Duration::from_secs(2) {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            // Hold the connection without replying to the TLS handshake.
            std::thread::sleep(Duration::from_millis(100));
            trigger.cancel();
            std::thread::sleep(Duration::from_millis(200));
            drop(socket);
        });
        let start = std::time::Instant::now();
        let result = download(&url, 30, false, &cancel, |_| Ok(()));
        assert_eq!(result.unwrap_err().code, 499);
        assert!(start.elapsed() < Duration::from_secs(3));
        server.join().unwrap();
    }
}
