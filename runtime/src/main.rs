//! Standalone CLI command dispatch; the machine runtime is owned by Apeiron.

mod app;
mod apps;

use anyhow::Result;

fn main() -> Result<()> {
    let application_args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match application_args.first().and_then(|s| s.to_str()) {
        Some("sudo") => std::process::exit(app::sudo::run(&application_args[1..])),
        Some("zygos") => std::process::exit(apps::run_zygos(&application_args[1..])),
        Some("sso") => std::process::exit(apps::run_sso(&application_args[1..])),
        Some("iam") => std::process::exit(apps::run_iam(&application_args[1..])),
        Some("task") => std::process::exit(apps::run_task(&application_args[1..])),
        _ => {}
    }
    if let Some(code) = app::run() {
        std::process::exit(code);
    }
    apeiron_runtime::run()
}
