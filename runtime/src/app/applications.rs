//! Lifecycle commands target an existing, identity-bound installation. They never
//! provision hosts, choose a kube context implicitly, or delete application data.
use super::{deploy::Context, process::Cancellation, *};
use std::{collections::BTreeSet, fs, path::PathBuf};

const HELP: &str = "apeiron app <install|update|uninstall> <app> --instance <name> [--bundle-dir /absolute/package] [--dry-run]\napeiron app list --instance <name>\n\nUse --config /absolute/config.json instead of --instance when needed.\ninstall: add one application; shared dependencies must already be installed.\nupdate: sync the application's pinned version from the current or specified package.\nuninstall: remove services, retain PVCs, credentials, databases and objects; requires --yes.\nOlder releases must first be updated to record data-retention policies.\n--dry-run validates and previews without changing cluster resources or instance configuration.\nThe package must support app lifecycle and retain other installed components' artifacts.\nInterrupted operations are retried with the same command and package. No automatic rollback.\n";

#[derive(Clone, Debug, PartialEq)]
struct Request {
    action: String,
    app: String,
    releases: Vec<String>,
}
fn request(action: &str, app: &str, current: &Value) -> Result<(Request, Value)> {
    if !["install", "update", "uninstall"].contains(&action) {
        return Err(fail("Use app install, update or uninstall", 2));
    }
    let catalog = config::apps();
    let selected = catalog
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == app)
        .ok_or_else(|| fail(format!("Unknown application: {app}"), 2))?;
    let old = current["apps"]
        .as_array()
        .ok_or_else(|| fail("Run apeiron init before managing applications", 2))?;
    if action == "uninstall" && selected["required"] == true {
        return Err(fail("Required platform applications cannot be uninstalled", 2));
    }
    if action == "update" && !old.contains(&json!(app)) {
        return Err(fail("Application is not selected; use app install", 2));
    }
    let mut apps = old.clone();
    if action == "uninstall" {
        for other in catalog.as_array().unwrap().iter().filter(|a| old.contains(&a["id"])) {
            if other["requires"].as_array().is_some_and(|r| r.contains(&json!(app))) {
                return Err(fail(
                    format!("{} depends on {app}; uninstall it first", other["id"].as_str().unwrap()),
                    2,
                ));
            }
        }
        apps.retain(|a| a != app);
    } else {
        for dependency in selected["requires"].as_array().into_iter().flatten() {
            if !apps.contains(dependency) {
                return Err(fail(
                    format!("Install dependency {} first", dependency.as_str().unwrap()),
                    2,
                ));
            }
        }
        if !apps.contains(&json!(app)) {
            apps.push(json!(app));
        }
    }
    let mut desired = current.clone();
    desired["apps"] = json!(apps);
    desired = config::validate(&desired, current)?;
    let releases = selected["releases"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![json!(app)])
        .iter()
        .map(|r| r.as_str().unwrap().to_owned())
        .collect();
    Ok((
        Request {
            action: action.into(),
            app: app.into(),
            releases,
        },
        desired,
    ))
}

