//! Interactive PKCE credentials are issued by Keycloak, never by Zygos.
use crate::{
    client::Client,
    error::{CliError, Result},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client as HttpClient, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Deserialize, Serialize)]
struct Credentials {
    base: String,
    issuer: String,
    client_id: String,
    token_endpoint: String,
    access_token: String,
    refresh_token: String,
    expires_at: u64,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn secret() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| CliError::new("internal_error", "Secure random source unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn http() -> Result<HttpClient> {
    Ok(HttpClient::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?)
}
fn path() -> Result<PathBuf> {
    std::env::var_os("ZYGOS_AUTH_FILE").map(PathBuf::from).filter(|p| p.is_absolute())
        .ok_or_else(|| CliError::usage("Set ZYGOS_AUTH_FILE to an absolute credential file path to opt in to private file storage"))
}
async fn save(c: &Credentials) -> Result<()> {
    let p = path()?;
    let parent = p
        .parent()
        .ok_or_else(|| CliError::usage("Invalid credential path"))?;
    // Persist atomically through a new private inode, never chmod a shared existing file.
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    use std::io::Write;
    file.write_all(
        &serde_json::to_vec(c).map_err(|_| CliError::integrity("Cannot encode credentials"))?,
    )?;
    file.as_file().sync_all()?;
    file.persist(p)
        .map_err(|_| CliError::new("internal_error", "Cannot save credential file"))?;
    Ok(())
}
async fn load(base: &str) -> Result<Option<Credentials>> {
    let Ok(p) = path() else { return Ok(None) };
    let metadata = match tokio::fs::symlink_metadata(&p).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CliError::usage("Credential file must have mode 0600"));
        }
    }
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(CliError::usage(
            "Credential file must be a bounded regular file",
        ));
    }
    let c: Credentials = serde_json::from_slice(&tokio::fs::read(&p).await?)
        .map_err(|_| CliError::usage("Invalid credential file"))?;
    if c.base != base.trim_end_matches('/') {
        return Err(CliError::usage(
            "Credential file belongs to another API; use a separate ZYGOS_AUTH_FILE",
        ));
    }
    let endpoint =
        Url::parse(&c.token_endpoint).map_err(|_| CliError::usage("Invalid stored issuer"))?;
    let issuer = Url::parse(&c.issuer).map_err(|_| CliError::usage("Invalid stored issuer"))?;
    if endpoint.origin() != issuer.origin() || !safe_url(&endpoint) {
        return Err(CliError::usage("Invalid stored token endpoint"));
    }
    Ok(Some(c))
}
fn safe_url(url: &Url) -> bool {
    (url.scheme() == "https"
        || (url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))))
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}
async fn token_response(response: reqwest::Response) -> Result<Value> {
    if !response.status().is_success() {
        return Err(CliError::new(
            "unauthorized",
            "Keycloak rejected credentials; run zygos auth login",
        ));
    }
    crate::client::read_json(response).await
}
fn text<'a>(v: &'a Value, name: &str) -> Result<&'a str> {
    v[name]
        .as_str()
        .ok_or_else(|| CliError::integrity("Invalid OIDC response"))
}

pub async fn access_token(base: &str) -> Result<Option<String>> {
    if let Ok(token) = std::env::var("ZYGOS_ACCESS_TOKEN") {
        return Ok(Some(token));
    }
    let Some(mut c) = load(base).await? else {
        return Ok(None);
    };
    if c.expires_at <= now() + 10 {
        let response = token_response(
            http()?
                .post(&c.token_endpoint)
                .form(&[
                    ("grant_type", "refresh_token"),
                    ("client_id", &c.client_id),
                    ("refresh_token", &c.refresh_token),
                ])
                .send()
                .await?,
        )
        .await?;
        c.access_token = text(&response, "access_token")?.into();
        if let Some(refresh) = response["refresh_token"].as_str() {
            c.refresh_token = refresh.into()
        }
        c.expires_at = now() + response["expires_in"].as_u64().unwrap_or(0);
        save(&c).await?;
    }
    Ok(Some(c.access_token))
}

