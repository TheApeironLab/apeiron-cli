mod chat;
mod register;
#[path = "../sudo.rs"]
pub(crate) mod sudo;
use chentu::app::{client, fail, output, parse, private_write, required, token_file, Error, Result};
use chentu::app::{config, process, resources, wizard};
use serde_json::{json, Value};
use std::{io::Read, path::Path, time::Duration};

fn chentu_build_info() -> Value {
    let mut info = chentu::build_info();
    let distribution: Value = serde_json::from_str(include_str!("../../../release/distribution-input.json"))
        .expect("validated distribution input");
    info["deployment"] = distribution["chentu"].clone();
    info
}

fn chentu_host() -> Result<chentu::Host> {
    let distribution: Value = serde_json::from_str(include_str!("../../../release/distribution-input.json"))
        .expect("validated distribution input");
    let deployment = serde_json::from_value(distribution["chentu"].clone())
        .map_err(|_| fail("Invalid compiled Chentu distribution", 2))?;
    Ok(chentu::Host {
        deployment: Some(deployment),
        embedded_scode: Some(sudo::build_info()["embedded"] == true),
    })
}
pub fn run_chentu(args: &[String]) -> i32 {
    match chentu_host() {
        Ok(host) => chentu::run_with_host(args, host),
        Err(error) => {
            eprintln!("{}", error.message);
            error.code
        }
    }
}
pub fn run() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let module = args.first().map(String::as_str).unwrap_or("--help");
    // Preserve runtime status; the old ontology-only status remains available through verify.
    if ["connect", "disconnect", "status", "serve", "cloud-receiver"].contains(&module)
        || (module.starts_with("--") && !["--help", "--version", "--build-info"].contains(&module))
    {
        return None;
    }
    let result = match module {
        "chentu" => return Some(run_chentu(&args[1..])),
        "app" | "init" | "platform" => return Some(run_chentu(&args)),
        "chat" => chat::run(&args[1..]),
        "register" => chentu_host()
            .and_then(chentu::host::configure)
            .and_then(|()| register::run(&args[1..])),
        "--build-info" => {
            println!(
                "{}",
                json!({"schemaVersion":1,"version":env!("APEIRON_VERSION"),"baseVersion":env!("CARGO_PKG_VERSION"),
                "releaseChannel":if env!("APEIRON_VERSION").contains("-nightly.") { "nightly" } else { "stable" },
                "revision":env!("APEIRON_SOURCE_REVISION"),"clean":env!("APEIRON_SOURCE_CLEAN") == "true",
                "sudocode":sudo::build_info(),"chentu":chentu_build_info(),
                "rustc":env!("APEIRON_BUILD_RUSTC"),"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH})
            );
            Ok(())
        }
        "--version" => {
            println!("{}", env!("APEIRON_VERSION"));
            Ok(())
        }
        "--help" | "-h" | "describe" => {
            println!("schema=apeiron.v1\ncommand\tusage\nsudo\tapeiron sudo [scode arguments] — local AI agent\nchentu\tapeiron chentu --help — installation and application lifecycle\napp\tCompatibility alias for apeiron chentu app\nsso\tapeiron sso login --server <URL> — platform SSO\ntask\tapeiron task --help — tasks, projects and workspaces\niam\tapeiron iam --help — organization, members and App access\nregister\tapeiron register --help — gateway enrollment\ninit\tCompatibility alias for apeiron chentu init\nplatform\tapeiron platform entry --help — public entry and connections\nchat\tapeiron chat --help — Matrix messaging\nconnect\tapeiron connect --help — attach this machine\nstatus\tapeiron status — local runtime service\ndisconnect\tapeiron disconnect — stop local runtime\nzygos\tapeiron zygos --help — datasets, training, models and evaluations\nonto\tapeiron onto <command> [flags]\n<module>\tAPEIRON_<MODULE>_BIN=/path/to/cli apeiron <module> <command>\nverify\tCheck ontology CLI entry point\n--version\tShow version");
            Ok(())
        }
        _ => forward(module, &args[1..]),
    };
    Some(match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("schema: apeiron.v1\nerror: {}", e.message);
            if (400..500).contains(&e.code) {
                2
            } else if e.code >= 500 {
                9
            } else {
                e.code
            }
        }
    })
}
fn forward(module: &str, args: &[String]) -> Result<()> {
    if !config::matches(r"^[a-z][a-z0-9-]*$", module) {
        return Err(fail("Invalid module name", 2));
    }
    let root = std::env::var_os("APEIRON_ONTO_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("../ontology")))
                .unwrap_or_else(|| std::path::PathBuf::from("../ontology"))
        });
    let entry = root.join("apps/onto/cli/main.ts");
    if module == "verify" {
        let present = entry.is_file();
        println!(
            "schema=apeiron.v1\nmodule\tentry\tavailable\nonto\t{}\t{present}",
            entry.display()
        );
        return if present {
            Ok(())
        } else {
            Err(fail("Ontology checkout missing; set APEIRON_ONTO_ROOT", 4))
        };
    }
    let env = format!("APEIRON_{}_BIN", module.to_uppercase().replace('-', "_"));
    let mut command = if let Some(binary) = std::env::var_os(&env) {
        std::process::Command::new(binary)
    } else if module == "onto" {
        if !entry.is_file() {
            return Err(fail(
                "Ontology checkout missing; set APEIRON_ONTO_ROOT to its repository path",
                4,
            ));
        }
        let mut command =
            std::process::Command::new(std::env::var_os("APEIRON_BUN_BIN").unwrap_or_else(|| "bun".into()));
        command.arg(entry).current_dir(root);
        command
    } else {
        return Err(fail(
            format!("Module {module} is not configured; set {env} to its executable"),
            4,
        ));
    };
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let _ = command.exec();
        Err(fail(format!("Cannot start {module}"), 4))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|_| fail(format!("Cannot start {module}"), 4))?;
        std::process::exit(status.code().unwrap_or(9));
    }
}
