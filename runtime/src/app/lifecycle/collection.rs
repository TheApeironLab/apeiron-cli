//! Explicit opt-in collection. Only fixed Kubernetes GETs; never resume or mutate.
//! Imported checkpoints and collected snapshots are evidence, not acceptance.
use super::*;
use std::process::Command;
const UNVERIFIED: &[&str] = &[
    "component-artifacts-and-schemas",
    "effective-values",
    "data-ownership",
    "foundation-configuration",
    "installation-completion",
    "application-health",
    "baseline-acceptance",
];

fn read(path: &str) -> Result<(Value, String)> {
    let bytes = resources::read(Path::new(path), LIMIT)?;
    Ok((
        serde_json::from_slice(&bytes).map_err(|_| invalid())?,
        config::hash(&bytes),
    ))
}
fn get(options: &BTreeMap<String, String>, resource: &[&str]) -> Result<Value> {
    let mut command = Command::new(required(options, "kubectl")?);
    command.args([
        "--kubeconfig",
        required(options, "kubeconfig")?,
        "--context",
        required(options, "context")?,
        "--request-timeout=15s",
        "get",
    ]);
    command.args(resource).args(["-o", "json"]);
    let bytes = process::capture(
        &mut command,
        b"",
        Duration::from_secs(20),
        LIMIT as usize,
        &process::Cancellation::default(),
    )?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}
