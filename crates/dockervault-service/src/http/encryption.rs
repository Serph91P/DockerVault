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

fn boolean(value: &serde_json::Value) -> Option<bool> {
    value.as_bool().or_else(
        || match value.as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("true" | "1" | "yes" | "on" | "t" | "y") => Some(true),
            Some("false" | "0" | "no" | "off" | "f" | "n") => Some(false),
            _ => match value.as_f64() {
                Some(0.0) => Some(false),
                Some(1.0) => Some(true),
                _ => None,
            },
        },
    )
}

pub(super) async fn confirm(
    State(state): State<HttpState>,
    mut request: axum::extract::Request,
) -> Response {
    let body = match super::auth::json_body(&state, &mut request).await {
        Ok(body) => body,
        Err(err) => return *err,
    };
    match body.get("confirmed").and_then(boolean) {
        Some(true) => {}
        Some(false) => return bad("You must confirm that you have saved your private key."),
        None => {
            return *super::auth::validation(vec![
                json!({"field":"body.confirmed","message":if body.get("confirmed").is_some(){"Input should be a valid boolean"}else{"Field required"}}),
            ]);
        }
    }
    match sqlx::query("UPDATE encryption_config SET setup_completed=1 WHERE id=(SELECT id FROM encryption_config ORDER BY id LIMIT 1) AND public_key!=''").execute(state.auth.pool()).await {
        Ok(result) if result.rows_affected()==1 => Json(json!({"message":"Encryption setup completed. Your backups will now be encrypted."})).into_response(),
        Ok(_) => bad("Encryption not set up. Call /setup first."),
        Err(_) => internal(),
    }
}

pub(super) async fn toggle(
    State(state): State<HttpState>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(enabled) = query
        .get("enabled")
        .and_then(|value| boolean(&json!(value)))
    else {
        return *super::auth::validation(vec![
            json!({"field":"query.enabled","message":if query.contains_key("enabled"){ "Input should be a valid boolean, unable to interpret input" }else{"Field required"}}),
        ]);
    };
    match sqlx::query("UPDATE encryption_config SET encryption_enabled=? WHERE id=(SELECT id FROM encryption_config ORDER BY id LIMIT 1) AND setup_completed=1").bind(enabled).execute(state.auth.pool()).await {
        Ok(result) if result.rows_affected()==1 => {
            let status = if enabled {"enabled"}else{"disabled"};
            Json(json!({"message":format!("Encryption {status} for new backups.")})).into_response()
        }
        Ok(_) => bad("Encryption must be set up before it can be toggled."),
        Err(_) => internal(),
    }
}

fn bad(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"detail":message}))).into_response()
}

fn internal() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"detail":"Internal server error"})),
    )
        .into_response()
}
