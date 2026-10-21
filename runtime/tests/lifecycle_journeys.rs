//! Lifecycle operator journeys through the real CLI: file-to-file only, no cluster
//! or subprocess may ever be contacted (the poison-bin sentinel in `cli()` fails the
//! test if any tool is invoked). Each journey stitches the stages that the unit
//! matrix tests in isolation: validate → plan → blocked-but-persisted output →
//! execution doors stay shut.

#[path = "support/lifecycle_fixture.rs"]
mod fixtures;
use fixtures::{cli, codes, Temp};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn plan_args(temp: &Temp, input: &Value) -> Vec<String> {
    let mut args = vec!["lifecycle".into(), "plan".into()];
    args.extend(temp.write_all(input));
    let out = temp.path().join("plan.json");
    args.extend(["--out".into(), out.to_string_lossy().into_owned()]);
    args
}

/// A blocked plan is still a successful CLI run: exit 0, a fresh 0600 file on
/// disk, `executionAllowed: false` and the six static executor blockers.
fn assert_blocked_persisted(temp: &Temp, args: &[String], want: &[&str]) -> Value {
    assert_blocked_persisted_labeled(temp, args, want, &want.join(","))
}
fn assert_blocked_persisted_labeled(temp: &Temp, args: &[String], want: &[&str], label: &str) -> Value {
    let out = temp.path().join("plan.json");
    let output = cli(args, temp.path());
    eprintln!(
        "[journey-cli] label={label} exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    assert!(
        output.status.success(),
        "blocked plans exit 0 [{label}]: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dump = "/tmp/journey-debug2";
    let _ = std::fs::remove_dir_all(dump);
    let _ = fs::create_dir_all(dump);
    let _ = fs::write(format!("{dump}/stderr.txt"), &output.stderr);
    let mut pairs = args.iter().peekable();
    while let Some(flag) = pairs.next() {
        if let (true, Some(path)) = (flag.starts_with("--") && *flag != "--out", pairs.peek()) {
            let _ = fs::copy(
                path,
                format!(
                    "{dump}/{}-{}",
                    flag.trim_start_matches('-'),
                    Path::new(path).file_name().unwrap().to_string_lossy()
                ),
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let plan: Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(plan["planningStatus"], "blocked");
    assert_eq!(plan["executionAllowed"], false);
    let got = codes(&plan);
    for code in want {
        assert!(got.iter().any(|c| c == code), "want {code} in {got:?}");
    }
    for static_blocker in [
        "EXECUTOR_UNIMPLEMENTED",
        "LIVE_REVALIDATION_REQUIRED",
        "ACCEPTANCE_REQUIRED",
        "EVIDENCE_CONTENT_VERIFICATION_UNIMPLEMENTED",
        "FULL_CAPACITY_ESTIMATE_UNIMPLEMENTED",
        "EFFECTIVE_VALUES_VALIDATION_UNIMPLEMENTED",
    ] {
        let execution = plan["executionBlockers"].as_array().unwrap();
        assert!(
            execution.iter().any(|b| b.to_string().contains(static_blocker)),
            "static blocker {static_blocker} missing"
        );
    }
    plan
}

/// J1 — one release manifest is validated once and then plans two independent
/// clusters; every artifact is a fresh private file and the two plans never mix.
#[test]
fn j1_one_release_is_validated_once_then_plans_two_clusters_independently() {
    let input = fixtures::fixture(true);
    let temp = Temp::new();
    let manifest_path = temp.write_input("release", &input["releaseManifest"]);

    let args = vec![
        "lifecycle".into(),
        "validate-manifest".into(),
        "--manifest".into(),
        manifest_path.clone(),
    ];
    let output = cli(&args, temp.path());
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let verdict: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(verdict["valid"], true);
    assert_eq!(verdict["hasLifecycle"], true);
    assert_eq!(verdict["executionAllowed"], false);

    let mut plans = vec![];
    for (inner, changed) in [(true, "apeiron"), (false, "ontology")] {
        let field = fixtures::fixture(inner);
        let temp = Temp::new();
        let mut args = vec!["lifecycle".into(), "plan".into()];
        args.extend(temp.write_all(&field));
        let out = temp.path().join("plan.json");
        args.extend(["--out".into(), out.to_string_lossy().into_owned()]);
        let output = cli(&args, temp.path());
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let plan: Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
        assert_eq!(plan["planningStatus"], "ready");
        assert_eq!(plan["executionAllowed"], false);
        assert_eq!(plan["changedComponents"], json!([changed]));
        plans.push(plan);
    }
    assert_ne!(plans[0]["planId"], plans[1]["planId"]);
    assert_ne!(plans[0]["stateNamespace"], plans[1]["stateNamespace"]);

    // Both installation records validate against their own observation digest.
    for inner in [true, false] {
        let field = fixtures::fixture(inner);
        let temp = Temp::new();
        let installation = temp.write_input("installation", &field["installation"]);
        let args = vec![
            "lifecycle".into(),
            "validate-record".into(),
            "--installation".into(),
            installation,
        ];
        let output = cli(&args, temp.path());
        assert!(output.status.success());
        let verdict: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(verdict["valid"], true);
        assert_eq!(verdict["liveVerified"], false);
        assert_eq!(
            verdict["recordSha256"], field["observation"]["recordSha256"],
            "the record digest equals the observation's pinned value"
        );
    }
}

/// J2 — field drift yields a blocked but auditable plan file; a schema-illegal
/// input is a hard error that writes nothing. That distinction is the contract.
#[test]
fn j2_field_drift_produces_a_blocked_but_auditable_plan_file() {
    let mutations: &[(&str, Value, &str)] = &[
        (
            "observation",
            json!({"drift": ["manual-image-change"]}),
            "UNEXPLAINED_DRIFT",
        ),
        ("intent", json!({"expectedGeneration": 2}), "STALE_BASELINE"),
        (
            "intent",
            json!({"clusterUid": "outer-cluster"}),
            "CLUSTER_IDENTITY_MISMATCH",
        ),
    ];
    // COMPONENT_STATE_UNKNOWN 在单测（lifecycle/tests.rs）以先补齐字段的 absent
    // 组件覆盖；记录层的资源完整性检查要求 shared-db 的 owner 与 consumer 均
    // installed，夹具内的 apeiron/postgres 都不可翻转状态，故旅程不重复此变异。
    for (field, patch, want) in mutations {
        let mut input = fixtures::fixture(true);
        for (key, value) in patch.as_object().unwrap() {
            if key == "components" {
                for (id, state) in value.as_object().unwrap() {
                    for (k, v) in state.as_object().unwrap() {
                        input["installation"]["components"][id][k] = v.clone();
                    }
                }
            } else {
                input[*field][key] = value.clone();
            }
        }
        fixtures::refresh(&mut input);
        let temp = Temp::new();
        let args = plan_args(&temp, &input);
        let plan = assert_blocked_persisted_labeled(&temp, &args, &[want], want);
        assert_eq!(plan["planningStatus"], "blocked", "{want}");
        let _ = std::fs::write(temp.path().join("want.txt"), want);
    }

    // A schema-illegal input is a hard error: exit != 0 and no output file.
    let mut input = fixtures::fixture(true);
    input["installation"]["unexpectedField"] = json!("denied");
    let temp = Temp::new();
    let mut args = plan_args(&temp, &input);
    let out = temp.path().join("never.json");
    args.extend(["--out".into(), out.to_string_lossy().into_owned()]);
    let output = cli(&args, temp.path());
    assert!(!output.status.success());
    assert!(!out.exists());
}

/// J3 — the upgrade contract is evaluated against the installed field, not the
/// declared target; several independent gates may fire in one blocked plan.
#[test]
fn j3_upgrade_contract_gates_are_evaluated_against_the_installed_field() {
    // Happy upgrade contract: ready with the apeiron migration and protection.
    let temp = Temp::new();
    let input = fixtures::fixture(true);
    let plan: Value = {
        let args = plan_args(&temp, &input);
        let output = cli(&args, temp.path());
        assert!(output.status.success());
        serde_json::from_slice(&fs::read(temp.path().join("plan.json")).unwrap()).unwrap()
    };
    assert_eq!(plan["planningStatus"], "ready");
    let migrations = plan["migrations"].as_array().unwrap();
    assert!(migrations.iter().any(|m| m["component"] == "apeiron"));

    // Gate 1: the upgrade path must uniquely cover the field's component set.
    let mut input = fixtures::fixture(true);
    input["releaseManifest"]["lifecycle"]["upgradePaths"][0]["componentSets"] = json!([["apeiron"]]);
    fixtures::refresh(&mut input);
    let temp = Temp::new();
    assert_blocked_persisted(
        &temp,
        &plan_args(&temp, &input),
        &["UPGRADE_PATH_NOT_UNIQUELY_VERIFIED"],
    );

    // Gate 2: a migration prerequisite that the field does not satisfy.
    let mut input = fixtures::fixture(true);
    input["releaseManifest"]["lifecycle"]["migrationRules"]["apeiron"]["requiredComponents"]["postgres"]
        ["dataSchema"] = json!("0");
    fixtures::refresh(&mut input);
    let temp = Temp::new();
    assert_blocked_persisted(&temp, &plan_args(&temp, &input), &["MIGRATION_PREREQUISITE_MISMATCH"]);

    // Gate 3: the target foundation digest moved under the installed field.
    let mut input = fixtures::fixture(true);
    for artifact in input["releaseManifest"]["artifacts"].as_array_mut().unwrap() {
        if artifact["id"] == "charts/postgres.tgz" {
            artifact["sha256"] = json!(fixtures::hash('7'));
        }
    }
    fixtures::refresh(&mut input);
    let temp = Temp::new();
    assert_blocked_persisted(&temp, &plan_args(&temp, &input), &["FOUNDATION_CHANGE_UNSUPPORTED"]);

    // Gate 4: a brand-new dependency in the target contract.
    let mut input = fixtures::fixture(true);
    input["releaseManifest"]["components"]["apeiron"]["requires"] = json!(["postgres", "ontology"]);
    fixtures::refresh(&mut input);
    let temp = Temp::new();
    assert_blocked_persisted(
        &temp,
        &plan_args(&temp, &input),
        &["NEW_DEPENDENCY_REQUIRES_SEPARATE_CHANGE"],
    );

    // Gate 5: selecting an uninstalled component reports both the missing
    // component and the rejected set change together.
    let mut input = fixtures::fixture(true);
    input["intent"]["selectedComponents"] = json!(["ontology", "postgres"]);
    fixtures::refresh(&mut input);
    let temp = Temp::new();
    assert_blocked_persisted(
        &temp,
        &plan_args(&temp, &input),
        &["COMPONENT_NOT_INSTALLED", "COMPONENT_SET_CHANGE_UNSUPPORTED"],
    );
}

/// J4 — standalone backup binds the source release contract and a verified
/// adapter; every deviation is a named blocker and execution stays shut.
#[test]
fn j4_standalone_backup_binds_the_source_release_and_a_verified_adapter() {
    let mut input = fixtures::fixture(true);
    fixtures::source_release(&mut input);
    input["intent"]["operation"] = json!("backup");
    let temp = Temp::new();
    let plan: Value = {
        let args = plan_args(&temp, &input);
        let output = cli(&args, temp.path());
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&fs::read(temp.path().join("plan.json")).unwrap()).unwrap()
    };
    assert!(
        plan["planningStatus"] == "ready" || plan["planningStatus"] == "blocked",
        "backup planning must terminate cleanly"
    );
    assert_eq!(plan["executionAllowed"], false);
    assert_eq!(plan["changedComponents"], json!([]));
    assert_eq!(
        plan["targetRelease"]["version"],
        input["installation"]["release"]["version"]
    );
    let resources = plan["protection"]["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 1, "shared data is deduplicated to its owner");
    assert_eq!(resources[0]["owner"], "postgres");

    // The backup manifest must be the source contract, not the upgrade target.
    let mutated = input.clone();
    let temp = Temp::new();
    let wrong = temp.write_input("target-manifest", &fixtures::fixture(true)["releaseManifest"]);
    let mut args = plan_args(&temp, &mutated);
    let pos = args.iter().position(|a| a == "--manifest").unwrap();
    args[pos + 1] = wrong;
    assert_blocked_persisted(&temp, &args, &["BACKUP_SOURCE_CONTRACT_MISMATCH"]);

    // Adapter verified → declared is rejected.
    let mut mutated = input.clone();
    mutated["releaseManifest"]["lifecycle"]["backupAdapters"]["pg-logical"]["status"] = json!("declared");
    fixtures::refresh(&mut mutated);
    let temp = Temp::new();
    assert_blocked_persisted(
        &temp,
        &plan_args(&temp, &mutated),
        &["BACKUP_RESTORE_ADAPTER_UNVERIFIED"],
    );

    // Capacity below the declared estimate is rejected.
    let mut mutated = input.clone();
    mutated["intent"]["protection"]["capacityBytes"] = json!(1);
    fixtures::refresh(&mut mutated);
    let temp = Temp::new();
    assert_blocked_persisted(&temp, &plan_args(&temp, &mutated), &["BACKUP_CAPACITY_INSUFFICIENT"]);

    // No protection policy at all is rejected.
    let mut mutated = input.clone();
    mutated["intent"]["protection"] = Value::Null;
    fixtures::refresh(&mut mutated);
    let temp = Temp::new();
    assert_blocked_persisted(&temp, &plan_args(&temp, &mutated), &["PROTECTION_POLICY_REQUIRED"]);

    // The backup execution door stays shut and writes nothing.
    let temp = Temp::new();
    let _ = temp.write_all(&input);
    let output = cli(&["lifecycle".into(), "backup".into()], temp.path());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("阻断"));
    assert!(!temp.path().join("unexpected-command").exists());
}

/// J5 — application restore honors expansion lineage: accepted agents are
/// preserved, the control plane is never replayed, and a pre-expansion backup
/// cannot silently revert the accepted baseline.
#[test]
fn j5_restore_honors_expansion_lineage_and_never_reverts_nodes() {
    let mut input = fixtures::fixture(true);
    fixtures::source_release(&mut input);
    let gen1_backup = fixtures::backup_of(&input);
    fixtures::accept_expansion(&mut input, &["agent-1", "agent-2", "agent-3", "agent-4", "agent-5"], 2);
    let temp = Temp::new();
    let manifest_path = temp.write_input("releaseManifest", &input["releaseManifest"]);
    fixtures::bind_manifest_to_file(&mut input, Path::new(&manifest_path));
    let gen2_backup = fixtures::backup_of(&input);
    input["intent"]["operation"] = json!("application-restore");

    let backup_path = temp.write_input("backup", &gen2_backup);
    let mut args = plan_args(&temp, &input);
    args.extend(["--backup".into(), backup_path]);
    let output = cli(&args, temp.path());
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let plan: Value = serde_json::from_slice(&fs::read(temp.path().join("plan.json")).unwrap()).unwrap();
    if plan["planningStatus"] != "ready" {
        println!("J5 first-step blockers: {}", plan["blockers"]);
        println!("J5 record.release={}", input["installation"]["release"]);
        println!("J5 input.releaseManifestSha256={}", input["releaseManifestSha256"]);
        println!("J5 backup.sourceRelease={}", gen2_backup["sourceRelease"]);
        let dump = "/tmp/j5-debug";
        let _ = fs::create_dir_all(dump);
        let mut pairs = args.iter().peekable();
        while let Some(flag) = pairs.next() {
            if let (true, Some(path)) = (flag.starts_with("--") && *flag != "--out", pairs.peek()) {
                let _ = fs::copy(
                    path,
                    format!(
                        "{dump}/{}-{}",
                        flag.trim_start_matches('-'),
                        Path::new(path).file_name().unwrap().to_string_lossy()
                    ),
                );
            }
        }
        let _ = fs::write(format!("{dump}/args.txt"), format!("{args:?}"));
    }
    assert_eq!(plan["planningStatus"], "ready");
    assert_eq!(plan["restore"]["restoreControlPlane"], false);
    assert_eq!(plan["restore"]["removeNodes"], json!([]));
    assert_eq!(plan["restore"]["preserveNodes"].as_array().unwrap().len(), 8);
    assert_eq!(
        plan["protection"]["requiredVerificationLevel"],
        "isolated-restore-verified"
    );

    // A backup that predates the accepted expansion is blocked, not applied.
    let temp = Temp::new();
    let stale = temp.write_input("backup", &gen1_backup);
    let mut args = plan_args(&temp, &input);
    args.extend(["--backup".into(), stale]);
    assert_blocked_persisted(&temp, &args, &["BACKUP_PREDATES_ACCEPTED_BASELINE"]);

    // A backup from another cluster is rejected.
    let mut foreign = gen2_backup.clone();
    foreign["clusterUid"] = json!("outer-cluster");
    let temp = Temp::new();
    let foreign_path = temp.write_input("backup", &foreign);
    let mut args = plan_args(&temp, &input);
    args.extend(["--backup".into(), foreign_path]);
    assert_blocked_persisted(&temp, &args, &["BACKUP_CLUSTER_MISMATCH"]);

    // A tampered data mapping is rejected.
    let mut tampered = gen2_backup.clone();
    tampered["resourceIdentities"]["shared-db"] = json!(fixtures::hash('8'));
    let temp = Temp::new();
    let tampered_path = temp.write_input("backup", &tampered);
    let mut args = plan_args(&temp, &input);
    args.extend(["--backup".into(), tampered_path]);
    assert_blocked_persisted(&temp, &args, &["BACKUP_DATA_MAPPING_MISMATCH"]);

    // An unverified backup level is rejected.
    let mut unverified = gen2_backup.clone();
    unverified["level"] = json!("integrity-verified");
    let temp = Temp::new();
    let unverified_path = temp.write_input("backup", &unverified);
    let mut args = plan_args(&temp, &input);
    args.extend(["--backup".into(), unverified_path]);
    assert_blocked_persisted(&temp, &args, &["BACKUP_RESTORE_NOT_VERIFIED"]);
}

/// J6 — expansion is planned as its own stage chain and every execution door
/// stays shut without touching the filesystem.
#[test]
fn j6_expansion_is_a_separate_stage_and_every_execution_door_stays_shut() {
    let mut input = fixtures::fixture(true);
    input["intent"]["addAgents"] = json!(["agent-1", "agent-2", "agent-3", "agent-4", "agent-5"]);
    let temp = Temp::new();
    let plan = assert_blocked_persisted(
        &temp,
        &plan_args(&temp, &input),
        &["EXPANSION_EXECUTOR_UNIMPLEMENTED", "POST_EXPANSION_BASELINE_REQUIRED"],
    );
    let stages = plan["stages"].as_array().unwrap();
    assert_eq!(stages[0]["operation"], "expand");
    assert_eq!(stages[0]["changeServerMembership"], false);
    assert!(!stages[0]["acceptanceChecks"].as_array().unwrap().is_empty());
    assert_eq!(stages[1]["operation"], "commit-baseline");
    assert_eq!(stages[1]["requiresFreshApplicationBackup"], true);

    // Every execution subcommand fails without writing anything, forwarding to
    // a plugin, or disturbing the blocked plan already on disk.
    for operation in ["upgrade", "expand", "backup", "restore", "execute"] {
        let output = cli(&["lifecycle".into(), operation.into()], temp.path());
        assert!(!output.status.success(), "{operation} must refuse");
        assert!(String::from_utf8_lossy(&output.stderr).contains("阻断"));
    }
    assert!(!temp.path().join("unexpected-command").exists());
}

// ---- J7/J8: real release artifacts and the legacy-line generation gap ----
//
// The two pinned files under tests/data are extracted release artifacts, not
// synthetic fixtures: manifest-20261016.json is the shipped 20261016.1 manifest
// (legacy, without a lifecycle block) and manifest-20261020.json is the 20261020
// candidate (release 0.1.0-dev.20261017.1, extracted from manifest-candidate40)
// with its conservative lifecycle contract. They contain artifact digests and
// public image references only — no passwords, keys or site credentials. Every
// journey re-verifies the pinned bytes before planning against them.

const SOURCE_MANIFEST_SHA256: &str =
    "6462e76c3baf4f474919020d455722a6f494444b3f6b737b65c69572dba948c4";
const TARGET_MANIFEST_SHA256: &str =
    "d60e618f297d0927e1a91510e9be9f6728d6c083a0e013e81ced69ef40bc45c6";
/// The site record pins exactly this compact-JSON identity of the source manifest.
const SOURCE_MANIFEST_IDENTITY: &str =
    "d5d6efdd365e566b54b7b60bea1c6d0f2ec9403c5875cd5bafab044efcc4b091";

fn pinned_manifest(name: &str, sha256: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    let bytes = fs::read(&path).unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        sha256,
        "{name} must stay byte-identical to the extracted release artifact"
    );
    serde_json::from_slice(&bytes).unwrap()
}

fn file_digest(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

/// Every blocker must tell the operator who owns it, what to do, and that the
/// guidance itself never unlocks execution.
fn assert_remediation(plan: &Value) {
    for blocker in plan["blockers"].as_array().unwrap() {
        let remediation = &blocker["remediation"];
        assert!(
            remediation["owner"].as_str().is_some_and(|s| !s.is_empty())
                && remediation["action"].as_str().is_some_and(|s| !s.is_empty()),
            "blocker {} needs an owner and an action: {remediation}",
            blocker["code"]
        );
        assert_eq!(remediation["allowsExecution"], false, "{}", blocker["code"]);
    }
}

/// The installed field mirrors the real record captured on site: every source
/// component is installed with the source manifest's own artifact digests, and
/// the release identity is the compact-JSON identity of that manifest.
fn real_pair_fixture() -> (Value, Value, Value) {
    let source = pinned_manifest("manifest-20261016.json", SOURCE_MANIFEST_SHA256);
    let target = pinned_manifest("manifest-20261020.json", TARGET_MANIFEST_SHA256);
    assert_eq!(fixtures::identity(&source), SOURCE_MANIFEST_IDENTITY);
    let ids: Vec<String> =
        source["components"].as_object().unwrap().keys().cloned().collect();
    let mut components = json!({});
    for id in &ids {
        components[id] = json!({"status":"installed","version":"1.0.0",
            "state":{"artifactDigests":fixtures::target_artifacts(&source, id),
                "configurationSchema":"1","dataSchema":"1"},
            "configurationSha256":fixtures::hash('c'),"dataBindings":{}});
    }
    let record = json!({"schemaVersion":1,"kind":"chentu-installation-record",
        "installationId":"legacy-install","clusterUid":"legacy-cluster","generation":1,
        "release":{"version":source["release"],"manifestSha256":fixtures::identity(&source)},
        "topology":"single-k3s","architecture":"amd64","nodes":[{"id":"server-1","role":"server"}],
        "configurationSha256":fixtures::hash('c'),"foundationSha256":fixtures::hash('f'),
        "components":components,"resources":{},
        "baselineEvidence":[fixtures::hash('e')],"acceptedExpansions":[]});
    let mut states = json!({});
    for (id, component) in record["components"].as_object().unwrap() {
        states[id] = component["state"].clone();
    }
    let observation = json!({"schemaVersion":1,"installationId":"legacy-install",
        "clusterUid":"legacy-cluster","generation":1,"recordSha256":fixtures::identity(&record),
        "configurationSha256":fixtures::hash('c'),"foundationSha256":fixtures::hash('f'),
        "nodes":record["nodes"],"components":states,"healthy":true,"drift":[],"network":"offline"});
    let intent = json!({"schemaVersion":1,"operation":"application-upgrade",
        "installationId":"legacy-install","clusterUid":"legacy-cluster","expectedGeneration":1,
        "selectedComponents":ids,"addAgents":[],
        "protection":{"owner":"operator","rpoSeconds":0,"rtoSeconds":3600,"downtimeSeconds":1800,
            "independentCopyRef":"external/offline-copy","recipientFingerprint":"age-recipient-ref",
            "capacityBytes":2048}});
    (
        json!({"installation":record,"observation":observation,"intent":intent}),
        source,
        target,
    )
}

/// J7 — the real version pair plans as blocked: the 20261020 candidate ships a
/// conservative contract (no accepted upgrade path, unknown target schemas), so
/// the plan names every reason per component with an owner, keeps execution
/// shut, and still lands on disk as a private reviewable file.
#[test]
fn j7_real_version_pair_plans_blocked_with_owner_per_blocker() {
    let (input, source, target) = real_pair_fixture();

    // Both real manifests validate; only the candidate carries lifecycle metadata.
    for (name, sha, has_lifecycle) in [
        ("manifest-20261016.json", SOURCE_MANIFEST_SHA256, false),
        ("manifest-20261020.json", TARGET_MANIFEST_SHA256, true),
    ] {
        let temp = Temp::new();
        let path = temp.path().join("manifest.json");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name),
            &path,
        )
        .unwrap();
        assert_eq!(file_digest(&path), sha, "{name}");
        let output = cli(
            &["lifecycle".into(), "validate-manifest".into(),
              "--manifest".into(), path.to_string_lossy().into_owned()],
            temp.path(),
        );
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let verdict: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(verdict["valid"], true, "{name}");
        assert_eq!(verdict["hasLifecycle"], has_lifecycle, "{name}");
        assert_eq!(verdict["executionAllowed"], false, "{name}");
    }

    // Plan the pair through the pinned candidate bytes, not a re-serialization.
    let temp = Temp::new();
    let target_path = temp.path().join("target-manifest.json");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/manifest-20261020.json"),
        &target_path,
    )
    .unwrap();
    let mut args = vec!["lifecycle".into(), "plan".into()];
    for (field, flag) in [
        ("installation", "--installation"),
        ("observation", "--observation"),
        ("intent", "--intent"),
    ] {
        args.push(flag.into());
        args.push(temp.write_input(field, &input[field]));
    }
    args.extend([
        "--manifest".into(),
        target_path.to_string_lossy().into_owned(),
        "--out".into(),
        temp.path().join("plan.json").to_string_lossy().into_owned(),
    ]);
    let plan = assert_blocked_persisted_labeled(
        &temp,
        &args,
        &["UPGRADE_PATH_NOT_UNIQUELY_VERIFIED", "TARGET_SCHEMA_UNKNOWN"],
        "real version pair",
    );
    assert_remediation(&plan);
    assert_eq!(plan["sourceRelease"]["version"], source["release"]);
    assert_eq!(plan["targetRelease"]["version"], target["release"]);
    assert_eq!(plan["targetRelease"]["manifestSha256"], TARGET_MANIFEST_SHA256);
    assert_eq!(plan["clusterUid"], "legacy-cluster");
}

