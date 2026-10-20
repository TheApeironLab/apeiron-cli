//! Pure snapshot planning. This module contains no subprocess or network calls.
//! Planning never authorizes execution, including when all input checks pass.
mod model;
#[cfg(test)]
mod tests;
use super::*;
use model::*;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const LIMIT: u64 = 4 * 1024 * 1024;
const KINDS: &[&str] = &[
    "database",
    "objects",
    "files",
    "identity",
    "configuration",
    "artifacts",
    "control-plane",
];
fn invalid() -> Error {
    fail("生命周期输入不符合 v1 契约；未执行任何集群操作。", 400)
}
fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|_| invalid())
}
fn identifier(value: &str) -> bool {
    config::matches(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$", value)
}
fn digest(value: &str) -> bool {
    config::matches(r"^[a-f0-9]{64}$", value)
}
fn check(test: bool) -> Result<()> {
    if test {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn names(values: &[String], nonempty: bool) -> Result<()> {
    check(
        (!nonempty || !values.is_empty())
            && values.iter().all(|v| identifier(v))
            && values.iter().collect::<BTreeSet<_>>().len() == values.len(),
    )
}
fn evidence(values: &[String], required: bool) -> Result<()> {
    names(values, required)?;
    check(values.iter().all(|v| digest(v)))
}
fn state(state: &ComponentState, installed: bool) -> Result<()> {
    check(
        (!installed || !state.artifact_digests.is_empty())
            && state.artifact_digests.iter().all(|(k, v)| identifier(k) && digest(v))
            && identifier(&state.configuration_schema)
            && identifier(&state.data_schema),
    )
}
fn node_set(nodes: &[Node], topology: &str) -> Result<BTreeSet<(String, String)>> {
    check(
        !nodes.is_empty()
            && nodes
                .iter()
                .all(|n| identifier(&n.id) && ["server", "agent"].contains(&n.role.as_str())),
    )?;
    let ids: BTreeSet<_> = nodes.iter().map(|n| &n.id).collect();
    check(ids.len() == nodes.len())?;
    let servers = nodes.iter().filter(|n| n.role == "server").count();
    check(match topology {
        "single-k3s" | "single-k3d" => servers == 1 && nodes.len() == 1,
        "multi-k3s" => servers > 0,
        _ => false,
    })?;
    Ok(nodes.iter().map(|n| (n.id.clone(), n.role.clone())).collect())
}
fn validate_record(record: &Installation) -> Result<()> {
    check(
        record.schema_version == 1
            && record.kind == "chentu-installation-record"
            && identifier(&record.installation_id)
            && identifier(&record.cluster_uid)
            && record.generation > 0
            && identifier(&record.release.version)
            && digest(&record.release.manifest_sha256)
            && digest(&record.configuration_sha256)
            && digest(&record.foundation_sha256)
            && ["amd64", "arm64"].contains(&record.architecture.as_str()),
    )?;
    node_set(&record.nodes, &record.topology)?;
    evidence(&record.baseline_evidence, false)?;
    check(!record.components.is_empty())?;
    for (id, component) in &record.components {
        check(
            identifier(id)
                && ["installed", "absent", "unknown"].contains(&component.status.as_str())
                && identifier(&component.version)
                && digest(&component.configuration_sha256),
        )?;
        state(&component.state, component.status == "installed")?;
        check(
            component
                .data_bindings
                .iter()
                .all(|(role, id)| identifier(role) && record.resources.contains_key(id)),
        )?;
    }
    for (id, resource) in &record.resources {
        check(
            identifier(id)
                && identifier(&resource.locator_ref)
                && KINDS.contains(&resource.kind.as_str())
                && record
                    .components
                    .get(&resource.owner)
                    .is_some_and(|c| c.status == "installed")
                && ["component", "service", "cluster"].contains(&resource.restore_granularity.as_str())
                && resource.backup_adapter.as_ref().is_none_or(|a| identifier(a)),
        )?;
        names(&resource.consumers, false)?;
        check(
            resource
                .consumers
                .iter()
                .all(|c| record.components.get(c).is_some_and(|s| s.status == "installed")),
        )?;
    }
    let mut ids = BTreeSet::new();
    for expansion in &record.accepted_expansions {
        check(
            identifier(&expansion.id)
                && ids.insert(&expansion.id)
                && expansion.generation > 0
                && expansion.generation <= record.generation,
        )?;
        evidence(&expansion.evidence, true)?;
        names(&expansion.node_ids, true)?;
        check(
            expansion
                .node_ids
                .iter()
                .all(|id| record.nodes.iter().any(|n| &n.id == id && n.role == "agent")),
        )?;
    }
    Ok(())
}
// Validate metadata on existing first-install paths too; absence only means legacy.
pub(super) fn validate_release(manifest: &Value) -> Result<()> {
    if manifest.get("lifecycle").is_none() {
        return Ok(());
    }
    let lc: Lifecycle = decode(&manifest["lifecycle"])?;
    check(lc.schema_version == 1)?;
    let components = manifest["components"].as_object().ok_or_else(invalid)?;
    check(lc.components.keys().collect::<BTreeSet<_>>() == components.keys().collect())?;
    for (id, c) in &lc.components {
        check(
            identifier(id)
                && ["application", "foundation", "unknown"].contains(&c.class.as_str())
                && identifier(&c.configuration_schema)
                && identifier(&c.data_schema),
        )?;
        names(&c.data_roles, false)?;
    }
    for (id, adapter) in &lc.backup_adapters {
        check(
            identifier(id)
                && identifier(&adapter.version)
                && ["component", "service", "cluster"].contains(&adapter.granularity.as_str())
                && ["unsupported", "declared", "verified"].contains(&adapter.status.as_str()),
        )?;
        names(&adapter.kinds, true)?;
        check(adapter.kinds.iter().all(|k| KINDS.contains(&k.as_str())))?;
        evidence(&adapter.evidence, adapter.status == "verified")?;
    }
    for (id, rule) in &lc.migration_rules {
        check(identifier(id))?;
        state(
            &ComponentState {
                artifact_digests: rule.source_artifacts.clone(),
                configuration_schema: rule.source_configuration_schema.clone(),
                data_schema: rule.source_data_schema.clone(),
            },
            true,
        )?;
        let target = lc.components.get(&rule.component).ok_or_else(invalid)?;
        check(
            rule.target_configuration_schema == target.configuration_schema
                && rule.target_data_schema == target.data_schema
                && [
                    &rule.source_configuration_schema,
                    &rule.source_data_schema,
                    &rule.target_configuration_schema,
                    &rule.target_data_schema,
                ]
                .iter()
                .all(|s| identifier(s) && s.as_str() != "unknown"),
        )?;
        names(&rule.protection_roles, false)?;
        check(rule.protection_roles.iter().all(|r| target.data_roles.contains(r)))?;
        for (id, required) in &rule.required_components {
            check(lc.components.contains_key(id))?;
            state(required, true)?;
        }
        evidence(&rule.evidence, true)?;
    }
    let mut ids = BTreeSet::new();
    for path in &lc.upgrade_paths {
        check(identifier(&path.id) && ids.insert(&path.id) && digest(&path.source_manifest_sha256))?;
        names(&path.topologies, true)?;
        names(&path.architectures, true)?;
        check(
            path.topologies
                .iter()
                .all(|t| ["single-k3d", "single-k3s", "multi-k3s"].contains(&t.as_str()))
                && path
                    .architectures
                    .iter()
                    .all(|a| ["amd64", "arm64"].contains(&a.as_str())),
        )?;
        names(&path.migration_rules, false)?;
        check(
            path.migration_rules.iter().all(|r| lc.migration_rules.contains_key(r)) && !path.component_sets.is_empty(),
        )?;
        let mut sets = BTreeSet::new();
        for set in &path.component_sets {
            names(set, true)?;
            check(
                set.iter().all(|c| lc.components.contains_key(c)) && sets.insert(set.iter().collect::<BTreeSet<_>>()),
            )?;
        }
        evidence(&path.evidence, true)?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Blocker {
    code: String,
    subject: String,
}
fn block(blockers: &mut Vec<Blocker>, code: &str, subject: &str) {
    if !blockers.iter().any(|b| b.code == code && b.subject == subject) {
        blockers.push(Blocker {
            code: code.into(),
            subject: subject.into(),
        });
    }
}
fn installed(record: &Installation) -> BTreeSet<String> {
    record
        .components
        .iter()
        .filter(|(_, c)| c.status == "installed")
        .map(|(id, _)| id.clone())
        .collect()
}
fn target_artifacts(manifest: &Value, id: &str) -> Result<BTreeMap<String, String>> {
    let artifacts = manifest["artifacts"].as_array().ok_or_else(invalid)?;
    let indexed: BTreeMap<_, _> = artifacts.iter().map(|a| (a["id"].as_str().unwrap_or(""), a)).collect();
    check(indexed.len() == artifacts.len())?;
    let mut result = BTreeMap::new();
    for artifact in manifest["components"][id]["artifacts"].as_array().ok_or_else(invalid)? {
        let key = artifact.as_str().ok_or_else(invalid)?;
        let item = indexed.get(key).ok_or_else(invalid)?;
        let hash = item["sha256"].as_str().ok_or_else(invalid)?;
        check(identifier(key) && digest(hash) && result.insert(key.into(), hash.into()).is_none())?;
    }
    check(!result.is_empty())?;
    Ok(result)
}

/// Canonical non-secret record identity; distinct from a publisher's file digest.
fn identity(value: &Value) -> String {
    config::hash(&serde_json::to_vec(value).unwrap())
}

pub(super) fn plan(input: &Value) -> Result<Value> {
    let object = input.as_object().ok_or_else(invalid)?;
    check(object.keys().all(|k| {
        [
            "installation",
            "observation",
            "intent",
            "releaseManifest",
            "releaseManifestSha256",
            "backupSet",
        ]
        .contains(&k.as_str())
    }))?;
    let record: Installation = decode(&input["installation"])?;
    validate_record(&record)?;
    let observation: Observation = decode(&input["observation"])?;
    let intent: Intent = decode(&input["intent"])?;
    check(
        observation.schema_version == 1
            && intent.schema_version == 1
            && ["application-upgrade", "backup", "application-restore", "expand"].contains(&intent.operation.as_str())
            && ["offline", "online"].contains(&observation.network.as_str()),
    )?;
    names(&intent.selected_components, true)?;
    names(&intent.add_agents, false)?;
    names(&observation.drift, false)?;
    let mut blockers = Vec::new();
    let cluster_matches = intent.cluster_uid == record.cluster_uid
        && intent.installation_id == record.installation_id
        && observation.cluster_uid == record.cluster_uid
        && observation.installation_id == record.installation_id;
    if !cluster_matches {
        block(&mut blockers, "CLUSTER_IDENTITY_MISMATCH", "installation");
    }
    if intent.expected_generation != record.generation || observation.generation != record.generation {
        block(&mut blockers, "STALE_BASELINE", "generation");
    }
    if observation.record_sha256 != identity(&input["installation"]) {
        block(&mut blockers, "RECORD_IDENTITY_MISMATCH", "installation");
    }
    if record.baseline_evidence.is_empty() {
        block(&mut blockers, "BASELINE_UNVERIFIED", "installation");
    }
    if !observation.healthy {
        block(&mut blockers, "CLUSTER_UNHEALTHY", "observation");
    }
    if !observation.drift.is_empty() {
        block(&mut blockers, "UNEXPLAINED_DRIFT", "observation");
    }
    let nodes = node_set(&record.nodes, &record.topology)?;
    if node_set(&observation.nodes, &record.topology)? != nodes
        || observation.configuration_sha256 != record.configuration_sha256
        || observation.foundation_sha256 != record.foundation_sha256
    {
        block(&mut blockers, "BASELINE_DRIFT", "observation");
    }
    let installed = installed(&record);
    if observation.components.keys().cloned().collect::<BTreeSet<_>>() != installed
        || installed
            .iter()
            .any(|id| observation.components[id] != record.components[id].state)
    {
        block(&mut blockers, "COMPONENT_DRIFT", "observation");
    }
    for (id, c) in &record.components {
        if c.status == "unknown" {
            block(&mut blockers, "COMPONENT_STATE_UNKNOWN", id);
        }
        if c.status == "installed" && (c.state.configuration_schema == "unknown" || c.state.data_schema == "unknown") {
            block(&mut blockers, "SOURCE_SCHEMA_UNKNOWN", id);
        }
    }
    let selected: BTreeSet<_> = intent.selected_components.iter().cloned().collect();
    for id in selected.difference(&installed) {
        block(&mut blockers, "COMPONENT_NOT_INSTALLED", id);
    }
    let mut changed = BTreeSet::new();
    let mut migrations = Vec::new();
    let mut target_release = Value::Null;
    let mut lifecycle = None;
    if !input["releaseManifest"].is_null() {
        let manifest = &input["releaseManifest"];
        check(
            manifest["schemaVersion"] == 1
                && manifest["kind"] == "chentu-release"
                && manifest["release"].as_str().is_some_and(identifier),
        )?;
        let file_sha = input["releaseManifestSha256"].as_str().ok_or_else(invalid)?;
        check(digest(file_sha))?;
        validate_release(manifest)?;
        // The CLI computes the digest from the exact manifest file bytes.
        target_release = json!({"version":manifest["release"], "manifestSha256":file_sha});
        if manifest.get("lifecycle").is_some() {
            lifecycle = Some(decode::<Lifecycle>(&manifest["lifecycle"])?);
        }
    }
    if intent.operation == "application-upgrade" {
        if selected != installed {
            block(&mut blockers, "COMPONENT_SET_CHANGE_UNSUPPORTED", "selection");
        }
        match &lifecycle {
            None => block(&mut blockers, "LIFECYCLE_CONTRACT_MISSING", "release"),
            Some(lc) => {
                let manifest = &input["releaseManifest"];
                if !manifest["profiles"].as_object().is_some_and(|profiles| {
                    profiles.values().any(|profile| {
                        profile["architecture"] == record.architecture
                            && profile["supportedTopologies"]
                                .as_array()
                                .is_some_and(|topologies| topologies.contains(&json!(record.topology)))
                    })
                }) {
                    block(&mut blockers, "TARGET_PLATFORM_UNSUPPORTED", "release");
                }
                let paths: Vec<_> = lc
                    .upgrade_paths
                    .iter()
                    .filter(|p| {
                        p.source_manifest_sha256 == record.release.manifest_sha256
                            && p.topologies.contains(&record.topology)
                            && p.architectures.contains(&record.architecture)
                            && p.component_sets
                                .iter()
                                .any(|s| s.iter().cloned().collect::<BTreeSet<_>>() == installed)
                    })
                    .collect();
                if paths.len() != 1 {
                    block(&mut blockers, "UPGRADE_PATH_NOT_UNIQUELY_VERIFIED", "release");
                }
                for id in &installed {
                    let Some(c) = lc.components.get(id) else {
                        block(&mut blockers, "TARGET_COMPONENT_MISSING", id);
                        continue;
                    };
                    let current = &record.components[id].state;
                    let target = ComponentState {
                        artifact_digests: target_artifacts(manifest, id)?,
                        configuration_schema: c.configuration_schema.clone(),
                        data_schema: c.data_schema.clone(),
                    };
                    if target == *current {
                        continue;
                    }
                    changed.insert(id.clone());
                    if c.class != "application" {
                        block(&mut blockers, "FOUNDATION_CHANGE_UNSUPPORTED", id);
                    }
                    if c.configuration_schema == "unknown" || c.data_schema == "unknown" {
                        block(&mut blockers, "TARGET_SCHEMA_UNKNOWN", id);
                    }
                    let rules: Vec<_> = paths
                        .first()
                        .into_iter()
                        .flat_map(|p| &p.migration_rules)
                        .filter_map(|key| lc.migration_rules.get(key).map(|r| (key, r)))
                        .filter(|(_, r)| {
                            r.component == *id
                                && r.source_artifacts == current.artifact_digests
                                && r.source_configuration_schema == current.configuration_schema
                                && r.source_data_schema == current.data_schema
                        })
                        .collect();
                    if rules.len() != 1 {
                        block(&mut blockers, "MIGRATION_RULE_NOT_UNIQUELY_VERIFIED", id);
                    }
                    for (key, rule) in rules {
                        for (dependency, required) in &rule.required_components {
                            if record
                                .components
                                .get(dependency)
                                .is_none_or(|c| c.status != "installed" || &c.state != required)
                            {
                                block(&mut blockers, "MIGRATION_PREREQUISITE_MISMATCH", dependency);
                            }
                        }
                        if !rule.replay_safe {
                            block(&mut blockers, "MIGRATION_RECOVERY_UNVERIFIED", id);
                        }
                        migrations.push(json!({"id":key,"component":id,"irreversible":rule.irreversible,"replaySafe":rule.replay_safe}));
                    }
                }
                // Keep Helmfile's graph authoritative: require its declared dependencies,
                // but never compute an alternative deployment order here.
                for id in &installed {
                    let Some(component) = manifest["components"].get(id) else {
                        continue;
                    };
                    for dep in component["requires"].as_array().ok_or_else(invalid)? {
                        let dep = dep.as_str().ok_or_else(invalid)?;
                        if !installed.contains(dep) {
                            block(&mut blockers, "NEW_DEPENDENCY_REQUIRES_SEPARATE_CHANGE", dep);
                        }
                    }
                }
            }
        }
    } else if intent.operation == "backup" {
        if target_release["manifestSha256"] != record.release.manifest_sha256 {
            block(&mut blockers, "BACKUP_SOURCE_CONTRACT_MISMATCH", "release");
        }
        if let Some(lc) = &lifecycle {
            for id in &installed {
                let current = &record.components[id].state;
                if lc.components.get(id).is_none_or(|c| {
                    c.configuration_schema != current.configuration_schema || c.data_schema != current.data_schema
                }) || target_artifacts(&input["releaseManifest"], id)? != current.artifact_digests
                {
                    block(&mut blockers, "SOURCE_CONTRACT_STATE_MISMATCH", id);
                }
            }
        }
    }
    if intent.operation != "application-restore" && !input["backupSet"].is_null() {
        block(&mut blockers, "BACKUP_REUSE_UNIMPLEMENTED", "backup");
    }
    let expansion_requested = !intent.add_agents.is_empty() || intent.operation == "expand";
    if expansion_requested {
        if record.topology != "multi-k3s" {
            block(&mut blockers, "EXPANSION_TOPOLOGY_UNSUPPORTED", "nodes");
        }
        if intent.add_agents.is_empty() {
            block(&mut blockers, "EXPANSION_NODES_MISSING", "nodes");
        }
        if intent
            .add_agents
            .iter()
            .any(|id| record.nodes.iter().any(|n| &n.id == id))
        {
            block(&mut blockers, "EXPANSION_NODE_ALREADY_PRESENT", "nodes");
        }
        block(&mut blockers, "EXPANSION_EXECUTOR_UNIMPLEMENTED", "nodes");
        if intent.operation == "application-upgrade" {
            block(&mut blockers, "POST_EXPANSION_BASELINE_REQUIRED", "generation");
        }
        if !["expand", "application-upgrade"].contains(&intent.operation.as_str()) {
            block(&mut blockers, "MIXED_OPERATION_UNSUPPORTED", "intent");
        }
    }
    let protected_components = if intent.operation == "application-upgrade" {
        &changed
    } else {
        &selected
    };
    let mut resources = BTreeSet::new();
    let mut freeze = BTreeSet::new();
    for id in protected_components {
        let Some(c) = record.components.get(id).filter(|c| c.status == "installed") else {
            continue;
        };
        freeze.insert(id.clone());
        let roles = lifecycle.as_ref().and_then(|lc| lc.components.get(id));
        if let Some(contract) = roles {
            for role in &contract.data_roles {
                if !c.data_bindings.contains_key(role) {
                    block(&mut blockers, "DATA_BINDING_MISSING", &format!("{id}/{role}"));
                }
            }
        } else {
            block(&mut blockers, "DATA_CONTRACT_UNVERIFIED", id);
        }
        for resource in c.data_bindings.values() {
            resources.insert(resource.clone());
        }
    }
    let mut total = Some(0u64);
    let mut protection_rows = Vec::new();
    for id in &resources {
        let resource = &record.resources[id];
        if !protected_components
            .iter()
            .any(|c| resource.owner == *c || resource.consumers.contains(c))
        {
            block(&mut blockers, "DATA_OWNERSHIP_MISMATCH", id);
        }
        freeze.insert(resource.owner.clone());
        freeze.extend(resource.consumers.iter().cloned());
        total = total.zip(resource.estimated_bytes).and_then(|(a, b)| a.checked_add(b));
        let adapter = resource
            .backup_adapter
            .as_ref()
            .and_then(|key| lifecycle.as_ref().and_then(|lc| lc.backup_adapters.get(key)));
        if adapter.is_none_or(|a| {
            a.status != "verified" || !a.kinds.contains(&resource.kind) || a.granularity != resource.restore_granularity
        }) {
            block(&mut blockers, "BACKUP_RESTORE_ADAPTER_UNVERIFIED", id);
        }
        if resource.restore_granularity != "component" && resource.consumers.iter().any(|c| !selected.contains(c)) {
            block(&mut blockers, "SHARED_RESTORE_SCOPE_EXCEEDS_SELECTION", id);
        }
        if intent.operation == "application-restore" && resource.kind == "control-plane" {
            block(&mut blockers, "APPLICATION_RESTORE_CANNOT_RESTORE_DATASTORE", id);
        }
        protection_rows.push(json!({"id":id,"kind":resource.kind,"owner":resource.owner,
            "consumers":resource.consumers,"estimatedBytes":resource.estimated_bytes,
            "backupAdapter":resource.backup_adapter,"restoreGranularity":resource.restore_granularity}));
    }
    if let Some(protection) = &intent.protection {
        check(
            identifier(&protection.owner)
                && identifier(&protection.independent_copy_ref)
                && identifier(&protection.recipient_fingerprint),
        )?;
        if protection.rto_seconds == 0 || protection.downtime_seconds == 0 {
            block(&mut blockers, "PROTECTION_BUDGET_MISSING", "protection");
        }
        // Data size alone does not include decrypt, drill and coexistence peaks.
        if total.is_none() {
            block(&mut blockers, "CAPACITY_ESTIMATE_INCOMPLETE", "protection");
        }
        if total.is_some_and(|n| protection.capacity_bytes < n) {
            block(&mut blockers, "BACKUP_CAPACITY_INSUFFICIENT", "protection");
        }
    } else {
        block(&mut blockers, "PROTECTION_POLICY_REQUIRED", "protection");
    }
    let mut restore = Value::Null;
    if intent.operation == "application-restore" {
        if input["backupSet"].is_null() {
            block(&mut blockers, "BACKUP_SET_REQUIRED", "backup");
        } else {
            let backup: BackupSet = decode(&input["backupSet"])?;
            check(
                backup.schema_version == 1
                    && identifier(&backup.backup_id)
                    && digest(&backup.manifest_sha256)
                    && digest(&backup.configuration_sha256)
                    && backup.generation > 0
                    && digest(&backup.foundation_sha256)
                    && digest(&backup.source_release.manifest_sha256)
                    && identifier(&backup.source_release.version)
                    && identifier(&backup.independent_copy_ref)
                    && identifier(&backup.recipient_fingerprint)
                    && [
                        "generated",
                        "integrity-verified",
                        "isolated-restore-verified",
                        "disaster-restore-verified",
                    ]
                    .contains(&backup.level.as_str()),
            )?;
            names(&backup.component_ids, true)?;
            names(&backup.resource_ids, false)?;
            evidence(&backup.restore_evidence, false)?;
            for component in backup.component_states.values() {
                state(component, true)?;
            }
            check(
                backup
                    .resource_identities
                    .iter()
                    .all(|(id, sha)| identifier(id) && digest(sha)),
            )?;
            if backup.cluster_uid != record.cluster_uid || backup.installation_id != record.installation_id {
                block(&mut blockers, "BACKUP_CLUSTER_MISMATCH", "backup");
            }
            if backup.generation > record.generation
                || backup.foundation_sha256 != record.foundation_sha256
                || backup.topology != record.topology
                || node_set(&backup.nodes, &backup.topology)? != nodes
                || record
                    .accepted_expansions
                    .iter()
                    .any(|e| e.generation > backup.generation)
            {
                block(&mut blockers, "BACKUP_PREDATES_ACCEPTED_BASELINE", "backup");
            }
            if !["isolated-restore-verified", "disaster-restore-verified"].contains(&backup.level.as_str())
                || backup.restore_evidence.is_empty()
            {
                block(&mut blockers, "BACKUP_RESTORE_NOT_VERIFIED", "backup");
            }
            if target_release["manifestSha256"] != backup.source_release.manifest_sha256 {
                block(&mut blockers, "RESTORE_SOURCE_CONTRACT_MISMATCH", "release");
            }
            if let Some(lc) = &lifecycle {
                for id in &selected {
                    let target = lc.components.get(id).zip(backup.component_states.get(id));
                    if target.is_none_or(|(c, s)| {
                        c.configuration_schema != s.configuration_schema || c.data_schema != s.data_schema
                    }) {
                        block(&mut blockers, "RESTORE_COMPONENT_STATE_MISMATCH", id);
                    } else if target_artifacts(&input["releaseManifest"], id)?
                        != backup.component_states[id].artifact_digests
                    {
                        block(&mut blockers, "RESTORE_ARTIFACT_MISMATCH", id);
                    }
                }
            } else {
                block(&mut blockers, "LIFECYCLE_CONTRACT_MISSING", "release");
            }
            if selected.iter().any(|c| !backup.component_ids.contains(c))
                || resources.iter().any(|r| !backup.resource_ids.contains(r))
            {
                block(&mut blockers, "BACKUP_COVERAGE_INCOMPLETE", "backup");
            }
            for id in &resources {
                if backup.resource_identities.get(id) != Some(&identity(&input["installation"]["resources"][id])) {
                    block(&mut blockers, "BACKUP_DATA_MAPPING_MISMATCH", id);
                }
            }
            restore = json!({"backupId":backup.backup_id,"manifestSha256":backup.manifest_sha256,
                "scope":"application","restoreControlPlane":false,"removeNodes":[],"preserveNodes":record.nodes,
                "restoreConfigurationSha256":backup.configuration_sha256,
                "sourceGeneration":backup.generation,"targetGeneration":record.generation});
        }
    }
    let planning_ready = blockers.is_empty();
    let required_verification =
        if intent.operation == "application-restore" || migrations.iter().any(|m| m["irreversible"] == true) {
            "isolated-restore-verified"
        } else {
            "integrity-verified"
        };
    let mut stages = Vec::new();
    if expansion_requested {
        stages.push(
            json!({"operation":"expand","status":"blocked","existingNodes":record.nodes,
            "addAgents":intent.add_agents,"changeServerMembership":false,"requiresIndependentAcceptance":true,
            "requiredProtection":["actual-datastore","server-token","node-configuration","affected-application-data"],
            "acceptanceChecks":["scheduling-quarantine","same-k3s-version","registry-and-ca","dns-and-cni",
                "storage-and-pvc-topology","rwo-and-local-path","workbench-and-business-workload"],
            "moveExistingVolumes":false}),
        );
        stages.push(
            json!({"operation":"commit-baseline","status":"blocked","requiresEvidence":true,
            "requiresFreshObservation":true,"requiresFreshApplicationBackup":true}),
        );
    }
    stages.push(
        json!({"operation":intent.operation,"status":if planning_ready {"planned"} else {"blocked"},
        "baselineGeneration":record.generation,"preserveAcceptedExpansions":true}),
    );
    let payload = json!({"schemaVersion":1,"kind":"chentu-lifecycle-plan","readOnly":true,
        "planningStatus":if planning_ready {"ready"} else {"blocked"},"executionAllowed":false,
        "executionBlockers":["EXECUTOR_UNIMPLEMENTED","LIVE_REVALIDATION_REQUIRED","ACCEPTANCE_REQUIRED",
            "EVIDENCE_CONTENT_VERIFICATION_UNIMPLEMENTED","FULL_CAPACITY_ESTIMATE_UNIMPLEMENTED",
            "EFFECTIVE_VALUES_VALIDATION_UNIMPLEMENTED"],
        "clusterUid":record.cluster_uid,"installationId":record.installation_id,"generation":record.generation,
        "topology":record.topology,"architecture":record.architecture,"network":observation.network,
        "sourceRelease":record.release,"targetRelease":target_release,
        "configurationSha256":record.configuration_sha256,"foundationSha256":record.foundation_sha256,
        "selectedComponents":selected,"changedComponents":changed,"migrations":migrations,
        "protection":{"resources":protection_rows,"freezeComponents":freeze,"estimatedDataBytes":total,
            "policy":intent.protection,"consistentRecoveryPointRequired":true,
            "requiredVerificationLevel":required_verification,"releasePathRequiresIsolationEvidence":true,
            "capacityBudgetIncludesDrillAndCoexistence":false},
        "restore":restore,"stages":stages,"blockers":blockers,
        "inputSha256":identity(input),"recordIdentityAlgorithm":"sorted-json-utf8-v1"});
    let plan_id = identity(&payload);
    let mut result = payload;
    result["planId"] = json!(plan_id);
    result["stateNamespace"] = json!(format!("{}/{}/{}", record.cluster_uid, record.installation_id, plan_id));
    Ok(result)
}

pub fn run(args: &[String]) -> Result<()> {
    let (pos, options) = parse(
        args,
        &["installation", "observation", "intent", "manifest", "backup", "out"],
        &["help"],
    )?;
    if options.contains_key("help") {
        println!("apeiron lifecycle validate-manifest --manifest FILE\napeiron lifecycle validate-record --installation FILE\napeiron lifecycle plan --installation FILE --observation FILE --intent FILE [--manifest FILE] [--backup FILE] [--out NEW_FILE]\nRead-only snapshot planning; all execution operations are blocked.");
        return Ok(());
    }
    if pos == ["validate-manifest"] {
        check(options.keys().all(|k| k == "manifest"))?;
        let value = resources::json_file(Path::new(required(&options, "manifest")?), LIMIT)?;
        check(value["schemaVersion"] == 1 && value["kind"] == "chentu-release")?;
        validate_release(&value)?;
        println!(
            "{}",
            json!({"valid":true,"scope":"lifecycle-metadata","hasLifecycle":value.get("lifecycle").is_some(),"executionAllowed":false})
        );
        return Ok(());
    }
    if pos == ["validate-record"] {
        check(options.keys().all(|k| k == "installation"))?;
        let value = resources::json_file(Path::new(required(&options, "installation")?), LIMIT)?;
        validate_record(&decode::<Installation>(&value)?)?;
        println!(
            "{}",
            json!({"schemaVersion":1,"kind":"chentu-installation-validation","valid":true,
            "liveVerified":false,"recordSha256":identity(&value),"identityAlgorithm":"sorted-json-utf8-v1"})
        );
        return Ok(());
    }
    if pos != ["plan"] {
        return Err(fail(
            "未实现或未验收的升级、扩容、备份及恢复执行均被阻断；仅支持 plan、validate-record 和 validate-manifest。",
            409,
        ));
    }
    let mut input = json!({});
    for key in ["installation", "observation", "intent"] {
        input[key] = resources::json_file(Path::new(required(&options, key)?), LIMIT)?;
    }
    if let Some(path) = options.get("manifest") {
        let bytes = resources::read(Path::new(path), LIMIT)?;
        input["releaseManifest"] = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        input["releaseManifestSha256"] = json!(config::hash(&bytes));
    }
    if let Some(path) = options.get("backup") {
        input["backupSet"] = resources::json_file(Path::new(path), LIMIT)?;
    }
    let result = plan(&input)?;
    let bytes = serde_json::to_vec_pretty(&result).map_err(|_| invalid())?;
    if let Some(path) = options.get("out") {
        private_write(Path::new(path), &bytes)?;
        println!(
            "{}",
            json!({"planId":result["planId"],"planningStatus":result["planningStatus"],"executionAllowed":false,"saved":true})
        );
    } else {
        println!("{}", String::from_utf8(bytes).unwrap());
    }
    Ok(())
}
