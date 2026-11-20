use super::HttpState;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize)]
struct Counts {
    backup_targets: u64,
    schedules: u64,
    retention_policies: u64,
    remote_storages: u64,
    encryption_config: u64,
    app_settings: u64,
    backups: u64,
    backup_schedules: u64,
    backup_storage_syncs: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Provider {
    Local,
    Ssh,
    Sftp,
    S3,
    Ftp,
    Webdav,
    Rclone,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Connection {
    Connected,
    Offline,
    CredentialsRequired,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Availability {
    Available,
    Unavailable,
    Offline,
    CredentialsRequired,
}

#[derive(Deserialize, Serialize)]
struct Remote {
    id: i64,
    #[serde(rename = "type")]
    provider: Provider,
    state: Connection,
}

#[derive(Deserialize, Serialize)]
struct CopyStatus {
    storage_id: i64,
    state: Availability,
}

#[derive(Deserialize, Serialize)]
struct CatalogEntry {
    id: i64,
    local: Availability,
    copies: Vec<CopyStatus>,
}

#[derive(Deserialize, Serialize)]
struct PublicReference {
    name: String,
    public_key: String,
}

#[derive(Deserialize, Serialize)]
struct RecoveryResult {
    counts: Counts,
    storages: Vec<Remote>,
    catalog: Vec<CatalogEntry>,
    archives_modified: bool,
    #[serde(skip_deserializing)]
    ssh_public_keys: Vec<PublicReference>,
}

pub(super) async fn status(State(state): State<HttpState>) -> impl IntoResponse {
    let row: Result<(Option<String>, Option<String>), _> = sqlx::query_as(
        "SELECT (SELECT value FROM app_settings WHERE key='recovery_result'), \
         (SELECT value FROM app_settings WHERE key='recovery_ssh_public_keys')",
    )
    .fetch_one(state.auth.pool())
    .await;
    match row
        .ok()
        .and_then(|(result, references)| decode(result.as_deref(), references.as_deref()))
    {
        Some(result) => Json(result).into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"detail":"Internal server error"})),
        )
            .into_response(),
    }
}

// Read the previous outcome, not current provider health. Project only public fields.
fn decode(result: Option<&str>, references: Option<&str>) -> Option<Option<RecoveryResult>> {
    let Some(result) = result.filter(|value| !value.is_empty()) else {
        return Some(None);
    };
    let mut result: RecoveryResult = serde_json::from_str(result).ok()?;
    if result.archives_modified {
        return None;
    }
    result.ssh_public_keys =
        serde_json::from_str(references.filter(|v| !v.is_empty()).unwrap_or("[]")).ok()?;
    for reference in &result.ssh_public_keys {
        if !(1..=128).contains(&reference.name.len())
            || !reference
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            || reference.public_key.contains(['\r', '\n'])
        {
            return None;
        }
        dockervault_core::ssh_public::canonical_public_key(reference.public_key.as_bytes()).ok()?;
    }
    Some(Some(result))
}
