use super::{HttpState, auth::required_user};
use crate::{
    jobs::Resource,
    journal::{BackupKind, Journal, JournalError},
    storage::{VerifiedSource, local},
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use dockervault_core::crypto::{decrypt_private, encrypt_dek, encrypt_stream};
use dockervault_tools::archive_worker::{Request, archive_private, create_archive_from_directory};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use std::{
    fs::File,
    io::{self, Seek, Write},
    os::fd::AsRawFd,
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
        if execute(&worker, id, input.target_id, &cancel).await.is_err() {
            let result = sqlx::query("UPDATE backups SET status='FAILED',error_message='Archive job did not complete',completed_at=strftime('%Y-%m-%d %H:%M:%f000','now') WHERE id=? AND status IN ('PENDING','RUNNING')")
                .bind(id).execute(worker.auth.pool()).await;
            if result.is_err() { tracing::error!(backup_id=id, "Failed to record interrupted backup state"); }
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
          coalesce(sync_to_remote,0) OR coalesce(delete_local_after_sync,0) OR retention_policy_id IS NOT NULL
          OR coalesce(pre_backup_command,'')<>'' OR coalesce(post_backup_command,'')<>''
          OR coalesce(dependencies,'[]') NOT IN ('[]','null') AS unsupported,
          (SELECT public_key FROM encryption_config WHERE encryption_enabled=1 ORDER BY id LIMIT 1) AS recipient
        FROM backup_targets WHERE id=? AND removed_at IS NULL")
        .bind(id).fetch_optional(state.auth.pool()).await.map_err(|_| INTERNAL)?
        .ok_or((StatusCode::NOT_FOUND,"Target not found"))?;
    let plan = (|| -> Result<_, sqlx::Error> {
        Ok(Plan {
            name: row.try_get("name")?,
            host_path: row.try_get("host_path")?,
            target_type: row.try_get("target_type")?,
            enabled: row.try_get("enabled")?,
            compression_enabled: row.try_get("compression_enabled")?,
            include_paths: row.try_get("include_paths")?,
            exclude_paths: row.try_get("exclude_paths")?,
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
    plan(state, target_id).await?;
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
    let request = json!({"version":1,"dest":Path::new("/backups").join(&relative),"sources":[[&layout, &source]],"compress":compress,
        "include_paths":parse_filter(plan.include_paths)?,"exclude_paths":parse_filter(plan.exclude_paths)?});
    let pool = state.auth.pool();
    let update = sqlx::query("UPDATE backups SET status='RUNNING',started_at=strftime('%Y-%m-%d %H:%M:%f000','now'),backup_metadata=json_patch(backup_metadata,?) WHERE id=? AND status='PENDING'")
        .bind(json!({"archive_layout":{"version":1,"kind":"single","root":layout},"active_artifacts":std::iter::once(&stored).chain(sidecar.iter()).collect::<Vec<_>>()}).to_string())
        .bind(id).execute(pool).await.map_err(|_| ())?;
    if update.rows_affected() != 1 {
        return Err(());
    }
    let job_cancel = cancel.clone();
    let output = stored.clone();
    let key_output = sidecar.clone();
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
            let backup_root = local::open_root(&root)?;
            let parent = local::open_or_create_root(&root.join(&safe))?;
            let request =
                Request::parse(&serde_json::to_vec(&request)?).map_err(io::Error::other)?;
            if let (Some(recipient), Some(key_output)) = (plan.recipient, key_output) {
                return write_encrypted(
                    &request,
                    &fd,
                    &parent,
                    &recipient,
                    Path::new(&output),
                    Path::new(&key_output),
                    &cancelled,
                );
            }
            let report = create_archive_from_directory(&request, &fd, &backup_root, &cancelled)?;
            if report.skipped_count() != 0 {
                return Err(io::Error::other("Archive source contains unreadable files"));
            }
            let result = VerifiedSource::open(&path, &cancelled)?;
            Ok((result.len(), result.sha256_hex()))
        })
        .map_err(|_| ())?
        .wait()
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    let len = i64::try_from(result.0).map_err(|_| ())?;
    let changed = sqlx::query("UPDATE backups SET status='COMPLETED', file_path=?,file_size=?,checksum=?,encrypted=?,encryption_key_path=?,completed_at=strftime('%Y-%m-%d %H:%M:%f000','now'), duration_seconds=max(0,cast((julianday('now')-julianday(started_at))*86400 as integer)),backup_metadata=json_remove(backup_metadata,'$.active_artifacts') WHERE id=? AND status='RUNNING'")
        .bind(stored).bind(len).bind(result.1).bind(sidecar.is_some()).bind(sidecar).bind(id).execute(pool).await.map_err(|_| ())?;
    if changed.rows_affected() == 1 {
        Ok(())
    } else {
        Err(())
    }
}

fn write_encrypted(
    request: &Request,
    source: &File,
    parent: &File,
    recipient: &str,
    archive: &Path,
    sidecar: &Path,
    cancel: &dyn Fn() -> bool,
) -> io::Result<(u64, String)> {
    use nix::{
        fcntl::{AT_FDCWD, AtFlags, OFlag, openat},
        sys::stat::Mode,
        unistd::linkat,
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
    let publish = |file: &File, name: &Path| -> io::Result<()> {
        let leaf = name
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
    };
    // First preserve recoverable key custody; a crash cannot leave a newly published archive without its key.
    publish(&wrapped_file, sidecar)?;
    publish(&cipher, archive)?;
    Ok((length, digest.iter().map(|b| format!("{b:02x}")).collect()))
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
