use super::*;
use axum::extract::Request;

const SECRETS: [&str; 3] = ["password", "s3_access_key", "s3_secret_key"];
const OPTIONAL: [&str; 11] = [
    "host",
    "username",
    "password",
    "ssh_key_path",
    "s3_bucket",
    "s3_region",
    "s3_access_key",
    "s3_secret_key",
    "s3_endpoint_url",
    "webdav_url",
    "rclone_remote",
];

pub(in crate::http) async fn create(
    State(state): State<HttpState>,
    mut request: Request,
) -> Response {
    let mut data = match auth::json_body(&state, &mut request).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let body = data.as_object_mut().expect("validated object");
    body.entry("enabled").or_insert(json!(true));
    body.entry("base_path").or_insert(json!("/backups"));
    coerce_storage_scalars(body);
    let mut errors = Vec::new();
    let mut invalid = |field: &str, message: &str| {
        errors.push(json!({"field":format!("body.{field}"),"message":message}))
    };
    for field in ["name", "storage_type", "base_path"] {
        match body.get(field) {
            None => invalid(field, "Field required"),
            Some(v) if !v.is_string() => invalid(field, "Input should be a valid string"),
            _ => {}
        }
    }
    if let Some(name) = body.get("name").and_then(Value::as_str) {
        if name.is_empty() {
            invalid("name", "String should have at least 1 character");
        }
        if name.chars().count() > 255 {
            invalid("name", "String should have at most 255 characters");
        }
    }
    if let Some(kind) = body.get("storage_type").and_then(Value::as_str)
        && !matches!(
            kind,
            "local" | "ssh" | "sftp" | "webdav" | "s3" | "ftp" | "rclone"
        )
    {
        invalid(
            "storage_type",
            "Input should be 'local', 'ssh', 'sftp', 'webdav', 's3', 'ftp' or 'rclone'",
        );
    }
    if !body["enabled"].is_boolean() {
        invalid("enabled", "Input should be a valid boolean");
    }
    if let Some(port) = body.get("port").filter(|value| !value.is_null()) {
        match port.as_i64() {
            None => invalid("port", "Input should be a valid integer"),
            Some(port) if port < 1 => invalid("port", "Input should be greater than or equal to 1"),
            Some(port) if port > 65535 => {
                invalid("port", "Input should be less than or equal to 65535")
            }
            _ => {}
        }
    }
    for field in OPTIONAL {
        if body
            .get(field)
            .is_some_and(|v| !v.is_null() && !v.is_string())
        {
            invalid(field, "Input should be a valid string");
        }
    }
    if !errors.is_empty() {
        return *auth::validation(errors);
    }
    let mut errors = storage_errors(&data);
    if let Some(kind) = data["storage_type"].as_str() {
        let marker = format!(" is required for {kind};");
        let enum_marker = format!(
            " is required for StorageType.{};",
            kind.to_ascii_uppercase()
        );
        for error in &mut errors {
            *error = error.replace(&marker, &enum_marker);
        }
    }
    if !errors.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"detail":{"message":"Invalid storage configuration","errors":errors}})),
        )
            .into_response();
    }
    persist_create(&state, data).await
}

