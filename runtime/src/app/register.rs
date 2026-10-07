use super::process;
use super::*;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
fn gateway(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(|_| fail("Invalid gateway URL", 2))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || (url.scheme() != "https"
            && !(url.scheme() == "http" && [Some("localhost"), Some("127.0.0.1")].contains(&url.host_str())))
    {
        return Err(fail(
            "Gateway must be an HTTPS origin (loopback HTTP is allowed for local development).",
            2,
        ));
    }
    Ok(url)
}
fn valid_request(v: &Value) -> bool {
    v["type"] == "request"
        && config::matches(r"^[0-9a-f-]{36}$", v["id"].as_str().unwrap_or(""))
        && [Some("GET"), Some("POST")].contains(&v["method"].as_str())
        && v["path"]
            .as_str()
            .is_some_and(|p| config::matches(r"^(?:|favicon\.ico|api/[a-z0-9./-]+)$", p) && !p.contains(".."))
        && v["body"].as_str().is_some_and(|s| s.len() <= 16384)
}
async fn response(message: Value, local: String) -> Value {
    let result = async {
        let base = reqwest::Url::parse(&local).unwrap();
        let url = base
            .join(message["path"].as_str().unwrap())
            .map_err(|_| fail("Invalid setup path", 2))?;
        let c = reqwest::Client::builder()
            .timeout(Duration::from_secs(290))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| fail("Setup request failed", 9))?;
        let mut request = if message["method"] == "POST" {
            c.post(url).body(message["body"].as_str().unwrap().to_owned())
        } else {
            c.get(url)
        };
        request = request
            .header("Origin", base.origin().ascii_serialization())
            .header("Content-Type", "application/json")
            .header("Sec-Fetch-Site", "same-origin");
        let mut r = request.send().await.map_err(|_| fail("Setup request failed", 9))?;
        let status = r.status().as_u16();
        let mut headers = serde_json::Map::new();
        for key in [
            "content-type",
            "content-disposition",
            "content-security-policy",
            "x-content-type-options",
            "referrer-policy",
        ] {
            if let Some(v) = r.headers().get(key).and_then(|h| h.to_str().ok()) {
                headers.insert(key.into(), json!(v));
            }
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = r.chunk().await.map_err(|_| fail("Setup response failed", 9))? {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err(fail("Setup response too large", 9));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, headers, bytes))
    }
    .await;
    let (status, headers, bytes) = result.unwrap_or_else(|_: Error| {
        (
            502,
            serde_json::Map::from_iter([("content-type".into(), json!("text/plain; charset=utf-8"))]),
            "部署向导请求失败、超时或响应超过 8 MiB。".as_bytes().to_vec(),
        )
    });
    json!({"type":"response","id":message["id"],"status":status,"headers":headers,"body":base64::engine::general_purpose::STANDARD.encode(bytes)})
}
pub fn run(args: &[String]) -> Result<()> {
    let (pos, options) = parse(args, &["gateway", "enrollment-token", "connection"], &["help"])?;
    if options.contains_key("help") {
        println!("apeiron register --gateway <https://gateway> --enrollment-token <one-time-token>\napeiron register --connection <saved-file>\nKeep this process running while using the remote deployment wizard. Ctrl+C disconnects and stops the wizard.");
        return Ok(());
    }
    if !pos.is_empty() {
        return Err(fail("Invalid register arguments", 2));
    }
    let (state, path) = if let Some(path) = options.get("connection") {
        if options.len() != 1 {
            return Err(fail("--connection cannot be combined with other flags", 2));
        }
        let path = std::path::PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|_| fail("Cannot read current directory", 9))?
                .join(path)
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let stat = std::fs::symlink_metadata(&path).map_err(|_| fail("Cannot read saved connection", 7))?;
            if !stat.is_file() || stat.mode() & 0o077 != 0 || stat.uid() != unsafe { libc::getuid() } {
                return Err(fail("Saved connection must be an owned private regular file", 7));
            }
        }
        (resources::json_file(&path, 16384)?, path)
    } else {
        if options.len() != 2 {
            return Err(fail("--gateway and --enrollment-token are required", 2));
        }
        let url = gateway(required(&options, "gateway")?)?;
        let token = required(&options, "enrollment-token")?;
        if !config::matches(r"^[A-Za-z0-9_-]{43}$", token) {
            return Err(fail("Invalid enrollment token", 2));
        }
        let r = client(15)?
            .post(url.join("v1/agent/enroll").unwrap())
            .json(&json!({"token":token}))
            .send()
            .map_err(|_| fail("Cannot enroll; generate a new command in the gateway and retry", 9))?;
        if !r.status().is_success() {
            return Err(fail(
                "Cannot enroll; generate a new command in the gateway and retry",
                7,
            ));
        }
        let mut bytes = Vec::new();
        r.take(16385)
            .read_to_end(&mut bytes)
            .map_err(|_| fail("Invalid enrollment response", 9))?;
        if bytes.len() > 16384 {
            return Err(fail("Invalid enrollment response", 9));
        }
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| fail("Invalid enrollment response", 9))?;
        validate(&value)?;
        value["gateway"] = json!(url.origin().ascii_serialization());
        let path = process::home()?
            .join(".apeiron/gateway")
            .join(format!("{}.json", value["deployment_id"].as_str().unwrap()));
        process::private_directory(path.parent().unwrap())?;
        private_write(&path, serde_json::to_string_pretty(&value).unwrap().as_bytes())?;
        (value, path)
    };
    validate(&state)?;
    let mut endpoint = gateway(state["gateway"].as_str().unwrap_or(""))?
        .join("v1/agent/tunnel")
        .unwrap();
    let scheme = if endpoint.scheme() == "https" { "wss" } else { "ws" };
    endpoint.set_scheme(scheme).unwrap();
    let setup = wizard::start_gateway(
        path.parent()
            .unwrap()
            .join(format!("{}.setup.json", state["deployment_id"].as_str().unwrap())),
        state["slug"].as_str().unwrap().into(),
    )?;
    println!("gateway: {}\nslug: {}\nconnection: {}\nKeep this command running. Open the deployment wizard from your gateway page.",state["gateway"].as_str().unwrap(),state["slug"].as_str().unwrap(),path.display());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| fail("Cannot initialize gateway runtime", 9))?;
    runtime.block_on(tunnel(
        endpoint.as_str(),
        state["agent_token"].as_str().unwrap(),
        &setup,
    ));
    drop(setup);
    Ok(())
}
fn validate(v: &Value) -> Result<()> {
    if !config::matches(r"^[0-9a-f-]{36}$", v["deployment_id"].as_str().unwrap_or(""))
        || !config::matches(r"^[A-Za-z0-9_-]{43}$", v["agent_token"].as_str().unwrap_or(""))
        || !config::matches(r"^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$", v["slug"].as_str().unwrap_or(""))
    {
        return Err(fail("Invalid saved connection or enrollment response", 2));
    }
    Ok(())
}
async fn tunnel(endpoint: &str, token: &str, setup: &wizard::GatewayWizard) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    while !setup.stopping() {
        let mut request = endpoint.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse().unwrap());
        let result = tokio::time::timeout(
            Duration::from_secs(15),
            tokio_tungstenite::connect_async_with_config(
                request,
                Some(tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
                    max_message_size: Some(32768),
                    max_frame_size: Some(32768),
                    ..Default::default()
                }),
                false,
            ),
        )
        .await;
        if let Ok(Ok((socket, _))) = result {
            println!("gateway: connected");
            let (mut sink, mut stream) = socket.split();
            let mut tasks = tokio::task::JoinSet::new();
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                tokio::select! {
                _=tick.tick()=>{if setup.stopping(){let _=sink.send(Message::Close(None)).await;return;}},
                completed=tasks.join_next(),if !tasks.is_empty()=>{if let Some(Ok(reply))=completed{if sink.send(Message::Text(reply)).await.is_err(){break;}}},
                message=stream.next()=>{match message{Some(Ok(Message::Text(text)))if text.len()<=32768=>{if let Ok(value)=serde_json::from_str::<Value>(&text){if valid_request(&value)&&tasks.len()<16{let url=setup.url();tasks.spawn(async move { response(value,url).await.to_string() });}}},Some(Ok(Message::Ping(payload)))=>{if sink.send(Message::Pong(payload)).await.is_err(){break;}},Some(Ok(Message::Close(frame)))=>{if frame.is_some_and(|f|u16::from(f.code)==4001){eprintln!("gateway: this connection was replaced");return;}break;},None|Some(Err(_))=>break,_=>{}}}
                }
            }
            tasks.abort_all();
        }
        if !setup.stopping() {
            println!("gateway: disconnected; retrying in 3 seconds");
            for _ in 0..30 {
                if setup.stopping() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg(test)]
mod contracts {
    use super::*;
    #[test]
    fn gateway_transport_allows_only_https_or_explicit_loopback() {
        assert_eq!(
            gateway("https://gateway.apeironlab.cn")
                .unwrap()
                .origin()
                .ascii_serialization(),
            "https://gateway.apeironlab.cn"
        );
        assert_eq!(
            gateway("http://localhost:8080").unwrap().origin().ascii_serialization(),
            "http://localhost:8080"
        );
        for value in [
            "http://evil.example",
            "https://user:secret@example.com",
            "https://example.com/path",
            "https://example.com/#token",
        ] {
            assert!(gateway(value).is_err());
        }
    }
    #[test]
    fn tunnel_requests_are_bounded_setup_requests_never_arbitrary_urls() {
        let base = json!({"type":"request","id":"aefcb762-56b7-4ff1-8969-5dd5a0895647","method":"GET","path":"api/config","body":"","headers":{}});
        assert!(valid_request(&base));
        for path in [
            "http://169.254.169.254",
            "//evil.example",
            "../secret",
            "api/../secret",
            "api/config?redirect=1",
            "api/%2e%2e/secret",
        ] {
            let mut v = base.clone();
            v["path"] = json!(path);
            assert!(!valid_request(&v));
        }
        let mut v = base.clone();
        v["method"] = json!("DELETE");
        assert!(!valid_request(&v));
        let mut v = base;
        v["body"] = json!("x".repeat(16385));
        assert!(!valid_request(&v));
    }
}
