use serde_json::{json, Value};
use std::{fs, process::Command};

fn fixture() -> Value {
    let mut value = json!({"schemaVersion":1,"kind":"chentu-component-baseline-observation",
        "clusterUid":"cluster-one","releases":{"app/app":{"revision":4,"status":"deployed",
        "chartName":"app","chartVersion":"1.0","chartSha256":"a".repeat(64),
        "storedValuesSha256":"b".repeat(64),"storedManifestSha256":"c".repeat(64),
        "schemaStatus":"unverified","sourceProvenance":"observed-helm-record"}},
        "resources":{},"configurationHashes":{},"containers":[],"gaps":[],
        "acceptanceStatus":"review-required","executionAllowed":false});
    let hash = sha256(value.to_string().as_bytes());
    value["observationSha256"] = json!(hash);
    value
}
fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn inspect(value: &Value) -> std::process::Output {
    let path = std::env::temp_dir().join(format!("component-observation-{:016x}.json", rand::random::<u64>()));
    fs::write(&path, value.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_apeiron"))
        .args(["lifecycle", "inspect-component-baseline", "--baseline"])
        .arg(&path)
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    output
}
#[test]
fn component_observation_is_inspected_without_baseline_or_execution_acceptance() {
    let output = inspect(&fixture());
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["releaseCount"], 1);
    assert_eq!(result["acceptanceStatus"], "review-required");
    assert_eq!(result["executionAllowed"], false);
    assert_eq!(result["liveVerified"], false);
    assert_eq!(result["blockers"].as_array().unwrap().len(), 6);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("chartVersion"));
}
#[test]
fn tampered_or_self_accepted_component_observation_is_rejected() {
    let mut changed = fixture();
    changed["clusterUid"] = json!("other-cluster");
    assert!(!inspect(&changed).status.success());
    for (key, value) in [
        ("executionAllowed", json!(true)),
        ("acceptanceStatus", json!("accepted")),
    ] {
        let mut changed = fixture();
        changed.as_object_mut().unwrap().remove("observationSha256");
        changed[key] = value;
        changed["observationSha256"] = json!(sha256(changed.to_string().as_bytes()));
        assert!(!inspect(&changed).status.success());
    }
}
