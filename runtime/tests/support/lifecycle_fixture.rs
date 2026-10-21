// Shared by unit and several independent integration-test crates.
#![allow(dead_code)]
use serde_json::{json, Value};
pub fn identity(value: &Value) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}
pub fn hash(c: char) -> String {
    c.to_string().repeat(64)
}
fn component_state(id: &str, hash_value: &str, schema: &str) -> Value {
    json!({"artifactDigests":{format!("charts/{id}.tgz"):hash_value},"configurationSchema":"1","dataSchema":schema})
}
pub fn fixture(inner: bool) -> Value {
    let app = if inner { "apeiron" } else { "ontology" };
    let old = hash(if inner { '1' } else { '2' });
    let source = hash(if inner { 'a' } else { 'b' });
    let configuration = hash(if inner { 'c' } else { 'd' });
    let uid = if inner { "inner-cluster" } else { "outer-cluster" };
    let topology = if inner { "multi-k3s" } else { "single-k3s" };
    let mut nodes = vec![json!({"id":"server-1","role":"server"})];
    if inner {
        nodes.extend([
            json!({"id":"server-2","role":"server"}),
            json!({"id":"server-3","role":"server"}),
        ]);
    }
    let mut components = json!({});
    let mut contracts = json!({});
    let mut release_components = json!({});
    let mut artifacts = vec![];
    let mut rules = json!({});
    for (id, current, target, class) in [
        ("apeiron", hash('1'), hash('4'), "application"),
        ("ontology", hash('2'), hash('5'), "application"),
        ("postgres", hash('3'), hash('3'), "foundation"),
    ] {
        contracts[id] = json!({"class":class,"configurationSchema":"1","dataSchema":if id == "postgres" {"1"} else {"2"},"dataRoles":["database"]});
        release_components[id] = json!({"artifacts":[format!("charts/{id}.tgz")],"requires":if id == "postgres" {vec![]} else {vec!["postgres"]}});
        artifacts.push(json!({"id":format!("charts/{id}.tgz"),"sha256":target}));
        if id != "postgres" {
            rules[id] = json!({"component":id,"sourceArtifacts":{format!("charts/{id}.tgz"):current},
                "sourceConfigurationSchema":"1","sourceDataSchema":"1","targetConfigurationSchema":"1","targetDataSchema":"2",
                "requiredComponents":{"postgres":component_state("postgres",&hash('3'),"1")},"protectionRoles":["database"],
                "irreversible":false,"replaySafe":true,"evidence":[hash('e')]});
        }
    }
    for (id, digest_value) in [(app, old), ("postgres", hash('3'))] {
        components[id] = json!({"status":"installed","version":if inner {"1.0.0"} else {"0.9.0"},
            "state":component_state(id,&digest_value,"1"),"configurationSha256":configuration,"dataBindings":{"database":"shared-db"}});
    }
    let paths = vec![
        json!({"id":"inner-path","sourceManifestSha256":hash('a'),"topologies":["multi-k3s"],"architectures":["amd64"],
            "componentSets":[["apeiron","postgres"]],"migrationRules":["apeiron"],"evidence":[hash('e')]}),
        json!({"id":"outer-path","sourceManifestSha256":hash('b'),"topologies":["single-k3s"],"architectures":["amd64"],
            "componentSets":[["ontology","postgres"]],"migrationRules":["ontology"],"evidence":[hash('e')]}),
    ];
    let record = json!({"schemaVersion":1,"kind":"chentu-installation-record","installationId":format!("{uid}-install"),
        "clusterUid":uid,"generation":1,"release":{"version":if inner {"1.0.0"} else {"0.9.0"},"manifestSha256":source},
        "topology":topology,"architecture":"amd64","nodes":nodes,"configurationSha256":configuration,"foundationSha256":hash('f'),
        "components":components,"resources":{"shared-db":{"kind":"database","owner":"postgres","consumers":[app],
            "locatorRef":"database/service-ref","estimatedBytes":128,"backupAdapter":"pg-logical","restoreGranularity":"component"}},
        "baselineEvidence":[hash('e')],"acceptedExpansions":[]});
    let mut states = json!({});
    for (id, c) in components.as_object().unwrap() {
        states[id] = c["state"].clone();
    }
    let observation = json!({"schemaVersion":1,"installationId":record["installationId"],"clusterUid":uid,"generation":1,
        "recordSha256":identity(&record),"configurationSha256":configuration,"foundationSha256":hash('f'),"nodes":nodes,"components":states,
        "healthy":true,"drift":[],"network":if inner {"offline"} else {"online"}});
    let intent = json!({"schemaVersion":1,"operation":"application-upgrade","installationId":record["installationId"],"clusterUid":uid,
        "expectedGeneration":1,"selectedComponents":[app,"postgres"],"addAgents":[],"protection":{"owner":"operator", "rpoSeconds":0,
            "rtoSeconds":3600,"downtimeSeconds":1800,"independentCopyRef":"external/offline-copy","recipientFingerprint":"age-recipient-ref","capacityBytes":2048}});
    let manifest = json!({"schemaVersion":1,"kind":"chentu-release","release":"2.0.0",
        "profiles":{"k3s-amd64":{"architecture":"amd64","supportedTopologies":["single-k3s","multi-k3s"]}},
        "components":release_components,"artifacts":artifacts,
        "lifecycle":{"schemaVersion":1,"components":contracts,"backupAdapters":{"pg-logical":{"version":"1","kinds":["database"],
            "granularity":"component","status":"verified","evidence":[hash('e')]}},"migrationRules":rules,"upgradePaths":paths}});
    json!({"installation":record,"observation":observation,"intent":intent,"releaseManifestSha256":identity(&manifest),"releaseManifest":manifest})
}

