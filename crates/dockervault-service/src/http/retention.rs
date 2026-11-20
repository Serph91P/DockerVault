use super::{HttpState, auth, backups::format_size};
use axum::{
    Json,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dockervault_core::timestamp::Timestamp;
use serde_json::{Value, json};
use sqlx::{Row, sqlite::SqliteRow};

const FIELDS: [(&str, i64, i64, i64); 6] = [
    ("keep_last", 3, 1, 1000),
    ("keep_daily", 7, 0, 1000),
    ("keep_weekly", 4, 0, 1000),
    ("keep_monthly", 6, 0, 1000),
    ("keep_yearly", 2, 0, 1000),
    ("max_age_days", 365, 1, 36500),
];

pub(super) struct Selection {
    pub kept: usize,
    pub expired: Vec<i64>,
}

pub(super) const RECOVERY_PENDING: &str = "Recovery activation is incomplete";

pub(super) async fn cleanup_orphaned(State(state): State<HttpState>) -> Response {
    match crate::recovery_activation::recovery_pending(state.auth.pool()).await {
        Ok(true) => return failure(StatusCode::SERVICE_UNAVAILABLE, RECOVERY_PENDING),
        Err(_) => return internal(),
        Ok(false) => {}
    }
    let root = std::path::PathBuf::from(&state.config.backup_base_path);
    let Ok(work) = state.download_jobs.submit_blocking(move |cancel| {
        crate::storage::local::protected_archives(&root, &|| cancel.is_cancelled())
    }) else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "Orphan scan capacity exhausted",
        );
    };
    let Ok((protected, scan_errors)) = work.wait().await else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "Orphan scan interrupted");
    };
    Json(
        json!({"protected":protected, "deleted":0,"failed":0,"scan_errors":scan_errors,
        "scan_complete":scan_errors==0,"outcome":if scan_errors==0 {"empty"}else{"failure"},
        "freed_bytes":0,"freed_human":format_size(0)}),
    )
    .into_response()
}

pub(super) fn start_sweep(state: &HttpState) {
    let ctx = state.clone();
    match state.backup_jobs.submit_async(move |cancel| async move {
        let targets: Vec<i64> = match sqlx::query_scalar(
            "SELECT id FROM backup_targets WHERE removed_at IS NULL ORDER BY id",
        )
        .fetch_all(ctx.auth.pool())
        .await
        {
            Ok(targets) => targets,
            Err(error) => {
                tracing::warn!(%error, "Could not enumerate retention targets");
                return;
            }
        };
        for target in targets {
            if cancel.is_cancelled() || ctx.draining.is_cancelled() {
                break;
            }
            if let Err(message) = super::copy_removal::retain_local(&ctx, target, &cancel).await {
                if message == RECOVERY_PENDING {
                    break;
                }
                tracing::warn!(
                    target_id = target,
                    message,
                    "Retention sweep did not settle target"
                );
            }
        }
    }) {
        Ok(work) => work.detach(),
        Err(_) => tracing::warn!("Retention sweep capacity exhausted"),
    }
}

