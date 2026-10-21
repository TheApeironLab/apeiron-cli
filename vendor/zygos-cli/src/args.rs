use crate::error::{CliError, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
pub struct CommandSpec {
    pub name: String,
    pub operation: String,
    pub positionals: Vec<PositionalSpec>,
    pub options: Vec<OptionSpec>,
    pub output: String,
    pub summary: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct PositionalSpec {
    pub name: String,
    pub required: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct OptionSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub short: Option<String>,
    #[serde(default)]
    pub multiple: bool,
}

#[derive(Debug)]
pub struct Invocation {
    pub spec: CommandSpec,
    pub positionals: Vec<String>,
    pub options: BTreeMap<String, Vec<String>>,
}
impl Invocation {
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options
            .get(name)
            .and_then(|v| v.last())
            .map(String::as_str)
    }
    pub fn flag(&self, name: &str) -> bool {
        self.options.contains_key(name)
    }
    pub fn require(&self, name: &str) -> Result<&str> {
        self.option(name)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CliError::usage(format!("--{name} is required")))
    }
    pub fn query(&self) -> Vec<(String, String)> {
        self.options
            .iter()
            .map(|(k, v)| (k.replace('-', "_"), v.last().unwrap().clone()))
            .collect()
    }
}

pub struct Parsed {
    pub base_url: String,
    pub compact: bool,
    pub timeout_seconds: u64,
    pub invocation: Option<Invocation>,
    pub help: Option<Value>,
    pub help_topic: Option<String>,
    pub view: crate::presentation::View,
    pub fields: Vec<String>,
}

pub fn catalog() -> Vec<CommandSpec> {
    let mut commands: Vec<CommandSpec> =
        serde_json::from_str(include_str!("../commands.json")).expect("compiled command catalog");
    for verb in ["login", "status", "logout"] {
        commands.push(CommandSpec {
            name: format!("auth {verb}"),
            operation: "localAuth".into(),
            positionals: vec![],
            options: vec![],
            output: "json".into(),
            summary: format!("Keycloak {verb}"),
        });
    }
    commands
}

