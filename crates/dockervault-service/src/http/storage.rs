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
pub(super) mod download;
mod draft;
mod rclone;
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

fn invalid_id_field() -> Value {
    json!({"field":"path.storage_id","message":"Input should be a valid integer, unable to parse string as an integer"})
}

fn invalid_id() -> Response {
    *auth::validation(vec![invalid_id_field()])
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
    if !open_probe_credentials(&state, &mut saved, None).await {
        return probe_response(false);
    }
    probe_provider(&state, Value::Object(saved), false).await
}

pub(super) async fn open_probe_credentials(
    state: &HttpState,
    config: &mut serde_json::Map<String, Value>,
    overrides: Option<&serde_json::Map<String, Value>>,
) -> bool {
    let fields = match config["storage_type"].as_str() {
        Some("webdav" | "ftp" | "ssh" | "sftp") => [Some("password"), None],
        Some("s3") => [Some("s3_access_key"), Some("s3_secret_key")],
        _ => return true,
    }
    .map(|field| field.filter(|field| !overrides.is_some_and(|data| data.contains_key(*field))));
    let stored = fields.map(|field| {
        field
            .and_then(|field| config[field].as_str())
            .map(str::to_owned)
    });
    let Ok(values) = crate::storage_recovery::open_delete_credentials(
        stored,
        state.auth.blocking(),
        &state.config,
    )
    .await
    else {
        return false;
    };
    for (key, value) in fields.into_iter().zip(values) {
        if let Some(key) = key {
            config.insert(key.into(), json!(value));
        }
    }
    true
}

async fn probe_provider(state: &HttpState, config: Value, draft: bool) -> Response {
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
        Some("s3") => probe_s3(state, &config).await.is_some(),
        Some("ssh" | "sftp") => return probe_ssh(state, &config, draft).await,
        Some("rclone") => rclone::probe(state, &config).await.is_some(),
        _ => {
            return storage_error(
                StatusCode::NOT_IMPLEMENTED,
                "Remote storage operations are unavailable",
            );
        }
    };
    probe_response(success)
}

async fn probe_ssh(state: &HttpState, config: &Value, draft: bool) -> Response {
    let (endpoint, held) = match ssh_endpoint(state, config, draft, String::new()).await {
        Ok(parts) => parts,
        Err(response) => return *response,
    };
    let Some(processes) = state.probe_processes.as_ref() else {
        return probe_response(false);
    };
    let Ok(work) = endpoint.probe_handshake(processes, held) else {
        return probe_response(false);
    };
    probe_response(matches!(work.wait().await, Ok(Ok(output)) if output.status.success()))
}

async fn ssh_endpoint(
    state: &HttpState,
    config: &Value,
    draft: bool,
    base: String,
) -> Result<(crate::storage::ssh::SftpEndpoint, Option<std::fs::File>), Box<Response>> {
    use crate::storage::ssh::{SftpEndpoint, key_file};
    use std::{os::fd::AsRawFd, path::PathBuf};
    let field = |key: &str| config[key].as_str().map(str::to_owned);
    let held = if let Some(path) = field("ssh_key_path").filter(|path| !path.is_empty()) {
        let dirs: Vec<String> = state
            .config
            .allowed_ssh_key_dirs
            .split(',')
            .map(str::trim)
            .filter(|root| !root.is_empty())
            .map(str::to_owned)
            .collect();
        let allowed: Vec<PathBuf> = dirs.iter().map(PathBuf::from).collect();
        let Ok(work) = state.auth.blocking().submit_blocking(move |cancel| {
            if cancel.is_cancelled() {
                return None;
            }
            key_file(std::path::Path::new(&path), &allowed)
        }) else {
            return Err(Box::new(probe_response(false)));
        };
        match work.wait().await {
            Ok(Some(file)) => Some(file),
            Ok(None) if draft => return Err(Box::new((StatusCode::BAD_REQUEST, Json(json!({"detail":format!("SSH key path must be within allowed directories: {dirs:?}")}))).into_response())),
            _ => return Err(Box::new(probe_response(false))),
        }
    } else {
        None
    };
    let port = config["port"].as_i64().unwrap_or(22);
    let (Some(host), Some(username), Ok(port)) =
        (field("host"), field("username"), u16::try_from(port))
    else {
        return Err(Box::new(probe_response(false)));
    };
    let endpoint = SftpEndpoint {
        host,
        username,
        port,
        base,
        known_hosts: "/app/data/ssh_known_hosts".into(),
        identity: held
            .as_ref()
            .map(|file| format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd()).into()),
        password: field("password"),
    };
    Ok((endpoint, held))
}

