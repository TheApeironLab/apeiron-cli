use crate::{
    args::Invocation,
    client::{Client, read_json},
    error::{CliError, Result},
    io::{self, Output},
};
use reqwest::{
    Body,
    multipart::{Form, Part},
};
use serde_json::{Value, json};
use std::collections::HashSet;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

fn nullable(i: &Invocation, name: &str) -> Value {
    json!(i.option(name))
}
fn stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}
fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name]
        .as_str()
        .ok_or_else(|| CliError::integrity(format!("API response is missing {name}")))
}
fn digest(text: &str) -> Result<&str> {
    if text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(text)
    } else {
        Err(CliError::new(
            "validation_error",
            "expected a lowercase 64-character digest",
        ))
    }
}

/// Execute a parsed command. None means the raw export stream was written to output.
pub async fn execute(
    client: &Client,
    i: &Invocation,
    output: &mut Output,
    compact: bool,
) -> Result<Option<Value>> {
    if let Some(result) = crate::evaluation_tasks::execute(client, i).await {
        return result.map(Some);
    }
    if let Some(result) = crate::baml::execute(client, i).await {
        return result.map(Some);
    }
    if let Some(result) = crate::dataset_crud::execute(client, i).await {
        return result.map(Some);
    }
    if let Some(result) = crate::lifecycle::execute(client, i).await {
        return result.map(Some);
    }
    let op = i.spec.operation.as_str();
    let p = &i.positionals;
    let value = match i.spec.name.as_str() {
        "dataset ingest" => {
            let source = io::input(&p[0]).await?;
            let mut form = Form::new();
            for (flag, wire) in [
                ("ref", "ref"),
                ("expected-ref-version", "expected_ref_version"),
                ("message", "message"),
            ] {
                if let Some(value) = i.option(flag) {
                    form = form.text(wire, value.to_owned());
                }
            }
            let part = Part::stream(Body::wrap_stream(ReaderStream::new(source)))
                .file_name("canonical.jsonl")
                .mime_str("application/x-ndjson")?;
            read_json(
                client
                    .send(
                        client
                            .request(op, &[], &[])?
                            .multipart(form.part("file", part)),
                    )
                    .await?,
            )
            .await?
        }
        "dataset show" | "dataset records" | "dataset audit" => {
            client
                .json(op, &[("ref_or_version", &p[0])], &i.query(), None)
                .await?
        }
        "dataset business-info" => {
            client
                .json(op, &[("dataset_version", digest(&p[0])?)], &[], None)
                .await?
        }
        "dataset baml-preview" => {
            let body = io::read_request(i.require("file")?).await?;
            io::raw_object(&body)?;
            client
                .json(op, &[("dataset_version", digest(&p[0])?)], &[], Some(body))
                .await?
        }
        "dataset export" => return export(client, i, output).await,
        "converter list" | "transform list" | "ref list" | "ref trash" | "model list" => {
            client.json(op, &[], &i.query(), None).await?
        }
        "converter show" | "ref show" => client.json(op, &[("name", &p[0])], &[], None).await?,
        "transform run" => {
            let inputs = i.options.get("input").cloned().unwrap_or_default();
            if inputs.is_empty() {
                return Err(CliError::usage("transform run requires --input"));
            }
            let params = io::raw_object(i.option("params").unwrap_or("{}"))?;
            let body = format!(
                "{{\"inputs\":{},\"params\":{},\"ref\":{},\"expected_ref_version\":{},\"message\":{}}}",
                stringify(&json!(inputs)),
                params,
                nullable(i, "ref"),
                nullable(i, "expected-ref-version"),
                nullable(i, "message")
            );
            client.json(op, &[("name", &p[0])], &[], Some(body)).await?
        }
        "ref move" | "ref delete" | "ref restore" => {
            let supplied = i.option("expected-version");
            if supplied.is_some() == i.flag("use-current") {
                return Err(CliError::usage(
                    "provide exactly one of --expected-version or --use-current",
                ));
            }
            let expected = if let Some(value) = supplied {
                json!(digest(value)?)
            } else {
                let current = if i.spec.name == "ref move" {
                    active_ref(client, &p[0]).await?
                } else {
                    current_ref(client, &p[0], i.spec.name == "ref restore").await?
                };
                match current {
                    Some(value) => json!(digest(field(&value, "version")?)?),
                    None if i.spec.name == "ref move" => Value::Null,
                    None => return Err(CliError::new("not_found", "ref was not found")),
                }
            };
            let body = if i.spec.name == "ref move" {
                json!({"new_version":digest(&p[1])?,"expected_version":expected,"message":nullable(i,"message")})
            } else {
                json!({"expected_version":expected})
            };
            client
                .json(op, &[("name", &p[0])], &[], Some(stringify(&body)))
                .await?
        }
        "lineage show" => {
            client
                .json(op, &[("ref_or_version", &p[0])], &i.query(), None)
                .await?
        }
        "model show" | "model versions" => {
            client
                .json(op, &[("model_id", &p[0])], &i.query(), None)
                .await?
        }
        "model registration inspect" | "model registration commit" => {
            let request = io::read_request(i.require("input")?).await?;
            io::raw_object(&request)?;
            if i.spec.name == "model registration inspect" {
                let plan = client.json(op, &[], &[], Some(request)).await?;
                if let Some(path) = i.option("output") {
                    let mut file = io::AtomicFile::new(path)?;
                    file.write(&io::encoded(&plan, compact)?)?;
                    json!({"path":file.commit()?,"plan_profile":plan["plan_profile"],"registration_digest":plan["registration_digest"]})
                } else {
                    plan
                }
            } else {
                let expected = digest(i.require("expected-digest")?)?;
                let shape: Value = serde_json::from_str(&request)
                    .map_err(|_| CliError::usage("invalid registration JSON"))?;
                let body = format!(
                    "{{\"request\":{request},\"expected_registration_digest\":{}}}",
                    stringify(&json!(expected))
                );
                if shape["target"]["kind"] == "existing_model" {
                    client
                        .json(
                            "registerModelVersionV2",
                            &[("model_id", field(&shape["target"], "model_id")?)],
                            &[],
                            Some(body),
                        )
                        .await?
                } else {
                    client.json(op, &[], &[], Some(body)).await?
                }
            }
        }
        "model deployment list" => {
            client
                .json(op, &[("version_id", &p[0])], &i.query(), None)
                .await?
        }
        "model deployment activate" | "model deployment check" | "model deployment disable" => {
            client
                .json(
                    op,
                    &[("version_id", &p[0]), ("deployment_id", &p[1])],
                    &[],
                    Some("{}".into()),
                )
                .await?
        }
        _ => return Err(CliError::integrity("command has no implementation")),
    };
    Ok(Some(value))
}

