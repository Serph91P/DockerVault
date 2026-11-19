use super::{INTEGER, detail, internal, query_integer, required_user, sanitized_path, validation};
use crate::{
    http::{
        HttpState,
        storage::{
            attachment, completed_archive_copies,
            download::{Validators, response},
            saved_archive_source, snapshot_for_download,
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
use std::{
    collections::HashMap,
    io::{ErrorKind, Seek},
    path::Path as FsPath,
    sync::Arc,
};

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

    let saturated = || {
        detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Download capacity exhausted",
        )
    };
    let Ok(slot) = state.download_slots.clone().try_acquire_owned() else {
        return saturated();
    };
    let slot = Arc::new(slot);
    let local_slot = slot.clone();
    let root = state.config.backup_base_path.clone();
    let Ok(work) = state.download_jobs.submit_blocking(move |cancel| {
        let _local_slot = local_slot;
        let file = file.filter(|p| !p.is_empty())?;
        if root.is_empty() {
            return None;
        }
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
        Some(Ok((snapshot, len, validators, attachment(name))))
    }) else {
        return saturated();
    };
    match work.wait().await {
        Ok(Some(Ok((file, len, validators, disposition)))) => {
            archive_response(file, len, slot, disposition, validators, &headers)
        }
        Ok(Some(Err(message))) => detail(StatusCode::NOT_FOUND, message),
        Ok(None) => {
            remote_archive(
                &state,
                id,
                encrypted.unwrap_or(false),
                sidecar,
                slot,
                &headers,
            )
            .await
        }
        Err(_) => unavailable(),
    }
}

fn archive_response(
    file: std::fs::File,
    len: u64,
    slot: impl Send + 'static,
    disposition: String,
    validators: Validators,
    headers: &HeaderMap,
) -> Response {
    let mut reply = response(
        file,
        len,
        slot,
        disposition,
        validators,
        headers,
        "application/octet-stream",
    );
    if reply.status().is_success() {
        reply
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    reply
}

async fn remote_archive(
    state: &HttpState,
    id: i64,
    encrypted: bool,
    sidecar: bool,
    slot: Arc<tokio::sync::OwnedSemaphorePermit>,
    headers: &HeaderMap,
) -> Response {
    // The shared catalog checksum describes the plaintext TAR. For encrypted
    // downloads we can verify transfer completeness and catalog size, not
    // ciphertext integrity or authenticity: the caller has the recovery key.
    let Ok(expected) = sqlx::query_as::<_, (Option<i64>, Option<String>)>(
        "SELECT file_size, CASE WHEN NOT ? THEN checksum END FROM backups WHERE id=?",
    )
    .bind(encrypted)
    .bind(id)
    .fetch_one(state.auth.pool())
    .await
    else {
        return internal();
    };
    let Ok(copies) = completed_archive_copies(state, id).await else {
        return internal();
    };
    for (storage_id, path) in copies {
        let Some(source) = saved_archive_source(state, storage_id).await else {
            continue;
        };
        let Some((file, len, _, validators)) =
            snapshot_for_download(state, source.clone(), path.clone(), slot.clone()).await
        else {
            continue;
        };
        let Some(file) = verify_archive(state, file, len, &expected, slot.clone()).await else {
            continue;
        };
        let (file, len, validators, path) = if sidecar {
            drop(file);
            if !encrypted {
                return detail(StatusCode::NOT_FOUND, "Archive has no encryption sidecar");
            }
            let path = dockervault_core::crypto::key_sidecar_path(&path);
            let Some((file, len, _, validators)) =
                snapshot_for_download(state, source, path.clone(), slot.clone()).await
            else {
                continue;
            };
            if len == 0 {
                continue;
            }
            (file, len, validators, path)
        } else {
            (file, len, validators, path)
        };
        if let Some(name) = FsPath::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
        {
            return archive_response(file, len, slot, attachment(name), validators, headers);
        }
    }
    detail(StatusCode::NOT_FOUND, "Archive content unavailable")
}

pub(super) async fn verify_archive(
    state: &HttpState,
    mut file: std::fs::File,
    len: u64,
    (size, checksum): &(Option<i64>, Option<String>),
    slot: Arc<tokio::sync::OwnedSemaphorePermit>,
) -> Option<std::fs::File> {
    if size.is_some_and(|size| u64::try_from(size) != Ok(len)) {
        return None;
    }
    let Some(checksum) = checksum.clone() else {
        return Some(file);
    };
    state
        .download_jobs
        .submit_blocking(move |cancel| {
            let _slot = slot;
            file.rewind().ok()?;
            let hash = crate::storage::hash_reader(&mut file, &|| cancel.is_cancelled()).ok()?;
            file.rewind().ok()?;
            let hex = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
            checksum.eq_ignore_ascii_case(&hex).then_some(file)
        })
        .ok()?
        .wait()
        .await
        .ok()?
}
