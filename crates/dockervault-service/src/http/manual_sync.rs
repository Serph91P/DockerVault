//! A manual LOCAL copy reserves and settles through the existing publication journal.
use super::{HttpState, backup_submit::safe_name};
use crate::{
    jobs::Resource,
    storage::{
        LocalCopy, Publication, PublishedArtifact, local, relative_path, settle_local_copies,
    },
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::os::unix::fs::MetadataExt;
use tokio_util::sync::CancellationToken;

type Failure = (StatusCode, &'static str);
const BUSY: Failure = (StatusCode::CONFLICT, "Backup is not available for re-sync");
const TRANSFER: Failure = (StatusCode::BAD_GATEWAY, "Sync failed");
const DATABASE: Failure = (StatusCode::INTERNAL_SERVER_ERROR, "Sync failed");
const MISSING_ARCHIVE: Failure = (StatusCode::BAD_REQUEST, "Local backup file not available");
const MISSING_KEY: Failure = (
    StatusCode::BAD_REQUEST,
    "Encryption key sidecar not available",
);
const CONFLICT: Failure = (
    StatusCode::CONFLICT,
    "Remote path belongs to another backup",
);

#[derive(PartialEq)]
struct Storage {
    name: String,
    storage_type: String,
    enabled: bool,
    base_path: Option<String>,
}

#[derive(PartialEq)]
struct Backup {
    target_id: i64,
    status: String,
    file_path: Option<String>,
    encrypted: bool,
    encryption_key_path: Option<String>,
    checksum: Option<String>,
    backup_metadata: Option<String>,
}

async fn storage(db: &mut sqlx::SqliteConnection, id: i64) -> Result<Storage, Failure> {
    let (name, storage_type, enabled, base_path) = sqlx::query_as("SELECT name,storage_type,coalesce(enabled,0) AS enabled,base_path FROM remote_storages WHERE id=?")
        .bind(id).fetch_optional(db).await.map_err(|_| DATABASE)?
        .ok_or((StatusCode::NOT_FOUND, "Storage not found"))?;
    let row = Storage {
        name,
        storage_type,
        enabled,
        base_path,
    };
    if !row.enabled {
        return Err((StatusCode::CONFLICT, "Storage is disabled"));
    }
    Ok(row)
}

async fn backup(db: &mut sqlx::SqliteConnection, id: i64) -> Result<Backup, Failure> {
    let (target_id, status, file_path, encrypted, encryption_key_path, checksum, backup_metadata) = sqlx::query_as("SELECT target_id,status,file_path,coalesce(encrypted,0) AS encrypted,encryption_key_path,checksum,backup_metadata FROM backups WHERE id=?")
        .bind(id).fetch_optional(db).await.map_err(|_| DATABASE)?
        .ok_or((StatusCode::NOT_FOUND, "Backup not found"))?;
    let row = Backup {
        target_id,
        status,
        file_path,
        encrypted,
        encryption_key_path,
        checksum,
        backup_metadata,
    };
    if row.status != "COMPLETED" {
        return Err((StatusCode::CONFLICT, "Only completed backups can be synced"));
    }
    Ok(row)
}

pub(super) async fn sync(
    State(state): State<HttpState>,
    Path((storage_id, backup_id)): Path<(i64, i64)>,
) -> Response {
    let ctx = state.clone();
    let result = match state
        .backup_jobs
        .submit_async(move |cancel| async move { run(&ctx, storage_id, backup_id, &cancel).await })
    {
        Ok(work) => work.wait().await.unwrap_or(Err(BUSY)),
        Err(_) => Err(BUSY),
    };
    match result {
        Ok((name, path)) => {
            Json(json!({"message":format!("Backup synced to {name}"),"remote_path":path}))
                .into_response()
        }
        Err((status, detail)) => (status, Json(json!({"detail":detail}))).into_response(),
    }
}

async fn run(
    state: &HttpState,
    storage_id: i64,
    backup_id: i64,
    cancel: &CancellationToken,
) -> Result<(String, String), Failure> {
    let pool = state.auth.pool();
    let mut db = pool.acquire().await.map_err(|_| DATABASE)?;
    storage(&mut db, storage_id).await?;
    let initial = backup(&mut db, backup_id).await?;
    drop(db);
    let _claim = state
        .backup_claims
        .acquire([Resource::target(initial.target_id)], cancel)
        .await
        .map_err(|_| BUSY)?;
    let mut db = pool.acquire().await.map_err(|_| DATABASE)?;
    let destination = storage(&mut db, storage_id).await?;
    let row = backup(&mut db, backup_id).await?;
    if initial.target_id != row.target_id {
        return Err(BUSY);
    }
    let target: String = sqlx::query_scalar("SELECT name FROM backup_targets WHERE id=?")
        .bind(row.target_id)
        .fetch_optional(&mut *db)
        .await
        .map_err(|_| DATABASE)?
        .ok_or((StatusCode::NOT_FOUND, "Backup target not found"))?;
    drop(db);
    let archive = row
        .file_path
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or(MISSING_ARCHIVE)?;
    let root = std::path::absolute(&state.config.backup_base_path).map_err(|_| MISSING_ARCHIVE)?;
    let archive = std::path::absolute(archive).map_err(|_| MISSING_ARCHIVE)?;
    let leaf = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(MISSING_ARCHIVE)?;
    let relative =
        relative_path(&format!("{}/{leaf}", safe_name(&target)), false).ok_or(CONFLICT)?;
    if destination.storage_type != "local" {
        return Err(TRANSFER);
    }
    let base = destination
        .base_path
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or(TRANSFER)?;
    let destination_root = std::path::absolute(base).map_err(|_| TRANSFER)?;
    let key = if row.encrypted {
        Some(
            std::path::absolute(
                row.encryption_key_path
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .ok_or(MISSING_KEY)?,
            )
            .map_err(|_| MISSING_KEY)?,
        )
    } else {
        None
    };
    let original = row
        .backup_metadata
        .as_deref()
        .map(serde_json::from_str::<serde_json::Value>)
        .transpose()
        .map_err(|_| BUSY)?;
    if original.as_ref().is_some_and(|value| !value.is_object()) {
        return Err(BUSY);
    }
    let pending = original
        .as_ref()
        .and_then(|value| value.get("native_local_sync"));
    let owner = state.auth.blocking();
    if let Some(pending) = pending {
        let proof: Publication = serde_json::from_value(pending.clone()).map_err(|_| BUSY)?;
        if !original
            .as_ref()
            .is_some_and(|value| value.get("native_manual_sync_original").is_some())
            || proof.delete_local_after_sync
            || proof.local_copies.len() != 1
            || proof.local_copies[0].storage_id != storage_id
            || proof.local_copies[0].archive != relative
            || proof.local_copies[0].root != destination_root
        {
            return Err(BUSY);
        }
    } else {
        let cancellation = cancel.clone();
        let remote_path = relative.clone();
        let proof = owner
            .submit_blocking(move |physical| {
                let cancelled = || cancellation.is_cancelled() || physical.is_cancelled();
                let primary = PublishedArtifact::snapshot(&root, &archive, &cancelled)
                    .map_err(|_| MISSING_ARCHIVE)?;
                let mut artifacts = vec![primary];
                if let Some(key) = key {
                    artifacts.push(
                        PublishedArtifact::snapshot(&root, &key, &cancelled)
                            .map_err(|_| MISSING_KEY)?,
                    );
                }
                let root_info = local::open_root(&destination_root)
                    .and_then(|file| file.metadata())
                    .map_err(|_| TRANSFER)?;
                Ok::<_, Failure>(Publication {
                    token: String::new(),
                    checksum: artifacts[0].digest.clone(),
                    artifacts,
                    local_copies: vec![LocalCopy {
                        storage_id,
                        root: destination_root,
                        device: root_info.dev(),
                        inode: root_info.ino(),
                        archive: remote_path,
                    }],
                    s3_copies: Vec::new(),
                    webdav_copies: Vec::new(),
                    sftp_copies: Vec::new(),
                    ftp_copies: Vec::new(),
                    delete_local_after_sync: false,
                })
            })
            .map_err(|_| BUSY)?
            .wait()
            .await
            .map_err(|_| BUSY)??;
        if !row.encrypted
            && row
                .checksum
                .as_ref()
                .is_some_and(|checksum| !checksum.eq_ignore_ascii_case(&proof.checksum))
        {
            return Err(TRANSFER);
        }
        let proof = serde_json::to_string(&proof).map_err(|_| DATABASE)?;
        let mut tx = pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|_| DATABASE)?;
        if cancel.is_cancelled()
            || storage(&mut tx, storage_id).await? != destination
            || backup(&mut tx, backup_id).await? != row
        {
            return Err(BUSY);
        }
        let conflict: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE storage_id=? AND remote_path=? AND backup_id!=?)")
            .bind(storage_id).bind(&relative).bind(backup_id).fetch_one(&mut *tx).await.map_err(|_| DATABASE)?;
        if conflict {
            return Err(CONFLICT);
        }
        let existing: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as("SELECT id,remote_path,sync_status FROM backup_storage_syncs WHERE backup_id=? AND storage_id=? LIMIT 2")
            .bind(backup_id).bind(storage_id).fetch_all(&mut *tx).await.map_err(|_| DATABASE)?;
        match existing.as_slice() {
            [] => {
                sqlx::query("INSERT INTO backup_storage_syncs(backup_id,storage_id,remote_path,sync_status) VALUES(?,?,?,'running')")
                .bind(backup_id).bind(storage_id).bind(&relative).execute(&mut *tx).await.map_err(|_| DATABASE)?;
            }
            [(id, path, Some(status))]
                if path.as_deref() == Some(&relative)
                    && matches!(status.as_str(), "completed" | "failed") =>
            {
                sqlx::query("UPDATE backup_storage_syncs SET sync_status='running',error_message=NULL WHERE id=?")
                    .bind(id).execute(&mut *tx).await.map_err(|_| DATABASE)?;
            }
            _ => return Err(BUSY),
        }
        sqlx::query("UPDATE backups SET backup_metadata=json_set(coalesce(backup_metadata,'{}'),'$.native_local_sync',json(?),'$.native_manual_sync_original',backup_metadata) WHERE id=?")
            .bind(proof).bind(backup_id).execute(&mut *tx).await.map_err(|_| DATABASE)?;
        tx.commit().await.map_err(|_| DATABASE)?;
    }
    settle_local_copies(pool, owner, backup_id, cancel)
        .await
        .map_err(|_| TRANSFER)?;
    let completed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_storage_syncs s JOIN backups b ON b.id=s.backup_id WHERE s.backup_id=? AND s.storage_id=? AND s.remote_path=? AND s.sync_status='completed' AND json_type(b.backup_metadata,'$.native_local_sync') IS NULL)")
        .bind(backup_id).bind(storage_id).bind(&relative).fetch_one(pool).await.map_err(|_| DATABASE)?;
    if completed {
        Ok((destination.name, relative))
    } else {
        Err(TRANSFER)
    }
}
