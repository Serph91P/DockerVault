use super::*;
use axum::extract::Request;

pub(in crate::http) async fn test_config(
    State(state): State<HttpState>,
    mut request: Request,
) -> Response {
    let bad_shape = || {
        storage_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid storage draft configuration",
        )
    };
    let mut body = match auth::json_body(&state, &mut request).await {
        Ok(body) => body,
        Err(response) if response.status() == StatusCode::UNPROCESSABLE_ENTITY => {
            return bad_shape();
        }
        Err(response) => return *response,
    };
    let fields = body.as_object_mut().expect("validated object");
    let Some(mut effective) = fields.remove("config").filter(Value::is_object) else {
        return bad_shape();
    };
    let errors = match fields.remove("mode").as_ref().and_then(Value::as_str) {
        Some("create") if fields.is_empty() => {
            if !create::validate_create(&mut effective).is_empty() {
                return bad_shape();
            }
            create::create_storage_errors(&effective)
        }
        Some("edit") if fields.len() == 1 => {
            let Some(id) = fields
                .get("storage_id")
                .and_then(draft_id)
                .filter(|id| *id > 0)
            else {
                return bad_shape();
            };
            let updates = effective.as_object_mut().expect("validated object");
            if !create::validate_update(updates, true).is_empty() {
                return bad_shape();
            }
            let row = sqlx::query_scalar::<_, String>(select_storage!(" WHERE id=?"))
                .bind(id)
                .fetch_optional(state.auth.pool())
                .await;
            let db_error =
                || storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
            let raw = match row {
                Ok(Some(row)) => row,
                Ok(None) => return storage_error(StatusCode::NOT_FOUND, "Storage not found"),
                Err(_) => return db_error(),
            };
            let Ok(mut saved) = serde_json::from_str::<serde_json::Map<String, Value>>(&raw) else {
                return db_error();
            };
            // Only inherited values are ciphertext. Draft replacements are literal secrets.
            if !open_probe_credentials(&state, &mut saved, Some(updates)).await {
                return probe_response(false);
            }
            saved.append(updates);
            effective = Value::Object(saved);
            storage_errors(&effective)
        }
        _ => return bad_shape(),
    };
    if !errors.is_empty() {
        return invalid_storage(errors);
    }
    probe_provider(&state, effective).await
}

fn draft_id(value: &Value) -> Option<i64> {
    match value {
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::String(text) => super::super::operations::query_integer(text),
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|value| value.fract() == 0.0 && value.abs() < i64::MAX as f64)
                .map(|value| value as i64)
        }),
        _ => None,
    }
}