pub(super) async fn select(
    db: &mut sqlx::SqliteConnection,
    target: i64,
    now: i64,
) -> Result<Selection, &'static str> {
    use dockervault_core::retention::{Policy, keep};
    if crate::recovery_activation::recovery_pending_in(&mut *db)
        .await
        .map_err(|_| "Could not read retention selection")?
    {
        return Err(RECOVERY_PENDING);
    }
    let (removed, policy_id): (bool, Option<i64>) = sqlx::query_as(
        "SELECT removed_at IS NOT NULL, retention_policy_id FROM backup_targets WHERE id=?",
    )
    .bind(target)
    .fetch_optional(&mut *db)
    .await
    .map_err(|_| "Could not read retention selection")?
    .ok_or("Target not found")?;
    if removed {
        return Err("Plan removed");
    }
    let (keep_last, daily, weekly, monthly, yearly, max_age_days): (Option<i64>, i64, i64, i64, i64, i64) = sqlx::query_as("SELECT keep_last,keep_daily,keep_weekly,keep_monthly,keep_yearly,max_age_days FROM retention_policies WHERE id=?1 OR (?1 IS NULL AND name='default')")
        .bind(policy_id.filter(|id| *id != 0)).fetch_optional(&mut *db).await.map_err(|_| "Could not read retention selection")?.ok_or("No policy found")?;
    let rows = sqlx::query_as::<_, (i64, String)>("SELECT id,created_at FROM backups WHERE target_id=? AND status='COMPLETED' ORDER BY created_at DESC,id ASC")
        .bind(target).fetch_all(&mut *db).await.map_err(|_| "Could not read retention selection")?;
    let rows = rows
        .into_iter()
        .map(|(id, raw)| Timestamp::parse(&raw).map(|stamp| (id, stamp)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Could not read retention selection")?;
    let kept = keep(
        &rows,
        &Policy {
            keep_last: keep_last.unwrap_or(0),
            buckets: [daily, weekly, monthly, yearly],
            max_age_days,
        },
        now,
    );
    Ok(Selection {
        kept: kept.len(),
        expired: rows
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| !kept.contains(id))
            .collect(),
    })
}

pub(super) async fn apply(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(response) = auth::required_user(&state, &headers).await {
        return *response;
    }
    let Some(id) = super::operations::query_integer(&id) else {
        return invalid_id("path.target_id");
    };
    match super::copy_removal::apply_retention(&state, id).await {
        Ok((kept, deleted)) => Json(json!({"kept":kept,"deleted":deleted})).into_response(),
        Err(message @ ("Target not found" | "Plan removed" | "No policy found")) => {
            failure(StatusCode::BAD_REQUEST, message)
        }
        Err(
            message @ (RECOVERY_PENDING | "Retention interrupted" | "Retention capacity exhausted"),
        ) => failure(StatusCode::SERVICE_UNAVAILABLE, message),
        Err(_) => internal(),
    }
}

type PolicyFields<'a> = (Option<&'a str>, [Option<i64>; 6]);

fn write_fields(body: &Value, create: bool) -> Result<PolicyFields<'_>, Vec<Value>> {
    let mut errors = Vec::new();
    let name = match body.get("name") {
        None if create => {
            errors.push(json!({"field":"body.name","message":"Field required"}));
            None
        }
        None | Some(Value::Null) if !create => None,
        Some(Value::String(name)) => Some(name.as_str()),
        _ => {
            errors.push(json!({"field":"body.name","message":"Input should be a valid string"}));
            None
        }
    };
    let values = FIELDS.map(|(field, default, min, max)| {
        let value = match body.get(field) {
            None => return create.then_some(default),
            Some(Value::Null) if !create => return None,
            Some(Value::Bool(value)) => Ok(f64::from(u8::from(*value))),
            Some(Value::Number(value)) => value
                .as_f64()
                .filter(|n| n.fract() == 0.0)
                .ok_or("Input should be a valid integer, got a number with a fractional part"),
            Some(Value::String(value)) => super::operations::query_integer(value)
                .map(|n| n as f64)
                .ok_or("Input should be a valid integer, unable to parse string as an integer"),
            _ => Err("Input should be a valid integer"),
        };
        let message = match value {
            Ok(value) if value < min as f64 => {
                format!("Input should be greater than or equal to {min}")
            }
            Ok(value) if value > max as f64 => {
                format!("Input should be less than or equal to {max}")
            }
            Ok(value) => return Some(value as i64),
            Err(message) => message.to_owned(),
        };
        errors.push(json!({"field":format!("body.{field}"),"message":message}));
        None
    });
    if errors.is_empty() {
        Ok((name, values))
    } else {
        Err(errors)
    }
}

pub(super) async fn create(State(state): State<HttpState>, request: Request) -> Response {
    write(state, request, None).await
}
pub(super) async fn update(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let Some(id) = super::operations::query_integer(&id) else {
        return invalid_id("path.policy_id");
    };
    write(state, request, Some(id)).await
}

