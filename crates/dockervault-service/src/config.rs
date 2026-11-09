//! Published environment configuration. Values are never included in diagnostics.
use std::{collections::BTreeMap, fmt, path::Path, time::Duration};

#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid configuration for {}", self.0)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub docker_socket: String,
    pub backup_base_path: String,
    pub default_keep_last: i64,
    pub default_keep_daily: i64,
    pub default_keep_weekly: i64,
    pub default_keep_monthly: i64,
    pub default_keep_yearly: i64,
    pub cors_origins: String,
    pub cookie_secure: bool,
    pub credential_encryption_key: Secret,
    pub allowed_hook_commands: String,
    pub hook_command_timeout: i64,
    pub allowed_ssh_key_dirs: String,
    pub enable_docs: bool,
    pub log_level: String,
    pub log_format: String,
    pub shutdown_timeout: i64,
    pub tar_worker_timeout: i64,
    pub tar_worker_idle_timeout: i64,
    pub storage_max_retries: i64,
    pub storage_verify_after_upload: bool,
    pub storage_upload_chunk_size: i64,
    pub s3_multipart_threshold_bytes: i64,
    pub s3_multipart_chunk_size: i64,
    pub komodo_api_url: String,
    pub komodo_api_key: Secret,
    pub komodo_api_secret: Secret,
    pub komodo_enabled: bool,
    pub komodo_allowed_hosts: String,
    pub tz: String,
}

// URLs and filesystem paths can contain credentials or private infrastructure too.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Config { values: [REDACTED] }")
    }
}

const NAMES: &[&str] = &[
    "DATABASE_URL",
    "DOCKER_SOCKET",
    "BACKUP_BASE_PATH",
    "DEFAULT_KEEP_LAST",
    "DEFAULT_KEEP_DAILY",
    "DEFAULT_KEEP_WEEKLY",
    "DEFAULT_KEEP_MONTHLY",
    "DEFAULT_KEEP_YEARLY",
    "CORS_ORIGINS",
    "COOKIE_SECURE",
    "CREDENTIAL_ENCRYPTION_KEY",
    "ALLOWED_HOOK_COMMANDS",
    "HOOK_COMMAND_TIMEOUT",
    "ALLOWED_SSH_KEY_DIRS",
    "ENABLE_DOCS",
    "LOG_LEVEL",
    "LOG_FORMAT",
    "SHUTDOWN_TIMEOUT",
    "TAR_WORKER_TIMEOUT",
    "TAR_WORKER_IDLE_TIMEOUT",
    "STORAGE_MAX_RETRIES",
    "STORAGE_VERIFY_AFTER_UPLOAD",
    "STORAGE_UPLOAD_CHUNK_SIZE",
    "S3_MULTIPART_THRESHOLD_BYTES",
    "S3_MULTIPART_CHUNK_SIZE",
    "KOMODO_API_URL",
    "KOMODO_API_KEY",
    "KOMODO_API_SECRET",
    "KOMODO_ENABLED",
    "KOMODO_ALLOWED_HOSTS",
    "TZ",
];

