//! Contracts migrated from the former TypeScript reference implementation.
//! Pure transformations stay unit tests; process and HTTP scenarios run the CLI.
use super::*;
use std::{fs, path::PathBuf};

pub struct Temp(pub PathBuf);
impl Temp {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!("apeiron-contract-{:016x}", rand::random::<u64>()));
        process::private_directory(&root).unwrap();
        Self(root)
    }
    pub fn write(&self, path: &str, data: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, data).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn install(topology: &str) -> Value {
    let mut i = config::defaults()["installation"].clone();
    i["topology"] = json!(topology);
    i["domain"] = json!("example.internal");
    i["entryIp"] = json!(if topology == "single-k3d" {
        "127.0.0.1"
    } else {
        "192.0.2.10"
    });
    i["httpPort"] = json!(80);
    i["httpsPort"] = json!(443);
    if topology == "multi-k3s" {
        i["nodes"] = json!([
            {"host":"node-0","name":"node-0","address":"192.0.2.10","role":"server"},
            {"host":"node-1","name":"node-1","address":"192.0.2.11","role":"agent"}
        ]);
    }
    i
}
fn input(i: Value, offline: bool) -> Value {
    json!({"slug":"example","apps":["nexus","vasi","ontology","apeiron"],"deployment":{"installation":i,"offline":offline,"bundleDir":if offline{"/opt/fixture-bundle"}else{""}}})
}
fn valid(v: &Value) -> Value {
    config::validate(v, &Value::Null).unwrap()
}