async fn probe_s3(state: &HttpState, config: &Value) -> Option<()> {
    s3_provider(state, config)?
        .test_connection()
        .ok()?
        .wait()
        .await
        .ok()?
        .ok()
}

pub(super) fn s3_provider(state: &HttpState, config: &Value) -> Option<crate::storage::s3::S3> {
    use crate::storage::s3::{S3, S3Config};
    let field = |key: &str| config[key].as_str().map(str::to_owned);
    let io = state.auth.blocking();
    S3::new(
        S3Config {
            endpoint_url: field("s3_endpoint_url").filter(|url| !url.is_empty()),
            region: field("s3_region").unwrap_or_default(),
            bucket: field("s3_bucket")?,
            base_path: field("base_path").unwrap_or_default(),
            access_key: field("s3_access_key")?,
            secret_key: field("s3_secret_key")?,
        },
        state.probe_jobs.clone(),
        io.clone(),
    )
    .ok()
}

async fn probe_ftp(state: &HttpState, config: &Value) -> Option<()> {
    let provider = ftp_provider(state, config)?;
    provider.test_connection().ok()?.wait().await.ok()?.ok()
}

fn ftp_provider(state: &HttpState, config: &Value) -> Option<crate::storage::ftp::Ftp> {
    use crate::storage::ftp::{Ftp, FtpConfig};
    let field = |key: &str| config[key].as_str().unwrap_or_default().to_owned();
    let port = if config["port"].is_null() {
        None
    } else {
        Some(u16::try_from(config["port"].as_i64()?).ok()?)
    };
    Ftp::new(
        FtpConfig {
            host: field("host"),
            port,
            username: field("username"),
            password: field("password"),
            base_path: field("base_path"),
        },
        state.auth.blocking().clone(),
    )
    .ok()
}

async fn probe_webdav(
    state: &HttpState,
    base: Option<String>,
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
) -> Option<()> {
    let provider = webdav_provider(state, base, url, username, password)?;
    provider.test_connection().ok()?.wait().await.ok()?.ok()
}

fn webdav_provider(
    state: &HttpState,
    base: Option<String>,
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
) -> Option<crate::storage::webdav::WebDav> {
    use crate::storage::webdav::{DavConfig, WebDav};
    WebDav::new(
        DavConfig {
            url: url?,
            base_path: base.unwrap_or_default(),
            username,
            password,
            max_attempts: 1,
            verify_head: false,
        },
        state.probe_jobs.clone(),
        state.auth.blocking().clone(),
    )
    .ok()
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

type BrowseQuery = axum::extract::Query<std::collections::HashMap<String, String>>;

async fn configured_browse_root(
    state: &HttpState,
    id: i64,
) -> Result<Option<String>, Box<Response>> {
    match sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT storage_type,base_path FROM remote_storages WHERE id=?",
    )
    .bind(id)
    .fetch_optional(state.auth.pool())
    .await
    {
        Ok(Some((kind, path))) if kind == "local" => Ok(Some(path.unwrap_or_default())),
        Ok(Some(_)) => Ok(None),
        Ok(None) => Err(Box::new(storage_error(
            StatusCode::NOT_FOUND,
            "Storage not found",
        ))),
        Err(_) => Err(Box::new(storage_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error",
        ))),
    }
}

pub(super) async fn configured_size(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    axum::extract::Query(query): BrowseQuery,
) -> Response {
    let (id, path) = match configured_path(&id, query.get("path").map(String::as_str)) {
        Ok(inputs) => inputs,
        Err(response) => return *response,
    };
    match configured_browse_root(&state, id).await {
        Ok(Some(root)) => size_at(&state, root, path).await,
        Ok(None) => unavailable_size(path),
        Err(response) => *response,
    }
}

