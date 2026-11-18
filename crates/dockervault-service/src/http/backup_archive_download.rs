use super::{INTEGER, detail, internal, query_integer, required_user, sanitized_path, validation};
use crate::{
    http::{
        HttpState,
        storage::{
            attachment,
            download::{Validators, response},
        },
    },
    storage::local::{open_download_source, snapshot_download_source},
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use serde_json::json;
use std::{collections::HashMap, io::ErrorKind, path::Path as FsPath};

pub(in crate::http) async fn whole_archive(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let id = query_integer(&id);
    let sidecar = match query.get("sidecar").map(|value| value.to_ascii_lowercase()) {
        None => Some(false),
        Some(value) => match value.as_str() {
            "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
            "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
            _ => None,
        },
    };
    let (Some(id), Some(sidecar)) = (id, sidecar) else {
        return *validation([
            id.is_none().then(|| json!({"field":"path.backup_id","message":INTEGER})),
            sidecar.is_none().then(|| json!({"field":"query.sidecar","message":"Input should be a valid boolean, unable to interpret input"})),
        ].into_iter().flatten().collect());
    };
    let found =
        sqlx::query_as::<_, (Option<String>, Option<String>, Option<bool>, Option<String>)>(
            "SELECT status,file_path,encrypted,encryption_key_path FROM backups WHERE id=?",
        )
        .bind(id)
        .fetch_optional(state.auth.pool())
        .await;
    let (status, file, encrypted, key) = match found {
        Ok(Some(found)) => found,
        Ok(None) => return detail(StatusCode::NOT_FOUND, "Archive not found"),
        Err(_) => return internal(),
    };
    if status.as_deref() != Some("COMPLETED") {
        return detail(StatusCode::CONFLICT, "Run has no completed archive");
    }
    let unavailable = || detail(StatusCode::NOT_FOUND, "Archive content unavailable");
    let Some(file) = file.filter(|p| !p.is_empty()) else {
        return unavailable();
    };
    let saturated = || {
        detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Download capacity exhausted",
        )
    };
    let Ok(slot) = state.download_slots.clone().try_acquire_owned() else {
        return saturated();
    };
    if state.config.backup_base_path.is_empty() {
        return unavailable();
    }
    let root = state.config.backup_base_path.clone();
    let Ok(work) = state.download_jobs.submit_blocking(move |cancel| {
        let root = std::path::absolute(root).ok()?;
        let open = |path: &str| {
            let absolute = std::path::absolute(path)?;
            let relative = absolute
                .strip_prefix(&root)
                .map_err(std::io::Error::other)?;
            open_download_source(&root, relative)
        };
        let (archive, file) = match open(&file) {
            Ok(archive) => (archive, file),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let fallback = sanitized_path(&file);
                if file == fallback {
                    return None;
                }
                (open(&fallback).ok()?, fallback)
            }
            Err(_) => return None,
        };
        if sidecar && !encrypted.unwrap_or(false) {
            return Some(Err("Archive has no encryption sidecar"));
        }
        let (source, name) = if sidecar {
            let key = key
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| dockervault_core::crypto::key_sidecar_path(&file));
            (open(&key).ok()?, key)
        } else {
            (archive, file)
        };
        let name = FsPath::new(&name).file_name()?.to_str()?;
        let (snapshot, len) = snapshot_download_source(source, &|| cancel.is_cancelled()).ok()?;
        let validators = Validators::read(&snapshot, len).ok()?;
        Some(Ok((snapshot, len, validators, attachment(name), slot)))
    }) else {
        return saturated();
    };
    match work.wait().await {
        Ok(Some(Ok((file, len, validators, disposition, slot)))) => {
            let mut reply = response(
                file,
                len,
                slot,
                disposition,
                validators,
                &headers,
                "application/octet-stream",
            );
            if reply.status().is_success() {
                reply
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            }
            reply
        }
        Ok(Some(Err(message))) => detail(StatusCode::NOT_FOUND, message),
        _ => unavailable(),
    }
}
