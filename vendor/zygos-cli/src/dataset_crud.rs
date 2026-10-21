use crate::{
    args::Invocation,
    client::{Client, read_json},
    commands::current_ref,
    error::{CliError, Result},
    io,
};
use reqwest::Body;
use serde_json::{Value, json};
use tokio_util::io::ReaderStream;

pub async fn execute(client: &Client, i: &Invocation) -> Option<Result<Value>> {
    if !matches!(
        i.spec.name.as_str(),
        "dataset create"
            | "dataset list"
            | "dataset import"
            | "dataset archive"
            | "dataset restore"
            | "dataset record add"
            | "dataset record show"
            | "dataset record update"
            | "dataset record delete"
    ) {
        return None;
    }
    Some(run(client, i).await)
}
async fn run(client: &Client, i: &Invocation) -> Result<Value> {
    let op = &i.spec.operation;
    let p = &i.positionals;
    let query = i
        .query()
        .into_iter()
        .filter(|(key, _)| key != "file")
        .collect::<Vec<_>>();
    match i.spec.name.as_str() {
        "dataset create" => {
            client
                .json(
                    op,
                    &[],
                    &[],
                    Some(json!({"name":p[0],"message":i.option("message")}).to_string()),
                )
                .await
        }
        "dataset list" => client.json(op, &[], &query, None).await,
        "dataset archive" | "dataset restore" => {
            let expected = if let Some(value) = i.option("expected-version") {
                value.to_owned()
            } else {
                let current = current_ref(client, &p[0], i.spec.name == "dataset restore")
                    .await?
                    .ok_or_else(|| CliError::new("not_found", "dataset name was not found"))?;
                current["version"]
                    .as_str()
                    .ok_or_else(|| CliError::integrity("missing ref version"))?
                    .to_owned()
            };
            client
                .json(
                    op,
                    &[("name", &p[0])],
                    &[],
                    Some(json!({"expected_version":expected}).to_string()),
                )
                .await
        }
        "dataset import" | "dataset record add" | "dataset record update" => {
            let input = io::input(i.require("file")?).await?;
            let params = if i.spec.name == "dataset record update" {
                vec![
                    ("ref_or_version", p[0].as_str()),
                    ("record_id", p[1].as_str()),
                ]
            } else {
                vec![("name", p[0].as_str())]
            };
            let content_type = if i.spec.name == "dataset import" {
                "application/x-ndjson"
            } else {
                "application/json"
            };
            // Send original bytes; validation belongs to the API and returns corrective guidance.
            read_json(
                client
                    .send(
                        client
                            .request(op, &params, &query)?
                            .header("content-type", content_type)
                            .body(Body::wrap_stream(ReaderStream::new(input))),
                    )
                    .await?,
            )
            .await
        }
        "dataset record show" | "dataset record delete" => {
            client
                .json(
                    op,
                    &[("ref_or_version", &p[0]), ("record_id", &p[1])],
                    &query,
                    None,
                )
                .await
        }
        _ => unreachable!(),
    }
}