fn configured_path<'a>(id: &str, path: Option<&'a str>) -> Result<(i64, &'a str), Box<Response>> {
    let id = super::operations::query_integer(id);
    match (id, path) {
        (Some(id), Some(path)) => Ok((id, path)),
        _ => Err(auth::validation(
            [
                id.is_none().then(invalid_id_field),
                path.is_none().then(missing_size_path_field),
            ]
            .into_iter()
            .flatten()
            .collect(),
        )),
    }
}

pub(super) async fn configured_download(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    axum::extract::Query(query): BrowseQuery,
    headers: axum::http::HeaderMap,
) -> Response {
    let (id, path) = match configured_path(&id, query.get("path").map(String::as_str)) {
        Ok(inputs) => inputs,
        Err(response) => return *response,
    };
    let mut config = match configured_browse_config(&state, id).await {
        Ok(config) => config,
        Err(response) => return *response,
    };
    match config["storage_type"].as_str() {
        Some("local") => {
            download_at(
                &state,
                config["base_path"].as_str().unwrap_or_default().to_owned(),
                path,
                &headers,
            )
            .await
        }
        Some("rclone") => {
            download_from(
                &state,
                DownloadSource::Rclone(Value::Object(config)),
                path,
                &headers,
            )
            .await
        }
        Some("ssh" | "sftp") => {
            if crate::storage::relative_path(path, false).is_none() {
                return storage_error(StatusCode::BAD_REQUEST, "Invalid file path");
            }
            if !open_probe_credentials(&state, &mut config, None).await {
                return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Download failed");
            }
            let base = config["base_path"].as_str().unwrap_or_default().to_owned();
            let Ok((endpoint, key)) =
                ssh_endpoint(&state, &Value::Object(config), false, base).await
            else {
                return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Download failed");
            };
            download_from(
                &state,
                DownloadSource::Sftp(endpoint, key.map(std::sync::Arc::new)),
                path,
                &headers,
            )
            .await
        }
        Some("webdav" | "s3" | "ftp") => {
            if !open_probe_credentials(&state, &mut config, None).await {
                return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Download failed");
            }
            let source = match config["storage_type"].as_str() {
                Some("s3") => s3_provider(&state, &Value::Object(config)).map(DownloadSource::S3),
                Some("ftp") => {
                    ftp_provider(&state, &Value::Object(config)).map(DownloadSource::Ftp)
                }
                _ => configured_webdav(&state, &config).map(DownloadSource::Webdav),
            };
            let Some(source) = source else {
                return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Download failed");
            };
            download_from(&state, source, path, &headers).await
        }
        _ => storage_error(
            StatusCode::NOT_IMPLEMENTED,
            "Remote storage operations are unavailable",
        ),
    }
}