/// Reject an application package that would also change shared services or other
/// installed applications. Source/CLI metadata may advance independently.
/// A package may only change the selected application: hosted artifacts of every other
/// active component are byte-identical, and each of their images (install.json) is
/// already installed (`images/install-manifest.json` of the current bundle).
fn compatible(
    old: &Value,
    installed: &Value,
    new: &Value,
    catalog: &Value,
    selected: &[String],
    active: &Value,
) -> Result<()> {
    let artifacts = |manifest: &Value| -> BTreeMap<String, Value> {
        let protected: BTreeSet<&str> = manifest["components"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| !selected.contains(name) && active.as_array().is_some_and(|a| a.contains(&json!(name))))
            .flat_map(|(_, component)| {
                component["artifacts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
            })
            .collect();
        manifest["artifacts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| protected.contains(a["id"].as_str().unwrap_or("")))
            .filter(|a| a["kind"] == "chart" || a["kind"] == "resource")
            // The runtime source archive carries the lifecycle entry itself.
            // Its digest is verified by preparation, but can advance with CLI code.
            .filter(|a| {
                !(a["kind"] == "resource"
                    && a["name"] == "chentu-deployment"
                    && a["id"] == "sources/deployment-source.tar.gz")
            })
            .map(|a| {
                (
                    format!("{}:{}", a["kind"].as_str().unwrap(), a["name"].as_str().unwrap()),
                    json!({"sha256":a["sha256"],"kind":a["kind"]}),
                )
            })
            .collect()
    };
    let present: BTreeSet<(String, String)> = installed
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| {
            (
                i["reference"].as_str().unwrap_or("").into(),
                i["digest"].as_str().unwrap_or("").into(),
            )
        })
        .collect();
    let unchanged_images = catalog["components"]
        .as_object()
        .into_iter()
        .flatten()
        .all(|(name, component)| {
            selected.contains(name)
                || !active.as_array().is_some_and(|a| a.contains(&json!(name)))
                || component["images"].as_array().into_iter().flatten().all(|i| {
                    present.contains(&(
                        i["reference"].as_str().unwrap_or("").into(),
                        i["digest"].as_str().unwrap_or("").into(),
                    ))
                })
        });
    if artifacts(old) != artifacts(new) || !unchanged_images {
        return Err(fail("Package changes shared services or other installed applications; use a compatible app package or apeiron init for a platform upgrade", 2));
    }
    Ok(())
}

fn source_package(
    record: &Value,
    current: &Value,
    desired: &Value,
    req: &Request,
    c: &Context,
) -> Result<(Value, BTreeMap<String, String>, Value, PathBuf)> {
    let old_env: BTreeMap<String, String> = serde_json::from_value(record["environment"].clone())
        .map_err(|_| fail("Invalid installation environment", 2))?;
    let previous = resources::yaml(Path::new(
        old_env
            .get("CHENTU_ENV")
            .ok_or_else(|| fail("Missing installation environment", 2))?,
    ))?;
    if previous["nexusFirst"] != true {
        return Err(fail("Application lifecycle requires a Nexus-first installation", 2));
    }
    let mut effective = desired.clone();
    if req.action == "uninstall" {
        effective["apps"] = current["apps"].clone();
    }
    // Uninstall retry still needs a declaration for the removed release.
    if !effective["apps"].as_array().unwrap().contains(&json!(req.app)) {
        effective["apps"].as_array_mut().unwrap().push(json!(req.app));
    }
    let root = resources::resolve(&effective["deployment"], &c.cancel, &|m| c.event(m))?;
    let key = format!(
        "{}-{}",
        if record["target"]["runner"] == "docker" {
            "k3d"
        } else {
            "k3s"
        },
        previous["architecture"]
            .as_str()
            .ok_or_else(|| fail("Missing architecture", 2))?
    );
    let catalog = resources::catalog(&root, &key)?;
    let mut plan = resources::plan(&catalog, &key, &config::components(&effective["apps"]))?;
    let cache = PathBuf::from(
        effective["deployment"]["bundleDir"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| fail("Specify --bundle-dir for the installed or new package", 2))?,
    )
    .join(catalog["resourceDirectory"].as_str().unwrap_or(""));
    distribution::prepare(
        &root,
        &cache,
        &catalog,
        &mut plan,
        &key,
        previous["topology"].as_str().unwrap(),
        distribution::Preparation {
            offline: effective["deployment"]["offline"] == true,
            cancel: &c.cancel,
            progress: &|m| c.event(m),
        },
    )?;
    if !root.join("deploy/helmfile/scripts/app_lifecycle.py").is_file() {
        return Err(fail(
            "Package does not support app lifecycle; supply a newer --bundle-dir",
            2,
        ));
    }
    let old_manifest = resources::json_file(
        &Path::new(record["bundle"].as_str().unwrap()).join("setup/release-manifest.json"),
        16 * 1024 * 1024,
    )?;
    let installed = resources::json_file(
        &Path::new(record["bundle"].as_str().unwrap()).join("images/install-manifest.json"),
        16 * 1024 * 1024,
    )?;
    compatible(
        &old_manifest,
        &installed,
        &plan["releaseManifest"],
        &catalog,
        &req.releases,
        &plan["components"],
    )?;
    resources::prepare_files(&plan, &cache, true, &c.cancel, &|m| c.event(m))?;
    // Stage a new immutable bundle below the existing toolbox mount. Never
    // overwrite the prior installation's recovery files or runtime toolchain.
    let work = PathBuf::from(record["target"]["workDir"].as_str().unwrap());
    let relative = format!("apps/{}/bundle", c.directory.file_name().unwrap().to_string_lossy());
    let bundle = work.join(&relative);
    process::private_directory(&bundle)?;
    for f in plan["files"].as_array().unwrap() {
        c.cancel.check()?;
        let rel = f["path"].as_str().unwrap();
        let dest = resources::destination(&bundle, rel)?;
        process::private_directory(dest.parent().unwrap())?;
        fs::copy(resources::destination(&cache, rel)?, dest).map_err(|_| fail("Cannot stage app resources", 9))?;
    }
    process::private_directory(&bundle.join("nexus"))?;
    private_write(
        &bundle.join("nexus/install-selection.json"),
        &serde_json::to_vec(&plan["installSelection"]).unwrap(),
    )?;
    let mut values = previous.clone();
    let mut package_values = plan["environment"].clone();
    let docker = record["target"]["runner"] == "docker";
    let mounted_bundle = if docker {
        format!("/work/{relative}")
    } else {
        bundle.to_string_lossy().into_owned()
    };
    bootstrap::replace(
        &mut package_values,
        &[
            ("__DOMAIN__", previous["domain"].as_str().unwrap().into()),
            ("__REGISTRY__", previous["registry"].as_str().unwrap().into()),
            ("__NODE__", previous["node"].as_str().unwrap().into()),
            ("__WORK__", previous["work"].as_str().unwrap().into()),
            ("__BUNDLE__", mounted_bundle.clone()),
            ("__ARCH__", previous["architecture"].as_str().unwrap().into()),
        ],
    )?;
    // Preserve operator values. Only package-owned image fields and Chart pins
    // advance for selected releases; newly selected apps use package defaults.
    for name in &req.releases {
        let new = &package_values["releases"][name];
        if new.is_null() {
            return Err(fail(format!("Package has no release {name}"), 2));
        }
        if !values["releases"][name]["values"].is_object() {
            values["releases"][name] = new.clone();
        } else {
            replace_images(&mut values["releases"][name]["values"], &new["values"]);
        }
        values["releases"][name]["enabled"] = json!(true);
        if !package_values["chartPins"][name].is_null() {
            values["chartPins"][name] = package_values["chartPins"][name].clone();
        }
    }
    values["imagePins"] = package_values["imagePins"].clone();
    // Shared resource paths (e.g. Skills) move to the identical verified files.
    let previous_bundle = previous["bundle"]
        .as_str()
        .ok_or_else(|| fail("Missing bundle path", 2))?;
    relocate(&mut values, previous_bundle, &mounted_bundle);
    values["bundle"] = json!(mounted_bundle);
    values = deploy::environment_for(&effective, values)?;
    let output = c.directory.join("environment.yaml");
    resources::write_yaml(&output, &values)?;
    let mut target = record["target"].clone();
    target["root"] = json!(root);
    target["environment"] = json!(output);
    let mut env = old_env;
    env.insert("CHENTU_ENV".into(), output.to_string_lossy().into_owned());
    env.insert("CHENTU_ROOT".into(), root.to_string_lossy().into_owned());
    env.insert("PYTHONPATH".into(), root.join("cli/src").to_string_lossy().into_owned());
    if docker {
        env.insert("LAB_ENV".into(), output.to_string_lossy().into_owned());
        env.insert("LAB_BUNDLE_DIR".into(), bundle.to_string_lossy().into_owned());
        env.insert(
            "LAB_CONTAINER".into(),
            format!("apeiron-app-{}", c.directory.file_name().unwrap().to_string_lossy()),
        );
    }
    Ok((target, env, plan, bundle))
}
fn replace_images(old: &mut Value, new: &Value) {
    if let Some(fields) = new.as_object() {
        for (key, value) in fields {
            if key == "image" || key.ends_with("Image") {
                old[key] = value.clone();
            } else if value.is_object() && old[key].is_object() {
                replace_images(&mut old[key], value);
            }
        }
    }
}
fn relocate(v: &mut Value, before: &str, after: &str) {
    match v {
        Value::String(s) if s == before || s.starts_with(&format!("{before}/")) => {
            *s = format!("{after}{}", &s[before.len()..])
        }
        Value::Array(a) => {
            for x in a {
                relocate(x, before, after)
            }
        }
        Value::Object(o) => {
            for x in o.values_mut() {
                relocate(x, before, after)
            }
        }
        _ => (),
    }
}
struct ConfigLock(PathBuf);
impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn lock_config(path: &Path, token: &str) -> Result<ConfigLock> {
    let lock = PathBuf::from(format!("{}.lock", path.display()));
    if lock.exists()
        && resources::read(&lock, 1024).is_ok_and(|b| {
            if b == token.as_bytes() {
                return true;
            }
            String::from_utf8(b)
                .ok()
                .and_then(|s| s.strip_prefix("apeiron-app-")?.split('-').next()?.parse::<i64>().ok())
                .is_some_and(|pid| !resume::running(pid, false))
        })
    {
        // Caller holds the installation owner lock and proved the previous
        // process group inactive before reclaiming this operation's own lock.
        fs::remove_file(&lock).map_err(|_| fail("Cannot recover app configuration lock", 9))?;
    }
    private_write(&lock, token.as_bytes()).map_err(|_| fail("Instance configuration is busy", 5))?;
    Ok(ConfigLock(lock))
}

fn invoke(context: &Context, args: &[String], root: &Path, env: &BTreeMap<String, String>) -> Result<()> {
    context.run("bash", args, root, env).map_err(|error| {
        let detail = fs::read_to_string(context.directory.join("install.log"))
            .unwrap_or_default()
            .lines()
            .rev()
            .find(|l| l.starts_with("application operation failed:"))
            .map(str::to_owned)
            .unwrap_or(error.message);
        fail(detail, error.code)
    })
}

pub fn run(args: &[String]) -> Result<()> {
    let (pos, options) = parse(
        args,
        &["instance", "config", "bundle-dir"],
        &["help", "dry-run", "yes", "json"],
    )?;
    if options.contains_key("help") || pos.is_empty() {
        println!("{HELP}");
        return Ok(());
    }
    let path = match (options.get("instance"), options.get("config")) {
        (Some(name), None) => instances::path_for(&instances::base()?, name)?,
        (None, Some(p)) => process::config_path(Some(p))?,
        _ => return Err(fail("Choose exactly one --instance or --config", 2)),
    };
    if pos == ["list"] {
        let state = config::read(&path)?;
        let rows=config::apps().as_array().unwrap().iter().map(|a| json!({"id":a["id"],"name":a["name"],"required":a["required"],"selected":state["config"]["apps"].as_array().is_some_and(|s|s.contains(&a["id"]))})).collect::<Vec<_>>();
        return output(
            json!({"items":rows,"note":"selected is desired configuration, not live health"}),
            &options,
            "apeiron.apps.v1",
        );
    }
    if pos.len() != 2 {
        return Err(fail(HELP, 2));
    }
    if pos[0] == "uninstall" && !options.contains_key("yes") && !options.contains_key("dry-run") {
        return Err(fail(
            "Uninstall stops services and retains data; use --dry-run to preview, then --yes",
            2,
        ));
    }
    if pos[0] == "uninstall" && options.contains_key("bundle-dir") {
        return Err(fail(
            "Uninstall uses the installed package; update first to change packages",
            2,
        ));
    }
    config::location(&path)?;
    let _guard = resume::guard(&path)?;
    resume::bind_executable()?;
    let saved = config::read(&path)?;
    let current = &saved["config"];
    let (req, mut desired) = request(&pos[0], &pos[1], current)?;
    if let Some(bundle) = options.get("bundle-dir") {
        if !Path::new(bundle).is_absolute() {
            return Err(fail("--bundle-dir must be absolute", 2));
        }
        desired["deployment"]["bundleDir"] = json!(bundle);
    }
    let cancel = Cancellation::default();
    let pending = resume::load(&path)?;
    let record =
        resume::recovery(&path)?.ok_or_else(|| fail("No completed installation; run apeiron init first", 2))?;
    resume::inactive(&record)?;
    let operation = json!({"action":req.action,"app":req.app,"bundle":desired["deployment"]["bundleDir"]});
    if pending.is_some() {
        if record["appOperation"]["request"] != operation
            || (record["appOperation"]["beforeHash"] != resume::config_hash(current)
                && record["configHash"] != resume::config_hash(current))
        {
            return Err(fail(
                "An unfinished operation exists; retry its exact app command before starting another operation",
                5,
            ));
        }
        resume::validate(&record, &desired, &cancel)?;
    } else {
        resume::validate_completed(&record, current, &cancel)?;
    }
    let directory = if pending.is_some() {
        PathBuf::from(record["directory"].as_str().unwrap())
    } else {
        path.parent()
            .unwrap()
            .join("deployments")
            .join(format!("run-{:016x}", rand::random::<u64>()))
    };
    let context = Context::application(&path, &directory)?;
    let signal = context.cancel.clone();
    std::thread::spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
            rt.block_on(async {
                if tokio::signal::ctrl_c().await.is_ok() {
                    signal.cancel();
                }
            });
        }
    });
    eprintln!(
        "operation\t{}\napplication\t{}\nlog\t{}",
        req.action,
        req.app,
        directory.join("install.log").display()
    );
    let (target, env, plan, bundle) = if pending.is_some() {
        (
            record["target"].clone(),
            serde_json::from_value(record["environment"].clone()).unwrap(),
            record["plan"].clone(),
            PathBuf::from(record["bundle"].as_str().unwrap()),
        )
    } else {
        source_package(&record, current, &desired, &req, &context)?
    };
    let root = Path::new(target["root"].as_str().unwrap());
    let script = root.join(if target["runner"] == "docker" {
        "tests/lab/helmfile.sh"
    } else {
        "deploy/helmfile/run.sh"
    });
    let mut command = vec![script.to_string_lossy().into_owned(), "app".into(), req.action.clone()];
    command.extend(req.releases.clone());
    let mut preview = command.clone();
    preview.push("--dry-run".into());
    // Planning queries the live Helm graph and retention before creating a pending
    // mutation record. A failed preflight leaves the instance untouched.
    if record["appOperation"]["applied"] != true || pending.is_none() {
        invoke(&context, &preview, root, &env)?;
    }
    if options.contains_key("dry-run") {
        let live_plan = fs::read_to_string(directory.join("install.log"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .rfind(|v| v["schema"] == "chentu.app-plan.v1")
            .unwrap_or(Value::Null);
        return output(
            json!({"operation":req.action,"app":req.app,"releases":req.releases,
            "dryRun":true,"retainData":true,"plan":live_plan,"log":directory.join("install.log")}),
            &options,
            "apeiron.app-operation.v1",
        );
    }
    let token = if pending.is_some() {
        record["appOperation"]["lockToken"].as_str().unwrap().to_owned()
    } else {
        format!("apeiron-app-{}-{:016x}", std::process::id(), rand::random::<u64>())
    };
    let _config_lock = lock_config(&path, &token)?;
    if config::read(&path)?["revision"] != saved["revision"] {
        return Err(fail("Instance configuration changed; retry", 5));
    }
    let operation_record = if pending.is_some() {
        record["appOperation"].clone()
    } else {
        json!({"request":operation,"beforeHash":resume::config_hash(current),"lockToken":token})
    };
    let mut checkpoint = if pending.is_some() {
        record.clone()
    } else {
        resume::checkpoint(
            &path,
            &desired,
            &directory,
            &target,
            &env,
            (&plan, &bundle),
            &context.cancel,
        )?
    };
    checkpoint["appOperation"] = operation_record;
    checkpoint["ownerPid"] = json!(std::process::id());
    checkpoint["finished"] = json!(false);
    resume::write(&resume::location(&path), &checkpoint)?;
    context.app_checkpoint();
    command.push("--yes".into());
    let result = (|| {
        if checkpoint["appOperation"]["applied"] != true {
            invoke(&context, &command, root, &env)?;
            let previous_environment = Path::new(env.get("CHENTU_ENV").unwrap());
            let mut values = resources::yaml(previous_environment)?;
            if req.action == "uninstall" {
                for name in &req.releases {
                    values["releases"][name]["enabled"] = json!(false);
                }
            }
            // Publish a new environment and then switch the checkpoint atomically.
            // If interrupted before config commit, retry only commits metadata.
            let environment = directory.join("environment-complete.yaml");
            resources::write_yaml(&environment, &values)?;
            let mut latest = resources::json_file(&resume::location(&path), 16 * 1024 * 1024)?;
            latest["target"]["environment"] = json!(environment);
            latest["environment"]["CHENTU_ENV"] = json!(environment);
            if target["runner"] == "docker" {
                latest["environment"]["LAB_ENV"] = json!(environment);
            }
            latest["files"][environment.to_string_lossy().as_ref()] =
                json!(config::hash(&resources::read(&environment, 64 * 1024 * 1024)?));
            latest["appOperation"]["applied"] = json!(true);
            resume::write(&resume::location(&path), &latest)?;
        }
        resume::write(&path, &desired)?;
        resume::complete(&path, &directory)?;
        Ok(())
    })();
    let cleaned = context.app_cleanup();
    if let Some(mut record) = resume::load(&path)? {
        record["finished"] = json!(cleaned);
        resume::write(&resume::location(&path), &record)?;
    }
    if !cleaned {
        return Err(fail(
            "Application toolbox could not be stopped; inspect it before retrying",
            9,
        ));
    }
    result?;
    output(
        json!({"operation":req.action,"app":req.app,"status":"succeeded","dataRetained":true}),
        &options,
        "apeiron.app-operation.v1",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn current() -> Value {
        json!({"schemaVersion":2,"slug":"example-team","apps":["nexus","vasi","ontology","apeiron","zygos","gateway","git","ci"]})
    }
    #[test]
    fn catalog_drives_application_release_mapping_and_dependencies() {
        let (r, c) = request("install", "kps", &current()).unwrap();
        assert_eq!(r.releases, ["kps", "loki", "promtail"]);
        assert!(c["apps"].as_array().unwrap().contains(&json!("zygos")));
        assert_eq!(
            request("install", "browser", &current()).unwrap().0.releases,
            ["chrome-headless"]
        );
        assert_eq!(request("update", "ci", &current()).unwrap().0.releases, ["woodpecker"]);
        assert!(request("uninstall", "git", &current())
            .unwrap_err()
            .message
            .contains("ci depends"));
        let (_, without_ci) = request("uninstall", "ci", &current()).unwrap();
        let (_, without_git) = request("uninstall", "git", &without_ci).unwrap();
        assert!(request("install", "ci", &without_git)
            .unwrap_err()
            .message
            .contains("dependency git"));
    }
    #[test]
    fn required_unknown_and_missing_updates_fail_without_dropping_other_apps() {
        for app in ["nexus", "vasi", "gateway", "apeiron"] {
            assert!(request("uninstall", app, &current()).is_err());
        }
        let (_, without_ontology) = request("uninstall", "ontology", &current()).unwrap();
        assert_eq!(
            without_ontology["apps"],
            json!(["nexus", "vasi", "apeiron", "zygos", "gateway", "git", "ci"])
        );
        for app in ["unknown", "--all", "zygos;echo x"] {
            assert!(request("install", app, &current()).is_err());
        }
        assert!(request("update", "task", &current()).is_err());
        let (_, removed) = request("uninstall", "zygos", &current()).unwrap();
        assert_eq!(
            removed["apps"],
            json!(["nexus", "vasi", "ontology", "apeiron", "gateway", "git", "ci"])
        );
        let (_, added) = request("install", "zygos", &removed).unwrap();
        assert_eq!(added["apps"], current()["apps"]);
        assert_eq!(request("install", "zygos", &added).unwrap().1, added);
    }
    fn manifest() -> Value {
        json!({"components":{"zygos":{"artifacts":["app"]},"postgres":{"artifacts":["shared"]}},
            "artifacts":[{"id":"app","name":"zygos","kind":"image","sha256":"a"},
                         {"id":"shared","name":"postgres","kind":"chart","sha256":"b"}]})
    }
    #[test]
    fn single_app_update_cannot_silently_change_shared_artifacts() {
        let old = manifest();
        let mut new = old.clone();
        let selected = vec!["zygos".into()];
        let active = json!(["zygos", "postgres"]);
        new["artifacts"][0]["sha256"] = json!("next-app");
        assert!(compatible(&old, &json!([]), &new, &json!({}), &selected, &active).is_ok());
        new["artifacts"][1]["sha256"] = json!("changed-database");
        assert!(compatible(&old, &json!([]), &new, &json!({}), &selected, &active).is_err());
        new["artifacts"].as_array_mut().unwrap().pop();
        assert!(compatible(&old, &json!([]), &new, &json!({}), &selected, &active).is_err());
        // Images come from install.json: another active component's image must be installed.
        let catalog = json!({"components":{"zygos":{"images":[{"reference":"z:2","digest":"d2"}]},
            "postgres":{"images":[{"reference":"pg:1","digest":"d1"}]}}});
        let installed = json!([{"reference":"z:1","digest":"d0"},{"reference":"pg:1","digest":"d1"}]);
        assert!(compatible(&old, &installed, &old, &catalog, &selected, &active).is_ok());
        let changed = json!([{"reference":"z:1","digest":"d0"},{"reference":"pg:1","digest":"d9"}]);
        assert!(compatible(&old, &changed, &old, &catalog, &selected, &active).is_err());
    }
    #[test]
    fn deployment_code_can_advance_but_shared_business_resources_cannot() {
        let mut old = manifest();
        old["components"]["postgres"]["artifacts"] = json!(["shared", "sources/deployment-source.tar.gz", "skills"]);
        old["artifacts"].as_array_mut().unwrap().extend([
            json!({"id":"sources/deployment-source.tar.gz","name":"chentu-deployment","kind":"resource","sha256":"old-code"}),
            json!({"id":"skills","name":"system-skills","kind":"resource","sha256":"old-skills"}),
        ]);
        let mut new = old.clone();
        new["artifacts"][2]["sha256"] = json!("new-code");
        let selected = vec!["zygos".into()];
        let active = json!(["zygos", "postgres"]);
        assert!(compatible(&old, &json!([]), &new, &json!({}), &selected, &active).is_ok());
        new["artifacts"][3]["sha256"] = json!("new-skills");
        assert!(compatible(&old, &json!([]), &new, &json!({}), &selected, &active).is_err());
    }
    #[test]
    fn image_upgrade_preserves_operator_storage_and_runtime_values() {
        let mut old = json!({"api":{"image":"old","resources":{"memory":"4Gi"}},"worker":{"image":"old-worker"},"workspaceStorage":"50Gi","tenant":"example-team"});
        let new = json!({"api":{"image":"new","resources":{"memory":"1Gi"}},"worker":{"image":"new-worker"},"workspaceStorage":"10Gi","tenant":"other"});
        replace_images(&mut old, &new);
        assert_eq!(
            old,
            json!({"api":{"image":"new","resources":{"memory":"4Gi"}},"worker":{"image":"new-worker"},"workspaceStorage":"50Gi","tenant":"example-team"})
        );
    }
    #[test]
    fn bundle_relocation_does_not_change_unrelated_strings_or_keys() {
        let mut v = json!({"bundle":"/old/bundle","skills":"/old/bundle/a.bundle","other":"/old/bundle-other/file","array":["/old/bundle/x"]});
        relocate(&mut v, "/old/bundle", "/new/bundle");
        assert_eq!(
            v,
            json!({"bundle":"/new/bundle","skills":"/new/bundle/a.bundle","other":"/old/bundle-other/file","array":["/new/bundle/x"]})
        );
    }
    #[test]
    fn configuration_lock_never_claims_wizard_or_live_operation_locks() {
        let tmp = contracts::Temp::new();
        let path = tmp.0.join("config.json");
        let lock = tmp.0.join("config.json.lock");
        private_write(&lock, b"").unwrap();
        assert!(lock_config(&path, "different").is_err());
        fs::remove_file(&lock).unwrap();
        let live = format!("apeiron-app-{}-one", std::process::id());
        private_write(&lock, live.as_bytes()).unwrap();
        assert!(lock_config(&path, "apeiron-app-other").is_err());
        // Recovery may reclaim exactly its own token after the caller's owner/child checks.
        drop(lock_config(&path, &live).unwrap());
        assert!(!lock.exists());
    }
}
