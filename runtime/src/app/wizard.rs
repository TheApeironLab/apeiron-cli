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
    path: Mutex<PathBuf>,
    gateway: Option<String>,
    origin: String,
    advertise: Option<String>,
    base: String,
    nonce: String,
    stop: AtomicBool,
    cancel: Cancellation,
    gate: Mutex<()>,
    deployment: deploy::Deployment,
    access: access::LocalAccess,
    verification: Mutex<Value>,
    deployed_revision: Mutex<Value>,
    probe: discovery::EnvironmentProbe,
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
fn split_host(host: &str) -> (&str, Option<&str>) {
    host.rsplit_once(':')
        .filter(|(name, _)| !name.contains(':'))
        .map_or((host, None), |(name, port)| (name, Some(port)))
}
/// The wizard only listens on loopback. SSH port forwarding presents the same
/// loopback names with arbitrary local ports; any other name must be opted in
/// with `--advertise-host`. DNS-rebinding hosts never match.
pub fn host_allowed(host: Option<&str>, advertise: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let (name, port) = split_host(host);
    if !port.is_none_or(|p| config::matches(r"^[1-9]\d{0,4}$", p) && p.parse::<u16>().is_ok()) {
        return false;
    }
    if ["127.0.0.1", "localhost"].contains(&name) {
        return true;
    }
    advertise.is_some_and(|a| {
        let (expected, expected_port) = split_host(a);
        expected.eq_ignore_ascii_case(name) && expected_port.is_none_or(|p| Some(p) == port)
    })
}
/// Browsers send the Origin for the Host they connected to; cross-origin
/// requests still fail because the Host allowlist and Origin must agree.
pub fn origin_allowed(origin: Option<&str>, host: &str) -> bool {
    origin == Some(format!("http://{host}").as_str())
}
pub fn advertise_host(value: &str) -> Result<String> {
    let (name, port) = split_host(value);
    if !config::safe_host(name)
        || ["127.0.0.1", "localhost"].contains(&name)
        || !port.is_none_or(|p| config::matches(r"^[1-9]\d{0,4}$", p) && p.parse::<u16>().is_ok())
    {
        return Err(fail("--advertise-host must be <host[:port]>", 2));
    }
    Ok(value.to_ascii_lowercase())
}
impl State {
    fn busy(&self) -> Result<()> {
        if self.stop.load(Ordering::Acquire) || self.deployment.active() || self.access.active() {
            Err(fail("部署、配置或测试正在进行，请稍后重试。", 409))
        } else {
            Ok(())
        }
    }
    fn path(&self) -> PathBuf {
        self.path.lock().unwrap().clone()
    }
    fn instances(&self, route: &str, input: Option<&Value>) -> Result<Reply> {
        let path = self.path();
        let base = instances::base()?;
        let runner = instances::Host;
        let Some(input) = input else {
            if route == "api/instances" {
                // Best effort: a busy scheduler lock must not hide the listing.
                let _ = instances::adopt(&base, &path, &self.cancel);
                return Ok(Reply::json(instances::list(&base, &path, &runner)?, 200));
            }
            let id = route
                .strip_prefix("api/instances/")
                .and_then(|rest| rest.strip_suffix("/status"))
                .filter(|id| config::matches(r"^[a-f0-9]{12}$", id))
                .ok_or_else(|| fail("页面不存在。", 404))?;
            let registry = instances::read_registry(&base)?;
            return Ok(Reply::json(
                instances::status(instances::entry(&registry, id)?, &runner),
                200,
            ));
        };
        if self.gateway.is_some() {
            return Err(fail("网关部署向导绑定单一组织，不支持切换实例。", 409));
        }
        self.busy()?;
        let data = match route {
            "api/instances/create" => {
                let select = input.get("select").map(Value::as_bool).unwrap_or(Some(false));
                if input
                    .as_object()
                    .is_none_or(|o| o.len() != 2 + usize::from(input.get("select").is_some()))
                    || select.is_none()
                {
                    return Err(fail("创建实例需要 name 和 topology（可选 select）。", 400));
                }
                let name = input["name"].as_str().unwrap_or("");
                let topology = input["topology"].as_str().unwrap_or("");
                // Idempotent for the instance the wizard already runs in (start.sh
                // with APEIRON_INSTANCE registers it on open): same name and topology
                // return the existing record instead of a duplicate-name conflict.
                let _ = instances::adopt(&base, &path, &self.cancel);
                let existing = instances::read_registry(&base)
                    .ok()
                    .and_then(|r| instances::entry(&r, &instances::id_for(&path)).ok().cloned())
                    .filter(|e| e["name"] == name);
                if let Some(entry) = existing {
                    if entry["topology"] != topology {
                        // Changing only the index leaves the saved configuration
                        // and reserved ports on the original topology. Creation
                        // is idempotent only for the same installation identity.
                        return Err(fail("当前实例已按其他拓扑登记，请使用新的实例名称。", 409));
                    }
                    json!({"instance":instances::status(&entry, &runner),"existing":true,"selected":true})
                } else {
                    let source = config::read(&path).map(|c| c["config"].clone()).unwrap_or(Value::Null);
                    let root = instances::root(&path, &base);
                    let creation = instances::Creation {
                        base: &base,
                        root: &root,
                        source: &source,
                        bind: &instances::bindable,
                        runner: &runner,
                        cancel: &self.cancel,
                    };
                    let entry = instances::create(&creation, name, topology)?;
                    let mut data =
                        json!({"instance":instances::status(&entry, &runner),"existing":false,"selected":false});
                    if select == Some(true) {
                        data = self.select(&entry, &runner)?;
                        data["existing"] = json!(false);
                        data["selected"] = json!(true);
                    }
                    data
                }
            }
            "api/instances/select" => {
                if input.as_object().is_none_or(|o| o.len() != 1) {
                    return Err(fail("切换实例只接受 id 或 name。", 400));
                }
                let registry = instances::read_registry(&base)?;
                let entry = instances::entry(&registry, &instances::resolve_id(&registry, input)?)?.clone();
                self.select(&entry, &runner)?
            }
            "api/instances/delete" => {
                if input.as_object().is_none_or(|o| o.len() != 1) {
                    return Err(fail("删除实例只接受 id 或 name。", 400));
                }
                let id = instances::resolve_id(&instances::read_registry(&base)?, input)?;
                instances::delete(&base, &id, &path, &runner, &self.cancel)?
            }
            _ => return Err(fail("页面不存在。", 404)),
        };
        Ok(Reply::json(data, 200))
    }
    fn select(&self, entry: &Value, runner: &dyn instances::Runner) -> Result<Value> {
        let target = PathBuf::from(entry["config"].as_str().unwrap());
        config::location(&target)?;
        config::read(&target)?;
        self.deployment.reload(&target)?;
        *self.path.lock().unwrap() = target.clone();
        self.access.reset();
        *self.verification.lock().unwrap() = Value::Null;
        *self.deployed_revision.lock().unwrap() = Value::Null;
        println!("status\tselected\nconfig\t{}", target.display());
        let mut public = config::public(&config::read(&target)?);
        public["deployment"] = self.deployment.snapshot();
        Ok(
            json!({"instance":instances::status(entry, runner),"path":target,"config":public["config"],"revision":public["revision"],"deployment":public["deployment"]}),
        )
    }
    /// The registry record for the configuration the wizard currently edits, if any.
    fn current_instance(&self, path: &Path) -> Value {
        let Ok(base) = instances::base() else {
            return Value::Null;
        };
        let _ = instances::adopt(&base, path, &self.cancel);
        instances::read_registry(&base)
            .ok()
            .and_then(|r| instances::entry(&r, &instances::id_for(path)).ok().cloned())
            .map_or(Value::Null, |e| instances::status(&e, &instances::Host))
    }
    fn route(&self, r: &mut Request) -> Result<Reply> {
        let host = header(r, "host").map(str::to_owned);
        if !host_allowed(host.as_deref(), self.advertise.as_deref())
            || header(r, "sec-fetch-site") == Some("cross-site")
        {
            return Err(fail("请求来源不允许。", 403));
        }
        let host = host.unwrap();
        let path = self.path();
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
                    let mut data = config::public(&config::read(&path)?);
                    data["connections"] = pairing::list(&path)?;
                    if let Some(slug) = &self.gateway {
                        data["gateway"] = json!({"slug":slug});
                    }
                    data["apps"] = config::apps();
                    data["path"] = json!(path);
                    data["host"] = discovery::host();
                    data["defaults"] = config::defaults();
                    data["deployment"] = self.deployment.snapshot();
                    data["instance"] = self.current_instance(&path);
                    Ok(Reply::json(data, 200))
                }
                "api/deployment" => Ok(Reply::json(self.deployment.snapshot(), 200)),
                "api/access" => Ok(Reply::json(
                    json!({"capability":access::capability(),"status":self.access.snapshot()}),
                    200,
                )),
                "api/verification" => Ok(Reply::json(json!({"result":*self.verification.lock().unwrap()}), 200)),
                "api/ssh-aliases" => Ok(Reply::json(discovery::aliases()?, 200)),
                "api/instances" => self.instances(route, None),
                _ if route.starts_with("api/instances/") => self.instances(route, None),
                "api/log" | "api/log/download" => {
                    if header(r, "origin").is_some_and(|o| !origin_allowed(Some(o), &host)) {
                        return Err(fail("请求来源不允许。", 403));
                    }
                    let snapshot = self.deployment.snapshot();
                    let path = snapshot["log"]
                        .as_str()
                        .ok_or_else(|| fail("尚未生成安装日志。", 404))?;
                    let mut reply = Reply::bytes(
                        resources::read(Path::new(path), 64 * 1024 * 1024)
                            .map_err(|_| fail("安装日志不可用。", 404))?,
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
        if !origin_allowed(header(r, "origin"), &host)
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
            let result = self.probe.run(offline, input["refresh"] == true)?;
            return Ok(Reply::json(result, 200));
        }
        let _guard = self
            .gate
            .try_lock()
            .map_err(|_| fail("检测、保存或配置正在进行，请稍后重试。", 409))?;
        if self.stop.load(Ordering::Acquire) {
            return Err(fail("向导正在关闭。", 409));
        }
        if route.starts_with("api/instances/") {
            return self.instances(route, Some(&input));
        }
        let data = match route {
            "api/models/list" | "api/models/test" => {
                self.busy()?;
                let saved = config::read(&path)?;
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
                        &path,
                        input["code"].as_str().ok_or_else(|| fail("配对码格式不正确。", 400))?,
                        &self.cancel,
                    )?
                } else {
                    pairing::action(
                        &path,
                        verb,
                        input["id"].as_str().ok_or_else(|| fail("连接 ID 格式不正确。", 400))?,
                        &self.cancel,
                    )?
                };
                json!({"connection":connection,"connections":pairing::list(&path)?})
            }
            "api/config" | "api/deploy" => {
                self.busy()?;
                let mut preflight = String::new();
                if self
                    .gateway
                    .as_ref()
                    .is_some_and(|slug| input["slug"].as_str() != Some(slug))
                {
                    return Err(fail("组织标识必须与网关申请的名称一致。", 400));
                }
                if route == "api/deploy" {
                    if resume::load(&path)?.is_some() {
                        return Err(fail("存在原部署检查点，请继续原部署；不会覆盖配置启动另一安装。", 409));
                    }
                    let current = config::read(&path)?;
                    let candidate = config::validate(&input, &current["config"])?;
                    preflight = cluster::preflight(&path, &candidate["deployment"]["installation"], &self.cancel)?;
                }
                let saved = config::save(&path, &input)?;
                println!("status\tsaved");
                let mut public = config::public(&saved);
                if route == "api/deploy" {
                    *self.deployed_revision.lock().unwrap() = saved["revision"].clone();
                    self.deployment.start(&path, saved["config"].clone())?;
                    self.access.reset();
                    *self.verification.lock().unwrap() = Value::Null;
                    public["deployment"] = self.deployment.snapshot();
                    public["preflight"] = json!(preflight);
                    return Ok(Reply::json(public, 202));
                }
                public
            }
            "api/preflight" => {
                // Read-only coexistence/port preflight for the summary step; nothing is saved.
                self.busy()?;
                let current = config::read(&path)?;
                let candidate = config::validate(&input, &current["config"])?;
                let installation = candidate["deployment"]["installation"].clone();
                if installation.is_null() {
                    return Err(fail("请填写部署环境。", 400));
                }
                match cluster::preflight(&path, &installation, &self.cancel) {
                    Ok(preflight) => json!({"preflight":preflight,"blocked":Value::Null,"installation":installation}),
                    Err(e) if (400..500).contains(&e.code) => {
                        json!({"preflight":"","blocked":e.message,"installation":installation})
                    }
                    Err(e) => return Err(e),
                }
            }
            "api/deployment/resume" => {
                self.busy()?;
                if input.as_object().is_none_or(|o| o.len() != 1) || !input["revision"].is_string() {
                    return Err(fail("继续部署需要当前配置版本。", 400));
                }
                let saved = config::read(&path)?;
                if input["revision"] != saved["revision"] {
                    return Err(fail("配置已变化。", 409));
                }
                self.deployment.resume(&path, saved["config"].clone())?;
                *self.deployed_revision.lock().unwrap() = saved["revision"].clone();
                self.access.reset();
                *self.verification.lock().unwrap() = Value::Null;
                return Ok(Reply::json(json!({"deployment":self.deployment.snapshot()}), 202));
            }
            "api/deployment/retry" => {
                self.busy()?;
                if input.as_object().is_none_or(|o| o.len() != 1) || !input["revision"].is_string() {
                    return Err(fail("重新部署需要当前配置版本。", 400));
                }
                if ![json!("failed"), json!("cancelled")].contains(&self.deployment.snapshot()["phase"]) {
                    return Err(fail("等待部署完全停止或失败后重试。", 409));
                }
                let saved = config::read(&path)?;
                if input["revision"] != *self.deployed_revision.lock().unwrap()
                    || input["revision"] != saved["revision"]
                {
                    return Err(fail("配置已变化，请返回配置步骤确认。", 409));
                }
                let preflight =
                    cluster::preflight(&path, &saved["config"]["deployment"]["installation"], &self.cancel)?;
                self.deployment.start(&path, saved["config"].clone())?;
                self.access.reset();
                *self.verification.lock().unwrap() = Value::Null;
                return Ok(Reply::json(
                    json!({"deployment":self.deployment.snapshot(),"preflight":preflight}),
                    202,
                ));
            }
            "api/access/install" => {
                let artifacts = self.deployment.artifacts()?;
                empty(&input)?;
                self.access.start(artifacts)?;
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
    let (pos, options) = parse(
        args,
        &["port", "config", "instance", "advertise-host"],
        &["no-open", "help"],
    )?;
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
    let path = match (options.get("config"), options.get("instance")) {
        (Some(_), Some(_)) => return Err(fail("Use either --config or --instance", 2)),
        (None, Some(name)) => instances::path_for(&instances::base()?, name)?,
        (config, None) => process::config_path(config)?,
    };
    let advertise = options.get("advertise-host").map(|v| advertise_host(v)).transpose()?;
    serve(path, port, None, !options.contains_key("no-open"), None, advertise)
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
    let task = thread::spawn(move || serve(path, 0, Some(slug), false, Some(send), None));
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
    advertise: Option<String>,
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
        path: Mutex::new(path.clone()),
        gateway,
        origin,
        advertise,
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
        deployment: deploy::Deployment::restore(&path)?,
        access: access::LocalAccess::new(),
        verification: Mutex::new(Value::Null),
        deployed_revision: Mutex::new(Value::Null),
        probe: discovery::EnvironmentProbe::default(),
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
            "schema=apeiron.init.v1\nkey\tvalue\nstatus\tlistening\nurl\t{url}\nconfig\t{}\nforward\tssh -L {port}:127.0.0.1:{port} <user>@<this-host>  # then open the url above",
            path.display(),
            port = addr.port()
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
    state.probe.cancel();
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

#[cfg(test)]
mod contracts {
    use super::*;
    #[test]
    fn host_allowlist_accepts_loopback_forwarding_and_only_advertised_names() {
        // 1. Loopback names on any local port: SSH -L may pick another port.
        for host in ["127.0.0.1:3210", "127.0.0.1:49152", "localhost:3210", "localhost"] {
            assert!(host_allowed(Some(host), None), "{host}");
        }
        // 2. Other names are rejected without an explicit advertisement.
        for host in ["wizard.lan:3210", "10.0.0.5:3210", "[::1]:3210"] {
            assert!(!host_allowed(Some(host), None), "{host}");
        }
        // 3. Rebinding and malformed ports never match a loopback name.
        for host in [
            "127.0.0.1.evil.example:80",
            "evil.localhost:80",
            "localhost:abc",
            "localhost:0",
            "127.0.0.1:",
            "",
        ] {
            assert!(!host_allowed(Some(host), None), "{host}");
        }
        assert!(!host_allowed(None, None));
        // 4. Advertised host matches exactly; a pinned port must agree.
        assert!(host_allowed(Some("Wizard.lan:3210"), Some("wizard.lan")));
        assert!(host_allowed(Some("wizard.lan:3210"), Some("wizard.lan:3210")));
        assert!(!host_allowed(Some("wizard.lan:3211"), Some("wizard.lan:3210")));
        assert!(!host_allowed(Some("other.lan:3210"), Some("wizard.lan")));
        assert!(origin_allowed(Some("http://localhost:3210"), "localhost:3210"));
        assert!(!origin_allowed(Some("http://127.0.0.1:3210"), "localhost:3210"));
        assert!(!origin_allowed(Some("https://localhost:3210"), "localhost:3210"));
        assert!(!origin_allowed(None, "localhost:3210"));
        assert_eq!(advertise_host("Wizard.lan:3210").unwrap(), "wizard.lan:3210");
        for value in ["localhost", "127.0.0.1:3210", "a b", "wizard.lan:0", "http://x"] {
            assert!(advertise_host(value).is_err(), "{value}");
        }
    }
    #[test]
    fn forwarded_loopback_hosts_reach_the_wizard_and_foreign_hosts_do_not() {
        let temp = crate::app::contracts::Temp::new();
        let mut server = start_gateway(temp.0.join("config.json"), "fixture".into()).unwrap();
        let client = reqwest::blocking::Client::new();
        let port = server.state.origin.rsplit(':').next().unwrap().to_owned();
        let url = format!("{}api/deployment", server.url());
        for (host, status) in [
            (format!("localhost:{port}"), 200),
            (format!("127.0.0.1:{port}"), 200),
            ("localhost:65000".to_owned(), 200),
            ("wizard.lan:3210".to_owned(), 403),
        ] {
            let response = client.get(&url).header("Host", &host).send().unwrap();
            assert_eq!(response.status().as_u16(), status, "{host}");
        }
        let post = client
            .post(format!("{}api/finish", server.url()))
            .header("Host", format!("localhost:{port}"))
            .header("Origin", format!("http://127.0.0.1:{port}"))
            .json(&json!({}))
            .send()
            .unwrap();
        assert_eq!(post.status().as_u16(), 403, "origin must match the request host");
        let listing: Value = client
            .get(format!("{}api/instances/0123456789ab/status", server.url()))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert!(listing["error"].is_string());
        server.stop();
    }
    #[test]
    fn pending_local_authorization_blocks_mutations_finish_and_duplicate_installation() {
        let temp = crate::app::contracts::Temp::new();
        let a = access::contracts::artifacts(&temp);
        let mut server = start_gateway(temp.0.join("config.json"), "fixture".into()).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        server
            .state
            .access
            .start_with(a, json!({"available":true}), move |_| {
                receive.recv().unwrap();
                Ok(json!({"phase":"succeeded","message":"fixture"}))
            })
            .unwrap();
        let client = reqwest::blocking::Client::new();
        for route in ["config", "deploy", "finish", "access/install"] {
            let response = client
                .post(format!("{}api/{route}", server.url()))
                .header("Origin", &server.state.origin)
                .json(&json!({}))
                .send()
                .unwrap();
            assert_eq!(response.status().as_u16(), 409, "{route}");
        }
        let status: Value = client
            .get(format!("{}api/access", server.url()))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(status["status"]["phase"], "installing");
        send.send(()).unwrap();
        server.state.access.wait();
        assert_eq!(server.state.access.snapshot()["phase"], "succeeded");
        server.stop();
    }
}