/// The checkpoint exactly as the 20261016 installer wrote it
/// (runtime/src/app/resume.rs): a resume structure with config and tool
/// bindings, but no installationId, generation or kind. All values synthetic.
fn sanitized_old_resume(configuration: &Value) -> Value {
    json!({"schemaVersion":1,
        "configPath":"/opt/legacy/installation/config.json",
        "configHash":fixtures::identity(configuration),
        "directory":"/opt/legacy/installation/run-0000000000000001",
        "target":{"root":"/opt/legacy/source",
            "environment":"/opt/legacy/installation/environment.yaml",
            "kubeconfig":"/opt/legacy/installation/kubeconfig","runner":"native"},
        "environment":{"CHENTU_ENV":"/opt/legacy/installation/environment.yaml",
            "KUBECONFIG":"/opt/legacy/installation/kubeconfig"},
        "files":{"/opt/legacy/resources/values.yaml":fixtures::hash('1')},
        "plan":{"files":[{"path":"values.yaml","size":1024,"sha256":fixtures::hash('1'),
            "url":"https://example.internal/resources/values.yaml"}]},
        "bundle":"/opt/legacy/bundle","clusterUid":"legacy-uid","ownerPid":12345,
        "commandActive":false,"checkpoint":"bootstrap-complete","cliSha256":fixtures::hash('y')})
}

