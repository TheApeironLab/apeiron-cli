use serde_json::{json, Value};
fn identity(value: &Value) -> String {
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
