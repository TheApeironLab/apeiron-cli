// Build-time access to the same Rust validation and rendering used by the CLI.
// This example is never included in release archives.
#[allow(dead_code)]
#[path = "../src/app/mod.rs"]
mod app;
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = match args.first().map(String::as_str) {
        Some("catalog") if args.len() == 2 => app::resources::catalog(std::path::Path::new(&args[1]), ""),
        Some("environment") if args.len() == 3 => (|| {
            let config = app::resources::json_file(std::path::Path::new(&args[1]), 1024 * 1024)?;
            let values = app::resources::yaml(std::path::Path::new(&args[2]))?;
            app::deploy::environment_for(&config, values)
        })(),
        Some("validate-release") if args.len() == 6 => (|| {
            let root = std::path::Path::new(&args[1]);
            let catalog = app::resources::catalog(root, &args[3])?;
            let apps = serde_json::from_str(&args[5]).map_err(|_| app::fail("Invalid apps JSON", 2))?;
            let mut plan = app::resources::plan(&catalog, &args[3], &apps)?;
            let base = std::path::Path::new(&args[2]).join(catalog["resourceDirectory"].as_str().unwrap_or(""));
            app::distribution::prepare(root, &base, &catalog, &mut plan, &args[3], &args[4],
                app::distribution::Preparation { offline: true, cancel: &Default::default(), progress: &|_| {} })?;
            app::resources::prepare_files(&plan, &base, true, &Default::default(), &|_| {})?;
            Ok(serde_json::json!({"selection":plan["installSelection"],"bootstrapImages":plan["bootstrapImages"]}))
        })(),
        Some("prepare-files") if args.len() == 4 => (|| {
            let plan = app::resources::json_file(std::path::Path::new(&args[1]), 4 * 1024 * 1024)?;
            app::resources::prepare_files(
                &plan,
                std::path::Path::new(&args[2]),
                args[3] == "offline",
                &Default::default(),
                &|_| {},
            )?;
            Ok(serde_json::Value::Null)
        })(),
        _ => Err(app::fail(
            "Usage: build-support catalog ROOT | environment CONFIG VALUES | prepare-files PLAN BASE MODE | validate-release CHENTU_ROOT RESOURCE_ROOT PROFILE TOPOLOGY APPS_JSON",
            2,
        )),
    };
    match result {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!("{}", error.message);
            std::process::exit(1);
        }
    }
}