/// The collect() output shape for one single-k3s server; the digests bind the
/// exact bytes of the manifest, legacy and configuration files that migrate
/// later re-verifies.
fn legacy_collection(manifest_sha: &str, legacy_sha: &str, configuration_sha: &str, configuration: &Value) -> Value {
    json!({"schemaVersion":1,"kind":"chentu-legacy-collection","readOnly":true,
        "executionAllowed":false,
        "startedAt":"2026-10-21T00:00:00+00:00","completedAt":"2026-10-21T00:00:00+00:00",
        "clusterUid":"legacy-uid","topology":"single-k3s","network":"offline",
        "manifestSha256":manifest_sha,"legacySha256":legacy_sha,
        "configurationSha256":configuration_sha,
        "resumeConfigurationSha256":fixtures::identity(configuration),
        "nodes":[{"id":"server-1","uid":"legacy-node-uid","role":"server",
            "architecture":"amd64","kubeletVersion":"v1.36.3+k3s1","ready":true}],
        "pods":[],"volumes":[],"atomicSnapshot":false,
        "coverage":["cluster-identity","nodes","runtime-images","pvc-metadata"],
        "unverified":["component-artifacts-and-schemas","effective-values","data-ownership",
            "foundation-configuration","installation-completion","application-health",
            "baseline-acceptance"]})
}

