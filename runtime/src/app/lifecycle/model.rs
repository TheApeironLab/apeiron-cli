//! Versioned, non-secret site inputs. Artifact digests are SHA-256 of bytes.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

macro_rules! model {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name { $(pub $field: $ty),* }
    };
}
model!(ComponentState {
    artifact_digests: BTreeMap<String, String>, configuration_schema: String, data_schema: String
});
model!(ComponentContract {
    class: String, configuration_schema: String, data_schema: String, data_roles: Vec<String>
});
model!(BackupAdapter { version: String, kinds: Vec<String>, granularity: String, status: String, evidence: Vec<String> });
model!(MigrationRule {
    component: String, source_artifacts: BTreeMap<String, String>,
    source_configuration_schema: String, source_data_schema: String,
    target_configuration_schema: String, target_data_schema: String,
    required_components: BTreeMap<String, ComponentState>, protection_roles: Vec<String>,
    irreversible: bool, replay_safe: bool, evidence: Vec<String>
});
model!(UpgradePath {
    id: String, source_manifest_sha256: String, topologies: Vec<String>, architectures: Vec<String>,
    component_sets: Vec<Vec<String>>, migration_rules: Vec<String>, evidence: Vec<String>
});
model!(Lifecycle {
    schema_version: u64, components: BTreeMap<String, ComponentContract>,
    backup_adapters: BTreeMap<String, BackupAdapter>, migration_rules: BTreeMap<String, MigrationRule>,
    upgrade_paths: Vec<UpgradePath>
});
model!(Node {
    id: String,
    role: String
});
model!(ReleaseIdentity {
    version: String,
    manifest_sha256: String
});
model!(InstalledComponent {
    status: String, version: String, state: ComponentState, configuration_sha256: String,
    data_bindings: BTreeMap<String, String>
});
model!(DataResource {
    kind: String, owner: String, consumers: Vec<String>, locator_ref: String,
    estimated_bytes: Option<u64>, backup_adapter: Option<String>, restore_granularity: String
});
model!(AcceptedExpansion { id: String, generation: u64, node_ids: Vec<String>, evidence: Vec<String> });
model!(Installation {
    schema_version: u64, kind: String, installation_id: String, cluster_uid: String,
    generation: u64, release: ReleaseIdentity, topology: String, architecture: String,
    nodes: Vec<Node>, configuration_sha256: String, foundation_sha256: String,
    components: BTreeMap<String, InstalledComponent>, resources: BTreeMap<String, DataResource>,
    baseline_evidence: Vec<String>, accepted_expansions: Vec<AcceptedExpansion>
});
model!(Observation {
    schema_version: u64, installation_id: String, cluster_uid: String, generation: u64,
    record_sha256: String, configuration_sha256: String, foundation_sha256: String,
    nodes: Vec<Node>, components: BTreeMap<String, ComponentState>,
    healthy: bool, drift: Vec<String>, network: String
});
model!(Protection {
    owner: String,
    rpo_seconds: u64,
    rto_seconds: u64,
    downtime_seconds: u64,
    independent_copy_ref: String,
    recipient_fingerprint: String,
    capacity_bytes: u64
});
model!(Intent {
    schema_version: u64, operation: String, installation_id: String, cluster_uid: String,
    expected_generation: u64, selected_components: Vec<String>, add_agents: Vec<String>,
    protection: Option<Protection>
});
model!(BackupSet {
    schema_version: u64, backup_id: String, installation_id: String, cluster_uid: String,
    generation: u64, foundation_sha256: String, topology: String, nodes: Vec<Node>,
    component_ids: Vec<String>, resource_ids: Vec<String>, level: String,
    manifest_sha256: String, independent_copy_ref: String, recipient_fingerprint: String,
    restore_evidence: Vec<String>, source_release: ReleaseIdentity,
    component_states: BTreeMap<String, ComponentState>, resource_identities: BTreeMap<String, String>,
    configuration_sha256: String
});
