use super::process::{self, Cancellation};
use super::*;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tiny_http::{Header, Request, Response, Server, StatusCode};
struct State {
    path: PathBuf,
    gateway: Option<String>,
    origin: String,
    host: String,
    base: String,
    nonce: String,
    stop: AtomicBool,
    cancel: Cancellation,
    gate: Mutex<()>,
    deployment: deploy::Deployment,
    access: access::LocalAccess,
    verification: Mutex<Value>,
    deployed_revision: Mutex<Value>,
    probe: Mutex<Option<(std::time::Instant, Value)>>,
    probe_cancel: Mutex<Cancellation>,
}
struct Reply {
    status: u16,
    body: Vec<u8>,
    kind: &'static str,
    disposition: Option<&'static str>,
}
impl Reply {
    fn json(value: Value, status: u16) -> Self {
        Self {
            status,
            body: value.to_string().into_bytes(),
            kind: "application/json; charset=utf-8",
            disposition: None,
        }
    }
    fn bytes(body: Vec<u8>, kind: &'static str) -> Self {
        Self {
            status: 200,
            body,
            kind,
            disposition: None,
        }
    }
}
fn header<'a>(r: &'a Request, key: &str) -> Option<&'a str> {
    r.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(key))
        .map(|h| h.value.as_str())
}
fn empty(input: &Value) -> Result<()> {
    if input.as_object().is_none_or(|v| !v.is_empty()) {
        return Err(fail("请求不接受路径、命令或凭据参数。", 400));
    }
    Ok(())
}
impl State {
    fn busy(&self) -> Result<()> {
        if self.stop.load(Ordering::Acquire) || self.deployment.active() || self.access.active() {
            Err(fail("部署、配置或测试正在进行，请稍后重试。", 409))
        } else {
            Ok(())
        }
    }
    fn route(&self, r: &mut Request) -> Result<Reply> {
        if header(r, "host") != Some(&self.host) || header(r, "sec-fetch-site") == Some("cross-site") {
            return Err(fail("请求来源不允许。", 403));
        }
        let url = r.url().to_owned();
        if !url.starts_with(&self.base) || url.contains('?') {
            return Err(fail("页面不存在。", 404));
        }
        let route = &url[self.base.len()..];
        let method = r.method().as_str();
        if method == "GET" {
            return match route {
                "" => Ok(Reply::bytes(
                    include_str!("../../assets/setup.html")
                        .replace("APEIRON_NONCE_PLACEHOLDER", &self.nonce)
                        .into_bytes(),
                    "text/html; charset=utf-8",
                )),
                "favicon.ico" => Ok(Reply::bytes(
                    include_bytes!("../../../src/init/assets/apeiron-favicon.ico").to_vec(),
                    "image/vnd.microsoft.icon",
                )),
                "api/config" => {
                    let mut data = config::public(&config::read(&self.path)?);
                    data["connections"] = pairing::list(&self.path)?;
                    if let Some(slug) = &self.gateway {
                        data["gateway"] = json!({"slug":slug});
                    }
                    data["apps"] = config::apps();
                    data["path"] = json!(self.path);
                    data["host"] = discovery::host();
                    data["defaults"] = config::defaults();
                    data["deployment"] = self.deployment.snapshot();
                    Ok(Reply::json(data, 200))
                }
                "api/deployment" => Ok(Reply::json(self.deployment.snapshot(), 200)),
                "api/access" => Ok(Reply::json(
                    json!({"capability":access::capability(),"status":self.access.snapshot()}),
                    200,
                )),
                "api/verification" => Ok(Reply::json(json!({"result":*self.verification.lock().unwrap()}), 200)),
                "api/ssh-aliases" => Ok(Reply::json(discovery::aliases()?, 200)),
                "api/log" | "api/log/download" => {
                    if header(r, "origin").is_some_and(|o| o != self.origin) {
                        return Err(fail("请求来源不允许。", 403));
                    }
                    let snapshot = self.deployment.snapshot();
                    let path = snapshot["log"]
                        .as_str()
                        .ok_or_else(|| fail("尚未生成安装日志。", 404))?;
                    let mut reply = Reply::bytes(
                        resources::read(Path::new(path), 64 * 1024 * 1024)?,
                        "text/plain; charset=utf-8",
                    );
                    reply.disposition = Some(if route.ends_with("download") {
                        "attachment; filename=\"install.log\""
                    } else {
                        "inline; filename=\"install.log\""
                    });
                    Ok(reply)
                }
                "api/ca.crt" | "api/hosts.txt" => {
                    let a = self
                        .deployment
                        .artifacts()
                        .map_err(|_| fail("部署完成并生成文件后才可下载。", 404))?;
                    let ca = route.ends_with("ca.crt");
                    let body = a[if ca { "ca" } else { "hosts" }]
                        .as_str()
                        .ok_or_else(|| fail("文件尚未生成。", 404))?;
                    let mut reply = Reply::bytes(
                        body.as_bytes().to_vec(),
                        if ca {
                            "application/x-pem-file"
                        } else {
                            "text/plain; charset=utf-8"
                        },
                    );
                    reply.disposition = Some(if ca {
                        "attachment; filename=\"chentu-ca.crt\""
                    } else {
                        "attachment; filename=\"apeiron-hosts.txt\""
                    });
                    Ok(reply)
                }
                _ => Err(fail("页面不存在。", 404)),
            };
        }
        if method != "POST" {
            return Err(fail("页面不存在。", 404));
        }
        if header(r, "origin") != Some(&self.origin)
            || header(r, "content-type")
                .and_then(|s| s.split(';').next())
                .map(str::trim)
                != Some("application/json")
        {
            return Err(fail("请求来源或格式不允许。", 403));
        }
        if r.body_length().is_some_and(|n| n > 16384) {
            return Err(fail("请求超过大小限制。", 413));
        }
        let mut bytes = Vec::new();
        r.as_reader()
            .take(16385)
            .read_to_end(&mut bytes)
            .map_err(|_| fail("无法读取请求。", 400))?;
        if bytes.len() > 16384 {
            return Err(fail("请求超过大小限制。", 413));
        }
        let input: Value = serde_json::from_slice(&bytes).map_err(|_| fail("请求需为有效 JSON。", 400))?;
        if route == "api/deployment/stop" {
            empty(&input)?;
            if !self.deployment.active() || self.access.active() {
                return Err(fail("当前没有可停止的部署。", 409));
            }
            let deployment = self.deployment.clone();
            thread::spawn(move || deployment.stop());
            return Ok(Reply::json(json!({"deployment":self.deployment.snapshot()}), 202));
        }
        if route == "api/probe" {
            if self.stop.load(Ordering::Acquire) {
                return Err(fail("向导正在关闭。", 409));
            }
            let offline = input["offline"]
                .as_bool()
                .ok_or_else(|| fail("探测需要明确指定在线或离线模式。", 400))?;
            if input.get("refresh").is_some_and(|v| !v.is_boolean()) {
                return Err(fail("refresh 必须是布尔值。", 400));
            }
            let cancel = {
                let mut active = self.probe_cancel.lock().unwrap();
                active.cancel();
                *active = Cancellation::default();
                let mut cache = self.probe.lock().unwrap();
                if !offline && input["refresh"] != true {
                    if let Some((at, value)) = cache.as_ref() {
                        if at.elapsed() < Duration::from_secs(30) {
                            return Ok(Reply::json(value.clone(), 200));
                        }
                    }
                }
                *cache = None;
                active.clone()
            };
            let result = discovery::probe(offline, &cancel)?;
            let _active = self.probe_cancel.lock().unwrap();
            if !offline && cancel.check().is_ok() {
                *self.probe.lock().unwrap() = Some((std::time::Instant::now(), result.clone()));
            }
            return Ok(Reply::json(result, 200));
        }
        let _guard = self
            .gate
            .try_lock()
            .map_err(|_| fail("检测、保存或配置正在进行，请稍后重试。", 409))?;
        if self.stop.load(Ordering::Acquire) {
            return Err(fail("向导正在关闭。", 409));
        }
        let data = match route {
            "api/models/list" | "api/models/test" => {
                self.busy()?;
                let saved = config::read(&self.path)?;
                if input.get("revision") != saved.get("revision") {
                    return Err(fail("配置已变化，请刷新后重试。", 409));
                }
                let model = config::models(&input["models"], &saved["config"]["models"])?;
                models::request(
                    &model,
                    if route.ends_with("list") { "list" } else { "test" },
                    &self.cancel,
                )?
            }
            "api/connections/pair" | "api/connections/status" | "api/connections/test" | "api/connections/revoke" => {
                self.busy()?;
                if input.as_object().is_none_or(|o| o.len() != 1) {
                    return Err(fail("连接请求格式不正确。", 400));
                }
                let verb = route.rsplit('/').next().unwrap();
                let connection = if verb == "pair" {
                    pairing::pair(
                        &self.path,
                        input["code"].as_str().ok_or_else(|| fail("配对码格式不正确。", 400))?,
                        &self.cancel,
                    )?
                } else {
                    pairing::action(
                        &self.path,
                        verb,
                        input["id"].as_str().ok_or_else(|| fail("连接 ID 格式不正确。", 400))?,
                        &self.cancel,
                    )?
                };
                json!({"connection":connection,"connections":pairing::list(&self.path)?})
            }
            "api/config" | "api/deploy" => {
                self.busy()?;
                if self
                    .gateway
                    .as_ref()
                    .is_some_and(|slug| input["slug"].as_str() != Some(slug))
                {
                    return Err(fail("组织标识必须与网关申请的名称一致。", 400));
                }
                if route == "api/deploy" {
                    let current = config::read(&self.path)?;
                    let candidate = config::validate(&input, &current["config"])?;
                    cluster::preflight(&self.path, &candidate["deployment"]["installation"], &self.cancel)?;
                }
                let saved = config::save(&self.path, &input)?;
                println!("status\tsaved");
                let mut public = config::public(&saved);
                if route == "api/deploy" {
                    *self.deployed_revision.lock().unwrap() = saved["revision"].clone();
                    self.deployment.start(&self.path, saved["config"].clone())?;
                    self.access.reset();
                    *self.verification.lock().unwrap() = Value::Null;
                    public["deployment"] = self.deployment.snapshot();
                    return Ok(Reply::json(public, 202));
                }
                public
            }
            "api/deployment/retry" => {
                self.busy()?;
                if input.as_object().is_none_or(|o| o.len() != 1) || !input["revision"].is_string() {
                    return Err(fail("重新部署需要当前配置版本。", 400));
                }
                if ![json!("failed"), json!("cancelled")].contains(&self.deployment.snapshot()["phase"]) {
                    return Err(fail("等待部署完全停止或失败后重试。", 409));
                }
                let saved = config::read(&self.path)?;
                if input["revision"] != *self.deployed_revision.lock().unwrap()
                    || input["revision"] != saved["revision"]
                {
                    return Err(fail("配置已变化，请返回配置步骤确认。", 409));
                }
                cluster::preflight(&self.path, &saved["config"]["deployment"]["installation"], &self.cancel)?;
                self.deployment.start(&self.path, saved["config"].clone())?;
                self.access.reset();
                *self.verification.lock().unwrap() = Value::Null;
                return Ok(Reply::json(json!({"deployment":self.deployment.snapshot()}), 202));
            }
            "api/access/install" => {
                empty(&input)?;
                self.access.start(self.deployment.artifacts()?)?;
                return Ok(Reply::json(json!({"status":self.access.snapshot()}), 202));
            }
            "api/credentials" => {
                empty(&input)?;
                self.deployment.credentials(&self.cancel)?
            }
            "api/verification" => {
                empty(&input)?;
                if self.access.active() {
                    return Err(fail("本机访问配置正在进行。", 409));
                }
                let a = self.deployment.artifacts()?;
                let result = access::verify(&a["info"], &self.cancel)?;
                *self.verification.lock().unwrap() = result.clone();
                json!({"result":result})
            }
            "api/dns" => discovery::dns(&input, false, &self.cancel)?,
            "api/nodes" => {
                self.busy()?;
                discovery::nodes(&input, &self.cancel)?
            }
            "api/pick-bundle" => {
                self.busy()?;
                if !cfg!(target_os = "macos") {
                    return Err(fail("远程或无桌面环境请填写 CLI 主机上的绝对路径。", 400));
                }
                let path = process::text(
                    Command::new("osascript")
                        .args(["-e", "POSIX path of (choose folder with prompt \"选择离线安装包目录\")"]),
                    b"",
                    120,
                    8192,
                    &self.cancel,
                )
                .unwrap_or_default();
                json!({"path":path.trim()})
            }
            "api/finish" => {
                self.busy()?;
                if self.deployment.snapshot()["phase"] == "idle" {
                    return Err(fail("部署尚未开始。", 409));
                }
                self.stop.store(true, Ordering::Release);
                json!({"ok":true})
            }
            _ => return Err(fail("页面不存在。", 404)),
        };
        Ok(Reply::json(data, 200))
    }
    fn handle(&self, mut request: Request) {
        let reply = match self.route(&mut request) {
            Ok(r) => r,
            Err(e) => Reply::json(
                json!({"error":e.message}),
                if (400..600).contains(&e.code) {
                    e.code as u16
                } else {
                    500
                },
            ),
        };
        let mut response = Response::from_data(reply.body).with_status_code(StatusCode(reply.status));
        for (k, v) in [
            ("Content-Type", reply.kind),
            ("Cache-Control", "no-store"),
            ("Referrer-Policy", "no-referrer"),
            ("X-Content-Type-Options", "nosniff"),
        ] {
            response.add_header(Header::from_bytes(k, v).unwrap());
        }
        response.add_header(Header::from_bytes("Content-Security-Policy",format!("default-src 'none'; script-src 'nonce-{}'; style-src 'nonce-{}'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",self.nonce,self.nonce)).unwrap());
        if let Some(d) = reply.disposition {
            response.add_header(Header::from_bytes("Content-Disposition", d).unwrap());
        }
        let _ = request.respond(response);
    }
}
pub fn run(args: &[String]) -> Result<()> {
    let (pos, options) = parse(args, &["port", "config"], &["no-open", "help"])?;
    if options.contains_key("help") {
        println!("{}", include_str!("../../assets/init-help.txt"));
        return Ok(());
    }
    if !pos.is_empty() {
        return Err(fail("Unknown init argument", 2));
    }
    let port = options.get("port").map(String::as_str).unwrap_or("0");
    if !config::matches(r"^\d+$", port) {
        return Err(fail("--port must be an integer from 0 to 65535", 2));
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| fail("--port must be an integer from 0 to 65535", 2))?;
    let path = process::config_path(options.get("config"))?;
    serve(path, port, None, !options.contains_key("no-open"), None)
}
pub struct GatewayWizard {
    state: Arc<State>,
    task: Option<thread::JoinHandle<Result<()>>>,
}
impl GatewayWizard {
    pub fn url(&self) -> String {
        format!("{}{}", self.state.origin, self.state.base)
    }
    pub fn stopping(&self) -> bool {
        self.state.stop.load(Ordering::Acquire)
    }
    pub fn stop(&mut self) {
        self.state.cancel.cancel();
        self.state.stop.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}
impl Drop for GatewayWizard {
    fn drop(&mut self) {
        self.stop();
    }
}
pub fn start_gateway(path: PathBuf, slug: String) -> Result<GatewayWizard> {
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let task = thread::spawn(move || serve(path, 0, Some(slug), false, Some(send)));
    match receive.recv_timeout(Duration::from_secs(15)) {
        Ok(state) => Ok(GatewayWizard {
            state,
            task: Some(task),
        }),
        Err(_) => {
            let _ = task.join();
            Err(fail("无法启动网关部署向导。", 9))
        }
    }
}
fn serve(
    path: PathBuf,
    port: u16,
    gateway: Option<String>,
    open_browser: bool,
    ready: Option<std::sync::mpsc::SyncSender<Arc<State>>>,
) -> Result<()> {
    config::location(&path)?;
    config::read(&path)?;
    let server = Server::http(("127.0.0.1", port)).map_err(|_| fail("无法启动本地向导，请检查端口和权限。", 9))?;
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| fail("无法读取向导端口。", 9))?;
    let host = format!("127.0.0.1:{}", addr.port());
    let origin = format!("http://{host}");
    let base = format!(
        "/setup/{:016x}{:016x}{:016x}/",
        rand::random::<u64>(),
        rand::random::<u64>(),
        rand::random::<u64>()
    );
    let url = format!("{origin}{base}");
    let state = Arc::new(State {
        path: path.clone(),
        gateway,
        origin,
        host,
        base,
        nonce: format!(
            "{:016x}{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>(),
            rand::random::<u64>()
        ),
        stop: AtomicBool::new(false),
        cancel: Cancellation::default(),
        gate: Mutex::new(()),
        deployment: deploy::Deployment::new(),
        access: access::LocalAccess::new(),
        verification: Mutex::new(Value::Null),
        deployed_revision: Mutex::new(Value::Null),
        probe: Mutex::new(None),
        probe_cancel: Mutex::new(Cancellation::default()),
    });
    let signals = state.clone();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            #[cfg(unix)]
            {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
        });
        signals.cancel.cancel();
        signals.stop.store(true, Ordering::Release);
    });
    if let Some(ready) = ready {
        ready.send(state.clone()).map_err(|_| fail("网关向导启动已取消。", 9))?;
    } else {
        println!(
            "schema=apeiron.init.v1\nkey\tvalue\nstatus\tlistening\nurl\t{url}\nconfig\t{}",
            path.display()
        );
    }
    if open_browser {
        let command = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "windows") {
            "rundll32.exe"
        } else {
            "xdg-open"
        };
        let mut cmd = Command::new(command);
        if cfg!(target_os = "windows") {
            cmd.arg("url.dll,FileProtocolHandler");
        }
        if cmd
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_err()
        {
            eprintln!("browser: Could not open the browser; open the printed local URL manually.");
        }
    }
    let mut workers = Vec::new();
    while !state.stop.load(Ordering::Acquire) {
        workers.retain(|worker: &thread::JoinHandle<()>| !worker.is_finished());
        if let Some(request) = server
            .recv_timeout(Duration::from_millis(100))
            .map_err(|_| fail("本地向导服务失败。", 9))?
        {
            if workers.len() >= 16 {
                let _ = request.respond(Response::empty(StatusCode(503)));
                continue;
            }
            let state = state.clone();
            workers.push(thread::spawn(move || state.handle(request)));
        }
    }
    state.cancel.cancel();
    state.probe_cancel.lock().unwrap().cancel();
    state.access.wait();
    state.deployment.stop();
    for worker in workers {
        let _ = worker.join();
    }
    match state.deployment.snapshot()["phase"].as_str() {
        Some("failed" | "stopping") => Err(fail("Deployment did not complete successfully", 9)),
        Some("cancelled") => Err(fail("Deployment cancelled", 130)),
        _ => Ok(()),
    }
}
