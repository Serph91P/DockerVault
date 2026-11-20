use super::{HttpState, auth, schedules, targets};
use axum::{
    Json,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dockervault_core::cron::Schedule;
use serde_json::{Map, Value, json};
use sqlx::Sqlite;

pub(super) async fn create(State(state): State<HttpState>, request: Request) -> Response {
    write(state, None, request).await
}

pub(super) async fn update(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let Some(id) = super::operations::query_integer(&id) else {
        return *auth::validation(vec![
            json!({"field":"path.target_id","message":"Input should be a valid integer, unable to parse string as an integer"}),
        ]);
    };
    write(state, Some(id), request).await
}

fn detail(status: StatusCode, value: Value) -> Response {
    (status, Json(json!({"detail":value}))).into_response()
}
fn invalid(field: &str, message: &str) -> Response {
    *auth::validation(vec![json!({"field":field,"message":message})])
}

const LISTS: [&str; 4] = [
    "dependencies",
    "selected_volumes",
    "include_paths",
    "exclude_paths",
];
const BOOLS: [(&str, bool); 4] = [
    ("enabled", true),
    ("stop_container", true),
    ("compression_enabled", true),
    ("delete_local_after_sync", false),
];
const SOURCE: [&str; 4] = ["container_name", "volume_name", "host_path", "stack_name"];
const NULLABLE: [&str; 5] = [
    "schedule_id",
    "retention_policy_id",
    "schedule_cron",
    "pre_backup_command",
    "post_backup_command",
];

fn parse(input: &Value, create: bool) -> Result<Map<String, Value>, (String, String)> {
    let mut out = Map::new();
    let Some(input) = input.as_object() else {
        return Err(("body".into(), "Input should be a valid dictionary".into()));
    };
    let err = |key: &str, message: &str| (format!("body.{key}"), message.to_owned());
    let mut keys = vec!["name", "remote_storage_ids", "per_volume_rules"];
    keys.extend(LISTS);
    keys.extend(BOOLS.map(|(key, _)| key));
    keys.extend(NULLABLE);
    if create {
        keys.extend(SOURCE);
        keys.push("target_type");
    }
    for key in keys {
        let value = if let Some(value) = input.get(key) {
            value.clone()
        } else if !create {
            continue;
        } else if let Some((_, default)) = BOOLS.iter().find(|(k, _)| *k == key) {
            json!(default)
        } else if LISTS.contains(&key) || key == "remote_storage_ids" {
            json!([])
        } else if key == "per_volume_rules" {
            json!({})
        } else if matches!(key, "name" | "target_type") {
            return Err(err(key, "Field required"));
        } else {
            Value::Null
        };
        if value.is_null() {
            if NULLABLE.contains(&key) || SOURCE.contains(&key) {
                out.insert(key.into(), value);
                continue;
            }
            if !create {
                return Err((
                    "body".into(),
                    format!("Value error, {key} cannot be null; omit it to keep the current value"),
                ));
            }
        }
        let parsed = match key {
            "schedule_id" | "retention_policy_id" => {
                json!(integer(&value).ok_or_else(|| err(key, "Input should be a valid integer"))?)
            }
            "schedule_cron" => {
                let raw = value
                    .as_str()
                    .ok_or_else(|| err(key, "Input should be a valid string"))?
                    .trim();
                if raw.is_empty() {
                    Value::Null
                } else {
                    Schedule::parse(raw).map_err(|e| err(key, &format!("Value error, {e}")))?;
                    json!(raw)
                }
            }
            "target_type"
                if !matches!(
                    value.as_str(),
                    Some("container" | "volume" | "path" | "stack")
                ) =>
            {
                return Err(err(
                    key,
                    "Input should be 'container', 'volume', 'path' or 'stack'",
                ));
            }
            "name" => {
                let name = value
                    .as_str()
                    .ok_or_else(|| err(key, "Input should be a valid string"))?;
                match name.chars().take(256).count() {
                    0 => return Err(err(key, "String should have at least 1 character")),
                    256 => return Err(err(key, "String should have at most 255 characters")),
                    _ => value,
                }
            }
            "pre_backup_command" | "post_backup_command" => {
                let text = value
                    .as_str()
                    .ok_or_else(|| err(key, "Input should be a valid string"))?;
                if !text.trim().is_empty() {
                    return Err(err(
                        key,
                        "Hook commands are not yet supported by the native candidate",
                    ));
                }
                Value::Null
            }
            "remote_storage_ids" => json!(
                value
                    .as_array()
                    .ok_or_else(|| err(key, "Input should be a valid list"))?
                    .iter()
                    .map(integer)
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| err(key, "Input should contain valid integers"))?
            ),
            "per_volume_rules" => {
                let rules = value
                    .as_object()
                    .ok_or_else(|| err(key, "Input should be a valid dictionary"))?;
                if !rules
                    .values()
                    .all(|v| v.as_object().is_some_and(|m| m.values().all(strings)))
                {
                    return Err(err(key, "Input should map volumes to path rules"));
                }
                value
            }
            key if LISTS.contains(&key) => {
                if !value.is_array() {
                    return Err(err(key, "Input should be a valid list"));
                }
                if !strings(&value) {
                    return Err(err(key, "Input should be a list of strings"));
                }
                value
            }
            key if BOOLS.iter().any(|(k, _)| *k == key) => {
                let b = value
                    .as_bool()
                    .or_else(|| match value.as_f64() {
                        Some(0.0) => Some(false),
                        Some(1.0) => Some(true),
                        _ => None,
                    })
                    .or_else(
                        || match value.as_str().map(str::to_ascii_lowercase).as_deref() {
                            Some("0" | "off" | "f" | "false" | "n" | "no") => Some(false),
                            Some("1" | "on" | "t" | "true" | "y" | "yes") => Some(true),
                            _ => None,
                        },
                    )
                    .ok_or_else(|| err(key, "Input should be a valid boolean"))?;
                json!(b)
            }
            _ => {
                if !value.is_string() {
                    return Err(err(key, "Input should be a valid string"));
                }
                value
            }
        };
        out.insert(key.into(), parsed);
    }
    Ok(out)
}

