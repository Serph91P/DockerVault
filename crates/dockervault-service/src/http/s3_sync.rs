//! S3 copy intents use the same compatible publication receipt as LOCAL.
use super::{
    HttpState,
    auth::required_user,
    storage::{configured_browse_config, open_probe_credentials, s3_provider},
};
use crate::storage::{
    Publication, S3Copy,
    s3::{S3Error, S3UploadPolicy},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
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
    settle_mode(state, id, cancel, false).await
}

async fn settle_mode(
    state: &HttpState,
    id: i64,
    cancel: &CancellationToken,
    manual_retry: bool,
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
        'artifacts: for (index, artifact) in proof.artifacts.iter().enumerate().rev() {
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
            let digest = parse_digest(&artifact.digest)?;
            loop {
                if cancel.is_cancelled() {
                    return Ok(());
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
                        break 'artifacts;
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
                        break 'artifacts;
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
                    if let Some(upload_id) = upload_id.as_ref() {
                        let work = provider
                            .settle_multipart(path.clone(), upload_id.clone())
                            .map_err(work_error)?;
                        if wait(work, cancel).await?.is_err() {
                            verified = false;
                            break 'artifacts;
                        }
                    }
                    let work = provider
                        .verify_object(path.clone(), artifact.length, digest)
                        .map_err(work_error)?;
                    match wait(work, cancel).await? {
                        Ok(true) => (),
                        Err(error)
                            if manual_retry
                                && !cancel.is_cancelled()
                                && error.http_status() == Some(404)
                                && error.provider_code() == Some("NoSuchKey")
                                && upload_id.is_some() =>
                        {
                            // Only rearm after settling the old upload and confirming absence of the exact object.
                            let changed = sqlx::query("UPDATE backups SET backup_metadata=json_remove(backup_metadata,?) WHERE id=? AND json_extract(backup_metadata,?)=? AND EXISTS(SELECT 1 FROM backup_storage_syncs WHERE id=? AND backup_id=backups.id AND sync_status='running')")
                            .bind(&effect).bind(id).bind(format!("{effect}.multipart_id")).bind(upload_id).bind(row).execute(pool).await?;
                            if changed.rows_affected() != 1 {
                                return Err(Error::RowNotFound);
                            }
                            continue;
                        }
                        _ => {
                            verified = false;
                            break 'artifacts;
                        }
                    }
                }
                break;
            }
        }
        if verified && !cancel.is_cancelled() {
            sqlx::query("UPDATE backup_storage_syncs SET sync_status='completed',synced_at=strftime('%Y-%m-%d %H:%M:%f000','now') WHERE id=? AND sync_status='running' AND EXISTS(SELECT 1 FROM remote_storages WHERE id=? AND storage_type='s3' AND enabled=1 AND s3_bucket=? AND s3_region IS ? AND s3_endpoint_url IS ? AND base_path IS ?)")
                .bind(row).bind(copy.storage_id).bind(&copy.bucket).bind(&copy.region).bind(&copy.endpoint).bind(&copy.prefix).execute(pool).await?;
        }
    }
    Ok(())
}

pub(super) async fn retry(
    State(state): State<HttpState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = required_user(&state, &headers).await {
        return *response;
    }
    let worker = state.clone();
    let Ok(work) = state
        .backup_jobs
        .submit_async(move |cancel| async move { retry_owned(&worker, id, &cancel).await })
    else {
        return retry_error(StatusCode::SERVICE_UNAVAILABLE, "Backup capacity exhausted");
    };
    match work.wait().await {
        Ok(Ok(response)) => response,
        _ => retry_error(StatusCode::INTERNAL_SERVER_ERROR, "Backup operation failed"),
    }
}

async fn retry_owned(
    state: &HttpState,
    id: i64,
    cancel: &CancellationToken,
) -> Result<Response, Error> {
    let pool = state.auth.pool();
    let target: Option<i64> = sqlx::query_scalar("SELECT target_id FROM backups WHERE id=?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let Some(target) = target else {
        return Ok(retry_error(StatusCode::NOT_FOUND, "Backup not found"));
    };
    let Ok(_claim) = state
        .backup_claims
        .acquire([crate::jobs::Resource::target(target)], cancel)
        .await
    else {
        return Ok(retry_error(
            StatusCode::CONFLICT,
            "Backup is not available for re-sync",
        ));
    };
    let metadata: Option<String> = sqlx::query_scalar("SELECT CASE WHEN json_valid(backup_metadata) THEN json_extract(backup_metadata,'$.native_local_sync') END FROM backups WHERE id=? AND status='COMPLETED'").bind(id).fetch_optional(pool).await?.flatten();
    let Some(metadata) = metadata else {
        return Ok(retry_error(
            StatusCode::BAD_REQUEST,
            "No failed or missing sync records to retry",
        ));
    };
    let proof: Publication =
        serde_json::from_str(&metadata).map_err(|e| Error::Decode(Box::new(e)))?;
    let before = super::backups::sync_details(state, id).await?;
    let pending: Vec<_> = proof
        .s3_copies
        .iter()
        .filter(|copy| {
            before.iter().any(|row| {
                row["storage_id"] == copy.storage_id
                    && row["remote_path"] == copy.archive
                    && row["status"] == "running"
            })
        })
        .collect();
    if pending.is_empty() {
        return Ok(retry_error(
            StatusCode::BAD_REQUEST,
            "No failed or missing sync records to retry",
        ));
    }
    settle_mode(state, id, cancel, true).await?;
    let after = super::backups::sync_details(state, id).await?;
    let succeeded = pending
        .iter()
        .filter(|copy| {
            after.iter().any(|row| {
                row["storage_id"] == copy.storage_id
                    && row["remote_path"] == copy.archive
                    && row["status"] == "completed"
            })
        })
        .count();
    if succeeded == 0 {
        return Ok(retry_error(
            StatusCode::CONFLICT,
            "Remote sync has an unsettled provider effect",
        ));
    }
    let failed = pending.len() - succeeded;
    finalize_copies(state, id, cancel).await?;
    Ok(Json(json!({"message":format!("Retry complete: {succeeded} succeeded, {failed} failed"),"succeeded":succeeded,"failed":failed,"sync_details":after})).into_response())
}

