//! Process/HTTP contracts: fixtures model the public wire, never Rust internals.
use serde_json::{json, Value};
#[cfg(unix)]
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

const BIN: &str = env!("CARGO_BIN_EXE_apeiron");
#[derive(Clone, Debug, PartialEq)]
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}
struct Response {
    status: u16,
    body: String,
    delay: Duration,
    partial: bool,
}
impl Response {
    fn json(value: Value) -> Self {
        Self {
            status: 200,
            body: value.to_string(),
            delay: Duration::ZERO,
            partial: false,
        }
    }
}
struct Server {
    url: String,
    seen: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(Request) -> Response + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let requests = seen.clone();
        let handler = Arc::new(handler);
        let handle = thread::spawn(move || {
            let mut connections = Vec::new();
            while !done.load(Ordering::Relaxed) {
                if let Ok((stream, _)) = listener.accept() {
                    let handler = handler.clone();
                    let requests = requests.clone();
                    connections.push(thread::spawn(move || serve(stream, &*handler, requests)));
                } else {
                    thread::sleep(Duration::from_millis(2));
                }
            }
            for connection in connections {
                connection.join().unwrap();
            }
        });
        Self {
            url,
            seen,
            stop,
            handle: Some(handle),
        }
    }
    fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap();
    }
}
fn serve(mut stream: TcpStream, handler: &dyn Fn(Request) -> Response, seen: Arc<Mutex<Vec<Request>>>) {
    // macOS inherits O_NONBLOCK from the listener; each connection is served
    // on its own blocking thread, including fragmented request bodies.
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let end = loop {
        let n = stream.read(&mut buffer).expect("fixture request headers");
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..end]);
    let mut lines = head.lines();
    let first: Vec<_> = lines.next().unwrap().split_whitespace().collect();
    let method = first[0].to_owned();
    let path = first[1].to_owned();
    let headers: HashMap<_, _> = lines
        .filter_map(|s| s.split_once(':'))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let size = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < end + size {
        let n = stream.read(&mut buffer).unwrap_or(0);
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    let request = Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&bytes[end..end + size]).into_owned(),
    };
    seen.lock().unwrap().push(request.clone());
    let response = handler(request);
    if response.partial {
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10000\r\nConnection: close\r\n\r\n{").unwrap();
        let _ = stream.flush();
        thread::sleep(response.delay);
    } else {
        thread::sleep(response.delay);
        let wire = format!(
            "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.status,
            response.body.len(),
            response.body
        );
        stream.write_all(wire.as_bytes()).unwrap();
        if let Err(error) = stream.shutdown(std::net::Shutdown::Write) {
            // A signal test deliberately closes the client before its delayed
            // response. Linux reports ENOTCONN here; macOS permits shutdown.
            assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);
        }
        let _ = stream.read(&mut [0; 1]);
    }
}
struct User {
    home: PathBuf,
}
impl User {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!(
            "apeiron-kaneo-{}-{:x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir(&home).unwrap();
        Self { home }
    }
    fn command(&self, url: &str, args: &[&str]) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.args(["task"])
            .args(args)
            .env("KANEO_API_URL", url)
            .env("KANEO_ACCESS_TOKEN", "fixture-token")
            .env("XDG_CONFIG_HOME", &self.home)
            .env_remove("KANEO_WORKSPACE_ID")
            .env_remove("KANEO_ACCESS_TOKEN_FILE")
            .env_remove("KANEO_API_KEY")
            .env_remove("KANEO_API_KEY_FILE")
            .env("APEIRON_TOKEN", "unrelated-machine-token")
            .env_remove("HTTPS_PROXY")
            .env_remove("HTTP_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("https_proxy")
            .env_remove("http_proxy")
            .env_remove("all_proxy")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .env_remove("REQUEST_METHOD");
        cmd
    }
    fn run(&self, url: &str, args: &[&str]) -> Output {
        self.command(url, args).output().unwrap()
    }
}
impl Drop for User {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}
fn value(output: &Output, code: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn identity() -> Value {
    json!({"user":{"id":"user-1","name":"User","email":"fixture@example.test","secret":"redact"},"auth":{"type":"bearer","clientId":"broker","expiresAt":"2099-01-01T00:00:00.000Z"},"workspaces":[]})
}

#[test]
fn offline_help_and_machine_commands_do_not_initialize_app_auth() {
    let user = User::new();
    for args in [vec!["--help"], vec!["--version"], vec!["create", "--help"]] {
        let output = user
            .command("invalid", &args)
            .env("KANEO_API_KEY", "ambiguous")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    }
    let help = user.run("invalid", &["--help"]);
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("--workspace <ID>"));
    assert!(help.contains("not its name or slug"));
    assert!(help.contains("workspace list --json"));
    assert!(help.contains("project list --workspace <ID> --json"));
    assert!(help.contains("task create --project <ID> --title <TEXT> --status <SLUG> --json"));
    let help = user.run("invalid", &["list", "--help"]);
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("--project <ID>"));
    assert!(help.contains("project list --workspace <ID> --json"));
    let help = user.run("invalid", &["search", "--help"]);
    let help = String::from_utf8_lossy(&help.stdout);
    for kind in ["all", "tasks", "projects", "workspaces", "comments", "activities"] {
        assert!(help.contains(kind));
    }
    for args in [vec![], vec!["--help"], vec!["status"]] {
        let output = Command::new(BIN)
            .args(&args)
            .env("HOME", &user.home)
            .env("KANEO_API_URL", "invalid")
            .env("KANEO_ACCESS_TOKEN_FILE", "missing")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // The unified host handles --help before command dispatch; exercise the
    // retired command itself so this still proves it cannot initialize auth.
    let retired = Command::new(BIN)
        .args(["app", "kaneo", "whoami", "--json"])
        .output()
        .unwrap();
    assert_eq!(retired.status.code(), Some(2));
    #[cfg(unix)]
    {
        let alias = user.home.join("kaneo");
        std::os::unix::fs::symlink(BIN, &alias).unwrap();
        // An old symlink must not trigger the removed argv[0] dispatch.
        let retired = Command::new(&alias)
            .args(["workspace", "list", "--json"])
            .env("HOME", &user.home)
            .output()
            .unwrap();
        // Unknown modules use the host's external-command error contract.
        // The former alias must behave exactly like the normal host, never
        // select Task authentication based on argv[0].
        let normal = Command::new(BIN)
            .args(["workspace", "list", "--json"])
            .env("HOME", &user.home)
            .output()
            .unwrap();
        assert!(!normal.status.success());
        assert_eq!(retired.status.code(), normal.status.code());
        assert_eq!(retired.stdout, normal.stdout);
        assert_eq!(retired.stderr, normal.stderr);
    }
}

