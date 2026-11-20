use super::{HttpState, auth, operations, security};
use crate::{docker::DockerClient, jobs::Resource};
use axum::{
    Json,
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::collections::HashMap;

pub(super) async fn start(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    execute(state, id, None, request).await
}

pub(super) async fn stop(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
) -> Response {
    let timeout = match query.get("timeout") {
        None => Ok(30),
        Some(value) => match operations::query_integer(value) {
            Some(value @ 1..=300) => Ok(value as i32),
            Some(value) if value < 1 => Err("Input should be greater than or equal to 1"),
            Some(_) => Err("Input should be less than or equal to 300"),
            None => Err("Input should be a valid integer, unable to parse string as an integer"),
        },
    };
    match timeout {
        Ok(timeout) => execute(state, id, Some(timeout), request).await,
        Err(message) => *auth::validation(vec![json!({"field":"query.timeout","message":message})]),
    }
}

type Failure = (StatusCode, &'static str);

async fn execute(state: HttpState, id: String, timeout: Option<i32>, request: Request) -> Response {
    let (rate_key, status, failure) = if timeout.is_some() {
        (
            "docker-stop",
            "stopped",
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to stop container",
            ),
        )
    } else {
        (
            "docker-start",
            "started",
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to start container",
            ),
        )
    };
    if let Err(response) = security::rate_limit(&state, &request, rate_key) {
        return *response;
    }
    let ctx = state.clone();
    let work = state.probe_jobs.submit_async(move |cancel| async move {
        let docker = DockerClient::connect_with_timeout(&ctx.config.docker_socket, timeout.map_or(60, |t| 60_u64.max(t as u64 + 15))).await.map_err(|_|failure)?;
        let result = async {
            let identity = docker.identity(&id).await.map_err(|_|failure)?;
            let resource = Resource::container(identity.clone()).map_err(|_|failure)?;
            let _claim = ctx.backup_claims.acquire([resource], &cancel).await.map_err(|_|failure)?;
            crate::recovery_activation::require_recovery_ready(ctx.auth.pool()).await.map_err(|_|(StatusCode::SERVICE_UNAVAILABLE, "Recovery activation is incomplete"))?;
            let busy = sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM workload_operations w WHERE w.state!='finished' AND EXISTS(SELECT 1 FROM json_each(w.resources) WHERE key=?))").bind(&identity).fetch_one(ctx.auth.pool()).await.map_err(|_|failure)?;
            if busy { return Err((StatusCode::CONFLICT, "Workload recovery or backup is still active")); }
            if cancel.is_cancelled() || ctx.draining.is_cancelled() { return Err(failure); }
            match timeout {
                Some(timeout) => docker.stop_with_timeout(&identity,timeout).await,
                None => docker.start(&identity).await,
            }.map_err(|_|failure)
        }.await;
        docker.shutdown(None).await;
        result
    });
    let result = match work {
        Ok(work) => work.wait().await.unwrap_or(Err(failure)),
        Err(_) => Err(failure),
    };
    match result {
        Ok(()) => Json(json!({"status":status})).into_response(),
        Err((code, detail)) => error((code, detail)),
    }
}

fn error((code, detail): Failure) -> Response {
    (code, Json(json!({"detail":detail}))).into_response()
}
