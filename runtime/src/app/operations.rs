//! Thin transport for Chentu's existing checkpoint. Observation is read-only;
//! start can only submit an explicitly prepared operation, never adopt a source.
use super::{process::Cancellation, *};
use std::{path::PathBuf, process::Command};

const HELP: &str = "apeiron operation <status|events> --instance <name> [--after <sequence>] [--json]\napeiron operation <start|resume|commit> --instance <name> [--json]\nUse --config /absolute/config.json instead of --instance.\nRequires the operation API advertised by its fixed Chentu package. Legacy checkpoints are not adopted.\nstart submits only an already prepared native operation; it does not prepare a plan or commit configuration.\nresume requalifies only a supported committed migration continuation; it never steals an active executor.\ncommit records the prepared instance configuration only after fixed application verification and executor completion.\n";

fn unsupported() -> Error {
    fail(
        "UNSUPPORTED_OPERATION_CHECKPOINT: the checkpoint has no supported Chentu operation API",
        2,
    )
}

fn command(
    record: &Value,
    checkpoint: &Path,
    action: &str,
    after: Option<u64>,
    cancel: &Cancellation,
) -> Result<Command> {
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
    Ok(command)
}

fn query(record: &Value, checkpoint: &Path, action: &str, after: Option<u64>, cancel: &Cancellation) -> Result<Value> {
    let mut command = command(record, checkpoint, action, after, cancel)?;
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
    if pos.len() != 1 || !["status", "events", "start", "resume", "commit"].contains(&pos[0].as_str()) {
        return Err(fail("Use operation status, events, start, resume or commit", 2));
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
    let cancel = Cancellation::default();
    let result = match pos[0].as_str() {
        "start" => start(&path, &cancel)?,
        "resume" => submit(&path, "resume", &cancel)?,
        "commit" => commit(&path, &cancel)?,
        _ => observe(&path, &pos[0], after, &cancel)?,
    };
    output(result, &options, &format!("apeiron.operation-{}.v1", pos[0]))
}

pub(crate) fn commit(path: &Path, cancel: &Cancellation) -> Result<Value> {
    let record = resume::recovery(path)?.ok_or_else(unsupported)?;
    let intent = &record["appOperation"]["configurationCommit"];
    if record["target"]["runner"] != "native"
        || intent["schemaVersion"] != 1
        || record["appOperation"]["facts"]["phase"] != "succeeded"
        || record["appOperation"]["applied"] != true
        || record["cliSha256"] != resume::cli_identity()?
    {
        return Err(unsupported());
    }
    let _owner = resume::guard(path)?;
    if resume::recovery(path)?.as_ref() != Some(&record) {
        return Err(fail("Verified operation changed before configuration commit", 409));
    }
    config::location(path)?;
    let token = record["appOperation"]["lockToken"].as_str().ok_or_else(unsupported)?;
    if !config::matches(r"^apeiron-app-[A-Za-z0-9-]{1,128}$", token) {
        return Err(unsupported());
    }
    let _configuration = applications::lock_config(path, token)?;
    let directory = Path::new(record["directory"].as_str().ok_or_else(unsupported)?);
    let desired_path = directory.join("desired-configuration.json");
    if intent["desiredRef"] != json!(desired_path) {
        return Err(unsupported());
    }
    let mut bytes = Vec::new();
    resume::private_file(&desired_path, false)?
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| fail("Cannot read prepared configuration", 409))?;
    if bytes.len() > 16384 || record["files"][desired_path.to_string_lossy().as_ref()] != config::hash(&bytes) {
        return Err(fail("Prepared configuration changed", 409));
    }
    // The proposal is complete private configuration, never a patch inheriting
    // values from a concurrent edit. Use the existing config schema/normalizer.
    let desired = config::read(&desired_path)?["config"].clone();
    let previous_path = directory.join("previous-configuration.json");
    if intent["beforeRef"] != json!(previous_path) {
        return Err(unsupported());
    }
    let mut previous_bytes = Vec::new();
    resume::private_file(&previous_path, false)?
        .take(16385)
        .read_to_end(&mut previous_bytes)
        .map_err(|_| fail("Cannot read previous configuration", 409))?;
    if previous_bytes.len() > 16384
        || record["files"][previous_path.to_string_lossy().as_ref()] != config::hash(&previous_bytes)
        || intent["beforeFileSha256"] != config::hash(&previous_bytes)
    {
        return Err(fail("Previous configuration evidence changed", 409));
    }
    let previous = config::read(&previous_path)?["config"].clone();
    configuration_transition(&record, &previous, &desired)?;
    let desired_bytes = serde_json::to_vec(&desired).map_err(|_| unsupported())?;
    let desired_sha = config::hash(&desired_bytes);
    let before = intent["beforeFileSha256"].as_str().ok_or_else(unsupported)?;
    if !config::matches(r"^[a-f0-9]{64}$", before) {
        return Err(unsupported());
    }
    let unchanged = || -> Result<()> {
        let current = resume::digest(path)?;
        if current != before && current != desired_sha {
            return Err(fail("Instance configuration changed since preparation", 409));
        }
        Ok(())
    };
    unchanged()?;
    let status = observe(path, "status", None, cancel)?;
    if status["phase"] != "succeeded"
        || status["applied"] != true
        || status["verificationRef"].as_str().is_none_or(str::is_empty)
        || status["executorObserved"] != true
        || status["executor"]["ActiveState"] != "inactive"
        || status["executor"]["ExecMainStatus"] != "0"
    {
        return Err(fail(
            "Application verification and executor completion are required",
            409,
        ));
    }
    if record["succeeded"] == true && record["finished"] == true {
        if resume::digest(path)? != desired_sha || status["configurationCommitted"] != true {
            return Err(fail("Completed configuration differs from its operation", 409));
        }
    } else {
        unchanged()?;
        resume::write(path, &desired)?;
        let mut latest = record.clone();
        latest["configHash"] = json!(resume::config_hash(&desired));
        resume::write(&resume::location(path), &latest)?;
        // A crash after config write retries only this metadata commit; the
        // same existing completed checkpoint/archive mechanism is authoritative.
        resume::complete(path, directory)?;
    }
    Ok(json!({"operationId":status["operationId"],"phase":"succeeded","configurationCommitted":true}))
}

