use super::{HttpState, auth};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dockervault_core::{storage_validation::storage_errors, timestamp::Timestamp};
use serde_json::{Value, json};

macro_rules! select_storage {
    ($suffix:literal) => { concat!("SELECT json_object(
    'id', id, 'name', name, 'storage_type', storage_type,
    'enabled', json(CASE WHEN enabled IS NULL THEN 'null' WHEN enabled THEN 'true' ELSE 'false' END),
    'host', host, 'port', port, 'username', username, 'base_path', base_path,
    'ssh_key_path', ssh_key_path, 's3_bucket', s3_bucket, 's3_region', s3_region,
    's3_endpoint_url', s3_endpoint_url, 'webdav_url', webdav_url, 'rclone_remote', rclone_remote,
    'created_at', created_at, 'updated_at', updated_at,
    'password', password, 's3_access_key', s3_access_key, 's3_secret_key', s3_secret_key
) FROM remote_storages", $suffix) };
}

mod create;
mod draft;
pub(super) use create::{create, update};
pub(super) use draft::test_config;

fn storage_view(row: String) -> Option<Value> {
    let mut value: Value = serde_json::from_str(&row).ok()?;
    let errors = storage_errors(&value);
    let fields = value.as_object_mut()?;
    for key in ["password", "s3_access_key", "s3_secret_key"] {
        fields.remove(key);
    }
    for key in ["created_at", "updated_at"] {
        let date = Timestamp::parse(fields.get(key)?.as_str()?).ok()?;
        fields.insert(key.into(), Value::String(date.api()));
    }
    fields.insert("validation_errors".into(), json!(errors));
    Some(value)
}

pub(super) async fn list(State(state): State<HttpState>) -> Response {
    match sqlx::query_scalar::<_, String>(select_storage!(" ORDER BY id"))
        .fetch_all(state.auth.pool())
        .await
    {
        Ok(rows) => match rows
            .into_iter()
            .map(storage_view)
            .collect::<Option<Vec<_>>>()
        {
            Some(rows) => Json(rows).into_response(),
            None => storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
        },
        Err(_) => storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
}

pub(super) async fn detail(State(state): State<HttpState>, Path(raw): Path<String>) -> Response {
    let Some(id) = super::operations::query_integer(&raw) else {
        return invalid_id();
    };
    match sqlx::query_scalar::<_, String>(select_storage!(" WHERE id=?"))
        .bind(id)
        .fetch_optional(state.auth.pool())
        .await
    {
        Ok(Some(row)) => match storage_view(row) {
            Some(row) => Json(row).into_response(),
            None => storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
        },
        Ok(None) => storage_error(StatusCode::NOT_FOUND, "Storage not found"),
        Err(_) => storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
}

fn invalid_id() -> Response {
    *auth::validation(vec![
        json!({"field":"path.storage_id","message":"Input should be a valid integer, unable to parse string as an integer"}),
    ])
}

pub(super) async fn delete(State(state): State<HttpState>, Path(raw): Path<String>) -> Response {
    let Some(id) = super::operations::query_integer(&raw) else {
        return invalid_id();
    };
    const REFERENCED: &str =
        "Remove backup sync records before deleting their storage configuration";
    let result = async {
        let mut tx = state.auth.pool().begin_with("BEGIN IMMEDIATE").await?;
        let deleted = sqlx::query("DELETE FROM remote_storages WHERE id=?1 AND NOT EXISTS (SELECT 1 FROM backup_storage_syncs WHERE storage_id=?1)")
            .bind(id).execute(&mut *tx).await?.rows_affected();
        let exists: bool = deleted == 0 && sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM remote_storages WHERE id=?)")
            .bind(id).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>((deleted, exists))
    }.await;
    match result {
        Ok((1, _)) => Json(json!({"message":"Storage deleted"})).into_response(),
        Ok((0, true)) => storage_error(StatusCode::CONFLICT, REFERENCED),
        Ok((0, false)) => storage_error(StatusCode::NOT_FOUND, "Storage not found"),
        _ => storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
}

fn storage_error(status: StatusCode, detail: &'static str) -> Response {
    (status, Json(json!({"detail":detail}))).into_response()
}

fn invalid_storage(errors: Vec<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"detail":{"message":"Invalid storage configuration","errors":errors}})),
    )
        .into_response()
}

fn probe_response(success: bool) -> Response {
    Json(json!({"success": success, "message": if success { "Connection test succeeded" } else { "Connection test failed" }})).into_response()
}

async fn probe_local(state: &HttpState, path: Option<String>) -> bool {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return false;
    };
    match state.auth.blocking().submit_blocking(move |cancel| {
        !cancel.is_cancelled() && local_writable(std::path::Path::new(&path)).is_ok()
    }) {
        Ok(work) => work.wait().await.unwrap_or(false),
        Err(_) => false,
    }
}