// ---- journey helpers (moved from src/app/lifecycle/tests.rs so integration tests share one implementation) ----
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn refresh(input: &mut Value) {
    input["observation"]["recordSha256"] = json!(identity(&input["installation"]));
}

pub fn codes(plan: &Value) -> Vec<String> {
    plan["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["code"].as_str().unwrap().to_string())
        .collect()
}

/// Pure-JSON mirror of lifecycle::target_artifacts: {artifactId: sha256} for one component.
pub fn target_artifacts(manifest: &Value, id: &str) -> Value {
    let mut out = json!({});
    for artifact in manifest["components"][id]["artifacts"].as_array().unwrap() {
        let key = artifact.as_str().unwrap();
        for row in manifest["artifacts"].as_array().unwrap() {
            if row["id"] == *key {
                out[key] = row["sha256"].clone();
            }
        }
    }
    out
}

/// Rewrite the manifest into the installed source contract (restore/backup inputs
/// must describe the actual source package, not the upgrade target).
pub fn source_release(input: &mut Value) {
    let record = input["installation"].clone();
    let manifest = &mut input["releaseManifest"];
    manifest["release"] = record["release"]["version"].clone();
    manifest["lifecycle"]["migrationRules"] = json!({});
    manifest["lifecycle"]["upgradePaths"] = json!([]);
    for (id, component) in record["components"].as_object().unwrap() {
        if component["status"] != "installed" {
            continue;
        }
        let state = &component["state"];
        manifest["lifecycle"]["components"][id]["configurationSchema"] = state["configurationSchema"].clone();
        manifest["lifecycle"]["components"][id]["dataSchema"] = state["dataSchema"].clone();
        let hashes = state["artifactDigests"].as_object().unwrap();
        manifest["components"][id]["artifacts"] = json!(hashes.keys().collect::<Vec<_>>());
        for artifact in manifest["artifacts"].as_array_mut().unwrap() {
            if let Some(hash) = hashes.get(artifact["id"].as_str().unwrap()) {
                artifact["sha256"] = hash.clone();
            }
        }
    }
    let digest = identity(manifest);
    input["releaseManifestSha256"] = json!(digest);
    input["installation"]["release"]["manifestSha256"] = input["releaseManifestSha256"].clone();
    refresh(input);
}

pub fn backup_of(input: &Value) -> Value {
    let record = &input["installation"];
    let manifest = &input["releaseManifest"];
    let mut states = json!({});
    for id in input["intent"]["selectedComponents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
    {
        let c = &manifest["lifecycle"]["components"][id];
        states[id] = json!({"artifactDigests":target_artifacts(manifest,id),"configurationSchema":c["configurationSchema"],"dataSchema":c["dataSchema"]});
    }
    json!({"schemaVersion":1,"backupId":"backup-1","installationId":record["installationId"],"clusterUid":record["clusterUid"],
        "generation":record["generation"],"foundationSha256":record["foundationSha256"],"topology":record["topology"],"nodes":record["nodes"],
        "componentIds":input["intent"]["selectedComponents"],"resourceIds":["shared-db"],"level":"isolated-restore-verified",
        "manifestSha256":hash('d'),"independentCopyRef":"external/offline-copy","recipientFingerprint":"age-recipient-ref","restoreEvidence":[hash('e')],
        "sourceRelease":record["release"],"componentStates":states,"resourceIdentities":{"shared-db":identity(&record["resources"]["shared-db"])},"configurationSha256":record["configurationSha256"]})
}

pub fn accept_expansion(input: &mut Value, agents: &[&str], generation: i64) {
    let record = &mut input["installation"];
    for id in agents {
        record["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":id,"role":"agent"}));
    }
    record["generation"] = json!(generation);
    record["foundationSha256"] = json!(hash('9'));
    let node_ids: Vec<Value> = agents.iter().map(|a| json!(a)).collect();
    record["acceptedExpansions"] =
        json!([{"id": "expand-1", "generation": generation, "nodeIds": node_ids, "evidence": [hash('e')]}]);
    input["observation"]["nodes"] = input["installation"]["nodes"].clone();
    input["observation"]["generation"] = json!(generation);
    input["observation"]["foundationSha256"] = json!(hash('9'));
    input["intent"]["expectedGeneration"] = json!(generation);
}