fn configuration_transition(record: &Value, before: &Value, desired: &Value) -> Result<()> {
    let error = || {
        fail(
            "Prepared configuration changes settings outside its application intent",
            409,
        )
    };
    let action = record["appOperation"]["request"]["action"].as_str().ok_or_else(error)?;
    let apps = |value: &Value| -> Result<std::collections::BTreeSet<String>> {
        value
            .as_array()
            .ok_or_else(error)?
            .iter()
            .map(|app| app.as_str().map(str::to_owned).ok_or_else(error))
            .collect()
    };
    let selected = apps(&record["appOperation"]["facts"]["applications"])?;
    if selected.is_empty()
        || record["appOperation"]["request"]["app"] != selected.iter().cloned().collect::<Vec<_>>().join(",")
    {
        return Err(error());
    }
    let previous = apps(&before["apps"])?;
    let mut expected = previous.clone();
    match action {
        "update" if selected.is_subset(&previous) => {}
        "install" => expected.extend(selected.iter().cloned()),
        "uninstall" => expected.retain(|app| !selected.contains(app)),
        _ => return Err(error()),
    }
    if apps(&desired["apps"])? != expected {
        return Err(error());
    }
    for key in ["slug", "models", "llm", "gpustack"] {
        let new_gpu =
            key == "gpustack" && action == "install" && selected.contains("gpustack") && !previous.contains("gpustack");
        if !new_gpu && before[key] != desired[key] {
            return Err(error());
        }
    }
    let mut old_deployment = before["deployment"].clone();
    let mut new_deployment = desired["deployment"].clone();
    for key in [
        "root",
        "environment",
        "kubeconfig",
        "workDir",
        "bundleDir",
        "offline",
        "image",
    ] {
        if old_deployment[key] != new_deployment[key] && record["target"][key] != new_deployment[key] {
            return Err(error());
        }
        if let Some(object) = old_deployment.as_object_mut() {
            object.remove(key);
        }
        if let Some(object) = new_deployment.as_object_mut() {
            object.remove(key);
        }
    }
    if record["appOperation"]["legacyObservation"].is_object() {
        if new_deployment["installation"] != old_deployment["installation"]
            && new_deployment["installation"] != record["target"]["installation"]
        {
            return Err(error());
        }
        if let Some(object) = old_deployment.as_object_mut() {
            object.remove("installation");
        }
        if let Some(object) = new_deployment.as_object_mut() {
            object.remove("installation");
        }
    }
    if old_deployment != new_deployment {
        return Err(error());
    }
    Ok(())
}

