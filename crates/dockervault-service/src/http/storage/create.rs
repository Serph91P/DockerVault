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
    let errors = storage_errors(&data);
    if !errors.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"detail":{"message":"Invalid storage configuration","errors":errors}})),
        )
            .into_response();
    }
    let config = state.config.clone();
    let needs_key = SECRETS
        .iter()
        .any(|field| data[*field].as_str().is_some_and(|text| !text.is_empty()));
    let needs_path = data["ssh_key_path"]
        .as_str()
        .is_some_and(|text| !text.is_empty());
    if needs_key || needs_path {
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
                let cipher = crate::storage_recovery::load_delete_cipher(
                    std::path::Path::new("/app/data/.credential_key"),
                    config.credential_encryption_key.expose(),
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
                    return (status, Json(json!({"detail":detail}))).into_response();
                }
                Err(_) => {
                    return storage_error(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable");
                }
            },
            Err(_) => return storage_error(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable"),
        };
    }
    let result = async {
        let mut tx = state.auth.pool().begin_with("BEGIN IMMEDIATE").await?;
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
        Ok::<_, sqlx::Error>(view)
    }.await;
    match result {
        Ok(view) => Json(view).into_response(),
        Err(_) => storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    }
}