#[test]
fn native_k3s_accepts_only_supported_ubuntu_architecture_pairs() {
    for os in ["Ubuntu", "ubuntu"] {
        for v in ["22.04", "22.04.5", "24.04", "24.04.4"] {
            let arches: &[&str] = if v.starts_with("22.") {
                &["amd64", "x64", "x86_64"]
            } else {
                &["arm64", "aarch64"]
            };
            for a in arches {
                assert!(discovery::supported(os, v, a));
            }
        }
    }
    for (os, v, a) in [
        ("macOS", "26.6.2", "arm64"),
        ("debian", "12", "aarch64"),
        ("ubuntu", "20.04", "x86_64"),
        ("ubuntu", "24.10", "aarch64"),
        ("ubuntu", "24.040", "aarch64"),
        ("ubuntu", "24.04", "armv7l"),
        ("ubuntu", "22.04", "arm64"),
        ("ubuntu", "24.04", "amd64"),
    ] {
        assert!(!discovery::supported(os, v, a));
    }
}
#[test]
fn native_packages_match_every_node_os_version_and_cpu_before_installation() {
    let target = json!({"hostPlatform":{"os":"ubuntu","version":"24.04","architecture":"arm64"}});
    let spark = json!({"os":"ubuntu","version":"24.04","architecture":"aarch64"});
    assert!(discovery::validate_platform(&target, std::slice::from_ref(&spark)).is_ok());
    assert!(discovery::validate_platform(
        &target,
        &[json!({"os":"ubuntu","version":"24.04.4","architecture":"arm64"})]
    )
    .is_ok());
    assert!(discovery::validate_platform(&json!({}), std::slice::from_ref(&spark)).is_err());
    let mut wrong = spark.clone();
    wrong["version"] = json!("22.04");
    assert!(discovery::validate_platform(&target, &[spark.clone(), wrong]).is_err());
    let mut wrong = spark;
    wrong["architecture"] = json!("x86_64");
    assert!(discovery::validate_platform(&target, &[wrong]).is_err());
}
#[test]
fn iframe_origins_follow_configured_domain_and_allow_https_ports() {
    let mut source = json!({"releases":{},"embedAllowedOrigins":["https://old.example.com:443"]});
    for domain in [
        "alpha.apeironlab.internal",
        "beta.apeironlab.internal",
        "platform.example.com",
    ] {
        let mut i = install("single-k3d");
        i["domain"] = json!(domain);
        i["httpsPort"] = json!(54321);
        let config = valid(&input(i, false));
        source = deploy::environment_for(&config, source).unwrap();
        assert_eq!(
            source["embedAllowedOrigins"],
            json!([format!("https://{domain}:*"), format!("https://*.{domain}:*")])
        );
        assert_eq!(deploy::environment_for(&config, source.clone()).unwrap(), source);
    }
}
#[test]
fn deployment_mode_overrides_stale_nexus_proxies_and_preserves_values_on_replay() {
    for topology in ["single-k3s", "multi-k3s", "single-k3d"] {
        let mut source = json!({"topology":topology,"releases":{"nexus":{"enabled":true,"values":{"publicProxies":false,"storage":"50Gi","node":"node-0"}},"postgres":{"enabled":true,"values":{"storage":"10Gi"}}}});
        for offline in [false, true, false] {
            let config = valid(&input(install(topology), offline));
            source = deploy::environment_for(&config, source).unwrap();
            assert_eq!(
                source["releases"]["nexus"],
                json!({"enabled":true,"values":{"publicProxies":!offline,"storage":"50Gi","node":"node-0"}})
            );
            assert_eq!(
                source["releases"]["postgres"],
                json!({"enabled":true,"values":{"storage":"10Gi"}})
            );
            assert_eq!(deploy::environment_for(&config, source.clone()).unwrap(), source);
        }
    }
}
#[test]
fn two_node_installation_and_ha_control_node_contract() {
    let mut i = install("multi-k3s");
    assert_eq!(config::installation(&i).unwrap()["nodes"].as_array().unwrap().len(), 2);
    i["ha"] = json!(true);
    assert!(config::installation(&i).is_err());
    i["nodes"] = json!((0..3).map(|n|json!({"host":format!("node-{n}"),"name":format!("node-{n}"),"address":format!("192.0.2.{}",n+10),"role":"server"})).collect::<Vec<_>>());
    assert_eq!(config::installation(&i).unwrap()["nodes"].as_array().unwrap().len(), 3);
    i["ha"] = json!(false);
    i["nodes"].as_array_mut().unwrap().pop();
    assert!(config::installation(&i).is_err());
    let mut duplicate = install("multi-k3s");
    duplicate["nodes"][1] = duplicate["nodes"][0].clone();
    assert!(config::installation(&duplicate).is_err());
    i["topology"] = json!("existing");
    assert!(config::installation(&i).is_err());
}
#[test]
fn ssh_inventory_injection_and_implicit_offline_bundle_are_rejected() {
    for host in [
        "-oProxyCommand=bad",
        "test;echo",
        "{{lookup(\"pipe\", \"id\")}}",
        "user@host",
    ] {
        let mut i = install("multi-k3s");
        i["nodes"][0]["host"] = json!(host);
        assert!(config::installation(&i).is_err());
        assert!(!config::safe_host(host));
    }
    let mut i = install("multi-k3s");
    i["sshKey"] = json!("/tmp/{{bad}}");
    assert!(config::installation(&i).is_err());
    i = install("multi-k3s");
    i["sshPort"] = json!(70000);
    assert!(config::installation(&i).is_err());
    let mut v = input(install("multi-k3s"), true);
    v["deployment"]["bundleDir"] = json!("");
    assert!(config::validate(&v, &Value::Null).is_err());
}
fn catalog() -> Value {
    let bytes = b"fixture artifact\n";
    json!({"schemaVersion":1,"targets":{"k3s-amd64":{"deploymentTopology":true,"base":["core"],"environment":{"releases":{}}}},"components":{"core":{"requires":[],"files":["core.bin"]},"task":{"requires":["core"],"files":["task.bin"]},"optional":{"requires":["core"],"files":["optional.bin"]}},"files":(["core.bin","task.bin","optional.bin"].map(|p|json!({"path":p,"size":bytes.len(),"sha256":config::hash(bytes),"url":format!("https://downloads.example.internal/{p}")})))})
}
#[test]
fn old_packages_profiles_and_mismatched_architectures_fail_before_preparation() {
    let mut c = catalog();
    c["targets"]["k3s-amd64"]["deploymentTopology"] = json!(false);
    assert!(resources::plan(&c, "k3s-amd64", &json!([])).is_err());
    c["targets"]["k3s-amd64"]["deploymentTopology"] = json!(true);
    for env in [json!({"profile":"ubuntu"}), json!({"architecture":"arm64"})] {
        c["targets"]["k3s-amd64"]["environment"] = env;
        assert!(resources::plan(&c, "k3s-amd64", &json!([])).is_err());
    }
}
#[test]
fn resource_closure_excludes_unselected_apps_and_rejects_missing_or_cyclic_dependencies() {
    let mut c = catalog();
    let p = resources::plan(&c, "k3s-amd64", &json!(["task"])).unwrap();
    assert_eq!(p["components"], json!(["core", "task"]));
    assert_eq!(
        p["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].clone())
            .collect::<Vec<_>>(),
        vec![json!("core.bin"), json!("task.bin")]
    );
    assert!(resources::plan(&c, "k3s-arm64", &json!(["task"])).is_err());
    assert!(resources::plan(&c, "k3s-amd64", &json!(["missing"]))
        .unwrap_err()
        .message
        .contains("missing"));
    c["components"]["core"]["requires"] = json!(["task"]);
    assert!(resources::plan(&c, "k3s-amd64", &json!(["task"]))
        .unwrap_err()
        .message
        .contains("循环"));
}
#[test]
fn isolated_target_catalogs_keep_identical_paths_separate() {
    let dir = Temp::new();
    let targets = ["k3d-arm64", "k3s-arm64"];
    let mut catalogs = json!({});
    for (index, t) in targets.iter().enumerate() {
        catalogs[t] = json!({"schemaVersion":1,"targets":{(*t):{"base":["app"],"environment":{},"deploymentTopology":true,"k3sAirgap":"images/app.tar"}},"components":{"app":{"requires":[],"files":["images/app.tar"]}},"files":[{"path":"images/app.tar","size":10,"sha256":index.to_string().repeat(64)}]});
    }
    dir.write(
        "setup/install.json",
        json!({"schemaVersion":2,"catalogs":catalogs}).to_string(),
    );
    for (index, t) in targets.iter().enumerate() {
        let c = resources::catalog(&dir.0, t).unwrap();
        let p = resources::plan(&c, t, &json!([])).unwrap();
        assert_eq!(p["files"][0]["sha256"], json!(index.to_string().repeat(64)));
    }
    assert!(resources::catalog(&dir.0, "k3s-amd64").is_err());
    assert!(resources::catalog(&dir.0, "").is_err());
    catalogs["k3s-arm64"]["targets"] = catalogs["k3d-arm64"]["targets"].clone();
    dir.write(
        "setup/install.json",
        json!({"schemaVersion":2,"catalogs":catalogs}).to_string(),
    );
    assert!(resources::catalog(&dir.0, "k3s-arm64").is_err());
}

#[test]
fn k3d_airgap_is_in_resource_closure_and_mismatched_k3s_fails_before_deployment() {
    let dir = Temp::new();
    dir.write("setup/install.json",json!({"schemaVersion":1,"targets":{"k3d-arm64":{"deploymentTopology":true,"base":[],"environment":{}}},"components":{},"files":[]}).to_string());
    dir.write("stack/k3s/artifacts.yaml", "version: v1.35.5+k3s1\n");
    assert!(resources::catalog(&dir.0, "k3d-arm64").is_err());
    dir.write("stack/k3s/artifacts.yaml", "version: v1.36.3+k3s1\n");
    let c = resources::catalog(&dir.0, "k3d-arm64").unwrap();
    assert_eq!(c, resources::catalog(&dir.0, "k3d-arm64").unwrap());
    let p = resources::plan(&c, "k3d-arm64", &json!([])).unwrap();
    assert_eq!(p["files"].as_array().unwrap().len(), 1);
    assert_eq!(p["files"][0]["path"], json!("k3s/k3s-airgap-images-arm64.tar.zst"));
    assert_eq!(p["files"][0]["size"], json!(241933732u64));
    assert_eq!(
        p["files"][0]["sha256"],
        json!("856feb047453cd697c2bc4dcf20127448554c8366be992a211558a13d830a038")
    );
    assert_eq!(p["target"]["k3sAirgap"], p["files"][0]["path"]);
    let mut c = c;
    c["targets"]["k3d-arm64"]["k3sAirgap"] = json!("unselected.tar");
    assert!(resources::plan(&c, "k3d-arm64", &json!([])).is_err());
}
#[test]
fn image_archives_are_selected_verified_files_and_conflicting_versions_fail() {
    let dir = Temp::new();
    let mut c = catalog();
    let image = json!({"file":"task.bin","reference":"example/task:v1","digest":format!("sha256:{}","a".repeat(64))});
    c["components"]["task"]["images"] = json!([image]);
    dir.write("setup/install.json", c.to_string());
    let parsed = resources::catalog(&dir.0, "k3s-amd64").unwrap();
    assert_eq!(
        resources::plan(&parsed, "k3s-amd64", &json!(["task"])).unwrap()["images"],
        json!([image])
    );
    assert_eq!(
        resources::plan(&parsed, "k3s-amd64", &json!([])).unwrap()["images"],
        json!([])
    );
    let mut conflict = image.clone();
    conflict["file"] = json!("core.bin");
    conflict["digest"] = json!(format!("sha256:{}", "b".repeat(64)));
    c["components"]["core"]["images"] = json!([conflict]);
    assert!(resources::plan(&c, "k3s-amd64", &json!(["task"])).is_err());
    c["components"]["task"]["images"][0]["file"] = json!("optional.bin");
    dir.write("setup/install.json", c.to_string());
    assert!(resources::catalog(&dir.0, "k3s-amd64").is_err());
    c["targets"]["k3s-amd64"]["toolboxArchive"] = json!("optional.bin");
    c["targets"]["k3s-amd64"]["toolboxImageId"] = json!(format!("sha256:{}", "a".repeat(64)));
    assert!(resources::plan(&c, "k3s-amd64", &json!([])).is_err());
    c["targets"]["k3s-amd64"]["toolboxArchive"] = json!("core.bin");
    c["targets"]["k3s-amd64"]["dockerArchives"] =
        json!([{"file":"core.bin","images":[{"name":"example/k3s:v1","id":"latest"}]}]);
    assert!(resources::plan(&c, "k3s-amd64", &json!([])).is_err());
}
#[test]
fn release_paths_and_symlinks_cannot_escape_the_package_directory() {
    let dir = Temp::new();
    let mut c = catalog();
    c["files"][0]["path"] = json!("../escape");
    dir.write("setup/install.json", c.to_string());
    assert!(resources::catalog(&dir.0, "k3s-amd64").is_err());
    for path in [
        "../escape",
        "/tmp/escape",
        "images/../escape",
        "images//app",
        "images/./app",
    ] {
        assert!(resources::destination(&dir.0, path).is_err());
    }
    #[cfg(unix)]
    {
        let outside = Temp::new();
        outside.write("core.bin", b"fixture artifact\n");
        std::os::unix::fs::symlink(&outside.0, dir.0.join("linked")).unwrap();
        assert!(resources::destination(&dir.0, "linked/core.bin").is_err());
        std::os::unix::fs::symlink(outside.0.join("core.bin"), dir.0.join("core.bin")).unwrap();
        let p = resources::plan(&catalog(), "k3s-amd64", &json!([])).unwrap();
        assert!(resources::prepare_files(&p, &dir.0, true, &process::Cancellation::default(), &|_| {}).is_err());
    }
}
#[test]
fn model_keys_are_private_redacted_preserved_for_same_endpoint_and_explicitly_clearable() {
    let dir = Temp::new();
    let path = dir.0.join("config.json");
    let model = json!({"provider":"bigmodel","baseUrl":"https://models.example.internal/v1","apiKey":"test-model-secret","fast":"fast-model","deep":"reasoning-model"});
    let mut v = input(install("single-k3d"), false);
    v["models"] = model.clone();
    v["revision"] = Value::Null;
    let saved = config::save(&path, &v).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(!config::public(&saved).to_string().contains("test-model-secret"));
    assert_eq!(config::public(&saved)["config"]["models"]["hasApiKey"], json!(true));
    let mut public = model.clone();
    public.as_object_mut().unwrap().remove("apiKey");
    assert_eq!(config::models(&public, &model).unwrap()["apiKey"], model["apiKey"]);
    let mut other = public.clone();
    other["baseUrl"] = json!("https://other.example/v1");
    assert!(config::models(&other, &model).is_err());
    public["apiKey"] = json!("");
    assert_eq!(config::models(&public, &model).unwrap()["apiKey"], json!(""));
    v["models"] = Value::Null;
    assert!(valid(&v).get("models").is_none());
    for provider in ["", "Big Model", "../invalid"] {
        let mut m = model.clone();
        m["provider"] = json!(provider);
        assert!(config::models(&m, &Value::Null).is_err());
    }
    let mut m = model.clone();
    m.as_object_mut().unwrap().remove("provider");
    assert!(config::models(&m, &Value::Null).is_err());
    for url in [
        "file:///etc/passwd",
        "https://user:pass@host/v1",
        "https://host/v1?key=secret",
        "https://host/#secret",
    ] {
        let mut m = model.clone();
        m["baseUrl"] = json!(url);
        assert!(config::models(&m, &Value::Null).is_err());
    }
}
#[test]
fn model_deployment_replaces_deferred_modes_and_queues_without_keys_in_values() {
    let mut v = input(install("single-k3d"), false);
    v["models"] = json!({"provider":"bigmodel","baseUrl":"https://models.example.internal/v1","apiKey":"test-model-secret","fast":"fast-model","deep":"reasoning-model"});
    let c = valid(&v);
    let out = deploy::environment_for(
        &c,
        json!({"releases":{"apeiron":{"values":{"models":{"deferred":true},"keep":true}}}}),
    )
    .unwrap();
    assert!(!out.to_string().contains("test-model-secret"));
    let values = &out["releases"]["apeiron"]["values"];
    assert_eq!(values["keep"], json!(true));
    assert_eq!(values["allowedModels"], json!(""));
    assert_eq!(values["defaultModel"], json!(""));
    assert!(values["models"].get("deferred").is_none());
    assert_eq!(
        values["models"]["modes"],
        json!({"fast":"apeiron-flash","deep":"apeiron-pro"})
    );
    assert_eq!(
        values["models"]["queues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["model"].clone())
            .collect::<Vec<_>>(),
        vec![json!("fast-model"), json!("reasoning-model")]
    );
    assert_eq!(values["models"]["providers"][0]["id"], json!("bigmodel"));
    assert_eq!(
        values["models"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| vec![m["id"].clone(), m["provider"].clone(), m["model"].clone()])
            .collect::<Vec<_>>(),
        vec![
            vec![json!("apeiron-flash"), json!("bigmodel"), json!("fast-model")],
            vec![json!("apeiron-pro"), json!("bigmodel"), json!("reasoning-model")]
        ]
    );
    v["models"]["deep"] = v["models"]["fast"].clone();
    let same = models::values(&v["models"]);
    assert_eq!(same["models"].as_array().unwrap().len(), 2);
    assert_eq!(same["modes"]["deep"], json!("apeiron-pro"));
    assert_eq!(same["queues"].as_array().unwrap().len(), 1);
}

#[cfg(unix)]
impl Temp {
    pub fn tool(&self, name: &str, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.write(name, script);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    pub fn env(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "PATH".into(),
                format!("{}:{}", self.0.display(), std::env::var("PATH").unwrap_or_default()),
            ),
            ("CAPTURE".into(), self.0.join("capture").to_string_lossy().into_owned()),
        ])
    }
}
#[test]
#[cfg(unix)]
fn model_credentials_use_only_stdin_for_native_and_docker_and_failures_are_redacted() {
    use base64::Engine;
    let dir = Temp::new();
    for name in ["kubectl", "docker"] {
        dir.tool(name,"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURE.args\"\ncat > \"$CAPTURE.stdin\"\necho 'secret diagnostic' >&2\nexit \"${FAIL:-0}\"\n");
    }
    let mut env = dir.env();
    env.insert("LAB_CONTAINER".into(), "setup-owned".into());
    env.insert("LAB_CLUSTER".into(), "setup-test".into());
    let model = json!({"apiKey":"test-model-secret"});
    for runner in ["native", "docker"] {
        let target = json!({"runner":runner,"kubeconfig":"/tmp/kubeconfig","workDir":"/tmp/work","image":"toolbox"});
        cluster::model_key(&model, &target, &env, &process::Cancellation::default()).unwrap();
        let args = fs::read_to_string(dir.0.join("capture.args")).unwrap();
        assert!(!args.contains("test-model-secret"));
        assert!(args.contains("--server-side"));
        let sent: Value = serde_json::from_slice(&fs::read(dir.0.join("capture.stdin")).unwrap()).unwrap();
        for (index, name, key) in [
            (1, "apeiron-model-keys", "APEIRON_MODEL_API_KEY_SETUP"),
            (2, "apeiron-scode-creds", "MODEL_API_KEY_SETUP"),
        ] {
            assert_eq!(sent["items"][index]["metadata"]["name"], json!(name));
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(sent["items"][index]["data"][key].as_str().unwrap())
                    .unwrap(),
                b"test-model-secret"
            );
        }
        if runner == "docker" {
            assert!(args.contains("--pull=never"));
            assert!(args.contains("/tmp/work/state/kubeconfig:/kubeconfig:ro"));
        }
        env.insert("FAIL".into(), "1".into());
        let error = cluster::model_key(&model, &target, &env, &process::Cancellation::default()).unwrap_err();
        assert!(error.message.contains("无法写入模型 Secret"));
        assert!(!error.message.contains("secret diagnostic"));
        env.remove("FAIL");
    }
}
#[test]
#[cfg(unix)]
fn helm_preflight_pins_kubeconfig_rejects_pending_and_unreadable_results() {
    let dir = Temp::new();
    for name in ["helm", "docker"] {
        dir.tool(name,"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURE.args\"\ncat \"$CAPTURE.response\"\necho 'private connection data' >&2\nexit \"${FAIL:-0}\"\n");
    }
    let mut env = dir.env();
    let target = json!({"runner":"native","kubeconfig":"/fixture/kubeconfig"});
    dir.write("capture.response", "[]");
    cluster::helm_state(&target, &env, &process::Cancellation::default()).unwrap();
    assert_eq!(
        fs::read_to_string(dir.0.join("capture.args"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        vec![
            "--kubeconfig",
            "/fixture/kubeconfig",
            "list",
            "--pending",
            "--all-namespaces",
            "--output",
            "json"
        ]
    );
    for status in ["pending-install", "pending-upgrade", "pending-rollback"] {
        dir.write(
            "capture.response",
            json!([{"name":"apeiron","namespace":"apeiron","status":status}]).to_string(),
        );
        assert!(cluster::helm_state(&target, &env, &process::Cancellation::default())
            .unwrap_err()
            .message
            .contains(&format!("apeiron/apeiron（{status}）")));
    }
    for value in ["", "{}", "[{\"name\":\"bad data\"}]"] {
        dir.write("capture.response", value);
        assert!(cluster::helm_state(&target, &env, &process::Cancellation::default()).is_err());
    }
    dir.write("capture.response", "[]");
    env.insert("FAIL".into(), "1".into());
    assert!(!cluster::helm_state(&target, &env, &process::Cancellation::default())
        .unwrap_err()
        .message
        .contains("private connection data"));
    env.remove("FAIL");
    env.insert("LAB_CONTAINER".into(), "owned-toolbox".into());
    env.insert("LAB_CLUSTER".into(), "owned-cluster".into());
    cluster::helm_state(
        &json!({"runner":"docker","workDir":"/fixture/work","image":"fixture-image"}),
        &env,
        &process::Cancellation::default(),
    )
    .unwrap();
    let args = fs::read_to_string(dir.0.join("capture.args")).unwrap();
    for expected in [
        "--pull=never",
        "owned-toolbox",
        "k3d-owned-cluster",
        "/fixture/work/state/kubeconfig:/kubeconfig:ro",
    ] {
        assert!(args.contains(expected));
    }
    assert!(!args.contains("docker.sock"));
}
fn public_installation() -> Value {
    let mut i = install("single-k3s");
    i["domain"] = json!("team.example.com");
    i["entryIp"] = json!("192.168.1.10");
    i["publicAccess"] = json!({"mode":"relay","publicIp":"8.8.8.8","tunnelPort":19444,"gateway":{"host":"edge-host","sshUser":"root","sshKey":"","sshPort":22}});
    i
}
#[test]
fn public_edge_keeps_origin_separate_and_rejects_private_addresses_names_and_shell_input() {
    let i = public_installation();
    let result = config::installation(&i).unwrap();
    assert_eq!(result["entryIp"], json!("192.168.1.10"));
    assert_eq!(result["publicAccess"]["publicIp"], json!("8.8.8.8"));
    for ip in ["127.0.0.1", "10.0.0.1", "172.16.1.1", "192.168.1.1", "100.64.0.1"] {
        let mut v = i.clone();
        v["publicAccess"]["publicIp"] = json!(ip);
        assert!(config::installation(&v).is_err());
    }
    for domain in ["team.internal", "team.local", "team.test"] {
        let mut v = i.clone();
        v["domain"] = json!(domain);
        assert!(config::installation(&v).is_err());
    }
    let mut v = i.clone();
    v["topology"] = json!("single-k3d");
    assert!(config::installation(&v).is_err());
    let mut v = i;
    v["publicAccess"]["gateway"]["host"] = json!("host;id");
    assert!(config::installation(&v).is_err());
}
#[test]
fn offline_setup_cannot_claim_to_provision_online_public_edge() {
    assert!(config::validate(&input(public_installation(), true), &Value::Null).is_err());
}
#[test]
fn public_access_never_requests_private_ca_or_hosts_installation() {
    let result = access::collect(
        &json!({"installation":public_installation()}),
        Path::new("/unused"),
        &process::Cancellation::default(),
    )
    .unwrap();
    assert_eq!(result["info"]["public"], json!(true));
    assert_eq!(result["info"]["entryIp"], json!("8.8.8.8"));
    assert!(result.get("ca").is_none());
    assert!(result.get("hosts").is_none());
}
#[test]
fn paired_configuration_accepts_opaque_id_but_never_manual_ssh_or_forged_ids() {
    let mut i = public_installation();
    i["publicAccess"].as_object_mut().unwrap().remove("gateway");
    i["publicAccess"]["pairingId"] = json!("a".repeat(32));
    assert_eq!(config::installation(&i).unwrap()["publicAccess"], i["publicAccess"]);
    let mut wrong = i.clone();
    wrong["publicAccess"]["pairingId"] = json!("../other");
    assert!(config::installation(&wrong).is_err());
    let mut wrong = i.clone();
    wrong["publicAccess"]["gateway"] = json!({"host":"elsewhere"});
    assert!(config::installation(&wrong).is_err());
    i["publicAccess"]["mode"] = json!("direct");
    assert!(config::installation(&i).is_err());
}
#[test]
fn pairing_projection_hides_secrets_and_cannot_cross_teams_or_resurrect_revoked_entries() {
    let dir = Temp::new();
    let config = dir.0.join("config.json");
    let id = "a".repeat(32);
    let mut descriptor = json!({"id":id,"version":1,"identity":"apeiron-123456789abc","domain":"team.example.com","publicIp":"8.8.8.8","host":"edge.example.com","sshPort":22,"tunnelPort":19444,"hostKey":"public-host-key","deviceKey":"must-not-reach-browser","inviteKey":"must-not-reach-browser"});
    let file = format!("connections/{id}/connection.json");
    dir.write(&file, descriptor.to_string());
    fs::create_dir_all(dir.0.join("connections").join("b".repeat(32))).unwrap();
    let rows = pairing::list(&config).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert!(!rows.to_string().contains("must-not-reach-browser"));
    assert!(!rows.to_string().contains("public-host-key"));
    let mut installation = public_installation();
    assert!(pairing::scope(&config, &id, &installation).is_ok());
    installation["domain"] = json!("other.example.com");
    assert!(pairing::scope(&config, &id, &installation).is_err());
    installation["domain"] = json!("team.example.com");
    descriptor["state"] = json!("revoked");
    dir.write(&file, descriptor.to_string());
    assert!(pairing::scope(&config, &id, &installation).is_err());
    assert_eq!(
        pairing::action(&config, "revoke", &id, &process::Cancellation::default()).unwrap()["state"],
        json!("revoked")
    );
}

#[test]
#[cfg(unix)]
fn public_access_missing_release_and_failed_verification_never_report_success() {
    let temp = Temp::new();
    let c = deploy::Context::fixture(&temp.0);
    let target = json!({"root":temp.0,"workDir":temp.0,"kubeconfig":"/tmp/kubeconfig","installation":{"topology":"single-k3s","domain":"team.example.com","entryIp":"192.168.1.10","publicAccess":{"mode":"relay","publicIp":"8.8.8.8","tunnelPort":19444,"gateway":{"host":"edge-host","sshUser":"root","sshKey":"","sshPort":22}}}});
    assert!(bootstrap::public_access("check", &target, &c, &temp.env())
        .unwrap_err()
        .message
        .contains("未包含"));
    assert!(!temp.0.join("capture").exists());
    temp.write("bootstrap/public_access.py", "fixture");
    for command in ["sudo", "python3"] {
        temp.tool(command, "#!/bin/sh\nprintf '%s\n' \"$*\" > \"$CAPTURE\"\nexit 1\n");
    }
    assert!(bootstrap::public_access("finish", &target, &c, &temp.env()).is_err());
    let plan = resources::json_file(&temp.0.join("public-access.json"), 65536).unwrap();
    assert_eq!(plan["access"]["mode"], "relay");
    assert_eq!(plan["ca"], temp.0.join("state/chentu-ca.crt").to_str().unwrap());
}

#[test]
fn offline_installation_uses_only_explicit_bundle_and_ports_preserve_topology_contract() {
    let temp = Temp::new();
    let target = json!({"root":"/stale/developer/source","offline":true,"bundleDir":temp.0});
    assert!(resources::resolve(&target, &process::Cancellation::default(), &|_| {})
        .unwrap_err()
        .message
        .contains("离线安装包缺少"));
    for path in [
        "setup/install.json",
        "deploy/helmfile/run.sh",
        "deploy/helmfile/helmfile.yaml.gotmpl",
        "deploy/helmfile/scripts/check.py",
        "cli/src/chentu/environment.py",
        "tests/lab/helmfile.sh",
    ] {
        temp.write(&format!("chentu/{path}"), "fixture");
    }
    assert_eq!(
        resources::resolve(&target, &process::Cancellation::default(), &|_| {}).unwrap(),
        temp.0.join("chentu")
    );
    let mut value = input(install("multi-k3s"), true);
    value["deployment"]["root"] = json!("/stale/developer/source");
    value["deployment"]["bundleDir"] = json!(temp.0);
    assert_eq!(valid(&value)["deployment"]["root"], "");
    let mut local = install("single-k3d");
    local["httpPort"] = json!(54322);
    local["httpsPort"] = json!(54323);
    let parsed = valid(&input(local.clone(), false));
    assert_eq!(parsed["deployment"]["installation"]["httpPort"], 54322);
    assert_eq!(parsed["deployment"]["installation"]["httpsPort"], 54323);
    for port in [
        json!(0),
        json!(65536),
        json!(1.5),
        json!("54321"),
        json!(true),
        json!(54322),
    ] {
        local["httpsPort"] = port;
        assert!(config::validate(&input(local.clone(), false), &Value::Null).is_err());
    }
    let mut native = install("multi-k3s");
    native["httpsPort"] = json!(54321);
    assert!(config::validate(&input(native, false), &Value::Null).is_err());
    for (topology, ip) in [("single-k3s", "127.0.0.1"), ("single-k3d", "192.0.2.10")] {
        let mut i = install(topology);
        i["entryIp"] = json!(ip);
        assert!(config::validate(&input(i, false), &Value::Null).is_err());
    }
}
