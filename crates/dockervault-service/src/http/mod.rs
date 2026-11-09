//! HTTP contracts; protocol state is shared, never retained in request-owned jobs.
mod auth;
pub mod security;
pub mod ws;
use crate::{auth::AuthService, config::Config};
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
}
impl HttpState {
    pub fn new(auth: AuthService, config: Config) -> Self {
        Self {
            auth,
            config: Arc::new(config),
            limits: Arc::new(security::RateLimits::default()),
            notifications: Arc::new(ws::Notifications::default()),
        }
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
