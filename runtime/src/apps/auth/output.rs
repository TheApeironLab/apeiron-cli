use super::error::{Error, Result};
use serde_json::{json, Value};
use std::io::{self, Write};

pub fn success(data: Value, meta: Option<Value>, compact: bool) -> Result<()> {
    let mut result = if compact {
        json!({"schemaVersion": 1, "ok": true, "data": data})
    } else if meta.is_some() {
        json!({"data":data})
    } else {
        data
    };
    if let Some(meta) = meta {
        result["meta"] = meta;
    }
    let text = if compact {
        serde_json::to_string(&result)
    } else {
        serde_json::to_string_pretty(&result)
    }
    .map_err(|_| Error::internal())?;
    writeln!(io::stdout().lock(), "{text}").map_err(|_| Error::internal())
}

pub fn failure(error: Error, compact: bool) -> i32 {
    let text = if compact {
        json!({"schemaVersion":1, "ok":false, "error":{
            "code":error.code, "message":error.message, "httpStatus":error.status, "outcome":error.outcome
        }})
        .to_string()
    } else {
        format!("{}: {}", error.code, error.message)
    };
    let _ = writeln!(io::stdout().lock(), "{text}");
    error.exit
}