async fn write(state: HttpState, mut request: Request, id: Option<i64>) -> Response {
    let body = match auth::json_body(&state, &mut request).await {
        Ok(body) => body,
        Err(error) => return *error,
    };
    let (name, [last, daily, weekly, monthly, yearly, age]) =
        match write_fields(&body, id.is_none()) {
            Ok(fields) => fields,
            Err(errors) => return *auth::validation(errors),
        };
    let mut tx = match state.auth.pool().begin_with("BEGIN IMMEDIATE").await {
        Ok(tx) => tx,
        Err(_) => return internal(),
    };
    let query = if let Some(id) = id {
        sqlx::query("UPDATE retention_policies SET updated_at=CASE WHEN
            (?1 IS NOT NULL AND name IS NOT ?1) OR (?2 IS NOT NULL AND keep_last IS NOT ?2)
            OR (?3 IS NOT NULL AND keep_daily IS NOT ?3) OR (?4 IS NOT NULL AND keep_weekly IS NOT ?4)
            OR (?5 IS NOT NULL AND keep_monthly IS NOT ?5) OR (?6 IS NOT NULL AND keep_yearly IS NOT ?6)
            OR (?7 IS NOT NULL AND max_age_days IS NOT ?7) THEN strftime('%Y-%m-%d %H:%M:%f000','now') ELSE updated_at END,
            name=coalesce(?1,name), keep_last=coalesce(?2,keep_last), keep_daily=coalesce(?3,keep_daily),
            keep_weekly=coalesce(?4,keep_weekly), keep_monthly=coalesce(?5,keep_monthly), keep_yearly=coalesce(?6,keep_yearly),
            max_age_days=coalesce(?7,max_age_days) WHERE id=?8 RETURNING *")
            .bind(name).bind(last).bind(daily).bind(weekly).bind(monthly).bind(yearly).bind(age).bind(id)
    } else {
        sqlx::query("INSERT INTO retention_policies(name,keep_last,keep_daily,keep_weekly,keep_monthly,keep_yearly,max_age_days,created_at,updated_at)
            VALUES(?,?,?,?,?,?,?,strftime('%Y-%m-%d %H:%M:%f000','now'),strftime('%Y-%m-%d %H:%M:%f000','now')) RETURNING *")
            .bind(name).bind(last).bind(daily).bind(weekly).bind(monthly).bind(yearly).bind(age)
    };
    let result = match query
        .fetch_optional(&mut *tx)
        .await
        .and_then(|row| row.map(policy).transpose())
    {
        Ok(Some(value)) => value,
        Ok(None) => return failure(StatusCode::NOT_FOUND, "Policy not found"),
        Err(e)
            if id.is_none()
                && e.as_database_error()
                    .is_some_and(|e| e.is_unique_violation()) =>
        {
            return failure(
                StatusCode::BAD_REQUEST,
                "Policy with this name already exists",
            );
        }
        Err(_) => return internal(),
    };
    if tx.commit().await.is_err() {
        return internal();
    }
    Json(result).into_response()
}

pub(super) async fn delete(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    let Some(id) = super::operations::query_integer(&id) else {
        return invalid_id("path.policy_id");
    };
    let mut tx = match state.auth.pool().begin_with("BEGIN IMMEDIATE").await {
        Ok(tx) => tx,
        Err(_) => return internal(),
    };
    let row: Result<Option<(String,bool)>, _> = sqlx::query_as("SELECT name, EXISTS(SELECT 1 FROM backup_targets WHERE retention_policy_id=?1) FROM retention_policies WHERE id=?1").bind(id).fetch_optional(&mut *tx).await;
    match row {
        Ok(Some((name, _))) if name == "default" => {
            return failure(StatusCode::BAD_REQUEST, "Cannot delete default policy");
        }
        Ok(Some((_, true))) => {
            return failure(
                StatusCode::CONFLICT,
                "Policy is still referenced by a plan, including removed plans",
            );
        }
        Ok(Some((_, false))) => {}
        Ok(None) => return failure(StatusCode::NOT_FOUND, "Policy not found"),
        Err(_) => return internal(),
    }
    match sqlx::query("DELETE FROM retention_policies WHERE id=?")
        .bind(id)
        .execute(&mut *tx)
        .await
    {
        Ok(result) if result.rows_affected() == 1 => {}
        Err(e)
            if e.as_database_error()
                .is_some_and(|e| e.is_foreign_key_violation()) =>
        {
            return failure(
                StatusCode::CONFLICT,
                "Policy is still referenced by a plan, including removed plans",
            );
        }
        _ => return internal(),
    }
    if tx.commit().await.is_err() {
        return internal();
    }
    Json(json!({"status":"deleted"})).into_response()
}

fn failure(status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({"detail":detail}))).into_response()
}
fn internal() -> Response {
    failure(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

fn timestamp(raw: String) -> Result<String, sqlx::Error> {
    Timestamp::parse(&raw)
        .map(Timestamp::api)
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))
}

