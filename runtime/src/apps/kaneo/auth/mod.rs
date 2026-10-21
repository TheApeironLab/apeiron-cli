use super::super::auth::{oidc, store};

use super::{
    api,
    config::{self, Config, Credential},
    output::{self, Error, Result},
};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use store::{Binding, Store, Stored, Tokens};

fn bearer(value: &str) -> Credential {
    Credential {
        value: value.into(),
        api_key: false,
        source: "local-login".into(),
    }
}
async fn verified(http: &Client, config: &Config, binding: &Binding, tokens: &oidc::TokenResponse) -> Result<Stored> {
    let refresh = tokens
        .refresh_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::new(
                "REAUTH_REQUIRED",
                "Identity provider did not provide a refresh grant",
                3,
            )
        })?;
    let me = output::identity(
        &api::request(
            http,
            config,
            Some(&bearer(&tokens.access_token)),
            Method::GET,
            &["me"],
            &[],
            None,
        )
        .await?,
    )?;
    let expiry = me["auth"]["expiresAt"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|v| v.timestamp_millis());
    if me["auth"]["type"] != "bearer"
        || me["auth"]["clientId"].as_str() != Some(&binding.client_id)
        || expiry.is_none_or(|e| e <= config::now())
    {
        return Err(Error::new(
            "REAUTH_REQUIRED",
            "Kaneo did not accept a current user login",
            3,
        ));
    }
    let mut workspaces: Vec<String> = output::array(&me["workspaces"])?
        .iter()
        .map(|w| output::string(&w["id"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    workspaces.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    Ok(Stored {
        version: 1,
        api_url: config.url.to_string(),
        binding: binding.clone(),
        tokens: Tokens {
            access_token: tokens.access_token.clone(),
            refresh_token: refresh.into(),
            expires_at: expiry.unwrap_or_default() as f64,
        },
        user_id: output::string(&me["user"]["id"])?.into(),
        workspaces,
        pending_refresh: None,
        sso: None,
    })
}

pub async fn credential(http: &Client, config: &Config) -> Result<Credential> {
    if let Some(credential) = super::super::sso::app_credential(http, "task", &config.url, config.timeout).await? {
        return Ok(Credential {
            value: credential,
            api_key: false,
            source: "sso".into(),
        });
    }
    if let Some(external) = config::external().await? {
        return Ok(external);
    }
    let store = Store::open("kaneo-cli", config.url.as_str()).await?;
    let mut previous = store.read()?.ok_or_else(|| {
        Error::new(
            "AUTH_REQUIRED",
            "Run apeiron sso login --server <APEIRON_URL>, or apeiron task auth login for an independent Task instance",
            3,
        )
    })?;
    if previous.pending_refresh.is_none() && previous.tokens.expires_at > (config::now() + 30_000) as f64 {
        return Ok(bearer(&previous.tokens.access_token));
    }
    if previous
        .pending_refresh
        .as_ref()
        .is_none_or(|t| t.expires_at <= (config::now() + 30_000) as f64)
    {
        let refresh = &previous
            .pending_refresh
            .as_ref()
            .unwrap_or(&previous.tokens)
            .refresh_token;
        let tokens = oidc::refresh(http, config.timeout, &previous.binding, refresh).await?;
        let refresh = tokens.refresh_token.filter(|s| !s.is_empty()).ok_or_else(|| {
            Error::new(
                "REAUTH_REQUIRED",
                "Identity provider did not provide a refresh grant",
                3,
            )
        })?;
        previous.pending_refresh = Some(Tokens {
            access_token: tokens.access_token,
            refresh_token: refresh,
            expires_at: config::now() as f64 + tokens.expires_in.unwrap_or(0.0) * 1000.0,
        });
        // A consumed refresh token must never become current again just because
        // /me is temporarily unavailable. Only a verified identity is exposed.
        store.save(&previous)?;
    }
    let pending = previous.pending_refresh.as_ref().ok_or_else(Error::internal)?;
    let tokens = oidc::TokenResponse {
        access_token: pending.access_token.clone(),
        refresh_token: Some(pending.refresh_token.clone()),
        expires_in: None,
    };
    let next = verified(http, config, &previous.binding, &tokens).await?;
    if next.user_id != previous.user_id || next.workspaces != previous.workspaces {
        return Err(Error::new(
            "REAUTH_REQUIRED",
            "Login identity or workspace changed; run apeiron task auth login explicitly",
            3,
        ));
    }
    store.save(&next)?;
    Ok(bearer(&next.tokens.access_token))
}
pub async fn login(http: &Client, config: &Config, open: bool) -> Result<Value> {
    let value = api::request(http, config, None, Method::GET, &["config"], &[], None).await?;
    let metadata = &value["cliAuth"];
    match metadata["type"].as_str() {
        Some("api-key") => return Err(unsupported()),
        Some("oidc") => {}
        _ => return Err(Error::response()),
    }
    let issuer = output::string(&metadata["issuer"])?;
    if url::Url::parse(issuer).is_err() {
        return Err(Error::response());
    }
    if output::string(&metadata["audience"])?.is_empty() {
        return Err(Error::response());
    }
    let client = output::object(metadata)?.get("clientId").ok_or_else(Error::response)?;
    if client.is_null() {
        return Err(unsupported());
    }
    let client = output::nonempty(client)?;
    let binding = Binding {
        issuer: issuer.into(),
        client_id: client.into(),
    };
    let tokens = oidc::login(http, config.timeout, &binding, open).await?;
    let value = verified(http, config, &binding, &tokens).await?;
    let store = Store::open("kaneo-cli", config.url.as_str()).await?;
    store.save(&value)?;
    let expiry = chrono::DateTime::from_timestamp_millis(value.tokens.expires_at as i64)
        .ok_or_else(Error::internal)?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    Ok(json!({"loggedIn":true,"userId":value.user_id,"expiresAt":expiry}))
}
fn unsupported() -> Error {
    Error::new(
        "MANAGED_AUTH_UNSUPPORTED",
        "This Kaneo instance does not provide CLI OIDC login",
        4,
    )
}
pub async fn logout(http: &Client, config: &Config) -> Result<Value> {
    let store = Store::open("kaneo-cli", config.url.as_str()).await?;
    let previous = store.read()?;
    let mut revoked = false;
    if let Some(previous) = previous {
        let token = &previous
            .pending_refresh
            .as_ref()
            .unwrap_or(&previous.tokens)
            .refresh_token;
        if oidc::revoke(http, config.timeout, &previous.binding, token)
            .await
            .is_err()
        {
            store.clear()?;
            return Err(Error::new(
                "REVOCATION_FAILED",
                "Local credentials were removed, but refresh-grant revocation could not be confirmed",
                8,
            ));
        }
        revoked = true;
    }
    store.clear()?;
    Ok(json!({"localCredentialsRemoved":true,"refreshGrantRevoked":revoked}))
}
