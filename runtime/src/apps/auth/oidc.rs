use super::{
    error::{Error, Result},
    now,
    store::Binding,
};
use base64::Engine;
use reqwest::Client;
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use url::Url;

pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<f64>,
}
struct Provider {
    issuer: Url,
    metadata: Value,
}
fn network(message: &str) -> Error {
    Error::new("NETWORK_ERROR", message, 8)
}
fn metadata_error() -> Error {
    network("Identity provider metadata could not be loaded or validated")
}

async fn provider(http: &Client, timeout: Duration, binding: &Binding) -> Result<Provider> {
    let issuer = Url::parse(&binding.issuer).map_err(|_| metadata_error())?;
    if !issuer.username().is_empty()
        || issuer.password().is_some()
        || issuer.query().is_some()
        || issuer.fragment().is_some()
        || !(issuer.scheme() == "https"
            || (issuer.scheme() == "http" && matches!(issuer.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
    {
        return Err(Error::config("OIDC requires HTTPS, except for a loopback test issuer"));
    }
    let mut discovery = issuer.clone();
    discovery.set_path(&format!(
        "{}/.well-known/openid-configuration",
        issuer.path().trim_end_matches('/')
    ));
    let response = http
        .get(discovery)
        .timeout(timeout)
        .send()
        .await
        .map_err(|_| metadata_error())?;
    if response.status() != 200 {
        return Err(metadata_error());
    }
    let metadata: Value = response.json().await.map_err(|_| metadata_error())?;
    let actual = metadata["issuer"].as_str().and_then(|s| Url::parse(s).ok());
    if actual.as_ref() != Some(&issuer) {
        return Err(metadata_error());
    }
    for key in [
        "token_endpoint",
        "device_authorization_endpoint",
        "revocation_endpoint",
        "jwks_uri",
    ] {
        if let Some(value) = metadata.get(key) {
            let endpoint = value
                .as_str()
                .and_then(|s| Url::parse(s).ok())
                .ok_or_else(metadata_error)?;
            if endpoint.origin() != issuer.origin() {
                return Err(Error::config("OIDC metadata has an unapproved endpoint origin"));
            }
        }
    }
    Ok(Provider { issuer, metadata })
}
impl Provider {
    fn endpoint(&self, key: &str) -> Result<Url> {
        let url = self.metadata[key]
            .as_str()
            .and_then(|s| Url::parse(s).ok())
            .ok_or_else(metadata_error)?;
        if url.origin() != self.issuer.origin() {
            return Err(Error::config("OIDC endpoint is outside the configured issuer origin"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(metadata_error());
        }
        Ok(url)
    }
}

async fn post(
    http: &Client,
    timeout: Duration,
    url: Url,
    fields: &[(&str, &str)],
) -> std::result::Result<(u16, Value), ()> {
    let response = http
        .post(url)
        .timeout(timeout)
        .form(fields)
        .send()
        .await
        .map_err(|_| ())?;
    let status = response.status().as_u16();
    if status != 200
        && ((400..500).contains(&status) && !json_content(&response)
            || response.headers().contains_key("www-authenticate"))
    {
        return Err(());
    }
    let value = response.json().await.map_err(|_| ())?;
    Ok((status, value))
}
fn json_content(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
}
fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .filter(|v: &f64| v.is_finite() && *v >= 0.0)
}
fn nonempty(value: &Value) -> std::result::Result<&str, ()> {
    value.as_str().filter(|s| !s.is_empty()).ok_or(())
}
fn tokens(value: Value, provider: &Provider, binding: &Binding) -> std::result::Result<TokenResponse, ()> {
    let access_token = nonempty(&value["access_token"])?.to_owned();
    if !["bearer", "dpop"].contains(&nonempty(&value["token_type"])?.to_ascii_lowercase().as_str()) {
        return Err(());
    }
    let refresh_token = value
        .get("refresh_token")
        .map(|v| nonempty(v).map(str::to_owned))
        .transpose()?;
    let expires_in = value.get("expires_in").map(|v| number(v).ok_or(())).transpose()?;
    if value.get("scope").is_some_and(|v| !v.is_string()) {
        return Err(());
    }
    if let Some(id_token) = value.get("id_token") {
        id_token_claims(nonempty(id_token)?, provider, binding)?;
    }
    Ok(TokenResponse {
        access_token,
        refresh_token,
        expires_in,
    })
}
fn id_token_claims(token: &str, provider: &Provider, binding: &Binding) -> std::result::Result<(), ()> {
    // Like oauth4webapi's processDeviceCodeResponse/RefreshTokenResponse,
    // this validates token-endpoint ID-token claims over the authenticated TLS
    // channel. Kaneo /me remains the authority for the access token and user;
    // unverified JWT contents are never used to construct business identity.
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(());
    }
    let decode = |part: &str| -> std::result::Result<Value, ()> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(part.trim_end_matches('='))
            .map_err(|_| ())?;
        serde_json::from_slice(&bytes).map_err(|_| ())
    };
    let header = decode(parts[0])?;
    let claims = decode(parts[1])?;
    if !header.is_object() || !claims.is_object() || header.get("crit").is_some() {
        return Err(());
    }
    let algorithm = nonempty(&header["alg"])?;
    let accepted = if let Some(algorithms) = provider.metadata["id_token_signing_alg_values_supported"].as_array() {
        algorithms.iter().any(|a| a.as_str() == Some(algorithm))
    } else {
        algorithm == "RS256"
    };
    if !accepted {
        return Err(());
    }
    for key in ["aud", "exp", "iat", "iss", "sub"] {
        if claims.get(key).is_none() {
            return Err(());
        }
    }
    if claims["iss"].as_str() != provider.metadata["issuer"].as_str()
        || claims["sub"].as_str().is_none()
        || !claims["iat"].is_number()
    {
        return Err(());
    }
    let now = (now() / 1000) as f64;
    if claims["exp"].as_f64().is_none_or(|v| v <= now - 30.0) {
        return Err(());
    }
    if claims
        .get("nbf")
        .is_some_and(|v| v.as_f64().is_none_or(|v| v > now + 30.0))
    {
        return Err(());
    }
    if claims
        .get("auth_time")
        .is_some_and(|v| v.as_f64().is_none_or(|n| n < 0.0))
    {
        return Err(());
    }
    match &claims["aud"] {
        Value::String(aud) if aud == &binding.client_id => {}
        Value::Array(aud) if aud.iter().any(|v| v.as_str() == Some(&binding.client_id)) => {
            if aud.len() != 1 && claims["azp"].as_str() != Some(&binding.client_id) {
                return Err(());
            }
        }
        _ => return Err(()),
    }
    Ok(())
}
pub async fn refresh(http: &Client, timeout: Duration, binding: &Binding, refresh: &str) -> Result<TokenResponse> {
    let provider = provider(http, timeout, binding).await?;
    let error = || network("Login refresh could not be completed; the previous credential was retained");
    let url = provider
        .endpoint("token_endpoint")
        .map_err(|e| if e.code == "INVALID_CONFIG" { e } else { error() })?;
    let (status, value) = post(
        http,
        timeout,
        url,
        &[
            ("client_id", &binding.client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
        ],
    )
    .await
    .map_err(|_| error())?;
    if (400..500).contains(&status) && value["error"] == "invalid_grant" {
        return Err(Error::new(
            "REAUTH_REQUIRED",
            "The login authorization expired; run apeiron task auth login",
            3,
        ));
    }
    if status != 200 {
        return Err(error());
    }
    tokens(value, &provider, binding).map_err(|_| error())
}
pub async fn revoke(http: &Client, timeout: Duration, binding: &Binding, token: &str) -> Result<()> {
    let provider = provider(http, timeout, binding).await?;
    let response = http
        .post(provider.endpoint("revocation_endpoint")?)
        .timeout(timeout)
        .form(&[
            ("client_id", binding.client_id.as_str()),
            ("token", token),
            ("token_type_hint", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|_| metadata_error())?;
    if response.status() != 200 {
        return Err(metadata_error());
    }
    Ok(())
}
pub async fn login(http: &Client, timeout: Duration, binding: &Binding, open: bool) -> Result<TokenResponse> {
    let provider = provider(http, timeout, binding).await?;
    let start_error = || network("Device authorization could not be started");
    let url = provider.endpoint("device_authorization_endpoint").map_err(|e| {
        if e.code == "INVALID_CONFIG" {
            e
        } else {
            start_error()
        }
    })?;
    let (status, code) = post(
        http,
        timeout,
        url,
        &[("client_id", &binding.client_id), ("scope", "openid profile email")],
    )
    .await
    .map_err(|_| start_error())?;
    if (400..500).contains(&status) && code["error"].is_string() {
        return Err(Error::new(
            "MANAGED_AUTH_UNSUPPORTED",
            "Identity provider did not accept the CLI device client",
            4,
        ));
    }
    if status != 200 {
        return Err(start_error());
    }
    let device = nonempty(&code["device_code"]).map_err(|_| start_error())?;
    let user = nonempty(&code["user_code"]).map_err(|_| start_error())?;
    let verification = nonempty(&code["verification_uri"]).map_err(|_| start_error())?;
    let target = code
        .get("verification_uri_complete")
        .map(nonempty)
        .transpose()
        .map_err(|_| start_error())?
        .unwrap_or(verification);
    let target_url = Url::parse(target).map_err(|_| start_error())?;
    if target_url.origin() != provider.issuer.origin() {
        return Err(Error::config("Device verification URL is outside the issuer origin"));
    }
    let expiry = number(&code["expires_in"]).ok_or_else(start_error)?;
    let mut interval = code
        .get("interval")
        .map(|v| v.as_f64().filter(|n| n.is_finite() && *n > 0.0).ok_or_else(start_error))
        .transpose()?
        .unwrap_or(5.0)
        .max(1.0);
    eprintln!("Authorize SSO at {target}\nCode: {user}");
    let deadline = now() as f64 + expiry * 1000.0;
    if open && open_browser(target).is_err() {
        eprintln!("Open the authorization URL in your browser.");
    }
    while (now() as f64) < deadline {
        let delay = (interval * 1000.0).min((deadline - now() as f64).max(0.0));
        tokio::time::sleep(std::time::Duration::from_secs_f64(delay / 1000.0)).await;
        if now() as f64 >= deadline {
            break;
        }
        let complete = || network("Device authorization could not be completed");
        let url = provider.endpoint("token_endpoint").map_err(|_| complete())?;
        let (status, value) = post(
            http,
            timeout,
            url,
            &[
                ("client_id", &binding.client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device),
            ],
        )
        .await
        .map_err(|_| complete())?;
        if (400..500).contains(&status) {
            match value["error"].as_str() {
                Some("authorization_pending") => continue,
                Some("slow_down") => {
                    interval += 5.0;
                    continue;
                }
                Some(_) => {
                    return Err(Error::new(
                        "REAUTH_REQUIRED",
                        "Device authorization was denied or expired",
                        3,
                    ))
                }
                None => return Err(complete()),
            }
        }
        if status != 200 {
            return Err(complete());
        }
        return tokens(value, &provider, binding).map_err(|_| complete());
    }
    Err(Error::new("REAUTH_REQUIRED", "Device authorization expired", 3))
}
fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = tokio::process::Command::new("rundll32.exe");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = tokio::process::Command::new("xdg-open");
    let mut child = command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Launchers may live as long as the browser. Reap independently so Device
    // Flow polling can complete while the authorization page is still open.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}