#[test]
fn task_entry_exposes_all_commands_and_offline_help_without_a_second_task_level() {
    let user = User::new();
    let commands = [
        "list",
        "get",
        "create",
        "update",
        "move",
        "delete",
        "restore",
        "trash",
        "activity",
        "fields",
        "field set",
        "project list",
        "project get",
        "project create",
        "project archive",
        "project unarchive",
        "project columns",
        "project fields",
        "workspace list",
        "workspace members",
        "comment list",
        "comment add",
        "comment update",
        "label list",
        "label create",
        "label attach",
        "label detach",
        "search",
        "whoami",
        "auth login",
        "auth status",
        "auth logout",
    ];
    for command in commands {
        let mut args: Vec<_> = command.split_whitespace().collect();
        args.push("--help");
        let output = user
            .command("invalid", &args)
            .env("KANEO_API_KEY", "ambiguous")
            .output()
            .unwrap();
        assert!(output.status.success(), "{command}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("Usage: apeiron task {command}")));
    }
    for args in [vec!["--help"], vec!["--version"], vec!["project", "list", "--version"]] {
        let output = user.command("invalid", &args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    }
    let output = user
        .command("invalid", &["task", "get", "id", "--json"])
        .output()
        .unwrap();
    assert_eq!(value(&output, 2)["error"]["code"], "INVALID_ARGUMENT");

    let server = Server::new(|_| Response::json(json!([])));
    for args in [
        vec![
            "--api-url",
            &server.url,
            "--workspace",
            "w",
            "--timeout-ms",
            "5000",
            "--json",
            "project",
            "list",
        ],
        vec![
            "project",
            "list",
            "--api-url",
            &server.url,
            "--workspace",
            "w",
            "--timeout-ms",
            "5000",
            "--json",
        ],
    ] {
        let output = user.command("invalid", &args).output().unwrap();
        assert_eq!(value(&output, 0)["data"]["items"], json!([]));
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[0].path, "/api/project?workspaceId=w");
    assert_eq!(requests[0].headers["authorization"], "Bearer fixture-token");
}

#[test]
fn argument_guidance_identifies_missing_fields_without_echoing_input() {
    let server = Server::new(|_| panic!("invalid arguments must not contact the API"));
    let user = User::new();
    for (args, hints) in [
        (
            vec!["list"],
            vec!["--project <ID>", "project list --workspace <ID> --json"],
        ),
        (
            vec!["create", "--title", "private-input"],
            vec!["--project <ID>", "project list --workspace <ID> --json"],
        ),
        (
            vec!["project", "list"],
            vec!["--workspace", "workspace list --json", "not its name or slug"],
        ),
        (vec!["comment", "list"], vec!["--task <ID>"]),
        (
            vec!["create", "--project", "p", "--title", "private-input"],
            vec!["--status", "project columns <ID> --json"],
        ),
        (
            vec!["create", "--project", "p", "--status", "private-input"],
            vec!["--title"],
        ),
        (
            vec!["search", "private-input", "--workspace", "w", "--type", "task"],
            vec!["--type", "all, tasks, projects, workspaces, comments, activities"],
        ),
        (vec!["search", "--workspace", "private-input"], vec!["<query>"]),
        (vec!["get", "id", "--private-input"], vec!["--help"]),
    ] {
        for json in [false, true] {
            let mut args = args.clone();
            if json {
                args.push("--json");
            }
            let output = user.run(&server.url, &args);
            assert_eq!(output.status.code(), Some(2));
            let message = if json {
                let result = value(&output, 2);
                assert_eq!(result["schemaVersion"], 1);
                assert_eq!(result["error"]["code"], "INVALID_ARGUMENT");
                assert_eq!(result["error"]["outcome"], "not_sent");
                assert!(result["error"]["httpStatus"].is_null());
                result["error"]["message"].as_str().unwrap().to_owned()
            } else {
                String::from_utf8(output.stdout).unwrap()
            };
            for hint in &hints {
                assert!(message.contains(hint), "{args:?}: {message}");
            }
            assert!(!message.contains("private-input"));
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn workspace_denials_explain_ids_without_changing_auth_or_retrying() {
    let server = Server::new(|request| {
        let (status, body) = match request.path.as_str() {
            "/api/project?workspaceId=w1" => return Response::json(json!([{"id":"p1","workspaceId":"w1"}])),
            "/api/project?workspaceId=platform-w1" => (403, "PLATFORM_WORKSPACE_FORBIDDEN".into()),
            "/api/project?workspaceId=workspace-name" => {
                (403, json!({"code":"PLATFORM_WORKSPACE_FORBIDDEN"}).to_string())
            }
            "/api/project?workspaceId=json-message" => {
                (403, json!({"message":"PLATFORM_WORKSPACE_FORBIDDEN"}).to_string())
            }
            "/api/project?workspaceId=unknown" => (403, json!({"message":"private-input"}).to_string()),
            "/api/project?workspaceId=message-only" => (403, json!({"message":"REAUTH_REQUIRED"}).to_string()),
            "/api/project?workspaceId=expired" => (401, "UNAUTHORIZED private-input".into()),
            "/api/project?workspaceId=html" => (403, "<html>private-input PLATFORM_WORKSPACE_FORBIDDEN</html>".into()),
            "/api/project" if request.method == "POST" => (403, "PLATFORM_WORKSPACE_FORBIDDEN".into()),
            _ => panic!("unexpected request {} {}", request.method, request.path),
        };
        Response {
            status,
            body,
            delay: Duration::ZERO,
            partial: false,
        }
    });
    let user = User::new();
    for (workspace, exit, guided) in [
        ("platform-w1", 4, true),
        ("workspace-name", 4, true),
        ("json-message", 4, true),
        ("unknown", 4, false),
        ("message-only", 4, false),
        ("html", 4, false),
        ("expired", 3, false),
    ] {
        let before = server.requests().len();
        let result = value(
            &user.run(&server.url, &["project", "list", "--workspace", workspace, "--json"]),
            exit,
        );
        assert_eq!(result["schemaVersion"], 1);
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"].as_object().unwrap().len(), 4);
        assert_eq!(
            result["error"]["code"],
            if exit == 4 { "FORBIDDEN" } else { "UNAUTHORIZED" }
        );
        assert_eq!(result["error"]["httpStatus"], if exit == 4 { 403 } else { 401 });
        assert_eq!(result["error"]["outcome"], "not_applicable");
        let message = result["error"]["message"].as_str().unwrap();
        assert_eq!(message.contains("PLATFORM_WORKSPACE_FORBIDDEN"), guided);
        assert_eq!(message.contains("workspace list --json"), guided);
        assert_eq!(message.contains("not its name or slug"), guided);
        assert!(!message.contains("private-input"));
        assert_eq!(server.requests().len(), before + 1);
    }
    let result = value(
        &user.run(&server.url, &["project", "list", "--workspace", "w1", "--json"]),
        0,
    );
    assert_eq!(result["data"]["items"][0]["id"], "p1");
    let before = server.requests().len();
    let result = value(
        &user.run(
            &server.url,
            &[
                "project",
                "create",
                "--workspace",
                "platform-w1",
                "--name",
                "name",
                "--slug",
                "slug",
                "--icon",
                "Box",
                "--json",
            ],
        ),
        4,
    );
    assert_eq!(result["error"]["code"], "FORBIDDEN");
    assert_eq!(result["error"]["outcome"], "rejected");
    assert_eq!(server.requests().len(), before + 1);
    for request in server.requests() {
        assert_eq!(request.headers["authorization"], "Bearer fixture-token");
    }
}

#[test]
fn identity_projection_external_sources_and_validation_do_not_change() {
    let server = Server::new(|_| Response::json(identity()));
    let user = User::new();
    let data = value(&user.run(&server.url, &["auth", "status", "--json"]), 0);
    assert_eq!(data["data"]["credentialSource"], "KANEO_ACCESS_TOKEN");
    assert!(data["data"]["user"].get("secret").is_none());
    assert_eq!(
        server.requests()[0].headers.get("authorization").unwrap(),
        "Bearer fixture-token"
    );
    assert!(!server.requests()[0].headers.contains_key("cookie"));
    let before = server.requests().len();
    let output = user
        .command(&server.url, &["whoami", "--json"])
        .env("KANEO_API_KEY", "other")
        .output()
        .unwrap();
    assert_eq!(value(&output, 2)["error"]["code"], "INVALID_CONFIG");
    assert_eq!(server.requests().len(), before);
    let output = user.run(
        &server.url,
        &["update", "id", "--title", "a", "--status", "b", "--json"],
    );
    assert_eq!(value(&output, 2)["error"]["outcome"], "not_sent");
    assert_eq!(server.requests().len(), before);
    let output = user.run(&server.url, &["get", "id", "--wat", "--json"]);
    assert_eq!(value(&output, 2)["error"]["code"], "INVALID_ARGUMENT");
    let file = user.home.join("access");
    fs::write(&file, "first").unwrap();
    for token in ["first", "second"] {
        fs::write(&file, token).unwrap();
        let output = user
            .command(&server.url, &["whoami", "--json"])
            .env_remove("KANEO_ACCESS_TOKEN")
            .env("KANEO_ACCESS_TOKEN_FILE", &file)
            .output()
            .unwrap();
        value(&output, 0);
        assert_eq!(
            server.requests().last().unwrap().headers["authorization"],
            format!("Bearer {token}")
        );
    }
}

#[test]
fn resource_commands_preserve_wire_requests_and_projections() {
    let server = Server::new(|request| {
        let base =
            json!({"id":"id","title":"Task","name":"Name","description":"Text","secret":"hidden","taskId":"task"});
        if request.path == "/api/me" {
            return Response::json(identity());
        }
        if request.path == "/api/task/move/id" {
            return Response::json(json!({"task":base,"sourceProjectId":"p","destinationProjectId":"p2"}));
        }
        if request.path.contains("/members")
            || request.path.starts_with("/api/column/")
            || request.path.starts_with("/api/activity/")
            || request.path.starts_with("/api/custom-field/project/")
            || request.path.starts_with("/api/custom-field/task/")
            || request.path.starts_with("/api/label/workspace/")
            || request.path.starts_with("/api/label/task/")
            || (request.method == "GET"
                && (request.path.starts_with("/api/project?") || request.path == "/api/comment/task"))
        {
            return Response::json(json!([base]));
        }
        Response::json(base)
    });
    let user = User::new();
    let cases: Vec<(Vec<&str>, &str, &str, Option<Value>)> = vec![
        (vec!["whoami"], "GET", "/api/me", None),
        (vec!["auth", "status"], "GET", "/api/me", None),
        (vec!["workspace", "list"], "GET", "/api/me", None),
        (
            vec!["workspace", "members", "--workspace", "w"],
            "GET",
            "/api/workspace/w/members",
            None,
        ),
        (
            vec!["project", "list", "--workspace", "w", "--include-archived"],
            "GET",
            "/api/project?workspaceId=w&includeArchived=true",
            None,
        ),
        (vec!["project", "get", "id"], "GET", "/api/project/id", None),
        (
            vec![
                "project",
                "create",
                "--workspace",
                "w",
                "--name",
                "项目",
                "--slug",
                "p",
                "--icon",
                "Box",
            ],
            "POST",
            "/api/project",
            Some(json!({"workspaceId":"w","name":"项目","slug":"p","icon":"Box"})),
        ),
        (vec!["project", "archive", "id"], "PUT", "/api/project/id/archive", None),
        (
            vec!["project", "unarchive", "id"],
            "PUT",
            "/api/project/id/unarchive",
            None,
        ),
        (vec!["project", "columns", "id"], "GET", "/api/column/id", None),
        (
            vec!["project", "fields", "id"],
            "GET",
            "/api/custom-field/project/id",
            None,
        ),
        (vec!["get", "id"], "GET", "/api/task/id", None),
        (
            vec!["create", "--project", "p", "--title", "Task", "--status", "todo"],
            "POST",
            "/api/task/p",
            Some(json!({"title":"Task","status":"todo","priority":"no-priority","description":"","customFields":[]})),
        ),
        (
            vec!["update", "id", "--unassign"],
            "PUT",
            "/api/task/assignee/id",
            Some(json!({"userId":null})),
        ),
        (
            vec!["update", "id", "--description", "- checklist item\n- 第二项"],
            "PUT",
            "/api/task/description/id",
            Some(json!({"description":"- checklist item\n- 第二项"})),
        ),
        (
            vec!["update", "id", "--clear-due"],
            "PUT",
            "/api/task/due-date/id",
            Some(json!({})),
        ),
        (
            vec!["move", "id", "--project", "p2"],
            "PUT",
            "/api/task/move/id",
            Some(json!({"destinationProjectId":"p2"})),
        ),
        (vec!["delete", "id"], "DELETE", "/api/task/id", None),
        (vec!["restore", "id"], "POST", "/api/task/id/restore", None),
        (vec!["activity", "id"], "GET", "/api/activity/id", None),
        (vec!["fields", "id"], "GET", "/api/custom-field/task/id", None),
        (
            vec!["field", "set", "id", "--field", "f", "--value", ""],
            "PUT",
            "/api/custom-field/value",
            Some(json!({"taskId":"id","fieldId":"f","value":""})),
        ),
        (
            vec!["comment", "list", "--task", "task"],
            "GET",
            "/api/comment/task",
            None,
        ),
        (
            vec!["comment", "add", "--task", "task", "--body", "你好"],
            "POST",
            "/api/comment/task",
            Some(json!({"content":"你好"})),
        ),
        (
            vec!["comment", "update", "id", "--body", "changed"],
            "PUT",
            "/api/comment/id",
            Some(json!({"content":"changed"})),
        ),
        (
            vec!["comment", "add", "--task", "task", "--body", "--help"],
            "POST",
            "/api/comment/task",
            Some(json!({"content":"--help"})),
        ),
        (
            vec!["label", "list", "--workspace", "w"],
            "GET",
            "/api/label/workspace/w",
            None,
        ),
        (
            vec!["label", "list", "--task", "task"],
            "GET",
            "/api/label/task/task",
            None,
        ),
        (
            vec!["label", "create", "--workspace", "w", "--name", "n", "--color", "red"],
            "POST",
            "/api/label",
            Some(json!({"workspaceId":"w","name":"n","color":"red"})),
        ),
        (
            vec!["label", "attach", "id", "--task", "task"],
            "PUT",
            "/api/label/id/task",
            Some(json!({"taskId":"task"})),
        ),
        (
            vec!["label", "detach", "id", "--task", "task"],
            "DELETE",
            "/api/label/id/task",
            None,
        ),
    ];
    for (mut args, method, path, body) in cases {
        args.push("--json");
        let result = value(&user.run(&server.url, &args), 0);
        assert!(!result.to_string().contains("hidden"), "{args:?}");
        let requests = server.requests();
        let request = requests.last().unwrap();
        assert_eq!(request.headers["authorization"], "Bearer fixture-token");
        assert_eq!(request.method, method, "{args:?}");
        assert_eq!(request.path, path, "{args:?}");
        if let Some(body) = body {
            assert_eq!(serde_json::from_str::<Value>(&request.body).unwrap(), body, "{args:?}");
        } else {
            assert!(request.body.is_empty(), "{args:?}");
        }
    }
    // A flag consumed as text must not also switch the successful output mode.
    let result = value(
        &user.run(&server.url, &["comment", "add", "--task", "task", "--body", "--json"]),
        0,
    );
    assert_eq!(result["id"], "id");
    assert!(result.get("schemaVersion").is_none());
    let request = server.requests().pop().unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/api/comment/task");
    assert_eq!(
        serde_json::from_str::<Value>(&request.body).unwrap(),
        json!({"content":"--json"})
    );
}

#[test]
fn explicit_proxies_and_no_proxy_keep_their_transport_boundaries() {
    let user = User::new();
    let direct = Server::new(|_| Response::json(json!({"id":"direct"})));
    let proxy = Server::new(|request| {
        let mut response = Response::json(json!({"id":"proxied"}));
        if request.method == "CONNECT" {
            // A rejected tunnel still proves HTTPS selected the explicit proxy,
            // without accepting an untrusted certificate or reaching the Internet.
            response.status = 502;
        }
        response
    });
    for key in ["HTTP_PROXY", "http_proxy"] {
        let output = user
            .command("http://kaneo.invalid", &["get", "id", "--json"])
            .env(key, &proxy.url)
            .output()
            .unwrap();
        assert_eq!(value(&output, 0)["data"]["id"], "proxied");
        let request = proxy.requests().pop().unwrap();
        assert_eq!(request.path, "http://kaneo.invalid/api/task/id");
        assert_eq!(request.headers["authorization"], "Bearer fixture-token");
        for bypass in ["NO_PROXY", "no_proxy"] {
            let before = proxy.requests().len();
            let output = user
                .command(&direct.url, &["get", "id", "--json"])
                .env(key, &proxy.url)
                .env(bypass, "127.0.0.1")
                .output()
                .unwrap();
            assert_eq!(value(&output, 0)["data"]["id"], "direct");
            assert_eq!(proxy.requests().len(), before);
        }
    }
    for key in ["HTTPS_PROXY", "https_proxy"] {
        let before = proxy.requests().len();
        let output = user
            .command("https://kaneo.invalid", &["get", "id", "--json"])
            .env(key, &proxy.url)
            .output()
            .unwrap();
        assert_eq!(value(&output, 8)["error"]["code"], "NETWORK_ERROR");
        assert_eq!(proxy.requests().len(), before + 1);
        let request = proxy.requests().pop().unwrap();
        assert_eq!(request.method, "CONNECT");
        assert_eq!(request.path, "kaneo.invalid:443");
        assert!(!request.headers.contains_key("authorization"));
    }
}

#[cfg(unix)]
#[test]
fn device_login_polls_while_the_browser_launcher_is_still_running() {
    use std::os::unix::fs::PermissionsExt;
    let user = User::new();
    let started = user.home.join("browser-started");
    let release = user.home.join("browser-release");
    let finished = user.home.join("browser-finished");
    let opener = user
        .home
        .join(if cfg!(target_os = "macos") { "open" } else { "xdg-open" });
    fs::write(
        &opener,
        "#!/bin/sh\n: > \"$XDG_CONFIG_HOME/browser-started\"\nwhile [ ! -f \"$XDG_CONFIG_HOME/browser-release\" ]; do /bin/sleep 0.1; done\n: > \"$XDG_CONFIG_HOME/browser-finished\"\n",
    )
    .unwrap();
    fs::set_permissions(&opener, fs::Permissions::from_mode(0o700)).unwrap();
    let issuer = Arc::new(Mutex::new(String::new()));
    let shared = issuer.clone();
    let server = Server::new(move |request| {
        let issuer = shared.lock().unwrap();
        match request.path.as_str() {
            "/api/config" => Response::json(
                json!({"cliAuth":{"type":"oidc","issuer":*issuer,"clientId":"broker","audience":"kaneo"}}),
            ),
            "/.well-known/openid-configuration" => Response::json(
                json!({"issuer":*issuer,"token_endpoint":format!("{issuer}/token"),"device_authorization_endpoint":format!("{issuer}/device")}),
            ),
            "/device" => Response::json(
                json!({"device_code":"fixture-device","user_code":"ABCD","verification_uri":format!("{issuer}/verify"),"expires_in":60,"interval":1}),
            ),
            "/token" => Response::json(
                json!({"access_token":"fixture-access","refresh_token":"fixture-refresh","token_type":"Bearer","expires_in":300}),
            ),
            "/api/me" => Response::json(identity()),
            _ => panic!("unexpected request: {}", request.path),
        }
    });
    *issuer.lock().unwrap() = server.url.clone();
    let mut paths = vec![user.home.clone()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let mut child = user
        .command(&server.url, &["auth", "login", "--json"])
        .env_remove("KANEO_ACCESS_TOKEN")
        .env("PATH", std::env::join_paths(paths).unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let completed = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let opened = started.exists();
    let browser_still_running = !finished.exists();
    // Release the fixture even when the regression is present, so a failed
    // assertion never leaves a browser launcher behind.
    fs::write(&release, "").unwrap();
    if !completed {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while opened && !finished.exists() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(opened && browser_still_running);
    assert!(finished.exists());
    assert!(completed, "login waited for the browser launcher instead of polling");
    assert_eq!(value(&output, 0)["data"]["loggedIn"], true);
    assert!(server.requests().iter().any(|r| r.path == "/token"));
    let output = user
        .command(&server.url, &["auth", "status", "--json"])
        .env_remove("KANEO_ACCESS_TOKEN")
        .output()
        .unwrap();
    assert_eq!(value(&output, 0)["data"]["credentialSource"], "local-login");
}

#[test]
fn pagination_stdin_and_write_uncertainty_are_process_contracts() {
    let server = Server::new(|request| {
        if request.path.starts_with("/api/task/tasks/") {
            return Response::json(
                json!({"data":{"columns":[{"tasks":[{"id":"b","number":2},{"id":"a","number":1}]}],"archivedTasks":[{"id":"b","number":2,"title":"last"}],"plannedTasks":[{"id":"c","number":null}]},"pagination":{"page":2,"pageSize":3,"total":7,"totalPages":3}}),
            );
        }
        if request.path.starts_with("/api/search?") {
            return Response::json(json!({"results":[],"totalCount":4}));
        }
        if request.path.starts_with("/api/task/trash/") {
            return Response::json(json!({"tasks":[],"pagination":{"page":2,"limit":3,"total":7}}));
        }
        let body: Value = serde_json::from_str(&request.body).unwrap_or(Value::Null);
        if body["title"] == "failure" {
            let mut r = Response::json(json!({}));
            r.status = 500;
            return r;
        }
        if body["title"] == "timeout" {
            let mut r = Response::json(json!({}));
            r.partial = true;
            r.delay = Duration::from_millis(300);
            return r;
        }
        Response::json(json!({"id":"id","description":body["description"]}))
    });
    let user = User::new();
    let data = value(&user.run(&server.url, &["list", "--project", "p", "--json"]), 0);
    assert_eq!(data["meta"], json!({"page":2,"limit":3,"total":7,"hasMore":true}));
    assert_eq!(data["data"]["items"][1]["title"], "last");
    assert_eq!(
        value(
            &user.run(&server.url, &["search", "needle", "--workspace", "w", "--json"]),
            0
        )["meta"],
        json!({"limit":20,"totalCount":4})
    );
    assert_eq!(
        value(&user.run(&server.url, &["trash", "--workspace", "w", "--json"]), 0)["meta"]["hasMore"],
        true
    );
    let mut child = user
        .command(&server.url, &["update", "id", "--description-file", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let content = "第一行\n🦀".repeat(1000);
    let mut stdin = child.stdin.take().unwrap();
    for bytes in content.as_bytes().chunks(2) {
        stdin.write_all(bytes).unwrap();
    }
    drop(stdin);
    assert_eq!(
        value(&child.wait_with_output().unwrap(), 0)["data"]["description"],
        content
    );
    for title in ["failure", "timeout"] {
        let before = server.requests().len();
        let output = user.run(
            &server.url,
            &["update", "id", "--title", title, "--timeout-ms", "100", "--json"],
        );
        assert_eq!(value(&output, 9)["error"]["outcome"], "unknown");
        assert_eq!(server.requests().len(), before + 1);
    }
}

#[cfg(unix)]
fn saved_login(user: &User, url: &str) -> PathBuf {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let api = format!("{url}/api");
    let hash = |value: &Value| format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()));
    let directory = user.home.join("kaneo-cli").join(hash(&json!(api)));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)
        .unwrap();
    let profile = directory.join(format!("{}.json", hash(&json!([api, url, "broker"]))));
    let value = json!({"version":1,"apiUrl":api,"issuer":url,"clientId":"broker","accessToken":"old","refreshToken":"old-refresh","expiresAt":1,"userId":"user-1","workspaces":[]});
    for (file, data) in [
        (profile.clone(), value),
        (
            directory.join("current.json"),
            json!({"issuer":url,"clientId":"broker"}),
        ),
    ] {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(file)
            .unwrap()
            .write_all(data.to_string().as_bytes())
            .unwrap();
    }
    profile
}

#[cfg(unix)]
#[test]
fn concurrent_refresh_recovers_pending_grant_and_logout_revokes_latest() {
    #[derive(Default)]
    struct State {
        url: String,
        rotations: usize,
        available: bool,
        revoked: Vec<String>,
    }
    let state = Arc::new(Mutex::new(State::default()));
    let state2 = state.clone();
    let server = Server::new(move |request| {
        let mut state = state2.lock().unwrap();
        if request.path == "/.well-known/openid-configuration" {
            return Response::json(
                json!({"issuer":state.url,"token_endpoint":format!("{}/token",state.url),"revocation_endpoint":format!("{}/revoke",state.url)}),
            );
        }
        if request.path == "/token" {
            let body: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            let expected = if state.rotations == 0 {
                "old-refresh".into()
            } else {
                format!("refresh-{}", state.rotations)
            };
            if body["refresh_token"] != expected {
                let mut r = Response::json(json!({"error":"invalid_grant"}));
                r.status = 400;
                return r;
            }
            state.rotations += 1;
            return Response::json(
                json!({"access_token":format!("access-{}",state.rotations),"refresh_token":format!("refresh-{}",state.rotations),"token_type":"Bearer","expires_in":300}),
            );
        }
        if request.path == "/revoke" {
            let body: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            state.revoked.push(body["token"].clone());
            let mut response = Response::json(Value::Null);
            response.body = String::new();
            return response;
        }
        if !state.available {
            let mut r = Response::json(json!({}));
            r.status = 503;
            return r;
        }
        assert_eq!(
            request.headers["authorization"],
            format!("Bearer access-{}", state.rotations)
        );
        Response::json(identity())
    });
    state.lock().unwrap().url = server.url.clone();
    let user = User::new();
    let profile = saved_login(&user, &server.url);
    let run = || {
        user.command(&server.url, &["whoami", "--json"])
            .env_remove("KANEO_ACCESS_TOKEN")
            .output()
            .unwrap()
    };
    assert_eq!(value(&run(), 8)["error"]["code"], "SERVICE_UNAVAILABLE");
    let cached: Value = serde_json::from_slice(&fs::read(&profile).unwrap()).unwrap();
    assert_eq!(cached["accessToken"], "old");
    assert_eq!(cached["pendingRefresh"]["refreshToken"], "refresh-1");
    state.lock().unwrap().available = true;
    thread::scope(|scope| {
        let a = scope.spawn(run);
        let b = scope.spawn(run);
        value(&a.join().unwrap(), 0);
        value(&b.join().unwrap(), 0);
    });
    assert_eq!(state.lock().unwrap().rotations, 1);
    let cached: Value = serde_json::from_slice(&fs::read(&profile).unwrap()).unwrap();
    assert_eq!(cached["refreshToken"], "refresh-1");
    assert!(cached.get("pendingRefresh").is_none());
    let output = user
        .command(&server.url, &["auth", "logout", "--json"])
        .env("KANEO_ACCESS_TOKEN", "external-still-set")
        .output()
        .unwrap();
    assert_eq!(value(&output, 0)["data"]["refreshGrantRevoked"], true);
    assert_eq!(state.lock().unwrap().revoked, vec!["refresh-1"]);
    assert!(!profile.exists());
}

#[test]
fn iso_dates_keep_zod_precision_and_calendar_rules() {
    let server = Server::new(|_| Response::json(json!({"id":"task"})));
    let user = User::new();
    for (date, code) in [
        ("2026-10-12T12:30Z", 0),
        ("2026-10-12T12:30+08:00", 0),
        ("2024-02-29T12:30:00.1234567890Z", 0),
        ("0000-01-01T00:00Z", 0),
        ("2026-10-12T12:30:00+23:59", 0),
        ("2026-10-12T12:30:60Z", 2),
        ("2026-10-12t12:30:00z", 2),
        ("2026-02-30T00:00:00Z", 2),
        ("2026-10-12T12:30", 2),
        ("2026-10-12T24:00Z", 2),
        ("2026-10-12T12:30.1Z", 2),
    ] {
        let output = user.run(&server.url, &["update", "task", "--due", date, "--json"]);
        value(&output, code);
    }
}

#[cfg(unix)]
#[test]
fn saved_login_errors_and_signal_cleanup_preserve_process_contract() {
    use std::os::unix::process::ExitStatusExt;
    let server = Server::new(|_| {
        let mut response = Response::json(json!({}));
        response.delay = Duration::from_millis(500);
        response
    });
    let user = User::new();
    let profile = saved_login(&user, &server.url);
    let directory = profile.parent().unwrap();
    let pointer = directory.join("current.json");
    let original = fs::read(&pointer).unwrap();
    let run = || {
        user.command(&server.url, &["whoami", "--json"])
            .env_remove("KANEO_ACCESS_TOKEN")
            .output()
            .unwrap()
    };
    fs::write(&pointer, b"{}").unwrap();
    assert_eq!(value(&run(), 2)["error"]["message"], "Invalid saved login binding");
    fs::write(&pointer, b"{").unwrap();
    assert_eq!(
        value(&run(), 2)["error"]["message"],
        "CLI credential storage is unreadable or invalid"
    );
    fs::write(&pointer, original).unwrap();
    let original = fs::read(&profile).unwrap();
    fs::write(&profile, b"{}").unwrap();
    assert_eq!(
        value(&run(), 2)["error"]["message"],
        "Saved login does not match the configured API and identity provider"
    );
    fs::write(&profile, original).unwrap();
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let before = server.requests().len();
        let mut child = user
            .command(&server.url, &["whoami", "--json"])
            .env_remove("KANEO_ACCESS_TOKEN")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        for _ in 0..500 {
            if server.requests().len() > before {
                break;
            }
            assert!(child.try_wait().unwrap().is_none());
            thread::sleep(Duration::from_millis(10));
        }
        assert!(server.requests().len() > before);
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.signal(), Some(signal));
        assert!(output.stdout.is_empty());
        assert!(!directory.with_extension("lock").exists());
    }
    // A pipe that stays open must not keep Ctrl-C waiting for stdin EOF.
    let mut child = user
        .command(&server.url, &["update", "task", "--description-file", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let mut exited = false;
    for _ in 0..100 {
        if child.try_wait().unwrap().is_some() {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !exited {
        child.kill().unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(exited, "signal cancellation waited for stdin EOF");
    assert_eq!(output.status.signal(), Some(libc::SIGINT));
    assert!(output.stdout.is_empty());
}
