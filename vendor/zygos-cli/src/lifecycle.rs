//! HTTP-only bridges for Studio sessions, artifacts, deployments and evaluation records.
use crate::{args::Invocation, client::Client, error::Result, io};
use serde_json::{Value, json};

pub async fn execute(client: &Client, i: &Invocation) -> Option<Result<Value>> {
    let op = i.spec.operation.as_str();
    let p = &i.positionals;
    let name = i.spec.name.as_str();
    let params = match name {
        "model deployment config list"
        | "model deployment config create"
        | "model weights bind-base"
        | "model weights bindings"
        | "model weights list"
        | "model weights inspect"
        | "model weights commit"
        | "training bundle inspect"
        | "training bundle list"
        | "training bundle create"
        | "training resources"
        | "training models"
        | "training run list"
        | "training run create" => {
            vec![]
        }
        "model deployment show"
        | "model deployment start"
        | "model deployment stop"
        | "model deployment capabilities"
        | "model deployment test" => {
            vec![("deployment_id", p[0].as_str())]
        }
        "training bundle show" => vec![("bundle_id", p[0].as_str())],
        "training run show" | "training run logs" | "training run stop" => {
            vec![("run_id", p[0].as_str())]
        }
        "training session create" => return Some(create_session(client, i).await),
        "training node list"
        | "training node create"
        | "training node show"
        | "training node check"
        | "training node connect"
        | "training node disconnect" => vec![],
        "training session list"
        | "model artifact list"
        | "model artifact import"
        | "evaluation list"
        | "evaluation create" => vec![],
        "training session show" | "training session outputs" | "training session close" => {
            vec![("session_id", p[0].as_str())]
        }
        "model artifact show" => vec![("artifact_id", p[0].as_str())],
        "model artifact import-status" => vec![("import_id", p[0].as_str())],
        "model version show" | "model deployment create" => vec![("version_id", p[0].as_str())],
        "evaluation show"
        | "evaluation start"
        | "evaluation complete"
        | "evaluation fail"
        | "evaluation cancel" => vec![("run_id", p[0].as_str())],
        _ => return None,
    };
    Some(
        async {
            let body = match name {
                "training run stop"
                | "training session close"
                | "evaluation start"
                | "training node disconnect" => Some("{}".into()),
                "model deployment test"
                | "model deployment start"
                | "model deployment stop"
                | "model deployment config create"
                | "model weights bind-base"
                | "model weights inspect"
                | "model weights commit"
                | "training bundle inspect"
                | "training bundle create"
                | "training run create"
                | "training node create"
                | "training node check"
                | "training node connect"
                | "model artifact import"
                | "model deployment create"
                | "evaluation create"
                | "evaluation complete"
                | "evaluation fail"
                | "evaluation cancel" => {
                    let raw = io::read_request(i.require("input")?).await?;
                    io::raw_object(&raw)?;
                    Some(raw)
                }
                _ => None,
            };
            let query = if body.is_none() { i.query() } else { vec![] };
            client.json(op, &params, &query, body).await
        }
        .await,
    )
}

async fn create_session(client: &Client, i: &Invocation) -> Result<Value> {
    let options = io::raw_object(i.option("options").unwrap_or("{}"))?;
    let plan = client
        .json(
            "inspectDatasetExportV2",
            &[("ref_or_version", &i.positionals[0])],
            &[],
            Some(format!(
                "{{\"converter\":\"ms-swift\",\"options\":{options}}}"
            )),
        )
        .await?;
    let version = plan["dataset_version"]
        .as_str()
        .ok_or_else(|| crate::CliError::integrity("export plan missing dataset_version"))?;
    // The server owns fidelity validation; do not implicitly accept a semantic loss.
    let display_ref = if i.positionals[0] == version {
        Value::Null
    } else {
        json!(i.positionals[0])
    };
    let body = format!(
        "{{\"dataset_version\":{},\"display_ref\":{},\"converter\":\"ms-swift\",\"options\":{},\"accepted_fidelity_digest\":{}}}",
        json!(version),
        display_ref,
        options,
        json!(i.option("accept-fidelity"))
    );
    let body = if let Some(request_id) = i.option("request-id") {
        format!("{{\"request_id\":{},{}", json!(request_id), &body[1..])
    } else {
        body
    };
    client.json(&i.spec.operation, &[], &[], Some(body)).await
}
