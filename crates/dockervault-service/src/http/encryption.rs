use super::HttpState;
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dockervault_core::timestamp::Timestamp;
use serde_json::json;

const DEFERRED: &str = "encryption_onboarding_deferred";

pub(super) async fn status(State(state): State<HttpState>) -> Response {
    match read_status(&state).await {
        Ok(value) => Json(value).into_response(),
        Err(_) => internal(),
    }
}

async fn read_status(state: &HttpState) -> Result<serde_json::Value, sqlx::Error> {
    let mut tx = state.auth.pool().begin().await?;
    let deferred: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM app_settings WHERE key=? AND value='true')",
    )
    .bind(DEFERRED)
    .fetch_one(&mut *tx)
    .await?;
    type Config = (bool, bool, Option<String>, Option<String>);
    let (completed, enabled, key, created) = sqlx::query_as::<_, Config>("SELECT coalesce(setup_completed,0),coalesce(encryption_enabled,0),public_key,key_created_at FROM encryption_config ORDER BY id LIMIT 1")
        .fetch_optional(&mut *tx).await?.unwrap_or_default();
    let created = created
        .as_deref()
        .map(Timestamp::parse)
        .transpose()
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?
        .map(Timestamp::api);
    tx.commit().await?;
    Ok(
        json!({"setup_completed":completed,"encryption_enabled":enabled,"onboarding_deferred":deferred,"public_key":key,"key_created_at":created}),
    )
}

pub(super) async fn defer(State(state): State<HttpState>) -> Response {
    match sqlx::query("INSERT INTO app_settings(key,value,updated_at) VALUES(?,'true',strftime('%Y-%m-%d %H:%M:%f000','now')) ON CONFLICT(key) DO UPDATE SET value='true',updated_at=excluded.updated_at")
        .bind(DEFERRED).execute(state.auth.pool()).await {
        Ok(result) if result.rows_affected() == 1 => Json(json!({"onboarding_deferred":true})).into_response(),
        _ => internal(),
    }
}

fn internal() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"detail":"Internal server error"})),
    )
        .into_response()
}
