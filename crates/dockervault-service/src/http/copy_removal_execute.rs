use super::{Confirmation, HttpState, Item, detail, inspect, required_user, validation};
use crate::{jobs::Resource, storage::local::delete_reviewed_leaf};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{os::unix::fs::MetadataExt, path::PathBuf, sync::Arc, time::Instant};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
pub(in crate::http) struct Execute {
    token: String,
}

pub(in crate::http) async fn execute(
    State(state): State<HttpState>,
    headers: HeaderMap,
    input: Result<Json<Execute>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let Ok(Json(Execute { token })) = input else {
        return *validation(vec![
            json!({"field":"body.token","message":"Invalid token"}),
        ]);
    };
    if token.is_empty() || token.len() > 128 {
        return *validation(vec![
            json!({"field":"body.token","message":"Invalid token"}),
        ]);
    }
    let confirmation = state
        .removal_previews
        .lock()
        .unwrap()
        .0
        .get(&token)
        .filter(|entry| entry.expires > Instant::now())
        .cloned();
    let Some(confirmation) = confirmation else {
        return detail(
            StatusCode::CONFLICT,
            "Preview expired or server restarted; review the scope again",
        );
    };
    let ctx = state.clone();
    let (send, recv) = tokio::sync::oneshot::channel();
    let Ok(work) = state.backup_jobs.submit_async(move |cancel| async move {
        let result = settle(&ctx, confirmation, &cancel).await;
        let _ = send.send(result);
    }) else {
        return detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Removal capacity exhausted",
        );
    };
    work.detach();
    match recv.await {
        Ok(result) => Json(result).into_response(),
        Err(_) => detail(StatusCode::SERVICE_UNAVAILABLE, "Removal interrupted"),
    }
}

async fn settle(
    state: &HttpState,
    confirmation: Arc<Confirmation>,
    cancel: &CancellationToken,
) -> Value {
    let mut result = confirmation.result.lock().await;
    if let Some(result) = result.as_ref() {
        return result.clone();
    }
    let (mut succeeded, mut failed, mut skipped) = (0, 0, 0);
    let mut items = Vec::new();
    let mut retry = Vec::new();
    // Provider removal is not delegated to the generic browser DELETE route:
    // that route cannot settle this backup's confirmed catalog identity.
    for item in confirmation
        .items
        .iter()
        .filter(|i| i.local.is_none())
        .chain(confirmation.items.iter().filter(|i| i.local.is_some()))
    {
        let (outcome, message) = match remove_local(state, item, cancel).await {
            Ok(()) => {
                succeeded += 1;
                ("succeeded", "Selected copy removed")
            }
            Err(Skipped(message)) => {
                skipped += 1;
                ("skipped", message)
            }
            Err(Failed(message)) => {
                failed += 1;
                if item.local.is_some() {
                    retry.push(item.clone());
                }
                ("failed", message)
            }
        };
        let mut row = json!(item);
        row["outcome"] = json!(outcome);
        row["message"] = json!(message);
        items.push(row);
    }
    items.extend(confirmation.retained.clone());
    let incomplete = failed + skipped > 0;
    let mut response = json!({"status":if incomplete {"incomplete"} else {"completed"},"succeeded":succeeded,"failed":failed,"skipped":skipped,"items":items,"retry_token":null});
    if incomplete {
        response["warning"] = json!(
            "Not all confirmed copies were removed. Retained and failed copies are not a success."
        );
    }
    if !retry.is_empty()
        && let Ok(token) = state
            .removal_previews
            .lock()
            .unwrap()
            .insert(retry, confirmation.retained.clone())
    {
        response["retry_token"] = json!(token);
    }
    *result = Some(response.clone());
    response
}

