use super::process::{self, Cancellation};
use super::*;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};
const STAGE_IDS: [&str; 6] = ["check", "cluster", "nexus", "import", "applications", "access"];

fn advance_stage(status: &mut Value, index: usize, detail: &str) -> bool {
    if !status["stages"].is_array() {
        status["stages"] = json!(STAGE_IDS.map(|id| json!({"id": id, "state": "pending", "detail": ""})));
    }
    let current = STAGE_IDS.iter().position(|id| status["stage"] == *id);
    if current.is_some_and(|old| index < old) {
        return false;
    }
    let stages = status["stages"].as_array_mut().unwrap();
    for (previous, item) in stages.iter_mut().enumerate().take(index) {
        if item["state"] == "active" {
            item["state"] = json!("done");
        } else if item["state"] == "pending" && current != Some(previous) {
            item["state"] = json!("skipped");
        }
    }
    stages[index]["state"] = json!("active");
    stages[index]["detail"] = json!(detail);
    status["stage"] = json!(STAGE_IDS[index]);
    true
}

fn finish_stages(status: &mut Value, outcome: &str) {
    let Some(index) = STAGE_IDS.iter().position(|id| status["stage"] == *id) else {
        return;
    };
    let failure = (outcome == "failed").then(|| status["message"].clone());
    if let Some(stages) = status["stages"].as_array_mut() {
        stages[index]["state"] = json!(if outcome == "succeeded" { "done" } else { outcome });
        if let Some(message) = failure.filter(|v| v.as_str().is_some_and(|s| !s.is_empty())) {
            stages[index]["detail"] = message;
        }
    }
}

#[cfg(test)]
mod stage_tests {
    use super::*;

    #[test]
    fn failed_stage_shows_error_instead_of_last_successful_progress() {
        let mut status = json!({});
        advance_stage(&mut status, 0, "所选组件的资源文件均已完整校验。");
        status["message"] = json!("Nexus 发布清单不完整：目录资源无效；尚未修改集群。");
        finish_stages(&mut status, "failed");
        assert_eq!(status["stages"][0]["detail"], status["message"]);
        assert_eq!(status["stages"][0]["state"], "failed");
        assert_eq!(status["stages"][1]["state"], "pending");
    }

    #[cfg(unix)]
    #[test]
    fn recovery_owner_process_fixture() {
        let Some(directory) = std::env::var_os("APEIRON_RECOVERY_PROCESS_FIXTURE") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let context = Context::fixture(&directory);
        let checkpoint = directory.join("checkpoint.json");
        resume::write(
            &checkpoint,
            &json!({"ownerPid":std::process::id(),"commandActive":false}),
        )
        .unwrap();
        *context.owner.0.checkpoint.lock().unwrap() = Some(checkpoint);
        context
            .run(
                "sh",
                &strings(&["-c", "while [ ! -f release ]; do sleep 0.1; done"]),
                &directory,
                &BTreeMap::new(),
            )
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn killed_owner_does_not_allow_recovery_over_a_surviving_process_group() {
        let temp = contracts::Temp::new();
        let mut owner = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::deploy::stage_tests::recovery_owner_process_fixture",
                "--nocapture",
            ])
            .env("APEIRON_RECOVERY_PROCESS_FIXTURE", &temp.0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let record = loop {
            if let Ok(record) = resources::json_file(&temp.0.join("checkpoint.json"), 4096) {
                if record["commandPid"].is_number() {
                    break record;
                }
            }
            if Instant::now() > deadline {
                let _ = owner.kill();
                let _ = owner.wait();
                panic!("command checkpoint was not written");
            }
            thread::sleep(Duration::from_millis(20));
        };
        assert!(resume::inactive(&record).is_err());
        owner.kill().unwrap();
        owner.wait().unwrap();
        assert!(!resume::running(record["ownerPid"].as_i64().unwrap(), false));
        let group = record["commandPid"].as_i64().unwrap();
        assert!(resume::running(group, true));
        assert!(resume::inactive(&record).unwrap_err().message.contains("子进程"));
        fs::write(temp.0.join("release"), b"done").unwrap();
        // Explicitly clean only this fixture's process group; normal recovery
        // never kills a possibly live previous deployment.
        thread::sleep(Duration::from_millis(200));
        unsafe {
            libc::kill(-(group as i32), libc::SIGKILL);
        }
    }