fn retry_error(status: StatusCode, detail: &'static str) -> Response {
    (status, Json(json!({"detail":detail}))).into_response()
}

pub(super) async fn finalize_copies(
    state: &HttpState,
    id: i64,
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let pool = state.auth.pool();
    let receipt: Option<String> = sqlx::query_scalar("SELECT CASE WHEN json_valid(backup_metadata) THEN json_extract(backup_metadata,'$.native_local_sync') END FROM backups WHERE id=? AND status='COMPLETED'")
        .bind(id).fetch_optional(pool).await?.flatten();
    let Some(receipt) = receipt else {
        return Ok(());
    };
    let proof: Publication =
        serde_json::from_str(&receipt).map_err(|e| Error::Decode(Box::new(e)))?;
    let mut verified = false;
    if proof.delete_local_after_sync {
        verified = verify_cleanup_destinations(state, id, &proof, cancel).await?
            && super::webdav_sync::verify_cleanup_destinations(state, id, &proof, cancel).await?;
    }
    crate::storage::settle_verified_copies(
        pool,
        state.auth.blocking(),
        id,
        cancel,
        verified.then_some(receipt.as_str()),
    )
    .await
}

async fn verify_cleanup_destinations(
    state: &HttpState,
    id: i64,
    proof: &Publication,
    cancel: &CancellationToken,
) -> Result<bool, Error> {
    if !(1..=2).contains(&proof.artifacts.len()) {
        return Ok(false);
    }
    let pool = state.auth.pool();
    for copy in &proof.s3_copies {
        let completed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=? AND storage_id=? AND remote_path=? AND sync_status='completed')")
            .bind(id).bind(copy.storage_id).bind(&copy.archive).fetch_one(pool).await?;
        if !completed || cancel.is_cancelled() {
            return Ok(false);
        }
        let Ok(mut cfg) = configured_browse_config(state, copy.storage_id).await else {
            return Ok(false);
        };
        if from_config(&cfg, copy.storage_id, copy.archive.clone()).as_ref() != Some(copy)
            || !open_probe_credentials(state, &mut cfg, None).await
        {
            return Ok(false);
        }
        let Some(provider) = s3_provider(state, &Value::Object(cfg)) else {
            return Ok(false);
        };
        for (index, artifact) in proof.artifacts.iter().enumerate() {
            if cancel.is_cancelled() {
                return Ok(false);
            }
            let path = if index == 0 {
                copy.archive.clone()
            } else {
                dockervault_core::crypto::key_sidecar_path(&copy.archive)
            };
            let work = provider
                .verify_object(path, artifact.length, parse_digest(&artifact.digest)?)
                .map_err(work_error)?;
            if !matches!(wait(work, cancel).await?, Ok(true)) {
                return Ok(false);
            }
        }
    }
    Ok(!cancel.is_cancelled())
}

pub(super) fn parse_digest(value: &str) -> Result<[u8; 32], Error> {
    let mut digest = [0u8; 32];
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Protocol("Invalid S3 publication digest".into()));
    }
    for (j, b) in digest.iter_mut().enumerate() {
        *b = u8::from_str_radix(&value[j * 2..j * 2 + 2], 16)
            .map_err(|e| Error::Decode(Box::new(e)))?;
    }
    Ok(digest)
}

pub async fn recover_s3_copies(state: &HttpState) -> Result<(), Error> {
    let mut after = i64::MIN;
    loop {
        let ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM backups WHERE id>? AND status='COMPLETED' AND CASE WHEN json_valid(backup_metadata) THEN json_type(backup_metadata,'$.native_local_sync.s3_copies')='array' OR json_type(backup_metadata,'$.native_local_sync.webdav_copies')='array' ELSE 0 END ORDER BY id LIMIT 64").bind(after).fetch_all(state.auth.pool()).await?;
        if ids.is_empty() {
            return Ok(());
        }
        for id in ids {
            after = id;
            settle(state, id, &state.draining).await?;
            super::webdav_sync::settle(state, id, &state.draining).await?;
            finalize_copies(state, id, &state.draining).await?;
        }
    }
}

pub(super) async fn wait<T>(
    work: crate::ownership::Work<T>,
    cancel: &CancellationToken,
) -> Result<T, Error> {
    let stop = work.cancellation_token();
    let waiting = work.wait();
    tokio::pin!(waiting);
    tokio::select! { result=&mut waiting=>result, _=cancel.cancelled()=>{stop.cancel(); waiting.await} }.map_err(work_error)
}

pub(super) fn work_error(error: crate::ownership::WorkError) -> Error {
    Error::Io(std::io::Error::other(error))
}