enum EntryError {
    Skipped(&'static str),
    Failed(&'static str),
}
use EntryError::{Failed, Skipped};
const INCOMPLETE: EntryError = Failed("Copy removal incomplete; metadata retained for retry");
const CHANGED: EntryError = Skipped("Copy scope changed; review again before deleting");

async fn check_remote(state: &HttpState, item: &Item) -> Result<(), EntryError> {
    let (Some(storage_id), Some(remote)) = (item.storage_id, &item.remote) else {
        return Err(CHANGED);
    };
    let mut tx = state.auth.pool().begin().await.map_err(|_| INCOMPLETE)?;
    let current = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<bool>)>("SELECT s.remote_path,s.sync_status,s.synced_at,b.created_at,b.checksum,b.file_size,b.encrypted FROM backup_storage_syncs s JOIN backups b ON b.id=s.backup_id WHERE s.id=? AND s.backup_id=? AND s.storage_id=? AND b.target_id=?")
        .bind(remote.sync_id).bind(item.backup_id).bind(storage_id).bind(item.target_id)
        .fetch_optional(&mut *tx).await.map_err(|_| INCOMPLETE)?;
    if current.is_none_or(
        |(path, status, synced_at, created, checksum, size, encrypted)| {
            path != remote.archive
                || status != remote.status
                || synced_at != remote.synced_at
                || (created, checksum, size, encrypted) != remote.backup_identity
        },
    ) {
        return Err(CHANGED);
    }
    let destination = crate::storage_recovery::remote_storage_identity_in(&mut *tx, storage_id)
        .await
        .map_err(|_| INCOMPLETE)?;
    if destination != remote.destination {
        return Err(CHANGED);
    }
    if let Some(archive) = &remote.archive {
        let selected = replica_paths(archive, remote.backup_identity.3.unwrap_or(false));
        let others = sqlx::query_as::<_, (String, Option<bool>)>(
            "SELECT s.remote_path,b.encrypted FROM backup_storage_syncs s JOIN backups b ON b.id=s.backup_id WHERE s.storage_id=? AND s.id!=? AND s.remote_path IS NOT NULL",
        )
        .bind(storage_id)
        .bind(remote.sync_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| INCOMPLETE)?;
        if others.into_iter().any(|(path, encrypted)| {
            replica_paths(&path, encrypted.unwrap_or(false))
                .iter()
                .any(|path| selected.contains(path))
        }) {
            return Err(Skipped(
                "Copy is shared by another archive identity; retained for safety",
            ));
        }
    }
    Err(Failed("Confirmed remote copy removal is not available"))
}

fn replica_paths(archive: &str, encrypted: bool) -> Vec<String> {
    let Some(path) = crate::storage::relative_path(archive, false) else {
        return Vec::new();
    };
    let sidecar = encrypted.then(|| dockervault_core::crypto::key_sidecar_path(&path));
    std::iter::once(path).chain(sidecar).collect()
}

type Catalog = (
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<bool>,
    Option<String>,
);

async fn remove_local(
    state: &HttpState,
    item: &Item,
    cancel: &CancellationToken,
) -> Result<(), EntryError> {
    let Some(local) = &item.local else {
        return check_remote(state, item).await;
    };
    let Some(files) = local.files.clone() else {
        return Err(CHANGED);
    };
    let _claim = state
        .backup_claims
        .acquire([Resource::target(item.target_id)], cancel)
        .await
        .map_err(|_| INCOMPLETE)?;
    let mut db = state.auth.pool().acquire().await.map_err(|_| INCOMPLETE)?;
    let mut tx = sqlx::Connection::begin_with(&mut *db, "BEGIN IMMEDIATE")
        .await
        .map_err(|_| INCOMPLETE)?;
    let row=sqlx::query_as::<_,Catalog>("SELECT target_id,file_path,encryption_key_path,created_at,checksum,file_size,encrypted,status FROM backups WHERE id=?")
        .bind(item.backup_id).fetch_optional(&mut *tx).await.map_err(|_|INCOMPLETE)?;
    let Some((target, archive, key, created, checksum, size, encrypted, status)) = row else {
        return Err(Skipped("Archive or copy no longer exists; review again"));
    };
    if target != item.target_id
        || archive != local.archive
        || key != local.key
        || (created, checksum, size, encrypted) != local.row_identity
    {
        return Err(CHANGED);
    }
    let pending: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(backup_metadata,'$.native_copy_removal') FROM backups WHERE id=?",
    )
    .bind(item.backup_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| INCOMPLETE)?;
    if let Some(raw) = pending {
        let intent: LocalIntent = serde_json::from_str(&raw).map_err(|_| INCOMPLETE)?;
        if intent.files.iter().any(|file| !files.contains(file)) {
            return Err(CHANGED);
        }
        tx.rollback().await.map_err(|_| INCOMPLETE)?;
        drop(db);
        return settle_local(state, item.backup_id, intent, &raw, cancel).await;
    }
    let active: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workload_operations WHERE backup_id=? AND state!='finished') OR EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=? AND sync_status IN ('pending','running'))")
        .bind(item.backup_id).bind(item.backup_id).fetch_one(&mut *tx).await.map_err(|_|INCOMPLETE)?;
    if active || matches!(status.as_deref(), Some("PENDING" | "RUNNING")) {
        return Err(Failed(
            "Active or unresolved operation; retry after recovery",
        ));
    }
    // Check aliases owned by other catalog entries before removing either leaf.
    let others=sqlx::query_as::<_,(Option<String>,Option<String>)>("SELECT file_path,encryption_key_path FROM backups WHERE id!=? AND (file_path IS NOT NULL OR encryption_key_path IS NOT NULL)")
        .bind(item.backup_id).fetch_all(&mut *tx).await.map_err(|_|INCOMPLETE)?;
    let remote: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=?)")
            .bind(item.backup_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| INCOMPLETE)?;
    let root = PathBuf::from(&state.config.backup_base_path);
    let owned_cancel = cancel.clone();
    let retained_key = key.clone().filter(|_| remote);
    let reviewed = files.clone();
    let job = state
        .download_jobs
        .submit_blocking(move |cancel| {
            if cancel.is_cancelled() || owned_cancel.is_cancelled() {
                return Err(INCOMPLETE);
            }
            let current =
                inspect(&root, archive.as_deref(), key.as_deref()).map_err(|_| CHANGED)?;
            if current != files {
                return Err(CHANGED);
            }
            for (archive, key) in others {
                for raw in [archive, key].into_iter().flatten() {
                    let Ok(other) = inspect(&root, Some(&raw), None) else {
                        continue;
                    };
                    if other.iter().any(|(path, stamp)| {
                        files
                            .iter()
                            .any(|(selected, s)| selected == path || (s.is_some() && s == stamp))
                    }) {
                        return Err(Skipped(
                            "Copy is shared by another archive identity; retained for safety",
                        ));
                    }
                }
            }
            let root_meta = crate::storage::local::open_root(&root)
                .and_then(|f| f.metadata())
                .map_err(|_| CHANGED)?;
            Ok((root_meta.dev(), root_meta.ino()))
        })
        .map_err(|_| INCOMPLETE)?;
    let root_identity = job.wait().await.map_err(|_| INCOMPLETE)??;
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| INCOMPLETE)?;
    let nonce: String = bytes.iter().map(|v| format!("{v:02x}")).collect();
    let root = PathBuf::from(&state.config.backup_base_path);
    let keep = retained_key
        .as_deref()
        .and_then(|p| std::path::absolute(p).ok());
    let intent = LocalIntent {
        nonce,
        root_identity,
        root: root.clone(),
        files: reviewed
            .into_iter()
            .filter(|(path, _)| Some(root.join(path)) != keep)
            .collect(),
        target: item.target_id,
        remote,
        retained_key,
        row_identity: local.row_identity.clone(),
        archive: local.archive.clone(),
        key: local.key.clone(),
    };
    let raw = serde_json::to_string(&intent).map_err(|_| INCOMPLETE)?;
    let result=sqlx::query("UPDATE backups SET backup_metadata=json_set(coalesce(nullif(backup_metadata,'null'),'{}'),'$.native_copy_removal',json(?)) WHERE id=? AND json_type(backup_metadata,'$.native_copy_removal') IS NULL")
        .bind(&raw).bind(item.backup_id).execute(&mut *tx).await.map_err(|_|INCOMPLETE)?;
    if result.rows_affected() != 1 {
        return Err(INCOMPLETE);
    }
    tx.commit().await.map_err(|_| INCOMPLETE)?;
    drop(db);
    settle_local(state, item.backup_id, intent, &raw, cancel).await
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LocalIntent {
    nonce: String,
    root_identity: (u64, u64),
    root: PathBuf,
    files: Vec<(PathBuf, Option<crate::storage::FileIdentity>)>,
    target: i64,
    remote: bool,
    retained_key: Option<String>,
    row_identity: (Option<String>, Option<String>, Option<i64>, Option<bool>),
    archive: Option<String>,
    key: Option<String>,
}

