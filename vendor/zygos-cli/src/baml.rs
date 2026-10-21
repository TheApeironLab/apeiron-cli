use crate::{
    args::Invocation,
    client::Client,
    error::{CliError, Result},
    io,
};
use serde_json::{Value, json};
pub async fn execute(client: &Client, i: &Invocation) -> Option<Result<Value>> {
    if !i.spec.name.starts_with("baml ") {
        return None;
    }
    Some(call(client, i).await)
}
async fn call(client: &Client, i: &Invocation) -> Result<Value> {
    let name = i.spec.name.as_str();
    let params = if name == "baml list" || name == "baml create" {
        vec![]
    } else {
        vec![("template_id", i.positionals[0].as_str())]
    };
    let body = match name {
        "baml create" | "baml update" => {
            let raw = io::read_request(i.require("file")?).await?;
            io::raw_object(&raw)?;
            Some(raw)
        }
        "baml publish" | "baml archive" | "baml restore" => {
            let revision = i
                .require("expected-revision")?
                .parse::<u32>()
                .map_err(|_| {
                    CliError::usage("--expected-revision must be a non-negative integer")
                })?;
            let mut body = json!({"expected_revision":revision});
            if name == "baml publish" {
                body["dataset_version"] = json!(i.require("dataset-version")?);
            }
            Some(body.to_string())
        }
        _ => None,
    };
    let query = if name == "baml list" {
        i.query()
    } else {
        vec![]
    };
    client.json(&i.spec.operation, &params, &query, body).await
}
