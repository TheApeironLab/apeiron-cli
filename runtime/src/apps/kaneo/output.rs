use serde_json::{json, Map, Value};
use std::io::{self, Write};

pub use super::super::auth::error::{Error, Result};

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

pub fn object(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().ok_or_else(Error::response)
}
pub fn array(value: &Value) -> Result<&Vec<Value>> {
    value.as_array().ok_or_else(Error::response)
}
pub fn string(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(Error::response)
}
pub fn nonempty(value: &Value) -> Result<&str> {
    string(value).and_then(|s| if s.is_empty() { Err(Error::response()) } else { Ok(s) })
}
pub fn integer(value: &Value, min: u64, max: u64) -> Result<u64> {
    value
        .as_u64()
        .filter(|v| *v >= min && *v <= max)
        .ok_or_else(Error::response)
}

pub fn project(value: &Value, keys: &[&str]) -> Result<Value> {
    let source = object(value)?;
    if keys.contains(&"id") {
        nonempty(&value["id"])?;
    }
    let mut projected = Map::new();
    for key in keys {
        if let Some(value) = source.get(*key) {
            if matches!(*key, "options" | "fieldOptions") {
                if !value.is_null() {
                    for item in array(value)? {
                        string(item)?;
                    }
                }
            } else if value.is_array() || value.is_object() {
                return Err(Error::response());
            }
            projected.insert((*key).into(), value.clone());
        }
    }
    Ok(Value::Object(projected))
}
pub fn list(value: &Value, keys: &[&str]) -> Result<Value> {
    Ok(json!({"items":array(value)?.iter().map(|v| project(v, keys)).collect::<Result<Vec<_>>>()?}))
}

pub fn identity(value: &Value) -> Result<Value> {
    for key in ["id", "name", "email"] {
        string(&value["user"][key])?;
    }
    if !matches!(string(&value["auth"]["type"])?, "bearer" | "cookie" | "api-key") {
        return Err(Error::response());
    }
    let auth = object(&value["auth"])?;
    let expires = auth.get("expiresAt").ok_or_else(Error::response)?;
    if !expires.is_null() {
        string(expires)?;
    }
    if let Some(client) = auth.get("clientId") {
        string(client)?;
    }
    let workspaces = array(&value["workspaces"] )?.iter().map(|workspace| {
        for key in ["id", "name", "slug", "role"] { string(&workspace[key])?; }
        // Identity schemas allow empty IDs, unlike resource projections.
        Ok(json!({"id":workspace["id"], "name":workspace["name"], "slug":workspace["slug"], "role":workspace["role"]}))
    }).collect::<Result<Vec<_>>>()?;
    let mut projected_auth = json!({"type":auth["type"],"expiresAt":expires});
    if let Some(client) = auth.get("clientId") {
        projected_auth["clientId"] = client.clone();
    }
    Ok(
        json!({"user":{"id":value["user"]["id"],"name":value["user"]["name"],"email":value["user"]["email"]},"auth":projected_auth,"workspaces":workspaces}),
    )
}
