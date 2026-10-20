use super::*;

#[path = "../../../tests/support/lifecycle_fixture.rs"]
mod fixtures;
use fixtures::{fixture, hash};

fn refresh(input: &mut Value) {
    input["observation"]["recordSha256"] = json!(identity(&input["installation"]));
}
fn codes(plan: &Value) -> Vec<&str> {
    plan["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["code"].as_str().unwrap())
        .collect()
}
// Restore inputs must describe the actual source package, not the upgrade target.
fn source_contract(input: &mut Value) {
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
fn backup(input: &Value) -> Value {
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
        states[id] = json!({"artifactDigests":target_artifacts(manifest,id).unwrap(),"configurationSchema":c["configurationSchema"],"dataSchema":c["dataSchema"]});
    }
    json!({"schemaVersion":1,"backupId":"backup-1","installationId":record["installationId"],"clusterUid":record["clusterUid"],
        "generation":record["generation"],"foundationSha256":record["foundationSha256"],"topology":record["topology"],"nodes":record["nodes"],
        "componentIds":input["intent"]["selectedComponents"],"resourceIds":["shared-db"],"level":"isolated-restore-verified",
        "manifestSha256":hash('d'),"independentCopyRef":"external/offline-copy","recipientFingerprint":"age-recipient-ref","restoreEvidence":[hash('e')],
        "sourceRelease":record["release"],"componentStates":states,"resourceIdentities":{"shared-db":identity(&record["resources"]["shared-db"])},"configurationSha256":record["configurationSha256"]})
}

#[test]
fn same_release_plans_independent_clusters_and_preserves_selection() {
    let inner = plan(&fixture(true)).unwrap();
    let outer = plan(&fixture(false)).unwrap();
    assert_eq!(inner["planningStatus"], "ready");
    assert_eq!(outer["planningStatus"], "ready");
    assert_eq!(inner["changedComponents"], json!(["apeiron"]));
    assert_eq!(outer["changedComponents"], json!(["ontology"]));
    assert_ne!(inner["planId"], outer["planId"]);
    assert_ne!(inner["stateNamespace"], outer["stateNamespace"]);
    assert_eq!(inner["configurationSha256"], hash('c'));
    assert_eq!(outer["configurationSha256"], hash('d'));
    assert_ne!(inner["configurationSha256"], outer["configurationSha256"]);
    assert_eq!(inner["executionAllowed"], false);
    assert_eq!(inner["network"], "offline");
    assert_eq!(inner["protection"]["resources"].as_array().unwrap().len(), 1);
    assert_eq!(inner["protection"]["resources"][0]["owner"], "postgres");
    assert_eq!(plan(&fixture(true)).unwrap(), inner);
}
#[test]
fn expansion_is_a_separate_blocked_stage_and_requires_new_baseline() {
    let mut input = fixture(true);
    input["intent"]["addAgents"] = json!(["agent-1", "agent-2", "agent-3", "agent-4", "agent-5"]);
    let result = plan(&input).unwrap();
    assert!(codes(&result).contains(&"EXPANSION_EXECUTOR_UNIMPLEMENTED"));
    assert!(codes(&result).contains(&"POST_EXPANSION_BASELINE_REQUIRED"));
    assert_eq!(result["stages"][0]["existingNodes"].as_array().unwrap().len(), 3);
    assert_eq!(result["stages"][0]["addAgents"].as_array().unwrap().len(), 5);
    assert_eq!(result["stages"][0]["changeServerMembership"], false);
    assert_eq!(result["stages"][1]["requiresFreshApplicationBackup"], true);
}
#[test]
fn application_restore_preserves_accepted_agents_and_rejects_pre_expansion_backup() {
    let mut input = fixture(true);
    source_contract(&mut input);
    let before = backup(&input);
    let record = &mut input["installation"];
    for n in 1..=5 {
        record["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":format!("agent-{n}"),"role":"agent"}));
    }
    record["generation"] = json!(2);
    record["foundationSha256"] = json!(hash('9'));
    record["acceptedExpansions"] = json!([{"id":"expand-1","generation":2,"nodeIds":["agent-1","agent-2","agent-3","agent-4","agent-5"],"evidence":[hash('e')]}]);
    input["observation"]["nodes"] = input["installation"]["nodes"].clone();
    input["observation"]["generation"] = json!(2);
    input["observation"]["foundationSha256"] = json!(hash('9'));
    input["intent"]["expectedGeneration"] = json!(2);
    input["intent"]["operation"] = json!("application-restore");
    refresh(&mut input);
    source_contract(&mut input);
    input["backupSet"] = backup(&input);
    let result = plan(&input).unwrap();
    assert_eq!(result["planningStatus"], "ready");
    assert_eq!(result["restore"]["restoreControlPlane"], false);
    assert_eq!(result["restore"]["removeNodes"], json!([]));
    assert_eq!(result["restore"]["preserveNodes"].as_array().unwrap().len(), 8);
    input["backupSet"] = before;
    assert!(codes(&plan(&input).unwrap()).contains(&"BACKUP_PREDATES_ACCEPTED_BASELINE"));
}
#[test]
fn cross_cluster_backup_is_rejected_even_with_same_release() {
    let mut inner = fixture(true);
    inner["intent"]["operation"] = json!("application-restore");
    inner["backupSet"] = backup(&fixture(false));
    assert!(codes(&plan(&inner).unwrap()).contains(&"BACKUP_CLUSTER_MISMATCH"));
}
#[test]
fn unknown_absent_and_unselected_catalog_components_are_distinct() {
    let mut input = fixture(true);
    let mut absent = input["installation"]["components"]["apeiron"].clone();
    absent["status"] = json!("absent");
    absent["dataBindings"] = json!({});
    input["installation"]["components"]["ontology"] = absent;
    refresh(&mut input);
    assert_eq!(plan(&input).unwrap()["planningStatus"], "ready");
    input["installation"]["components"]["ontology"]["status"] = json!("unknown");
    refresh(&mut input);
    assert!(codes(&plan(&input).unwrap()).contains(&"COMPONENT_STATE_UNKNOWN"));
}
#[test]
fn mismatched_context_record_generation_or_component_state_blocks() {
    for (path, replacement, code) in [
        (
            "/intent/clusterUid",
            json!("outer-cluster"),
            "CLUSTER_IDENTITY_MISMATCH",
        ),
        ("/intent/expectedGeneration", json!(2), "STALE_BASELINE"),
        (
            "/observation/recordSha256",
            json!(hash('d')),
            "RECORD_IDENTITY_MISMATCH",
        ),
        ("/observation/healthy", json!(false), "CLUSTER_UNHEALTHY"),
        (
            "/observation/drift",
            json!(["manual-image-change"]),
            "UNEXPLAINED_DRIFT",
        ),
        ("/observation/foundationSha256", json!(hash('8')), "BASELINE_DRIFT"),
        (
            "/observation/components/apeiron/dataSchema",
            json!("99"),
            "COMPONENT_DRIFT",
        ),
    ] {
        let mut input = fixture(true);
        *input.pointer_mut(path).unwrap() = replacement;
        assert!(codes(&plan(&input).unwrap()).contains(&code), "{code}");
    }
}
#[test]
fn compatibility_requires_whole_combination_and_exact_migration_prerequisites() {
    let mut input = fixture(true);
    input["releaseManifest"]["lifecycle"]["upgradePaths"][0]["componentSets"] = json!([["apeiron"]]);
    assert!(codes(&plan(&input).unwrap()).contains(&"UPGRADE_PATH_NOT_UNIQUELY_VERIFIED"));
    let mut input = fixture(true);
    input["releaseManifest"]["lifecycle"]["migrationRules"]["apeiron"]["requiredComponents"]["postgres"]
        ["dataSchema"] = json!("0");
    assert!(codes(&plan(&input).unwrap()).contains(&"MIGRATION_PREREQUISITE_MISMATCH"));
}
#[test]
fn new_dependency_and_foundation_changes_are_blocked_before_execution() {
    let mut input = fixture(true);
    input["releaseManifest"]["components"]["apeiron"]["requires"] = json!(["postgres", "ontology"]);
    assert!(codes(&plan(&input).unwrap()).contains(&"NEW_DEPENDENCY_REQUIRES_SEPARATE_CHANGE"));
    let mut input = fixture(true);
    input["releaseManifest"]["artifacts"][2]["sha256"] = json!(hash('8'));
    assert!(codes(&plan(&input).unwrap()).contains(&"FOUNDATION_CHANGE_UNSUPPORTED"));
}
#[test]
fn backup_needs_verified_adapter_binding_capacity_and_policy() {
    for (path, replacement, code) in [
        (
            "/releaseManifest/lifecycle/backupAdapters/pg-logical/status",
            json!("declared"),
            "BACKUP_RESTORE_ADAPTER_UNVERIFIED",
        ),
        (
            "/installation/components/apeiron/dataBindings",
            json!({}),
            "DATA_BINDING_MISSING",
        ),
        (
            "/installation/resources/shared-db/estimatedBytes",
            Value::Null,
            "CAPACITY_ESTIMATE_INCOMPLETE",
        ),
        (
            "/intent/protection/capacityBytes",
            json!(1),
            "BACKUP_CAPACITY_INSUFFICIENT",
        ),
        ("/intent/protection", Value::Null, "PROTECTION_POLICY_REQUIRED"),
    ] {
        let mut input = fixture(true);
        *input.pointer_mut(path).unwrap() = replacement;
        refresh(&mut input);
        assert!(codes(&plan(&input).unwrap()).contains(&code), "{code}");
    }
}
#[test]
fn shared_service_restore_cannot_overwrite_other_consumers() {
    let mut input = fixture(true);
    input["installation"]["components"]["ontology"] = input["installation"]["components"]["apeiron"].clone();
    input["observation"]["components"]["ontology"] = input["installation"]["components"]["ontology"]["state"].clone();
    input["installation"]["resources"]["shared-db"]["consumers"] = json!(["apeiron", "ontology"]);
    input["installation"]["resources"]["shared-db"]["restoreGranularity"] = json!("service");
    input["intent"]["operation"] = json!("application-restore");
    refresh(&mut input);
    source_contract(&mut input);
    input["backupSet"] = backup(&input);
    assert!(codes(&plan(&input).unwrap()).contains(&"SHARED_RESTORE_SCOPE_EXCEEDS_SELECTION"));
}
#[test]
fn strict_contracts_reject_site_fields_duplicate_nodes_and_evidenceless_capabilities() {
    for mutation in 0..6 {
        let mut input = fixture(true);
        match mutation {
            0 => input["releaseManifest"]["lifecycle"]["clusterUid"] = json!("inner-cluster"),
            1 => input["installation"]["nodes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"server-1","role":"server"})),
            2 => input["releaseManifest"]["lifecycle"]["backupAdapters"]["pg-logical"]["evidence"] = json!([]),
            3 => input["installation"]["secret"] = json!("do-not-echo"),
            4 => input["installation"]["clusterUid"] = json!("cluster/../../other"),
            _ => input["intent"]["addAgents"] = json!(["path/agent"]),
        }
        let error = plan(&input).unwrap_err();
        assert!(!error.message.contains("do-not-echo"));
    }
}
#[test]
fn standalone_backup_uses_source_contract_without_upgrade_target() {
    let mut input = fixture(true);
    input["intent"]["operation"] = json!("backup");
    for id in ["apeiron", "postgres"] {
        input["releaseManifest"]["lifecycle"]["components"][id]["dataSchema"] = json!("1");
    }
    input["releaseManifest"]["lifecycle"]["migrationRules"] = json!({});
    input["releaseManifest"]["lifecycle"]["upgradePaths"] = json!([]);
    input["releaseManifest"]["artifacts"][0]["sha256"] = json!(hash('1'));
    input["releaseManifestSha256"] = json!(identity(&input["releaseManifest"]));
    input["installation"]["release"]["manifestSha256"] = input["releaseManifestSha256"].clone();
    refresh(&mut input);
    let result = plan(&input).unwrap();
    assert_eq!(result["planningStatus"], "ready");
    assert_eq!(result["changedComponents"], json!([]));
    assert_eq!(result["executionAllowed"], false);
}
#[test]
fn unsupported_execution_never_forwards_to_plugins_or_subprocesses() {
    for op in ["upgrade", "expand", "backup", "restore", "execute"] {
        assert_eq!(run(&[op.to_owned()]).unwrap_err().code, 409);
    }
}
#[test]
fn no_plan_contains_locator_refs_or_secret_values() {
    let mut input = fixture(true);
    input["installation"]["resources"]["shared-db"]["locatorRef"] = json!("private-resource-location");
    refresh(&mut input);
    let result = plan(&input).unwrap().to_string();
    assert!(!result.contains("private-resource-location"));
}

#[test]
fn restore_requires_isolation_and_exact_resource_mapping() {
    let mut input = fixture(true);
    input["intent"]["operation"] = json!("application-restore");
    source_contract(&mut input);
    input["backupSet"] = backup(&input);
    input["backupSet"]["level"] = json!("integrity-verified");
    assert!(codes(&plan(&input).unwrap()).contains(&"BACKUP_RESTORE_NOT_VERIFIED"));
    input["backupSet"]["level"] = json!("isolated-restore-verified");
    input["backupSet"]["resourceIdentities"]["shared-db"] = json!(hash('8'));
    assert!(codes(&plan(&input).unwrap()).contains(&"BACKUP_DATA_MAPPING_MISMATCH"));
}
#[test]
fn irreversible_migration_requires_current_backup_restore_drill() {
    let mut input = fixture(true);
    input["releaseManifest"]["lifecycle"]["migrationRules"]["apeiron"]["irreversible"] = json!(true);
    assert_eq!(
        plan(&input).unwrap()["protection"]["requiredVerificationLevel"],
        "isolated-restore-verified"
    );
}

#[test]
fn restore_rejects_same_generation_with_another_source_and_unproven_history() {
    let mut input = fixture(true);
    input["intent"]["operation"] = json!("application-restore");
    source_contract(&mut input);
    input["backupSet"] = backup(&input);
    input["backupSet"]["sourceRelease"]["version"] = json!("another-release");
    assert!(codes(&plan(&input).unwrap()).contains(&"BACKUP_SOURCE_BASELINE_MISMATCH"));
    input["backupSet"] = backup(&input);
    input["installation"]["generation"] = json!(2);
    input["observation"]["generation"] = json!(2);
    input["intent"]["expectedGeneration"] = json!(2);
    refresh(&mut input);
    assert!(codes(&plan(&input).unwrap()).contains(&"HISTORICAL_BACKUP_LINEAGE_UNVERIFIED"));
}
