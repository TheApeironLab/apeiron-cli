use super::{
    auth,
    config::{Config, Credential, WORKSPACE_ID_HELP},
    output::{Error, Result},
};
use reqwest::{Client, Method};
use serde_json::Value;

pub struct Api {
    pub config: Config,
    pub http: Client,
    credential: Option<Credential>,
    pub write_sent: bool,
}
impl Api {
    pub fn new(config: Config) -> Result<Self> {
        let http = Client::builder()
            // Reqwest's system-proxy feature is disabled in Cargo.toml: honor
            // explicit proxy/NO_PROXY environment variables, without OS discovery.
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| Error::internal())?;
        Ok(Self {
            config,
            http,
            credential: None,
            write_sent: false,
        })
    }
    pub async fn credential(&mut self) -> Result<Credential> {
        if let Some(credential) = &self.credential {
            return Ok(credential.clone());
        }
        let credential = auth::credential(&self.http, &self.config).await?;
        self.credential = Some(credential.clone());
        Ok(credential)
    }
    pub async fn request(
        &mut self,
        method: Method,
        path: &[&str],
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        let credential = self.credential().await?;
        let write = method != Method::GET && method != Method::HEAD;
        if write {
            self.write_sent = true;
        }
        request(&self.http, &self.config, Some(&credential), method, path, query, body).await
    }
    pub async fn get(&mut self, path: &[&str], query: &[(&str, String)]) -> Result<Value> {
        self.request(Method::GET, path, query, None).await
    }
}

pub async fn request(
    http: &Client,
    config: &Config,
    credential: Option<&Credential>,
    method: Method,
    path: &[&str],
    query: &[(&str, String)],
    body: Option<Value>,
) -> Result<Value> {
    let mut url = config.url.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| Error::config("Request escaped the configured API"))?;
        for segment in path {
            segments.push(segment);
        }
    }
    if url.origin() != config.url.origin() || !url.path().starts_with(&format!("{}/", config.url.path())) {
        return Err(Error::config("Request escaped the configured API"));
    }
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query.iter().map(|(k, v)| (*k, v)));
    }
    let write = method != Method::GET && method != Method::HEAD;
    let mut request = http.request(method, url).timeout(config.timeout);
    if let Some(credential) = credential {
        request = if credential.api_key {
            request.header("x-api-key", &credential.value)
        } else {
            request.bearer_auth(&credential.value)
        };
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = async {
        let response = request.send().await?;
        let status = response.status().as_u16();
        // Read before classifying: the timeout also covers an incomplete body.
        let text = response.text().await?;
        Ok::<_, reqwest::Error>((status, text))
    }
    .await
    .map_err(|_| {
        if write {
            Error::unknown(
                "The write result is unknown; inspect the resource before continuing",
                None,
            )
        } else {
            let mut error = Error::new(
                "NETWORK_ERROR",
                "Kaneo could not be reached or its response timed out",
                8,
            );
            error.outcome = "not_applicable";
            error
        }
    })?;
    let (status, text) = response;
    if !(200..300).contains(&status) {
        if write && (!(400..500).contains(&status)) {
            return Err(Error::unknown(
                "Kaneo may have committed the write; inspect the resource before continuing",
                Some(status),
            ));
        }
        let (mut code, exit) = match status {
            401 => ("UNAUTHORIZED", 3),
            403 => ("FORBIDDEN", 4),
            404 => ("NOT_FOUND", 5),
            409 => ("CONFLICT", 6),
            429 => ("RATE_LIMITED", 7),
            400 | 422 => ("INVALID_REQUEST", 2),
            _ => ("SERVICE_UNAVAILABLE", 8),
        };
        let parsed = serde_json::from_str::<Value>(&text).ok();
        let service = parsed
            .as_ref()
            .and_then(|v| v["code"].as_str())
            .unwrap_or(if parsed.is_none() { &text } else { "" });
        if let Some(known) = ["UNTRUSTED_CLIENT", "MANAGED_AUTH_UNSUPPORTED", "REAUTH_REQUIRED"]
            .into_iter()
            .find(|v| *v == service)
        {
            code = known;
        }
        // Keep the v1 code/exit status stable. Only a recognized business reason
        // gets guidance; arbitrary response bodies may contain secrets or HTML.
        let reason = parsed
            .as_ref()
            .and_then(|v| v["code"].as_str().or_else(|| v["message"].as_str()))
            .unwrap_or(&text);
        let message = if status == 403 && reason.trim() == "PLATFORM_WORKSPACE_FORBIDDEN" {
            format!("PLATFORM_WORKSPACE_FORBIDDEN: Workspace access was denied. {WORKSPACE_ID_HELP}")
        } else {
            format!("Kaneo returned HTTP {status}")
        };
        return Err(Error {
            code,
            message,
            exit,
            outcome: if write { "rejected" } else { "not_applicable" },
            status: Some(status),
        });
    }
    serde_json::from_str(&text).map_err(|_| {
        if write {
            Error::unknown("Kaneo returned an invalid JSON response", Some(status))
        } else {
            Error {
                code: "INVALID_RESPONSE",
                message: "Kaneo returned an invalid JSON response".into(),
                exit: 8,
                outcome: "not_applicable",
                status: Some(status),
            }
        }
    })
}
