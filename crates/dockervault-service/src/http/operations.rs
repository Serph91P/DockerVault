use super::{
    HttpState,
    auth::{json_body, required_user, validation},
};
use crate::{
    storage::relative_path,
    storage_recovery::{
        LocalSelection, StorageRecoveryError, storage_operation_detail, storage_operation_list,
    },
};
use axum::{
    Json,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::collections::HashMap;

pub(super) async fn submit(State(state): State<HttpState>, mut request: Request) -> Response {
    let user = match required_user(&state, request.headers()).await {
        Ok(user) => user,
        Err(response) => return *response,
    };
    let body = match json_body(&state, &mut request).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let invalid =
        |field: &str, message: &str| *validation(vec![json!({"field":field,"message":message})]);
    if body["action"] != "storage_delete" {
        return invalid("body.action", "Input should be 'storage_delete'");
    }
    let remote_id = if body["storage_id"] == "local" {
        None
    } else if let Some(id) = body["storage_id"].as_i64().filter(|id| *id > 0) {
        Some(id)
    } else {
        return invalid("body.storage_id", "Expected 'local' or a positive integer");
    };
    let Some(path) = body["path"].as_str() else {
        return invalid("body.path", "Input should be a valid string");
    };
    if path.chars().count() > 1024 {
        return invalid("body.path", "String should have at most 1024 characters");
    }
    if relative_path(path, true).is_none() {
        return failure(StatusCode::UNPROCESSABLE_ENTITY, "Invalid browser path");
    }
    let Some(paths) = body["paths"].as_array() else {
        return invalid("body.paths", "Input should be a valid list");
    };
    // Bound the wire list separately; admission limits the normalized unique selection.
    if paths.len() > 10_000 {
        return invalid(
            "body.paths",
            &format!(
                "List should have at most 10000 items after validation, not {}",
                paths.len()
            ),
        );
    }
    let mut targets = Vec::with_capacity(paths.len());
    for (index, item) in paths.iter().enumerate() {
        let Some(path) = item.as_str() else {
            return invalid(
                &format!("body.paths.{index}"),
                "Input should be a valid string",
            );
        };
        if path.chars().count() > 1024 {
            return invalid(
                &format!("body.paths.{index}"),
                "String should have at most 1024 characters",
            );
        }
        targets.push(path.to_owned());
    }
    let Some(request_id) = canonical_uuid(&body["request_id"]) else {
        return invalid("body.request_id", "Input should be a valid UUID");
    };
    if let Some(id) = remote_id {
        match crate::storage_recovery::replay_remote_delete(
            state.auth.pool(),
            user.id,
            id,
            &request_id,
            path,
            &targets,
        )
        .await
        {
            Ok(None) => {}
            Ok(Some(receipt)) => return admission_response(Ok(receipt), false),
            Err(error) => return admission_response(Err(error), false),
        }
    }
    admit_local(
        &state,
        LocalSelection {
            user_id: user.id,
            request_id,
            path: path.to_owned(),
            paths: targets,
            remote_id,
            parent: None,
        },
    )
    .await
}

pub(super) async fn retry(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    mut request: Request,
) -> Response {
    let user = match required_user(&state, request.headers()).await {
        Ok(user) => user,
        Err(response) => return *response,
    };
    let body = match json_body(&state, &mut request).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let Some(request_id) = canonical_uuid(&body["request_id"]) else {
        return *validation(vec![
            json!({"field":"body.request_id","message":"Input should be a valid UUID"}),
        ]);
    };
    match LocalSelection::retry(state.auth.pool(), user.id, id, request_id).await {
        Ok(selection) => admit_local(&state, selection).await,
        Err(error) => admission_response(Err(error), true),
    }
}

async fn admit_local(state: &HttpState, selection: LocalSelection) -> Response {
    let retry = selection.parent.is_some();
    let notifications = state.notifications.clone();
    let result = state
        .local_delete
        .submit(
            state.auth.pool().clone(),
            state.config.clone(),
            selection,
            state.draining.clone(),
            move |user, id| {
                notifications
                    .send_to_user(user, &json!({"type":"operation_update","operation_id":id}))
            },
        )
        .await;
    admission_response(result, retry)
}

fn admission_response(result: Result<String, StorageRecoveryError>, retry: bool) -> Response {
    use StorageRecoveryError as Error;
    match result {
        Ok(id) => (StatusCode::ACCEPTED, Json(json!({"operation_id":id}))).into_response(),
        Err(Error::InvalidRequest) => failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Select between 1 and 1000 nonempty paths",
        ),
        Err(Error::RequestConflict) => failure(
            StatusCode::CONFLICT,
            "Request ID already used for another selection",
        ),
        Err(Error::IdentityChanged) => failure(
            StatusCode::CONFLICT,
            if retry {
                "Storage identity changed; inspect before starting a new operation"
            } else {
                "Storage root is unavailable"
            },
        ),
        Err(Error::OperationNotFound) => failure(StatusCode::NOT_FOUND, "Operation not found"),
        Err(Error::StorageNotFound) => failure(StatusCode::NOT_FOUND, "Storage not found"),
        Err(Error::StorageDisabled) => failure(StatusCode::CONFLICT, "Storage is disabled"),
        Err(Error::InvalidStorageConfiguration) => {
            failure(StatusCode::CONFLICT, "Storage configuration is unavailable")
        }
        Err(Error::OperationActive) => {
            failure(StatusCode::CONFLICT, "Wait for operation settlement")
        }
        Err(Error::NothingToRetry) => failure(
            StatusCode::CONFLICT,
            "No failed items to retry; inspect unknown outcomes manually",
        ),
        Err(Error::UnsupportedProvider) => failure(
            StatusCode::NOT_IMPLEMENTED,
            "Remote storage operations are unavailable",
        ),
        Err(Error::ShuttingDown) => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "Storage worker is shutting down",
        ),
        Err(Error::QueueFull) => failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Storage operation queue is full",
        ),
        Err(Error::RecoveryPending) => {
            failure(StatusCode::CONFLICT, "System recovery setup is in progress")
        }
        Err(_) => failure(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
}

fn canonical_uuid(value: &Value) -> Option<String> {
    let value = value.as_str()?;
    let value = value.strip_prefix("urn:uuid:").unwrap_or(value);
    let value = value
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(value);
    let hex = if value.len() == 36 {
        if ![8, 13, 18, 23]
            .into_iter()
            .all(|position| value.as_bytes()[position] == b'-')
        {
            return None;
        }
        value.replace('-', "")
    } else {
        value.to_owned()
    };
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let hex = hex.to_ascii_lowercase();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

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
