use super::output::{Error, Result};
use clap::ArgMatches;
use std::{
    env,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

pub const WORKSPACE_ID_HELP: &str = "Use the id from `apeiron task workspace list --json`, not its name or slug.";
pub const PROJECT_ID_HELP: &str = "Use the id from `apeiron task project list --workspace <ID> --json`.";
pub const STATUS_SLUG_HELP: &str = "Use a column slug from `apeiron task project columns <ID> --json`.";

pub struct Config {
    pub url: Url,
    pub timeout: Duration,
    pub workspace: Option<String>,
}
#[derive(Clone)]
pub struct Credential {
    pub value: String,
    pub api_key: bool,
    pub source: String,
}

pub fn text<'a>(args: &'a ArgMatches, name: &str) -> Option<&'a str> {
    args.try_get_one::<String>(name).ok().flatten().map(String::as_str)
}
pub fn flag(args: &ArgMatches, name: &str) -> bool {
    args.try_get_one::<bool>(name).ok().flatten().copied().unwrap_or(false)
}
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl Config {
    pub async fn read(args: &ArgMatches) -> Result<Self> {
        let raw_timeout = text(args, "timeout-ms").unwrap_or("10000");
        let millis = raw_timeout
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0 && *n <= 2_147_483_647 && raw_timeout.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| Error::config("--timeout-ms must be a positive integer up to 2147483647"))?;
        let raw = match text(args, "api-url")
            .map(str::to_owned)
            .or_else(|| env::var("KANEO_API_URL").ok())
        {
            Some(raw) => raw,
            None => super::super::sso::app_api_url("task").await?.unwrap_or_default(),
        };
        let error = || {
            Error::config("Set --api-url or KANEO_API_URL to an HTTP(S) API URL without credentials, query or fragment")
        };
        let mut url = Url::parse(&raw).map_err(|_| error())?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some_and(|v| !v.is_empty())
            || url.fragment().is_some_and(|v| !v.is_empty())
        {
            return Err(error());
        }
        let path = url.path().trim_end_matches('/');
        let path = if path.ends_with("/api") {
            path.to_owned()
        } else {
            format!("{path}/api")
        };
        url.set_path(&path);
        Ok(Self {
            url,
            timeout: Duration::from_millis(millis),
            workspace: text(args, "workspace")
                .map(str::to_owned)
                .or_else(|| env::var("KANEO_WORKSPACE_ID").ok()),
        })
    }
    pub fn required<'a>(&'a self, args: &'a ArgMatches, key: &str) -> Result<&'a str> {
        text(args, key)
            .or_else(|| {
                if key == "workspace" {
                    self.workspace.as_deref()
                } else {
                    None
                }
            })
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                let hint = match key {
                    "workspace" => WORKSPACE_ID_HELP,
                    "project" => PROJECT_ID_HELP,
                    _ => "Use --help for available options.",
                };
                Error::invalid(format!("Missing --{key}. {hint}"))
            })
    }
}

pub async fn external() -> Result<Option<Credential>> {
    let supplied: Vec<_> = [
        "KANEO_ACCESS_TOKEN",
        "KANEO_ACCESS_TOKEN_FILE",
        "KANEO_API_KEY",
        "KANEO_API_KEY_FILE",
    ]
    .into_iter()
    .filter_map(|k| env::var_os(k).map(|v| (k, v)))
    .collect();
    if supplied.len() > 1 {
        return Err(Error::config("Configure exactly one external credential source"));
    }
    let Some((name, raw)) = supplied.first() else {
        return Ok(None);
    };
    let value = if name.ends_with("_FILE") {
        tokio::fs::read_to_string(PathBuf::from(raw))
            .await
            .map_err(|_| Error::config("Cannot read the external credential file"))?
    } else {
        raw.to_string_lossy().into_owned()
    };
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(Error::config(
            "External credential must be nonempty and contain no whitespace",
        ));
    }
    Ok(Some(Credential {
        value: value.into(),
        api_key: name.starts_with("KANEO_API_KEY"),
        source: (*name).into(),
    }))
}

pub fn positive(value: Option<&str>, fallback: u64, max: u64) -> Result<u64> {
    let Some(value) = value else { return Ok(fallback) };
    value
        .parse::<u64>()
        .ok()
        .filter(|v| *v > 0 && *v <= max && value.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| Error::invalid(format!("Expected an integer from 1 to {max}")))
}
pub fn due_date(value: &str) -> Result<String> {
    if value.len() == 10 {
        if let Ok(day) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
            if day.format("%Y-%m-%d").to_string() == value {
                return Ok(format!("{value}T00:00:00Z"));
            }
        }
    }
    date(Some(value))
        .map_err(|_| Error::invalid("Due date must be YYYY-MM-DD or ISO 8601 with an explicit timezone"))?;
    Ok(value.to_owned())
}
pub fn date(value: Option<&str>) -> Result<Option<&str>> {
    if let Some(value) = value {
        // Match z.iso.datetime({offset:true}): uppercase T/Z, minute precision
        // allowed, no leap seconds, four-digit years and an explicit timezone.
        let valid = || -> Option<()> {
            let (day, time) = value.split_once('T')?;
            if day.len() != 10
                || !day.bytes().enumerate().all(|(i, b)| {
                    if i == 4 || i == 7 {
                        b == b'-'
                    } else {
                        b.is_ascii_digit()
                    }
                })
            {
                return None;
            }
            chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
            let time = if let Some(time) = time.strip_suffix('Z') {
                time
            } else {
                let (time, offset) = time.rsplit_once(['+', '-'])?;
                if offset.len() != 5 || offset.as_bytes()[2] != b':' {
                    return None;
                }
                pair(&offset.as_bytes()[..2], 23)?;
                pair(&offset.as_bytes()[3..], 59)?;
                time
            };
            let bytes = time.as_bytes();
            if bytes.len() < 5 || bytes[2] != b':' {
                return None;
            }
            pair(&bytes[..2], 23)?;
            pair(&bytes[3..5], 59)?;
            if bytes.len() != 5 {
                if bytes.len() < 8 || bytes[5] != b':' {
                    return None;
                }
                pair(&bytes[6..8], 59)?;
                if bytes.len() != 8
                    && (bytes.len() < 10 || bytes[8] != b'.' || !bytes[9..].iter().all(u8::is_ascii_digit))
                {
                    return None;
                }
            }
            Some(())
        };
        if valid().is_none() {
            return Err(Error::invalid("Dates must use ISO 8601 with an explicit timezone"));
        }
    }
    Ok(value)
}
fn pair(bytes: &[u8], max: u8) -> Option<()> {
    (bytes.iter().all(u8::is_ascii_digit) && (bytes[0] - b'0') * 10 + bytes[1] - b'0' <= max).then_some(())
}
pub async fn input(value: Option<&str>, file: Option<&str>) -> Result<Option<String>> {
    if value.is_some() && file.is_some() {
        return Err(Error::invalid(
            "Text and file input for the same field are mutually exclusive",
        ));
    }
    let Some(file) = file else {
        return Ok(value.map(str::to_owned));
    };
    let bytes = if file == "-" {
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        tokio::io::stdin().read_to_end(&mut bytes).await.map(|_| bytes)
    } else {
        tokio::fs::read(file).await
    }
    .map_err(|_| Error::invalid("Unable to read the requested input file"))?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}