pub struct Temp(PathBuf);
impl Temp {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("apeiron-lifecycle-journey-{:016x}", rand::random::<u64>()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn write_input(&self, name: &str, value: &Value) -> String {
        let path = self.0.join(format!("{name}.json"));
        fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        path.to_string_lossy().into_owned()
    }
    pub fn write_all(&self, input: &Value) -> Vec<String> {
        let mut args = vec![];
        for (field, flag) in [
            ("installation", "--installation"),
            ("observation", "--observation"),
            ("intent", "--intent"),
            ("releaseManifest", "--manifest"),
        ] {
            args.push(flag.into());
            args.push(self.write_input(field, &input[field]));
        }
        args
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn cli(args: &[String], root: &Path) -> std::process::Output {
    // Any cluster tool invocation is a test failure. The absolute CLI still works.
    let bin = root.join("poison-bin");
    fs::create_dir_all(&bin).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for tool in [
            "kubectl",
            "helm",
            "helmfile",
            "k3s",
            "ansible-playbook",
            "python3",
            "curl",
        ] {
            let path = bin.join(tool);
            fs::write(
                &path,
                format!(
                    "#!/bin/sh\nprintf invoked > '{}'\nexit 91\n",
                    root.join("unexpected-command").display()
                ),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let output = Command::new(
        option_env!("CARGO_BIN_EXE_apeiron")
            .map(str::to_owned)
            .or_else(|| std::env::var("CARGO_BIN_EXE_apeiron").ok())
            .expect("integration tests build the apeiron binary first"),
    )
    .args(args)
    .env("PATH", bin)
    .env("KUBECONFIG", root.join("nonexistent-context"))
    .env("APEIRON_LIFECYCLE_BIN", root.join("forbidden-forwarder"))
    .output()
    .unwrap();
    assert!(!root.join("unexpected-command").exists());
    output
}

use std::process::Command;

/// Bind the manifest to the exact file bytes the CLI will hash: the deployment
/// side derives the release digest from the file, so journeys must pin the
/// record/backup to those bytes, not to the compact value identity.
pub fn bind_manifest_to_file(input: &mut Value, path: &Path) {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path).unwrap();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    input["releaseManifestSha256"] = json!(digest);
    input["installation"]["release"]["manifestSha256"] = json!(digest);
    refresh(input);
}
