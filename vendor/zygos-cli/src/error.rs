use serde_json::{Value, json};
use std::fmt;

#[derive(Debug)]
pub struct CliError {
    pub envelope: Value,
    pub exit_code: i32,
}

impl CliError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self::from_envelope(
            json!({"error": {"code": code, "message": message.into(), "detail": null}}),
        )
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new("bad_request", message)
    }

    pub fn integrity(message: impl Into<String>) -> Self {
        Self::new("integrity_error", message)
    }

    pub fn from_envelope(envelope: Value) -> Self {
        let code = envelope["error"]["code"]
            .as_str()
            .unwrap_or("internal_error");
        let exit_code = match code {
            "bad_request" => 2,
            "not_found" => 3,
            "conflict"
            | "identity_conflict"
            | "determinism_conflict"
            | "layout_conflict"
            | "ref_conflict"
            | "ref_state_conflict"
            | "fidelity_error" => 4,
            "validation_error" | "unsupported_profile" | "resource_limit" => 5,
            _ if code.ends_with("_conflict") => 4,
            _ => 1,
        };
        Self {
            envelope,
            exit_code,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.envelope["error"]["message"])
    }
}
impl std::error::Error for CliError {}
impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            Self::new(
                "service_unavailable",
                "output pipe closed; operation cancelled",
            )
        } else {
            Self::new(
                "internal_error",
                format!("file or stream I/O failed: {}", error.kind()),
            )
        }
    }
}
impl From<reqwest::Error> for CliError {
    fn from(error: reqwest::Error) -> Self {
        // Do not include URLs, query values, or credentials from reqwest's display text.
        let message = if error.is_timeout() {
            "API request timed out"
        } else {
            "API request failed"
        };
        Self::new("service_unavailable", message)
    }
}

pub type Result<T> = std::result::Result<T, CliError>;