pub async fn recover(state: &HttpState) -> Result<(), sqlx::Error> {
    let mut last = i64::MIN;
    loop {
        let page = sqlx::query_as::<_, (i64,String)>("SELECT id,json_extract(backup_metadata,'$.native_copy_removal') FROM backups WHERE id>? AND json_type(backup_metadata,'$.native_copy_removal')='object' ORDER BY id LIMIT 64")
            .bind(last).fetch_all(state.auth.pool()).await?;
        if page.is_empty() {
            return Ok(());
        }
        for (id, raw) in page {
            last = id;
            let Ok(intent) = serde_json::from_str::<LocalIntent>(&raw) else {
                tracing::warn!(backup_id = id, "Invalid copy removal receipt retained");
                continue;
            };
            if settle_local(state, id, intent, &raw, &CancellationToken::new())
                .await
                .is_err()
            {
                tracing::warn!(backup_id = id, "Unsettled local copy removal retained");
            }
        }
    }
}

async fn settle_local(
    state: &HttpState,
    id: i64,
    intent: LocalIntent,
    raw: &str,
    cancel: &CancellationToken,
) -> Result<(), EntryError> {
    let mut db = state.auth.pool().acquire().await.map_err(|_| INCOMPLETE)?;
    let mut tx = sqlx::Connection::begin_with(&mut *db, "BEGIN IMMEDIATE")
        .await
        .map_err(|_| INCOMPLETE)?;
    let current=sqlx::query_as::<_,Catalog>("SELECT target_id,file_path,encryption_key_path,created_at,checksum,file_size,encrypted,status FROM backups WHERE id=? AND json_extract(backup_metadata,'$.native_copy_removal')=?")
        .bind(id).bind(raw).fetch_optional(&mut *tx).await.map_err(|_|INCOMPLETE)?;
    let Some((target, archive, key, created, checksum, size, encrypted, status)) = current else {
        return Err(CHANGED);
    };
    if target != intent.target
        || archive != intent.archive
        || key != intent.key
        || (created, checksum, size, encrypted) != intent.row_identity
        || matches!(status.as_deref(), Some("PENDING" | "RUNNING"))
    {
        return Err(CHANGED);
    }
    let remote: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=?)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| INCOMPLETE)?;
    if remote != intent.remote {
        return Err(CHANGED);
    }
    let active: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workload_operations WHERE backup_id=? AND state!='finished') OR EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=? AND sync_status IN ('pending','running'))")
        .bind(id).bind(id).fetch_one(&mut *tx).await.map_err(|_|INCOMPLETE)?;
    if active {
        return Err(INCOMPLETE);
    }
    let root = PathBuf::from(&state.config.backup_base_path);
    if root != intent.root {
        return Err(CHANGED);
    }
    let retained_key = intent.retained_key;
    let request_cancel = cancel.clone();
    let work = state
        .download_jobs
        .submit_blocking(move |worker_cancel| {
            let root_meta = crate::storage::local::open_root(&root)
                .and_then(|f| f.metadata())
                .map_err(|_| CHANGED)?;
            if (root_meta.dev(), root_meta.ino()) != intent.root_identity {
                return Err(CHANGED);
            }
            for (index, (path, stamp)) in intent.files.into_iter().enumerate() {
                if worker_cancel.is_cancelled() || request_cancel.is_cancelled() {
                    return Err(INCOMPLETE);
                }
                let name = path.to_str().ok_or(CHANGED)?;
                if crate::storage::relative_path(name, false).as_deref() != Some(name) {
                    return Err(CHANGED);
                }
                if let Some(stamp) = stamp {
                    delete_reviewed_leaf(
                        &root,
                        &path,
                        stamp,
                        &format!(".dv-removal-{}-{index}", intent.nonce),
                    )
                    .map_err(|_| INCOMPLETE)?;
                } else {
                    match crate::storage::local::open_download_source(&root, &path) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        _ => return Err(CHANGED),
                    }
                }
            }
            Ok(())
        })
        .map_err(|_| INCOMPLETE)?;
    work.wait().await.map_err(|_| INCOMPLETE)??;
    if remote || retained_key.is_some() {
        sqlx::query("UPDATE backups SET file_path=NULL,encryption_key_path=?,backup_metadata=json_set(json_remove(backup_metadata,'$.native_copy_removal'),'$.local_copy_removed',json('true')) WHERE id=?")
            .bind(retained_key).bind(id).execute(&mut *tx).await.map_err(|_|INCOMPLETE)?;
    } else {
        sqlx::query("DELETE FROM backups WHERE id=?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| INCOMPLETE)?;
    }
    tx.commit().await.map_err(|_| INCOMPLETE)
}
