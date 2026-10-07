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
#[derive(Clone)]
pub struct Deployment(Arc<Inner>);
struct Inner {
    status: Mutex<Value>,
    cancel: Mutex<Cancellation>,
    task: Mutex<Option<thread::JoinHandle<()>>>,
    completed: Mutex<Option<(Value, Value)>>,
    lock: Mutex<Option<PathBuf>>,
    container: Mutex<Option<String>>,
}
pub struct Context {
    pub config: PathBuf,
    pub directory: PathBuf,
    pub cancel: Cancellation,
    owner: Deployment,
    log: fs::File,
}
impl Context {
    pub fn event(&self, message: &str) {
        let tagged = format!("[INFO] {message}");
        let mut s = self.owner.0.status.lock().unwrap();
        s["message"] = json!(message);
        let events = s["events"].as_array_mut().unwrap();
        events.push(json!(tagged));
        if events.len() > 100 {
            events.remove(0);
        }
        drop(s);
        let _ = (&self.log).write_all(format!("[{}] {tagged}\n", chrono::Utc::now().to_rfc3339()).as_bytes());
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
        let mut child = cmd.spawn().map_err(|_| fail("无法启动部署工具。", 500))?;
        let mut stopping = None;
        loop {
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
                    self.cancel.check()?;
                    self.owner.0.status.lock().unwrap()["exitCode"] = json!(status.code().unwrap_or(130));
                    self.log
                        .sync_all()
                        .map_err(|_| fail("无法持久化安装日志，已停止安装。", 500))?;
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(fail(
                            format!(
                                "部署命令失败（退出码 {}），请查看本机安装日志。",
                                status.code().unwrap_or(130)
                            ),
                            500,
                        ))
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
pub fn environment_for(config: &Value, mut values: Value) -> Result<Value> {
    if !values.is_object() || values.get("profile").is_some() {
        return Err(fail("环境文件必须是普通 YAML 对象，且不能包含 profile。", 400));
    }
    if values.get("releases").is_none() {
        values["releases"] = json!({});
    }
    if !values["releases"].is_object() {
        return Err(fail("releases 必须是对象。", 400));
    }
    for app in config::apps().as_array().unwrap() {
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
            release["enabled"] = json!(config["apps"].as_array().unwrap().contains(&app["id"]));
        }
    }
    if config["deployment"]["installation"].is_object() && config["apps"].as_array().unwrap().contains(&json!("vasi")) {
        if values["releases"]["cluster-access"].is_null() {
            values["releases"]["cluster-access"] = json!({});
        }
        values["releases"]["cluster-access"]["enabled"] = json!(true);
    }
    for name in ["nexus", "apeiron"] {
        if values["releases"][name]["values"].is_null() {
            values["releases"][name]["values"] = json!({});
        }
        if !values["releases"][name]["values"].is_object() {
            return Err(fail("应用 values 必须是对象。", 400));
        }
    }
    values["releases"]["nexus"]["values"]["publicProxies"] = json!(config["deployment"]["offline"] != true);
    if config["models"].is_object() && config["apps"].as_array().unwrap().contains(&json!("apeiron")) {
        let v = &mut values["releases"]["apeiron"]["values"];
        v["defaultModel"] = json!("");
        v["allowedModels"] = json!("");
        v["models"] = models::values(&config["models"]);
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
        }))
    }
    pub fn snapshot(&self) -> Value {
        self.0.status.lock().unwrap().clone()
    }
    pub fn active(&self) -> bool {
        ["preparing", "running", "stopping"].contains(&self.snapshot()["phase"].as_str().unwrap_or(""))
    }
    pub fn start(&self, path: &Path, config: Value) -> Result<()> {
        if self.active() {
            return Err(fail("部署正在进行。", 409));
        }
        if let Some(task) = self.0.task.lock().unwrap().take() {
            let _ = task.join();
        }
        *self.0.cancel.lock().unwrap() = Cancellation::default();
        *self.0.completed.lock().unwrap() = None;
        *self.0.status.lock().unwrap() = json!({"phase":"preparing","startedAt":chrono::Utc::now().to_rfc3339(),"message":"正在检查部署配置…","events":[]});
        let owner = self.clone();
        let path = path.to_owned();
        *self.0.task.lock().unwrap() = Some(thread::spawn(move || owner.execute(path, config)));
        Ok(())
    }
    fn execute(&self, path: PathBuf, config: Value) {
        let cancel = self.0.cancel.lock().unwrap().clone();
        let result = (|| {
            let directory = path
                .parent()
                .unwrap()
                .join("deployments")
                .join(format!("run-{:016x}", rand::random::<u64>()));
            process::private_directory(&directory)?;
            let log_path = directory.join("install.log");
            private_write(&log_path, b"")?;
            self.0.status.lock().unwrap()["log"] = json!(log_path);
            let log = OpenOptions::new()
                .append(true)
                .open(&log_path)
                .map_err(|_| fail("无法打开安装日志。", 500))?;
            let context = Context {
                config: path.clone(),
                directory: directory.clone(),
                cancel: cancel.clone(),
                owner: self.clone(),
                log,
            };
            let mut target = config["deployment"].clone();
            let fresh = target["installation"].is_object();
            let lock_path = if fresh {
                PathBuf::from(format!("{}.installation.lock", path.display()))
            } else {
                let env = fs::canonicalize(target["environment"].as_str().unwrap_or(""))
                    .map_err(|_| fail("环境 values 文件不存在。", 400))?;
                PathBuf::from(format!("{}.apeiron-deploy.lock", env.display()))
            };
            private_write(&lock_path, std::process::id().to_string().as_bytes())
                .map_err(|_| fail("此环境已有部署锁，请检查安装进程。", 409))?;
            *self.0.lock.lock().unwrap() = Some(lock_path);
            let env = if fresh {
                let prepared = super::bootstrap::prepare(&config, &context)?;
                target = prepared.target.clone();
                resources::write_yaml(
                    Path::new(target["environment"].as_str().unwrap()),
                    &environment_for(&config, prepared.values.clone())?,
                )?;
                super::bootstrap::bootstrap(&mut target, &context, &prepared)?
            } else {
                target["root"] = json!(resources::resolve(&target, &cancel, &|m| context.event(m))?);
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
            resources::write_yaml(&output, &environment_for(&config, resources::yaml(input)?)?)?;
            self.0.status.lock().unwrap()["environment"] = json!(output);
            let mut env = env;
            env.insert("CHENTU_ENV".into(), output.to_string_lossy().into_owned());
            if target["runner"] == "docker" {
                env.insert("LAB_ENV".into(), output.to_string_lossy().into_owned());
                if let Some(name) = env.get("LAB_CONTAINER") {
                    *self.0.container.lock().unwrap() = Some(name.clone());
                }
            }
            context.event("检查 Helm 是否存在未完成的操作。");
            cluster::helm_state(&target, &env, &cancel)?;
            if config["models"].is_object() && config["apps"].as_array().unwrap().contains(&json!("apeiron")) {
                cluster::model_key(&config["models"], &target, &env, &cancel)?;
            }
            self.0.status.lock().unwrap()["phase"] = json!("running");
            let root = Path::new(target["root"].as_str().unwrap());
            let script = root.join(if target["runner"] == "docker" {
                "tests/lab/helmfile.sh"
            } else {
                "deploy/helmfile/run.sh"
            });
            context.event("正在运行 Helmfile sync。");
            context.run(
                "bash",
                &[script.to_string_lossy().into_owned(), "sync".into()],
                root,
                &env,
            )?;
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
            Ok(())
        })();
        let stopped = cancel.check().is_err();
        let cleanup = self.cleanup();
        let mut s = self.0.status.lock().unwrap();
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
                s["exitCode"].as_i64().unwrap_or(9)
            });
            if let Err(error) = result {
                s["message"] = json!(error.message);
            }
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
