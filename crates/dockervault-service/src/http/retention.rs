use super::{HttpState, auth, backups::format_size};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dockervault_core::timestamp::Timestamp;
use serde_json::{Value, json};
use sqlx::{Row, sqlite::SqliteRow};

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
