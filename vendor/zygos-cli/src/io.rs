use crate::error::{CliError, Result};
use serde_json::{Value, value::RawValue};
use std::{
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub type Input = Box<dyn AsyncRead + Unpin + Send>;
pub type Output = Box<dyn AsyncWrite + Unpin + Send>;

pub async fn input(path: &str) -> Result<Input> {
    if path == "-" {
        Ok(Box::new(tokio::io::stdin()))
    } else {
        Ok(Box::new(tokio::fs::File::open(path).await?))
    }
}

pub async fn read_request(path: &str) -> Result<String> {
    let mut bytes = Vec::new();
    input(path)
        .await?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > 128 * 1024 {
        return Err(CliError::new(
            "resource_limit",
            "registration input exceeds 128 KiB",
        ));
    }
    String::from_utf8(bytes).map_err(|_| CliError::usage("registration input must be UTF-8 JSON"))
}

/// Validate JSON syntax, retaining the original numeric lexemes and duplicate keys
/// so the service's strict raw parser remains authoritative.
pub fn raw_object(text: &str) -> Result<&str> {
    let raw: &RawValue =
        serde_json::from_str(text).map_err(|_| CliError::usage("expected a JSON object"))?;
    if !raw.get().trim_start().starts_with('{') {
        return Err(CliError::usage("expected a JSON object"));
    }
    Ok(text)
}

pub fn encoded(value: &Value, compact: bool) -> Result<Vec<u8>> {
    let mut bytes = if compact {
        serde_json::to_vec(value)
    } else {
        serde_json::to_vec_pretty(value)
    }
    .map_err(|_| CliError::integrity("could not encode JSON result"))?;
    bytes.push(b'\n');
    Ok(bytes)
}
pub async fn emit(output: &mut Output, value: &Value, compact: bool) -> Result<()> {
    output.write_all(&encoded(value, compact)?).await?;
    output.flush().await?;
    Ok(())
}

pub fn reject_binary_tty(media_type: &str) -> Result<()> {
    if std::io::stdout().is_terminal()
        && !media_type.starts_with("text/")
        && !matches!(media_type, "application/json" | "application/x-ndjson")
    {
        return Err(CliError::usage(
            "binary export to a terminal is not allowed; use --output",
        ));
    }
    Ok(())
}

/// Temp file lives beside the destination; drop (including cancellation) removes it.
/// Persist replaces the destination atomically only after flush and fsync.
pub struct AtomicFile {
    file: tempfile::NamedTempFile,
    target: PathBuf,
}
impl AtomicFile {
    pub fn new(path: &str) -> Result<Self> {
        let target = std::path::absolute(path)?;
        let parent = target
            .parent()
            .ok_or_else(|| CliError::usage("output requires a parent directory"))?;
        let file = tempfile::Builder::new()
            .prefix(".zygos-")
            .tempfile_in(parent)?;
        Ok(Self { file, target })
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.file.write_all(bytes)?;
        Ok(())
    }
    pub fn commit(mut self) -> Result<String> {
        self.file.flush()?;
        self.file.as_file().sync_all()?;
        self.file
            .persist(&self.target)
            .map_err(|e| CliError::from(e.error))?;
        #[cfg(unix)]
        std::fs::File::open(self.target.parent().unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(self.target.to_string_lossy().into_owned())
    }
}
