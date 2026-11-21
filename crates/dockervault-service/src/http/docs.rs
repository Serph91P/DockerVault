use axum::{Router, http::header, response::Html, routing::get};

use super::HttpState;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route(
            "/openapi.json",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "application/json"),
                        (header::CACHE_CONTROL, "no-store"),
                    ],
                    include_str!("docs/openapi.json"),
                )
            }),
        )
        .route(
            "/docs",
            get(|| async {
                (
                    [(header::CACHE_CONTROL, "no-store")],
                    Html(include_str!("docs/swagger.html")),
                )
            }),
        )
        .route(
            "/redoc",
            get(|| async {
                (
                    [(header::CACHE_CONTROL, "no-store")],
                    Html(include_str!("docs/redoc.html")),
                )
            }),
        )
}
