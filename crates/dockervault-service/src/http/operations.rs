use super::{HttpState, auth::required_user};
use crate::storage_recovery::{storage_operation_detail, storage_operation_list};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

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

#[derive(serde::Deserialize)]
pub(super) struct ListQuery {
    storage_id: Option<String>,
    limit: Option<String>,
}

pub(super) async fn list(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<axum::extract::Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let user = match required_user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return *response,
    };
    let Ok(axum::extract::Query(query)) = query else {
        return failure(StatusCode::UNPROCESSABLE_ENTITY, "Validation error");
    };
    let limit = query.limit.as_deref().map(str::parse::<i64>).transpose();
    let Ok(limit) = limit else {
        return failure(StatusCode::UNPROCESSABLE_ENTITY, "Validation error");
    };
    let limit = limit.unwrap_or(20);
    let storage = match query.storage_id.as_deref() {
        None => None,
        Some("local") => Some("local".into()),
        Some(value) => match value.parse::<i64>() {
            Ok(value) if value > 0 => Some(value.to_string()),
            _ => return failure(StatusCode::UNPROCESSABLE_ENTITY, "Validation error"),
        },
    };
    if !(1..=100).contains(&limit) {
        return failure(StatusCode::UNPROCESSABLE_ENTITY, "Validation error");
    }
    match storage_operation_list(state.auth.pool(), user.id, storage.as_deref(), limit).await {
        Ok(rows) => Json(rows).into_response(),
        Err(_) => failure(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
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
