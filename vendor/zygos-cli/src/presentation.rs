//! CLI-only projections. Never alter REST responses, requests, identity or export bytes.
use crate::error::{CliError, Result};
use crate::field_groups;
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Summary,
    Full,
}
impl View {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "summary" => Ok(Self::Summary),
            "full" => Ok(Self::Full),
            _ => Err(CliError::usage("--view must be summary or full")),
        }
    }
}

pub fn parse_fields(value: &str) -> Result<Vec<String>> {
    let fields: Vec<_> = value.split(',').map(str::trim).map(str::to_owned).collect();
    if fields.iter().any(|field| field == "@groups") {
        return if fields.len() == 1 {
            Ok(fields)
        } else {
            Err(CliError::usage("--fields @groups must be used alone"))
        };
    }
    if fields.iter().any(|field| {
        if field.starts_with('@') {
            return !field_groups::GROUPS.contains(&field.as_str());
        }
        field.split('.').any(|part| {
            part.is_empty() || !part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
    }) {
        return Err(CliError::usage(
            "--fields requires field paths or groups, e.g. id,state.status,@metrics; use --fields @groups to discover fields",
        ));
    }
    Ok(fields)
}

/// Explicitly exclude commands with side effects and inspection/acceptance contracts.
pub fn can_select(command: &str) -> bool {
    matches!(
        command,
        "dataset list"
            | "dataset show"
            | "dataset records"
            | "dataset record show"
            | "dataset business-info"
            | "converter list"
            | "converter show"
            | "transform list"
            | "ref list"
            | "ref trash"
            | "ref show"
            | "lineage show"
            | "model list"
            | "model show"
            | "model versions"
            | "model version show"
            | "model weights list"
            | "model weights bindings"
            | "model artifact list"
            | "model artifact show"
            | "model artifact import-status"
            | "model deployment list"
            | "model deployment show"
            | "model deployment config list"
            | "model deployment capabilities"
            | "training node list"
            | "training node show"
            | "training run list"
            | "training run show"
            | "training session list"
            | "training session show"
            | "training session outputs"
            | "training bundle list"
            | "training bundle show"
            | "training resources"
            | "training models"
            | "baml list"
            | "baml show"
            | "evaluation list"
            | "evaluation show"
            | "evaluation task list"
            | "evaluation task show"
            | "evaluation task report"
            | "evaluation task config"
            | "evaluation task benchmarks"
            | "evaluation task metrics"
    )
}

pub fn render(command: &str, value: Value, view: View, fields: &[String]) -> Result<Value> {
    // Explicit selection always reads the full result and takes precedence over the view.
    if !fields.is_empty() {
        return select(value, fields, false);
    }
    if view == View::Full || !can_select(command) {
        return Ok(value);
    }
    select(value, &["@basic".into()], true)
}

fn select(mut value: Value, fields: &[String], passthrough_scalars: bool) -> Result<Value> {
    if fields == ["@groups"] {
        return Ok(field_groups::discover(&value));
    }
    if let Some(items) = value.get_mut("items").and_then(Value::as_array_mut) {
        for item in items {
            *item = project(item, fields, passthrough_scalars)?;
        }
        // Keep all page metadata, including cursors, counts, errors and partial availability.
    } else if let Some(items) = value.as_array_mut() {
        for item in items {
            *item = project(item, fields, passthrough_scalars)?;
        }
    } else {
        value = project(&value, fields, passthrough_scalars)?;
    }
    Ok(value)
}

fn project(value: &Value, fields: &[String], passthrough_scalars: bool) -> Result<Value> {
    if !value.is_object() {
        if passthrough_scalars {
            return Ok(value.clone());
        }
        return Err(CliError::usage(
            "--fields requires an object or a list of objects",
        ));
    }
    let mut projected = Map::new();
    for field in fields {
        if field.starts_with('@') {
            // Groups select present keys only; heterogeneous rows and empty groups stay valid.
            for (key, value) in value.as_object().unwrap() {
                if key != "_cli" && field_groups::group(key) == field {
                    projected.insert(key.clone(), value.clone());
                }
            }
            continue;
        }
        let parts: Vec<_> = field.split('.').collect();
        let mut current = value;
        for key in &parts {
            current = current.get(*key).ok_or_else(|| {
                CliError::usage(format!(
                    "field '{field}' is absent; inspect this read command with --view full"
                ))
            })?;
        }
        insert_path(&mut projected, &parts, current.clone());
    }
    Ok(Value::Object(projected))
}

fn insert_path(object: &mut Map<String, Value>, path: &[&str], value: Value) {
    if path.len() == 1 {
        object.insert(path[0].into(), value);
    } else {
        let child = object.entry(path[0]).or_insert_with(|| json!({}));
        // A parent field selected earlier already includes the child.
        if let Some(child) = child.as_object_mut() {
            insert_path(child, &path[1..], value);
        }
    }
}
