//! Native LOCAL copy obligations share the archive publication transaction.
use super::{FileIdentity, Publication, VerifiedSource, check, invalid, local};
use crate::ownership::Owner;
use serde::{Deserialize, Serialize};
use sqlx::{Error, SqlitePool};
use std::{
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize, Serialize)]
pub(crate) struct LocalCopy {
    pub storage_id: i64,
    pub root: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub archive: String,
}

pub(crate) async fn complete_publication(
    pool: &SqlitePool,
    id: i64,
    proof: &Publication,
) -> Result<(), Error> {
    let archive = proof
        .artifacts
        .first()
        .ok_or_else(|| Error::Protocol("Missing publication archive".into()))?;
    let path = archive
        .path
        .to_str()
        .ok_or_else(|| Error::Protocol("Invalid publication path".into()))?;
    let length = i64::try_from(archive.length).map_err(|e| Error::Encode(Box::new(e)))?;
    let sidecar = proof.artifacts.get(1).and_then(|key| key.path.to_str());
    let sync = (!proof.local_copies.is_empty())
        .then(|| serde_json::to_string(proof))
        .transpose()
        .map_err(|e| Error::Encode(Box::new(e)))?;
    let mut tx = pool.begin().await?;
    let update = sqlx::query("UPDATE backups SET status='COMPLETED',file_path=?,file_size=?,checksum=?,encrypted=?,encryption_key_path=?,
        completed_at=strftime('%Y-%m-%d %H:%M:%f000','now'),error_message=NULL,
        duration_seconds=max(0,cast((julianday('now')-julianday(started_at))*86400 as integer)),
        backup_metadata=json_patch(json_remove(backup_metadata,'$.active_artifacts','$.native_ephemeral_archive','$.native_publication'),json_object('native_local_sync',json(?)))
        WHERE id=? AND status='RUNNING' AND file_path IS NULL AND json_extract(backup_metadata,'$.native_publication.token')=?")
        .bind(path).bind(length).bind(&proof.checksum).bind(sidecar.is_some()).bind(sidecar)
        .bind(sync).bind(id).bind(&proof.token).execute(&mut *tx).await?;
    if update.rows_affected() != 1 {
        return Err(Error::RowNotFound);
    }
    for copy in &proof.local_copies {
        let write = sqlx::query("INSERT INTO backup_storage_syncs(backup_id,storage_id,remote_path,sync_status) VALUES(?,?,?,'running')")
            .bind(id).bind(copy.storage_id).bind(&copy.archive).execute(&mut *tx).await?;
        if write.rows_affected() != 1 {
            return Err(Error::RowNotFound);
        }
    }
    tx.commit().await
}

pub(crate) async fn settle_local_copies(
    pool: &SqlitePool,
    owner: &Owner,
    id: i64,
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let receipt: Option<String> = sqlx::query_scalar("SELECT json_extract(backup_metadata,'$.native_local_sync') FROM backups WHERE id=? AND status='COMPLETED'")
        .bind(id).fetch_optional(pool).await?.flatten();
    let Some(receipt) = receipt else {
        return Ok(());
    };
    let proof: Publication =
        serde_json::from_str(&receipt).map_err(|e| Error::Decode(Box::new(e)))?;
    for copy in proof.local_copies {
        let pending: Option<i64> = sqlx::query_scalar("SELECT id FROM backup_storage_syncs WHERE backup_id=? AND storage_id=? AND remote_path=? AND sync_status='running'")
            .bind(id).bind(copy.storage_id).bind(&copy.archive).fetch_optional(pool).await?;
        let Some(pending) = pending else {
            continue;
        };
        let current: Option<String> = sqlx::query_scalar("SELECT base_path FROM remote_storages WHERE id=? AND enabled=1 AND storage_type='local' AND base_path IS NOT NULL")
            .bind(copy.storage_id).fetch_optional(pool).await?;
        if current
            .as_deref()
            .and_then(|base| std::path::absolute(base).ok())
            .as_ref()
            != Some(&copy.root)
        {
            tracing::warn!(
                backup_id = id,
                storage_id = copy.storage_id,
                "Pending LOCAL copy: storage configuration changed"
            );
            continue;
        }
        let job_cancel = cancel.clone();
        let (storage_id, path) = (copy.storage_id, copy.archive.clone());
        let artifacts = proof.artifacts.clone();
        let result = owner
            .submit_blocking(move |worker_cancel| {
                let cancelled = || job_cancel.is_cancelled() || worker_cancel.is_cancelled();
                check(&cancelled)?;
                // Validate both artifacts against the publication receipt before writing even a sidecar.
                let mut sources = Vec::new();
                for entry in &artifacts {
                    let source = VerifiedSource::open(&entry.path, &cancelled)?;
                    let identity: FileIdentity = source.identity;
                    if identity.device != entry.device
                        || identity.inode != entry.inode
                        || source.len() != entry.length
                        || source.sha256_hex() != entry.digest
                    {
                        return Err(invalid(
                            "Local recovery point no longer matches the publication receipt",
                        ));
                    }
                    sources.push(source);
                }
                let remote = Path::new(&copy.archive);
                let current = local::open_root(&copy.root)?;
                let meta = current.metadata()?;
                if (meta.dev(), meta.ino()) != (copy.device, copy.inode) {
                    return Err(invalid("LOCAL destination root changed"));
                }
                for (index, source) in sources.iter().enumerate().rev() {
                    let target = if index == 0 {
                        remote.to_owned()
                    } else {
                        PathBuf::from(dockervault_core::crypto::key_sidecar_path(&copy.archive))
                    };
                    local::ensure_copy(&current, &target, source, &cancelled)?;
                }
                let final_meta = local::open_root(&copy.root)?.metadata()?;
                if (final_meta.dev(), final_meta.ino()) != (copy.device, copy.inode) {
                    return Err(invalid("LOCAL destination root changed"));
                }
                Ok::<_, io::Error>(())
            })
            .map_err(work_error)?
            .wait()
            .await
            .map_err(work_error)?;
        if let Err(error) = result {
            tracing::warn!(backup_id=id, storage_id, %error, "LOCAL copy remains unsettled");
            continue;
        }
        let update = sqlx::query("UPDATE backup_storage_syncs SET sync_status='completed',error_message=NULL,synced_at=strftime('%Y-%m-%d %H:%M:%f000','now')
            WHERE id=? AND backup_id=? AND storage_id=? AND remote_path=? AND sync_status='running'
            AND EXISTS (SELECT 1 FROM remote_storages r WHERE r.id=backup_storage_syncs.storage_id AND r.enabled=1 AND r.storage_type='local' AND r.base_path=?)")
            .bind(pending).bind(id).bind(storage_id).bind(path).bind(current).execute(pool).await?;
        if update.rows_affected() != 1 {
            tracing::warn!(
                backup_id = id,
                storage_id,
                "LOCAL copy could not settle: its intent or destination changed"
            );
        }
    }
    // A copy receipt is not a local cleanup receipt. Keep the obligation until
    // catalog recovery and crash-safe deletion of both local artifacts can settle it.
    if !proof.delete_local_after_sync {
        sqlx::query("UPDATE backups SET backup_metadata=json_remove(backup_metadata,'$.native_local_sync')
            WHERE id=? AND NOT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=? AND sync_status='running')")
            .bind(id).bind(id).execute(pool).await?;
    }
    Ok(())
}

pub(super) async fn reconcile_local_copies(pool: &SqlitePool, owner: &Owner) -> Result<(), Error> {
    let mut after = i64::MIN;
    let cancel = CancellationToken::new();
    loop {
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM backups WHERE id>? AND status='COMPLETED' AND CASE WHEN json_valid(backup_metadata) THEN json_type(backup_metadata,'$.native_local_sync')='object' ELSE 0 END ORDER BY id LIMIT 64")
            .bind(after).fetch_all(pool).await?;
        if ids.is_empty() {
            return Ok(());
        }
        for id in ids {
            settle_local_copies(pool, owner, id, &cancel).await?;
            after = id;
        }
    }
}

fn work_error(error: crate::ownership::WorkError) -> Error {
    Error::Io(io::Error::other(error))
}