async fn active_ref(client: &Client, name: &str) -> Result<Option<Value>> {
    match client.json("getRefV2", &[("name", name)], &[], None).await {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.exit_code == 3 => Ok(None),
        Err(e) => Err(e),
    }
}
async fn deleted_ref(client: &Client, name: &str) -> Result<Option<Value>> {
    let mut seen = HashSet::new();
    let mut query = vec![("limit".into(), "100".into())];
    loop {
        let page = client.json("listDeletedRefsV2", &[], &query, None).await?;
        let items = page["items"]
            .as_array()
            .ok_or_else(|| CliError::integrity("invalid deleted-ref page"))?;
        if let Some(value) = items.iter().find(|v| v["name"] == name) {
            return Ok(Some(value.clone()));
        }
        match page["next_cursor"].as_str() {
            None if page["next_cursor"].is_null() => return Ok(None),
            Some(cursor) if seen.insert(cursor.to_owned()) => {
                query = vec![
                    ("limit".into(), "100".into()),
                    ("cursor".into(), cursor.to_owned()),
                ];
            }
            _ => {
                return Err(CliError::integrity(
                    "invalid or repeated deleted-ref cursor",
                ));
            }
        }
    }
}
pub(crate) async fn current_ref(
    client: &Client,
    name: &str,
    deleted_first: bool,
) -> Result<Option<Value>> {
    if deleted_first {
        if let Some(value) = deleted_ref(client, name).await? {
            return Ok(Some(value));
        }
        active_ref(client, name).await
    } else {
        if let Some(value) = active_ref(client, name).await? {
            return Ok(Some(value));
        }
        deleted_ref(client, name).await
    }
}

async fn export(client: &Client, i: &Invocation, output: &mut Output) -> Result<Option<Value>> {
    if i.flag("inspect") && (i.option("output").is_some() || i.option("accept-fidelity").is_some())
    {
        return Err(CliError::usage(
            "--inspect cannot be combined with --output or --accept-fidelity",
        ));
    }
    let converter = json!(i.option("converter").unwrap_or("canonical-jsonl"));
    let options = io::raw_object(i.option("options").unwrap_or("{}"))?;
    let body = format!("{{\"converter\":{converter},\"options\":{options}}}");
    let plan = client
        .json(
            "inspectDatasetExportV2",
            &[("ref_or_version", &i.positionals[0])],
            &[],
            Some(body),
        )
        .await?;
    if i.flag("inspect") {
        return Ok(Some(plan));
    }
    // Resolve once and pin the export to the inspected immutable dataset version.
    let version = digest(field(&plan, "dataset_version")?)?;
    let media_type = field(&plan, "media_type")?;
    let accepted = nullable(i, "accept-fidelity");
    if let Some(value) = i.option("accept-fidelity") {
        digest(value)?;
    }
    // The API owns digest validation and semantic-loss approval. Never recompute identity here.
    let body = format!(
        "{{\"converter\":{converter},\"options\":{options},\"accepted_fidelity_digest\":{accepted}}}"
    );
    if i.option("output").is_none() {
        io::reject_binary_tty(media_type)?;
    }
    let mut response = client
        .send(
            client
                .request("exportDatasetV2", &[("dataset_version", version)], &[])?
                .header("Content-Type", "application/json")
                .body(body),
        )
        .await?;
    if response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        != Some(media_type)
    {
        return Err(CliError::integrity(
            "export media type changed after inspection",
        ));
    }
    let mut file = i.option("output").map(io::AtomicFile::new).transpose()?;
    while let Some(chunk) = response.chunk().await? {
        if let Some(file) = file.as_mut() {
            file.write(&chunk)?;
        } else {
            output.write_all(&chunk).await?;
        }
    }
    if let Some(file) = file {
        Ok(Some(json!({"path":file.commit()?,"plan":plan})))
    } else {
        output.flush().await?;
        Ok(None)
    }
}
