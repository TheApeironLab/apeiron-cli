//! Durable, same-installation recovery. Checkpoints are private metadata, never
//! copies of configuration values or credentials. UI stages are not checkpoints.
use super::process::Cancellation;
use super::*;
use std::{fs, path::PathBuf};

pub fn location(config: &Path) -> PathBuf {
    PathBuf::from(format!("{}.installation-resume.json", config.display()))
}
pub fn completed_location(config: &Path) -> PathBuf {
    PathBuf::from(format!("{}.installation-complete.json", config.display()))
}
pub fn recovery(config: &Path) -> Result<Option<Value>> {
    if let Some(record) = load(config)? {
        return Ok(Some(record));
    }
    let record = load_record(config, &completed_location(config))?;
    if record
        .as_ref()
        .is_some_and(|r| r["succeeded"] != true || r["finished"] != true || r["commandActive"] != false)
    {
        return Err(error());
    }
    Ok(record)
}
pub fn complete(config: &Path, directory: &Path) -> Result<()> {
    let mut record = load(config)?.ok_or_else(error)?;
    record["succeeded"] = json!(true);
    record["finished"] = json!(true);
    record["commandActive"] = json!(false);
    record["commandPid"] = Value::Null;
    // Publish the completed record before archiving the active checkpoint. A
    // crash between the two leaves the active recovery path taking precedence.
    write(&completed_location(config), &record)?;
    fs::rename(location(config), directory.join("completed-checkpoint.json")).map_err(|_| error())
}
fn error() -> Error {
    fail(
        "无法安全继续部署：检查点、配置、安装包或集群身份已变化。请查看原部署记录；不会自动接管或升级。",
        409,
    )
}
pub fn write(path: &Path, value: &Value) -> Result<()> {
    let temp = path.with_extension(format!("tmp-{:016x}", rand::random::<u64>()));
    private_write(&temp, &serde_json::to_vec(value).map_err(|_| error())?)?;
    let result = (|| {
        fs::File::open(&temp).and_then(|f| f.sync_all()).map_err(|_| error())?;
        fs::rename(&temp, path).map_err(|_| error())?;
        fs::File::open(path.parent().ok_or_else(error)?)
            .and_then(|f| f.sync_all())
            .map_err(|_| error())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
fn private_file(path: &Path, create: bool) -> Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let file = OpenOptions::new()
            .read(true)
            .write(create)
            .create(create)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| error())?;
        let metadata = file.metadata().map_err(|_| error())?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(error());
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, create);
        Err(error())
    }
}
pub fn load(config: &Path) -> Result<Option<Value>> {
    load_record(config, &location(config))
}
fn load_record(config: &Path, path: &Path) -> Result<Option<Value>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(error()),
        Ok(_) => {}
    }
    // Validate the opened inode, then parse that same file; a path-only mode
    // check could race a replacement between validation and reading.
    let file = private_file(path, false)?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(error());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| error())?;
    if value["schemaVersion"] != 1 || value["configPath"] != json!(config) {
        return Err(error());
    }
    for field in ["configHash", "cliSha256"] {
        if !config::matches(r"^[a-f0-9]{64}$", value[field].as_str().unwrap_or("")) {
            return Err(error());
        }
    }
    if value["checkpoint"] != "bootstrap-complete"
        || ![json!("native"), json!("docker")].contains(&value["target"]["runner"])
        || !value["commandActive"].is_boolean()
        || value["ownerPid"]
            .as_i64()
            .is_none_or(|pid| pid <= 0 || pid > i32::MAX as i64)
        || !config::matches(r"^[a-zA-Z0-9-]{1,128}$", value["clusterUid"].as_str().unwrap_or(""))
    {
        return Err(error());
    }
    for field in ["root", "environment", "kubeconfig"] {
        if !Path::new(value["target"][field].as_str().ok_or_else(error)?).is_absolute() {
            return Err(error());
        }
    }
    if value["target"]["runner"] == "docker"
        && (!Path::new(value["target"]["workDir"].as_str().ok_or_else(error)?).is_absolute()
            || !value["target"]["image"].is_string())
    {
        return Err(error());
    }
    if value["environment"]
        .as_object()
        .ok_or_else(error)?
        .values()
        .any(|v| !v.is_string())
    {
        return Err(error());
    }
    for (path, sha) in value["files"].as_object().ok_or_else(error)? {
        if !Path::new(path).is_absolute() || !config::matches(r"^[a-f0-9]{64}$", sha.as_str().unwrap_or("")) {
            return Err(error());
        }
    }
    if !value["plan"].is_null() {
        let bundle = Path::new(value["bundle"].as_str().ok_or_else(error)?);
        if !bundle.is_absolute() {
            return Err(error());
        }
        for file in value["plan"]["files"].as_array().ok_or_else(error)? {
            resources::destination(bundle, file["path"].as_str().ok_or_else(error)?)?;
            if file["size"].as_u64().is_none()
                || !config::matches(r"^[a-f0-9]{64}$", file["sha256"].as_str().unwrap_or(""))
            {
                return Err(error());
            }
        }
    }
    let directory = PathBuf::from(value["directory"].as_str().ok_or_else(error)?);
    if directory.parent() != Some(&config.parent().ok_or_else(error)?.join("deployments"))
        || !config::matches(
            r"^run-[a-f0-9]{16}$",
            directory.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        )
    {
        return Err(error());
    }
    if fs::symlink_metadata(&directory)
        .map_err(|_| error())?
        .file_type()
        .is_symlink()
    {
        return Err(error());
    }
    Ok(Some(value))
}
pub fn config_hash(config: &Value) -> String {
    config::hash(&serde_json::to_vec(config).unwrap())
}
pub fn running(pid: i64, group: bool) -> bool {
    if pid <= 0 || pid > i32::MAX as i64 {
        return true;
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(if group { -(pid as i32) } else { pid as i32 }, 0) == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        }
    }
    #[cfg(not(unix))]
    {
        true
    }
}
pub fn inactive(record: &Value) -> Result<()> {
    if record["finished"] != true && record["ownerPid"].as_i64().is_none_or(|pid| running(pid, false)) {
        return Err(fail("原部署进程仍存在，不能并发继续部署。", 409));
    }
    if record["commandActive"] == true && record["commandPid"].as_i64().is_none_or(|pid| running(pid, true)) {
        return Err(fail(
            "原部署子进程可能仍在运行，尚不能安全继续；不会自动终止或接管。",
            409,
        ));
    }
    Ok(())
}
// Serializes restart attempts, including independent wizard processes. Keep the
// file itself: unlinking a flock file would allow a second inode/lock owner.
pub fn guard(config: &Path) -> Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let file = private_file(Path::new(&format!("{}.installation-owner", config.display())), true)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(fail("另一个进程正在部署或继续此安装。", 409));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        Err(fail("此平台不支持安全部署恢复锁。", 409))
    }
}
fn digest(path: &Path) -> Result<String> {
    Ok(config::hash(&resources::read(path, 64 * 1024 * 1024)?))
}
fn executable() -> Result<&'static fs::File> {
    static EXECUTABLE: std::sync::OnceLock<Result<fs::File>> = std::sync::OnceLock::new();
    match EXECUTABLE.get_or_init(|| {
        #[cfg(target_os = "linux")]
        let path = PathBuf::from("/proc/self/exe");
        #[cfg(not(target_os = "linux"))]
        let path = std::env::current_exe().map_err(|_| error())?;
        fs::File::open(path).map_err(|_| error())
    }) {
        Ok(file) => Ok(file),
        Err(error) => Err(error.clone()),
    }
}
pub fn bind_executable() -> Result<()> {
    executable().map(|_| ())
}
pub fn cli_identity() -> Result<String> {
    static IDENTITY: std::sync::OnceLock<Result<String>> = std::sync::OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            use sha2::{Digest, Sha256};
            // Bind the running inode at wizard startup, but hash it only in the
            // deployment worker. Idle UI startup must not scan a large debug binary.
            // Replacing the executable path cannot change this process's identity.
            let mut file = executable()?.try_clone().map_err(|_| error())?;
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = file.read(&mut buffer).map_err(|_| error())?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            Ok(format!("{:x}", hash.finalize()))
        })
        .clone()
}
pub fn cluster_uid(target: &Value, env: &BTreeMap<String, String>, cancel: &Cancellation) -> Result<String> {
    let uid = process::text(
        cluster::toolbox_command(target, env, "kubectl").args([
            "get",
            "namespace",
            "kube-system",
            "-o",
            "jsonpath={.metadata.uid}",
        ]),
        b"",
        20,
        4096,
        cancel,
    )?;
    if !config::matches(r"^[a-zA-Z0-9-]{1,128}$", uid.trim()) {
        return Err(error());
    }
    Ok(uid.trim().to_owned())
}
fn native_binding(target: &Value, env: &BTreeMap<String, String>) -> Result<()> {
    if target["runner"] != "native" || (target["kubectlPath"].is_null() && !env.contains_key("CHENTU_KUBECTL")) {
        return Ok(());
    }
    let work = Path::new(target["workDir"].as_str().ok_or_else(error)?);
    let tool = work.join("operator/venv/bin/kubectl");
    if !work.is_absolute()
        || target["kubectlPath"] != json!(tool)
        || env.get("CHENTU_KUBECTL") != Some(&tool.to_string_lossy().into_owned())
        || env.get("CHENTU_HELM") != Some(&work.join("bundle/bin/helm").to_string_lossy().into_owned())
        || target["kubeconfig"] != json!(work.join("state/kubeconfig"))
        || env.get("KUBECONFIG") != target["kubeconfig"].as_str().map(|s| s.to_owned()).as_ref()
        || fs::read_link(tool).map_err(|_| error())? != work.join("bundle/k3s/k3s")
    {
        return Err(error());
    }
    Ok(())
}

