//! Read-only follow-ups. argv is structured data, never shell-interpolated or executed here.
use crate::{Parsed, View, args::Invocation, presentation::can_select};
use serde_json::{Value, json};

#[cfg(test)]
#[path = "guidance_tests.rs"]
mod tests;

pub fn attach(mut displayed: Value, original: &Value, parsed: &Parsed) -> Value {
    if parsed.view == View::Full {
        return displayed;
    }
    let i = parsed.invocation.as_ref().unwrap();
    // Never overwrite a provider's field that happens to use our display namespace.
    if original.get("_cli").is_some() && displayed.get("_cli").is_some() {
        displayed = json!({"result":displayed});
    }
    if !displayed.is_object() {
        displayed = if displayed.is_array() {
            json!({"items":displayed})
        } else {
            json!({"result":displayed})
        };
    }
    let mut meta = json!({});
    if can_select(&i.spec.name) {
        meta["full"] = json!("--view full");
        meta["fields"] = json!(crate::field_groups::selectors(original));
    } else if let Some((name, positionals, options)) = follow_up(i, original) {
        meta["full"] = json!(argv(&name, &positionals, &options, parsed));
    } else {
        let mut args = vec!["zygos".to_owned()];
        args.extend(i.spec.name.split_whitespace().map(str::to_owned));
        args.extend(["--help".into(), "--view".into(), "full".into()]);
        meta["help"] = json!(args);
    }
    displayed["_cli"] = meta;
    displayed
}

fn argv(name: &str, positionals: &[String], options: &[String], parsed: &Parsed) -> Vec<String> {
    let mut args = vec!["zygos".to_owned()];
    args.extend(name.split_whitespace().map(str::to_owned));
    args.extend_from_slice(options);
    // Preserve the service but never reflect credentials from an invalid URL into diagnostics.
    if reqwest::Url::parse(&parsed.base_url).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    }) {
        args.extend(["--base-url".into(), parsed.base_url.clone()]);
    }
    args.extend(["--view".into(), "full".into()]);
    if !positionals.is_empty() {
        args.push("--".into());
        args.extend_from_slice(positionals);
    }
    args
}

type FollowUp = (String, Vec<String>, Vec<String>);
fn follow_up(i: &Invocation, value: &Value) -> Option<FollowUp> {
    let name = i.spec.name.as_str();
    let resource = |show: &str, list: &str, id: Option<&str>| {
        Some((
            if id.is_some() { show } else { list }.to_owned(),
            id.into_iter().map(str::to_owned).collect(),
            vec![],
        ))
    };
    let id = value.get("id").and_then(Value::as_str);
    let first = i.positionals.first().map(String::as_str);
    match name {
        "baml create" | "baml update" | "baml publish" | "baml archive" | "baml restore" => {
            resource("baml show", "baml list", id.or(first))
        }
        "training run create" | "training run stop" => {
            resource("training run show", "training run list", id.or(first))
        }
        "training bundle create" => resource("training bundle show", "training bundle list", id),
        "training session create" => resource("training session show", "training session list", id),
        "training session close" => resource(
            "training session show",
            "training session list",
            id.or(first),
        ),
        "training node create" | "training node connect" | "training node disconnect" => {
            resource("", "training node list", None)
        }
        "model weights commit" | "model weights bind-base" => {
            resource("", "model weights list", None)
        }
        "model registration commit" => resource(
            "model show",
            "model list",
            value
                .pointer("/model/id")
                .and_then(Value::as_str)
                .or_else(|| value.get("model_id").and_then(Value::as_str)),
        ),
        "model deployment start" | "model deployment stop" => resource(
            "model deployment show",
            "model deployment config list",
            first,
        ),
        "model deployment config create" => {
            resource("model deployment show", "model deployment config list", id)
        }
        "model deployment create"
        | "model deployment activate"
        | "model deployment check"
        | "model deployment disable" => resource("model deployment list", "model list", first),
        "evaluation create" => resource("evaluation show", "evaluation list", id),
        "evaluation start" | "evaluation complete" | "evaluation fail" | "evaluation cancel" => {
            resource("evaluation show", "evaluation list", id.or(first))
        }
        "evaluation task create" | "evaluation task stop" => Some((
            "evaluation task show".into(),
            vec![],
            vec![
                "--kind".into(),
                i.option("kind")?.into(),
                "--task-id".into(),
                i.option("task-id")?.into(),
            ],
        )),
        "ref delete" | "dataset archive" => resource("", "ref trash", None),
        "ref move" | "ref restore" | "dataset restore" | "dataset create" => {
            resource("dataset show", "dataset list", first)
        }
        _ => None,
    }
}

pub fn error(mut value: Value, parsed: Option<&Parsed>) -> Value {
    if parsed.is_some_and(|p| p.view == View::Full) {
        return value;
    }
    let mut args = vec!["zygos".to_owned()];
    if let Some(i) = parsed.and_then(|p| p.invocation.as_ref()) {
        args.extend(i.spec.name.split_whitespace().map(str::to_owned));
    }
    args.extend(["--help".into(), "--view".into(), "full".into()]);
    value["_cli"] = json!({"help":args});
    value
}