/// Arguments exclude the executable name. Parsing and help require no running API.
pub fn parse(args: &[String]) -> Result<Parsed> {
    let mut result = Parsed {
        base_url: std::env::var("ZYGOS_API_BASE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8000".into()),
        compact: false,
        timeout_seconds: 300,
        invocation: None,
        help: None,
        help_topic: None,
        view: crate::presentation::View::Summary,
        fields: Vec::new(),
    };
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let token = &args[i];
        if token == "--" {
            rest.extend_from_slice(&args[i..]);
            break;
        }
        let (key, inline) = token
            .split_once('=')
            .map_or((token.as_str(), None), |(k, v)| (k, Some(v)));
        match key {
            "--json" => {
                if inline.is_some() {
                    return Err(CliError::usage("--json does not take a value"));
                }
            }
            "--compact" => {
                if inline.is_some() {
                    return Err(CliError::usage("--compact does not take a value"));
                }
                result.compact = true;
            }
            "--base-url" | "--timeout-seconds" | "--view" | "--fields" => {
                let value = match inline {
                    Some(v) => v.to_owned(),
                    None => {
                        i += 1;
                        args.get(i)
                            .filter(|s| !s.starts_with("--"))
                            .ok_or_else(|| CliError::usage(format!("{key} requires a value")))?
                            .clone()
                    }
                };
                if key == "--view" {
                    result.view = crate::presentation::View::parse(&value)?;
                } else if key == "--fields" {
                    result.fields = crate::presentation::parse_fields(&value)?;
                } else if key == "--base-url" {
                    result.base_url = value;
                } else {
                    result.timeout_seconds =
                        value.parse().ok().filter(|n| *n > 0).ok_or_else(|| {
                            CliError::usage("--timeout-seconds must be a positive integer")
                        })?;
                }
            }
            "--database-url" => {
                return Err(CliError::usage(
                    "Rust CLI uses HTTP; replace --database-url with --base-url",
                ));
            }
            _ => rest.push(token.clone()),
        }
        i += 1;
    }
    if rest.is_empty() || matches!(rest[0].as_str(), "help" | "--help" | "-h") {
        let topic = rest.get(1..).unwrap_or_default().join(" ");
        set_help(&mut result, &topic)?;
        return Ok(result);
    }
    if rest.len() == 2 && matches!(rest[1].as_str(), "help" | "--help" | "-h") {
        set_help(&mut result, &rest[0])?;
        return Ok(result);
    }
    if rest
        .last()
        .is_some_and(|s| matches!(s.as_str(), "--help" | "-h"))
    {
        let topic = rest[..rest.len() - 1].join(" ");
        if catalog()
            .iter()
            .any(|spec| spec.name == topic || spec.name.starts_with(&format!("{topic} ")))
        {
            set_help(&mut result, &topic)?;
            return Ok(result);
        }
    }
    if rest[0] == "lineage" && rest.get(1).is_some_and(|s| s != "show") {
        rest.insert(1, "show".into());
    }
    let spec = catalog()
        .into_iter()
        .find(|spec| {
            let parts = spec.name.split(' ').collect::<Vec<_>>();
            rest.len() >= parts.len() && parts.iter().zip(&rest).all(|(a, b)| a == b)
        })
        .ok_or_else(|| CliError::usage("unknown command; run zygos help"))?;
    let skip = spec.name.split(' ').count();
    let mut invocation = Invocation {
        spec,
        positionals: Vec::new(),
        options: BTreeMap::new(),
    };
    let mut positional_only = false;
    i = skip;
    while i < rest.len() {
        let token = &rest[i];
        if token == "--" && !positional_only {
            positional_only = true;
            i += 1;
            continue;
        }
        if !positional_only && matches!(token.as_str(), "--help" | "-h" | "help") {
            set_help(&mut result, &invocation.spec.name)?;
            return Ok(result);
        }
        if !positional_only && token.starts_with('-') && token != "-" {
            let (key, inline) = token
                .split_once('=')
                .map_or((token.as_str(), None), |(k, v)| (k, Some(v)));
            let opt = invocation
                .spec
                .options
                .iter()
                .find(|o| {
                    key == format!("--{}", o.name)
                        || o.short.as_ref().is_some_and(|s| key == format!("-{s}"))
                })
                .ok_or_else(|| CliError::usage(format!("unknown option: {key}")))?;
            let value = if opt.kind == "boolean" {
                if inline.is_some() {
                    return Err(CliError::usage(format!("{key} does not take a value")));
                }
                "true".into()
            } else {
                match inline {
                    Some(v) => v.to_owned(),
                    None => {
                        i += 1;
                        rest.get(i)
                            .filter(|s| !s.starts_with("--"))
                            .ok_or_else(|| CliError::usage(format!("{key} requires a value")))?
                            .clone()
                    }
                }
            };
            let values = invocation.options.entry(opt.name.clone()).or_default();
            if !opt.multiple {
                values.clear();
            }
            values.push(value);
        } else {
            invocation.positionals.push(token.clone());
        }
        i += 1;
    }
    if invocation.positionals.len() != invocation.spec.positionals.len() {
        return Err(CliError::usage(format!(
            "{} requires {} positional arguments",
            invocation.spec.name,
            invocation.spec.positionals.len()
        )));
    }
    if !result.fields.is_empty() && !crate::presentation::can_select(&invocation.spec.name) {
        return Err(CliError::usage(
            "--fields is only supported for JSON read commands; mutations, inspection and raw export retain their full result",
        ));
    }
    result.invocation = Some(invocation);
    Ok(result)
}

fn set_help(parsed: &mut Parsed, topic: &str) -> Result<()> {
    if !parsed.fields.is_empty() {
        return Err(CliError::usage(
            "--fields is not supported for help; use a command topic or --view full",
        ));
    }
    parsed.help_topic = (!topic.is_empty()).then(|| topic.to_owned());
    parsed.help = Some(crate::help::full(parsed.help_topic.as_deref())?);
    Ok(())
}
