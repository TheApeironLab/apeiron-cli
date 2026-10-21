//! Offline help generated from the same command and request catalogs as execution.
use crate::error::{CliError, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn render(full: &Value, topic: Option<&str>, view: crate::presentation::View) -> Value {
    if view == crate::presentation::View::Full {
        return full.clone();
    }
    let topic = topic.unwrap_or("");
    let catalog = crate::args::catalog();
    if catalog.iter().any(|spec| spec.name == topic) {
        let mut value = full.clone();
        for group in value["commands"].as_array_mut().unwrap() {
            group.as_object_mut().unwrap().remove("input");
            for verb in group["verbs"].as_array_mut().unwrap() {
                let object = verb.as_object_mut().unwrap();
                if let Some(schema) = object.remove("input_schema") {
                    object.insert("input_required".into(), schema["required"].clone());
                }
                object.remove("input_guide");
            }
        }
        value["details"] = json!(format!("zygos {topic} --help --view full"));
        value["hint"] = json!(
            "Use --view full for complete input_schema, input formats and workflow guidance before constructing a request."
        );
        return value;
    }
    let prefix = if topic.is_empty() {
        String::new()
    } else {
        format!("{topic} ")
    };
    let mut children = BTreeMap::new();
    for spec in &catalog {
        let Some(suffix) = spec.name.strip_prefix(&prefix) else {
            continue;
        };
        let child = suffix.split(' ').next().unwrap();
        let child_path = format!("{prefix}{child}");
        let summary = if suffix == child {
            spec.summary.as_str()
        } else {
            group_summary(child)
        };
        children.entry(child.to_owned()).or_insert_with(|| json!({
            "name": child_path.strip_prefix(&format!("{} ", topic.split(' ').next().unwrap_or(""))).unwrap_or(&child_path),
            "summary":summary,
            "help":format!("zygos {child_path} --help")
        }));
    }
    let commands = if topic.is_empty() {
        children.into_values().collect::<Vec<_>>()
    } else {
        vec![
            json!({"name":topic.split(' ').next().unwrap(),"verbs":children.into_values().collect::<Vec<_>>()}),
        ]
    };
    json!({
        "commands":commands,
        "global_options":full["global_options"],
        "hint":"Help lists one level at a time. Use a child command's --help; --view full includes complete schemas. Read results default to @basic fields; --fields selects read fields; --compact only removes whitespace."
    })
}

fn group_summary(name: &str) -> &str {
    match name {
        "auth" => "Authentication",
        "baml" => "BAML templates and published versions",
        "dataset" => "Datasets, records, import and export",
        "converter" => "Export converters and fidelity contracts",
        "transform" => "Dataset transforms",
        "ref" => "Named dataset references",
        "lineage" => "Dataset lineage",
        "model" => "Model versions, weights and deployments",
        "training" => "Training nodes, bundles, sessions and runs",
        "evaluation" => "Evaluation tasks and saved results",
        "deployment" => "Inference deployments and lifecycle",
        "weights" => "Registered model weights",
        "registration" => "Inspect and commit model registration",
        "artifact" => "Archived model artifacts",
        "node" => "SSH training nodes",
        "session" => "Studio sessions",
        "bundle" => "Prepared training data",
        "run" => "Training runs",
        "task" => "Execute and query evaluation tasks",
        "record" => "Individual dataset records",
        "config" => "Saved deployment configurations",
        "version" => "Model version details",
        _ => "Subcommands",
    }
}

pub(crate) fn full(topic: Option<&str>) -> Result<Value> {
    let mut all: Value = serde_json::from_str(include_str!("../commands.json")).unwrap();
    for verb in ["login", "status", "logout"] {
        all.as_array_mut().unwrap().push(json!({"name":format!("auth {verb}"), "operation":"localAuth", "positionals":[], "options":[], "output":"json", "summary":format!("Keycloak {verb}")}));
    }
    let requests: Value = serde_json::from_str(include_str!("request-help.json")).unwrap();
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for mut command in all.as_array().unwrap().clone() {
        let full_name = command["name"].as_str().unwrap().to_owned();
        let (group, verb) = full_name.split_once(' ').unwrap();
        if topic.is_some_and(|t| full_name != t && !full_name.starts_with(&format!("{t} "))) {
            continue;
        }
        if topic.is_some_and(|t| t.contains(' ')) {
            let schema = &requests[command["operation"].as_str().unwrap()];
            if !schema.is_null() {
                command["input_schema"] = schema.clone();
            }
        }
        if full_name == "evaluation task create" {
            command["input_guide"] =
                serde_json::from_str::<Value>(include_str!("evaluation-task-help.json")).unwrap();
        }
        command["name"] = json!(verb);
        command.as_object_mut().unwrap().remove("operation");
        groups.entry(group.to_owned()).or_default().push(command);
    }
    if groups.is_empty() {
        return Err(CliError::usage("unknown help topic"));
    }
    Ok(json!({"commands":groups.into_iter().map(|(name,verbs)| {
        let mut group=json!({"name":name,"verbs":verbs});
        if name=="lineage" { group["default_verb"]=json!("show"); }
        if name=="dataset" && topic.is_some_and(|t| t=="dataset" || t.starts_with("dataset ")) {
            group["input"]=serde_json::from_str(include_str!("input-help.json")).expect("compiled input help");
        }
        group
    }).collect::<Vec<_>>(), "global_options":[
        {"name":"base-url","type":"string","env":"ZYGOS_API_BASE_URL","default":"http://127.0.0.1:8000"},
        {"name":"compact","type":"boolean"},
        {"name":"timeout-seconds","type":"string","default":"300"},
        {"name":"view","type":"string","enum":["summary","full"],"default":"summary"},
        {"name":"fields","type":"string","description":"Select paths or @groups (e.g. id,@metrics); --fields @groups lists grouped fields; page metadata is preserved"},
        {"name":"json","type":"boolean","description":"Explicit JSON output (also the default)"}
    ]}))
}