pub(crate) fn start(path: &Path, cancel: &Cancellation) -> Result<Value> {
    submit(path, "start", cancel)
}

fn submit(path: &Path, action: &str, cancel: &Cancellation) -> Result<Value> {
    #[cfg(unix)]
    {
        use std::os::{fd::AsRawFd, unix::process::CommandExt};
        let record = resume::load(path)?.ok_or_else(unsupported)?;
        let phase = record["appOperation"]["facts"]["phase"].as_str().unwrap_or("");
        let allowed = if action == "start" {
            phase == "prepared"
        } else {
            ["prepared", "running", "verifying", "needs-reconciliation"].contains(&phase)
        };
        if record["target"]["runner"] != "native" || !allowed {
            return Err(unsupported());
        }
        let checkpoint = resume::location(path);
        let mut command = command(&record, &checkpoint, action, None, cancel)?;
        let owner = resume::guard(path)?;
        if resume::load(path)?.as_ref() != Some(&record) {
            return Err(fail("Prepared operation changed before submission", 409));
        }
        let fd = owner.as_raw_fd();
        command.args(["--owner-fd", &fd.to_string()]);
        // Share this open description with the fixed process API. The parent
        // keeps ownership until it returns; the service waits for that release.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let bytes = process::capture(&mut command, b"", Duration::from_secs(240), 65536, cancel)?;
        let data: Value = serde_json::from_slice(&bytes).map_err(|_| fail("Invalid Chentu submission receipt", 502))?;
        if data["schema"] != "chentu.application-executor.v1"
            || data["operationId"] != record["appOperation"]["facts"]["operationId"]
            || data["phase"] != "prepared"
        {
            return Err(fail("Chentu submission receipt differs from its operation", 409));
        }
        return Ok(json!({"operationId":data["operationId"],"unit":data["unit"],
            "phase":data["phase"],"submitted":true,"configurationCommitted":false}));
    }
    #[cfg(not(unix))]
    {
        let _ = (path, action, cancel);
        Err(unsupported())
    }
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

    #[cfg(unix)]
    fn verified_configuration(t: &contracts::Temp) -> (PathBuf, Value, Value) {
        let path = t.0.join("config.json");
        let entry = t.write("deploy/helmfile/run.sh", r#"set -eu
test "$1" = operation; test "$2" = status
python3 - "$4" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]));o=r['appOperation'];f=o['facts']
print(json.dumps({'schema':'chentu.application-operation-status.v1','operationId':f['operationId'],'phase':f['phase'],'applied':o['applied'],'verificationRef':f.get('verificationRef'),'readOnly':True,'executorObserved':True,'executor':{'ActiveState':o.get('fixtureExecutorState','inactive'),'ExecMainStatus':'0'},'configurationCommitted':r.get('succeeded') is True and r.get('finished') is True and r.get('commandActive') is False}))
PY
"#);
        let api = t.write("cli/src/chentu/application_operation.py", "# protocol fixture\n");
        let directory = t.0.join("deployments/run-0000000000000001");
        process::private_directory(&directory).unwrap();
        let before = json!({"schemaVersion":2,"slug":"fixture","apps":["apeiron"]});
        private_write(&path, &serde_json::to_vec(&before).unwrap()).unwrap();
        let previous_path = directory.join("previous-configuration.json");
        private_write(&previous_path, &std::fs::read(&path).unwrap()).unwrap();
        let desired = json!({"schemaVersion":2,"slug":"fixture","apps":["apeiron","browser"]});
        let desired_path = directory.join("desired-configuration.json");
        private_write(&desired_path, &serde_json::to_vec_pretty(&desired).unwrap()).unwrap();
        let record = json!({"schemaVersion":1,"configPath":path,"configHash":"0".repeat(64),
            "cliSha256":resume::cli_identity().unwrap(),"checkpoint":"bootstrap-complete","ownerPid":std::process::id(),
            "commandActive":false,"finished":false,"clusterUid":"fixture-uid","directory":directory,"plan":null,
            "target":{"runner":"native","root":t.0,"environment":t.0.join("environment.yaml"),"kubeconfig":t.0.join("absent-kubeconfig")},
            "environment":{"PATH":"/usr/bin:/bin"},
            "files":{entry.to_str().unwrap():resume::digest(&entry).unwrap(),api.to_str().unwrap():resume::digest(&api).unwrap(),desired_path.to_str().unwrap():resume::digest(&desired_path).unwrap(),previous_path.to_str().unwrap():resume::digest(&previous_path).unwrap()},
            "appOperation":{"api":{"schemaVersion":1,"operations":["status"]},"lockToken":"apeiron-app-test",
                "configurationCommit":{"schemaVersion":1,"desiredRef":desired_path,"beforeRef":previous_path,"beforeFileSha256":resume::digest(&path).unwrap()},
                "request":{"action":"install","app":"browser"},
                "facts":{"operationId":"one","phase":"succeeded","applications":["browser"],"verificationRef":"fixed-business-verified"},"applied":true}});
        resume::write(&resume::location(&path), &record).unwrap();
        (path, record, desired)
    }

    #[cfg(unix)]
    #[test]
    fn configuration_commit_uses_existing_completion_and_retries_metadata_only() {
        for interrupted_after_config_write in [false, true] {
            let t = contracts::Temp::new();
            let (path, _record, desired) = verified_configuration(&t);
            if interrupted_after_config_write {
                resume::write(&path, &desired).unwrap();
            }
            let result = commit(&path, &Cancellation::default()).unwrap();
            assert_eq!(result["configurationCommitted"], true);
            assert_eq!(config::read(&path).unwrap()["config"], desired);
            assert!(resume::load(&path).unwrap().is_none());
            let completed = resume::recovery(&path).unwrap().unwrap();
            assert_eq!(completed["succeeded"], true);
            assert_eq!(completed["configHash"], resume::config_hash(&desired));
            let archive: Value = serde_json::from_slice(
                &std::fs::read(Path::new(completed["directory"].as_str().unwrap()).join("completed-checkpoint.json"))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(archive, completed);
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(
                commit(&path, &Cancellation::default()).unwrap()["configurationCommitted"],
                true
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[cfg(unix)]
    #[test]
    fn configuration_commit_refuses_drift_unverified_and_live_executor_without_overwriting() {
        for failure in [
            "current-config",
            "desired-config",
            "verifying",
            "live-executor",
            "cli-changed",
            "settings",
            "scope",
        ] {
            let t = contracts::Temp::new();
            let (path, mut record, _) = verified_configuration(&t);
            match failure {
                "current-config" => std::fs::write(&path, b"user-edited-configuration").unwrap(),
                "desired-config" => std::fs::write(
                    record["appOperation"]["configurationCommit"]["desiredRef"]
                        .as_str()
                        .unwrap(),
                    b"changed proposal",
                )
                .unwrap(),
                "verifying" => record["appOperation"]["facts"]["phase"] = json!("verifying"),
                "live-executor" => record["appOperation"]["fixtureExecutorState"] = json!("active"),
                "settings" | "scope" => {
                    let desired_path = record["appOperation"]["configurationCommit"]["desiredRef"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    let mut proposed: Value = serde_json::from_slice(&std::fs::read(&desired_path).unwrap()).unwrap();
                    if failure == "settings" {
                        proposed["slug"] = json!("another-tenant");
                    } else {
                        proposed["apps"] = json!(["apeiron", "browser", "corpus"]);
                    }
                    std::fs::write(&desired_path, serde_json::to_vec(&proposed).unwrap()).unwrap();
                    record["files"][&desired_path] = json!(resume::digest(Path::new(&desired_path)).unwrap());
                }
                _ => record["cliSha256"] = json!("0".repeat(64)),
            }
            resume::write(&resume::location(&path), &record).unwrap();
            let before = std::fs::read(&path).unwrap();
            assert!(commit(&path, &Cancellation::default()).is_err(), "{failure}");
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert_eq!(resume::load(&path).unwrap().unwrap(), record);
        }
    }

    #[cfg(unix)]
    #[test]
    fn start_lends_the_live_owner_descriptor_and_never_replays_unprepared_work() {
        let t = contracts::Temp::new();
        let path = t.0.join("config.json");
        let entry = t.write("deploy/helmfile/run.sh", r#"set -eu
test "$1" = operation; { test "$2" = start || test "$2" = resume; }; test "$5" = --owner-fd
owner="${4%.installation-resume.json}.installation-owner"
python3 - "$6" "$owner" <<'PY'
import fcntl,json,os,sys
fd=int(sys.argv[1]);actual=os.fstat(fd);expected=os.stat(sys.argv[2])
assert (actual.st_dev,actual.st_ino)==(expected.st_dev,expected.st_ino)
with open(sys.argv[2],'r+b') as other:
 try:fcntl.flock(other,fcntl.LOCK_EX|fcntl.LOCK_NB)
 except BlockingIOError:pass
 else:raise RuntimeError('original installation owner was not held')
print(json.dumps({'schema':'chentu.application-executor.v1','operationId':'one','phase':'prepared','unit':'chentu-app-one.service'}))
PY
"#);
        let api = t.write("cli/src/chentu/application_operation.py", "# protocol fixture\n");
        let directory = t.0.join("deployments/run-0000000000000001");
        std::fs::create_dir_all(&directory).unwrap();
        let mut record = json!({"schemaVersion":1,"configPath":path,"configHash":"0".repeat(64),
            "cliSha256":"0".repeat(64),"checkpoint":"bootstrap-complete","ownerPid":std::process::id(),
            "commandActive":false,"finished":false,"clusterUid":"fixture-uid","directory":directory,"plan":null,
            "target":{"runner":"native","root":t.0,"environment":t.0.join("environment.yaml"),"kubeconfig":t.0.join("absent-kubeconfig")},
            "environment":{"PATH":"/usr/bin:/bin"},
            "files":{entry.to_str().unwrap():resume::digest(&entry).unwrap(),api.to_str().unwrap():resume::digest(&api).unwrap()},
            "appOperation":{"api":{"schemaVersion":1,"operations":["start"]},"facts":{"operationId":"one","phase":"prepared"}}});
        resume::write(&resume::location(&path), &record).unwrap();
        let blocker = resume::guard(&path).unwrap();
        assert_eq!(start(&path, &Cancellation::default()).unwrap_err().code, 409);
        drop(blocker);
        let value = start(&path, &Cancellation::default()).unwrap();
        assert_eq!(value["submitted"], true);
        assert_eq!(value["configurationCommitted"], false);
        assert_eq!(resume::load(&path).unwrap().unwrap(), record);
        record["appOperation"]["facts"]["phase"] = json!("verifying");
        resume::write(&resume::location(&path), &record).unwrap();
        assert_eq!(start(&path, &Cancellation::default()).unwrap_err().code, 2);
        assert_eq!(resume::load(&path).unwrap().unwrap(), record);
        record["appOperation"]["api"]["operations"] = json!(["resume"]);
        resume::write(&resume::location(&path), &record).unwrap();
        assert_eq!(
            submit(&path, "resume", &Cancellation::default()).unwrap()["submitted"],
            true
        );
        assert_eq!(resume::load(&path).unwrap().unwrap(), record);
    }

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
