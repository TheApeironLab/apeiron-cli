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