pub(super) async fn list_configured_files(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    axum::extract::Query(query): BrowseQuery,
) -> Response {
    let Some(id) = super::operations::query_integer(&id) else {
        return invalid_id();
    };
    let mut config = match configured_browse_config(&state, id).await {
        Ok(config) => config,
        Err(response) => return *response,
    };
    let raw = query.get("path").map_or("", String::as_str);
    match config["storage_type"].as_str() {
        Some("local") => {
            list_at(
                &state,
                config["base_path"].as_str().unwrap_or_default().to_owned(),
                raw,
            )
            .await
        }
        Some("webdav" | "s3") => {
            let is_s3 = config["storage_type"] == "s3";
            let Some(path) = crate::storage::relative_path(raw, true) else {
                return storage_error(StatusCode::BAD_REQUEST, "Invalid file path");
            };
            let files = if is_s3 {
                async {
                    if !open_probe_credentials(&state, &mut config, None).await {
                        return None;
                    }
                    let provider = s3_provider(&state, &Value::Object(config))?;
                    provider
                        .list_files(path.clone())
                        .ok()?
                        .wait()
                        .await
                        .ok()?
                        .ok()
                }
                .await
            } else {
                list_webdav(&state, &mut config, path.clone()).await
            };
            match files {
                Some(files) => Json(json!({"files":files,"path":path})).into_response(),
                None => storage_error(StatusCode::BAD_GATEWAY, "Remote listing failed"),
            }
        }
        Some("ssh" | "sftp") => {
            let Some(path) = crate::storage::relative_path(raw, true) else {
                return storage_error(StatusCode::BAD_REQUEST, "Invalid file path");
            };
            let files = async {
                let processes = state.probe_processes.as_ref()?;
                if !open_probe_credentials(&state, &mut config, None).await {
                    return None;
                }
                let base = config["base_path"].as_str().unwrap_or_default().to_owned();
                let (endpoint, held) = ssh_endpoint(&state, &Value::Object(config), false, base)
                    .await
                    .ok()?;
                endpoint
                    .list_holding(processes, &state.probe_jobs, &path, held)
                    .ok()?
                    .wait()
                    .await
                    .ok()?
                    .ok()
            }
            .await;
            match files {
                Some(files) => Json(json!({"files":files,"path":path})).into_response(),
                None => storage_error(StatusCode::BAD_GATEWAY, "Remote listing failed"),
            }
        }
        Some("ftp") => {
            let Some(path) = crate::storage::relative_path(raw, true) else {
                return storage_error(StatusCode::BAD_REQUEST, "Invalid file path");
            };
            let files = async {
                if !open_probe_credentials(&state, &mut config, None).await {
                    return None;
                }
                let provider = ftp_provider(&state, &Value::Object(config))?;
                provider.list(path.clone()).ok()?.wait().await.ok()?.ok()
            }
            .await;
            match files {
                Some(files) => Json(json!({"files":files,"path":path})).into_response(),
                None => storage_error(StatusCode::BAD_GATEWAY, "Remote listing failed"),
            }
        }
        Some("rclone") => {
            let Some(path) = crate::storage::relative_path(raw, true) else {
                return storage_error(StatusCode::BAD_REQUEST, "Invalid file path");
            };
            match rclone::list(&state, &Value::Object(config), &path).await {
                Some(files) => Json(json!({"files":files,"path":path})).into_response(),
                None => storage_error(StatusCode::BAD_GATEWAY, "Remote listing failed"),
            }
        }
        _ => storage_error(
            StatusCode::NOT_IMPLEMENTED,
            "Remote storage operations are unavailable",
        ),
    }
}

pub(super) async fn configured_browse_config(
    state: &HttpState,
    id: i64,
) -> Result<serde_json::Map<String, Value>, Box<Response>> {
    let internal = || {
        Box::new(storage_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error",
        ))
    };
    let row = sqlx::query_scalar::<_, String>(select_storage!(" WHERE id=?"))
        .bind(id)
        .fetch_optional(state.auth.pool())
        .await
        .map_err(|_| internal())?;
    let row =
        row.ok_or_else(|| Box::new(storage_error(StatusCode::NOT_FOUND, "Storage not found")))?;
    serde_json::from_str(&row).map_err(|_| internal())
}

async fn list_webdav(
    state: &HttpState,
    config: &mut serde_json::Map<String, Value>,
    path: String,
) -> Option<Vec<crate::storage::webdav::StorageEntry>> {
    if !open_probe_credentials(state, config, None).await {
        return None;
    }
    configured_webdav(state, config)?
        .list(path)
        .ok()?
        .wait()
        .await
        .ok()?
        .ok()
}

pub(super) fn configured_webdav(
    state: &HttpState,
    config: &serde_json::Map<String, Value>,
) -> Option<crate::storage::webdav::WebDav> {
    let field = |key: &str| config[key].as_str().map(str::to_owned);
    webdav_provider(
        state,
        field("base_path"),
        field("webdav_url"),
        field("username"),
        field("password"),
    )
}

fn unavailable_size(path: &str) -> Response {
    Json(json!({"path":path,"state":"unavailable","bytes":null})).into_response()
}

fn missing_size_path_field() -> Value {
    json!({"field":"query.path","message":"Field required"})
}

fn missing_size_path() -> Response {
    *auth::validation(vec![missing_size_path_field()])
}

pub(super) async fn local_download(
    State(state): State<HttpState>,
    axum::extract::Query(query): BrowseQuery,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(path) = query.get("path") else {
        return missing_size_path();
    };
    download_at(
        &state,
        state.config.backup_base_path.clone(),
        path,
        &headers,
    )
    .await
}

