use super::{
    HttpState,
    auth::{json_body, required_user, validation},
};
use crate::storage_recovery::{
    DeleteOutcome, LocalSelection, StorageRecoveryError, delete_request_id,
};
use axum::{
    Json,
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::collections::HashMap;

pub(super) async fn single(
    State(state): State<HttpState>,
    storage: Option<Path<String>>,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
) -> Response {
    let user = match required_user(&state, request.headers()).await {
        Ok(user) => user,
        Err(response) => return *response,
    };
    let Some(path) = query.get("path") else {
        return invalid("query.path", "Field required");
    };
    let remote_id = match configured_id(storage) {
        Ok(id) => id,
        Err(response) => return *response,
    };
    match run(&state, user.id, remote_id, std::slice::from_ref(path)).await {
        Ok(results) => match results[0] {
            (DeleteOutcome::Succeeded, Some(kind)) => {
                Json(json!({"message":"Deleted target","kind":kind})).into_response()
            }
            (outcome, _) => {
                let (status, message) = failure(outcome);
                detail(status, message)
            }
        },
        Err(response) => *response,
    }
}

pub(super) async fn bulk(
    State(state): State<HttpState>,
    storage: Option<Path<String>>,
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
    let Some(raw) = body.get("paths") else {
        return invalid("body.paths", "Field required");
    };
    let Some(raw) = raw.as_array() else {
        return invalid("body.paths", "Input should be a valid list");
    };
    // Bound the journal and its ordered response buffer like other storage deletes.
    if raw.len() > 1000 {
        return invalid("body.paths", "Select at most 1000 paths");
    }
    let mut paths = Vec::with_capacity(raw.len());
    for (index, path) in raw.iter().enumerate() {
        let Some(path) = path.as_str() else {
            return invalid(
                &format!("body.paths.{index}"),
                "Input should be a valid string",
            );
        };
        paths.push(path.to_owned());
    }
    let remote_id = match configured_id(storage) {
        Ok(id) => id,
        Err(response) => return *response,
    };
    let outcomes = match run(&state, user.id, remote_id, &paths).await {
        Ok(outcomes) => outcomes,
        Err(response) => return *response,
    };
    let mut succeeded = 0;
    let results: Vec<Value> = paths
        .into_iter()
        .zip(outcomes)
        .map(|(path, (outcome, kind))| {
            let success = matches!(outcome, DeleteOutcome::Succeeded);
            succeeded += usize::from(success);
            let error = (!success).then(|| failure(outcome).1);
            json!({"path":path,"success":success,"kind":kind,"error":error})
        })
        .collect();
    Json(json!({"results":results,"succeeded":succeeded,"failed":results.len()-succeeded}))
        .into_response()
}

fn configured_id(storage: Option<Path<String>>) -> Result<Option<i64>, Box<Response>> {
    storage
        .map(|Path(id)| {
            id.parse().map_err(|_| {
                Box::new(invalid(
                    "path.storage_id",
                    "Input should be a valid integer",
                ))
            })
        })
        .transpose()
}

async fn run(
    state: &HttpState,
    user_id: i64,
    remote_id: Option<i64>,
    paths: &[String],
) -> Result<Vec<(DeleteOutcome, Option<&'static str>)>, Box<Response>> {
    if paths.is_empty() {
        if let Some(id) = remote_id {
            let storage = sqlx::query_as::<_, (String, Option<bool>)>(
                "SELECT storage_type, enabled FROM remote_storages WHERE id=?",
            )
            .bind(id)
            .fetch_optional(state.auth.pool())
            .await
            .map_err(|error| admission_error(StorageRecoveryError::Database(error)))?;
            match storage {
                None => return Err(admission_error(StorageRecoveryError::StorageNotFound)),
                Some((_, None | Some(false))) => {
                    return Err(admission_error(StorageRecoveryError::StorageDisabled));
                }
                Some((kind, _)) if kind != "local" => {
                    return Err(admission_error(StorageRecoveryError::UnsupportedProvider));
                }
                _ => {}
            }
        }
        return Ok(Vec::new());
    }
    let request_id = delete_request_id().map_err(admission_error)?;
    let (send, mut receive) = tokio::sync::mpsc::channel(paths.len());
    let notifications = state.notifications.clone();
    state
        .local_delete
        .submit(
            state.auth.pool().clone(),
            state.config.clone(),
            LocalSelection {
                user_id,
                request_id,
                path: String::new(),
                paths: paths.to_vec(),
                remote_id,
                parent: None,
            },
            state.draining.clone(),
            Some(send),
            move |user, id| {
                notifications
                    .send_to_user(user, &json!({"type":"operation_update","operation_id":id}))
            },
        )
        .await
        .map_err(admission_error)?;
    let mut outcomes = Vec::with_capacity(paths.len());
    for _ in paths {
        let result = receive
            .recv()
            .await
            .ok_or_else(|| Box::new(detail(StatusCode::INTERNAL_SERVER_ERROR, "Delete failed")))?;
        outcomes.push(result);
    }
    Ok(outcomes)
}

fn admission_error(error: StorageRecoveryError) -> Box<Response> {
    use StorageRecoveryError as Error;
    let (status, message) = match error {
        Error::StorageNotFound => (StatusCode::NOT_FOUND, "Storage not found"),
        Error::StorageDisabled => (StatusCode::CONFLICT, "Storage is disabled"),
        Error::UnsupportedProvider => (
            StatusCode::NOT_IMPLEMENTED,
            "Browser deletion is available only for LOCAL storage",
        ),
        Error::QueueFull | Error::ShuttingDown | Error::RecoveryPending => {
            (StatusCode::SERVICE_UNAVAILABLE, "Delete unavailable")
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "Delete failed"),
    };
    Box::new(detail(status, message))
}

fn failure(outcome: DeleteOutcome) -> (StatusCode, &'static str) {
    match outcome {
        DeleteOutcome::InvalidPath => (StatusCode::BAD_REQUEST, "Invalid file path"),
        DeleteOutcome::NotFound => (StatusCode::NOT_FOUND, "Target not found"),
        DeleteOutcome::PermissionDenied => (StatusCode::FORBIDDEN, "Permission denied"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "Delete failed"),
    }
}

fn invalid(field: &str, message: &str) -> Response {
    *validation(vec![json!({"field":field,"message":message})])
}

fn detail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"detail":message}))).into_response()
}
