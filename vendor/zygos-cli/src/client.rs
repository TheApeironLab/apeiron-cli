use crate::error::{CliError, Result};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Client as HttpClient, Method, RequestBuilder, Response, Url};
use serde_json::Value;
use std::time::Duration;

/// HTTP transport with redirects, ambient proxies and automatic retries disabled.
#[derive(Clone)]
pub struct Client {
    base: Url,
    http: HttpClient,
    token: Option<String>,
}

impl Client {
    pub fn new(base: &str, timeout_seconds: u64) -> Result<Self> {
        let mut base = Url::parse(base)
            .map_err(|_| CliError::usage("--base-url must be an absolute HTTP(S) URL"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(CliError::usage(
                "--base-url must be HTTP(S), without credentials, query or fragment",
            ));
        }
        let prefix = format!("{}/", base.path().trim_end_matches('/'));
        base.set_path(&prefix);
        let http = HttpClient::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(timeout_seconds))
            .user_agent(concat!("zygos-cli/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            base,
            http,
            token: None,
        })
    }
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }
    pub async fn get_json(&self, path: &str) -> Result<Value> {
        let url = self
            .base
            .join(path)
            .map_err(|_| CliError::usage("Invalid API path"))?;
        let mut request = self.http.get(url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        read_json(self.send(request).await?).await
    }
    pub(crate) async fn provider_json(
        &self,
        path: &str,
        method: Method,
        query: &[(String, String)],
        body: Option<String>,
        task_id: Option<&str>,
    ) -> Result<Value> {
        let url = self
            .base
            .join(path)
            .map_err(|_| CliError::usage("Invalid provider path"))?;
        let mut request = self.http.request(method, url).query(query);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(id) = task_id {
            request = request.header("evalscope-task-id", id);
        }
        if let Some(body) = body {
            request = request
                .header("Content-Type", "application/json")
                .body(body);
        }
        read_json(self.send(request).await?).await
    }
    pub fn request(
        &self,
        operation: &str,
        params: &[(&str, &str)],
        query: &[(String, String)],
    ) -> Result<RequestBuilder> {
        let operations: Value = serde_json::from_str(include_str!("operations.json")).unwrap();
        let spec = &operations[operation];
        let method = spec["method"]
            .as_str()
            .ok_or_else(|| CliError::integrity("unknown compiled API operation"))?;
        let mut path = spec["path"].as_str().unwrap().to_owned();
        for (name, value) in params {
            if value.is_empty() || matches!(*value, "." | "..") {
                return Err(CliError::usage("invalid empty or dot API path parameter"));
            }
            // Percent encoding is per parameter, never applied to the complete route.
            let encoded = utf8_percent_encode(value, NON_ALPHANUMERIC).to_string();
            path = path.replace(&format!("{{{name}}}"), &encoded);
        }
        if path.contains('{') {
            return Err(CliError::integrity("unresolved API path parameter"));
        }
        let url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| CliError::usage("invalid API URL"))?;
        let mut request = self
            .http
            .request(Method::from_bytes(method.as_bytes()).unwrap(), url)
            .query(query);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        Ok(request)
    }
    pub async fn send(&self, request: RequestBuilder) -> Result<Response> {
        let response = request.send().await?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let envelope = read_json(response).await.map_err(|_| {
            CliError::new(
                "service_unavailable",
                format!("API returned HTTP {status} without a valid error envelope"),
            )
        })?;
        if envelope["error"]["code"].as_str().is_none()
            || envelope["error"]["message"].as_str().is_none()
        {
            return Err(CliError::new(
                "service_unavailable",
                format!("API returned HTTP {status} without a valid error envelope"),
            ));
        }
        Err(CliError::from_envelope(envelope))
    }
    pub async fn json(
        &self,
        operation: &str,
        params: &[(&str, &str)],
        query: &[(String, String)],
        body: Option<String>,
    ) -> Result<Value> {
        let mut request = self.request(operation, params, query)?;
        if let Some(body) = body {
            request = request
                .header("Content-Type", "application/json")
                .body(body);
        }
        read_json(self.send(request).await?).await
    }
}

pub async fn read_json(mut response: Response) -> Result<Value> {
    const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > MAX_JSON_BYTES {
            return Err(CliError::new(
                "resource_limit",
                "API JSON response exceeds 16 MiB",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| CliError::integrity("API returned invalid JSON"))
}
