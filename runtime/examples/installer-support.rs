// Maintainer-only adapter. Compile against the exact CLI source used by a release.
// No deployment, network access, or configuration writes are performed.
// Runtime preparation reads local management-machine facts for platform admission.
#[allow(dead_code)]
#[path = "../src/app/mod.rs"]
mod app;

use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

fn policy(config: &Value, hosted: bool) -> app::Result<()> {
    let installation = &config["deployment"]["installation"];
    let nodes = installation["nodes"]
        .as_array()
        .ok_or_else(|| app::fail("Missing nodes", 2))?;
    let servers = nodes.iter().filter(|n| n["role"] == "server").count();
    let agents = nodes.iter().filter(|n| n["role"] == "agent").count();
    if config["deployment"]["offline"] != true
        || installation["topology"] != "multi-k3s"
        || nodes.len() != 3
        || if hosted {
            servers != 3 || agents != 0 || installation["ha"] != true
        } else {
            servers != 1 || agents != 2 || installation["ha"] != false
        }
    {
        return Err(app::fail(
            "Installer contract requires offline multi-k3s with legacy 1 server + 2 agents or hosted 3 servers",
            2,
        ));
    }
    if !hosted {
        let actual = config["apps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let expected = ["nexus", "vasi", "ontology", "apeiron", "task", "corpus", "stalwart"]
            .into_iter()
            .collect::<BTreeSet<_>>();
        if actual != expected {
            return Err(app::fail(
                "Legacy installer requires the declared seven applications",
                2,
            ));
        }
    }
    Ok(())
}

fn pinned_ansible(requirements: &str, tool_paths: &[&str]) -> app::Result<()> {
    let pinned_ansible = requirements
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ansible-core=="))
        .collect::<Vec<_>>();
    if pinned_ansible.len() != 1 || !app::config::matches(r"^[0-9]+\.[0-9]+\.[0-9]+$", pinned_ansible[0]) {
        return Err(app::fail(
            "Operator requirements must pin exactly one Ansible version",
            2,
        ));
    }
    let ansible_prefix = format!("cli/ansible_core-{}-", pinned_ansible[0]);
    if !tool_paths
        .iter()
        .any(|p| p.starts_with(&ansible_prefix) && p.ends_with(".whl"))
    {
        return Err(app::fail("Missing pinned Ansible wheel from operator requirements", 2));
    }
    Ok(())
}

fn validate(bundle: &Path, config_path: &Path) -> app::Result<Value> {
    let raw = app::resources::json_file(config_path, 1024 * 1024)?;
    let config = app::config::validate(raw.get("config").unwrap_or(&raw), &Value::Null)?;
    let root = bundle.join("chentu");
    let profile = "k3s-amd64";
    let catalog = app::resources::catalog(&root, profile)?;
    let mut plan = app::resources::plan(&catalog, profile, &config["apps"])?;
    let hosted = plan["target"].get("nexusFirst").is_some();
    policy(&config, hosted)?;
    if plan["target"]["hostPlatform"] != json!({"os":"ubuntu","version":"22.04","architecture":"amd64"}) {
        return Err(app::fail(
            "Installer requires declared Ubuntu 22.04 AMD64 hostPlatform",
            2,
        ));
    }
    let base = bundle.join(catalog["resourceDirectory"].as_str().unwrap_or(""));
    // Use prepare (not just validate): source binding and platform admission are
    // part of the release runtime's real contract. An older parser MUST fail v7.
    app::distribution::prepare(
        &root,
        &base,
        &catalog,
        &mut plan,
        profile,
        "multi-k3s",
        app::distribution::Preparation {
            offline: true,
            cancel: &Default::default(),
            progress: &|_| {},
        },
    )?;
    app::resources::prepare_files(&plan, &base, true, &Default::default(), &|_| {})?;
    let manifest = if hosted {
        app::resources::json_file(&base.join("setup/release-manifest.json"), 4 * 1024 * 1024)?
    } else {
        Value::Null
    };
    let mut expected_cli = manifest["sources"]["apeironCli"].clone();
    if hosted {
        expected_cli["binary"] = manifest["toolchain"]["cli"]["binary"].clone();
    }
    if let Some(contract) = raw.get("contract") {
        if contract != if hosted { "hosted-first" } else { "legacy-native" } {
            return Err(app::fail("Explicit assembly contract disagrees with payload", 2));
        }
    }
    let tool_paths = plan["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect::<Vec<_>>();
    let python = tool_paths
        .iter()
        .filter(|p| app::config::matches(r"^python/cpython-3\.12[.-][a-zA-Z0-9._-]+\.tar\.gz$", p))
        .collect::<Vec<_>>();
    let mut tools = vec![
        "bin/uv",
        "bin/helm",
        "bin/helmfile",
        "bin/age",
        "bin/age-keygen",
        "bin/s5cmd",
        "k3s/k3s",
    ];
    if hosted {
        tools.push("bin/crane");
    }
    if python.len() != 1 || tools.iter().any(|p| !tool_paths.contains(p)) {
        return Err(app::fail(
            "Missing native operator tools or unique Python 3.12 archive",
            2,
        ));
    }
    tools.push(python[0]);
    let requirements = app::resources::read(&root.join("bootstrap/requirements.txt"), 1024 * 1024)?;
    let requirements = String::from_utf8(requirements).map_err(|_| app::fail("Invalid operator requirements", 2))?;
    pinned_ansible(&requirements, &tool_paths)?;
    for pattern in [r"^cli/ansible_core-[^/]+\.whl$", r"^cli/chentu-[^/]+\.whl$"] {
        let matches = tool_paths
            .iter()
            .filter(|p| app::config::matches(pattern, p))
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Err(app::fail("Missing operator wheel", 2));
        }
        tools.extend(matches.into_iter().copied());
    }
    let prefix = catalog["resourceDirectory"].as_str().unwrap_or("");
    let mut files = plan["files"].clone();
    for file in files.as_array_mut().unwrap() {
        file["path"] = json!(if prefix.is_empty() {
            file["path"].as_str().unwrap().to_owned()
        } else {
            format!("{prefix}/{}", file["path"].as_str().unwrap())
        });
    }
    Ok(json!({
        "schemaVersion":1,
        "contract":if hosted {"hosted-first"} else {"legacy-native"},
        "profile":profile,"topology":"multi-k3s",
        "target":{"os":"ubuntu","ubuntu":"jammy","arch":"amd64"},
        "hostPlatform":plan["target"]["hostPlatform"],
        "applications":config["apps"],"baseComponents":plan["target"]["base"],
        "componentClosure":plan["components"],"files":files,
        "runtimeTools":tools.iter().map(|p|json!({"path":p,"status":"declared-file-verified"})).collect::<Vec<_>>(),
        "operatorTools":plan["target"]["operatorTools"],
        "expectedCli":expected_cli,
        "bundleSource":manifest["sources"]["chentu"],
        "validator":{"sourceRevision":env!("APEIRON_SOURCE_REVISION"),"sourceClean":env!("APEIRON_SOURCE_CLEAN"),"version":env!("APEIRON_VERSION")},
        "checks":{"configuration":"passed","resourceResolution":"passed","resourceFiles":"passed","runtimeTools":"passed","runtimeToolExecution":"not-run","pythonDependencyInstallation":"not-run","releaseContract":if hosted {"passed"} else {"not-applicable"},"deployment":"not-run","offlineDeployment":"not-run","login":"not-run","business":"not-run"}
    }))
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = if args.len() == 4 && args[0] == "--bundle" && args[2] == "--config" {
        validate(Path::new(&args[1]), Path::new(&args[3]))
    } else {
        Err(app::fail(
            "Usage: installer-support --bundle BUNDLE --config CONFIG_JSON",
            2,
        ))
    };
    match result {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!("{}", error.message);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(hosted: bool) -> Value {
        json!({"apps":["nexus","vasi","ontology","apeiron","task","corpus","stalwart"],"deployment":{"offline":true,"installation":{"topology":"multi-k3s","ha":hosted,"nodes":[{"role":"server"},{"role":if hosted {"server"} else {"agent"}},{"role":if hosted {"server"} else {"agent"}}]}}})
    }
    #[test]
    fn pinned_wheel_allows_other_declared_versions() {
        let files = [
            "cli/ansible_core-2.19.0-py3-none-any.whl",
            "cli/ansible_core-2.19.3-py3-none-any.whl",
        ];
        assert!(pinned_ansible("ansible-core==2.19.0\n", &files).is_ok());
        assert!(pinned_ansible("ansible-core==2.19.1\n", &files).is_err());
        assert!(pinned_ansible("ansible-core>=2.19.0\n", &files).is_err());
    }
    #[test]
    fn policies_require_exact_roles_and_offline() {
        assert!(policy(&config(false), false).is_ok());
        assert!(policy(&config(true), true).is_ok());
        assert!(policy(&config(false), true).is_err());
        assert!(policy(&config(true), false).is_err());
        let mut value = config(false);
        value["deployment"]["offline"] = json!(false);
        assert!(policy(&value, false).is_err());
        value = config(false);
        value["apps"] = json!(["apeiron"]);
        assert!(policy(&value, false).is_err());
    }
}