pub(super) async fn test_saved(
    State(state): State<HttpState>,
    Path(raw): Path<String>,
) -> Response {
    let Some(id) = super::operations::query_integer(&raw) else {
        return invalid_id();
    };
    let raw = match sqlx::query_scalar::<_, String>(select_storage!(" WHERE id=?"))
        .bind(id)
        .fetch_optional(state.auth.pool())
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return storage_error(StatusCode::NOT_FOUND, "Storage not found"),
        Err(_) => return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    };
    let Ok(mut saved) = serde_json::from_str::<serde_json::Map<String, Value>>(&raw) else {
        return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
    };
    if !open_probe_password(&state, &mut saved).await {
        return probe_response(false);
    }
    probe_provider(&state, Value::Object(saved)).await
}

async fn open_probe_password(
    state: &HttpState,
    config: &mut serde_json::Map<String, Value>,
) -> bool {
    if !matches!(config["storage_type"].as_str(), Some("webdav" | "ftp")) {
        return true;
    }
    let password = config
        .get("password")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let Ok([password, _]) = crate::storage_recovery::open_delete_credentials(
        [password, None],
        state.auth.blocking(),
        &state.config,
    )
    .await
    else {
        return false;
    };
    config.insert("password".into(), json!(password));
    true
}

async fn probe_provider(state: &HttpState, config: Value) -> Response {
    let field = |key: &str| config[key].as_str().map(str::to_owned);
    let success = match config["storage_type"].as_str() {
        Some("local") => probe_local(state, field("base_path")).await,
        Some("webdav") => probe_webdav(
            state,
            field("base_path"),
            field("webdav_url"),
            field("username"),
            field("password"),
        )
        .await
        .is_some(),
        Some("ftp") => probe_ftp(state, &config).await.is_some(),
        _ => {
            return storage_error(
                StatusCode::NOT_IMPLEMENTED,
                "Remote storage operations are unavailable",
            );
        }
    };
    probe_response(success)
}

async fn probe_ftp(state: &HttpState, config: &Value) -> Option<()> {
    use crate::storage::ftp::{Ftp, FtpConfig};
    let field = |key: &str| config[key].as_str().unwrap_or_default().to_owned();
    let port = if config["port"].is_null() {
        None
    } else {
        Some(u16::try_from(config["port"].as_i64()?).ok()?)
    };
    let provider = Ftp::new(
        FtpConfig {
            host: field("host"),
            port,
            username: field("username"),
            password: field("password"),
            base_path: field("base_path"),
        },
        state.auth.blocking().clone(),
    )
    .ok()?;
    provider.test_connection().ok()?.wait().await.ok()?.ok()
}

async fn probe_webdav(
    state: &HttpState,
    base: Option<String>,
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
) -> Option<()> {
    use crate::storage::webdav::{DavConfig, WebDav};
    let io = state.auth.blocking();
    let provider = WebDav::new(
        DavConfig {
            url: url?,
            base_path: base.unwrap_or_default(),
            username,
            password,
            max_attempts: 1,
            verify_head: false,
        },
        io.clone(),
        io.clone(),
    )
    .ok()?;
    provider.test_connection().ok()?.wait().await.ok()?.ok()
}

fn local_writable(path: &std::path::Path) -> std::io::Result<()> {
    use nix::fcntl::{OFlag, openat};
    use std::io::Write;
    let directory = crate::storage::local::open_or_create_root(path)?;
    let probe = openat(
        &directory,
        ".",
        OFlag::O_WRONLY | OFlag::O_TMPFILE | OFlag::O_CLOEXEC,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )?;
    let mut file = std::fs::File::from(probe);
    file.write_all(b"test")?;
    file.sync_all()
}

pub(super) async fn types() -> Json<Value> {
    Json(json!({
        "local": {
            "name": "Local/Network Path",
            "description": "Local filesystem or mounted network storage (NFS, SMB)",
            "required": ["base_path"],
            "optional": [],
        },
        "ssh": {
            "name": "SSH/SFTP",
            "description": "Remote server via SSH with rsync",
            "required": ["host", "username", "base_path"],
            "optional": ["port", "ssh_key_path", "password"],
        },
        "webdav": {
            "name": "WebDAV",
            "description": "WebDAV compatible storage (Nextcloud, ownCloud, etc.)",
            "required": ["webdav_url", "base_path"],
            "optional": ["username", "password"],
        },
        "s3": {
            "name": "S3 Compatible",
            "description": "AWS S3, MinIO, Backblaze B2, Wasabi, etc.",
            "required": ["s3_bucket", "s3_access_key", "s3_secret_key"],
            "optional": ["s3_region", "s3_endpoint_url", "base_path"],
        },
        "ftp": {
            "name": "FTP (unencrypted)",
            "description": "Plain FTP server; use SSH/SFTP for secure transport. FTPS is not supported.",
            "required": ["host", "username", "password", "base_path"],
            "optional": ["port"],
        },
        "rclone": {
            "name": "Rclone",
            "description": "Any rclone-supported backend (40+ providers)",
            "required": ["rclone_remote", "base_path"],
            "optional": [],
        },
    }))
}
