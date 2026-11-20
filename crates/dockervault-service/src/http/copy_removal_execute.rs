use super::{
    Confirmation, HttpState, Item, LocalCopy, RemoteCopy, Scope, detail, inspect, inspect_replica,
    required_user, validation,
};
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

pub(in crate::http) async fn apply_retention(
    state: &HttpState,
    target: i64,
) -> Result<(usize, usize), &'static str> {
    let ctx = state.clone();
    let (send, recv) = tokio::sync::oneshot::channel();
    state
        .backup_jobs
        .submit_async(move |cancel| async move {
            let _ = send.send(retain_local(&ctx, target, &cancel).await);
        })
        .map_err(|_| "Retention capacity exhausted")?
        .detach();
    recv.await.map_err(|_| "Retention interrupted")?
}

pub(in crate::http) async fn retain_local(
    state: &HttpState,
    target: i64,
    cancel: &CancellationToken,
) -> Result<(usize, usize), &'static str> {
    const ERROR: &str = "Could not read retention selection";
    let _claim = state
        .backup_claims
        .acquire([Resource::target(target)], cancel)
        .await
        .map_err(|_| "Retention interrupted")?;
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000) as i64;
    let mut tx = state.auth.pool().begin().await.map_err(|_| ERROR)?;
    let selection = super::super::retention::select(&mut tx, target, now).await?;
    let rows = sqlx::query_as::<_, super::Backup>("SELECT b.id,b.target_id,t.name,b.file_path,b.encryption_key_path,b.created_at,b.checksum,b.file_size,b.encrypted FROM backups b JOIN backup_targets t ON t.id=b.target_id WHERE b.target_id=? AND b.id IN (SELECT value FROM json_each(?)) AND (coalesce(b.file_path,'')!='' OR (coalesce(b.encryption_key_path,'')!='' AND NOT EXISTS(SELECT 1 FROM backup_storage_syncs s WHERE s.backup_id=b.id)))")
        .bind(target).bind(json!(selection.expired).to_string()).fetch_all(&mut *tx).await.map_err(|_| ERROR)?;
    tx.commit().await.map_err(|_| ERROR)?;
    let root = PathBuf::from(&state.config.backup_base_path);
    let reviewed = state
        .download_jobs
        .submit_blocking(move |cancel| {
            rows.into_iter()
                .take_while(|_| !cancel.is_cancelled())
                .map(
                    |(
                        backup_id,
                        target_id,
                        target_name,
                        archive,
                        key,
                        created,
                        checksum,
                        size,
                        encrypted,
                    )| {
                        let files = inspect(&root, archive.as_deref(), key.as_deref()).ok();
                        let local = LocalCopy {
                            archive,
                            key,
                            row_identity: (created, checksum, size, encrypted),
                            files,
                        };
                        Item {
                            backup_id,
                            target_id,
                            target_name,
                            copy: Scope::Local,
                            storage_id: None,
                            label: "Local copy".into(),
                            local: Some(local),
                            remote: None,
                        }
                    },
                )
                .collect::<Vec<_>>()
        })
        .map_err(|_| "Retention capacity exhausted")?
        .wait()
        .await
        .map_err(|_| "Retention interrupted")?;
    let mut deleted = 0;
    for item in reviewed {
        if cancel.is_cancelled() {
            return Err("Retention interrupted");
        }
        if let Some(local) = &item.local
            && remove_claimed_primary(state, &item, local, Some(now), cancel)
                .await
                .is_ok()
        {
            deleted += 1;
        }
    }
    Ok((selection.kept, deleted))
}

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
                if item.local.is_some()
                    || item
                        .remote
                        .as_ref()
                        .is_some_and(|r| r.local_files.is_some())
                {
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

async fn check_remote(
    tx: &mut sqlx::SqliteConnection,
    backup: i64,
    target: i64,
    storage_id: i64,
    remote: &RemoteCopy,
) -> Result<(), EntryError> {
    let current = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<bool>)>("SELECT s.remote_path,s.sync_status,s.synced_at,b.created_at,b.checksum,b.file_size,b.encrypted FROM backup_storage_syncs s JOIN backups b ON b.id=s.backup_id WHERE s.id=? AND s.backup_id=? AND s.storage_id=? AND b.target_id=?")
        .bind(remote.sync_id).bind(backup).bind(storage_id).bind(target)
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
    Ok(())
}

