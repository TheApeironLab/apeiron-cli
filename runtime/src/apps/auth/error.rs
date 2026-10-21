pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub exit: i32,
    pub outcome: &'static str,
    pub status: Option<u16>,
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>, exit: i32) -> Self {
        Self {
            code,
            message: message.into(),
            exit,
            outcome: "not_sent",
            status: None,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("INVALID_ARGUMENT", message, 2)
    }
    pub fn config(message: impl Into<String>) -> Self {
        Self::new("INVALID_CONFIG", message, 2)
    }
    pub fn internal() -> Self {
        Self::new("INTERNAL_ERROR", "CLI could not complete the command", 1)
    }
    pub fn response() -> Self {
        let mut error = Self::new("INVALID_RESPONSE", "Kaneo returned an incompatible response", 8);
        error.outcome = "not_applicable";
        error
    }
    pub fn unknown(message: &str, status: Option<u16>) -> Self {
        Self {
            code: "WRITE_OUTCOME_UNKNOWN",
            message: message.into(),
            exit: 9,
            outcome: "unknown",
            status,
        }
    }
}