pub(in crate::http) async fn update(
    State(state): State<HttpState>,
    Path(raw): Path<String>,
    mut request: Request,
) -> Response {
    let Some(id) = super::super::operations::query_integer(&raw) else {
        return invalid_id();
    };
    let mut data = match auth::json_body(&state, &mut request).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let values = data.as_object_mut().expect("validated object");
    coerce_storage_scalars(values);
    values.retain(|key, value| {
        (OPTIONAL.contains(&key.as_str())
            || matches!(key.as_str(), "name" | "base_path" | "enabled" | "port"))
            && !(SECRETS.contains(&key.as_str()) && value.as_str() == Some(""))
    });
    let mut errors = Vec::new();
    for (field, value) in values.iter() {
        let invalid = match field.as_str() {
            "enabled" if !value.is_null() && !value.is_boolean() => {
                Some("Input should be a valid boolean")
            }
            "port" if !value.is_null() => match value.as_i64() {
                None => Some("Input should be a valid integer"),
                Some(v) if v < 1 => Some("Input should be greater than or equal to 1"),
                Some(v) if v > 65535 => Some("Input should be less than or equal to 65535"),
                _ => None,
            },
            "port" | "enabled" => None,
            "name" if value.as_str() == Some("") => Some("String should have at least 1 character"),
            "name" if value.as_str().is_some_and(|v| v.chars().count() > 255) => {
                Some("String should have at most 255 characters")
            }
            _ if !value.is_null() && !value.is_string() => Some("Input should be a valid string"),
            _ => None,
        };
        if let Some(message) = invalid {
            errors.push(json!({"field":format!("body.{field}"),"message":message}));
        }
    }
    if !errors.is_empty() {
        return *auth::validation(errors);
    }
    match persist_update(&state, id, data).await {
        Ok(response) => response,
        Err(_) => storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
}

async fn persist_update(state: &HttpState, id: i64, data: Value) -> Result<Response, sqlx::Error> {
    let mut tx = state.auth.pool().begin_with("BEGIN IMMEDIATE").await?;
    let Some(row) = sqlx::query_scalar::<_, String>(select_storage!(" WHERE id=?"))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(storage_error(StatusCode::NOT_FOUND, "Storage not found"));
    };
    let original: serde_json::Map<String, Value> = serde_json::from_str(&row)
        .map_err(|_| sqlx::Error::Protocol("Invalid storage record".into()))?;
    let mut effective = original.clone();
    for (key, value) in data.as_object().expect("validated object") {
        effective.insert(key.clone(), value.clone());
    }
    let errors = storage_errors(&Value::Object(effective.clone()));
    if !errors.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({"detail":{"message":"Invalid storage configuration","errors":errors}})),
        )
            .into_response());
    }
    let encoded = match prepare(state, data, &mut tx).await {
        Ok(value) => value,
        Err(response) => return Ok(*response),
    };
    let mut changed = false;
    for (key, value) in encoded.as_object().expect("validated object") {
        changed |= original.get(key) != Some(value);
        effective.insert(key.clone(), value.clone());
    }
    if changed {
        let saved = Value::Object(effective).to_string();
        sqlx::query("UPDATE remote_storages SET name=json_extract(?1,'$.name'),enabled=json_extract(?1,'$.enabled'),host=json_extract(?1,'$.host'),port=json_extract(?1,'$.port'),username=json_extract(?1,'$.username'),password=json_extract(?1,'$.password'),base_path=json_extract(?1,'$.base_path'),ssh_key_path=json_extract(?1,'$.ssh_key_path'),s3_bucket=json_extract(?1,'$.s3_bucket'),s3_region=json_extract(?1,'$.s3_region'),s3_access_key=json_extract(?1,'$.s3_access_key'),s3_secret_key=json_extract(?1,'$.s3_secret_key'),s3_endpoint_url=json_extract(?1,'$.s3_endpoint_url'),webdav_url=json_extract(?1,'$.webdav_url'),rclone_remote=json_extract(?1,'$.rclone_remote'),updated_at=strftime('%Y-%m-%d %H:%M:%f','now') WHERE id=?2")
            .bind(saved).bind(id).execute(&mut *tx).await?;
    }
    let row = sqlx::query_scalar::<_, String>(select_storage!(" WHERE id=?"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let view = storage_view(row).ok_or(sqlx::Error::Protocol("Invalid storage record".into()))?;
    tx.commit().await?;
    Ok(Json(view).into_response())
}

fn coerce_storage_scalars(values: &mut serde_json::Map<String, Value>) {
    if let Some(value) = values.get_mut("enabled") {
        let flag = match value {
            Value::String(text) => match text.to_ascii_lowercase().as_str() {
                "0" | "false" | "off" | "n" | "no" | "f" => Some(false),
                "1" | "true" | "on" | "y" | "yes" | "t" => Some(true),
                _ => None,
            },
            Value::Number(number) if number.as_f64() == Some(0.0) => Some(false),
            Value::Number(number) if number.as_f64() == Some(1.0) => Some(true),
            _ => None,
        };
        if let Some(flag) = flag {
            *value = json!(flag);
        }
    }
    if let Some(value) = values.get_mut("port") {
        let port = match value {
            Value::Bool(flag) => Some(i64::from(*flag)),
            Value::String(text) => super::super::operations::query_integer(text),
            Value::Number(number) => number.as_i64().or_else(|| {
                number
                    .as_f64()
                    .filter(|value| value.fract() == 0.0 && value.abs() < i64::MAX as f64)
                    .map(|value| value as i64)
            }),
            _ => None,
        };
        if let Some(port) = port {
            *value = json!(port);
        }
    }
}

async fn prepare(
    state: &HttpState,
    mut data: Value,
    tx: &mut sqlx::SqliteConnection,
) -> Result<Value, Box<Response>> {
    let config = state.config.clone();
    let needs_key = SECRETS
        .iter()
        .any(|field| data[*field].as_str().is_some_and(|text| !text.is_empty()));
    let needs_path = data["ssh_key_path"]
        .as_str()
        .is_some_and(|text| !text.is_empty());
    if needs_key || needs_path {
        let first_write = if needs_key && config.credential_encryption_key.expose().is_empty() {
            sqlx::query_scalar::<_, bool>("SELECT NOT EXISTS (SELECT 1 FROM remote_storages WHERE password LIKE 'fernet:%' OR s3_access_key LIKE 'fernet:%' OR s3_secret_key LIKE 'fernet:%') AND NOT EXISTS (SELECT 1 FROM app_settings WHERE key IN ('recovery_rclone','recovery_activation'))")
                .fetch_one(tx).await.map_err(|_| Box::new(storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")))?
        } else {
            false
        };
        let work = state.auth.blocking().submit_blocking(move |cancel| {
            if needs_path {
                let dirs: Vec<_> = config
                    .allowed_ssh_key_dirs
                    .split(',')
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .collect();
                let path =
                    std::fs::canonicalize(data["ssh_key_path"].as_str().expect("validated path"))
                        .ok();
                let Some(path) = path.filter(|path| {
                    dirs.iter()
                        .filter_map(|root| std::fs::canonicalize(root).ok())
                        .any(|root| path.starts_with(root))
                }) else {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!("SSH key path must be within allowed directories: {dirs:?}"),
                    ));
                };
                data["ssh_key_path"] = json!(path);
            }
            if needs_key {
                let cipher = crate::storage_recovery::load_write_cipher(
                    std::path::Path::new("/app/data/.credential_key"),
                    config.credential_encryption_key.expose(),
                    first_write,
                    || cancel.is_cancelled(),
                )
                .map_err(|_| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Credential encryption key unavailable".to_owned(),
                    )
                })?;
                for field in SECRETS {
                    if let Some(text) = data[field].as_str().filter(|text| !text.is_empty()) {
                        data[field] = json!(cipher.encrypt_value(text));
                    }
                }
            }
            Ok(data)
        });
        data = match work {
            Ok(work) => match work.wait().await {
                Ok(Ok(data)) => data,
                Ok(Err((status, detail))) => {
                    return Err(Box::new(
                        (status, Json(json!({"detail":detail}))).into_response(),
                    ));
                }
                Err(_) => {
                    return Err(Box::new(storage_error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Service unavailable",
                    )));
                }
            },
            Err(_) => {
                return Err(Box::new(storage_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Service unavailable",
                )));
            }
        };
    }
    Ok(data)
}