fn replica_paths(archive: &str, encrypted: bool) -> Vec<String> {
    let Some(path) = crate::storage::relative_path(archive, false) else {
        return Vec::new();
    };
    let sidecar = encrypted.then(|| dockervault_core::crypto::key_sidecar_path(&path));
    std::iter::once(path).chain(sidecar).collect()
}

#[derive(serde::Serialize, Deserialize)]
struct ReplicaIntent {
    nonce: String,
    target_id: i64,
    storage_id: i64,
    replica: RemoteCopy,
}

async fn remove_replica(
    state: &HttpState,
    item: &Item,
    cancel: &CancellationToken,
) -> Result<(), EntryError> {
    let (Some(storage_id), Some(replica)) = (item.storage_id, &item.remote) else {
        return Err(CHANGED);
    };
    let _claim = state
        .backup_claims
        .acquire([Resource::target(item.target_id)], cancel)
        .await
        .map_err(|_| INCOMPLETE)?;
    let mut tx = state
        .auth
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| INCOMPLETE)?;
    check_remote(&mut tx, item.backup_id, item.target_id, storage_id, replica).await?;
    let Some(root) = &replica.local_root else {
        return Err(Failed("Confirmed remote copy removal is not available"));
    };
    let (Some(path), Some(reviewed)) = (&replica.archive, &replica.local_files) else {
        return Err(CHANGED);
    };
    let pending: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(backup_metadata,'$.native_copy_removal') FROM backups WHERE id=?",
    )
    .bind(item.backup_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| INCOMPLETE)?;
    if let Some(raw) = pending {
        let intent: ReplicaIntent = serde_json::from_str(&raw).map_err(|_| INCOMPLETE)?;
        if intent.storage_id != storage_id
            || intent.replica.sync_id != replica.sync_id
            || intent.replica.local_root.as_ref() != Some(root)
            || intent
                .replica
                .local_files
                .as_ref()
                .is_none_or(|(id, files)| {
                    *id != reviewed.0
                        || files
                            .iter()
                            .zip(&reviewed.1)
                            .any(|((old_path, old), (path, now))| {
                                old_path != path || (now.is_some() && now != old)
                            })
                        || files.len() != reviewed.1.len()
                })
        {
            return Err(CHANGED);
        }
        tx.rollback().await.map_err(|_| INCOMPLETE)?;
        return settle_replica(state, item.backup_id, &intent, &raw, cancel).await;
    }
    ensure_backup_idle(&mut tx, item.backup_id).await?;
    let (root, path, encrypted) = (
        root.clone(),
        path.clone(),
        replica.backup_identity.3.unwrap_or(false),
    );
    let current = state
        .download_jobs
        .submit_blocking(move |_| inspect_replica(&root, &path, encrypted))
        .map_err(|_| INCOMPLETE)?
        .wait()
        .await
        .map_err(|_| INCOMPLETE)?
        .map_err(|_| CHANGED)?;
    if &current != reviewed {
        return Err(CHANGED);
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| INCOMPLETE)?;
    let intent = ReplicaIntent {
        nonce: bytes.iter().map(|v| format!("{v:02x}")).collect(),
        target_id: item.target_id,
        storage_id,
        replica: replica.clone(),
    };
    let raw = serde_json::to_string(&intent).map_err(|_| INCOMPLETE)?;
    let created = sqlx::query("UPDATE backups SET backup_metadata=json_set(coalesce(nullif(backup_metadata,'null'),'{}'),'$.native_copy_removal',json(?)) WHERE id=? AND json_type(backup_metadata,'$.native_copy_removal') IS NULL")
        .bind(&raw).bind(item.backup_id).execute(&mut *tx).await.map_err(|_| INCOMPLETE)?;
    if created.rows_affected() != 1 {
        return Err(INCOMPLETE);
    }
    tx.commit().await.map_err(|_| INCOMPLETE)?;
    settle_replica(state, item.backup_id, &intent, &raw, cancel).await
}

async fn ensure_backup_idle(
    tx: &mut sqlx::SqliteConnection,
    backup: i64,
) -> Result<(), EntryError> {
    let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backups WHERE id=? AND status IN ('PENDING','RUNNING')) OR EXISTS(SELECT 1 FROM workload_operations WHERE backup_id=? AND state!='finished') OR EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=? AND sync_status IN ('pending','running'))")
        .bind(backup).bind(backup).bind(backup).fetch_one(tx).await.map_err(|_| INCOMPLETE)?;
    if active {
        return Err(INCOMPLETE);
    }
    Ok(())
}

