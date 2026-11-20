use super::HttpState;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::json;

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