async fn download_at(
    state: &HttpState,
    root: String,
    path: &str,
    headers: &axum::http::HeaderMap,
) -> Response {
    if root.is_empty() {
        return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Download failed");
    }
    download_from(state, DownloadSource::Local(root.into()), path, headers).await
}

#[derive(Clone)]
pub(super) enum DownloadSource {
    Local(std::path::PathBuf),
    Webdav(crate::storage::webdav::WebDav),
    S3(crate::storage::s3::S3),
    Ftp(crate::storage::ftp::Ftp),
    Sftp(
        crate::storage::ssh::SftpEndpoint,
        Option<std::sync::Arc<std::fs::File>>,
    ),
    Rclone(Value),
}

pub(super) async fn completed_archive_copies(
    state: &HttpState,
    id: i64,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.storage_id,s.remote_path FROM backup_storage_syncs s JOIN remote_storages r ON s.storage_id=r.id
         WHERE s.backup_id=? AND s.sync_status='completed' AND r.enabled=1 AND s.remote_path IS NOT NULL AND s.remote_path<>''
         ORDER BY s.synced_at DESC,s.id ASC",
    ).bind(id).fetch_all(state.auth.pool()).await
}

pub(super) async fn saved_archive_source(state: &HttpState, id: i64) -> Option<DownloadSource> {
    let mut config = configured_browse_config(state, id).await.ok()?;
    if config.get("enabled").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    if !open_probe_credentials(state, &mut config, None).await {
        return None;
    }
    match config["storage_type"].as_str()? {
        "local" => Some(DownloadSource::Local(
            config["base_path"]
                .as_str()
                .filter(|path| !path.is_empty())?
                .into(),
        )),
        "rclone" => Some(DownloadSource::Rclone(Value::Object(config))),
        "ssh" | "sftp" => {
            let base = config["base_path"].as_str().unwrap_or_default().to_owned();
            let (endpoint, key) = ssh_endpoint(state, &Value::Object(config), false, base)
                .await
                .ok()?;
            Some(DownloadSource::Sftp(endpoint, key.map(std::sync::Arc::new)))
        }
        "s3" => s3_provider(state, &Value::Object(config)).map(DownloadSource::S3),
        "ftp" => ftp_provider(state, &Value::Object(config)).map(DownloadSource::Ftp),
        "webdav" => configured_webdav(state, &config).map(DownloadSource::Webdav),
        _ => None,
    }
}

async fn download_from(
    state: &HttpState,
    source: DownloadSource,
    path: &str,
    headers: &axum::http::HeaderMap,
) -> Response {
    let Some(path) = crate::storage::relative_path(path, false) else {
        return storage_error(StatusCode::BAD_REQUEST, "Invalid file path");
    };
    let disposition = attachment(path.rsplit('/').next().unwrap_or_default());
    let Ok(slot) = state.download_slots.clone().try_acquire_owned() else {
        return storage_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Download capacity exhausted",
        );
    };
    let result = snapshot_for_download(state, source, path, slot).await;
    let Some((file, length, slot, validators)) = result else {
        return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Download failed");
    };
    download::response(
        file,
        length,
        slot,
        disposition,
        validators,
        headers,
        "application/octet-stream",
    )
}

pub(super) fn attachment(name: &str) -> String {
    const FILENAME_ENCODE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    let encoded = percent_encoding::utf8_percent_encode(name, FILENAME_ENCODE).to_string();
    if encoded == name {
        format!("attachment; filename=\"{name}\"")
    } else {
        format!("attachment; filename*=utf-8''{encoded}")
    }
}

