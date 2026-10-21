//! No real cluster access: fixed GET collection with a fake local kubectl.
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("lifecycle-migrate-{:016x}", rand::random::<u64>()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn write(&self, name: &str, v: &Value) {
        fs::write(self.0.join(name), v.to_string()).unwrap();
    }
    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_apeiron"))
            .args(["lifecycle"])
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[cfg(unix)]
#[test]
fn collect_migrate_preserves_legacy_and_secrets_and_never_accepts_unknown_inventory() {
    use std::os::unix::fs::PermissionsExt;
    let t = Temp::new();
    t.write("manifest", &json!({"schemaVersion":1,"kind":"chentu-release","release":"0.1.0-dev.20261016.1","components":{"apeiron":{},"postgres":{}}}));
    // Actual legacy generation: no installationId, generation or clusterUid.
    let legacy = json!({"schemaVersion":1,"checkpoint":"bootstrap-complete","commandActive":false,"environment":{"secret":"legacy-private-value"}});
    t.write("legacy", &legacy);
    t.write("configuration", &json!({"password":"configuration-private-value"}));
    t.write("namespace", &json!({"metadata":{"uid":"cluster-one"}}));
    t.write("nodes", &json!({"items":[{"metadata":{"name":"server-one","uid":"node-uid","labels":{"node-role.kubernetes.io/control-plane":""}},"status":{"nodeInfo":{"architecture":"amd64","kubeletVersion":"v1.36.3+k3s1"},"conditions":[{"type":"Ready","status":"True"}]}}]}));
    t.write("pods", &json!({"items":[{"metadata":{"name":"app-1","namespace":"app","uid":"pod-uid","annotations":{"secret":"pod-private-value"}},"spec":{"containers":[{"env":[{"value":"env-private-value"}]}]},"status":{"phase":"Running","containerStatuses":[{"name":"app","image":"app@sha256:abc","imageID":"containerd://sha256:abc","ready":true}]}}]}));
    t.write("volumes", &json!({"items":[]}));
    let tool = t.0.join("kubectl");
    fs::write(&tool, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{0}/commands'\ncase \"$*\" in\n*'get namespace kube-system'*) /bin/cat '{0}/namespace';;\n*'get nodes'*) /bin/cat '{0}/nodes';;\n*'get pods --all-namespaces'*) /bin/cat '{0}/pods';;\n*'get persistentvolumeclaims --all-namespaces'*) /bin/cat '{0}/volumes';;\n*) exit 91;;\nesac\n",t.0.display())).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
    let kubeconfig = t.0.join("kubeconfig");
    fs::write(&kubeconfig, "not-a-real-context").unwrap();
    let collect = [
        "collect",
        "--kubectl",
        tool.to_str().unwrap(),
        "--kubeconfig",
        kubeconfig.to_str().unwrap(),
        "--context",
        "lab",
        "--manifest",
        "manifest",
        "--legacy",
        "legacy",
        "--configuration",
        "configuration",
        "--topology",
        "single-k3s",
        "--network",
        "offline",
        "--out",
        "collection",
    ];
    let output = t.cli(&collect);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let data = fs::read_to_string(t.0.join("collection")).unwrap();
    for secret in [
        "legacy-private-value",
        "configuration-private-value",
        "pod-private-value",
        "env-private-value",
    ] {
        assert!(!data.contains(secret));
    }
    let commands = fs::read_to_string(t.0.join("commands")).unwrap();
    assert_eq!(commands.lines().count(), 6);
    assert!(commands
        .lines()
        .all(|s| s.contains("--context lab --request-timeout=15s get ") && !s.contains("secret")));
    assert!(!t.cli(&collect).status.success());
    assert_eq!(
        fs::read_to_string(t.0.join("commands")).unwrap(),
        commands,
        "existing output rejected before reads"
    );
    let migrate = [
        "migrate-record",
        "--collection",
        "collection",
        "--manifest",
        "manifest",
        "--legacy",
        "legacy",
        "--configuration",
        "configuration",
        "--installation-id",
        "legacy-one",
        "--out",
        "migration",
    ];
    let output = t.cli(&migrate);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let migration: Value = serde_json::from_slice(&fs::read(t.0.join("migration")).unwrap()).unwrap();
    assert_eq!(migration["status"], "review-required");
    assert_eq!(migration["executionAllowed"], false);
    assert_eq!(migration["legacyClusterUidPresent"], false);
    t.write("record-candidate", &migration["installationCandidate"]);
    t.write("observation-candidate", &migration["observationCandidate"]);
    t.write(
        "intent",
        &json!({"schemaVersion":1,"operation":"application-upgrade",
        "installationId":"legacy-one","clusterUid":"cluster-one","expectedGeneration":1,
        "selectedComponents":["apeiron","postgres"],"addAgents":[],"protection":null}),
    );
    let planned = t.cli(&[
        "plan",
        "--installation",
        "record-candidate",
        "--observation",
        "observation-candidate",
        "--intent",
        "intent",
        "--manifest",
        "manifest",
    ]);
    assert!(planned.status.success(), "{}", String::from_utf8_lossy(&planned.stderr));
    let plan: Value = serde_json::from_slice(&planned.stdout).unwrap();
    assert_eq!(plan["planningStatus"], "blocked");
    assert_eq!(plan["executionAllowed"], false);
    for code in ["BASELINE_UNVERIFIED", "COMPONENT_STATE_UNKNOWN", "CLUSTER_UNHEALTHY"] {
        assert!(plan["blockers"].as_array().unwrap().iter().any(|b| b["code"] == code));
    }
    assert_eq!(migration["installationCandidate"]["baselineEvidence"], json!([]));
    assert_eq!(
        migration["installationCandidate"]["components"]["apeiron"]["status"],
        "unknown"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(t.0.join("legacy")).unwrap()).unwrap(),
        legacy
    );
    for name in ["collection", "migration"] {
        assert_eq!(
            fs::metadata(t.0.join(name)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    fs::remove_file(t.0.join("migration")).unwrap();
    t.write("configuration", &json!({"password":"changed"}));
    assert!(!t.cli(&migrate).status.success());
    assert!(!t.0.join("migration").exists());
    // A legacy UID must not be rebound to a different collected cluster even
    // when the file digests themselves match the new collection.
    t.write("configuration", &json!({"password":"configuration-private-value"}));
    t.write(
        "legacy",
        &json!({"schemaVersion":1,"checkpoint":"bootstrap-complete",
        "commandActive":false,"clusterUid":"another-cluster"}),
    );
    let mut recollect = collect.to_vec();
    *recollect.last_mut().unwrap() = "collection-other-legacy";
    assert!(t.cli(&recollect).status.success());
    let mut wrong_cluster = migrate.to_vec();
    let at = wrong_cluster.iter().position(|s| *s == "--collection").unwrap();
    wrong_cluster[at + 1] = "collection-other-legacy";
    assert!(!t.cli(&wrong_cluster).status.success());
    assert!(!t.0.join("migration").exists());
    // Reject a cluster replacement during capture without leaving an output.
    fs::write(&tool, format!("#!/bin/sh\ncase \"$*\" in\n*'get namespace kube-system'*) /bin/cat '{0}/namespace'; /bin/cp '{0}/next-namespace' '{0}/namespace';;\n*'get nodes'*) /bin/cat '{0}/nodes';;\n*'get pods --all-namespaces'*) /bin/cat '{0}/pods';;\n*'get persistentvolumeclaims --all-namespaces'*) /bin/cat '{0}/volumes';;\n*) exit 91;;\nesac\n", t.0.display())).unwrap();
    t.write("next-namespace", &json!({"metadata":{"uid":"replacement-cluster"}}));
    *recollect.last_mut().unwrap() = "unstable-collection";
    assert!(!t.cli(&recollect).status.success());
    assert!(!t.0.join("unstable-collection").exists());
    // Imported collection metadata must not turn into an arbitrary secret echo.
    t.write("legacy", &legacy);
    let mut poisoned: Value = serde_json::from_str(&data).unwrap();
    poisoned["unverified"] = json!({"password":"collection-private-value"});
    t.write("poisoned-collection", &poisoned);
    wrong_cluster[at + 1] = "poisoned-collection";
    let rejected = t.cli(&wrong_cluster);
    assert!(!rejected.status.success());
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains("collection-private-value"));
    assert!(!t.0.join("migration").exists());
}
#[test]
fn inspect_source_distinguishes_declared_multi_k3s_from_unsupported_amd64_targets() {
    let t = Temp::new();
    fs::create_dir_all(t.0.join("source/setup")).unwrap();
    fs::create_dir_all(t.0.join("source/stack/k3s")).unwrap();
    fs::write(t.0.join("source/stack/k3s/artifacts.yaml"), "version: v1.36.3+k3s1\n").unwrap();
    t.write(
        "source/setup/install.json",
        &json!({"schemaVersion":1,"targets":{"k3s-amd64":{}},"files":[],"components":{}}),
    );
    t.write("manifest",&json!({"schemaVersion":1,"kind":"chentu-release","release":"0.1.0-dev.20261016.1","profiles":{"k3s-amd64":{"architecture":"amd64","supportedTopologies":["multi-k3s"]}}}));
    let output = t.cli(&[
        "inspect-source",
        "--root",
        "source",
        "--manifest",
        "manifest",
        "--architecture",
        "amd64",
        "--out",
        "inspection",
    ]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&fs::read(t.0.join("inspection")).unwrap()).unwrap();
    assert_eq!(report["executionAllowed"], false);
    assert_eq!(report["checks"][0]["blockers"], json!(["SOURCE_TOPOLOGY_NOT_DECLARED"]));
    assert_eq!(report["checks"][1]["blockers"], json!([]));
    assert_eq!(report["checks"][1]["installationVerified"], false);
    assert_eq!(
        report["checks"][2]["blockers"],
        json!(["SOURCE_TOPOLOGY_NOT_DECLARED", "SOURCE_CATALOG_REJECTED"])
    );
    assert_eq!(
        report["checks"][2]["catalogDiagnostic"],
        "K3s 版本或架构与固定基础镜像包不匹配。"
    );
}
