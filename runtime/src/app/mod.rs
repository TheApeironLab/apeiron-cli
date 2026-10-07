mod access;
mod bootstrap;
mod chat;
mod cluster;
mod config;
mod deploy;
mod discovery;
mod models;
mod pairing;
mod process;
mod resources;
mod wizard;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
    time::Duration,
};

#[derive(Debug)]
pub struct Error {
    pub message: String,
    pub code: i32,
}
pub type Result<T> = std::result::Result<T, Error>;
pub fn fail(message: impl Into<String>, code: i32) -> Error {
    Error {
        message: message.into(),
        code,
    }
}
pub fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str> {
    options
        .get(name)
        .filter(|s| !s.trim().is_empty())
        .map(String::as_str)
        .ok_or_else(|| fail(format!("Missing --{name}"), 2))
}
pub fn parse(args: &[String], strings: &[&str], flags: &[&str]) -> Result<(Vec<String>, BTreeMap<String, String>)> {
    let mut pos = Vec::new();
    let mut options = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-h" {
            options.insert("help".into(), "true".into());
        } else if let Some(key) = arg.strip_prefix("--") {
            let (key, inline) = key.split_once('=').map_or((key, None), |(k, v)| (k, Some(v)));
            if options.contains_key(key) {
                return Err(fail(format!("Duplicate --{key}"), 2));
            }
            let value = if flags.contains(&key) {
                if inline.is_some() {
                    return Err(fail("Boolean flag has a value", 2));
                }
                "true".to_owned()
            } else if strings.contains(&key) {
                if let Some(value) = inline {
                    value.to_owned()
                } else {
                    i += 1;
                    args.get(i)
                        .filter(|v| !v.starts_with("--"))
                        .cloned()
                        .ok_or_else(|| fail(format!("Missing --{key} value"), 2))?
                }
            } else {
                return Err(fail(format!("Unknown --{key}"), 2));
            };
            options.insert(key.into(), value);
        } else {
            pos.push(arg.clone());
        }
        i += 1;
    }
    Ok((pos, options))
}
pub fn client(timeout: u64) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| fail("Cannot initialize HTTPS client", 9))
}
pub fn token_file(path: &Path) -> Result<String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| fail("Cannot open credential file", 7))?;
    let metadata = file.metadata().map_err(|_| fail("Cannot read credential file", 7))?;
    if !metadata.is_file() || metadata.len() > 16384 {
        return Err(fail("Credential must be a regular file under 16 KiB", 7));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { libc::getuid() } {
            return Err(fail("Credential file must be owned by current user with mode 0600", 7));
        }
    }
    let mut token = String::new();
    file.take(16385)
        .read_to_string(&mut token)
        .map_err(|_| fail("Invalid credential file", 7))?;
    let token = token.trim();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return Err(fail("Invalid credential file", 7));
    }
    Ok(token.to_owned())
}
pub fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| fail("Cannot create output file; use a new writable path", 5))?;
    file.write_all(bytes).map_err(|_| fail("Cannot write output file", 9))
}
pub fn output(mut data: Value, options: &BTreeMap<String, String>, schema: &str) -> Result<()> {
    if let Some(path) = options.get("out") {
        data["schema"] = json!(schema);
        private_write(Path::new(path), serde_json::to_string_pretty(&data).unwrap().as_bytes())?;
        data = json!({"saved":true,"path":path,"next_cursor":data.get("next_cursor")});
    }
    if options.contains_key("jsonl") || options.contains_key("json") {
        data["schema"] = json!(schema);
        println!("{data}");
        return Ok(());
    }
    println!("schema={schema}");
    let rows = data
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![data.clone()]);
    let mut keys = Vec::new();
    for row in &rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
        }
    }
    if !keys.is_empty() {
        println!("{}", keys.join("\t"));
    }
    for row in &rows {
        println!(
            "{}",
            keys.iter()
                .map(|key| {
                    let value = &row[key];
                    let text = match value {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        _ => value.to_string(),
                    };
                    text.chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .take(160)
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\t")
        );
    }
    if data.get("items").is_some() {
        println!("count={}", rows.len());
    }
    for key in ["next_cursor", "limited"] {
        if let Some(value) = data.get(key) {
            println!("{key}={value}");
        }
    }
    Ok(())
}
pub fn run() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let module = args.first().map(String::as_str).unwrap_or("--help");
    // Preserve runtime status; the old ontology-only status remains available through verify.
    if ["connect", "disconnect", "status", "serve", "cloud-receiver"].contains(&module)
        || (module.starts_with("--") && !["--help", "--version"].contains(&module))
    {
        return None;
    }
    let result = match module {
        "chat" => chat::run(&args[1..]),
        "init" => wizard::run(&args[1..]),
        "platform" => pairing::run(&args[1..]),
        "--version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "--help" | "-h" | "describe" => {
            println!("schema=apeiron.v1\ncommand\tusage\ninit\tapeiron init --help — browser installation wizard\nplatform\tapeiron platform entry --help — public entry and connections\nchat\tapeiron chat --help — Matrix messaging\nconnect\tapeiron connect --help — attach this machine\nstatus\tapeiron status — local runtime service\ndisconnect\tapeiron disconnect — stop local runtime\nonto\tapeiron onto <command> [flags]\n<module>\tAPEIRON_<MODULE>_BIN=/path/to/cli apeiron <module> <command>\nverify\tCheck ontology CLI entry point\n--version\tShow version");
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
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ontology"));
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
