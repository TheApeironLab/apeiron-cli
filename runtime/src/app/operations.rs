//! Read-only views of Chentu's existing checkpoint. No owner lease, cluster
//! probing, source adoption, retries or alternative execution state machine.
use super::{process::Cancellation, *};
use std::{path::PathBuf, process::Command};

const HELP: &str = "apeiron operation <status|events> --instance <name> [--after <sequence>] [--json]\nUse --config /absolute/config.json instead of --instance.\nRequires the operation API advertised by its fixed Chentu package. Legacy checkpoints are not adopted.\n";

fn unsupported() -> Error {
    fail(
        "UNSUPPORTED_OPERATION_CHECKPOINT: the checkpoint has no supported Chentu operation API",
        2,
    )
}

fn query(record: &Value, checkpoint: &Path, action: &str, after: Option<u64>, cancel: &Cancellation) -> Result<Value> {
    let capability = record["appOperation"]
        .get("api")
        .unwrap_or(&record["appOperation"]["execution"]["api"]);
    if capability["schemaVersion"] != 1
        || !capability["operations"]
            .as_array()
            .is_some_and(|ops| ops.contains(&json!(action)))
        || !record["appOperation"]["facts"].is_object()
    {
        return Err(unsupported());
    }
    resume::validate_files(record, cancel)?;
    let root = Path::new(record["target"]["root"].as_str().ok_or_else(unsupported)?);
    let entry = root.join("deploy/helmfile/run.sh");
    for required in [&entry, &root.join("cli/src/chentu/application_operation.py")] {
        if record["files"].get(required.to_string_lossy().as_ref()).is_none() {
            return Err(unsupported());
        }
    }
    let mut command = if record["target"]["runner"] == "native" {
        let mut cmd = Command::new("bash");
        let env: BTreeMap<String, String> =
            serde_json::from_value(record["environment"].clone()).map_err(|_| unsupported())?;
        cmd.env_clear().envs(env);
        cmd
    } else if record["target"]["runner"] == "docker" {
        let mut cmd = Command::new("docker");
        cmd.args([
            "run",
            "--rm",
            "--pull=never",
            "--network",
            "none",
            "--read-only",
            "--entrypoint",
            "bash",
        ]);
        #[cfg(unix)]
        cmd.args([
            "--user",
            &format!("{}:{}", unsafe { libc::geteuid() }, unsafe { libc::getegid() }),
        ]);
        // The toolbox sees only immutable package/metadata paths, with neither
        // Docker socket nor network. It cannot reconnect to the application.
        for mount in [root, checkpoint.parent().ok_or_else(unsupported)?] {
            if mount.to_string_lossy().contains([':', ',']) || !mount.is_absolute() {
                return Err(unsupported());
            }
            cmd.arg("--mount").arg(format!(
                "type=bind,src={},dst={},readonly",
                mount.display(),
                mount.display()
            ));
        }
        cmd.arg(record["target"]["image"].as_str().ok_or_else(unsupported)?);
        cmd
    } else {
        return Err(unsupported());
    };
    command
        .arg(entry)
        .args(["operation", action, "--checkpoint"])
        .arg(checkpoint);
    if let Some(sequence) = after {
        command.args(["--after", &sequence.to_string()]);
    }
    let bytes = process::capture(&mut command, b"", Duration::from_secs(30), 1024 * 1024, cancel)?;
    let data: Value = serde_json::from_slice(&bytes).map_err(|_| fail("Invalid Chentu operation response", 502))?;
    let schema = format!("chentu.application-operation-{action}.v1");
    if data["schema"] != schema
        || data["readOnly"] != true
        || data["operationId"] != record["appOperation"]["facts"]["operationId"]
    {
        return Err(fail("Chentu operation response differs from its checkpoint", 409));
    }
    // Child diagnostics and arbitrary response fields are never forwarded.
    let keys: &[&str] = if action == "status" {
        &[
            "operationId",
            "phase",
            "applications",
            "protectionRef",
            "verificationRef",
            "applied",
            "configurationCommitted",
            "executorObserved",
            "executor",
            "observedPhase",
            "readOnly",
        ]
    } else {
        &["operationId", "events", "lastSequence", "readOnly"]
    };
    Ok(Value::Object(
        keys.iter()
            .filter_map(|key| data.get(*key).map(|v| ((*key).to_string(), v.clone())))
            .collect(),
    ))
}

