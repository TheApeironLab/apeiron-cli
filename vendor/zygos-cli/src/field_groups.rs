//! Display-only field families. Discover actual keys from the complete current response.
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const GROUPS: &[&str] = &[
    "@basic", "@source", "@runtime", "@metrics", "@storage", "@config", "@audit", "@other",
];

pub fn group(field: &str) -> &'static str {
    if matches!(
        field,
        "id" | "name"
            | "description"
            | "status"
            | "error"
            | "ok"
            | "enabled"
            | "type"
            | "kind"
            | "revision"
            | "version"
            | "version_id"
            | "version_label"
            | "weight_kind"
            | "model_type"
    ) {
        "@basic"
    } else if field.ends_with("_at")
        || field.ends_with("_digest")
        || field.starts_with("archive_")
        || matches!(
            field,
            "digest" | "evalscope_commit" | "verification_level_snapshot"
        )
    {
        "@audit"
    } else if field.ends_with("_config")
        || field.ends_with("_options")
        || matches!(
            field,
            "config"
                | "options"
                | "parameters"
                | "request"
                | "draft"
                | "template"
                | "create_profile"
        )
    {
        "@config"
    } else if matches!(
        field,
        "metrics" | "scores" | "primary_metric_id" | "primary_output_key"
    ) {
        "@metrics"
    } else if field.ends_with("_path")
        || field.ends_with("_bytes")
        || field.starts_with("result_artifact_")
        || matches!(field, "files" | "storage" | "artifacts" | "outputs")
    {
        "@storage"
    } else if field.starts_with("node_")
        || field.starts_with("provider_")
        || matches!(
            field,
            "host"
                | "port"
                | "state"
                | "health"
                | "gpu"
                | "gpus"
                | "execution"
                | "progress"
                | "provider"
                | "attempts"
                | "attempt_id"
        )
    {
        "@runtime"
    } else if field.starts_with("model_")
        || field.starts_with("dataset_")
        || field.starts_with("source_")
        || matches!(
            field,
            "benchmark" | "converter" | "converter_version" | "versions" | "base_model"
        )
    {
        "@source"
    } else {
        "@other"
    }
}

pub fn catalog(value: &Value) -> BTreeMap<&'static str, BTreeSet<String>> {
    let mut groups = BTreeMap::new();
    let mut collect = |value: &Value| {
        if let Some(object) = value.as_object() {
            for key in object.keys().filter(|key| key.as_str() != "_cli") {
                groups
                    .entry(group(key))
                    .or_insert_with(BTreeSet::new)
                    .insert(key.clone());
            }
        }
    };
    if let Some(items) = value
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
    {
        for item in items {
            collect(item);
        }
    } else {
        collect(value);
    }
    groups
}

pub fn selectors(value: &Value) -> Vec<&'static str> {
    let groups = catalog(value);
    std::iter::once("@groups")
        .chain(
            GROUPS
                .iter()
                .copied()
                .filter(|name| groups.contains_key(name)),
        )
        .collect()
}

pub fn discover(value: &Value) -> Value {
    let scope = if value.is_array() || value.get("items").is_some_and(Value::is_array) {
        "items"
    } else {
        "result"
    };
    json!({"fields":catalog(value), "scope":scope})
}
