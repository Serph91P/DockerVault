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
    let Ok(id) = raw.parse::<i64>() else {
        return *auth::validation(vec![
            json!({"field":"path.storage_id","message":"Input should be a valid integer, unable to parse string as an integer"}),
        ]);
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

fn storage_error(status: StatusCode, detail: &'static str) -> Response {
    (status, Json(json!({"detail":detail}))).into_response()
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
