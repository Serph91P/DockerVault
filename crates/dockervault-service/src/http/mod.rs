//! HTTP contracts; protocol state is shared, never retained in request-owned jobs.
mod auth;
mod operations;
pub mod security;
mod storage;
pub mod ws;
use crate::{auth::AuthService, config::Config, storage_recovery::LocalDeleteJobs};
use axum::{
    Json, Router, middleware,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::json;
use std::sync::Arc;
#[derive(Clone)]
pub struct HttpState {
    pub auth: AuthService,
    pub config: Arc<Config>,
    limits: Arc<security::RateLimits>,
    pub notifications: Arc<ws::Notifications>,
    draining: tokio_util::sync::CancellationToken,
    local_delete: Arc<LocalDeleteJobs>,
    probe_processes: Option<crate::process::ProcessOwner>,
    probe_jobs: crate::ownership::Owner,
    download_slots: Arc<tokio::sync::Semaphore>,
}
impl HttpState {
    pub fn new(auth: AuthService, config: Config) -> Self {
        Self {
            auth,
            config: Arc::new(config),
            limits: Arc::new(security::RateLimits::default()),
            notifications: Arc::new(ws::Notifications::default()),
            draining: tokio_util::sync::CancellationToken::new(),
            local_delete: Arc::new(LocalDeleteJobs::new()),
            probe_processes: crate::process::ProcessOwner::new(
                4,
                "/usr/local/libexec/dockervault-process-guard",
            )
            .ok(),
            probe_jobs: crate::ownership::Owner::new(4).expect("positive probe capacity"),
            download_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }

    pub fn begin_shutdown(&self) {
        self.draining.cancel();
    }

    pub async fn settle_storage(
        &self,
    ) -> Result<(), crate::storage_recovery::StorageRecoveryError> {
        self.begin_shutdown();
        self.local_delete.shutdown().await;
        self.probe_jobs.shutdown(None).await;
        if let Some(processes) = &self.probe_processes {
            processes.shutdown(None).await;
        }
        crate::storage_recovery::settle_interrupted(self.auth.pool()).await?;
        Ok(())
    }
}
pub fn router(state: HttpState) -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"status":"healthy"})) }),
        )
        .route("/api/v1/auth/status", get(auth::status))
        .route("/api/v1/auth/setup", post(auth::setup))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/me", get(auth::me))
        .route("/api/v1/auth/change-password", post(auth::change_password))
        .route(
            "/api/v1/operations",
            get(operations::list).post(operations::submit),
        )
        .route("/api/v1/operations/{id}", get(operations::detail))
        .route("/api/v1/operations/{id}/cancel", post(operations::cancel))
        .route("/api/v1/operations/{id}/retry", post(operations::retry))
        .route("/api/v1/storage/types", get(storage::types))
        .route(
            "/api/v1/storage/local/files",
            get(storage::list_local_files),
        )
        .route("/api/v1/storage/local/files/size", get(storage::local_size))
        .route(
            "/api/v1/storage/local/files/download",
            get(storage::local_download),
        )
        .route(
            "/api/v1/storage/{storage_id}/files",
            get(storage::list_configured_files),
        )
        .route(
            "/api/v1/storage/{storage_id}/files/size",
            get(storage::configured_size),
        )
        .route("/api/v1/storage/test-config", post(storage::test_config))
        .route(
            "/api/v1/storage/{storage_id}/test",
            post(storage::test_saved),
        )
        .route("/api/v1/storage", get(storage::list).post(storage::create))
        .route(
            "/api/v1/storage/{storage_id}",
            get(storage::detail)
                .put(storage::update)
                .delete(storage::delete),
        )
        .route("/ws/updates", get(ws::updates))
        .fallback(|| async {
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"detail":"Not Found"})),
            )
                .into_response()
        })
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::guard,
        ))
        .with_state(state)
}
