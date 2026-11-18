use super::{HttpState, auth::required_user};
use crate::storage_recovery::{storage_operation_detail, storage_operation_list};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::collections::HashMap;

pub(super) async fn detail(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user = match required_user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return *response,
    };
    snapshot(&state, user.id, &id).await
}

pub(super) async fn cancel(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user = match required_user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return *response,
    };
    match crate::storage_recovery::request_cancel(state.auth.pool(), user.id, &id).await {
        Ok(true) => snapshot(&state, user.id, &id).await,
        Ok(false) => failure(StatusCode::NOT_FOUND, "Operation not found"),
        Err(_) => failure(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
}

pub(super) async fn list(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<
        axum::extract::Query<HashMap<String, String>>,
        axum::extract::rejection::QueryRejection,
    >,
) -> Response {
    let user = match required_user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return *response,
    };
    let Ok(axum::extract::Query(query)) = query else {
        return *super::auth::validation(vec![json!({"field":"query","message":"Invalid value"})]);
    };
    const INTEGER: &str = "Input should be a valid integer, unable to parse string as an integer";
    let mut errors = Vec::new();
    // FastAPI uses the final value of repeated scalar query parameters.
    let storage = match query.get("storage_id").map(String::as_str) {
        None => None,
        Some("local") => Some("local".into()),
        Some(value) => match query_integer(value) {
            Some(value) if value > 0 => Some(value.to_string()),
            parsed => {
                errors.push(json!({"field":"query.storage_id.literal['local']","message":"Input should be 'local'"}));
                errors.push(json!({"field":"query.storage_id.constrained-int","message":if parsed.is_some() { "Input should be greater than 0" } else { INTEGER }}));
                None
            }
        },
    };
    let limit = query
        .get("limit")
        .map_or(Some(20), |value| query_integer(value));
    let message = match limit {
        None => Some(INTEGER),
        Some(n) if n < 1 => Some("Input should be greater than or equal to 1"),
        Some(n) if n > 100 => Some("Input should be less than or equal to 100"),
        _ => None,
    };
    if let Some(message) = message {
        errors.push(json!({"field":"query.limit","message":message}));
    }
    if !errors.is_empty() {
        return *super::auth::validation(errors);
    }
    let limit = limit.expect("invalid limit reported as field error");
    match storage_operation_list(state.auth.pool(), user.id, storage.as_deref(), limit).await {
        Ok(rows) => Json(rows).into_response(),
        Err(_) => failure(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
}

// Match the legacy coercion of query strings without rounding or changing JSON validation.
fn query_integer(value: &str) -> Option<i64> {
    let value = value.trim();
    let integer = match value.split_once('.') {
        Some((integer, fraction))
            if !fraction.is_empty() && fraction.bytes().all(|b| b == b'0') =>
        {
            integer
        }
        Some(_) => return None,
        None => value,
    };
    if integer.contains('_') {
        let digits = integer.strip_prefix(['+', '-']).unwrap_or(integer);
        if !digits
            .split('_')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        return integer.replace('_', "").parse().ok();
    }
    integer.parse().ok()
}

fn failure(status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({"detail":detail}))).into_response()
}

async fn snapshot(state: &HttpState, user_id: i64, id: &str) -> Response {
    match storage_operation_detail(state.auth.pool(), user_id, id).await {
        Ok(Some(snapshot)) => Json(snapshot).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"detail":"Operation not found"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"detail":"Internal server error"})),
        )
            .into_response(),
    }
}
