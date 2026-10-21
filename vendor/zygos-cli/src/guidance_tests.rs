use super::*;
use crate::parse;

fn parsed(args: &[&str]) -> Parsed {
    parse(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap()
}
fn next(value: &Value) -> Parsed {
    let args: Vec<_> = value["_cli"]
        .get("full")
        .or_else(|| value["_cli"].get("help"))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|s| s.as_str().unwrap().to_owned())
        .collect();
    parse(&args).unwrap()
}

#[test]
fn even_empty_unabridged_and_selected_results_offer_navigation() {
    for args in [
        vec!["model", "list"],
        vec!["model", "list", "--fields", "id"],
    ] {
        let p = parsed(&args);
        let empty = json!({"items":[],"next_cursor":null});
        let result = attach(empty.clone(), &empty, &p);
        assert_eq!(result["_cli"]["fields"], json!(["@groups"]));
        assert_eq!(result["_cli"].as_object().unwrap().len(), 2);
        assert_eq!(result["_cli"]["full"], "--view full");
    }
    let p = parsed(&["model", "list", "--fields", "id"]);
    let original = json!({"items":[{"id":"a","name":"model","files":[]},{"id":"b","optional":true}],"next_cursor":"opaque"});
    let selected = json!({"items":[{"id":"a"},{"id":"b"}],"next_cursor":"opaque"});
    let result = attach(selected, &original, &p);
    assert_eq!(
        result["_cli"]["fields"],
        json!(["@groups", "@basic", "@storage", "@other"])
    );
    assert_eq!(result["next_cursor"], "opaque");
    assert!(result["items"][0].get("_cli").is_none());
}

#[test]
fn mutation_follow_up_preserves_service_and_literal_resource_id() {
    let id = "$(touch /tmp/not-executed) -h";
    let p = parsed(&[
        "baml",
        "update",
        "--base-url",
        "http://localhost:9999/prefix",
        "--",
        id,
    ]);
    let value = json!({"id":id});
    let result = attach(value.clone(), &value, &p);
    let follow = next(&result);
    assert_eq!(follow.base_url, p.base_url);
    assert_eq!(follow.view, View::Full);
    let invocation = follow.invocation.unwrap();
    assert_eq!(invocation.spec.name, "baml show");
    assert_eq!(invocation.positionals, vec![id]);
}

#[test]
fn every_command_points_to_read_only_query_or_offline_help() {
    for spec in crate::args::catalog() {
        let mut args: Vec<_> = spec.name.split_whitespace().map(str::to_owned).collect();
        args.extend(spec.positionals.iter().map(|_| "exact-id".into()));
        if spec.name.starts_with("evaluation task ") {
            args.extend([
                "--kind".into(),
                "eval".into(),
                "--task-id".into(),
                "eval_12345678-1234-4234-8234-123456789abc".into(),
            ]);
            // Only the lifecycle task commands accept these options.
            if !matches!(
                spec.name.as_str(),
                "evaluation task create"
                    | "evaluation task stop"
                    | "evaluation task show"
                    | "evaluation task logs"
                    | "evaluation task report"
            ) {
                args.truncate(spec.name.split_whitespace().count());
            }
        }
        let p = parse(&args).unwrap();
        let value = json!({"id":"saved-id","model":{"id":"model-id"},"status":"prepared"});
        let result = attach(value.clone(), &value, &p);
        if can_select(&spec.name) {
            assert_eq!(result["_cli"]["full"], "--view full");
            assert_eq!(result["status"], "prepared");
            continue;
        }
        let follow = next(&result);
        if result["_cli"].get("full").is_some() {
            assert!(
                can_select(&follow.invocation.unwrap().spec.name),
                "{}",
                spec.name
            );
        } else {
            assert!(follow.help.is_some(), "{}", spec.name);
        }
        assert_eq!(result["status"], "prepared");
    }
}

#[test]
fn task_creation_guidance_pins_task_and_never_repeats_creation() {
    let id = "eval_12345678-1234-4234-8234-123456789abc";
    let p = parsed(&[
        "evaluation",
        "task",
        "create",
        "--kind",
        "eval",
        "--task-id",
        id,
        "--input",
        "-",
    ]);
    let value = json!({"status":"running"});
    let result = attach(value.clone(), &value, &p);
    let i = next(&result).invocation.unwrap();
    assert_eq!(i.spec.name, "evaluation task show");
    assert_eq!(i.option("task-id"), Some(id));
    assert_eq!(i.option("kind"), Some("eval"));
    assert!(i.option("input").is_none());
}

#[test]
fn full_output_remains_exact_and_large_field_discovery_is_bounded() {
    let p = parsed(&["model", "list", "--view", "full"]);
    let value = json!([{"id":"a"}]);
    assert_eq!(attach(value.clone(), &value, &p), value);
    let p = parsed(&["model", "list"]);
    let result = attach(value.clone(), &value, &p);
    assert_eq!(result["items"], value);
    assert_eq!(result["_cli"]["fields"], json!(["@groups", "@basic"]));
    let object: serde_json::Map<String, Value> =
        (0..100).map(|i| (format!("field_{i}"), json!(i))).collect();
    let value = json!(object);
    let result = attach(value.clone(), &value, &p);
    assert_eq!(result["_cli"]["fields"], json!(["@groups", "@other"]));
    assert_eq!(result["_cli"].as_object().unwrap().len(), 2);
    let discovery = crate::field_groups::discover(&value);
    assert_eq!(discovery["fields"]["@other"].as_array().unwrap().len(), 100);
}
