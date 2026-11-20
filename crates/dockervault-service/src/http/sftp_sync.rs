//! Journaled SFTP publication; an unconfirmed write is not replayed.
use super::{
    HttpState,
    s3_sync::{parse_digest, wait, work_error},
    storage::{configured_browse_config, open_probe_credentials, ssh_endpoint},
};
use crate::storage::{
    Publication, PublishedArtifact, SftpCopy, hash_reader,
    ssh::{SftpEndpoint, UploadStage, stage_dir},
};
use serde_json::{Map, Value};
use sqlx::Error;
use std::{fs::File, sync::Arc};
use tokio_util::sync::CancellationToken;

fn from_config(cfg: &Map<String, Value>, storage_id: i64, archive: String) -> Option<SftpCopy> {
    let get = |key| cfg.get(key).and_then(Value::as_str).map(str::to_owned);
    let storage_type = get("storage_type")?;
    if !matches!(storage_type.as_str(), "ssh" | "sftp")
        || cfg.get("enabled").and_then(Value::as_bool) != Some(true)
    {
        return None;
    }
    let port = cfg.get("port").and_then(Value::as_i64);
    if u16::try_from(port.unwrap_or(22)).ok()? == 0 {
        return None;
    }
    Some(SftpCopy {
        storage_id,
        archive,
        storage_type,
        host: get("host")?,
        port,
        prefix: get("base_path"),
        username: get("username")?,
        key_path: get("ssh_key_path"),
    })
}

pub(super) async fn destination(state: &HttpState, id: i64, archive: String) -> Option<SftpCopy> {
    from_config(
        &configured_browse_config(state, id).await.ok()?,
        id,
        archive,
    )
}

async fn endpoint(state: &HttpState, copy: &SftpCopy) -> Option<(SftpEndpoint, Option<File>)> {
    let mut cfg = configured_browse_config(state, copy.storage_id)
        .await
        .ok()?;
    if from_config(&cfg, copy.storage_id, copy.archive.clone()).as_ref() != Some(copy)
        || !open_probe_credentials(state, &mut cfg, None).await
    {
        return None;
    }
    ssh_endpoint(
        state,
        &Value::Object(cfg),
        false,
        copy.prefix.clone().unwrap_or_default(),
    )
    .await
    .ok()
}

fn artifact_path(copy: &SftpCopy, index: usize) -> String {
    if index == 0 {
        copy.archive.clone()
    } else {
        dockervault_core::crypto::key_sidecar_path(&copy.archive)
    }
}

async fn readback(
    state: &HttpState,
    endpoint: &SftpEndpoint,
    key: Option<File>,
    path: &str,
    artifact: &PublishedArtifact,
    cancel: &CancellationToken,
) -> Result<bool, Error> {
    let Some(processes) = &state.probe_processes else {
        return Ok(false);
    };
    let Ok(download) = endpoint.download_snapshot(processes, &state.probe_jobs, path, key, ())
    else {
        return Ok(false);
    };
    let Ok((mut file, length, ())) = wait(download, cancel).await? else {
        return Ok(false);
    };
    if length != artifact.length {
        return Ok(false);
    }
    let expected = parse_digest(&artifact.digest)?;
    let cancel_hash = cancel.clone();
    let work = state
        .auth
        .blocking()
        .submit_blocking(move |worker| {
            hash_reader(&mut file, &|| {
                worker.is_cancelled() || cancel_hash.is_cancelled()
            })
        })
        .map_err(work_error)?;
    Ok(matches!(wait(work, cancel).await?, Ok(actual) if actual == expected))
}