pub(super) async fn snapshot_for_download<T: Send + 'static>(
    state: &HttpState,
    source: DownloadSource,
    path: String,
    slot: T,
) -> Option<(std::fs::File, u64, T, download::Validators)> {
    let snapshot = match source {
        DownloadSource::Local(root) => state
            .auth
            .blocking()
            .submit_blocking(move |cancel| {
                let (file, length) = crate::storage::local::snapshot_download(
                    &root,
                    std::path::Path::new(&path),
                    &|| cancel.is_cancelled(),
                )?;
                Ok::<_, std::io::Error>((file, length, slot))
            })
            .ok()?
            .wait()
            .await
            .ok()?
            .ok()?,
        DownloadSource::Rclone(config) => {
            rclone::download_snapshot(state, config, path, slot).await?
        }
        DownloadSource::Sftp(endpoint, key) => endpoint
            .download_snapshot(
                state.probe_processes.as_ref()?,
                &state.probe_jobs,
                &path,
                key.map(|key| key.try_clone()).transpose().ok()?,
                slot,
            )
            .ok()?
            .wait()
            .await
            .ok()?
            .ok()?,
        DownloadSource::Ftp(provider) => provider
            .download_snapshot(path, state.auth.blocking(), slot)
            .ok()?
            .wait()
            .await
            .ok()?
            .ok()?,
        DownloadSource::S3(provider) => provider
            .download_snapshot(path, slot)
            .ok()?
            .wait()
            .await
            .ok()?
            .ok()?,
        DownloadSource::Webdav(provider) => provider
            .download_snapshot(path, slot)
            .ok()?
            .wait()
            .await
            .ok()?
            .ok()?,
    };
    state
        .auth
        .blocking()
        .submit_blocking(move |_| {
            let (file, length, slot) = snapshot;
            let validators = download::Validators::read(&file, length)?;
            Ok::<_, std::io::Error>((file, length, slot, validators))
        })
        .ok()?
        .wait()
        .await
        .ok()?
        .ok()
}

pub(super) async fn bulk_local_download(
    State(state): State<HttpState>,
    request: axum::extract::Request,
) -> Response {
    bulk_download(&state, None, request).await
}

pub(super) async fn bulk_configured_download(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: axum::extract::Request,
) -> Response {
    bulk_download(&state, Some(id), request).await
}

