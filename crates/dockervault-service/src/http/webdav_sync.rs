//! Journaled WebDAV copy publication and recovery readback.
use super::{
    HttpState,
    s3_sync::{parse_digest, wait, work_error},
    storage::{configured_browse_config, configured_webdav, open_probe_credentials},
};
use crate::storage::{Publication, WebDavCopy};
use serde_json::{Map, Value};
use sqlx::Error;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn from_config(cfg: &Map<String, Value>, storage_id: i64, archive: String) -> Option<WebDavCopy> {
    let get = |key| cfg.get(key).and_then(Value::as_str).map(str::to_owned);
    if get("storage_type").as_deref() != Some("webdav")
        || cfg.get("enabled").and_then(Value::as_bool) != Some(true)
    {
        return None;
    }
    let endpoint = get("webdav_url")?;
    let parsed = reqwest::Url::parse(&endpoint).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(WebDavCopy {
        storage_id,
        archive,
        endpoint,
        prefix: get("base_path"),
        username: get("username"),
    })
}

pub(super) async fn destination(state: &HttpState, id: i64, archive: String) -> Option<WebDavCopy> {
    from_config(
        &configured_browse_config(state, id).await.ok()?,
        id,
        archive,
    )
}

pub(super) async fn verify_cleanup_destinations(
    state: &HttpState,
    id: i64,
    proof: &Publication,
    cancel: &CancellationToken,
) -> Result<bool, Error> {
    if !(1..=2).contains(&proof.artifacts.len()) {
        return Ok(false);
    }
    for copy in &proof.webdav_copies {
        let completed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=? AND storage_id=? AND remote_path=? AND sync_status='completed')")
            .bind(id).bind(copy.storage_id).bind(&copy.archive).fetch_one(state.auth.pool()).await?;
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
        let Some(provider) = configured_webdav(state, &cfg) else {
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

pub(super) async fn settle(
    state: &HttpState,
    id: i64,
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let pool = state.auth.pool();
    let receipt:Option<String>=sqlx::query_scalar("SELECT CASE WHEN json_valid(backup_metadata) THEN json_extract(backup_metadata,'$.native_local_sync') END FROM backups WHERE id=? AND status='COMPLETED'").bind(id).fetch_optional(pool).await?.flatten();
    let Some(receipt) = receipt else {
        return Ok(());
    };
    let proof: Publication =
        serde_json::from_str(&receipt).map_err(|e| Error::Decode(Box::new(e)))?;
    'copies: for copy in &proof.webdav_copies {
        if cancel.is_cancelled() {
            return Ok(());
        }
        if !(1..=2).contains(&proof.artifacts.len()) {
            return Err(Error::Protocol(
                "Invalid WebDAV publication artifacts".into(),
            ));
        }
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
        let Some(provider) = configured_webdav(state, &cfg) else {
            continue;
        };
        let mut sources = None;
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
                "$.native_webdav_effects.destination_{}.artifact_{}",
                copy.storage_id, index
            );
            let attempted: bool = sqlx::query_scalar(
                "SELECT json_type(backup_metadata,?) IS NOT NULL FROM backups WHERE id=?",
            )
            .bind(&effect)
            .bind(id)
            .fetch_one(pool)
            .await?;
            if attempted {
                let work = provider
                    .verify_object(path, artifact.length, parse_digest(&artifact.digest)?)
                    .map_err(work_error)?;
                if !matches!(wait(work, cancel).await?, Ok(true)) {
                    continue 'copies;
                }
                continue;
            }
            if sources.is_none() {
                let items = proof.artifacts.clone();
                let owned = cancel.clone();
                let work = state
                    .auth
                    .blocking()
                    .submit_blocking(move |worker| {
                        items
                            .iter()
                            .map(|item| {
                                item.open_verified(&|| {
                                    worker.is_cancelled() || owned.is_cancelled()
                                })
                                .map(Arc::new)
                            })
                            .collect::<std::io::Result<Vec<_>>>()
                    })
                    .map_err(work_error)?;
                sources = wait(work, cancel).await?.ok();
            }
            let Some(source) = sources.as_ref().map(|files| files[index].clone()) else {
                continue 'copies;
            };
            let quota_enabled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM app_settings WHERE key='storage_check_quota_before_upload' AND value='true')").fetch_one(pool).await?;
            if quota_enabled
                && let Ok(work) = provider.quota_available()
                && let Ok(Ok(Some(available))) = wait(work, cancel).await
                && available < artifact.length
            {
                let gib = (1024_u64.pow(3)) as f64;
                let error = format!(
                    "Insufficient remote storage: {:.2} GB available, {:.2} GB required",
                    available as f64 / gib,
                    artifact.length as f64 / gib
                );
                sqlx::query("UPDATE backup_storage_syncs SET sync_status='failed',error_message=? WHERE id=? AND sync_status='running'")
                    .bind(error).bind(row).execute(pool).await?;
                continue 'copies;
            }
            if cancel.is_cancelled() {
                return Ok(());
            }
            let changed=sqlx::query("UPDATE backups SET backup_metadata=json_set(backup_metadata,?,json('{}')) WHERE id=? AND status='COMPLETED' AND json_type(backup_metadata,?) IS NULL AND json_extract(backup_metadata,'$.native_local_sync')=json(?) AND EXISTS(SELECT 1 FROM backup_storage_syncs s JOIN remote_storages r ON r.id=s.storage_id WHERE s.id=? AND s.backup_id=backups.id AND s.remote_path=? AND s.sync_status='running' AND r.enabled=1 AND r.storage_type='webdav' AND r.webdav_url=? AND r.base_path IS ? AND r.username IS ?)")
                .bind(&effect).bind(id).bind(&effect).bind(&receipt).bind(row).bind(&copy.archive).bind(&copy.endpoint).bind(&copy.prefix).bind(&copy.username).execute(pool).await?;
            if changed.rows_affected() != 1 {
                continue 'copies;
            }
            let work = provider.upload(source, path).map_err(work_error)?;
            if wait(work, cancel).await?.is_err() {
                continue 'copies;
            }
        }
        if !cancel.is_cancelled() {
            sqlx::query("UPDATE backup_storage_syncs SET sync_status='completed',error_message=NULL,synced_at=strftime('%Y-%m-%d %H:%M:%f000','now') WHERE id=? AND sync_status='running' AND EXISTS(SELECT 1 FROM remote_storages r WHERE r.id=backup_storage_syncs.storage_id AND r.enabled=1 AND r.storage_type='webdav' AND r.webdav_url=? AND r.base_path IS ? AND r.username IS ?)")
                .bind(row).bind(&copy.endpoint).bind(&copy.prefix).bind(&copy.username).execute(pool).await?;
        }
    }
    Ok(())
}
