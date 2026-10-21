//! One user SSO session; application credentials are exchanged lazily and never
//! reuse the browser's refresh grant. Confidential exchange stays at the server.
use super::auth::{
    error::{Error, Result},
    now, oidc,
    store::{AppToken, Binding, SsoSession, Store, Stored, Tokens},
};
use clap::{Arg, Command};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::{collections::BTreeMap, ffi::OsString, time::Duration};
use url::Url;

const TIMEOUT: Duration = Duration::from_secs(10);
const NAMESPACE: &str = "apeiron-sso";
const PROFILE: &str = "current";
fn required() -> Error {
    Error::new("SSO_LOGIN_REQUIRED", "Run apeiron sso login --server <APEIRON_URL>", 3)
}
fn invalid() -> Error {
    Error::new("INVALID_RESPONSE", "Invalid platform SSO response", 8)
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key].as_str().filter(|v| !v.is_empty()).ok_or_else(invalid)
}
fn endpoint(raw: &str) -> Result<Url> {
    let mut url = Url::parse(raw).map_err(|_| Error::config("Invalid SSO server URL"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https"
            || (url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
    {
        return Err(Error::config("SSO requires HTTPS, except for a loopback test server"));
    }
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/api") {
        path.to_owned()
    } else {
        format!("{path}/api")
    };
    url.set_path(&path);
    Ok(url)
}
async fn request(
    http: &Client,
    server: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
    timeout: Duration,
) -> Result<Value> {
    let mut url = endpoint(server)?;
    url.set_path(&format!("{}/sso/{path}", url.path()));
    let mut req = http
        .request(if body.is_some() { Method::POST } else { Method::GET }, url)
        .timeout(timeout);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().await.map_err(|_| {
        Error::new(
            "NETWORK_ERROR",
            "SSO platform is unavailable; no credentials were sent to another origin",
            8,
        )
    })?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 => required(),
            403 => Error::new(
                "APP_ACCESS_DENIED",
                "SSO user is not authorized for this application",
                3,
            ),
            404 | 503 => Error::new("SSO_UNAVAILABLE", "Platform SSO CLI authorization is not configured", 4),
            _ => Error::new("NETWORK_ERROR", "SSO authorization could not be completed", 8),
        });
    }
    response.json().await.map_err(|_| invalid())
}
async fn store() -> Result<Store> {
    Store::open(NAMESPACE, PROFILE).await
}
fn session(record: &Stored) -> Result<&SsoSession> {
    record.sso.as_ref().ok_or_else(required)
}
fn summary(record: &Stored) -> Result<Value> {
    let s = session(record)?;
    Ok(
        json!({"loggedIn":true,"server":s.server,"issuer":record.binding.issuer,"subject":record.user_id,"tenant":s.tenant,"expiresAt":record.tokens.expires_at as i64}),
    )
}
fn token_set(tokens: oidc::TokenResponse) -> Result<Tokens> {
    let lifetime = tokens
        .expires_in
        .filter(|v| *v > 0.0 && *v <= 300.0)
        .ok_or_else(invalid)?;
    Ok(Tokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token.filter(|v| !v.is_empty()).ok_or_else(invalid)?,
        expires_at: now() as f64 + lifetime * 1000.0,
    })
}
async fn validate(http: &Client, record: &Stored, tokens: &Tokens, timeout: Duration) -> Result<Value> {
    let result = request(
        http,
        &session(record)?.server,
        "session",
        Some(&tokens.access_token),
        None,
        timeout,
    )
    .await?;
    if text(&result, "issuer")? != record.binding.issuer
        || text(&result, "client_id")? != record.binding.client_id
        || text(&result, "subject")? != record.user_id
        || text(&result, "tenant")? != session(record)?.tenant
    {
        return Err(Error::new(
            "SSO_IDENTITY_CHANGED",
            "SSO identity changed; run apeiron sso login explicitly",
            3,
        ));
    }
    Ok(result)
}
async fn ensure(http: &Client, store: &Store, record: &mut Stored, timeout: Duration) -> Result<()> {
    if session(record)?.refresh_in_flight {
        return Err(required());
    }
    if record.pending_refresh.is_none() && record.tokens.expires_at > (now() + 30_000) as f64 {
        return Ok(());
    }
    if record.pending_refresh.is_none() {
        // Persist before sending: after a crash/ambiguous response the consumed
        // source refresh grant must never be retried by another command.
        record.sso.as_mut().ok_or_else(required)?.refresh_in_flight = true;
        store.save(record)?;
        let next = oidc::refresh(http, timeout, &record.binding, &record.tokens.refresh_token)
            .await
            .map_err(|mut error| {
                if error.code == "REAUTH_REQUIRED" {
                    error.message = required().message;
                }
                error
            })?;
        record.pending_refresh = Some(token_set(next)?);
        record.sso.as_mut().ok_or_else(required)?.refresh_in_flight = false;
        store.save(record)?;
    }
    let tokens = record.pending_refresh.as_ref().ok_or_else(invalid)?;
    let identity = validate(http, record, tokens, timeout).await?;
    record.tokens = record.pending_refresh.take().ok_or_else(invalid)?;
    let expiry = identity["expires_at"]
        .as_i64()
        .filter(|v| *v > now())
        .ok_or_else(invalid)?;
    record.tokens.expires_at = record.tokens.expires_at.min(expiry as f64);
    record.sso.as_mut().ok_or_else(required)?.app_tokens.clear();
    store.save(record)
}
/// IAM is served by the same platform API and audience as the source SSO.
/// Never accept an alternate server URL that could receive its credential.
pub async fn platform_credential(http: &Client, timeout: Duration) -> Result<(Url, String)> {
    let store = store().await?;
    let mut record = store.read()?.ok_or_else(required)?;
    ensure(http, &store, &mut record, timeout).await?;
    let url = endpoint(&session(&record)?.server)?;
    Ok((url, record.tokens.access_token.clone()))
}

