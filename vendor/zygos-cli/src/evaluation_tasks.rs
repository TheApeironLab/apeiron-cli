//! Fixed EvalScope gateway adapters. No direct Provider access or implicit service startup.
use crate::{
    args::Invocation,
    client::Client,
    error::{CliError, Result},
    io,
};
use reqwest::Method;
use serde_json::Value;

pub async fn execute(client: &Client, i: &Invocation) -> Option<Result<Value>> {
    let verb = i.spec.name.strip_prefix("evaluation task ")?;
    Some(run(client, i, verb).await)
}
async fn run(client: &Client, i: &Invocation, verb: &str) -> Result<Value> {
    let mut query = vec![];
    let mut body = None;
    let mut header = None;
    let mut method = Method::GET;
    let path = match verb {
        "config" => "config".to_owned(),
        "list" => {
            query = i.query();
            "tasks".to_owned()
        }
        "benchmarks" => {
            query.push(("all".into(), "true".into()));
            "eval/benchmarks".to_owned()
        }
        "metrics" => {
            query.push(("benchmark".into(), i.require("benchmark")?.into()));
            "eval/metrics".to_owned()
        }
        "create" | "show" | "stop" | "logs" | "report" => {
            let kind = i.require("kind")?;
            if !matches!(kind, "eval" | "perf") {
                return Err(CliError::usage("--kind must be eval or perf"));
            }
            let id = i.require("task-id")?;
            let suffix = id
                .strip_prefix(&format!("{kind}_"))
                .ok_or_else(|| CliError::usage("task ID prefix must match --kind"))?;
            let bytes = suffix.as_bytes();
            if bytes.len() != 36
                || !bytes.iter().enumerate().all(|(index, byte)| {
                    if [8, 13, 18, 23].contains(&index) {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
                    }
                })
                || bytes[14] != b'4'
                || !b"89ab".contains(&bytes[19])
            {
                return Err(CliError::usage("--task-id requires a canonical UUIDv4"));
            }
            let action = match verb {
                "create" => {
                    method = Method::POST;
                    let raw = io::read_request(i.require("input")?).await?;
                    io::raw_object(&raw)?;
                    body = Some(raw);
                    header = Some(id);
                    "invoke"
                }
                "stop" => {
                    method = Method::POST;
                    "stop"
                }
                "show" => "progress",
                "logs" => "log",
                _ => "report",
            };
            if verb != "create" {
                query.push(("task_id".into(), id.into()));
            }
            format!("{kind}/{action}")
        }
        _ => return Err(CliError::usage("Unknown evaluation task command")),
    };
    client
        .provider_json(
            &format!("evalscope-api/api/v1/{path}"),
            method,
            &query,
            body,
            header,
        )
        .await
}