fn uid(value: &Value) -> Result<String> {
    let uid = value["metadata"]["uid"].as_str().ok_or_else(invalid)?;
    check(scoped_id(uid))?;
    Ok(uid.into())
}
fn nodes(value: &Value) -> Result<Value> {
    let mut result = Vec::new();
    for n in value["items"].as_array().ok_or_else(invalid)? {
        let id = n["metadata"]["name"].as_str().ok_or_else(invalid)?;
        let labels = n["metadata"]["labels"].as_object().ok_or_else(invalid)?;
        let architecture = n["status"]["nodeInfo"]["architecture"].as_str().ok_or_else(invalid)?;
        let version = n["status"]["nodeInfo"]["kubeletVersion"].as_str().ok_or_else(invalid)?;
        check(
            scoped_id(id)
                && ["amd64", "arm64"].contains(&architecture)
                && config::matches(r"^[A-Za-z0-9][A-Za-z0-9.+_-]{0,127}$", version),
        )?;
        let server = labels.contains_key("node-role.kubernetes.io/control-plane")
            || labels.contains_key("node-role.kubernetes.io/master");
        let ready = n["status"]["conditions"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .any(|c| c["type"] == "Ready" && c["status"] == "True");
        result.push(json!({"id":id,"uid":uid(n)?,"role":if server {"server"} else {"agent"},
            "architecture":architecture,"kubeletVersion":version,"ready":ready}));
    }
    result.sort_by_key(|n| n["id"].as_str().unwrap().to_string());
    check(!result.is_empty())?;
    Ok(json!(result))
}
fn projection(value: &Value, volume: bool) -> Result<Value> {
    let mut result = Vec::new();
    for item in value["items"].as_array().ok_or_else(invalid)? {
        let metadata = &item["metadata"];
        let name = metadata["name"].as_str().ok_or_else(invalid)?;
        let namespace = metadata["namespace"].as_str().ok_or_else(invalid)?;
        check(scoped_id(name) && scoped_id(namespace))?;
        // Deliberately omit env, Secret refs, annotations, endpoints and spec values.
        let mut row = json!({"name":name,"namespace":namespace,"uid":uid(item)?,
            "phase":item["status"]["phase"].as_str().unwrap_or("Unknown")});
        if volume {
            row["storageClass"] = json!(item["spec"]["storageClassName"].as_str().unwrap_or("unknown"));
            row["capacity"] = json!(item["status"]["capacity"]["storage"].as_str().unwrap_or("unknown"));
        } else {
            let mut images = Vec::new();
            for field in [
                "initContainerStatuses",
                "containerStatuses",
                "ephemeralContainerStatuses",
            ] {
                if let Some(statuses) = item["status"][field].as_array() {
                    for status in statuses {
                        images.push(json!({"name":status["name"],"image":status["image"],
                            "imageId":status["imageID"],"ready":status["ready"]}));
                    }
                }
            }
            row["images"] = json!(images);
        }
        result.push(row);
    }
    result.sort_by_key(|r| format!("{}/{}", r["namespace"], r["name"]));
    Ok(json!(result))
}
fn collect(options: &BTreeMap<String, String>) -> Result<Value> {
    check(options.keys().all(|k| {
        [
            "kubectl",
            "kubeconfig",
            "context",
            "manifest",
            "legacy",
            "configuration",
            "topology",
            "network",
            "out",
        ]
        .contains(&k.as_str())
    }))?;
    for key in ["kubectl", "kubeconfig"] {
        check(Path::new(required(options, key)?).is_absolute())?;
    }
    check(scoped_id(required(options, "context")?))?;
    let network = required(options, "network")?;
    check(["offline", "online"].contains(&network))?;
    let topology = required(options, "topology")?;
    check(["single-k3d", "single-k3s", "multi-k3s"].contains(&topology))?;
    let (manifest, manifest_hash) = read(required(options, "manifest")?)?;
    check(manifest["kind"] == "chentu-release" && manifest["schemaVersion"] == 1)?;
    let (_, legacy_hash) = read(required(options, "legacy")?)?;
    let (configuration, configuration_hash) = read(required(options, "configuration")?)?;
    let start = chrono::Utc::now().to_rfc3339();
    let cluster_uid = uid(&get(options, &["namespace", "kube-system"])?)?;
    let first_nodes = nodes(&get(options, &["nodes"])?)?;
    let pods = projection(&get(options, &["pods", "--all-namespaces"])?, false)?;
    let volumes = projection(&get(options, &["persistentvolumeclaims", "--all-namespaces"])?, true)?;
    let last_nodes = nodes(&get(options, &["nodes"])?)?;
    check(first_nodes == last_nodes && cluster_uid == uid(&get(options, &["namespace", "kube-system"])?)?)?;
    let topology_nodes: Vec<Node> = last_nodes
        .as_array()
        .unwrap()
        .iter()
        .map(|n| Node {
            id: n["id"].as_str().unwrap().into(),
            role: n["role"].as_str().unwrap().into(),
        })
        .collect();
    node_set(&topology_nodes, topology)?;
    check(
        last_nodes
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["architecture"].as_str())
            .collect::<BTreeSet<_>>()
            .len()
            == 1,
    )?;
    Ok(
        json!({"schemaVersion":1,"kind":"chentu-legacy-collection","readOnly":true,"executionAllowed":false,
        "startedAt":start,"completedAt":chrono::Utc::now().to_rfc3339(),"clusterUid":cluster_uid,
        "topology":topology,"network":network,"manifestSha256":manifest_hash,"legacySha256":legacy_hash,
        "configurationSha256":configuration_hash,"resumeConfigurationSha256":resume::config_hash(&configuration),
        "nodes":last_nodes,"pods":pods,"volumes":volumes,"atomicSnapshot":false,
        "coverage":["cluster-identity","nodes","runtime-images","pvc-metadata"],
        "unverified":UNVERIFIED}),
    )
}
fn migrate(options: &BTreeMap<String, String>) -> Result<Value> {
    check(options.keys().all(|k| {
        [
            "collection",
            "manifest",
            "legacy",
            "configuration",
            "installation-id",
            "out",
        ]
        .contains(&k.as_str())
    }))?;
    let (collected, collection_hash) = read(required(options, "collection")?)?;
    let keys = collected.as_object().ok_or_else(invalid)?;
    let fields = [
        "schemaVersion",
        "kind",
        "readOnly",
        "executionAllowed",
        "startedAt",
        "completedAt",
        "clusterUid",
        "topology",
        "network",
        "manifestSha256",
        "legacySha256",
        "configurationSha256",
        "resumeConfigurationSha256",
        "nodes",
        "pods",
        "volumes",
        "atomicSnapshot",
        "coverage",
        "unverified",
    ];
    check(
        keys.len() == fields.len()
            && keys.keys().all(|k| fields.contains(&k.as_str()))
            && collected["atomicSnapshot"] == false
            && collected["unverified"] == json!(UNVERIFIED),
    )?;
    let (legacy, legacy_hash) = read(required(options, "legacy")?)?;
    let (manifest, manifest_hash) = read(required(options, "manifest")?)?;
    let (configuration, configuration_hash) = read(required(options, "configuration")?)?;
    let installation_id = required(options, "installation-id")?;
    check(
        scoped_id(installation_id)
            && collected["schemaVersion"] == 1
            && collected["kind"] == "chentu-legacy-collection"
            && collected["readOnly"] == true
            && collected["executionAllowed"] == false
            && collected["manifestSha256"] == manifest_hash
            && collected["legacySha256"] == legacy_hash
            && collected["configurationSha256"] == configuration_hash
            && collected["resumeConfigurationSha256"] == resume::config_hash(&configuration)
            && legacy["schemaVersion"] == 1
            && legacy["checkpoint"] == "bootstrap-complete"
            && legacy["commandActive"] == false
            && manifest["schemaVersion"] == 1
            && manifest["kind"] == "chentu-release",
    )?;
    if !legacy["configHash"].is_null() {
        check(legacy["configHash"] == collected["resumeConfigurationSha256"])?;
    }
    if !legacy["clusterUid"].is_null() {
        check(legacy["clusterUid"] == collected["clusterUid"])?;
    }
    let mut nodes = Vec::new();
    let mut architectures = BTreeSet::new();
    for n in collected["nodes"].as_array().ok_or_else(invalid)? {
        nodes.push(json!({"id":n["id"],"role":n["role"]}));
        architectures.insert(n["architecture"].as_str().ok_or_else(invalid)?);
    }
    check(architectures.len() == 1)?;
    let mut components = json!({});
    for id in manifest["components"].as_object().ok_or_else(invalid)?.keys() {
        check(identifier(id))?;
        components[id] = json!({"status":"unknown","version":"unknown",
            "state":{"artifactDigests":{},"configurationSchema":"unknown","dataSchema":"unknown"},
            "configurationSha256":"0".repeat(64),"dataBindings":{}});
    }
    let record = json!({"schemaVersion":1,"kind":"chentu-installation-record","installationId":installation_id,
        "clusterUid":collected["clusterUid"],"generation":1,
        "release":{"version":manifest["release"],"manifestSha256":manifest_hash},
        "topology":collected["topology"],"architecture":architectures.first().unwrap(),"nodes":nodes,
        "configurationSha256":configuration_hash,"foundationSha256":"0".repeat(64),
        "components":components,"resources":{},"baselineEvidence":[],"acceptedExpansions":[]});
    validate_record(&decode::<Installation>(&record)?)?;
    check([json!("offline"), json!("online")].contains(&collected["network"]))?;
    let observation = json!({"schemaVersion":1,"installationId":installation_id,
        "clusterUid":record["clusterUid"],"generation":1,"recordSha256":identity(&record),
        "configurationSha256":configuration_hash,"foundationSha256":record["foundationSha256"],
        "nodes":record["nodes"],"components":{},"healthy":false,
        "drift":["legacy-baseline-unverified"],"network":collected["network"]});
    Ok(
        json!({"schemaVersion":1,"kind":"chentu-installation-migration","readOnly":true,
        "executionAllowed":false,"status":"review-required","liveVerified":false,
        "installationCandidate":record,"observationCandidate":observation,"collectionSha256":collection_hash,"legacySha256":legacy_hash,
        "unknownDigestSentinel":"0".repeat(64),"legacyClusterUidPresent":legacy["clusterUid"].is_string(),
        "gaps":UNVERIFIED,"remediation":remediation::guidance("BASELINE_UNVERIFIED"),
        "preserveLegacyCheckpoint":true,"resumeCompatible":false,
        "warning":"候选记录不是已验收基线；未知组件不能推断为已安装或缺席。需独立核实后验收，生产执行保持阻断。"}),
    )
}
fn inspect(options: &BTreeMap<String, String>) -> Result<Value> {
    check(
        options
            .keys()
            .all(|k| ["root", "manifest", "architecture", "out"].contains(&k.as_str())),
    )?;
    let architecture = required(options, "architecture")?;
    check(["amd64", "arm64"].contains(&architecture))?;
    let root = Path::new(required(options, "root")?);
    let (manifest, manifest_hash) = read(required(options, "manifest")?)?;
    check(manifest["schemaVersion"] == 1 && manifest["kind"] == "chentu-release")?;
    let mut rows = Vec::new();
    for (runtime, topology) in [("k3s", "single-k3s"), ("k3s", "multi-k3s"), ("k3d", "single-k3d")] {
        let target = format!("{runtime}-{architecture}");
        let catalog = resources::catalog(root, &target);
        let profile = &manifest["profiles"][&target];
        let declared = profile["architecture"] == architecture
            && profile["supportedTopologies"]
                .as_array()
                .is_some_and(|values| values.contains(&json!(topology)));
        let mut blockers = Vec::new();
        if !declared {
            blockers.push("SOURCE_TOPOLOGY_NOT_DECLARED");
        }
        if catalog.is_err() {
            blockers.push("SOURCE_CATALOG_REJECTED");
        }
        rows.push(json!({"target":target,"topology":topology,"declared":declared,
            "catalogAccepted":catalog.is_ok(),"catalogDiagnostic":catalog.err().map(|e|e.message),
            "blockers":blockers,"installationVerified":false}));
    }
    Ok(
        json!({"schemaVersion":1,"kind":"chentu-source-inspection","release":manifest["release"],
        "manifestSha256":manifest_hash,"architecture":architecture,"checks":rows,
        "readOnly":true,"executionAllowed":false,"scope":"catalog-and-support-matrix-only"}),
    )
}
pub(super) fn run(args: &[String]) -> Result<()> {
    let (pos, options) = parse(
        args,
        &[
            "kubectl",
            "kubeconfig",
            "context",
            "manifest",
            "legacy",
            "configuration",
            "topology",
            "network",
            "collection",
            "installation-id",
            "out",
            "root",
            "architecture",
        ],
        &["help"],
    )?;
    if options.contains_key("help") {
        println!("apeiron lifecycle collect --kubectl ABS_FILE --kubeconfig ABS_FILE --context NAME --manifest SOURCE --legacy FILE --configuration FILE --topology TOPOLOGY --network offline|online --out NEW_FILE\napeiron lifecycle migrate-record --collection FILE --manifest SOURCE --legacy FILE --configuration FILE --installation-id ID --out NEW_FILE\napeiron lifecycle inspect-source --root DIR --manifest SOURCE --architecture amd64 --out NEW_FILE\nCollection performs fixed GETs only; migration produces an unaccepted candidate and never resumes installation.");
        return Ok(());
    }
    check(pos.len() == 1)?;
    let out = required(&options, "out")?;
    // Fail before any cluster access if the target already exists (including a dangling symlink).
    check(std::fs::symlink_metadata(out).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound))?;
    let value = match pos.first().map(String::as_str) {
        Some("collect") => collect(&options)?,
        Some("migrate-record") => migrate(&options)?,
        Some("inspect-source") => inspect(&options)?,
        _ => return Err(invalid()),
    };
    private_write(
        Path::new(out),
        &serde_json::to_vec_pretty(&value).map_err(|_| invalid())?,
    )?;
    println!(
        "{}",
        json!({"kind":value["kind"],"saved":true,"executionAllowed":false})
    );
    Ok(())
}