pub async fn app_api_url(app: &str) -> Result<Option<String>> {
    let store = store().await?;
    let Some(record) = store.read()? else { return Ok(None) };
    Ok(Some(session(&record)?.endpoints.get(app).cloned().ok_or_else(
        || Error::new("APP_UNAVAILABLE", "Application is not enabled on this SSO platform", 4),
    )?))
}
pub async fn app_credential(http: &Client, app: &str, api_url: &Url, timeout: Duration) -> Result<Option<String>> {
    let store = store().await?;
    let Some(mut record) = store.read()? else {
        return Ok(None);
    };
    let expected = session(&record)?.endpoints.get(app).ok_or_else(required)?;
    if endpoint(expected)? != *api_url {
        return Err(Error::config(
            "Application URL differs from the SSO platform; select the matching login explicitly",
        ));
    }
    ensure(http, &store, &mut record, timeout).await?;
    if let Some(token) = session(&record)?
        .app_tokens
        .get(app)
        .filter(|v| v.expires_at > now() + 30_000)
    {
        if endpoint(&token.api_url)? != *api_url {
            return Err(invalid());
        }
        return Ok(Some(token.access_token.clone()));
    }
    let response = request(
        http,
        &session(&record)?.server,
        "token",
        Some(&record.tokens.access_token),
        Some(json!({"app":app})),
        timeout,
    )
    .await?;
    if text(&response, "issuer")? != record.binding.issuer
        || text(&response, "subject")? != record.user_id
        || text(&response, "tenant")? != session(&record)?.tenant
        || endpoint(text(&response, "api_url")?)? != *api_url
    {
        return Err(invalid());
    }
    let access_token = text(&response, "access_token")?.to_owned();
    let expires_at = response["expires_at"]
        .as_i64()
        .filter(|v| *v > now() + 5_000 && *v <= now() + 300_000)
        .ok_or_else(invalid)?;
    record.sso.as_mut().ok_or_else(required)?.app_tokens.insert(
        app.into(),
        AppToken {
            api_url: api_url.to_string(),
            access_token: access_token.clone(),
            expires_at,
        },
    );
    store.save(&record)?;
    Ok(Some(access_token))
}
#[derive(serde::Serialize, serde::Deserialize)]
struct PendingLogin {
    server: String,
    binding: Binding,
    endpoints: BTreeMap<String, String>,
    device: oidc::DeviceAuthorization,
    tokens: Option<Tokens>,
    exchange_in_flight: bool,
}
fn pending_summary(pending: &PendingLogin) -> Value {
    json!({"loggedIn":false,"authorizationPending":true,
        "verificationUri":pending.device.verification_uri,"userCode":pending.device.user_code,
        "expiresAt":pending.device.expires_at,
        "retryAfterMs":(pending.device.next_poll_at-now()).max(0),
        "nextCommand":"apeiron sso login --complete --json"})
}
async fn save_login(
    http: &Client,
    store: &Store,
    server: String,
    binding: Binding,
    endpoints: BTreeMap<String, String>,
    mut tokens: Tokens,
) -> Result<Value> {
    let identity = request(http, &server, "session", Some(&tokens.access_token), None, TIMEOUT).await?;
    if text(&identity, "issuer")? != binding.issuer || text(&identity, "client_id")? != binding.client_id {
        return Err(invalid());
    }
    let expiry = identity["expires_at"]
        .as_i64()
        .filter(|v| *v > now())
        .ok_or_else(invalid)?;
    tokens.expires_at = tokens.expires_at.min(expiry as f64);
    let record = Stored {
        version: 1,
        api_url: PROFILE.into(),
        binding,
        tokens,
        user_id: text(&identity, "subject")?.into(),
        workspaces: vec![],
        pending_refresh: None,
        sso: Some(SsoSession {
            server,
            tenant: text(&identity, "tenant")?.into(),
            endpoints,
            app_tokens: BTreeMap::new(),
            refresh_in_flight: false,
        }),
    };
    store.save(&record)?;
    store.clear_pending_login()?;
    summary(&record)
}
async fn complete_login(http: &Client, store: &Store) -> Result<Value> {
    let Some(mut pending) = store.read_pending_login::<PendingLogin>()? else {
        let Some(mut record) = store.read()? else {
            return Err(Error::new(
                "SSO_LOGIN_REQUIRED",
                "Start apeiron sso login --server <APEIRON_URL> --no-open --no-wait",
                3,
            ));
        };
        ensure(http, store, &mut record, TIMEOUT).await?;
        validate(http, &record, &record.tokens, TIMEOUT).await?;
        return summary(&record);
    };
    if pending.exchange_in_flight {
        return Err(Error::new(
            "SSO_LOGIN_REQUIRED",
            "Device token exchange outcome is unknown; start a new SSO login with --no-wait",
            3,
        ));
    }
    if pending.tokens.is_none() {
        if now() >= pending.device.expires_at {
            store.clear_pending_login()?;
            return Err(Error::new(
                "REAUTH_REQUIRED",
                "Device authorization expired; start a new SSO login with --no-wait",
                3,
            ));
        }
        if now() < pending.device.next_poll_at {
            return Ok(pending_summary(&pending));
        }
        // A successful device exchange consumes the grant. Never replay an
        // ambiguous response; retain returned tokens before session validation.
        pending.exchange_in_flight = true;
        store.save_pending_login(&pending)?;
        let next = match oidc::poll_device(http, TIMEOUT, &pending.binding, &mut pending.device).await {
            Ok(next) => next,
            Err(error) => {
                if error.code == "REAUTH_REQUIRED" {
                    store.clear_pending_login()?;
                }
                return Err(error);
            }
        };
        pending.exchange_in_flight = false;
        pending.tokens = next.map(token_set).transpose()?;
        store.save_pending_login(&pending)?;
        if pending.tokens.is_none() {
            return Ok(pending_summary(&pending));
        }
    }
    save_login(
        http,
        store,
        pending.server,
        pending.binding,
        pending.endpoints,
        pending.tokens.ok_or_else(invalid)?,
    )
    .await
}
fn parser() -> Command {
    Command::new("sso")
        .bin_name("apeiron sso")
        .about("Sign in once to the platform and use authorized applications")
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(clap::ArgAction::SetTrue),
        )
        .subcommand(
            Command::new("login")
                .arg(Arg::new("server").long("server").env("APEIRON_SERVER"))
                .arg(Arg::new("no-open").long("no-open").action(clap::ArgAction::SetTrue))
                .arg(
                    Arg::new("no-wait")
                        .long("no-wait")
                        .help("Return the device URL immediately; finish with login --complete")
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("complete")
                        .long("complete")
                        .help("Complete a pending device login with at most one bounded poll")
                        .conflicts_with_all(["no-open", "no-wait"])
                        .action(clap::ArgAction::SetTrue),
                ),
        )
        .subcommand(Command::new("status"))
        .subcommand(Command::new("logout"))
        .subcommand_required(true)
}
async fn execute(args: &clap::ArgMatches) -> Result<Value> {
    let http = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| Error::internal())?;
    let (command, options) = args.subcommand().ok_or_else(required)?;
    if command == "login" {
        if options.get_flag("complete") {
            if options.value_source("server") == Some(clap::parser::ValueSource::CommandLine) {
                return Err(Error::invalid(
                    "--complete uses the saved authorization server; omit --server",
                ));
            }
            let store = store().await?;
            return complete_login(&http, &store).await;
        }
        let server = endpoint(options.get_one::<String>("server").ok_or_else(required)?)?.to_string();
        let metadata = request(&http, &server, "config", None, None, TIMEOUT).await?;
        let binding = Binding {
            issuer: text(&metadata, "issuer")?.into(),
            client_id: text(&metadata, "client_id")?.into(),
        };
        let endpoints: BTreeMap<String, String> =
            serde_json::from_value(metadata["apps"].clone()).map_err(|_| invalid())?;
        for value in endpoints.values() {
            endpoint(value)?;
        }
        if options.get_flag("no-wait") {
            let store = store().await?;
            if let Some(pending) = store.read_pending_login::<PendingLogin>()? {
                if pending.server == server && now() < pending.device.expires_at && !pending.exchange_in_flight {
                    return Ok(pending_summary(&pending));
                }
                // Do not discard an obtained refresh grant during a restart.
                if let Some(tokens) = pending.tokens {
                    oidc::revoke(&http, TIMEOUT, &pending.binding, &tokens.refresh_token).await?;
                }
            }
            let pending = PendingLogin {
                server,
                binding: binding.clone(),
                endpoints,
                device: oidc::begin_device(&http, TIMEOUT, &binding).await?,
                tokens: None,
                exchange_in_flight: false,
            };
            store.save_pending_login(&pending)?;
            return Ok(pending_summary(&pending));
        }
        let tokens = token_set(oidc::login(&http, TIMEOUT, &binding, !options.get_flag("no-open")).await?)?;
        let store = store().await?;
        return save_login(&http, &store, server, binding, endpoints, tokens).await;
    }
    let store = store().await?;
    if command == "logout" {
        let pending = store.read_pending_login::<PendingLogin>()?;
        let record = store.read()?;
        store.clear_pending_login()?;
        store.clear()?;
        let mut grants = Vec::new();
        if let Some(pending) = pending {
            if let Some(tokens) = pending.tokens {
                grants.push((pending.binding, tokens.refresh_token));
            }
        }
        if let Some(record) = record {
            if !session(&record)?.refresh_in_flight {
                let refresh = record
                    .pending_refresh
                    .as_ref()
                    .unwrap_or(&record.tokens)
                    .refresh_token
                    .clone();
                grants.push((record.binding, refresh));
            }
        }
        let revoked = !grants.is_empty();
        let mut outcome = Ok(());
        for (binding, refresh) in grants {
            if let Err(error) = oidc::revoke(&http, TIMEOUT, &binding, &refresh).await {
                outcome = Err(error);
            }
        }
        outcome?;
        return Ok(json!({"loggedIn":false,"revoked":revoked}));
    }
    let Some(mut record) = store.read()? else {
        if let Some(pending) = store.read_pending_login::<PendingLogin>()? {
            return Ok(pending_summary(&pending));
        }
        return Ok(json!({"loggedIn":false}));
    };
    ensure(&http, &store, &mut record, TIMEOUT).await?;
    validate(&http, &record, &record.tokens, TIMEOUT).await?;
    summary(&record)
}
pub fn run(argv: &[OsString]) -> i32 {
    let json = argv.iter().any(|v| v == "--json");
    let result =
        parser().try_get_matches_from(std::iter::once(OsString::from("apeiron sso")).chain(argv.iter().cloned()));
    let args = match result {
        Ok(args) => args,
        Err(e) if matches!(e.kind(), clap::error::ErrorKind::DisplayHelp) => {
            print!("{e}");
            return 0;
        }
        Err(_) => {
            eprintln!("INVALID_ARGUMENT: use apeiron sso --help");
            return 2;
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(_) => return 1,
    };
    let result = runtime.block_on(execute(&args));
    runtime.shutdown_timeout(Duration::ZERO);
    match result {
        Ok(value) => {
            if json {
                println!("{}", json!({"schemaVersion":1,"ok":true,"data":value}));
            } else {
                println!("schema=apeiron.sso.v1");
                for (key, value) in value.as_object().into_iter().flatten() {
                    println!("{key}\t{value}");
                }
            }
            0
        }
        Err(error) => {
            if json {
                eprintln!(
                    "{}",
                    json!({"schemaVersion":1,"ok":false,"error":{"code":error.code,"message":error.message}})
                );
            } else {
                eprintln!("{}: {}", error.code, error.message);
            }
            error.exit
        }
    }
}