async fn bulk_download(
    state: &HttpState,
    id: Option<String>,
    mut request: axum::extract::Request,
) -> Response {
    use std::io::Seek;
    use std::path::Path;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::compat::FuturesAsyncWriteCompatExt;

    let body = match auth::json_body(state, &mut request).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let parsed_id = id.as_deref().and_then(super::operations::query_integer);
    let mut errors = Vec::new();
    if id.is_some() && parsed_id.is_none() {
        errors.push(invalid_id_field());
    }
    let Some(paths) = body["paths"].as_array() else {
        errors.push(json!({"field":"body.paths","message":if body.get("paths").is_none() {"Field required"} else {"Input should be a valid list"}}));
        return *auth::validation(errors);
    };
    let mut selection = Vec::with_capacity(paths.len());
    for (index, value) in paths.iter().enumerate() {
        match value.as_str() {
            Some(path) => selection.push(path),
            None => errors.push(json!({"field":format!("body.paths.{index}"),"message":"Input should be a valid string"})),
        }
    }
    if !errors.is_empty() {
        return *auth::validation(errors);
    }
    if paths.is_empty() {
        return storage_error(StatusCode::BAD_REQUEST, "No files specified");
    }
    let source = if let Some(id) = parsed_id {
        let mut config = match configured_browse_config(state, id).await {
            Ok(config) => config,
            Err(response) => return *response,
        };
        match config["storage_type"].as_str() {
            Some("local") => {
                DownloadSource::Local(config["base_path"].as_str().unwrap_or_default().into())
            }
            Some("webdav") => {
                if !open_probe_credentials(state, &mut config, None).await {
                    return storage_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Bulk download failed",
                    );
                }
                let Some(provider) = configured_webdav(state, &config) else {
                    return storage_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Bulk download failed",
                    );
                };
                DownloadSource::Webdav(provider)
            }
            Some("s3") => {
                if !open_probe_credentials(state, &mut config, None).await {
                    return storage_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Bulk download failed",
                    );
                }
                let Some(provider) = s3_provider(state, &Value::Object(config)) else {
                    return storage_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Bulk download failed",
                    );
                };
                DownloadSource::S3(provider)
            }
            Some("ftp") => {
                if !open_probe_credentials(state, &mut config, None).await {
                    return storage_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Bulk download failed",
                    );
                }
                let Some(provider) = ftp_provider(state, &Value::Object(config)) else {
                    return storage_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Bulk download failed",
                    );
                };
                DownloadSource::Ftp(provider)
            }
            Some("ssh" | "sftp") => {
                if !open_probe_credentials(state, &mut config, None).await {
                    return storage_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Bulk download failed",
                    );
                }
                let base = config["base_path"].as_str().unwrap_or_default().to_owned();
                let Ok((endpoint, key)) =
                    ssh_endpoint(state, &Value::Object(config), false, base).await
                else {
                    return storage_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Bulk download failed",
                    );
                };
                DownloadSource::Sftp(endpoint, key.map(std::sync::Arc::new))
            }
            _ => {
                return storage_error(
                    StatusCode::NOT_IMPLEMENTED,
                    "Remote storage operations are unavailable",
                );
            }
        }
    } else {
        DownloadSource::Local(state.config.backup_base_path.clone().into())
    };
    if matches!(&source, DownloadSource::Local(root) if root.as_os_str().is_empty()) {
        return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Bulk download failed");
    }
    let folder = selection[0]
        .split_once('/')
        .map_or("download", |(folder, _)| folder);
    let disposition = attachment(&format!("{folder}.zip"));
    let Some(paths) = selection
        .into_iter()
        .map(|path| crate::storage::relative_path(path, false))
        .collect::<Option<Vec<_>>>()
    else {
        return storage_error(StatusCode::BAD_REQUEST, "Invalid file path");
    };
    let Ok(slot) = state.download_slots.clone().try_acquire_owned() else {
        return storage_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Download capacity exhausted",
        );
    };
    let result = async {
        let io_owner = state.auth.blocking().clone();
        let processes = state.probe_processes.clone();
        let probe_jobs = state.probe_jobs.clone();
        state
            .download_jobs
            .submit_async(move |cancel| async move {
                let stage = tokio::task::spawn_blocking(crate::storage::local::anonymous_download)
                    .await
                    .ok()?
                    .ok()?;
                let mut archive = async_zip::tokio::write::ZipFileWriter::with_tokio(
                    tokio::fs::File::from_std(stage),
                );
                let mut buf = [0; 64 * 1024];
                for path in paths {
                    if cancel.is_cancelled() {
                        return None;
                    }
                    let input = match &source {
                        DownloadSource::Sftp(endpoint, _key) => {
                            // The source retains the original identity FD until every child settles.
                            let work = endpoint
                                .download_snapshot(
                                    processes.as_ref()?,
                                    &probe_jobs,
                                    &path,
                                    None,
                                    (),
                                )
                                .ok()?;
                            let (file, _, ()) = rclone::settled(work, Some(&cancel)).await?.ok()?;
                            file
                        }
                        DownloadSource::Ftp(provider) => {
                            let work = provider
                                .download_snapshot(path.clone(), &io_owner, ())
                                .ok()?;
                            let (file, _, ()) = rclone::settled(work, Some(&cancel)).await?.ok()?;
                            file
                        }
                        DownloadSource::S3(provider) => {
                            let work = provider.download_snapshot(path.clone(), ()).ok()?;
                            match rclone::settled(work, Some(&cancel)).await? {
                                Ok((file, _, ())) => file,
                                Err(crate::storage::s3::S3Error::Http(404)) => continue,
                                Err(crate::storage::s3::S3Error::Provider {
                                    status: Some(404),
                                    code,
                                }) if matches!(code.as_str(), "NoSuchKey" | "NotFound" | "404") => {
                                    continue;
                                }
                                Err(_) => return None,
                            }
                        }
                        DownloadSource::Webdav(provider) => {
                            let work = provider.download_snapshot(path.clone(), ()).ok()?;
                            match rclone::settled(work, Some(&cancel)).await? {
                                Ok((file, _, ())) => file,
                                Err(crate::storage::webdav::DavError::Http(404)) => continue,
                                Err(_) => return None,
                            }
                        }
                        DownloadSource::Local(root) => {
                            let root = root.clone();
                            let source_path = path.clone();
                            let source_cancel = cancel.clone();
                            match tokio::task::spawn_blocking(move || {
                                crate::storage::local::snapshot_download(
                                    &root,
                                    Path::new(&source_path),
                                    &|| source_cancel.is_cancelled(),
                                )
                            })
                            .await
                            .ok()?
                            {
                                Ok((file, _)) => file,
                                Err(error)
                                    if matches!(
                                        error.kind(),
                                        std::io::ErrorKind::NotFound
                                            | std::io::ErrorKind::IsADirectory
                                    ) =>
                                {
                                    continue;
                                }
                                Err(_) => return None,
                            }
                        }
                        _ => return None,
                    };
                    let mut input = tokio::fs::File::from_std(input);
                    let opts = async_zip::ZipEntryBuilder::new(
                        path.rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_owned()
                            .into(),
                        async_zip::Compression::Deflate,
                    );
                    let mut entry = archive.write_entry_stream(opts).await.ok()?.compat_write();
                    loop {
                        if cancel.is_cancelled() {
                            return None;
                        }
                        let count = input.read(&mut buf).await.ok()?;
                        if count == 0 {
                            break;
                        }
                        entry.write_all(&buf[..count]).await.ok()?;
                    }
                    entry.into_inner().close().await.ok()?;
                }
                let mut file = archive.close().await.ok()?.into_inner();
                file.flush().await.ok()?;
                let mut file = file.into_std().await;
                tokio::task::spawn_blocking(move || {
                    let length = file.metadata().ok()?.len();
                    file.rewind().ok()?;
                    let validators = download::Validators::read(&file, length).ok()?;
                    Some((file, length, slot, validators))
                })
                .await
                .ok()?
            })
            .ok()?
            .wait()
            .await
            .ok()?
    }
    .await;
    let Some((file, length, slot, validators)) = result else {
        return storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Bulk download failed");
    };
    download::response(
        file,
        length,
        slot,
        disposition,
        validators,
        &axum::http::HeaderMap::new(),
        "application/zip",
    )
}