type Values = BTreeMap<String, String>;

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let mut values = Vec::new();
        for (name, value) in std::env::vars_os() {
            let Some(name) = name.to_str().filter(|name| NAMES.contains(name)) else {
                continue;
            };
            let value = value.into_string().map_err(|_| ConfigError(name.into()))?;
            values.push((name.to_owned(), value));
        }
        Self::from_sources(Path::new(".env"), values)
    }

    pub fn from_sources<K: Into<String>, V: Into<String>>(
        env_file: &Path,
        environment: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, ConfigError> {
        let mut values = Values::new();
        match dotenvy::from_path_iter(env_file) {
            Ok(items) => {
                for item in items {
                    let (name, value) = item.map_err(|_| ConfigError("env_file".into()))?;
                    if !NAMES.contains(&name.as_str()) {
                        return Err(ConfigError("env_file".into()));
                    }
                    values.insert(name, value);
                }
            }
            Err(dotenvy::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ConfigError("env_file".into())),
        }
        values.extend(environment.into_iter().map(|(k, v)| (k.into(), v.into())));
        Self::from_pairs(values)
    }

    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, ConfigError> {
        let values: Values = pairs
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        Ok(Self {
            database_url: text(
                &values,
                "DATABASE_URL",
                "sqlite+aiosqlite:///./data/backups.db",
            ),
            docker_socket: text(&values, "DOCKER_SOCKET", "/var/run/docker.sock"),
            backup_base_path: text(&values, "BACKUP_BASE_PATH", "/backups"),
            default_keep_last: number(&values, "DEFAULT_KEEP_LAST", 3)?,
            default_keep_daily: number(&values, "DEFAULT_KEEP_DAILY", 7)?,
            default_keep_weekly: number(&values, "DEFAULT_KEEP_WEEKLY", 4)?,
            default_keep_monthly: number(&values, "DEFAULT_KEEP_MONTHLY", 6)?,
            default_keep_yearly: number(&values, "DEFAULT_KEEP_YEARLY", 2)?,
            cors_origins: text(&values, "CORS_ORIGINS", "http://localhost"),
            cookie_secure: boolean(&values, "COOKIE_SECURE", true)?,
            credential_encryption_key: Secret(text(&values, "CREDENTIAL_ENCRYPTION_KEY", "")),
            allowed_hook_commands: text(
                &values,
                "ALLOWED_HOOK_COMMANDS",
                "pg_dump,pg_dumpall,mysqldump,mongodump,redis-cli,mariadb-dump",
            ),
            hook_command_timeout: number(&values, "HOOK_COMMAND_TIMEOUT", 300)?,
            allowed_ssh_key_dirs: text(
                &values,
                "ALLOWED_SSH_KEY_DIRS",
                "/app/data/ssh_keys,/root/.ssh",
            ),
            enable_docs: boolean(&values, "ENABLE_DOCS", false)?,
            log_level: text(&values, "LOG_LEVEL", "INFO"),
            log_format: text(&values, "LOG_FORMAT", "text"),
            shutdown_timeout: number(&values, "SHUTDOWN_TIMEOUT", 0)?,
            tar_worker_timeout: number(&values, "TAR_WORKER_TIMEOUT", 0)?,
            tar_worker_idle_timeout: number(&values, "TAR_WORKER_IDLE_TIMEOUT", 1800)?,
            storage_max_retries: number(&values, "STORAGE_MAX_RETRIES", 6)?,
            storage_verify_after_upload: boolean(&values, "STORAGE_VERIFY_AFTER_UPLOAD", true)?,
            storage_upload_chunk_size: number(&values, "STORAGE_UPLOAD_CHUNK_SIZE", 1024 * 1024)?,
            s3_multipart_threshold_bytes: number(
                &values,
                "S3_MULTIPART_THRESHOLD_BYTES",
                64 * 1024 * 1024,
            )?,
            s3_multipart_chunk_size: number(&values, "S3_MULTIPART_CHUNK_SIZE", 16 * 1024 * 1024)?,
            komodo_api_url: text(&values, "KOMODO_API_URL", ""),
            komodo_api_key: Secret(text(&values, "KOMODO_API_KEY", "")),
            komodo_api_secret: Secret(text(&values, "KOMODO_API_SECRET", "")),
            komodo_enabled: boolean(&values, "KOMODO_ENABLED", false)?,
            komodo_allowed_hosts: text(&values, "KOMODO_ALLOWED_HOSTS", ""),
            tz: text(&values, "TZ", "Europe/Berlin"),
        })
    }

    pub fn shutdown_limit(&self) -> Option<Duration> {
        limit(self.shutdown_timeout)
    }

    pub fn tar_worker_limit(&self) -> Option<Duration> {
        limit(self.tar_worker_timeout)
    }

    pub fn tar_worker_idle_limit(&self) -> Option<Duration> {
        limit(self.tar_worker_idle_timeout)
    }
}

fn limit(seconds: i64) -> Option<Duration> {
    (seconds != 0).then(|| Duration::from_secs(seconds.max(0) as u64))
}

fn text(values: &Values, name: &str, fallback: &str) -> String {
    values
        .get(name)
        .cloned()
        .unwrap_or_else(|| fallback.to_owned())
}

fn boolean(values: &Values, name: &str, fallback: bool) -> Result<bool, ConfigError> {
    match values.get(name).map(|v| v.to_ascii_lowercase()) {
        None => Ok(fallback),
        Some(value) => match value.as_str() {
            "true" | "1" | "on" | "yes" | "t" | "y" => Ok(true),
            "false" | "0" | "off" | "no" | "f" | "n" => Ok(false),
            _ => Err(ConfigError(name.into())),
        },
    }
}

fn number(values: &Values, name: &str, fallback: i64) -> Result<i64, ConfigError> {
    let Some(raw) = values.get(name) else {
        return Ok(fallback);
    };
    let raw = raw.trim();
    let digits = if let Some((whole, fraction)) = raw.split_once('.') {
        if whole.is_empty() || fraction.is_empty() || !fraction.bytes().all(|b| b == b'0') {
            return Err(ConfigError(name.into()));
        }
        whole
    } else {
        raw
    };
    let bytes = digits.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'_'
            && (index == 0
                || index + 1 == bytes.len()
                || !bytes[index - 1].is_ascii_digit()
                || !bytes[index + 1].is_ascii_digit())
        {
            return Err(ConfigError(name.into()));
        }
    }
    digits
        .replace('_', "")
        .parse()
        .map_err(|_| ConfigError(name.into()))
}