async fn settle_replica(
    state: &HttpState,
    backup: i64,
    intent: &ReplicaIntent,
    raw: &str,
    cancel: &CancellationToken,
) -> Result<(), EntryError> {
    let replica = &intent.replica;
    let (Some(root), Some((root_identity, files))) = (&replica.local_root, &replica.local_files)
    else {
        return Err(CHANGED);
    };
    let mut tx = state
        .auth
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| INCOMPLETE)?;
    let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backups WHERE id=? AND json_extract(backup_metadata,'$.native_copy_removal')=?)").bind(backup).bind(raw).fetch_one(&mut *tx).await.map_err(|_| INCOMPLETE)?;
    if !current {
        return Err(CHANGED);
    }
    check_remote(
        &mut tx,
        backup,
        intent.target_id,
        intent.storage_id,
        replica,
    )
    .await?;
    ensure_backup_idle(&mut tx, backup).await?;
    let primary = sqlx::query_as::<_, (Option<String>, Option<String>)>("SELECT file_path,encryption_key_path FROM backups WHERE file_path IS NOT NULL OR encryption_key_path IS NOT NULL")
        .fetch_all(&mut *tx).await.map_err(|_| INCOMPLETE)?;
    let aliases = sqlx::query_as::<_, (String, String, Option<bool>)>("SELECT r.base_path,s.remote_path,b.encrypted FROM backup_storage_syncs s JOIN remote_storages r ON r.id=s.storage_id JOIN backups b ON b.id=s.backup_id WHERE r.storage_type='local' AND s.storage_id!=? AND r.base_path IS NOT NULL AND s.remote_path IS NOT NULL")
        .bind(intent.storage_id).fetch_all(&mut *tx).await.map_err(|_| INCOMPLETE)?;
    let root = root.clone();
    let files = files.clone();
    let nonce = intent.nonce.clone();
    let root_identity = *root_identity;
    let request_cancel = cancel.clone();
    state.download_jobs.submit_blocking(move |worker_cancel| {
        use crate::storage::FileIdentity;
        let meta = crate::storage::local::open_root(&root).and_then(|f| f.metadata()).map_err(|_| CHANGED)?;
        if (meta.dev(), meta.ino()) != root_identity { return Err(CHANGED); }
        let foreign = primary.into_iter().flat_map(|(a,k)| [a,k].into_iter().flatten().map(PathBuf::from))
            .chain(aliases.into_iter().flat_map(|(base,path,encrypted)| replica_paths(&path, encrypted.unwrap_or(false)).into_iter().map(move |path| PathBuf::from(&base).join(path))));
        for other in foreign {
            let absolute = std::path::absolute(&other).ok();
            let actual = std::fs::metadata(other).ok().and_then(|m| FileIdentity::read(&m).ok());
            if files.iter().any(|(path, identity)| absolute.as_ref() == Some(&root.join(path)) || (identity.is_some() && identity == &actual)) {
                return Err(Skipped("Copy is shared by another archive identity; retained for safety"));
            }
        }
        for (index, (path, stamp)) in files.into_iter().enumerate() {
            if worker_cancel.is_cancelled() || request_cancel.is_cancelled() { return Err(INCOMPLETE); }
            let name = path.to_str().ok_or(CHANGED)?;
            if crate::storage::relative_path(name, false).as_deref() != Some(name) { return Err(CHANGED); }
            if let Some(stamp) = stamp {
                delete_reviewed_leaf(&root, &path, stamp, &format!(".dv-removal-{nonce}-{index}")).map_err(|_| INCOMPLETE)?;
            } else if !matches!(crate::storage::local::open_download_source(&root, &path), Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
                return Err(CHANGED);
            }
        }
        Ok(())
    }).map_err(|_| INCOMPLETE)?.wait().await.map_err(|_| INCOMPLETE)??;
    let removed_sync = sqlx::query("DELETE FROM backup_storage_syncs WHERE id=?")
        .bind(replica.sync_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| INCOMPLETE)?;
    if removed_sync.rows_affected() != 1 {
        return Err(INCOMPLETE);
    }
    let empty: bool = sqlx::query_scalar("SELECT coalesce(file_path,'')='' AND coalesce(encryption_key_path,'')='' AND NOT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=?) FROM backups WHERE id=?")
        .bind(backup).bind(backup).fetch_one(&mut *tx).await.map_err(|_| INCOMPLETE)?;
    let ack = if empty {
        "DELETE FROM backups WHERE id=? AND json_extract(backup_metadata,'$.native_copy_removal')=?"
    } else {
        "UPDATE backups SET backup_metadata=json_remove(backup_metadata,'$.native_copy_removal') WHERE id=? AND json_extract(backup_metadata,'$.native_copy_removal')=?"
    };
    let acknowledged = sqlx::query(ack)
        .bind(backup)
        .bind(raw)
        .execute(&mut *tx)
        .await
        .map_err(|_| INCOMPLETE)?;
    if acknowledged.rows_affected() != 1 {
        return Err(INCOMPLETE);
    }
    tx.commit().await.map_err(|_| INCOMPLETE)
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
        return remove_replica(state, item, cancel).await;
    };
    let _claim = state
        .backup_claims
        .acquire([Resource::target(item.target_id)], cancel)
        .await
        .map_err(|_| INCOMPLETE)?;
    remove_claimed_primary(state, item, local, None, cancel).await
}