fn integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| {
            value
                .as_f64()
                .filter(|v| v.fract() == 0.0 && *v >= i64::MIN as f64 && *v < -(i64::MIN as f64))
                .map(|v| v as i64)
        })
        .or_else(|| value.as_str().and_then(super::operations::query_integer))
        .or_else(|| value.as_bool().map(i64::from))
}
fn strings(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|xs| xs.iter().all(Value::is_string))
}

async fn write(state: HttpState, id: Option<i64>, mut request: Request) -> Response {
    let input = match auth::json_body(&state, &mut request).await {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let fields = match parse(&input, id.is_none()) {
        Ok(v) => v,
        Err((field, msg)) => return invalid(&field, &msg),
    };
    if id.is_none() {
        let kind = fields["target_type"].as_str().expect("validated type");
        let required = match kind {
            "container" => "container_name",
            "stack" => "stack_name",
            "volume" => "volume_name",
            _ => "host_path",
        };
        let Some(src) = fields[required].as_str().filter(|s| !s.is_empty()) else {
            return detail(
                StatusCode::BAD_REQUEST,
                json!(format!("{required} required for {kind} type")),
            );
        };
        if kind == "path" {
            let path = src.to_owned();
            let Ok(w) = state
                .auth
                .blocking()
                .submit_blocking(move |_| std::path::Path::new(&path).exists())
            else {
                return detail(
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!("Source validation capacity exhausted"),
                );
            };
            if !matches!(w.wait().await, Ok(true)) {
                return detail(
                    StatusCode::BAD_REQUEST,
                    json!(format!("Path '{src}' does not exist")),
                );
            }
        }
    }
    let target = match persist(&state, id, fields).await {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if let Err(error) = schedules::reconcile(&state).await {
        return error.into_response();
    }
    targets::read(&state, false, Some(target)).await
}

enum Failure {
    Database,
    Missing,
    References(Vec<String>),
    MissingSchedule(i64),
}
impl From<sqlx::Error> for Failure {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        match self {
            Self::Database => detail(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!("Could not save plan"),
            ),
            Self::Missing => detail(StatusCode::NOT_FOUND, json!("Target not found")),
            Self::MissingSchedule(id) => detail(
                StatusCode::BAD_REQUEST,
                json!(format!("Schedule with id {id} not found")),
            ),
            Self::References(errors) => detail(
                StatusCode::BAD_REQUEST,
                json!({"message":"Invalid target references","errors":errors}),
            ),
        }
    }
}

async fn persist(
    state: &HttpState,
    id: Option<i64>,
    mut fields: Map<String, Value>,
) -> Result<i64, Failure> {
    let mut tx = state.auth.pool().begin_with("BEGIN IMMEDIATE").await?;
    let (old_schedule, old_policy, old_remote) = if let Some(id) = id {
        sqlx::query_as::<_,(Option<i64>,Option<i64>,Option<String>)>("SELECT schedule_id,retention_policy_id,remote_storage_ids FROM backup_targets WHERE id=? AND removed_at IS NULL")
            .bind(id).fetch_optional(&mut *tx).await?.ok_or(Failure::Missing)?
    } else {
        (None, None, None)
    };
    let schedule = fields
        .get("schedule_id")
        .map(Value::as_i64)
        .unwrap_or(old_schedule);
    let policy = fields
        .get("retention_policy_id")
        .map(Value::as_i64)
        .unwrap_or(old_policy);
    let remote: Vec<i64> = if let Some(value) = fields.get("remote_storage_ids") {
        serde_json::from_value(value.clone()).map_err(|_| Failure::Database)?
    } else if let Some(raw) = old_remote {
        serde_json::from_str::<Option<Vec<i64>>>(&raw)
            .map_err(|_| Failure::Database)?
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut errors = Vec::new();
    if let Some(schedule) = schedule
        && !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM schedules WHERE id=?)")
            .bind(schedule)
            .fetch_one(&mut *tx)
            .await?
    {
        if id.is_some() && fields.contains_key("schedule_id") {
            return Err(Failure::MissingSchedule(schedule));
        }
        errors.push(format!("schedule_id {schedule} does not exist"));
    }
    if let Some(policy) = policy
        && !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM retention_policies WHERE id=?)",
        )
        .bind(policy)
        .fetch_one(&mut *tx)
        .await?
    {
        errors.push(format!("retention_policy_id {policy} does not exist"));
    }
    let known: std::collections::HashSet<_> =
        sqlx::query_scalar::<_, i64>("SELECT id FROM remote_storages")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    errors.extend(
        remote
            .iter()
            .filter(|id| !known.contains(id))
            .map(|id| format!("remote_storage_id {id} does not exist")),
    );
    if !errors.is_empty() {
        return Err(Failure::References(errors));
    }
    if fields.contains_key("remote_storage_ids") {
        fields.insert("sync_to_remote".into(), json!(!remote.is_empty()));
    }
    if remote.is_empty() && fields.contains_key("delete_local_after_sync") {
        fields.insert("delete_local_after_sync".into(), json!(false));
    }
    let target = if let Some(id) = id {
        id
    } else {
        let name = fields.remove("name").expect("validated name");
        let kind = fields.remove("target_type").expect("validated type");
        sqlx::query_scalar::<_,i64>("INSERT INTO backup_targets(name,target_type,created_at,updated_at) VALUES(?,?,strftime('%Y-%m-%d %H:%M:%f000','now'),strftime('%Y-%m-%d %H:%M:%f000','now')) RETURNING id")
            .bind(name.as_str()).bind(kind.as_str()).fetch_one(&mut *tx).await?
    };
    if !fields.is_empty() {
        let mut sql = sqlx::QueryBuilder::<Sqlite>::new(
            "UPDATE backup_targets SET updated_at=strftime('%Y-%m-%d %H:%M:%f000','now')",
        );
        // All column names are taken from the static parse whitelist or the internal sync bit.
        for (key, val) in fields {
            sql.push(", ").push(key).push("=");
            match val {
                Value::Null => {
                    sql.push("NULL");
                }
                Value::String(text) => {
                    sql.push_bind(text);
                }
                Value::Bool(flag) => {
                    sql.push_bind(flag);
                }
                Value::Number(num) => {
                    sql.push_bind(num.as_i64().ok_or(Failure::Database)?);
                }
                value => {
                    sql.push_bind(value.to_string());
                }
            }
        }
        let result = sql
            .push(" WHERE id=")
            .push_bind(target)
            .build()
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() != 1 {
            return Err(Failure::Database);
        }
    }
    tx.commit().await?;
    Ok(target)
}