pub async fn execute(base: &str, verb: &str) -> Result<Value> {
    let base = base.trim_end_matches('/');
    if verb == "logout" {
        if let Some(mut c) = load(base).await? {
            c.access_token.clear();
            c.refresh_token.clear();
            tokio::fs::remove_file(path()?).await?;
        }
        return Ok(json!({"logged_out": true}));
    }
    if verb == "status" {
        let token = access_token(base)
            .await?
            .ok_or_else(|| CliError::new("unauthorized", "Not logged in"))?;
        return Client::new(base, 15)?
            .with_token(Some(token))
            .get_json("v2/auth/me")
            .await;
    }
    path()?;
    let client = http()?;
    let config =
        crate::client::read_json(client.get(format!("{base}/v2/auth/config")).send().await?)
            .await?;
    let issuer = text(&config, "issuer")?;
    let client_id = text(&config, "client_id")?;
    let issuer_url = Url::parse(issuer).map_err(|_| CliError::usage("Invalid issuer"))?;
    if !safe_url(&issuer_url) {
        return Err(CliError::usage("OIDC issuer requires HTTPS"));
    }
    let metadata = crate::client::read_json(
        client
            .get(format!("{issuer}/.well-known/openid-configuration"))
            .send()
            .await?,
    )
    .await?;
    if text(&metadata, "issuer")? != issuer {
        return Err(CliError::integrity("OIDC issuer mismatch"));
    }
    let mut authorization = Url::parse(text(&metadata, "authorization_endpoint")?)
        .map_err(|_| CliError::integrity("Invalid authorization endpoint"))?;
    let token_endpoint = Url::parse(text(&metadata, "token_endpoint")?)
        .map_err(|_| CliError::integrity("Invalid token endpoint"))?;
    if authorization.origin() != issuer_url.origin()
        || token_endpoint.origin() != issuer_url.origin()
    {
        return Err(CliError::integrity("OIDC endpoint origin mismatch"));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:18991").await?;
    let redirect = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    let state = secret()?;
    let verifier = secret()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    authorization.query_pairs_mut().extend_pairs([
        ("client_id", client_id),
        ("response_type", "code"),
        ("redirect_uri", &redirect),
        ("scope", "openid profile email"),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ]);
    eprintln!("Open this URL to sign in:\n{authorization}");
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(180), listener.accept())
        .await
        .map_err(|_| CliError::new("unauthorized", "Login timed out"))??;
    let mut buf = [0u8; 8192];
    let count = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .map_err(|_| CliError::new("unauthorized", "Callback timed out"))??;
    let request =
        std::str::from_utf8(&buf[..count]).map_err(|_| CliError::usage("Invalid callback"))?;
    let target = request
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("GET "))
        .and_then(|line| line.strip_suffix(" HTTP/1.1"))
        .ok_or_else(|| CliError::usage("Invalid callback request"))?;
    let url = Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| CliError::usage("Invalid callback URL"))?;
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    if url.path() != "/callback" || query.get("state") != Some(&state) {
        return Err(CliError::new("unauthorized", "Callback state mismatch"));
    }
    let code = query
        .get("code")
        .ok_or_else(|| CliError::new("unauthorized", "Keycloak denied login"))?;
    let response = token_response(
        client
            .post(token_endpoint.as_str())
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client_id),
                ("redirect_uri", &redirect),
                ("code", code),
                ("code_verifier", &verifier),
            ])
            .send()
            .await?,
    )
    .await?;
    let credentials = Credentials {
        base: base.into(),
        issuer: issuer.into(),
        client_id: client_id.into(),
        token_endpoint: token_endpoint.into(),
        access_token: text(&response, "access_token")?.into(),
        refresh_token: text(&response, "refresh_token")?.into(),
        expires_at: now() + response["expires_in"].as_u64().unwrap_or(0),
    };
    // Server verifies signature, audience, tenant and role before credentials are persisted.
    let identity = Client::new(base, 15)?
        .with_token(Some(credentials.access_token.clone()))
        .get_json("v2/auth/me")
        .await?;
    save(&credentials).await?;
    let body = "Signed in. Close this browser.\n";
    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await?;
    Ok(json!({"logged_in": true, "user": identity}))
}
