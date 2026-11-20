use super::{HttpState, auth, schedules};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dockervault_core::timestamp::Timestamp;
use serde_json::{Value, json};
use sqlx::{Row, sqlite::SqliteRow};
use std::collections::{HashMap, HashSet};

const SELECT: &str = "SELECT t.*, s.name AS schedule_name,s.cron_expression AS shared_cron,
  p.name AS policy_name,p.keep_last,p.keep_daily,p.keep_weekly,p.keep_monthly
  FROM backup_targets t LEFT JOIN schedules s ON s.id=t.schedule_id
  LEFT JOIN retention_policies p ON p.id=t.retention_policy_id
  WHERE (?1 OR t.removed_at IS NULL) AND (?2 IS NULL OR t.id=?2) ORDER BY t.id";

pub(super) async fn list(
    State(state): State<HttpState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let include = match query
        .get("include_removed")
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        None | Some("0" | "off" | "f" | "false" | "n" | "no") => false,
        Some("1" | "on" | "t" | "true" | "y" | "yes") => true,
        _ => {
            return *auth::validation(vec![
                json!({"field":"query.include_removed","message":"Input should be a valid boolean, unable to interpret input"}),
            ]);
        }
    };
    read(&state, include, None).await
}

pub(super) async fn detail(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    let Some(id) = super::operations::query_integer(&id) else {
        return *auth::validation(vec![
            json!({"field":"path.target_id","message":"Input should be a valid integer, unable to parse string as an integer"}),
        ]);
    };
    read(&state, false, Some(id)).await
}

async fn read(state: &HttpState, include: bool, id: Option<i64>) -> Response {
    match fetch(state, include, id).await {
        Ok(mut rows) if id.is_some() => rows.pop().map_or_else(
            || {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"detail":"Target not found"})),
                )
                    .into_response()
            },
            |row| Json(row).into_response(),
        ),
        Ok(rows) => Json(rows).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"detail":"Internal server error"})),
        )
            .into_response(),
    }
}

async fn fetch(
    state: &HttpState,
    include: bool,
    id: Option<i64>,
) -> Result<Vec<Value>, sqlx::Error> {
    let mut tx = state.auth.pool().begin().await?;
    let rows = sqlx::query(SELECT)
        .bind(include)
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
    let known: HashSet<_> = sqlx::query_scalar::<_, i64>("SELECT id FROM remote_storages")
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
    tx.commit().await?;
    let cron = schedules::Cron::load(state).ok();
    rows.iter()
        .map(|row| project(row, &known, cron.as_ref()))
        .collect()
}

fn stored_json(row: &SqliteRow, key: &str, default: Value) -> Result<Value, sqlx::Error> {
    let raw: Option<String> = row.try_get(key)?;
    raw.map(|raw| serde_json::from_str::<Value>(&raw))
        .transpose()
        .map(|value| value.filter(|v| !v.is_null()).unwrap_or(default))
        .map_err(|err| sqlx::Error::Decode(Box::new(err)))
}

fn target_timestamp(row: &SqliteRow, key: &str) -> Result<Option<String>, sqlx::Error> {
    row.try_get::<Option<String>, _>(key)?
        .map(|raw| {
            let parsed =
                Timestamp::parse(&raw).map_err(|err| sqlx::Error::Decode(Box::new(err)))?;
            Ok(match parsed.unix_micros() {
                None => format!("{}Z", parsed.api()),
                Some(us) => {
                    let utc =
                        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(us) * 1_000)
                            .map_err(|err| sqlx::Error::Decode(Box::new(err)))?;
                    let raw = utc
                        .format(&time::format_description::well_known::Rfc3339)
                        .map_err(|err| sqlx::Error::Decode(Box::new(err)))?;
                    let parsed =
                        Timestamp::parse(&raw).map_err(|err| sqlx::Error::Decode(Box::new(err)))?;
                    parsed.api()
                }
            })
        })
        .transpose()
}

fn project(
    row: &SqliteRow,
    known: &HashSet<i64>,
    cron: Option<&schedules::Cron>,
) -> Result<Value, sqlx::Error> {
    let schedule_id: Option<i64> = row.try_get("schedule_id")?;
    let policy_id: Option<i64> = row.try_get("retention_policy_id")?;
    let legacy: Option<String> = row.try_get("schedule_cron")?;
    let shared: Option<String> = row.try_get("shared_cron")?;
    let schedule = row
        .try_get::<Option<String>, _>("schedule_name")?
        .map(|name| json!({"id":schedule_id,"name":name,"cron_expression":shared}));
    let policy = if let Some(name) = row.try_get::<Option<String>, _>("policy_name")? {
        Some(
            json!({"id":policy_id,"name":name,"keep_last":row.try_get::<i64,_>("keep_last")?,"keep_daily":row.try_get::<i64,_>("keep_daily")?,"keep_weekly":row.try_get::<i64,_>("keep_weekly")?,"keep_monthly":row.try_get::<i64,_>("keep_monthly")?}),
        )
    } else {
        None
    };
    let remote = stored_json(row, "remote_storage_ids", json!([]))?;
    let ids: Vec<i64> =
        serde_json::from_value(remote).map_err(|err| sqlx::Error::Decode(Box::new(err)))?;
    let mut validation = Vec::new();
    if let Some(id) = schedule_id
        && schedule.is_none()
    {
        validation.push(format!("schedule_id {id} does not exist"));
    }
    if let Some(id) = policy_id
        && policy.is_none()
    {
        validation.push(format!("retention_policy_id {id} does not exist"));
    }
    validation.extend(
        ids.iter()
            .filter(|id| !known.contains(id))
            .map(|id| format!("remote_storage_id {id} does not exist")),
    );
    let warnings = shared
        .as_deref()
        .or(legacy.as_deref())
        .and_then(|expression| cron.map(|cron| cron.inspect(expression).1))
        .unwrap_or_default();
    let mut target = json!({
        "id":row.try_get::<i64,_>("id")?,"schedule_id":schedule_id,"schedule":schedule,
        "schedule_cron":legacy,"cron_warnings":warnings,"retention_policy_id":policy_id,
        "retention_policy":policy,"remote_storage_ids":ids,"validation_errors":validation,
        "created_at":target_timestamp(row,"created_at")?.unwrap_or_default(),
        "updated_at":target_timestamp(row,"updated_at")?.unwrap_or_default(),
        "removed_at":target_timestamp(row,"removed_at")?,
    });
    for key in [
        "name",
        "target_type",
        "container_id",
        "container_name",
        "volume_name",
        "host_path",
        "stack_name",
        "pre_backup_command",
        "post_backup_command",
    ] {
        target[key] = json!(row.try_get::<Option<String>, _>(key)?);
    }
    for key in [
        "enabled",
        "stop_container",
        "compression_enabled",
        "delete_local_after_sync",
    ] {
        target[key] = json!(row.try_get::<Option<bool>, _>(key)?.unwrap_or_default());
    }
    for key in [
        "dependencies",
        "selected_volumes",
        "include_paths",
        "exclude_paths",
    ] {
        target[key] = stored_json(row, key, json!([]))?;
    }
    target["per_volume_rules"] = stored_json(row, "per_volume_rules", json!({}))?;
    Ok(target)
}
