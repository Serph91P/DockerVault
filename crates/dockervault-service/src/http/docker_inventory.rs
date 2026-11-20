use super::HttpState;
use crate::docker::DockerClient;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::json;

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
