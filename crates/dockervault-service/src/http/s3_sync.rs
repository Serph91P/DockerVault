//! S3 copy intents use the same compatible publication receipt as LOCAL.
use super::{
    HttpState,
    storage::{configured_browse_config, open_probe_credentials, s3_provider},
};
use crate::storage::{
    Publication, S3Copy,
    s3::{S3Error, S3UploadPolicy},
};
use serde_json::Value;
use sqlx::Error;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) async fn destination(state: &HttpState, id: i64, archive: String) -> Option<S3Copy> {
    let cfg = configured_browse_config(state, id).await.ok()?;
    from_config(&cfg, id, archive)
}

fn from_config(
    cfg: &serde_json::Map<String, Value>,
    storage_id: i64,
    archive: String,
) -> Option<S3Copy> {
    let get = |key| cfg.get(key).and_then(Value::as_str).map(str::to_owned);
    if get("storage_type").as_deref() != Some("s3")
        || cfg.get("enabled").and_then(Value::as_bool) != Some(true)
    {
        return None;
    }
    Some(S3Copy {
        storage_id,
        archive,
        endpoint: get("s3_endpoint_url"),
        bucket: get("s3_bucket")?,
        region: get("s3_region"),
        prefix: get("base_path"),
    })
}

pub(super) async fn settle(
    state: &HttpState,
    id: i64,
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let pool = state.auth.pool();
    let receipt:Option<String>=sqlx::query_scalar("SELECT json_extract(backup_metadata,'$.native_local_sync') FROM backups WHERE id=? AND status='COMPLETED'").bind(id).fetch_optional(pool).await?.flatten();
    let Some(receipt) = receipt else {
        return Ok(());
    };
    let proof: Publication =
        serde_json::from_str(&receipt).map_err(|e| Error::Decode(Box::new(e)))?;
    if proof.s3_copies.is_empty() {
        return Ok(());
    }
    for copy in &proof.s3_copies {
        let row:Option<i64>=sqlx::query_scalar("SELECT id FROM backup_storage_syncs WHERE backup_id=? AND storage_id=? AND remote_path=? AND sync_status='running'").bind(id).bind(copy.storage_id).bind(&copy.archive).fetch_optional(pool).await?;
        let Some(row) = row else {
            continue;
        };
        let Ok(mut cfg) = configured_browse_config(state, copy.storage_id).await else {
            continue;
        };
        if from_config(&cfg, copy.storage_id, copy.archive.clone()).as_ref() != Some(copy)
            || !open_probe_credentials(state, &mut cfg, None).await
        {
            continue;
        }
        let Some(provider) = s3_provider(state, &Value::Object(cfg)) else {
            continue;
        };
        if !(1..=2).contains(&proof.artifacts.len()) {
            return Err(Error::Protocol(
                "Invalid S3 publication artifact count".into(),
            ));
        }
        let mut sources = None;
        let mut verified = true;
        for (index, artifact) in proof.artifacts.iter().enumerate().rev() {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let path = if index == 0 {
                copy.archive.clone()
            } else {
                dockervault_core::crypto::key_sidecar_path(&copy.archive)
            };
            let effect = format!(
                "$.native_s3_effects.destination_{}.artifact_{}",
                copy.storage_id, index
            );
            let mut digest = [0u8; 32];
            if artifact.digest.len() != 64
                || !artifact.digest.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(Error::Protocol("Invalid S3 publication digest".into()));
            }
            for (j, b) in digest.iter_mut().enumerate() {
                *b = u8::from_str_radix(&artifact.digest[j * 2..j * 2 + 2], 16)
                    .map_err(|e| Error::Decode(Box::new(e)))?;
            }
            let previous: bool = sqlx::query_scalar(
                "SELECT json_type(backup_metadata,?) IS NOT NULL FROM backups WHERE id=?",
            )
            .bind(&effect)
            .bind(id)
            .fetch_one(pool)
            .await?;
            if !previous {
                if sources.is_none() {
                    let artifacts = proof.artifacts.clone();
                    let owned = cancel.clone();
                    sources = state
                        .auth
                        .blocking()
                        .submit_blocking(move |worker| {
                            artifacts
                                .iter()
                                .map(|entry| {
                                    entry
                                        .open_verified(&|| {
                                            worker.is_cancelled() || owned.is_cancelled()
                                        })
                                        .map(Arc::new)
                                })
                                .collect::<std::io::Result<Vec<_>>>()
                        })
                        .map_err(work_error)?
                        .wait()
                        .await
                        .map_err(work_error)?
                        .ok();
                }
                let Some(source) = sources.as_ref().map(|files| files[index].clone()) else {
                    verified = false;
                    break;
                };
                let changed=sqlx::query("UPDATE backups SET backup_metadata=json_set(backup_metadata,?,json('{}'))
                    WHERE id=? AND status='COMPLETED' AND json_type(backup_metadata,?) IS NULL AND json_type(backup_metadata,'$.native_local_sync')='object'
                    AND EXISTS (SELECT 1 FROM backup_storage_syncs s JOIN remote_storages r ON r.id=s.storage_id
                        WHERE s.id=? AND s.backup_id=backups.id AND s.storage_id=? AND s.remote_path=? AND s.sync_status='running'
                        AND r.storage_type='s3' AND r.enabled=1 AND r.s3_bucket=? AND r.s3_region IS ? AND r.s3_endpoint_url IS ? AND r.base_path IS ?)")
                    .bind(&effect).bind(id).bind(&effect).bind(row).bind(copy.storage_id).bind(&copy.archive)
                    .bind(&copy.bucket).bind(&copy.region).bind(&copy.endpoint).bind(&copy.prefix).execute(pool).await?;
                if changed.rows_affected() != 1 {
                    return Err(Error::RowNotFound);
                }
                let checkpoint_pool = pool.clone();
                let checkpoint = format!("{effect}.multipart_id");
                let work=provider.upload_checkpointed(source,path.clone(),S3UploadPolicy::default(),move |upload_id|async move {
                    let changed=sqlx::query("UPDATE backups SET backup_metadata=json_set(backup_metadata,?,?)
                        WHERE id=? AND status='COMPLETED' AND json_type(backup_metadata,?)='object' AND json_type(backup_metadata,?) IS NULL
                        AND EXISTS(SELECT 1 FROM backup_storage_syncs WHERE id=? AND backup_id=backups.id AND sync_status='running')")
                        .bind(&checkpoint).bind(upload_id).bind(id).bind(effect).bind(&checkpoint).bind(row).execute(&checkpoint_pool).await.map_err(|_|S3Error::Io)?;
                    if changed.rows_affected()!=1 {return Err(S3Error::Io);} Ok(())
                }).map_err(work_error)?;
                let outcome = wait(work, cancel).await?;
                if outcome.is_err() {
                    verified = false;
                    break;
                }
            } else {
                let checkpoint = format!("{effect}.multipart_id");
                let upload_id: Option<String> = sqlx::query_scalar(
                    "SELECT json_extract(backup_metadata,?) FROM backups WHERE id=?",
                )
                .bind(checkpoint)
                .bind(id)
                .fetch_one(pool)
                .await?;
                if let Some(upload_id) = upload_id {
                    let work = provider
                        .settle_multipart(path.clone(), upload_id)
                        .map_err(work_error)?;
                    if wait(work, cancel).await?.is_err() {
                        verified = false;
                        break;
                    }
                }
                let work = provider
                    .verify_object(path, artifact.length, digest)
                    .map_err(work_error)?;
                if !matches!(wait(work, cancel).await?, Ok(true)) {
                    verified = false;
                    break;
                }
            }
        }
        if verified && !cancel.is_cancelled() {
            sqlx::query("UPDATE backup_storage_syncs SET sync_status='completed',synced_at=strftime('%Y-%m-%d %H:%M:%f000','now') WHERE id=? AND sync_status='running' AND EXISTS(SELECT 1 FROM remote_storages WHERE id=? AND storage_type='s3' AND enabled=1 AND s3_bucket=? AND s3_region IS ? AND s3_endpoint_url IS ? AND base_path IS ?)")
                .bind(row).bind(copy.storage_id).bind(&copy.bucket).bind(&copy.region).bind(&copy.endpoint).bind(&copy.prefix).execute(pool).await?;
        }
    }
    Ok(())
}

pub async fn recover_s3_copies(state: &HttpState) -> Result<(), Error> {
    let mut after = i64::MIN;
    loop {
        let ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM backups WHERE id>? AND status='COMPLETED' AND CASE WHEN json_valid(backup_metadata) THEN json_type(backup_metadata,'$.native_local_sync.s3_copies')='array' ELSE 0 END ORDER BY id LIMIT 64").bind(after).fetch_all(state.auth.pool()).await?;
        if ids.is_empty() {
            return Ok(());
        }
        for id in ids {
            after = id;
            settle(state, id, &state.draining).await?;
            crate::storage::settle_local_copies(
                state.auth.pool(),
                state.auth.blocking(),
                id,
                &state.draining,
            )
            .await?;
        }
    }
}

async fn wait<T>(work: crate::ownership::Work<T>, cancel: &CancellationToken) -> Result<T, Error> {
    let stop = work.cancellation_token();
    let waiting = work.wait();
    tokio::pin!(waiting);
    tokio::select! { result=&mut waiting=>result, _=cancel.cancelled()=>{stop.cancel(); waiting.await} }.map_err(work_error)
}

fn work_error(error: crate::ownership::WorkError) -> Error {
    Error::Io(std::io::Error::other(error))
}
