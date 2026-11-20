use super::HttpState;
use crate::docker::DockerClient;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

pub(super) async fn containers(
    State(state): State<HttpState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let all = match query
        .get("all")
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        None | Some("1" | "on" | "true" | "t" | "yes" | "y") => true,
        Some("0" | "off" | "false" | "f" | "no" | "n") => false,
        Some(_) => {
            return *super::auth::validation(vec![
                json!({"field":"query.all","message":"Input should be a valid boolean, unable to interpret input"}),
            ]);
        }
    };
    match read_containers(state, all).await {
        Some(items) => Json(items).into_response(),
        None => internal(),
    }
}

pub(super) async fn container(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    match read_containers(state, true).await {
        Some(items) => match items
            .into_iter()
            .find(|item| item["id"] == id || item["name"] == id)
        {
            Some(item) => Json(item).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({"detail":"Container not found"})),
            )
                .into_response(),
        },
        None => internal(),
    }
}

async fn read_containers(state: HttpState, all: bool) -> Option<Vec<Value>> {
    let socket = state.config.docker_socket.clone();
    let result = state.probe_jobs.submit_async(move |_| async move {
        let docker = DockerClient::connect(&socket).await?;
        let result = docker.container_inventory(all).await;
        docker.shutdown(None).await;
        result
    });
    if let Ok(work) = result
        && let Ok(Ok(items)) = work.wait().await
    {
        return Some(items);
    }
    None
}

fn internal() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"detail":"Internal server error"})),
    )
        .into_response()
}

pub(super) async fn volumes(State(state): State<HttpState>) -> impl IntoResponse {
    let socket = state.config.docker_socket.clone();
    let result = state.probe_jobs.submit_async(move |_| async move {
        let docker = DockerClient::connect(&socket).await?;
        let result = docker.volume_inventory().await;
        docker.shutdown(None).await;
        result
    });
    if let Ok(work) = result
        && let Ok(Ok(volumes)) = work.wait().await
    {
        return Json(volumes).into_response();
    }
    internal()
}

pub(super) async fn health(State(state): State<HttpState>) -> impl IntoResponse {
    let socket = state.config.docker_socket.clone();
    let result = state.probe_jobs.submit_async(move |_| async move {
        let docker = DockerClient::connect(&socket).await?;
        let result = docker.ping().await;
        docker.shutdown(None).await;
        result
    });
    if let Ok(work) = result
        && matches!(work.wait().await, Ok(Ok(())))
    {
        return Json(json!({"status":"healthy"})).into_response();
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"detail":"Docker is not accessible"})),
    )
        .into_response()
}
