//! Fixed packaged driver bridge. Production admission remains unavailable.
use super::*;
use sha2::{Digest, Sha256};
use std::{fs::File, process::Command};

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|_| invalid())?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|_| invalid())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify_driver(package: &Path, expected: &str) -> Result<()> {
    check(package.is_absolute() && package.canonicalize().ok().as_deref() == Some(package))?;
    check(digest(expected) && hash_file(&package.join("SHA256SUMS"))? == expected)?;
    let sums = resources::read(&package.join("SHA256SUMS"), LIMIT)?;
    let sums = std::str::from_utf8(&sums).map_err(|_| invalid())?;
    let mut names = BTreeSet::new();
    let mut code_hashes = BTreeMap::new();
    for line in sums.lines() {
        let (hash, name) = line.split_once("  ").ok_or_else(invalid)?;
        check(digest(hash) && names.insert(name))?;
        let relative = Path::new(name);
        check(
            !relative.is_absolute()
                && relative
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
        )?;
        if name.starts_with("bundle/chentu/cli/src/chentu/") {
            let path = package.join(relative);
            check(path.canonicalize().ok().as_deref() == Some(path.as_path()))?;
            check(hash_file(&path)? == hash)?;
            code_hashes.insert(name.to_owned(), hash.to_owned());
        }
    }
    check(code_hashes.contains_key("bundle/chentu/cli/src/chentu/lifecycle_upgrade.py"))?;
    let mut directories = vec![package.join("bundle/chentu/cli/src/chentu")];
    while let Some(directory) = directories.pop() {
        for item in std::fs::read_dir(directory).map_err(|_| invalid())? {
            let item = item.map_err(|_| invalid())?;
            let kind = item.file_type().map_err(|_| invalid())?;
            check(!kind.is_symlink())?;
            if kind.is_dir() {
                directories.push(item.path());
            } else {
                check(kind.is_file())?;
                let path = item.path();
                let name = path
                    .strip_prefix(package)
                    .map_err(|_| invalid())?
                    .to_str()
                    .ok_or_else(invalid)?;
                check(code_hashes.contains_key(name))?;
            }
        }
    }
    Ok(())
}

pub(super) fn run(args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(fail(
            "升级执行已阻断：需要显式的隔离副本操作、固定安装包和已审计划；请查看 lifecycle upgrade --help。",
            409,
        ));
    }
    let (pos, options) = parse(
        args,
        &[
            "package",
            "package-sha256",
            "python",
            "python-sha256",
            "request",
            "plan",
            "plan-sha256",
            "state",
            "review-sha256",
            "reconciliation-sha256",
        ],
        &["help"],
    )?;
    if options.contains_key("help") {
        println!("apeiron lifecycle upgrade <prepare|execute|verify|status|reconcile|resume|commit>\n  --package DIR --package-sha256 SHA256_OF_SHA256SUMS\n  --python FILE --python-sha256 SHA256\n  prepare: --request FILE --state NEW_DIR\n  execute: --plan FILE --plan-sha256 SHA256 --state DIR\n  verify/status/reconcile: --plan FILE --plan-sha256 SHA256 --state DIR\n  resume: same bindings plus --reconciliation-sha256 SHA256\n  commit: same bindings plus --review-sha256 SHA256\nIsolated single-k3s application drill only. Production, expansion and restore are blocked.");
        return Ok(());
    }
    check(
        pos.len() == 1
            && [
                "prepare",
                "execute",
                "verify",
                "status",
                "reconcile",
                "resume",
                "commit",
            ]
            .contains(&pos[0].as_str()),
    )?;
    let package = Path::new(required(&options, "package")?);
    verify_driver(package, required(&options, "package-sha256")?)?;
    let python = Path::new(required(&options, "python")?);
    check(
        python.is_absolute()
            && digest(required(&options, "python-sha256")?)
            && hash_file(python)? == required(&options, "python-sha256")?,
    )?;
    let cli = std::env::current_exe().map_err(|_| invalid())?;
    check(hash_file(&cli)? == hash_file(&package.join("bin/apeiron"))?)?;
    let mut command = Command::new(python);
    command.env_clear().env("PATH", "/usr/bin:/bin").env("LANG", "C.UTF-8");
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    command
        .args(["-I", "-B"])
        .arg(package.join("bundle/chentu/cli/src/chentu/lifecycle_upgrade.py"));
    command.arg(&pos[0]);
    for (key, value) in &options {
        command.arg(format!("--{key}")).arg(value);
    }
    command.arg("--cli").arg(cli);
    // Python isolated mode excludes user site/PYTHONPATH. The packaged engine
    // constructs its own pinned native-tool environment and host-local lease.
    let status = command.status().map_err(|_| fail("无法启动固定的升级执行器。", 409))?;
    if !status.success() {
        return Err(fail(
            "升级操作未完成；请通过 status 检查持久状态。未自动重试或回退。",
            409,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_driver_rejects_changed_and_unlisted_imports() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("apeiron-upgrade-{}-{nonce}", std::process::id()));
        let directory = root.join("bundle/chentu/cli/src/chentu");
        std::fs::create_dir_all(&directory).unwrap();
        let engine = directory.join("lifecycle_upgrade.py");
        std::fs::write(&engine, "raise SystemExit(0)\n").unwrap();
        std::fs::write(
            root.join("SHA256SUMS"),
            format!(
                "{}  bundle/chentu/cli/src/chentu/lifecycle_upgrade.py\n",
                hash_file(&engine).unwrap()
            ),
        )
        .unwrap();
        let pin = hash_file(&root.join("SHA256SUMS")).unwrap();
        assert!(verify_driver(&root, &pin).is_ok());
        let extra = directory.join("injected.pyc");
        std::fs::write(&extra, "unexpected import").unwrap();
        assert!(verify_driver(&root, &pin).is_err());
        std::fs::remove_file(extra).unwrap();
        std::fs::write(&engine, "changed\n").unwrap();
        assert!(verify_driver(&root, &pin).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_missing_package_and_unknown_operation_before_execution() {
        assert!(run(&["production".into()]).is_err());
        assert!(run(&["execute".into(), "--package".into(), "/missing".into()]).is_err());
    }
}
