//! FTP owns a fresh directory before it writes the pair; no rename or blind replay.
use super::{
    HttpState,
    s3_sync::{parse_digest, wait, work_error},
    storage::{configured_browse_config, ftp_provider, open_probe_credentials},
};
use crate::storage::{FtpCopy, Publication, PublishedArtifact, ftp::Ftp, hash_reader};
use serde_json::{Map, Value};
use sqlx::Error;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn config(cfg: &Map<String, Value>, storage_id: i64, archive: String) -> Option<FtpCopy> {
    if cfg.get("storage_type")?.as_str()? != "ftp" || !cfg.get("enabled")?.as_bool()? {
        return None;
    }
    let get = |key| cfg.get(key).and_then(Value::as_str).map(str::to_owned);
    let port = cfg.get("port").and_then(Value::as_i64);
    if u16::try_from(port.unwrap_or(21)).ok()? == 0 {
        return None;
    }
    Some(FtpCopy {
        storage_id,
        archive,
        host: get("host").filter(|s| !s.is_empty())?,
        port,
        prefix: get("base_path"),
        username: get("username"),
    })
}

pub(super) async fn destination(state: &HttpState, id: i64, archive: &str) -> Option<FtpCopy> {
    let cfg = configured_browse_config(state, id).await.ok()?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).ok()?;
    let basename = archive.rsplit('/').next()?;
    config(
        &cfg,
        id,
        format!("dockervault-{:032x}/{basename}", u128::from_be_bytes(nonce)),
    )
}

async fn provider(state: &HttpState, copy: &FtpCopy) -> Option<Ftp> {
    let mut cfg = configured_browse_config(state, copy.storage_id)
        .await
        .ok()?;
    if config(&cfg, copy.storage_id, copy.archive.clone()).as_ref() != Some(copy)
        || !open_probe_credentials(state, &mut cfg, None).await
    {
        return None;
    }
    ftp_provider(state, &Value::Object(cfg))
}

fn artifact_path(copy: &FtpCopy, index: usize) -> String {
    if index == 0 {
        copy.archive.clone()
    } else {
        dockervault_core::crypto::key_sidecar_path(&copy.archive)
    }
}

async fn verify(
    state: &HttpState,
    ftp: &Ftp,
    path: String,
    artifact: &PublishedArtifact,
    cancel: &CancellationToken,
) -> Result<bool, Error> {
    let work = ftp
        .download_snapshot(path, state.auth.blocking(), ())
        .map_err(work_error)?;
    let Ok((mut file, length, ())) = wait(work, cancel).await? else {
        return Ok(false);
    };
    if length != artifact.length {
        return Ok(false);
    }
    let expected = parse_digest(&artifact.digest)?;
    let token = cancel.clone();
    let work = state
        .auth
        .blocking()
        .submit_blocking(move |worker| {
            hash_reader(&mut file, &|| worker.is_cancelled() || token.is_cancelled())
        })
        .map_err(work_error)?;
    Ok(matches!(wait(work, cancel).await?, Ok(digest) if digest == expected))
}

pub(super) async fn verify_cleanup_destinations(
    state: &HttpState,
    id: i64,
    proof: &Publication,
    cancel: &CancellationToken,
) -> Result<bool, Error> {
    for copy in &proof.ftp_copies {
        let completed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE backup_id=? AND storage_id=? AND remote_path=? AND sync_status='completed')")
            .bind(id).bind(copy.storage_id).bind(&copy.archive).fetch_one(state.auth.pool()).await?;
        if !completed || cancel.is_cancelled() {
            return Ok(false);
        }
        let Some(ftp) = provider(state, copy).await else {
            return Ok(false);
        };
        for (index, item) in proof.artifacts.iter().enumerate() {
            if !verify(state, &ftp, artifact_path(copy, index), item, cancel).await? {
                return Ok(false);
            }
        }
    }
    Ok(!cancel.is_cancelled())
}

async fn phase(state: &HttpState, id: i64, path: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT json_extract(backup_metadata,?) FROM backups WHERE id=?")
        .bind(path)
        .bind(id)
        .fetch_one(state.auth.pool())
        .await
}

