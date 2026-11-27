use super::HttpState;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::json;

const QUOTA: &str = "storage_check_quota_before_upload";

pub(super) async fn quota_enabled(pool: &sqlx::SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM app_settings WHERE key=? AND value='true')")
        .bind(QUOTA)
        .fetch_one(pool)
        .await
}

pub(super) async fn storage(State(state): State<HttpState>) -> axum::response::Response {
    match quota_enabled(state.auth.pool()).await {
        Ok(enabled) => Json(json!({"check_quota_before_upload":enabled})).into_response(),
        Err(_) => storage_error(),
    }
}

pub(super) async fn update_storage(
    State(state): State<HttpState>,
    mut request: axum::extract::Request,
) -> axum::response::Response {
    let body = match super::auth::json_body(&state, &mut request).await {
        Ok(body) => body,
        Err(error) => return *error,
    };
    let key = "check_quota_before_upload";
    let parse = |value: &serde_json::Value| {
        value.as_bool().or_else(
            || match value.as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("1" | "on" | "t" | "true" | "y" | "yes") => Some(true),
                Some("0" | "off" | "f" | "false" | "n" | "no") => Some(false),
                _ => match value.as_f64() {
                    Some(0.0) => Some(false),
                    Some(1.0) => Some(true),
                    _ => None,
                },
            },
        )
    };
    let Some(enabled) = body.get(key).and_then(parse) else {
        return *super::auth::validation(vec![
            json!({"field":"body.check_quota_before_upload","message":if body.get(key).is_some() { "Input should be a valid boolean" } else { "Field required" }}),
        ]);
    };
    match sqlx::query("INSERT INTO app_settings(key,value,updated_at) VALUES(?,?,strftime('%Y-%m-%d %H:%M:%f000','now')) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at").bind(QUOTA).bind(enabled.to_string()).execute(state.auth.pool()).await {
        Ok(result) if result.rows_affected() == 1 => Json(json!({"check_quota_before_upload":enabled})).into_response(),
        _ => storage_error(),
    }
}

fn storage_error() -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"detail":"Internal server error"})),
    )
        .into_response()
}

pub(super) async fn system_info(State(state): State<HttpState>) -> impl IntoResponse {
    let counts: Result<(i64, i64), _> = sqlx::query_as(
        "SELECT (SELECT count(*) FROM backups),(SELECT count(*) FROM backup_targets)",
    )
    .fetch_one(state.auth.pool())
    .await;
    let Ok((backup_count, target_count)) = counts else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"detail":"Internal server error"})),
        )
            .into_response();
    };
    let config = state.config.clone();
    let facts = state.auth.blocking().submit_blocking(move |_| {
        let disk = nix::sys::statvfs::statvfs(config.backup_base_path.as_str())
            .ok()
            .map(|fs| {
                let unit = fs.fragment_size();
                (
                    fs.blocks().saturating_mul(unit),
                    fs.blocks()
                        .saturating_sub(fs.blocks_free())
                        .saturating_mul(unit),
                    fs.blocks_available().saturating_mul(unit),
                )
            });
        let db_size = config
            .database_url
            .strip_prefix("sqlite+aiosqlite:///")
            .or_else(|| config.database_url.strip_prefix("sqlite:///"))
            .and_then(|path| std::fs::metadata(path).ok())
            .map_or(0, |m| m.len());
        (disk, db_size)
    });
    let facts = match facts {
        Ok(work) => work.wait().await.ok(),
        Err(_) => None,
    };
    let Some((disk, db_size)) = facts else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"detail":"System information temporarily unavailable"})),
        )
            .into_response();
    };
    let disk_status = if disk.is_some() {
        "available"
    } else {
        "unavailable"
    };
    let (disk_total, disk_used, disk_free) = disk.unwrap_or_default();
    Json(json!({
        "backup_dir":"[configured]", "database_path":"[configured]",
        "timezone":state.config.tz, "disk_total":disk_total, "disk_status":disk_status,
        "disk_used":disk_used, "disk_free":disk_free, "db_size":db_size,
        "backup_count":backup_count, "target_count":target_count,
        "uptime_seconds":state.started.elapsed().as_secs(), "app_version":"1.0.0"
    }))
    .into_response()
}
