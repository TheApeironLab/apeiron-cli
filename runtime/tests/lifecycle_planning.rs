//! Actual CLI acceptance: fixture files only; no cluster or model is contacted.
#[path = "support/lifecycle_fixture.rs"]
mod fixtures;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("apeiron-lifecycle-{:016x}", rand::random::<u64>()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn inputs(&self, inner: bool) -> Vec<String> {
        let input = fixtures::fixture(inner);
        let mut args = vec!["lifecycle".into(), "plan".into()];
        for (field, flag) in [
            ("installation", "--installation"),
            ("observation", "--observation"),
            ("intent", "--intent"),
            ("releaseManifest", "--manifest"),
        ] {
            let path = self.0.join(format!("{field}.json"));
            fs::write(&path, serde_json::to_vec_pretty(&input[field]).unwrap()).unwrap();
            args.extend([flag.into(), path.to_string_lossy().into_owned()]);
        }
        args
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn cli(args: &[String], root: &Path) -> std::process::Output {
    // Any cluster tool invocation is a test failure. The absolute CLI still works.
    let bin = root.join("poison-bin");
    fs::create_dir_all(&bin).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for tool in [
            "kubectl",
            "helm",
            "helmfile",
            "k3s",
            "ansible-playbook",
            "python3",
            "curl",
        ] {
            let path = bin.join(tool);
            fs::write(
                &path,
                format!(
                    "#!/bin/sh\nprintf invoked > '{}'\nexit 91\n",
                    root.join("unexpected-command").display()
                ),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_apeiron"))
        .args(args)
        .env("PATH", bin)
        .env("KUBECONFIG", root.join("nonexistent-context"))
        .env("APEIRON_LIFECYCLE_BIN", root.join("forbidden-forwarder"))
        .output()
        .unwrap();
    assert!(!root.join("unexpected-command").exists());
    output
}
#[test]
fn actual_cli_plans_each_cluster_without_tools_and_writes_private_non_overwriting_output() {
    for inner in [true, false] {
        let temp = Temp::new();
        let mut args = temp.inputs(inner);
        let out = temp.0.join("plan.json");
        args.extend(["--out".into(), out.to_string_lossy().into_owned()]);
        let output = cli(&args, &temp.0);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let plan: Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
        assert_eq!(plan["executionAllowed"], false);
        assert_eq!(plan["planningStatus"], "ready");
        assert_eq!(
            plan["changedComponents"],
            if inner { json!(["apeiron"]) } else { json!(["ontology"]) }
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let original = fs::read(&out).unwrap();
        assert!(!cli(&args, &temp.0).status.success());
        assert_eq!(fs::read(out).unwrap(), original);
    }
}
#[test]
fn record_validation_and_explicit_execution_block_work_through_cli() {
    let temp = Temp::new();
    let _ = temp.inputs(true);
    let args = vec![
        "lifecycle".into(),
        "validate-record".into(),
        "--installation".into(),
        temp.0.join("installation.json").to_string_lossy().into_owned(),
    ];
    let output = cli(&args, &temp.0);
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["liveVerified"], false);
    for operation in ["upgrade", "expand", "backup", "restore", "execute"] {
        let output = cli(&["lifecycle".into(), operation.into()], &temp.0);
        assert!(!output.status.success());
    }
}
#[test]
fn invalid_snapshot_does_not_write_output_or_echo_secret() {
    let temp = Temp::new();
    let mut args = temp.inputs(true);
    let path = temp.0.join("installation.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["password"] = json!("never-echo-this-value");
    fs::write(path, record.to_string()).unwrap();
    let out = temp.0.join("not-created.json");
    args.extend(["--out".into(), out.to_string_lossy().into_owned()]);
    let output = cli(&args, &temp.0);
    assert!(!output.status.success());
    assert!(!out.exists());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("never-echo-this-value"));
}