/// J8 — a legacy-line checkpoint is evidence to collect, never a v1 record: the
/// CLI rejects it as installation input without writing anything, migration
/// wraps it into an unaccepted candidate that preserves the checkpoint, and the
/// blocked plan for that candidate names an owner for every reason.
#[test]
fn j8_legacy_checkpoint_is_rejected_then_migrated_into_an_unaccepted_candidate() {
    let (input, source, _) = real_pair_fixture();
    let configuration = json!({"topology":"single-k3s","network":"offline"});
    let resume = sanitized_old_resume(&configuration);
    let entries = |temp: &Temp| {
        // poison-bin is created by the cli() harness itself, not by the CLI run.
        let mut names: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != "poison-bin")
            .collect();
        names.sort();
        names
    };

    // A: the resume shape cannot pass as a v1 installation record (it carries no
    // installationId, generation or kind) and validation writes nothing.
    let temp = Temp::new();
    let resume_path = temp.write_input("resume", &resume);
    let before = entries(&temp);
    let output = cli(
        &[
            "lifecycle".into(),
            "validate-record".into(),
            "--installation".into(),
            resume_path,
        ],
        temp.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("v1"));
    assert_eq!(entries(&temp), before, "a rejected record writes nothing");

    // B: migration binds the same checkpoint to a collected snapshot and always
    // produces a review candidate, never an accepted baseline.
    let temp = Temp::new();
    let manifest_path = temp.write_input("source-manifest", &source);
    let legacy_path = temp.write_input("legacy", &resume);
    let configuration_path = temp.write_input("configuration", &configuration);
    let collection = temp.write_input(
        "collection",
        &legacy_collection(
            &file_digest(Path::new(&manifest_path)),
            &file_digest(Path::new(&legacy_path)),
            &file_digest(Path::new(&configuration_path)),
            &configuration,
        ),
    );
    let migration_path = temp.path().join("migration.json");
    let output = cli(
        &[
            "lifecycle".into(),
            "migrate-record".into(),
            "--collection".into(),
            collection,
            "--manifest".into(),
            manifest_path,
            "--legacy".into(),
            legacy_path,
            "--configuration".into(),
            configuration_path,
            "--installation-id".into(),
            "legacy-one".into(),
            "--out".into(),
            migration_path.to_string_lossy().into_owned(),
        ],
        temp.path(),
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let migration: Value = serde_json::from_slice(&fs::read(&migration_path).unwrap()).unwrap();
    assert_eq!(migration["status"], "review-required");
    assert_eq!(migration["liveVerified"], false);
    assert_eq!(migration["executionAllowed"], false);
    assert_eq!(migration["preserveLegacyCheckpoint"], true);
    assert_eq!(migration["legacyClusterUidPresent"], true);
    assert_eq!(
        migration["installationCandidate"]["release"]["version"],
        source["release"],
        "the candidate keeps the installed legacy release identity"
    );
    for field in ["owner", "action"] {
        assert!(
            migration["remediation"][field].as_str().is_some_and(|s| !s.is_empty()),
            "migration guidance needs {field}"
        );
    }

    // C: planning the migrated candidate is blocked with an owner per blocker.
    let candidate = json!({
        "installation": migration["installationCandidate"],
        "observation": migration["observationCandidate"],
        "intent": {"schemaVersion":1,"operation":"application-upgrade",
            "installationId":"legacy-one","clusterUid":"legacy-uid","expectedGeneration":1,
            "selectedComponents":input["intent"]["selectedComponents"],"addAgents":[],
            "protection":input["intent"]["protection"]},
    });
    let temp = Temp::new();
    let mut args = vec!["lifecycle".into(), "plan".into()];
    for (field, flag) in [
        ("installation", "--installation"),
        ("observation", "--observation"),
        ("intent", "--intent"),
    ] {
        args.push(flag.into());
        args.push(temp.write_input(field, &candidate[field]));
    }
    args.extend([
        "--manifest".into(),
        temp.write_input("source-manifest", &source),
        "--out".into(),
        temp.path().join("plan.json").to_string_lossy().into_owned(),
    ]);
    let plan = assert_blocked_persisted_labeled(
        &temp,
        &args,
        &["LIFECYCLE_CONTRACT_MISSING", "BASELINE_UNVERIFIED"],
        "legacy candidate",
    );
    assert_remediation(&plan);
}