async fn persist_create(state: &HttpState, data: Value) -> Response {
    let result = async {
        let mut tx = state.auth.pool().begin_with("BEGIN IMMEDIATE").await?;
        let data = match prepare(state, data, &mut tx).await { Ok(value) => value, Err(response) => return Ok(*response) };
        let id = sqlx::query_scalar::<_, i64>("INSERT INTO remote_storages(name,storage_type,enabled,host,port,username,password,base_path,ssh_key_path,s3_bucket,s3_region,s3_access_key,s3_secret_key,s3_endpoint_url,webdav_url,rclone_remote,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,strftime('%Y-%m-%d %H:%M:%f','now'),strftime('%Y-%m-%d %H:%M:%f','now')) RETURNING id")
            .bind(data["name"].as_str()).bind(data["storage_type"].as_str()).bind(data["enabled"].as_bool())
            .bind(data["host"].as_str()).bind(data["port"].as_i64()).bind(data["username"].as_str())
            .bind(data["password"].as_str()).bind(data["base_path"].as_str()).bind(data["ssh_key_path"].as_str().filter(|path| !path.is_empty()))
            .bind(data["s3_bucket"].as_str()).bind(data["s3_region"].as_str()).bind(data["s3_access_key"].as_str())
            .bind(data["s3_secret_key"].as_str()).bind(data["s3_endpoint_url"].as_str()).bind(data["webdav_url"].as_str()).bind(data["rclone_remote"].as_str())
            .fetch_one(&mut *tx).await?;
        let row = sqlx::query_scalar::<_,String>(select_storage!(" WHERE id=?")).bind(id).fetch_one(&mut *tx).await?;
        let view = storage_view(row).ok_or(sqlx::Error::Protocol("Invalid storage record".into()))?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(Json(view).into_response())
    }.await;
    match result {
        Ok(response) => response,
        Err(_) => storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
}