pub(super) async fn local_size(
    State(state): State<HttpState>,
    axum::extract::Query(query): BrowseQuery,
) -> Response {
    let Some(path) = query.get("path") else {
        return missing_size_path();
    };
    size_at(&state, state.config.backup_base_path.clone(), path).await
}

async fn size_at(state: &HttpState, root: String, raw: &str) -> Response {
    let unavailable = || unavailable_size(raw);
    let Some(path) = crate::storage::relative_path(raw, false) else {
        return unavailable();
    };
    let Ok(work) = state.auth.blocking().submit_blocking(move |cancel| {
        let bytes =
            crate::storage::local::directory_size(std::path::Path::new(&root), &path, &|| {
                cancel.is_cancelled()
            })?;
        Ok::<_, std::io::Error>((path, bytes))
    }) else {
        return unavailable();
    };
    match work.wait().await {
        Ok(Ok((path, bytes))) => {
            Json(json!({"path":path,"state":"completed","bytes":bytes})).into_response()
        }
        _ => unavailable(),
    }
}

pub(super) async fn list_local_files(
    State(state): State<HttpState>,
    axum::extract::Query(query): BrowseQuery,
) -> Response {
    list_at(
        &state,
        state.config.backup_base_path.clone(),
        query.get("path").map_or("", String::as_str),
    )
    .await
}

async fn list_at(state: &HttpState, root: String, raw: &str) -> Response {
    let Some(path) = crate::storage::relative_path(raw, true) else {
        return storage_error(StatusCode::BAD_REQUEST, "Invalid file path");
    };
    let result = state.auth.blocking().submit_blocking(move |cancel| {
        let files =
            crate::storage::local::list_directory(std::path::Path::new(&root), &path, &|| {
                cancel.is_cancelled()
            })?;
        Ok::<_, std::io::Error>((path, files))
    });
    let Ok(work) = result else {
        return storage_error(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable");
    };
    match work.wait().await {
        Ok(Ok((path, files))) => {
            #[derive(serde::Serialize)]
            struct Listing {
                files: Vec<crate::storage::local::LocalEntry>,
                path: String,
            }
            Json(Listing { files, path }).into_response()
        }
        Ok(Err(error)) => match error.kind() {
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotADirectory => {
                storage_error(StatusCode::BAD_REQUEST, "Invalid file path")
            }
            std::io::ErrorKind::NotFound => {
                storage_error(StatusCode::NOT_FOUND, "Directory not found")
            }
            std::io::ErrorKind::PermissionDenied => {
                storage_error(StatusCode::FORBIDDEN, "Permission denied")
            }
            _ => storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Listing failed"),
        },
        Err(_) => storage_error(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable"),
    }
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