pub fn run(args: &[String]) -> Result<()> {
    let (pos, options) = parse(args, &["instance", "config", "after"], &["help", "json"])?;
    if options.contains_key("help") || pos.is_empty() {
        println!("{HELP}");
        return Ok(());
    }
    if pos.len() != 1 || !["status", "events"].contains(&pos[0].as_str()) {
        return Err(fail("Use operation status or events", 2));
    }
    let after = options
        .get("after")
        .map(|v| v.parse::<u64>().map_err(|_| fail("Invalid --after sequence", 2)))
        .transpose()?;
    if after.is_some() && pos[0] != "events" {
        return Err(fail("--after applies only to operation events", 2));
    }
    let path: PathBuf = match (options.get("instance"), options.get("config")) {
        (Some(name), None) => instances::path_for(&instances::base()?, name)?,
        (None, Some(path)) => process::config_path(Some(path))?,
        _ => return Err(fail("Choose exactly one --instance or --config", 2)),
    };
    let result = observe(&path, &pos[0], after, &Cancellation::default())?;
    output(result, &options, &format!("apeiron.operation-{}.v1", pos[0]))
}

/// Shared transport for the CLI and wizard. Both read Chentu's same process API.
pub(crate) fn observe(path: &Path, action: &str, after: Option<u64>, cancel: &Cancellation) -> Result<Value> {
    let record = resume::recovery(path)?.ok_or_else(unsupported)?;
    let checkpoint = if resume::location(&path).exists() {
        resume::location(&path)
    } else {
        resume::completed_location(&path)
    };
    query(&record, &checkpoint, action, after, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_metadata_is_never_executed_or_adopted() {
        assert_eq!(
            query(
                &json!({}),
                Path::new("/absent"),
                "status",
                None,
                &Cancellation::default()
            )
            .unwrap_err()
            .code,
            2
        );
    }

    #[test]
    fn package_drift_is_rejected_before_process_execution() {
        let t = contracts::Temp::new();
        let entry = t.write("deploy/helmfile/run.sh", "exit 93\n");
        let record = json!({"appOperation":{"api":{"schemaVersion":1,"operations":["status"]},"facts":{}},
                            "files":{entry.to_str().unwrap():"wrong"}});
        assert_eq!(
            query(&record, Path::new("/absent"), "status", None, &Cancellation::default())
                .unwrap_err()
                .code,
            409
        );
    }

    #[test]
    fn status_and_cursor_events_use_the_same_read_only_process_without_cluster_or_lease() {
        let t = contracts::Temp::new();
        let entry = t.write("deploy/helmfile/run.sh", r#"set -eu
test "$1" = operation
test "$3" = --checkpoint
test ! -e "$4.installation-owner"
if [ "$2" = events ]; then
 test "$5" = --after; test "$6" = 7
 printf '%s' '{"schema":"chentu.application-operation-events.v1","operationId":"one","readOnly":true,"events":[],"lastSequence":7}'
else
 printf '%s' '{"schema":"chentu.application-operation-status.v1","operationId":"one","readOnly":true,"phase":"verifying","applied":false,"configurationCommitted":false,"secret":"must-not-print"}'
fi
"#);
        let api = t.write("cli/src/chentu/application_operation.py", "# fixed adapter\n");
        let mut record = json!({"target":{"runner":"native","root":t.0},"environment":{"PATH":"/usr/bin:/bin"},
            "appOperation":{"api":{"schemaVersion":1,"operations":["status","events"]},"facts":{"operationId":"one"}},
            "files":{entry.to_str().unwrap():resume::digest(&entry).unwrap(),api.to_str().unwrap():resume::digest(&api).unwrap()}});
        let checkpoint = t.0.join("checkpoint.json");
        let result = query(&record, &checkpoint, "status", None, &Cancellation::default()).unwrap();
        assert_eq!(result["phase"], "verifying");
        assert_eq!(result["configurationCommitted"], false);
        assert!(result.get("secret").is_none());
        let capability = record["appOperation"].as_object_mut().unwrap().remove("api").unwrap();
        record["appOperation"]["execution"] = json!({"api":capability});
        assert_eq!(
            query(&record, &checkpoint, "events", Some(7), &Cancellation::default()).unwrap()["lastSequence"],
            7
        );
        assert!(!checkpoint.exists());
    }
}
