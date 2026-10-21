//! IAM uses Apeiron's organization API, whose live directory checks remain the
//! authority. Its audience is already the source CLI SSO audience; no Task
//! broker or Keycloak Admin API credential belongs in this client.
use super::auth::{
    error::{Error, Result},
    output,
};
use super::sso;
use clap::{Arg, ArgAction, ArgMatches, Command};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    time::Duration,
};

fn group(name: &'static str) -> Command {
    Command::new(name)
        .subcommand_required(true)
        .arg_required_else_help(true)
}
fn mutation(name: &'static str, subject: bool) -> Command {
    let command = Command::new(name)
        .arg(
            Arg::new("file")
                .long("file")
                .required(true)
                .help("JSON request file, or - for stdin; passwords never go in argv"),
        )
        .arg(
            Arg::new("idempotency-key")
                .long("idempotency-key")
                .required(true)
                .help("UUID for this intent; reuse the same UUID and input when the result is unknown"),
        );
    if subject {
        command.arg(Arg::new("subject").required(true))
    } else {
        command
    }
}
fn parser() -> Command {
    Command::new("apeiron iam")
        .about("Manage the current SSO tenant through the existing IAM organization API")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(Arg::new("json").long("json").global(true).action(ArgAction::SetTrue))
        .arg(Arg::new("timeout-ms").long("timeout-ms").global(true).default_value("60000").value_parser(clap::value_parser!(u64).range(1..=120000)))
        .subcommand(group("organization").subcommand(Command::new("get")))
        .subcommand(group("app").subcommand(Command::new("list")))
        .subcommand(group("operation").subcommand(Command::new("list")))
        .subcommand(group("member")
            .subcommand(Command::new("list").arg(Arg::new("offset").long("offset").default_value("0").value_parser(clap::value_parser!(u64).range(0..=1000000))))
            .subcommand(mutation("create", false))
            .subcommand(mutation("update", true))
            .subcommand(mutation("reset-password", true)))
        .after_help("Sign in: apeiron sso login --server <APEIRON_URL>\nRead: apeiron iam member list --json\nWrite: apeiron iam member update <SUBJECT> --file access.json --idempotency-key <UUID> --json\nThe server derives the tenant from your SSO identity and checks live administrator rights. A pending write must be reconciled with its original idempotency key.")
}
fn body(args: &ArgMatches) -> Result<Value> {
    const LIMIT: u64 = 1024 * 1024;
    let file = args
        .get_one::<String>("file")
        .ok_or_else(|| Error::invalid("--file is required"))?;
    let mut data = Vec::new();
    if file == "-" {
        io::stdin().take(LIMIT + 1).read_to_end(&mut data)
    } else {
        std::fs::File::open(file)
            .map_err(|_| Error::invalid("Cannot open --file"))?
            .take(LIMIT + 1)
            .read_to_end(&mut data)
    }
    .map_err(|_| Error::invalid("Cannot read JSON input"))?;
    if data.len() > LIMIT as usize {
        return Err(Error::invalid("JSON input exceeds 1 MiB"));
    }
    let value: Value = serde_json::from_slice(&data)
        .map_err(|_| Error::invalid("Input must be a JSON object matching the IAM API contract"))?;
    if !value.is_object() {
        return Err(Error::invalid("Input must be a JSON object"));
    }
    Ok(value)
}
async fn execute(args: &ArgMatches) -> Result<Value> {
    let (resource, sub) = args.subcommand().ok_or_else(Error::internal)?;
    let (action, input) = sub.subcommand().ok_or_else(Error::internal)?;
    let write = matches!(action, "create" | "update" | "reset-password");
    // Validate local input before login, network access, or any mutation.
    let payload = if write { Some(body(input)?) } else { None };
    let key = if write {
        let key = input.get_one::<String>("idempotency-key").ok_or_else(Error::internal)?;
        if key.len() != 36
            || !key.bytes().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
        {
            return Err(Error::invalid("--idempotency-key must be a UUID"));
        }
        Some(key)
    } else {
        None
    };
    let timeout = Duration::from_millis(*args.get_one::<u64>("timeout-ms").ok_or_else(Error::internal)?);
    let http = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| Error::internal())?;
    let (mut url, token) = sso::platform_credential(&http, timeout).await?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| Error::config("Invalid SSO platform API URL"))?;
        segments.push("organization");
        if resource == "member" {
            segments.push("members");
            if action != "list" && action != "create" {
                let subject = input.get_one::<String>("subject").ok_or_else(Error::internal)?;
                // Dot segments are normalized by URL libraries; never let a
                // caller turn a member ID into a different API route.
                if subject.is_empty() || matches!(subject.as_str(), "." | "..") {
                    return Err(Error::invalid("Invalid member subject"));
                }
                segments.push(subject);
            }
            if action == "reset-password" {
                segments.push("password");
            }
        } else if resource == "operation" {
            segments.push("operations");
        }
    }
    if resource == "member" && action == "list" {
        url.query_pairs_mut().append_pair(
            "offset",
            &input.get_one::<u64>("offset").ok_or_else(Error::internal)?.to_string(),
        );
    }
    let method = if action == "update" {
        Method::PATCH
    } else if write {
        Method::POST
    } else {
        Method::GET
    };
    let mut request = http.request(method, url).bearer_auth(token).timeout(timeout);
    if let Some(key) = key {
        request = request.header("Idempotency-Key", key);
    }
    if let Some(payload) = payload {
        request = request.json(&payload);
    }
    // Never retry management writes automatically: directory operations can
    // commit even when a response is lost. The API owns idempotent recovery.
    let response = request.send().await.map_err(|_| transport_error(write))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        if write && !(400..500).contains(&status) {
            return Err(Error::unknown("IAM write result is unknown; inspect operation list and retry the same intent with its original idempotency key", Some(status)));
        }
        let (code, message, exit) = match status {
            401 => ("SSO_LOGIN_REQUIRED", "Run apeiron sso login --server <APEIRON_URL>", 3),
            403 => (
                "FORBIDDEN",
                "Current SSO identity has no live IAM administrator permission for this tenant",
                4,
            ),
            404 => ("NOT_FOUND", "IAM tenant or member was not found", 5),
            409 => (
                "CONFLICT",
                "IAM operation conflicts with existing state; inspect operation list",
                6,
            ),
            400 | 422 => ("INVALID_ARGUMENT", "Input does not match the IAM API contract", 2),
            429 => ("RATE_LIMITED", "IAM request limit reached", 7),
            _ => ("IAM_UNAVAILABLE", "IAM API is unavailable", 8),
        };
        let mut error = Error::new(code, message, exit);
        error.status = Some(status);
        return Err(error);
    }
    let data: Value = response.json().await.map_err(|_| transport_error(write))?;
    if !data.is_object() {
        return Err(transport_error(write));
    }
    if resource == "app" {
        let apps = data
            .get("apps")
            .filter(|v| v.is_array())
            .ok_or_else(|| transport_error(false))?;
        return Ok(json!({"items":apps}));
    }
    Ok(data)
}
fn transport_error(write: bool) -> Error {
    if write {
        Error::unknown("IAM write result is unknown; inspect operation list and retry the same intent with its original idempotency key", None)
    } else {
        Error::new(
            "IAM_UNAVAILABLE",
            "IAM API could not be reached or returned an incompatible response",
            8,
        )
    }
}
pub(super) fn run(argv: &[OsString]) -> i32 {
    let json = argv.iter().any(|a| a == "--json");
    let args = match parser()
        .try_get_matches_from(std::iter::once(OsString::from("apeiron iam")).chain(argv.iter().cloned()))
    {
        Ok(args) => args,
        Err(e)
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            return if write!(io::stdout().lock(), "{e}").is_ok() {
                0
            } else {
                1
            }
        }
        Err(_) => return output::failure(Error::invalid("Invalid IAM arguments; run apeiron iam --help"), json),
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(_) => return output::failure(Error::internal(), json),
    };
    let result = runtime.block_on(execute(&args));
    runtime.shutdown_timeout(Duration::ZERO);
    match result {
        Ok(data) => output::success(data, None, json).map_or_else(|e| output::failure(e, json), |_| 0),
        Err(e) => output::failure(e, json),
    }
}