pub fn checkpoint(
    config_path: &Path,
    config: &Value,
    directory: &Path,
    target: &Value,
    env: &BTreeMap<String, String>,
    resources: (&Value, &Path),
    cancel: &Cancellation,
) -> Result<Value> {
    native_binding(target, env)?;
    let (plan, bundle) = resources;
    let root = Path::new(target["root"].as_str().ok_or_else(error)?);
    let package = resources::json_file(&root.join("chentu-package.json"), 4 * 1024 * 1024)?;
    let mut files = BTreeMap::new();
    for field in ["files", "generatedFiles"] {
        for (relative, expected) in package[field].as_object().ok_or_else(error)? {
            let path = resources::source_destination(root, relative)?;
            let actual = digest(&path)?;
            if expected != &actual {
                return Err(error());
            }
            files.insert(path.to_string_lossy().into_owned(), actual);
        }
    }
    for path in [
        root.join("chentu-package.json"),
        PathBuf::from(target["environment"].as_str().ok_or_else(error)?),
        PathBuf::from(target["kubeconfig"].as_str().ok_or_else(error)?),
    ] {
        files.insert(path.to_string_lossy().into_owned(), digest(&path)?);
    }
    if let Some(path) = env.get("CHENTU_ENV") {
        files.insert(path.clone(), digest(Path::new(path))?);
    }
    let inventory = directory.join("inventory.yaml");
    if inventory.exists() {
        files.insert(inventory.to_string_lossy().into_owned(), digest(&inventory)?);
    }
    // Bootstrap generates these files after validating the publisher's resources.
    // Bind those generated bytes separately; never compare them with pre-generation hashes.
    let generated = [
        "SHA256SUMS",
        "images/install-manifest.json",
        "nexus/install-selection.json",
    ];
    for relative in generated {
        let path = bundle.join(relative);
        if !bundle.as_os_str().is_empty() && path.exists() {
            files.insert(path.to_string_lossy().into_owned(), digest(&path)?);
        }
    }
    let resource_plan = if plan.is_object() {
        json!({"files":plan["files"].as_array().ok_or_else(error)?.iter()
            .filter(|file| !generated.contains(&file["path"].as_str().unwrap_or("")))
            .map(|file| json!({"path":file["path"],"size":file["size"],"sha256":file["sha256"]})).collect::<Vec<_>>()})
    } else {
        Value::Null
    };
    let value = json!({"schemaVersion":1,"configPath":config_path,"configHash":config_hash(config),
        "directory":directory,"target":target,"environment":env,"files":files,"plan":resource_plan,"bundle":bundle,
        "clusterUid":cluster_uid(target,env,cancel)?,"ownerPid":std::process::id(),"commandActive":false,
        "checkpoint":"bootstrap-complete","cliSha256":cli_identity()?});
    write(&location(config_path), &value)?;
    Ok(value)
}
/// A new application operation may use a newer CLI, but must prove the completed
/// installation's files, configuration and cluster identity unchanged.
pub(crate) fn validate_completed(
    record: &Value,
    config: &Value,
    cancel: &Cancellation,
) -> Result<BTreeMap<String, String>> {
    if record["succeeded"] != true || record["finished"] != true {
        return Err(error());
    }
    inactive(record)?;
    let mut record = record.clone();
    record["cliSha256"] = json!(cli_identity()?);
    validate(&record, config, cancel)
}
pub fn validate(record: &Value, config: &Value, cancel: &Cancellation) -> Result<BTreeMap<String, String>> {
    if record["configHash"] != config_hash(config)
        || record["checkpoint"] != "bootstrap-complete"
        || record["cliSha256"] != cli_identity()?
    {
        return Err(error());
    }
    for (path, expected) in record["files"].as_object().ok_or_else(error)? {
        cancel.check()?;
        if expected != &digest(Path::new(path))? {
            return Err(error());
        }
    }
    let env: BTreeMap<String, String> = serde_json::from_value(record["environment"].clone()).map_err(|_| error())?;
    if record["plan"].is_object() {
        resources::prepare_files(
            &record["plan"],
            Path::new(record["bundle"].as_str().ok_or_else(error)?),
            true,
            cancel,
            &|_| {},
        )?;
    }
    if let Some(name) = env.get("LAB_CONTAINER") {
        let output = process::text(
            std::process::Command::new("docker").args([
                "ps",
                "-a",
                "--filter",
                &format!("name=^/{name}$"),
                "--format",
                "{{.Names}}",
            ]),
            b"",
            20,
            4096,
            cancel,
        )?;
        if !output.trim().is_empty() {
            return Err(fail(
                "原部署工具箱容器仍存在，尚不能安全继续；请核对原任务并清理其已退出的容器。",
                409,
            ));
        }
    }
    native_binding(&record["target"], &env)?;
    if record["clusterUid"] != cluster_uid(&record["target"], &env, cancel)? {
        return Err(error());
    }
    Ok(env)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn native_recovery_rejects_retargeted_tool_or_foreign_kubeconfig_before_query() {
        let temp = contracts::Temp::new();
        let tool = temp.0.join("operator/venv/bin/kubectl");
        fs::create_dir_all(tool.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(temp.0.join("bundle/k3s/k3s"), &tool).unwrap();
        let target = json!({"runner":"native","workDir":temp.0,"kubectlPath":tool,
                            "kubeconfig":temp.0.join("state/kubeconfig")});
        let mut env = BTreeMap::from([
            ("CHENTU_KUBECTL".into(), tool.to_string_lossy().into_owned()),
            (
                "CHENTU_HELM".into(),
                temp.0.join("bundle/bin/helm").to_string_lossy().into_owned(),
            ),
            (
                "KUBECONFIG".into(),
                temp.0.join("state/kubeconfig").to_string_lossy().into_owned(),
            ),
        ]);
        native_binding(&target, &env).unwrap();
        env.insert("KUBECONFIG".into(), "/unrelated/kubeconfig".into());
        assert!(native_binding(&target, &env).is_err());
        env.insert(
            "KUBECONFIG".into(),
            temp.0.join("state/kubeconfig").to_string_lossy().into_owned(),
        );
        fs::remove_file(&tool).unwrap();
        std::os::unix::fs::symlink("/unrelated/kubectl-wrapper", &tool).unwrap();
        assert!(native_binding(&target, &env).is_err());
    }

    fn fixture() -> (contracts::Temp, Value, Value) {
        let temp = contracts::Temp::new();
        let directory = temp.0.join("deployments/run-0000000000000001");
        fs::create_dir_all(&directory).unwrap();

        let root = temp.0.join("source");
        fs::create_dir(&root).unwrap();
        let source = temp.write("source/driver.py", "official fixture source");
        let driver = temp.write(
            "source/deploy/helmfile/run.sh",
            "#!/bin/sh\nprintf 'synchronized\\n' >> \"$RESUME_FIXTURE_MARKER\"\n",
        );
        temp.write(
            "source/chentu-package.json",
            json!({"files":{"driver.py":digest(&source).unwrap(),"deploy/helmfile/run.sh":digest(&driver).unwrap()},"generatedFiles":{}}).to_string(),
        );
        let env_file = temp.write(
            "deployments/run-0000000000000001/environment.yaml",
            "domain: example.internal\n",
        );
        let kubeconfig = temp.write("kubeconfig", "private-fixture-kubeconfig");
        let tool = temp.write("bin/kubectl", "#!/bin/sh\ncat \"$RESUME_FIXTURE_UID\"\n");
        fs::set_permissions(tool, fs::Permissions::from_mode(0o700)).unwrap();
        let helm = temp.write("bin/helm", "#!/bin/sh\nprintf '[]\\n'\n");
        fs::set_permissions(helm, fs::Permissions::from_mode(0o700)).unwrap();
        temp.write("deployments/run-0000000000000001/install.log", "original run\n");
        let config = json!({"slug":"fixture","apps":[],"models":{"apiKey":"private-fixture-key"},"deployment":{"environment":env_file,"runner":"native","root":root,"kubeconfig":kubeconfig}});
        let uid = temp.write("uid", "fixture-cluster-uid");
        let target = json!({"root":root,"environment":env_file,"kubeconfig":kubeconfig,"runner":"native"});
        let env = BTreeMap::from([
            // macOS provides python3 in /usr/bin; keep fixture tools first.
            ("PATH".into(), format!("{}:/usr/bin:/bin", temp.0.join("bin").display())),
            ("RESUME_FIXTURE_UID".into(), uid.to_string_lossy().into_owned()),
            ("CHENTU_ENV".into(), env_file.to_string_lossy().into_owned()),
            (
                "RESUME_FIXTURE_MARKER".into(),
                temp.0.join("sync-marker").to_string_lossy().into_owned(),
            ),
        ]);
        let resource = temp.write("resources/resource.bin", b"verified resource");
        let plan = json!({"files":[{"path":"resource.bin","size":17,"sha256":digest(&resource).unwrap(),"url":"https://example.invalid/?token=private-url-token"}]});
        let record = checkpoint(
            &temp.0.join("config.json"),
            &config,
            &directory,
            &target,
            &env,
            (&plan, &temp.0.join("resources")),
            &Cancellation::default(),
        )
        .unwrap();
        (temp, config, record)
    }
    #[test]
    fn recovery_restart_fixture() {
        let Some(root) = std::env::var_os("APEIRON_RESTART_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root);
        let config_path = root.join("config.json");
        if std::env::var("APEIRON_RESTART_FIXTURE_MODE").unwrap() == "owner" {
            let mut record = load(&config_path).unwrap().unwrap();
            record["ownerPid"] = json!(std::process::id());
            write(&location(&config_path), &record).unwrap();
            fs::write(root.join("ready"), b"ready").unwrap();
            loop {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let config = resources::json_file(&config_path, 65536).unwrap();
        let deployment = deploy::Deployment::restore(&config_path).unwrap();
        assert_eq!(deployment.snapshot()["phase"], "interrupted");
        assert_eq!(
            deployment.snapshot()["completedRecovery"],
            std::env::var("APEIRON_RESTART_FIXTURE_MODE").unwrap() == "completed"
        );
        // Debug test executables are large; isolate integrity hashing from the
        // bounded deployment continuation assertion below. Production hashes in
        // its worker so wizard startup remains responsive.
        cli_identity().unwrap();
        deployment.resume(&config_path, config).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while deployment.active() {
            assert!(std::time::Instant::now() < deadline, "continuation timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
        write(&root.join("result.json"), &deployment.snapshot()).unwrap();
    }
    #[test]
    fn a_new_process_continues_the_same_run_and_rechecks_pending_helm() {
        for pending in [false, true] {
            let (temp, config, record) = fixture();
            temp.write("config.json", config.to_string());
            if pending {
                temp.write("bin/helm", "#!/bin/sh\nprintf '%s' '[{\"name\":\"fixture\",\"namespace\":\"default\",\"status\":\"pending-upgrade\"}]'\n");
            }
            let child = |mode| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "app::resume::tests::recovery_restart_fixture", "--nocapture"])
                    .env("APEIRON_RESTART_FIXTURE", &temp.0)
                    .env("APEIRON_RESTART_FIXTURE_MODE", mode)
                    .stdout(std::process::Stdio::null())
                    .stderr(fs::File::create(temp.0.join(format!("{mode}.stderr"))).unwrap())
                    .spawn()
                    .unwrap()
            };
            let mut old = child("owner");
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !temp.0.join("ready").exists() {
                if std::time::Instant::now() > deadline {
                    let _ = old.kill();
                    let _ = old.wait();
                    panic!("owner checkpoint missing");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            old.kill().unwrap();
            old.wait().unwrap();
            assert!(
                child("resume").wait().unwrap().success(),
                "{}",
                fs::read_to_string(temp.0.join("resume.stderr")).unwrap()
            );
            let status = resources::json_file(&temp.0.join("result.json"), 65536).unwrap();
            assert_eq!(
                status["phase"],
                if pending { "failed" } else { "succeeded" },
                "{status}"
            );
            assert_eq!(
                status["log"],
                json!(Path::new(record["directory"].as_str().unwrap()).join("install.log"))
            );
            if pending {
                assert!(status["message"].as_str().unwrap().contains("未完成的 Helm"));
                assert!(!temp.0.join("sync-marker").exists());
                assert!(load(&temp.0.join("config.json")).unwrap().is_some());
            } else {
                assert_eq!(
                    fs::read_to_string(temp.0.join("sync-marker")).unwrap(),
                    "synchronized\n"
                );
                assert!(load(&temp.0.join("config.json")).unwrap().is_none());
                assert!(Path::new(record["directory"].as_str().unwrap())
                    .join("completed-checkpoint.json")
                    .exists());
                assert!(
                    child("completed").wait().unwrap().success(),
                    "{}",
                    fs::read_to_string(temp.0.join("completed.stderr")).unwrap()
                );
                let restored = resources::json_file(&temp.0.join("result.json"), 65536).unwrap();
                assert_eq!(restored["phase"], "succeeded", "{restored}");
                assert_eq!(
                    fs::read_to_string(temp.0.join("sync-marker")).unwrap(),
                    "synchronized\n",
                    "restoring a completed deployment must never run sync again"
                );
            }
            assert_eq!(
                fs::read_to_string(temp.0.join("kubeconfig")).unwrap(),
                "private-fixture-kubeconfig"
            );
        }
    }
    #[test]
    fn cluster_dns_failure_survives_restart_and_reuses_identity_without_provisioning() {
        let (temp, mut config, original) = fixture();
        let path = temp.0.join("config.json");
        let installation = json!({"topology":"multi-k3s","domain":"fixture.example.internal",
            "entryIp":"192.0.2.10","httpsPort":443});
        config["deployment"]["installation"] = installation.clone();
        let mut target = original["target"].clone();
        target["installation"] = installation;
        target["workDir"] = json!(temp.0.join("work"));
        let helper = temp.write(
            "source/bootstrap/private_dns.py",
            r#"import sys
from pathlib import Path
root = Path(__file__).resolve().parents[1]
assert sys.argv[1:] == ['cluster', '--domain', 'fixture.example.internal', '--address', '192.0.2.10']
with (root / 'dns-calls').open('a') as stream:
    stream.write('cluster\n')
if (root / 'dns-blocked').exists():
    raise SystemExit(17)
"#,
        );
        let package_path = temp.0.join("source/chentu-package.json");
        let mut package = resources::json_file(&package_path, 65536).unwrap();
        package["files"]["bootstrap/private_dns.py"] = json!(digest(&helper).unwrap());
        fs::write(package_path, package.to_string()).unwrap();
        temp.tool(
            "bin/ansible-playbook",
            "#!/bin/sh\nprintf 'unexpected provision' > \"$RESUME_FIXTURE_MARKER.provision\"\nexit 99\n",
        );
        let env = serde_json::from_value(original["environment"].clone()).unwrap();
        let directory = Path::new(original["directory"].as_str().unwrap());
        let mut record = checkpoint(
            &path,
            &config,
            directory,
            &target,
            &env,
            (&original["plan"], Path::new(original["bundle"].as_str().unwrap())),
            &Cancellation::default(),
        )
        .unwrap();
        record["finished"] = json!(true);
        write(&location(&path), &record).unwrap();
        temp.write("config.json", config.to_string());
        temp.write("source/dns-blocked", "simulate transient Kubernetes failure");
        let child = |mode| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "app::resume::tests::recovery_restart_fixture", "--nocapture"])
                .env("APEIRON_RESTART_FIXTURE", &temp.0)
                .env("APEIRON_RESTART_FIXTURE_MODE", mode)
                .stdout(std::process::Stdio::null())
                .stderr(fs::File::create(temp.0.join(format!("{mode}.stderr"))).unwrap())
                .status()
                .unwrap()
        };
        assert!(
            child("dns-fail").success(),
            "{}",
            fs::read_to_string(temp.0.join("dns-fail.stderr")).unwrap()
        );
        let failed = resources::json_file(&temp.0.join("result.json"), 65536).unwrap();
        assert_eq!(failed["phase"], "failed");
        assert_eq!(failed["resumable"], true);
        assert!(failed["message"].as_str().unwrap().contains("退出码 17"), "{failed}");
        assert_eq!(load(&path).unwrap().unwrap()["clusterUid"], record["clusterUid"]);
        assert!(!temp.0.join("sync-marker").exists());
        fs::remove_file(temp.0.join("source/dns-blocked")).unwrap();
        assert!(
            child("dns-retry").success(),
            "{}",
            fs::read_to_string(temp.0.join("dns-retry.stderr")).unwrap()
        );
        let restored = resources::json_file(&temp.0.join("result.json"), 65536).unwrap();
        assert_eq!(restored["phase"], "succeeded", "{restored}");
        assert_eq!(
            fs::read_to_string(temp.0.join("source/dns-calls")).unwrap(),
            "cluster\ncluster\n"
        );
        assert_eq!(
            fs::read_to_string(temp.0.join("sync-marker")).unwrap(),
            "synchronized\n"
        );
        assert!(!temp.0.join("sync-marker.provision").exists());
        assert_eq!(
            fs::read_to_string(temp.0.join("kubeconfig")).unwrap(),
            "private-fixture-kubeconfig"
        );
        assert_eq!(restored["log"], json!(directory.join("install.log")));
    }
    #[test]
    fn checkpoint_binds_config_package_credentials_and_cluster_without_copying_secrets() {
        let (temp, config, record) = fixture();
        let cancel = Cancellation::default();
        validate(&record, &config, &cancel).unwrap();
        assert!(!record.to_string().contains("private-fixture-key"));
        assert!(!record.to_string().contains("private-url-token"));
        let mut wrong_cli = record.clone();
        wrong_cli["cliSha256"] = json!("0".repeat(64));
        assert!(validate(&wrong_cli, &config, &cancel).is_err());
        assert!(!record.to_string().contains("private-fixture-kubeconfig"));
        let path = location(&temp.0.join("config.json"));
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        let mut changed = config.clone();
        changed["models"]["apiKey"] = json!("changed");
        assert!(validate(&record, &changed, &cancel).is_err());
        for relative in [
            "source/driver.py",
            "source/chentu-package.json",
            "resources/resource.bin",
            "kubeconfig",
            "deployments/run-0000000000000001/environment.yaml",
            "uid",
        ] {
            let path = temp.0.join(relative);
            let original = fs::read(&path).unwrap();
            fs::write(&path, "changed").unwrap();
            assert!(validate(&record, &config, &cancel).is_err(), "{relative}");
            fs::write(path, original).unwrap();
        }
        validate(&record, &config, &cancel).unwrap();
    }
    #[test]
    fn completed_records_are_private_verified_and_active_recovery_takes_precedence() {
        let (temp, config, record) = fixture();
        let path = temp.0.join("config.json");
        complete(&path, Path::new(record["directory"].as_str().unwrap())).unwrap();
        assert!(load(&path).unwrap().is_none());
        let completed = recovery(&path).unwrap().unwrap();
        assert_eq!(completed["succeeded"], true);
        assert_eq!(
            fs::metadata(completed_location(&path)).unwrap().permissions().mode() & 0o777,
            0o600
        );
        validate(&completed, &config, &Cancellation::default()).unwrap();
        fs::write(temp.0.join("uid"), "replacement-cluster").unwrap();
        assert!(validate(&completed, &config, &Cancellation::default()).is_err());
        assert!(!completed.to_string().contains("private-fixture-key"));
        write(&location(&path), &record).unwrap();
        assert_eq!(recovery(&path).unwrap().unwrap(), record);
        fs::remove_file(location(&path)).unwrap();
        fs::set_permissions(completed_location(&path), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(recovery(&path).is_err());
    }
    #[test]
    fn completed_native_install_restores_access_only_after_identity_validation() {
        for changed_cluster in [false, true] {
            let (temp, mut config, mut record) = fixture();
            let path = temp.0.join("config.json");
            let installation = json!({"topology":"multi-k3s","domain":"fixture.example.internal",
                "entryIp":"192.0.2.10","httpsPort":443});
            config["deployment"]["installation"] = installation.clone();
            record["target"]["installation"] = installation;
            record["target"]["workDir"] = json!(temp.0.join("work"));
            record["configHash"] = json!(config_hash(&config));
            write(&location(&path), &record).unwrap();
            complete(&path, Path::new(record["directory"].as_str().unwrap())).unwrap();
            if changed_cluster {
                temp.write("uid", "different-cluster");
            }
            let deployment = deploy::Deployment::restore(&path).unwrap();
            assert_eq!(deployment.snapshot()["completedRecovery"], true);
            assert!(deployment.artifacts().is_err(), "identity is not verified at startup");
            deployment.resume(&path, config).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            while deployment.active() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                !temp.0.join("sync-marker").exists(),
                "access recovery never executes sync"
            );
            if changed_cluster {
                assert_eq!(deployment.snapshot()["phase"], "failed");
                assert!(deployment.artifacts().is_err());
            } else {
                assert_eq!(deployment.snapshot()["phase"], "succeeded", "{}", deployment.snapshot());
                assert_eq!(
                    deployment.artifacts().unwrap()["info"]["domain"],
                    "fixture.example.internal"
                );
            }
        }
    }
    #[test]
    fn active_or_reused_pids_and_uncertain_spawn_windows_fail_closed() {
        let mut record = json!({"ownerPid":std::process::id(),"commandActive":false});
        assert!(inactive(&record).is_err()); // Includes PID reuse: never assume ownership from PID alone.
        record["finished"] = json!(true);
        inactive(&record).unwrap();
        record["commandActive"] = json!(true);
        assert!(inactive(&record).is_err());
        record["commandPid"] = json!(0);
        assert!(inactive(&record).is_err());
        unsafe {
            record["commandPid"] = json!(libc::getpgrp());
        }
        assert!(inactive(&record).is_err());
    }
    #[test]
    fn simultaneous_recovery_and_symlink_locks_are_rejected() {
        let temp = contracts::Temp::new();
        let config = temp.0.join("config.json");
        let first = guard(&config).unwrap();
        assert!(guard(&config).is_err());
        drop(first);
        drop(guard(&config).unwrap());
        fs::remove_file(format!("{}.installation-owner", config.display())).unwrap();
        let victim = temp.write("victim", "unchanged");
        std::os::unix::fs::symlink(&victim, format!("{}.installation-owner", config.display())).unwrap();
        assert!(guard(&config).is_err());
        assert_eq!(fs::read_to_string(victim).unwrap(), "unchanged");
    }
    #[test]
    fn malformed_and_foreign_run_records_cannot_restore() {
        let (temp, _, mut record) = fixture();
        let config = temp.0.join("config.json");
        assert!(load(&config).unwrap().is_some());
        record["directory"] = json!("/tmp/foreign/run-0000000000000001");
        write(&location(&config), &record).unwrap();
        assert!(load(&config).is_err());
        fs::write(location(&config), "partial checkpoint").unwrap();
        assert!(load(&config).is_err());
    }
    #[test]
    fn checkpoint_and_guard_reject_shared_or_exposed_inodes() {
        let (temp, _, _) = fixture();
        let config = temp.0.join("config.json");
        let checkpoint = location(&config);
        fs::set_permissions(&checkpoint, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(load(&config).is_err());
        fs::set_permissions(&checkpoint, fs::Permissions::from_mode(0o600)).unwrap();
        let alias = temp.0.join("checkpoint-alias");
        fs::hard_link(&checkpoint, &alias).unwrap();
        assert!(load(&config).is_err());
        fs::remove_file(alias).unwrap();
        assert!(load(&config).unwrap().is_some());
        if unsafe { libc::geteuid() } == 0 {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::CString::new(checkpoint.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::chown(name.as_ptr(), 12345, u32::MAX) }, 0);
            assert!(load(&config).is_err());
            assert_eq!(unsafe { libc::chown(name.as_ptr(), 0, u32::MAX) }, 0);
        }
        fs::remove_file(&checkpoint).unwrap();
        std::os::unix::fs::symlink(temp.0.join("missing"), &checkpoint).unwrap();
        assert!(load(&config).is_err());
        let lock = PathBuf::from(format!("{}.installation-owner", config.display()));
        drop(guard(&config).unwrap());
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o660)).unwrap();
        assert!(guard(&config).is_err());
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&lock, temp.0.join("lock-alias")).unwrap();
        assert!(guard(&config).is_err());
    }
    #[test]
    fn malformed_checkpoint_keeps_wizard_available_without_allowing_deploy() {
        let (temp, config, mut record) = fixture();
        let path = temp.0.join("config.json");
        for malformed in [false, true] {
            if malformed {
                fs::write(location(&path), "broken JSON").unwrap();
            } else {
                record["directory"] = json!("/tmp/foreign/run-0000000000000001");
                write(&location(&path), &record).unwrap();
            }
            let deployment = deploy::Deployment::restore(&path).unwrap();
            assert_eq!(deployment.snapshot()["phase"], "failed");
            assert_eq!(deployment.snapshot()["checkpointBlocked"], true);
            assert_eq!(deployment.snapshot()["resumable"], false);
            assert!(!deployment.active());
            assert!(deployment.resume(&path, config.clone()).is_err());
            assert!(load(&path).is_err()); // New-run guards retain this refusal.
        }
    }
}
