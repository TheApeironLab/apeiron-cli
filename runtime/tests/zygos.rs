use serde_json::Value;
use std::process::Command;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_apeiron"))
        .env("APEIRON_ZYGOS_BIN", "/does/not/exist")
        .env("PATH", "/does/not/exist")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn zygos_is_embedded_and_help_uses_apeiron_prefix() {
    let out = cli(&["zygos", "--help", "--compact"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let commands = value["commands"].as_array().unwrap();
    assert!(commands.iter().any(|c| c["name"] == "model"));
    assert!(commands
        .iter()
        .all(|c| c["help"].as_str().unwrap().starts_with("apeiron zygos ")));
}

#[test]
fn zygos_errors_preserve_json_and_exit_status() {
    let out = cli(&["zygos", "not-a-command"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "bad_request");
    assert_eq!(error["_cli"]["help"][0], "apeiron");
    assert_eq!(error["_cli"]["help"][1], "zygos");
}

#[test]
fn top_level_help_discovers_zygos() {
    let out = cli(&["--help"]);
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout).unwrap().contains("apeiron zygos --help"));
}

#[test]
fn embedded_zygos_calls_rest_and_preserves_field_selection() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let worker = std::thread::spawn(move || {
        let request = server
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(request.method().as_str(), "GET");
        assert_eq!(request.url(), "/v2/models");
        assert!(request
            .headers()
            .iter()
            .any(|h| h.field.equiv("Authorization") && h.value.as_str() == "Bearer isolated-test-token"));
        request
            .respond(tiny_http::Response::from_string(
                r#"{"items":[{"id":"m1","name":"sample","description":"not selected"}]}"#,
            ))
            .unwrap();
    });
    let out = Command::new(env!("CARGO_BIN_EXE_apeiron"))
        .env("APEIRON_ZYGOS_BIN", "/does/not/exist")
        .env("PATH", "/does/not/exist")
        .env("ZYGOS_ACCESS_TOKEN", "isolated-test-token")
        .env("NO_PROXY", "127.0.0.1")
        .args([
            "zygos",
            "model",
            "list",
            "--base-url",
            &base,
            "--fields",
            "name",
            "--compact",
        ])
        .output()
        .unwrap();
    worker.join().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["items"][0], serde_json::json!({"name":"sample"}));
    assert_eq!(value["_cli"]["full"], "--view full");
}