pub(super) async fn verify_cleanup_destinations(
    state: &HttpState,
    id: i64,
    proof: &Publication,
    cancel: &CancellationToken,
) -> Result<bool, Error> {
    for copy in &proof.sftp_copies {
        let completed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=? AND storage_id=? AND remote_path=? AND sync_status='completed')")
            .bind(id).bind(copy.storage_id).bind(&copy.archive).fetch_one(state.auth.pool()).await?;
        if !completed || cancel.is_cancelled() {
            return Ok(false);
        }
        for (index, artifact) in proof.artifacts.iter().enumerate() {
            let Some((endpoint, key)) = endpoint(state, copy).await else {
                return Ok(false);
            };
            if !readback(
                state,
                &endpoint,
                key,
                &artifact_path(copy, index),
                artifact,
                cancel,
            )
            .await?
            {
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
    let Some(processes) = &state.probe_processes else {
        return Ok(());
    };
    let pool = state.auth.pool();
    let receipt: Option<String> = sqlx::query_scalar("SELECT CASE WHEN json_valid(backup_metadata) THEN json_extract(backup_metadata,'$.native_local_sync') END FROM backups WHERE id=? AND status='COMPLETED'")
        .bind(id).fetch_optional(pool).await?.flatten();
    let Some(receipt) = receipt else {
        return Ok(());
    };
    let proof: Publication =
        serde_json::from_str(&receipt).map_err(|e| Error::Decode(Box::new(e)))?;
    'copies: for copy in &proof.sftp_copies {
        if !(1..=2).contains(&proof.artifacts.len()) {
            return Err(Error::Protocol("Invalid SFTP publication artifacts".into()));
        }
        let row: Option<i64> = sqlx::query_scalar("SELECT id FROM backup_storage_syncs WHERE backup_id=? AND storage_id=? AND remote_path=? AND sync_status='running'")
            .bind(id).bind(copy.storage_id).bind(&copy.archive).fetch_optional(pool).await?;
        let Some(row) = row else {
            continue;
        };
        for (index, artifact) in proof.artifacts.iter().enumerate().rev() {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let Some((endpoint, key)) = endpoint(state, copy).await else {
                continue 'copies;
            };
            let path = artifact_path(copy, index);
            let effect = format!(
                "$.native_sftp_effects.destination_{}.artifact_{}",
                copy.storage_id, index
            );
            let marker: Option<String> = sqlx::query_scalar(
                "SELECT json_extract(backup_metadata,?) FROM backups WHERE id=?",
            )
            .bind(&effect)
            .bind(id)
            .fetch_one(pool)
            .await?;
            if let Some(marker) = marker {
                if let Some(nonce) = marker
                    .strip_prefix("owned:")
                    .and_then(|s| u128::from_str_radix(s, 16).ok())
                {
                    let clone = key
                        .as_ref()
                        .map(File::try_clone)
                        .transpose()
                        .map_err(Error::Io)?;
                    let staged = format!("{}/artifact", stage_dir(&path, nonce));
                    if readback(state, &endpoint, clone, &staged, artifact, cancel).await?
                        && !cancel.is_cancelled()
                    {
                        let changed = record_phase(
                            state,
                            id,
                            &effect,
                            &marker,
                            &format!("removing:{nonce:032x}"),
                            &receipt,
                        )
                        .await?;
                        if changed
                            && endpoint
                                .stage_directory(processes, &path, nonce, true, cancel)
                                .await
                                .is_ok()
                        {
                            record_phase(
                                state,
                                id,
                                &effect,
                                &format!("removing:{nonce:032x}"),
                                &format!("removed:{nonce:032x}"),
                                &receipt,
                            )
                            .await?;
                        }
                    }
                    drop(key);
                    continue 'copies;
                }
                // Equal bytes alone cannot prove this writer replaced no one else's object.
                if !marker.starts_with("verified:")
                    || !readback(state, &endpoint, key, &path, artifact, cancel).await?
                {
                    continue 'copies;
                }
                continue;
            }
            let item = artifact.clone();
            let observed_cancel = cancel.clone();
            let work = state
                .auth
                .blocking()
                .submit_blocking(move |worker| {
                    let source = item.open_verified(&|| {
                        worker.is_cancelled() || observed_cancel.is_cancelled()
                    })?;
                    Ok::<_, std::io::Error>((
                        Arc::new(source),
                        super::super::storage::local::open_root(
                            item.path.parent().ok_or(std::io::ErrorKind::InvalidInput)?,
                        )?,
                    ))
                })
                .map_err(work_error)?;
            let Ok((source, scratch)) = wait(work, cancel).await? else {
                continue 'copies;
            };
            if copy.storage_type == "ssh" {
                let enabled = super::settings::quota_enabled(pool).await?;
                if enabled {
                    let probe_key = key
                        .as_ref()
                        .map(File::try_clone)
                        .transpose()
                        .map_err(Error::Io)?;
                    if let Some(available) = endpoint
                        .quota_upper_bound(processes, cancel, probe_key)
                        .await
                        && available < artifact.length
                    {
                        let gib = 1024_u64.pow(3) as f64;
                        let error = format!(
                            "Insufficient remote storage: {:.2} GB available, {:.2} GB required",
                            available as f64 / gib,
                            artifact.length as f64 / gib
                        );
                        sqlx::query("UPDATE backup_storage_syncs SET sync_status='failed',error_message=? WHERE id=? AND sync_status='running'")
                            .bind(error).bind(row).execute(pool).await?;
                        continue 'copies;
                    }
                }
            }
            if cancel.is_cancelled() {
                return Ok(());
            }
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random)
                .map_err(|_| Error::Protocol("SFTP staging identity unavailable".into()))?;
            let nonce = u128::from_be_bytes(random);
            let marker = format!("creating:{nonce:032x}");
            let wrote = sqlx::query("UPDATE backups SET backup_metadata=json_set(backup_metadata,?,?) WHERE id=? AND status='COMPLETED' AND json_type(backup_metadata,?) IS NULL AND json_extract(backup_metadata,'$.native_local_sync')=json(?) AND EXISTS(SELECT 1 FROM backup_storage_syncs s JOIN remote_storages r ON r.id=s.storage_id WHERE s.id=? AND s.backup_id=backups.id AND s.remote_path=? AND s.sync_status='running' AND r.enabled=1 AND r.storage_type=? AND r.host=? AND r.port IS ? AND r.base_path IS ? AND r.username=? AND r.ssh_key_path IS ?)")
                .bind(&effect).bind(&marker).bind(id).bind(&effect).bind(&receipt).bind(row).bind(&copy.archive).bind(&copy.storage_type).bind(&copy.host).bind(copy.port).bind(&copy.prefix).bind(&copy.username).bind(&copy.key_path).execute(pool).await?;
            if wrote.rows_affected() != 1 || cancel.is_cancelled() {
                continue 'copies;
            }
            if endpoint
                .stage_directory(processes, &path, nonce, false, cancel)
                .await
                .is_err()
            {
                continue 'copies;
            }
            let owned = format!("owned:{nonce:032x}");
            if !record_phase(state, id, &effect, &marker, &owned, &receipt).await? {
                continue 'copies;
            }
            let Ok(upload) = endpoint.upload_staged(
                processes,
                &state.probe_jobs,
                &path,
                source,
                scratch,
                UploadStage {
                    nonce,
                    identity: key,
                    created: true,
                },
            ) else {
                continue 'copies;
            };
            if wait(upload, cancel).await?.is_err() {
                continue 'copies;
            }
            if !record_phase(
                state,
                id,
                &effect,
                &owned,
                &format!("verified:{nonce:032x}"),
                &receipt,
            )
            .await?
            {
                continue 'copies;
            }
        }
        if !cancel.is_cancelled() {
            sqlx::query("UPDATE backup_storage_syncs SET sync_status='completed',error_message=NULL,synced_at=strftime('%Y-%m-%d %H:%M:%f000','now') WHERE id=? AND sync_status='running' AND EXISTS(SELECT 1 FROM remote_storages r WHERE r.id=backup_storage_syncs.storage_id AND r.enabled=1 AND r.storage_type=? AND r.host=? AND r.port IS ? AND r.base_path IS ? AND r.username=? AND r.ssh_key_path IS ?)")
                .bind(row).bind(&copy.storage_type).bind(&copy.host).bind(copy.port).bind(&copy.prefix).bind(&copy.username).bind(&copy.key_path).execute(pool).await?;
        }
    }
    Ok(())
}

async fn record_phase(
    state: &HttpState,
    id: i64,
    effect: &str,
    from: &str,
    to: &str,
    receipt: &str,
) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE backups SET backup_metadata=json_set(backup_metadata,?,?) WHERE id=? AND status='COMPLETED' AND json_extract(backup_metadata,?)=? AND json_extract(backup_metadata,'$.native_local_sync')=json(?)")
        .bind(effect).bind(to).bind(id).bind(effect).bind(from).bind(receipt).execute(state.auth.pool()).await?.rows_affected() == 1)
}
