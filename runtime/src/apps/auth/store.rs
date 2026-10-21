//! Private atomic credential storage with cross-process rotation locking.
use super::{
    error::{Error, Result},
    home, now,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, UNIX_EPOCH},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub issuer: String,
    pub client_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: f64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct AppToken {
    pub api_url: String,
    pub access_token: String,
    pub expires_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SsoSession {
    pub server: String,
    pub tenant: String,
    pub endpoints: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub app_tokens: std::collections::BTreeMap<String, AppToken>,
    #[serde(default)]
    pub refresh_in_flight: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored {
    pub version: u8,
    pub api_url: String,
    #[serde(flatten)]
    pub binding: Binding,
    #[serde(flatten)]
    pub tokens: Tokens,
    pub user_id: String,
    pub workspaces: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_refresh: Option<Tokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sso: Option<SsoSession>,
}
struct Lock {
    path: PathBuf,
    mtime: i64,
    seconds: bool,
    last_update: i64,
    compromised: bool,
}
pub struct Store {
    directory: PathBuf,
    api_url: String,
    lock: Arc<Mutex<Lock>>,
    heartbeat: tokio::task::JoinHandle<()>,
}

fn hash<T: Serialize>(value: &T) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| Error::internal())?)
    ))
}
fn busy() -> Error {
    Error::new(
        "AUTH_REQUIRED",
        "Credential storage is busy; try again after the other login or refresh completes",
        3,
    )
}
fn lost() -> Error {
    Error::new("AUTH_REQUIRED", "Credential lock was lost; authenticate again", 3)
}
fn unreadable() -> Error {
    Error::config("CLI credential storage is unreadable or invalid")
}
fn private(path: &Path, directory: bool) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(|_| unreadable())?;
    let valid = !meta.file_type().is_symlink() && if directory { meta.is_dir() } else { meta.is_file() };
    #[cfg(unix)]
    let valid = {
        use std::os::unix::fs::MetadataExt;
        valid && meta.mode() & 0o077 == 0 && meta.uid() == unsafe { libc::getuid() }
    };
    if valid {
        Ok(())
    } else {
        Err(Error::config(
            "CLI credential storage must be private and owned by the current user",
        ))
    }
}
fn mtime(path: &Path) -> std::io::Result<i64> {
    Ok(fs::metadata(path)?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_millis() as i64)
}
fn touch(path: &Path, millis: i64) -> std::io::Result<()> {
    let time =
        filetime::FileTime::from_unix_time(millis.div_euclid(1000), (millis.rem_euclid(1000) as u32) * 1_000_000);
    filetime::set_file_times(path, time, time)
}
impl Lock {
    fn owns(&self) -> bool {
        !self.compromised && mtime(&self.path).is_ok_and(|v| v == self.mtime)
    }
    fn update(&mut self) -> bool {
        let now = now();
        match mtime(&self.path) {
            Ok(value) if value != self.mtime => {
                self.compromised = true;
                return false;
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound || now - self.last_update > 120_000 {
                    self.compromised = true;
                }
                return false;
            }
            _ => {}
        }
        let time = if self.seconds { (now + 999) / 1000 * 1000 } else { now };
        match touch(&self.path, time) {
            Ok(()) => {
                self.mtime = time;
                self.last_update = now;
                true
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound || now - self.last_update > 120_000 {
                    self.compromised = true;
                }
                false
            }
        }
    }
}
impl Store {
    pub async fn open(namespace: &str, api_url: &str) -> Result<Self> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"));
        if !base.is_absolute() {
            return Err(Error::config("XDG_CONFIG_HOME must be absolute"));
        }
        let root = base.join(namespace);
        let directory = root.join(hash(&api_url)?);
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory).map_err(|_| unreadable())?;
        private(&root, true)?;
        private(&directory, true)?;
        let path = directory.with_extension("lock");
        // Match node-retry's 100, 200, 400, 800, 1000... schedule, 20 retries.
        let mut acquired = false;
        for attempt in 0..=20 {
            match fs::create_dir(&path) {
                Ok(()) => {
                    acquired = true;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if mtime(&path).is_ok_and(|v| v < now() - 120_000) {
                        let _ = fs::remove_dir(&path);
                        if fs::create_dir(&path).is_ok() {
                            acquired = true;
                            break;
                        }
                    }
                }
                Err(_) => {}
            }
            if attempt < 20 {
                tokio::time::sleep(Duration::from_millis((100_u64 << attempt.min(4)).min(1000))).await;
            }
        }
        if !acquired {
            return Err(busy());
        }
        let probe = (now() + 999) / 1000 * 1000 + 5;
        let measured = touch(&path, probe).and_then(|_| mtime(&path));
        let time = match measured {
            Ok(time) => time,
            Err(_) => {
                let _ = fs::remove_dir(&path);
                return Err(busy());
            }
        };
        let lock = Arc::new(Mutex::new(Lock {
            path,
            mtime: time,
            seconds: time % 1000 == 0,
            last_update: now(),
            compromised: false,
        }));
        let updating = lock.clone();
        let heartbeat = tokio::spawn(async move {
            let mut delay = 10;
            loop {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                let Ok(mut lock) = updating.lock() else { break };
                delay = if lock.update() { 10 } else { 1 };
                if lock.compromised {
                    break;
                }
            }
        });
        Ok(Self {
            directory,
            api_url: api_url.into(),
            lock,
            heartbeat,
        })
    }
    fn profile(&self, binding: &Binding) -> Result<PathBuf> {
        Ok(self.directory.join(format!(
            "{}.json",
            hash(&[self.api_url.as_str(), &binding.issuer, &binding.client_id])?
        )))
    }
    fn read_json(&self, path: &Path) -> Result<Option<serde_json::Value>> {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unreadable()),
            _ => {}
        }
        private(path, false)?;
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|mut f| f.read_to_end(&mut bytes))
            .map_err(|_| unreadable())?;
        serde_json::from_slice(&bytes).map(Some).map_err(|_| unreadable())
    }
    fn binding(&self) -> Result<Option<Binding>> {
        let binding: Option<Binding> = self
            .read_json(&self.directory.join("current.json"))?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| Error::config("Invalid saved login binding"))?;
        if binding
            .as_ref()
            .is_some_and(|b| b.client_id.is_empty() || url::Url::parse(&b.issuer).is_err())
        {
            return Err(Error::config("Invalid saved login binding"));
        }
        Ok(binding)
    }
    pub fn read(&self) -> Result<Option<Stored>> {
        let Some(binding) = self.binding()? else {
            return Ok(None);
        };
        let invalid = || Error::config("Saved login does not match the configured API and identity provider");
        let value: Stored = serde_json::from_value(self.read_json(&self.profile(&binding)?)?.ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        let valid_tokens =
            |t: &Tokens| !t.access_token.is_empty() && !t.refresh_token.is_empty() && t.expires_at.is_finite();
        if value.version != 1
            || value.api_url != self.api_url
            || value.binding.issuer != binding.issuer
            || value.binding.client_id != binding.client_id
            || value.user_id.is_empty()
            || !valid_tokens(&value.tokens)
            || value.pending_refresh.as_ref().is_some_and(|t| !valid_tokens(t))
        {
            return Err(invalid());
        }
        Ok(Some(value))
    }
    fn atomic<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        // Hold the local mutex through rename, so a heartbeat cannot observe
        // this process in between its ownership check and credential commit.
        let lock = self.lock.lock().map_err(|_| lost())?;
        if !lock.owns() {
            return Err(lost());
        }
        let temporary = path.with_extension(format!("json.{:016x}.tmp", rand::random::<u64>()));
        let result = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(value).map_err(std::io::Error::other)?)?;
            fs::rename(&temporary, path)
        })();
        let _ = fs::remove_file(temporary);
        result.map_err(|_| unreadable())
    }
    pub fn save(&self, value: &Stored) -> Result<()> {
        let previous = self.binding()?;
        let path = self.profile(&value.binding)?;
        self.atomic(&path, value)?;
        self.atomic(&self.directory.join("current.json"), &value.binding)?;
        if let Some(previous) = previous {
            let old = self.profile(&previous)?;
            if old != path {
                remove(&old)?;
            }
        }
        Ok(())
    }
    pub fn read_pending_login<T: serde::de::DeserializeOwned>(&self) -> Result<Option<T>> {
        self.read_json(&self.directory.join("pending-login.json"))?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| unreadable())
    }
    pub fn save_pending_login<T: Serialize>(&self, value: &T) -> Result<()> {
        self.atomic(&self.directory.join("pending-login.json"), value)
    }
    pub fn clear_pending_login(&self) -> Result<()> {
        remove(&self.directory.join("pending-login.json"))
    }
    pub fn clear(&self) -> Result<()> {
        if let Some(binding) = self.binding()? {
            remove(&self.profile(&binding)?)?;
        }
        remove(&self.directory.join("current.json"))
    }
}
fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(unreadable()),
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        self.heartbeat.abort();
        if let Ok(lock) = self.lock.lock() {
            if lock.owns() {
                let _ = fs::remove_dir(&lock.path);
            }
        }
    }
}