fn policy(row: SqliteRow) -> Result<Value, sqlx::Error> {
    Ok(json!({
        "id": row.try_get::<i64,_>("id")?, "name": row.try_get::<String,_>("name")?,
        "keep_last": row.try_get::<i64,_>("keep_last")?, "keep_daily": row.try_get::<i64,_>("keep_daily")?,
        "keep_weekly": row.try_get::<i64,_>("keep_weekly")?, "keep_monthly": row.try_get::<i64,_>("keep_monthly")?,
        "keep_yearly": row.try_get::<i64,_>("keep_yearly")?, "max_age_days": row.try_get::<i64,_>("max_age_days")?,
        "created_at": timestamp(row.try_get("created_at")?)?, "updated_at": timestamp(row.try_get("updated_at")?)?,
    }))
}

fn invalid_id(field: &str) -> Response {
    *auth::validation(vec![
        json!({"field":field,"message":"Input should be a valid integer, unable to parse string as an integer"}),
    ])
}

pub(super) async fn list(State(state): State<HttpState>) -> Response {
    match sqlx::query("SELECT * FROM retention_policies ORDER BY id")
        .fetch_all(state.auth.pool())
        .await
        .and_then(|rows| rows.into_iter().map(policy).collect::<Result<Vec<_>, _>>())
    {
        Ok(policies) => Json(policies).into_response(),
        Err(_) => internal(),
    }
}

pub(super) async fn detail(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    let Ok(id) = id.trim().parse::<i64>() else {
        return invalid_id("path.policy_id");
    };
    match sqlx::query("SELECT * FROM retention_policies WHERE id=?")
        .bind(id)
        .fetch_optional(state.auth.pool())
        .await
        .and_then(|row| row.map(policy).transpose())
    {
        Ok(Some(policy)) => Json(policy).into_response(),
        Ok(None) => failure(StatusCode::NOT_FOUND, "Policy not found"),
        Err(_) => internal(),
    }
}

pub(super) async fn stats(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    let Ok(id) = id.trim().parse::<i64>() else {
        return invalid_id("path.target_id");
    };
    let stats: Result<(i64, i64, Option<String>, Option<String>), _> = sqlx::query_as(
        "SELECT count(*), coalesce(sum(file_size),0), min(created_at), max(created_at)
         FROM backups WHERE target_id=? AND status='COMPLETED'",
    )
    .bind(id)
    .fetch_one(state.auth.pool())
    .await;
    match stats {
        Ok((0, _, _, _)) => Json(json!({"total_backups":0,"total_size_bytes":0,"oldest_backup":null,"newest_backup":null})).into_response(),
        Ok((count, size, Some(oldest), Some(newest))) => {
            let (Ok(oldest), Ok(newest)) = (timestamp(oldest), timestamp(newest)) else { return internal(); };
            Json(json!({"total_backups":count,"total_size_bytes":size,"total_size_human":format_size(size),"oldest_backup":oldest,"newest_backup":newest})).into_response()
        }
        _ => internal(),
    }
}
