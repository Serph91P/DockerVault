use super::{HttpState, auth::required_user};
use crate::{
    jobs::Resource,
    journal::{BackupKind, Journal, JournalError},
    storage::{
        LocalCopy, Publication, PublishedArtifact, S3Copy, WebDavCopy, complete_publication, local,
        mark_publication,
    },
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use dockervault_core::crypto::{decrypt_private, encrypt_dek, encrypt_stream};
use dockervault_tools::archive_worker::{Request, archive_private};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use std::{
    fs::File,
    io::{self, Seek, Write},
    os::fd::AsRawFd,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use time::{UtcDateTime, macros::format_description};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
pub(super) struct Submit {
    target_id: i64,
    #[serde(default)]
    backup_type: RunType,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RunType {
    #[default]
    Full,
    Incremental,
}

struct Plan {
    name: String,
    host_path: Option<String>,
    target_type: String,
    enabled: Option<bool>,
    compression_enabled: Option<bool>,
    include_paths: Option<String>,
    exclude_paths: Option<String>,
    sync_to_remote: bool,
    delete_local_after_sync: bool,

    remote_storage_ids: Vec<i64>,
    unsupported: bool,
    recipient: Option<String>,
}

pub(super) async fn submit(
    State(state): State<HttpState>,
    headers: HeaderMap,
    input: Result<Json<Submit>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let Ok(Json(input)) = input else {
        return error((StatusCode::UNPROCESSABLE_ENTITY, "Invalid backup request"));
    };
    let worker = state.clone();
    let (admitted, reply) = tokio::sync::oneshot::channel();
    let result = state.backup_jobs.submit_async(move |cancel| async move {
        let (id, response) = match admit(&worker, input.target_id, input.backup_type).await {
            Ok(value) => value,
            Err(failure) => { let _ = admitted.send(Err(failure)); return; }
        };
        let _ = admitted.send(Ok(response));
        let started = std::time::Instant::now();
        if execute(&worker, id, input.target_id, &cancel).await.is_err() {
            let result = sqlx::query("UPDATE backups SET status='FAILED',error_message='Archive job did not complete',completed_at=strftime('%Y-%m-%d %H:%M:%f000','now') WHERE id=? AND status IN ('PENDING','RUNNING') AND json_extract(backup_metadata,'$.native_publication') IS NULL")
                .bind(id).execute(worker.auth.pool()).await;
            if result.is_err() { tracing::error!(backup_id=id, "Failed to record interrupted backup state"); }
        }
        match sqlx::query_as::<_, (String, Option<i64>)>("SELECT status,file_size FROM backups WHERE id=?")
            .bind(id).fetch_optional(worker.auth.pool()).await {
            Ok(Some((status, size))) if status == "COMPLETED" || status == "FAILED" => worker.backup_metrics.lock().unwrap().record(
                input.target_id, started.elapsed().as_secs_f64(),
                if status == "COMPLETED" { Some(size.unwrap_or_default()) } else { None },
            ),
            Ok(_) => {},
            Err(_) => tracing::warn!(backup_id=id, "Could not read backup settlement for metrics"),
        }
    });
    let Ok(work) = result else {
        return error((StatusCode::SERVICE_UNAVAILABLE, "Backup capacity exhausted"));
    };
    work.detach();
    match reply.await {
        Ok(Ok(response)) => Json(response).into_response(),
        Ok(Err(failure)) => error(failure),
        Err(_) => error(INTERNAL),
    }
}

type Failure = (StatusCode, &'static str);
const INTERNAL: Failure = (StatusCode::INTERNAL_SERVER_ERROR, "Backup operation failed");
const UNSUPPORTED: Failure = (
    StatusCode::SERVICE_UNAVAILABLE,
    "This backup configuration is not yet supported by the native candidate",
);
fn error((status, message): Failure) -> Response {
    (status, Json(json!({"detail":message}))).into_response()
}

async fn plan(state: &HttpState, id: i64) -> Result<Plan, Failure> {
    let row = sqlx::query("SELECT name,host_path,target_type,enabled,compression_enabled,include_paths,exclude_paths,
          coalesce(sync_to_remote,0) AS sync_to_remote,
          coalesce(delete_local_after_sync,0) AS delete_local_after_sync,
          coalesce(remote_storage_ids,'[]') AS remote_storage_ids,
          retention_policy_id IS NOT NULL
          OR coalesce(pre_backup_command,'')<>'' OR coalesce(post_backup_command,'')<>''
          OR coalesce(dependencies,'[]') NOT IN ('[]','null') AS unsupported,
          (SELECT public_key FROM encryption_config WHERE encryption_enabled=1 ORDER BY id LIMIT 1) AS recipient
        FROM backup_targets WHERE id=? AND removed_at IS NULL")
        .bind(id).fetch_optional(state.auth.pool()).await.map_err(|_| INTERNAL)?
        .ok_or((StatusCode::NOT_FOUND,"Target not found"))?;
    let plan = (|| -> Result<_, sqlx::Error> {
        let remote: String = row.try_get("remote_storage_ids")?;
        Ok(Plan {
            name: row.try_get("name")?,
            host_path: row.try_get("host_path")?,
            target_type: row.try_get("target_type")?,
            enabled: row.try_get("enabled")?,
            compression_enabled: row.try_get("compression_enabled")?,
            include_paths: row.try_get("include_paths")?,
            exclude_paths: row.try_get("exclude_paths")?,
            sync_to_remote: row.try_get("sync_to_remote")?,
            delete_local_after_sync: row.try_get("delete_local_after_sync")?,
            remote_storage_ids: serde_json::from_str::<Option<Vec<i64>>>(&remote)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?
                .unwrap_or_default(),
            unsupported: row.try_get("unsupported")?,
            recipient: row.try_get("recipient")?,
        })
    })()
    .map_err(|_| INTERNAL)?;
    if plan.enabled != Some(true) {
        return Err((StatusCode::CONFLICT, "Target is disabled"));
    }
    if plan.unsupported
        || plan.target_type != "path"
        || plan.host_path.as_deref().is_none_or(str::is_empty)
    {
        return Err(UNSUPPORTED);
    }
    Ok(plan)
}

async fn admit(state: &HttpState, target_id: i64, kind: RunType) -> Result<(i64, Value), Failure> {
    let plan = plan(state, target_id).await?;
    destinations(state, &plan, String::new()).await?;
    if !matches!(kind, RunType::Full) {
        return Err(UNSUPPORTED);
    }
    let journal = Journal::new(state.auth.pool().clone()).map_err(|_| INTERNAL)?;
    let id = journal
        .admit_backup(target_id, BackupKind::Full)
        .await
        .map_err(|error| match error {
            JournalError::Missing => (
                StatusCode::CONFLICT,
                "Plan removed before backup could start",
            ),
            JournalError::TargetDisabled => (StatusCode::CONFLICT, "Target is disabled"),
            JournalError::RecoveryRequired => (
                StatusCode::CONFLICT,
                "Workload recovery incomplete; retry reconciliation before new jobs",
            ),
            _ => INTERNAL,
        })?;
    let (name, stamp): (String, String) = sqlx::query_as(
        "SELECT json_extract(backup_metadata,'$.target_name'),created_at FROM backups WHERE id=?",
    )
    .bind(id)
    .fetch_one(state.auth.pool())
    .await
    .map_err(|_| INTERNAL)?;
    let created = dockervault_core::timestamp::Timestamp::parse(&stamp)
        .map_err(|_| INTERNAL)?
        .api();
    Ok((
        id,
        json!({"id":id,"target_id":target_id,"target_name":name,"backup_type":"full","status":"pending","file_path":null,"file_size":null,"file_size_human":null,"checksum":null,"started_at":null,"completed_at":null,"duration_seconds":null,"error_message":null,"encrypted":false,"created_at":created}),
    ))
}

#[derive(Default)]
struct Destinations {
    local: Vec<(i64, PathBuf)>,
    s3: Vec<S3Copy>,
    webdav: Vec<WebDavCopy>,
    sftp: Vec<crate::storage::SftpCopy>,
    ftp: Vec<crate::storage::FtpCopy>,
}

async fn destinations(
    state: &HttpState,
    plan: &Plan,
    archive: String,
) -> Result<Destinations, Failure> {
    let mut destinations = Destinations::default();
    if plan.sync_to_remote {
        let ids = plan
            .remote_storage_ids
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        for id in ids {
            let base: Option<String> = sqlx::query_scalar("SELECT base_path FROM remote_storages WHERE id=? AND enabled=1 AND storage_type='local' AND base_path IS NOT NULL")
                .bind(id).fetch_optional(state.auth.pool()).await.map_err(|_| INTERNAL)?;
            if let Some(base) = base.filter(|path| !path.is_empty()) {
                destinations
                    .local
                    .push((id, std::path::absolute(base).map_err(|_| INTERNAL)?));
            } else if let Some(copy) = super::s3_sync::destination(state, id, archive.clone()).await
            {
                destinations.s3.push(copy);
            } else if let Some(copy) =
                super::webdav_sync::destination(state, id, archive.clone()).await
            {
                destinations.webdav.push(copy);
            } else if let Some(copy) =
                super::sftp_sync::destination(state, id, archive.clone()).await
            {
                destinations.sftp.push(copy);
            } else if let Some(copy) = super::ftp_sync::destination(state, id, &archive).await {
                destinations.ftp.push(copy);
            } else {
                return Err(UNSUPPORTED);
            }
        }
    }
    Ok(destinations)
}

async fn execute(
    state: &HttpState,
    id: i64,
    target: i64,
    cancel: &CancellationToken,
) -> Result<(), ()> {
    let _target = state
        .backup_claims
        .acquire([Resource::target(target)], cancel)
        .await
        .map_err(|_| ())?;
    let plan = plan(state, target).await.map_err(|_| ())?;

    let root = PathBuf::from(&state.config.backup_base_path);
    if root.as_os_str().is_empty() {
        return Err(());
    }
    let source = PathBuf::from(plan.host_path.as_deref().ok_or(())?);
    let source_path = source.clone();
    let (source, source_claim) = state
        .auth
        .blocking()
        .submit_blocking(move |_| {
            let identity = std::fs::canonicalize(source)?;
            Ok::<_, io::Error>((identity.clone(), Resource::existing_path(&identity)?))
        })
        .map_err(|_| ())?
        .wait()
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    let _source = state
        .backup_claims
        .acquire([source_claim], cancel)
        .await
        .map_err(|_| ())?;
    let layout = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(())?
        .to_owned();
    let compress = plan.compression_enabled.unwrap_or(false);
    let safe = safe_name(&plan.name);
    let stamp = UtcDateTime::now()
        .format(format_description!(
            "[year][month][day]_[hour][minute][second]"
        ))
        .map_err(|_| ())?;
    let relative = PathBuf::from(&safe).join(format!(
        "{safe}_{stamp}.tar{}",
        if compress { ".gz" } else { "" }
    ));
    let root = std::path::absolute(root).map_err(|_| ())?;
    let path = root.join(&relative);
    let plaintext_path = path.to_str().ok_or(())?.to_owned();
    let stored = if plan.recipient.is_some() {
        format!("{plaintext_path}.enc")
    } else {
        plaintext_path
    };
    let sidecar = plan
        .recipient
        .as_ref()
        .map(|_| dockervault_core::crypto::key_sidecar_path(&stored));
    let remote_name = Path::new(&stored)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(())?;
    let remote_archive = format!("{safe}/{remote_name}");
    let destinations = destinations(state, &plan, remote_archive.clone())
        .await
        .map_err(|_| ())?;
    let request = json!({"version":1,"dest":Path::new("/backups").join(&relative),"sources":[[&layout, &source]],"compress":compress,
        "include_paths":parse_filter(plan.include_paths)?,"exclude_paths":parse_filter(plan.exclude_paths)?});
    let pool = state.auth.pool();
    let update = sqlx::query("UPDATE backups SET status='RUNNING',started_at=strftime('%Y-%m-%d %H:%M:%f000','now'),backup_metadata=json_patch(backup_metadata,?) WHERE id=? AND status='PENDING'")
        .bind(json!({"archive_layout":{"version":1,"kind":"single","root":layout},"native_ephemeral_archive":true,"active_artifacts":std::iter::once(&stored).chain(sidecar.iter()).collect::<Vec<_>>()}).to_string())
        .bind(id).execute(pool).await.map_err(|_| ())?;
    if update.rows_affected() != 1 {
        return Err(());
    }
    let job_cancel = cancel.clone();
    let output = stored.clone();
    let key_output = sidecar.clone();
    let journal_pool = pool.clone();
    let runtime = tokio::runtime::Handle::current();
    let result = state
        .auth
        .blocking()
        .submit_blocking(move |owner_cancel| {
            let cancelled = || job_cancel.is_cancelled() || owner_cancel.is_cancelled();
            if cancelled()
                || source.starts_with(&root)
                || root.starts_with(&source)
                || std::fs::canonicalize(&source_path)? != source
            {
                return Err(io::Error::other(
                    "Archive source unavailable or overlaps backup storage",
                ));
            }
            let fd = local::open_root(&source)?;
            let parent = local::open_or_create_root(&root.join(&safe))?;
            let mut local_copies = Vec::with_capacity(destinations.local.len());
            for (storage_id, copy_root) in destinations.local {
                if copy_root.starts_with(&source) || source.starts_with(&copy_root) || copy_root.starts_with(&root) || root.starts_with(&copy_root) {
                    return Err(io::Error::other("LOCAL destination overlaps source or backup storage"));
                }
                let meta = local::open_root(&copy_root)?.metadata()?;
                local_copies.push(LocalCopy { storage_id, root: copy_root, device: meta.dev(), inode: meta.ino(), archive: remote_archive.clone() });
            }
            let request =
                Request::parse(&serde_json::to_vec(&request)?).map_err(io::Error::other)?;
            let (mut archive, checksum, mut wrapped_file) = if let Some(recipient) = plan.recipient {
                let (cipher, checksum, key) = prepare_encrypted(&request, &fd, &parent, &recipient, &cancelled)?;
                (cipher, checksum, Some(key))
            } else {
                let mut plain = archive_private(&request, &fd, &parent, &cancelled)?;
                let checksum = hash_hex(&mut plain, &cancelled)?;
                (plain, checksum, None)
            };
            let mut nonce = [0u8; 16];
            getrandom::fill(&mut nonce).map_err(|_| io::Error::other("Publication identity unavailable"))?;
            let token: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
            let metadata = archive.metadata()?;
            let length = metadata.len();
            let digest = if wrapped_file.is_some() { hash_hex(&mut archive, &cancelled)? } else { checksum.clone() };
            mark_publication(&archive, &token)?;
            let mut artifacts = vec![PublishedArtifact { path: PathBuf::from(&output), length, digest, device: metadata.dev(), inode: metadata.ino() }];
            if let (Some(key), Some(path)) = (&mut wrapped_file, &key_output) {
                let metadata = key.metadata()?;
                artifacts.push(PublishedArtifact { path: PathBuf::from(path), length: metadata.len(), digest: hash_hex(key, &cancelled)?, device: metadata.dev(), inode: metadata.ino() });
                mark_publication(key, &token)?;
            }
            if cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let delete_local_after_sync = plan.delete_local_after_sync && !(local_copies.is_empty() && destinations.s3.is_empty() && destinations.webdav.is_empty() && destinations.sftp.is_empty() && destinations.ftp.is_empty());
            let proof = Publication { token, checksum, artifacts, local_copies, s3_copies: destinations.s3, webdav_copies: destinations.webdav, sftp_copies: destinations.sftp, ftp_copies: destinations.ftp, delete_local_after_sync };
            let serialized = serde_json::to_string(&proof)?;
            let staged = runtime.block_on(sqlx::query("UPDATE backups SET backup_metadata=json_set(backup_metadata,'$.native_publication',json(?)) WHERE id=? AND status='RUNNING'")
                .bind(serialized).bind(id).execute(&journal_pool)).map_err(io::Error::other)?;
            if staged.rows_affected() != 1 || cancelled() { return Err(io::ErrorKind::Interrupted.into()); }
            // The durable receipt precedes both links, and key custody precedes ciphertext.
            if let (Some(key), Some(path)) = (&wrapped_file, &key_output) { publish(&parent, key, Path::new(path))?; }
            publish(&parent, &archive, Path::new(&output))?;
            Ok(proof)
        })
        .map_err(|_| ())?
        .wait()
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    complete_publication(pool, id, &result)
        .await
        .map_err(|error| {
            tracing::warn!(backup_id=id, %error, "Archive publication remains unsettled");
        })?;
    super::s3_sync::settle(state, id, cancel)
        .await
        .map_err(|error| {
            tracing::warn!(backup_id=id, %error, "S3 sync settlement remains pending");
        })?;
    super::webdav_sync::settle(state, id, cancel)
        .await
        .map_err(|error| {
            tracing::warn!(backup_id=id, %error, "WebDAV sync settlement remains pending");
        })?;
    super::sftp_sync::settle(state, id, cancel)
        .await
        .map_err(|error| {
            tracing::warn!(backup_id=id, %error, "SFTP sync settlement remains pending");
        })?;
    super::ftp_sync::settle(state, id, cancel)
        .await
        .map_err(|error| {
            tracing::warn!(backup_id=id, %error, "FTP sync settlement remains pending");
        })?;
    super::s3_sync::finalize_copies(state, id, cancel)
        .await
        .map_err(|error| {
            tracing::warn!(backup_id=id, %error, "Sync settlement remains pending");
        })
}

fn prepare_encrypted(
    request: &Request,
    source: &File,
    parent: &File,
    recipient: &str,
    cancel: &dyn Fn() -> bool,
) -> io::Result<(File, String, File)> {
    use nix::{
        fcntl::{OFlag, openat},
        sys::stat::Mode,
    };
    let spool = || {
        openat(
            parent,
            ".",
            OFlag::O_TMPFILE | OFlag::O_RDWR | OFlag::O_CLOEXEC,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .map(File::from)
    };
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *key).map_err(|_| io::Error::other("Archive key generation failed"))?;
    let wrapped = encrypt_dek(&key[..], recipient).map_err(io::Error::other)?;
    let mut plain = archive_private(request, source, parent, cancel)?;
    let digest = crate::storage::hash_reader(&mut plain, cancel)?;
    plain.rewind()?;
    let mut cipher = spool()?;
    encrypt_stream(plain, &mut cipher, &key[..], cancel).map_err(io::Error::other)?;
    cipher.sync_all()?;
    let length = cipher.metadata()?.len();
    cipher.rewind()?;
    let mut checked = decrypt_private(&mut cipher, length, spool()?, &key[..], cancel)
        .map_err(io::Error::other)?;
    if crate::storage::hash_reader(&mut checked, cancel)? != digest {
        return Err(io::Error::other("Encrypted archive verification failed"));
    }
    let mut wrapped_file = spool()?;
    wrapped_file.write_all(&wrapped)?;
    wrapped_file.sync_all()?;
    wrapped_file.rewind()?;
    use std::io::Read;
    let mut readback = Vec::new();
    Read::by_ref(&mut wrapped_file)
        .take(wrapped.len() as u64 + 1)
        .read_to_end(&mut readback)?;
    if readback != wrapped || cancel() {
        return Err(io::Error::other("Archive sidecar verification interrupted"));
    }

    Ok((
        cipher,
        digest.iter().map(|b| format!("{b:02x}")).collect(),
        wrapped_file,
    ))
}

fn hash_hex(file: &mut File, cancel: &dyn Fn() -> bool) -> io::Result<String> {
    file.rewind()?;
    Ok(crate::storage::hash_reader(file, cancel)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn publish(parent: &File, file: &File, path: &Path) -> io::Result<()> {
    use nix::{
        fcntl::{AT_FDCWD, AtFlags},
        unistd::linkat,
    };
    let leaf = path
        .file_name()
        .ok_or_else(|| io::Error::other("Missing archive name"))?;
    let fd = format!("/proc/self/fd/{}", file.as_raw_fd());
    linkat(
        AT_FDCWD,
        fd.as_str(),
        parent,
        leaf,
        AtFlags::AT_SYMLINK_FOLLOW,
    )?;
    parent.sync_all()
}

fn parse_filter(raw: Option<String>) -> Result<Option<Vec<String>>, ()> {
    raw.as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map(Option::flatten)
        .map_err(|_| ())
}

fn safe_name(name: &str) -> String {
    let mut result = String::new();
    for c in name.chars() {
        let c = if "<>:\"/\\|?*".contains(c) || c.is_ascii_control() {
            '_'
        } else {
            c
        };
        if c != '_' || !result.ends_with('_') {
            result.push(c);
        }
    }
    match result.trim_matches([' ', '_']) {
        "" | "." | ".." => "unnamed_target".into(),
        name => name.into(),
    }
}