    #[cfg(unix)]
    #[test]
    fn exited_leader_cannot_hide_descendants_with_a_later_command() {
        let temp = contracts::Temp::new();
        let context = Context::fixture(&temp.0);
        let checkpoint = temp.0.join("checkpoint.json");
        resume::write(
            &checkpoint,
            &json!({"ownerPid":std::process::id(),"commandActive":false}),
        )
        .unwrap();
        *context.owner.0.checkpoint.lock().unwrap() = Some(checkpoint.clone());
        let first = context.run("sh", &strings(&["-c", "sleep 20 & exit 0"]), &temp.0, &BTreeMap::new());
        let record = resources::json_file(&checkpoint, 4096).unwrap();
        let group = record["commandPid"].as_i64().unwrap();
        struct Cleanup(i64);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(-(self.0 as i32), libc::SIGKILL);
                }
            }
        }
        let _cleanup = Cleanup(group);
        assert!(first.unwrap_err().message.contains("子进程"));
        assert_eq!(record["commandActive"], true);
        assert!(resume::running(group, true));
        assert!(context
            .run("sh", &strings(&["-c", "touch must-not-run"]), &temp.0, &BTreeMap::new())
            .is_err());
        assert!(!temp.0.join("must-not-run").exists());
        assert_eq!(resources::json_file(&checkpoint, 4096).unwrap(), record);
    }

    #[test]
    fn stages_advance_without_inventing_work_and_do_not_regress() {
        let mut status = json!({});
        advance_stage(&mut status, 0, "check");
        advance_stage(&mut status, 1, "cluster");
        advance_stage(&mut status, 4, "existing repository");
        assert_eq!(status["stages"][0]["state"], "done");
        assert_eq!(status["stages"][1]["state"], "done");
        assert_eq!(status["stages"][2]["state"], "skipped");
        assert_eq!(status["stages"][3]["state"], "skipped");
        assert!(!advance_stage(&mut status, 2, "late bootstrap output"));
        assert_eq!(status["stage"], "applications");
        finish_stages(&mut status, "failed");
        assert_eq!(status["stages"][4]["state"], "failed");
        assert_eq!(status["stages"][5]["state"], "pending");
    }

    #[test]
    fn stages_keep_failures_and_cancellation_at_the_current_step() {
        for outcome in ["failed", "cancelled"] {
            let mut status = json!({});
            for index in 0..=3 {
                advance_stage(&mut status, index, "working");
            }
            finish_stages(&mut status, outcome);
            for index in 0..3 {
                assert_eq!(status["stages"][index]["state"], "done");
            }
            assert_eq!(status["stages"][3]["state"], outcome);
            assert_eq!(status["stages"][4]["state"], "pending");
            assert_eq!(status["stages"][5]["state"], "pending");
        }
    }

    #[test]
    fn stages_follow_release_phases_then_finish_access_preparation() {
        let directory = std::env::temp_dir().join(format!("apeiron-stages-{:016x}", rand::random::<u64>()));
        fs::create_dir(&directory).unwrap();
        let context = Context::fixture(&directory);
        context.stage(0, "check");
        context.stage(1, "cluster");
        context.stage(2, "bootstrap");
        context.helm_progress("nexus");
        assert_eq!(context.owner.snapshot()["stage"], "nexus");
        context.stage(3, "supply");
        context.helm_progress("nexus");
        assert_eq!(context.owner.snapshot()["stage"], "import");
        assert_eq!(context.owner.snapshot()["stages"][3]["state"], "active");
        assert_eq!(context.owner.snapshot()["stages"][4]["state"], "pending");
        context.nexus_progress("NEXUS-FIRST docker-hosted reused fixture 1.0");
        assert!(context.owner.snapshot()["message"]
            .as_str()
            .unwrap()
            .contains("fixture 1.0"));
        context.stage(4, "deploy");
        context.helm_progress("apeiron");
        assert_eq!(context.owner.snapshot()["stage"], "applications");
        assert!(context.owner.snapshot()["message"]
            .as_str()
            .unwrap()
            .contains("apeiron"));
        context.stage(5, "access");
        let mut status = context.owner.snapshot();
        finish_stages(&mut status, "succeeded");
        assert!(status["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["state"] == "done"));
        fs::remove_dir_all(directory).unwrap();
    }
}

#[derive(Clone)]
pub struct Deployment(Arc<Inner>);
struct Inner {
    status: Mutex<Value>,
    cancel: Mutex<Cancellation>,
    task: Mutex<Option<thread::JoinHandle<()>>>,
    completed: Mutex<Option<(Value, Value)>>,
    lock: Mutex<Option<PathBuf>>,
    container: Mutex<Option<String>>,
    checkpoint: Mutex<Option<PathBuf>>,
}
pub struct Context {
    pub config: PathBuf,
    pub directory: PathBuf,
    pub cancel: Cancellation,
    owner: Deployment,
    log: fs::File,
}
impl Context {
    pub(crate) fn application(config: &Path, directory: &Path) -> Result<Self> {
        process::private_directory(directory)?;
        let path = directory.join("install.log");
        if !path.exists() {
            private_write(&path, b"")?;
        }
        config::location(&path)?;
        let owner = Deployment::new();
        Ok(Self {
            config: config.to_owned(),
            directory: directory.to_owned(),
            cancel: Cancellation::default(),
            owner,
            log: OpenOptions::new()
                .append(true)
                .open(path)
                .map_err(|_| fail("无法打开应用操作日志。", 9))?,
        })
    }
    pub(crate) fn app_checkpoint(&self) {
        *self.owner.0.checkpoint.lock().unwrap() = Some(resume::location(&self.config));
    }
    pub(crate) fn app_cleanup(&self) -> bool {
        self.owner.cleanup()
    }

    #[cfg(test)]
    pub fn fixture(directory: &Path) -> Self {
        Self {
            config: directory.join("config.json"),
            directory: directory.to_owned(),
            cancel: Cancellation::default(),
            owner: Deployment::new(),
            log: fs::File::create(directory.join("install.log")).unwrap(),
        }
    }

    fn command_checkpoint(&self, active: bool, pid: Option<u32>) -> Result<()> {
        if let Some(path) = self.owner.0.checkpoint.lock().unwrap().as_ref() {
            let mut record = resources::json_file(path, 16 * 1024 * 1024)?;
            if active && pid.is_none() && record["commandActive"] == true {
                return Err(fail("上一部署命令的子进程尚未确认退出，无法启动下一步。", 409));
            }
            record["commandActive"] = json!(active);
            record["commandPid"] = json!(pid);
            resume::write(path, &record)?;
        }
        Ok(())
    }
    pub fn event(&self, message: &str) {
        let tagged = format!("[INFO] {message}");
        let mut s = self.owner.0.status.lock().unwrap();
        s["message"] = json!(message);
        if let Some(index) = STAGE_IDS.iter().position(|id| s["stage"] == *id) {
            s["stages"][index]["detail"] = json!(message);
        }
        let events = s["events"].as_array_mut().unwrap();
        events.push(json!(tagged));
        if events.len() > 100 {
            events.remove(0);
        }
        drop(s);
        let _ = (&self.log).write_all(format!("[{}] {tagged}\n", chrono::Utc::now().to_rfc3339()).as_bytes());
    }
    pub fn stage(&self, index: usize, message: &str) {
        let changed = advance_stage(&mut self.owner.0.status.lock().unwrap(), index, message);
        if changed {
            self.event(message);
        }
    }
    fn nexus_progress(&self, line: &str) {
        let Some(message) = line.trim().strip_prefix("NEXUS-FIRST ") else {
            return;
        };
        // Phases set the stage; Chentu's per-resource lines only add detail to "import".
        if [
            "docker-hosted ",
            "helm-hosted ",
            "raw-hosted ",
            "npm-hosted ",
            "pypi-hosted ",
            "apt-hosted ",
        ]
        .iter()
        .any(|prefix| message.starts_with(prefix))
            && self.owner.snapshot()["stage"] == "import"
        {
            self.stage(
                3,
                &format!("正在导入并校验资源：{}", message.chars().take(400).collect::<String>()),
            );
        }
    }
    fn helm_progress(&self, release: &str) {
        let message = format!("正在同步应用：{release}");
        match self.owner.snapshot()["stage"].as_str() {
            Some("nexus") => self.stage(2, &message),
            Some("applications") => self.stage(4, &message),
            _ => self.event(&message),
        }
    }
    pub fn run(&self, command: &str, args: &[String], cwd: &Path, env: &BTreeMap<String, String>) -> Result<()> {
        self.cancel.check()?;
        if let Some(name) = env.get("LAB_CONTAINER") {
            *self.owner.0.container.lock().unwrap() = Some(name.clone());
        }
        if command == "docker" && args.first().is_some_and(|s| s == "run") {
            if let Some(index) = args.iter().position(|s| s == "--name") {
                *self.owner.0.container.lock().unwrap() = args.get(index + 1).cloned();
            }
        }
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(cwd)
            .envs(env)
            .stdin(Stdio::null())
            .stdout(self.log.try_clone().map_err(|_| fail("无法写入安装日志。", 500))?)
            .stderr(self.log.try_clone().map_err(|_| fail("无法写入安装日志。", 500))?);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut reader =
            fs::File::open(self.directory.join("install.log")).map_err(|_| fail("无法读取安装日志。", 500))?;
        use std::io::{Seek, SeekFrom};
        reader
            .seek(SeekFrom::End(0))
            .map_err(|_| fail("无法读取安装日志。", 500))?;
        let release = regex::Regex::new(r"(?:Upgrading|Installing) release=([a-z0-9-]+),").unwrap();
        let dependency =
            regex::Regex::new(r"^configuration error: ([a-z0-9-]+) requires enabled release ([a-z0-9/-]+)$").unwrap();
        let mut pending = String::new();
        // Mark the spawn window first. If the process dies before the child PID
        // is durable, recovery fails closed instead of assuming no child exists.
        self.command_checkpoint(true, None)?;
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(_) => {
                self.command_checkpoint(false, None)?;
                return Err(fail("无法启动部署工具。", 500));
            }
        };
        if let Err(error) = self.command_checkpoint(true, Some(child.id())) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let mut stopping = None;
        loop {
            let mut chunk = [0u8; 65536];
            let count = reader.read(&mut chunk).map_err(|_| fail("无法读取安装日志。", 500))?;
            pending.push_str(&String::from_utf8_lossy(&chunk[..count]));
            if let Some(last) = pending.rfind('\n') {
                for line in pending[..last].lines() {
                    self.nexus_progress(line);
                    if let Some(found) = release.captures(line) {
                        self.helm_progress(&found[1]);
                    }
                    if let Some(found) = dependency.captures(line.trim()) {
                        self.event(&format!("依赖检查未通过：{} 需要启用 {}。", &found[1], &found[2]));
                    }
                }
                pending = pending[last + 1..].to_owned();
            }
            if pending.len() > 8192 {
                let boundary = pending
                    .char_indices()
                    .find(|(i, _)| *i >= pending.len() - 8192)
                    .map(|(i, _)| i)
                    .unwrap();
                pending.drain(..boundary);
            }

            if self.cancel.check().is_err() {
                let since = stopping.get_or_insert_with(Instant::now);
                #[cfg(unix)]
                unsafe {
                    libc::kill(
                        -(child.id() as i32),
                        if since.elapsed() > Duration::from_secs(5) {
                            libc::SIGKILL
                        } else {
                            libc::SIGTERM
                        },
                    );
                }
                #[cfg(not(unix))]
                let _ = child.kill();
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    #[cfg(unix)]
                    if stopping.is_some() {
                        unsafe {
                            libc::kill(-(child.id() as i32), libc::SIGKILL);
                        }
                    }
                    let descendants = resume::running(child.id() as i64, true);
                    self.command_checkpoint(descendants, Some(child.id()))?;
                    if descendants {
                        return Err(fail("部署命令已退出，但仍有子进程运行；已停止后续步骤。", 409));
                    }
                    self.cancel.check()?;
                    self.owner.0.status.lock().unwrap()["exitCode"] = json!(status.code().unwrap_or(130));
                    self.log
                        .sync_all()
                        .map_err(|_| fail("无法持久化安装日志，已停止安装。", 500))?;
                    return if status.success() {
                        Ok(())
                    } else {
                        let message = format!(
                            "{}：部署命令失败（退出码 {}），请查看本机安装日志。",
                            self.owner.snapshot()["message"].as_str().unwrap_or("部署"),
                            status.code().unwrap_or(130)
                        );
                        self.event(&message);
                        Err(fail(message, 500))
                    };
                }
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(fail("无法等待部署命令退出。", 500));
                }
            }
        }
    }
    pub fn checked(
        &self,
        command: &str,
        args: Vec<String>,
        target: &Value,
        env: &BTreeMap<String, String>,
        message: &str,
    ) -> Result<()> {
        self.event(message);
        self.run(command, &args, Path::new(target["root"].as_str().unwrap()), env)
    }
}
pub fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}
pub fn environment_for(config: &Value, applications: &[Value], mut values: Value) -> Result<Value> {
    if !values.is_object() || values.get("profile").is_some() {
        return Err(fail("环境文件必须是普通 YAML 对象，且不能包含 profile。", 400));
    }
    if values.get("releases").is_none() {
        values["releases"] = json!({});
    }
    if !values["releases"].is_object() {
        return Err(fail("releases 必须是对象。", 400));
    }
    if config["apps"].as_array().unwrap().contains(&json!("gpustack")) {
        let gpu = config::gpustack(&config["gpustack"])?;
        if values["releases"]["gpustack"].is_null() {
            values["releases"]["gpustack"] = json!({});
        }
        if !values["releases"]["gpustack"].is_object() {
            return Err(fail("GPUStack 配置必须是对象。", 400));
        }
        if values["releases"]["gpustack"]["values"].is_null() {
            values["releases"]["gpustack"]["values"] = json!({});
        }
        if !values["releases"]["gpustack"]["values"].is_object() {
            return Err(fail("GPUStack values 必须是对象。", 400));
        }
        values["releases"]["gpustack"]["values"]["gpuHost"] = gpu["host"].clone();
        values["releases"]["gpustack"]["values"]["gpuPort"] = gpu["port"].clone();
    }
    for app in applications {
        let names = app["releases"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![app["id"].clone()]);
        for name in names {
            let key = name.as_str().unwrap();
            let release = &mut values["releases"][key];
            if release.is_null() {
                *release = json!({});
            }
            if !release.is_object() {
                return Err(fail("应用配置必须是对象。", 400));
            }
            // Fresh package planning has already selected the native Helmfile
            // dependency closure (e.g. Task requires mail). Keep those dependencies.
            release["enabled"] = json!(
                config["apps"].as_array().unwrap().contains(&app["id"])
                    || (config["deployment"]["installation"].is_object() && release["enabled"] == true)
            );
        }
    }
    if config["deployment"]["installation"].is_object() && config["apps"].as_array().unwrap().contains(&json!("vasi")) {
        if values["releases"]["cluster-access"].is_null() {
            values["releases"]["cluster-access"] = json!({});
        }
        values["releases"]["cluster-access"]["enabled"] = json!(true);
    }
    if config["deployment"]["installation"].is_object() && config["apps"].as_array().unwrap().contains(&json!("files"))
    {
        if values["releases"]["files"]["values"].is_null() {
            values["releases"]["files"]["values"] = json!({});
        }
        let v = &mut values["releases"]["files"]["values"];
        if !v.is_object() {
            return Err(fail("应用 values 必须是对象。", 400));
        }
        if v["operatorBucket"].is_null() || v["operatorBucket"] == "" {
            v["operatorBucket"] = json!("files-owner");
        }
    }
    for name in ["nexus", "apeiron"] {
        if values["releases"][name]["values"].is_null() {
            values["releases"][name]["values"] = json!({});
        }
        if !values["releases"][name]["values"].is_object() {
            return Err(fail("应用 values 必须是对象。", 400));
        }
    }
    // Proxy repositories remain ONLINE in offline deployments; connectivity controls upstream access.
    values["releases"]["nexus"]["values"]["publicProxies"] = json!(true);
    if config["models"].is_object() && config["apps"].as_array().unwrap().contains(&json!("apeiron")) {
        let v = &mut values["releases"]["apeiron"]["values"];
        v["defaultModel"] = json!("");
        v["allowedModels"] = json!("");
        v["models"] = models::values(&config["models"]);
        if models::uses_gateway(config) {
            v["models"] = models::gateway_values(&config["models"]);
            if values["releases"]["gateway"]["values"].is_null() {
                values["releases"]["gateway"]["values"] = json!({});
            }
            let gateway = &mut values["releases"]["gateway"]["values"];
            gateway["replicaCount"] = json!(1);
            gateway["modelStub"] = json!({"enabled":false});
            gateway["routes"] = models::routes(&config["models"]);
            gateway["clients"] = json!([{"name":"apeiron","routes":gateway["routes"].as_array().unwrap().iter().map(|r|r["name"].clone()).collect::<Vec<_>>()}]);
            // Respect the distribution/operator's existing egress policy. The
            // default permits public HTTPS only, excluding private/cluster ranges.
            if gateway["egress"]["proxy"]["host"].as_str().unwrap_or("").is_empty()
                && gateway["egress"]["cidrs"].as_array().is_none_or(|rows| rows.is_empty())
            {
                gateway["egress"] = json!({"port":443,"proxy":{},"cidrs":[{"cidr":"0.0.0.0/0","except":["0.0.0.0/8","10.0.0.0/8","100.64.0.0/10","127.0.0.0/8","169.254.0.0/16","172.16.0.0/12","192.168.0.0/16","224.0.0.0/4","240.0.0.0/4"]}]});
            }
        }
    }
    if let Some(domain) = config["deployment"]["installation"]["domain"].as_str() {
        values["embedAllowedOrigins"] = json!([format!("https://{domain}:*"), format!("https://*.{domain}:*")]);
    }
    values["tenantSlug"] = config["slug"].clone();
    Ok(values)
}
impl Deployment {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            status: Mutex::new(json!({"phase":"idle","message":"","events":[]})),
            cancel: Mutex::new(Cancellation::default()),
            task: Mutex::new(None),
            completed: Mutex::new(None),
            lock: Mutex::new(None),
            container: Mutex::new(None),
            checkpoint: Mutex::new(None),
        }))
    }
    pub fn restore(path: &Path) -> Result<Self> {
        resume::bind_executable()?;
        let owner = Self::new();
        let record = match resume::recovery(path) {
            Ok(record) => record,
            Err(_) => {
                *owner.0.status.lock().unwrap() = json!({"phase":"failed","events":[],
                    "checkpointBlocked":true,"resumable":false,
                    "message":"恢复记录损坏、权限不安全或身份无效，不能自动继续或开始另一安装。请先人工确认旧进程和工具箱已停止，再备份移走检查点；不会自动覆盖记录或接管集群。"});
                return Ok(owner);
            }
        };
        if let Some(record) = record {
            *owner.0.status.lock().unwrap() = json!({"phase":"interrupted","events":[],
                "message":if record["succeeded"] == true {"发现已成功部署的记录。恢复访问配置前将核对配置、安装包、资源和集群身份。"} else {"发现已完成集群引导的部署记录。继续前将核对配置、安装包、集群身份与运行中的命令，并重新执行 Helm 同步。"},
                "completedRecovery":record["succeeded"] == true,
                "log":Path::new(record["directory"].as_str().unwrap()).join("install.log"),"resumable":true});
        }
        Ok(owner)
    }
    /// Points an idle deployment at another instance's config path.
    pub fn reload(&self, path: &Path) -> Result<()> {
        if self.active() {
            return Err(fail("部署正在进行。", 409));
        }
        let restored = Self::restore(path)?;
        if let Some(task) = self.0.task.lock().unwrap().take() {
            let _ = task.join();
        }
        *self.0.status.lock().unwrap() = restored.snapshot();
        *self.0.completed.lock().unwrap() = None;
        *self.0.checkpoint.lock().unwrap() = None;
        *self.0.container.lock().unwrap() = None;
        Ok(())
    }
    pub fn resume(&self, path: &Path, config: Value) -> Result<()> {
        if self.active() {
            return Err(fail("部署正在进行。", 409));
        }
        let record =
            resume::recovery(path)?.ok_or_else(|| fail("没有可信的部署检查点；不能自动接管未完成的集群引导。", 409))?;
        if record.get("appOperation").is_some() && record["succeeded"] != true {
            return Err(fail("存在未完成的应用操作，请使用原 apeiron app 命令重试。", 409));
        }
        resume::inactive(&record)?;
        if record["configHash"] != resume::config_hash(&config) {
            return Err(fail("配置已变化，不能作为同一次安装继续。", 409));
        }
        if let Some(task) = self.0.task.lock().unwrap().take() {
            let _ = task.join();
        }
        *self.0.cancel.lock().unwrap() = Cancellation::default();
        *self.0.completed.lock().unwrap() = None;
        *self.0.status.lock().unwrap() = json!({"phase":"preparing","events":[],"completedRecovery":record["succeeded"] == true,"message":"正在核对原部署检查点…","startedAt":chrono::Utc::now().to_rfc3339()});
        advance_stage(&mut self.0.status.lock().unwrap(), 0, "正在核对原部署检查点。");
        let owner = self.clone();
        let path = path.to_owned();
        *self.0.task.lock().unwrap() = Some(thread::spawn(move || owner.execute(path, config, Some(record))));
        Ok(())
    }
    pub fn snapshot(&self) -> Value {
        self.0.status.lock().unwrap().clone()
    }
    pub fn active(&self) -> bool {
        ["preparing", "running", "stopping"].contains(&self.snapshot()["phase"].as_str().unwrap_or(""))
    }
    pub fn start(&self, path: &Path, config: Value) -> Result<()> {
        models::require_for_installation(&config)?;
        if self.active() {
            return Err(fail("部署正在进行。", 409));
        }
        if let Some(task) = self.0.task.lock().unwrap().take() {
            let _ = task.join();
        }
        *self.0.cancel.lock().unwrap() = Cancellation::default();
        *self.0.completed.lock().unwrap() = None;
        *self.0.status.lock().unwrap() = json!({"phase":"preparing","startedAt":chrono::Utc::now().to_rfc3339(),"message":"正在检查部署配置…","events":[]});
        advance_stage(&mut self.0.status.lock().unwrap(), 0, "正在检查部署配置和安装资源。");
        let owner = self.clone();
        let path = path.to_owned();
        *self.0.task.lock().unwrap() = Some(thread::spawn(move || owner.execute(path, config, None)));
        Ok(())
    }
    fn execute(&self, path: PathBuf, config: Value, recovered: Option<Value>) {
        let cancel = self.0.cancel.lock().unwrap().clone();
        let result = (|| {
            let _guard = resume::guard(&path)?;
            // Do not overwrite another process's pending recovery record.
            if recovered.is_none() && resume::load(&path)?.is_some() {
                return Err(fail("存在未完成的部署记录，请先继续原部署。", 409));
            }
            if recovered.is_none() {
                match fs::remove_file(resume::completed_location(&path)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(fail("无法清除上次完成记录；尚未开始新的部署。", 409)),
                }
            }
            let directory = if let Some(record) = &recovered {
                PathBuf::from(record["directory"].as_str().unwrap())
            } else {
                path.parent()
                    .unwrap()
                    .join("deployments")
                    .join(format!("run-{:016x}", rand::random::<u64>()))
            };
            process::private_directory(&directory)?;
            let log_path = directory.join("install.log");
            if recovered.is_none() {
                private_write(&log_path, b"")?;
            }
            self.0.status.lock().unwrap()["log"] = json!(log_path);
            let mut log_options = OpenOptions::new();
            log_options.append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                log_options.custom_flags(libc::O_NOFOLLOW);
            }
            let log = log_options
                .open(&log_path)
                .map_err(|_| fail("无法打开安装日志。", 500))?;
            let context = Context {
                config: path.clone(),
                directory: directory.clone(),
                cancel: cancel.clone(),
                owner: self.clone(),
                log,
            };
            context.event("开始安装检查。");
            let mut target = config["deployment"].clone();
            let fresh = target["installation"].is_object();
            let lock_path = if fresh {
                PathBuf::from(format!("{}.installation.lock", path.display()))
            } else {
                let env = fs::canonicalize(target["environment"].as_str().unwrap_or(""))
                    .map_err(|_| fail("环境 values 文件不存在。", 400))?;
                PathBuf::from(format!("{}.apeiron-deploy.lock", env.display()))
            };
            let recovered_env = if let Some(record) = &recovered {
                // Re-read under the cross-process guard; another continuation may
                // have updated the record after the HTTP request was accepted.
                let current = resume::recovery(&path)?.ok_or_else(|| fail("恢复记录已变化。", 409))?;
                if current != *record {
                    return Err(fail("恢复记录已变化。", 409));
                }
                resume::inactive(record)?;
                let env = resume::validate(record, &config, &cancel)?;
                if record["succeeded"] == true {
                    if lock_path.exists() {
                        return Err(fail("发现部署锁，请先确认原部署已完全停止后再恢复访问配置。", 409));
                    }
                    target = record["target"].clone();
                    context.stage(5, "已核对成功部署的身份，正在恢复访问配置。");
                    if fresh {
                        let artifacts = super::access::collect(&target, &directory, &cancel)?;
                        self.0.status.lock().unwrap()["access"] = artifacts["info"].clone();
                        *self.0.completed.lock().unwrap() = Some((target, artifacts));
                    }
                    context.event("已恢复成功部署记录；请继续配置访问并测试登录。");
                    return Ok(());
                }
                if lock_path.exists() {
                    let pid = String::from_utf8(resources::read(&lock_path, 128)?)
                        .ok()
                        .and_then(|v| v.parse::<i64>().ok())
                        .ok_or_else(|| fail("旧部署锁无法核对。", 409))?;
                    if Some(pid) != record["ownerPid"].as_i64() || resume::running(pid, false) {
                        return Err(fail("旧部署锁身份不符。", 409));
                    }
                    fs::remove_file(&lock_path).map_err(|_| fail("无法释放已退出进程的旧部署锁。", 409))?;
                }
                target = record["target"].clone();
                let mut current = record.clone();
                current["ownerPid"] = json!(std::process::id());
                current["finished"] = json!(false);
                current["commandActive"] = json!(false);
                current["commandPid"] = Value::Null;
                resume::write(&resume::location(&path), &current)?;
                *self.0.checkpoint.lock().unwrap() = Some(resume::location(&path));
                context.stage(
                    1,
                    "原部署身份与文件校验通过；复用已完成的集群引导与凭据，重新检查 Helm 状态并同步。",
                );
                Some(env)
            } else {
                None
            };
            private_write(&lock_path, std::process::id().to_string().as_bytes())
                .map_err(|_| fail("此环境已有部署锁，请检查安装进程。", 409))?;
            *self.0.lock.lock().unwrap() = Some(lock_path);
            let mut prepared_identity = (Value::Null, PathBuf::new());
            let env = if let Some(env) = recovered_env {
                env
            } else if fresh {
                let prepared = super::bootstrap::prepare(&config, &context)?;
                target = prepared.target.clone();
                resources::write_yaml(
                    Path::new(target["environment"].as_str().unwrap()),
                    &environment_for(
                        &config,
                        resources::applications(&prepared.plan)?,
                        prepared.values.clone(),
                    )?,
                )?;
                context.stage(1, "正在准备集群网络、存储和部署入口。");
                // Port probing and cluster creation on this host are serialized
                // across instances; the per-installation checkpoint is unchanged.
                let scheduler = if target["installation"]["topology"] == "single-k3d" {
                    let base = instances::base()?;
                    let lock = match instances::scheduler(&base, Duration::ZERO, &cancel) {
                        Ok(lock) => lock,
                        Err(_) => {
                            context.event("其他安装实例正在创建集群，等待宿主调度锁。");
                            instances::scheduler(&base, Duration::from_secs(1800), &cancel)?
                        }
                    };
                    Some(lock)
                } else {
                    None
                };
                let env = super::bootstrap::bootstrap(&mut target, &context, &prepared)?;
                drop(scheduler);
                prepared_identity = (prepared.plan.clone(), prepared.bundle.clone());
                env
            } else {
                target["root"] = json!(resources::resolve(&target, &cancel, &|m| context.event(m))?);
                context.stage(1, "正在连接已有集群并检查部署环境。");
                let mut env = BTreeMap::new();
                env.insert("HELMFILE_NO_COLOR".into(), "true".into());
                env.insert("HELMFILE_LOG_LEVEL".into(), "info".into());
                if target["runner"] == "native" {
                    env.insert("CHENTU_ROOT".into(), target["root"].as_str().unwrap().into());
                    env.insert("KUBECONFIG".into(), target["kubeconfig"].as_str().unwrap().into());
                } else {
                    env.insert("LAB_WORK_DIR".into(), target["workDir"].as_str().unwrap().into());
                    env.insert("LAB_IMAGE".into(), target["image"].as_str().unwrap().into());
                    env.insert(
                        "LAB_CONTAINER".into(),
                        format!("apeiron-init-{}", directory.file_name().unwrap().to_string_lossy()),
                    );
                }
                env
            };
            cancel.check()?;
            let input = Path::new(target["environment"].as_str().unwrap());
            config::location(input)?;
            if input.extension().is_some_and(|s| s == "gotmpl") {
                return Err(fail("请提供已渲染的普通 YAML 文件。", 400));
            }
            let output = directory.join("environment.yaml");
            if recovered.is_none() {
                let declared = resources::declared_applications(Path::new(target["root"].as_str().unwrap()))?;
                resources::write_yaml(&output, &environment_for(&config, &declared, resources::yaml(input)?)?)?;
            } else if env.get("CHENTU_ENV") != Some(&output.to_string_lossy().into_owned()) {
                return Err(fail("原部署环境路径不匹配，不能继续。", 409));
            }
            // Recovery reuses the exact environment bytes verified by the checkpoint.
            // Re-rendering here would overwrite the original installation contract.
            self.0.status.lock().unwrap()["environment"] = json!(output);
            let mut env = env;
            env.insert("CHENTU_ENV".into(), output.to_string_lossy().into_owned());
            if target["runner"] == "docker" {
                env.insert("LAB_ENV".into(), output.to_string_lossy().into_owned());
                if let Some(name) = env.get("LAB_CONTAINER") {
                    *self.0.container.lock().unwrap() = Some(name.clone());
                }
            }
            if fresh && recovered.is_none() && prepared_identity.0["releaseManifest"].is_object() {
                resume::checkpoint(
                    &path,
                    &config,
                    &directory,
                    &target,
                    &env,
                    (&prepared_identity.0, &prepared_identity.1),
                    &cancel,
                )?;
                *self.0.checkpoint.lock().unwrap() = Some(resume::location(&path));
            }
            // Retryable post-provision work belongs after the durable cluster
            // checkpoint and is replayed on same-installation continuation.
            super::bootstrap::private_cluster_dns(&target, &context, &env)?;
            context.event("检查 Helm 是否存在未完成的操作。");
            cluster::helm_state(&target, &env, &cancel)?;
            if config["models"].is_object() && config["apps"].as_array().unwrap().contains(&json!("apeiron")) {
                // This bounded capture owns its child internally. Persist an
                // uncertain-operation barrier if the CLI dies inside it.
                context.command_checkpoint(true, None)?;
                let applied = if models::uses_gateway(&config) {
                    cluster::gateway_model_key(&config["models"], &target, &env, &cancel)
                } else {
                    cluster::model_key(&config["models"], &target, &env, &cancel)
                };
                context.command_checkpoint(false, None)?;
                applied?;
            }
            self.0.status.lock().unwrap()["phase"] = json!("running");
            let root = Path::new(target["root"].as_str().unwrap());
            let script = root.join(if target["runner"] == "docker" {
                "tests/lab/helmfile.sh"
            } else {
                "deploy/helmfile/run.sh"
            });
            // Release packages (ADR-0019): bootstrap Nexus, supply the release, deploy from Nexus.
            // An existing cluster without a package is synced directly.
            let phases: &[(&str, usize, &str)] = if resources::yaml(&output)?["nexusFirst"] == true {
                &[
                    ("bootstrap", 2, "正在启动证书组件和 Nexus，初始化制品仓库。"),
                    ("supply", 3, "正在将部署资源导入 Nexus，并校验实际存储的内容。"),
                    ("deploy", 4, "资源导入与校验已完成，正在从 Nexus 安装应用。"),
                ]
            } else {
                &[("sync", 4, "正在按依赖安装和初始化所选应用。")]
            };
            for (phase, stage, message) in phases {
                context.stage(*stage, message);
                context.run(
                    "bash",
                    &[script.to_string_lossy().into_owned(), (*phase).into()],
                    root,
                    &env,
                )?;
            }
            context.stage(5, "应用部署与初始化已完成，正在准备访问和验证入口。");
            if fresh {
                if config["apps"].as_array().unwrap().contains(&json!("vasi")) {
                    context.event("正在配置集群 SSO 并验证权限。");
                    if target["runner"] == "docker" {
                        context.run(
                            "bash",
                            &[script.to_string_lossy().into_owned(), "configure-oidc".into()],
                            root,
                            &env,
                        )?;
                    } else {
                        context.run(
                            "python3",
                            &[
                                "-m".into(),
                                "chentu.cluster_oidc".into(),
                                "--inventory".into(),
                                directory.join("inventory.yaml").to_string_lossy().into_owned(),
                            ],
                            root,
                            &env,
                        )?;
                    }
                }
                super::bootstrap::public_access("finish", &target, &context, &env)?;
                let artifacts = super::access::collect(&target, &directory, &cancel)?;
                self.0.status.lock().unwrap()["access"] = artifacts["info"].clone();
                *self.0.completed.lock().unwrap() = Some((target, artifacts));
            }
            context.event("部署完成。");
            if self.0.checkpoint.lock().unwrap().is_some() {
                resume::complete(&path, &directory)?;
                *self.0.checkpoint.lock().unwrap() = None;
            }
            Ok(())
        })();
        if let Err(ref error) = result {
            if let Some(path) = self.snapshot()["log"].as_str() {
                let mut options = OpenOptions::new();
                options.append(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW);
                }
                if let Ok(mut log) = options.open(path) {
                    let _ = writeln!(log, "[ERROR] {}", error.message);
                }
            }
        }
        let stopped = cancel.check().is_err();
        let cleanup = self.cleanup();
        let resumable = if cleanup {
            if let Some(checkpoint) = self.0.checkpoint.lock().unwrap().as_ref() {
                if let Ok(mut record) = resources::json_file(checkpoint, 16 * 1024 * 1024) {
                    record["finished"] = json!(true);
                    resume::write(checkpoint, &record).is_ok()
                } else {
                    false
                }
            } else {
                resume::recovery(&path).ok().flatten().is_some()
            }
        } else {
            false
        };
        let mut s = self.0.status.lock().unwrap();
        s["resumable"] = json!(resumable);
        s["completedRecovery"] = json!(
            resume::load(&path).ok().flatten().is_none()
                && resume::recovery(&path)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r["succeeded"] == true)
        );
        if !cleanup {
            s["phase"] = json!("stopping");
            s["stopFailed"] = json!(true);
            s["message"] = json!("无法确认部署工具箱已停止或释放部署锁。请检查 Docker 和目录权限后重试停止。");
        } else {
            s["phase"] = json!(if stopped {
                "cancelled"
            } else if result.is_ok() {
                "succeeded"
            } else {
                "failed"
            });
            s["finishedAt"] = json!(chrono::Utc::now().to_rfc3339());
            s["exitCode"] = json!(if stopped {
                130
            } else if result.is_ok() {
                0
            } else {
                s["exitCode"].as_i64().filter(|code| *code != 0).unwrap_or(9)
            });
            if let Err(error) = result.as_ref() {
                s["message"] = json!(error.message);
            }
            finish_stages(
                &mut s,
                if stopped {
                    "cancelled"
                } else if result.is_ok() {
                    "succeeded"
                } else {
                    "failed"
                },
            );
        }
        println!(
            "deployment\t{}\nexit_code\t{}",
            s["phase"].as_str().unwrap(),
            s["exitCode"]
        );
    }
    fn cleanup(&self) -> bool {
        let mut container = self.0.container.lock().unwrap();
        if let Some(name) = container.as_ref() {
            if !cluster::remove_toolbox(name) {
                return false;
            }
        }
        *container = None;
        let mut lock = self.0.lock.lock().unwrap();
        if let Some(path) = lock.as_ref() {
            if let Err(e) = fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return false;
                }
            }
        }
        *lock = None;
        true
    }
    pub fn stop(&self) {
        if !self.active() {
            return;
        }
        self.0.status.lock().unwrap()["phase"] = json!("stopping");
        self.0.cancel.lock().unwrap().cancel();
        if let Some(task) = self.0.task.lock().unwrap().take() {
            let _ = task.join();
        }
        if self.cleanup() {
            let mut s = self.0.status.lock().unwrap();
            s["phase"] = json!("cancelled");
            s["stopFailed"] = json!(false);
            s["finishedAt"] = json!(chrono::Utc::now().to_rfc3339());
            let message = "部署已停止。已完成的变更保留，集群任务可能继续运行；重新部署会先检查 Helm 状态。";
            s["message"] = json!(message);
            if let Some(path) = s["log"].as_str() {
                if let Ok(mut file) = OpenOptions::new().append(true).open(path) {
                    let _ = writeln!(file, "{message}");
                }
            }
            *self.0.completed.lock().unwrap() = None;
        }
    }
    pub fn artifacts(&self) -> Result<Value> {
        if self.snapshot()["phase"] != "succeeded" {
            return Err(fail("部署成功后才可读取访问配置。", 409));
        }
        self.0
            .completed
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, a)| a.clone())
            .ok_or_else(|| fail("没有本次部署的访问配置。", 409))
    }
    pub fn credentials(&self, cancel: &Cancellation) -> Result<Value> {
        if self.snapshot()["phase"] != "succeeded" {
            return Err(fail("部署成功后才可读取凭据。", 409));
        }
        let target = self
            .0
            .completed
            .lock()
            .unwrap()
            .as_ref()
            .map(|(t, _)| t.clone())
            .ok_or_else(|| fail("没有已完成的部署。", 409))?;
        super::access::credentials(&target, cancel)
    }
}