struct Intent<'a> {
    id: i64,
    receipt: &'a str,
    row: i64,
    copy: &'a FtpCopy,
}
impl Intent<'_> {
    async fn record(
        &self,
        state: &HttpState,
        path: &str,
        before: Option<&str>,
        after: &str,
    ) -> Result<bool, Error> {
        let c = self.copy;
        Ok(sqlx::query("UPDATE backups SET backup_metadata=json_set(backup_metadata,?,?) WHERE id=? AND status='COMPLETED' AND json_extract(backup_metadata,?) IS ? AND json_extract(backup_metadata,'$.native_local_sync')=json(?) AND EXISTS(SELECT 1 FROM backup_storage_syncs s JOIN remote_storages r ON r.id=s.storage_id WHERE s.id=? AND s.backup_id=backups.id AND s.remote_path=? AND s.sync_status='running' AND r.enabled=1 AND r.storage_type='ftp' AND r.host=? AND r.port IS ? AND r.base_path IS ? AND r.username IS ?)")
            .bind(path).bind(after).bind(self.id).bind(path).bind(before).bind(self.receipt).bind(self.row).bind(&c.archive).bind(&c.host).bind(c.port).bind(&c.prefix).bind(&c.username).execute(state.auth.pool()).await?.rows_affected()==1)
    }
}

pub(super) async fn settle(
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
    'copies: for copy in &proof.ftp_copies {
        if cancel.is_cancelled() {
            break;
        }
        if !(1..=2).contains(&proof.artifacts.len()) {
            return Err(Error::Protocol("Invalid FTP publication artifacts".into()));
        }
        let row: Option<i64> = sqlx::query_scalar("SELECT id FROM backup_storage_syncs WHERE backup_id=? AND storage_id=? AND remote_path=? AND sync_status='running'")
            .bind(id).bind(copy.storage_id).bind(&copy.archive).fetch_optional(pool).await?;
        let (Some(row), Some((directory, _))) = (row, copy.archive.split_once('/')) else {
            continue;
        };
        let Some(ftp) = provider(state, copy).await else {
            continue;
        };
        let intent = Intent {
            id,
            receipt: &receipt,
            row,
            copy,
        };
        let root = format!("$.native_ftp_effects.destination_{}", copy.storage_id);
        let claim = format!("{root}.claim");
        match phase(state, id, &claim).await?.as_deref() {
            Some("owned") => (),
            Some(_) => continue,
            None => {
                if !intent.record(state, &claim, None, "creating").await? || cancel.is_cancelled() {
                    continue;
                }
                let work = ftp.create_claim(directory.into()).map_err(work_error)?;
                if wait(work, cancel).await?.is_err()
                    || !intent
                        .record(state, &claim, Some("creating"), "owned")
                        .await?
                {
                    continue;
                }
            }
        }
        for (index, item) in proof.artifacts.iter().enumerate().rev() {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let path = artifact_path(copy, index);
            let effect = format!("{root}.artifact_{index}");
            if phase(state, id, &effect).await?.is_some() {
                // Full bytes at a freshly owned name can settle a lost upload reply without STOR replay.
                if !verify(state, &ftp, path, item, cancel).await? {
                    continue 'copies;
                }
                continue;
            }
            let artifact = item.clone();
            let token = cancel.clone();
            let work = state
                .auth
                .blocking()
                .submit_blocking(move |worker| {
                    artifact
                        .open_verified(&|| worker.is_cancelled() || token.is_cancelled())
                        .map(Arc::new)
                })
                .map_err(work_error)?;
            let Ok(source) = wait(work, cancel).await? else {
                continue 'copies;
            };
            if !intent.record(state, &effect, None, "uploading").await? || cancel.is_cancelled() {
                continue 'copies;
            }
            let work = ftp
                .upload_claimed(source, path, state.auth.blocking())
                .map_err(work_error)?;
            if wait(work, cancel).await?.is_err() {
                continue 'copies;
            }
        }
        if !cancel.is_cancelled() {
            sqlx::query("UPDATE backup_storage_syncs SET sync_status='completed',error_message=NULL,synced_at=strftime('%Y-%m-%d %H:%M:%f000','now') WHERE id=? AND sync_status='running' AND EXISTS(SELECT 1 FROM remote_storages r WHERE r.id=backup_storage_syncs.storage_id AND r.enabled=1 AND r.storage_type='ftp' AND r.host=? AND r.port IS ? AND r.base_path IS ? AND r.username IS ?)")
                .bind(row).bind(&copy.host).bind(copy.port).bind(&copy.prefix).bind(&copy.username).execute(pool).await?;
        }
    }
    Ok(())
}