async fn remove_claimed_primary(
    state: &HttpState,
    item: &Item,
    local: &LocalCopy,
    retention_time: Option<i64>,
    cancel: &CancellationToken,
) -> Result<(), EntryError> {
    let Some(files) = local.files.clone() else {
        return Err(CHANGED);
    };
    let mut db = state.auth.pool().acquire().await.map_err(|_| INCOMPLETE)?;
    let mut tx = sqlx::Connection::begin_with(&mut *db, "BEGIN IMMEDIATE")
        .await
        .map_err(|_| INCOMPLETE)?;
    if let Some(now) = retention_time {
        let selection = super::super::retention::select(&mut tx, item.target_id, now)
            .await
            .map_err(|_| INCOMPLETE)?;
        if !selection.expired.contains(&item.backup_id) {
            return Err(CHANGED);
        }
    }
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
    let replicas = sqlx::query_as::<_, (Option<String>, String, Option<bool>)>("SELECT r.base_path,s.remote_path,b.encrypted FROM backup_storage_syncs s JOIN remote_storages r ON r.id=s.storage_id JOIN backups b ON b.id=s.backup_id WHERE r.storage_type='local' AND coalesce(s.remote_path,'')!=''")
        .fetch_all(&mut *tx).await.map_err(|_|INCOMPLETE)?;
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
            for (base, archive, encrypted) in replicas {
                let path = PathBuf::from(archive);
                let path = if path.is_absolute() {
                    path
                } else if let Some(base) = base {
                    PathBuf::from(base).join(path)
                } else {
                    continue;
                };
                let sidecar = if encrypted.unwrap_or(false) {
                    Some(PathBuf::from(dockervault_core::crypto::key_sidecar_path(
                        path.to_str().ok_or(CHANGED)?,
                    )))
                } else {
                    None
                };
                for path in std::iter::once(path).chain(sidecar) {
                    let resolved = std::fs::canonicalize(&path).ok();
                    if files.iter().any(|(relative, _)| {
                        let candidate = root.join(relative);
                        path == candidate || resolved.as_ref() == Some(&candidate)
                    }) {
                        return Err(Skipped(
                            "Copy is shared by another archive identity; retained for safety",
                        ));
                    }
                }
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
            if let Ok(intent) = serde_json::from_str::<ReplicaIntent>(&raw) {
                if settle_replica(state, id, &intent, &raw, &CancellationToken::new())
                    .await
                    .is_err()
                {
                    tracing::warn!(backup_id = id, "Unsettled LOCAL replica removal retained");
                }
                continue;
            }
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
    let acknowledged = if remote || retained_key.is_some() {
        sqlx::query("UPDATE backups SET file_path=NULL,encryption_key_path=?,backup_metadata=json_set(json_remove(backup_metadata,'$.native_copy_removal'),'$.local_copy_removed',json('true')) WHERE id=?")
            .bind(retained_key).bind(id).execute(&mut *tx).await
    } else {
        sqlx::query("DELETE FROM backups WHERE id=?")
            .bind(id)
            .execute(&mut *tx)
            .await
    }.map_err(|_| INCOMPLETE)?;
    if acknowledged.rows_affected() != 1 {
        return Err(INCOMPLETE);
    }
    tx.commit().await.map_err(|_| INCOMPLETE)
}
