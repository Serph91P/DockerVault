//! Atomic restart settlement for interrupted user-owned storage operations.
use crate::storage::{
    ftp::{Ftp, FtpConfig, FtpError},
    s3::{S3, S3Config},
    webdav::{DavConfig, DavError, WebDav},
};
use crate::{
    ownership::{Owner, Work, WorkError},
    process::ProcessOwner,
    recovery_activation::{JournalError, recovery_pending_in},
    storage::{
        local::{LocalStorage, TransferError},
        relative_path,
        ssh::{SftpDownloadError, SftpEndpoint},
    },
};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub enum StorageRecoveryError {
    Activation(JournalError),
    Database(sqlx::Error),
    IdentityChanged,
    InvalidRequest,
    RequestConflict,
    ShuttingDown,
    QueueFull,
    RecoveryPending,
    Randomness,
    OperationNotFound,
    OperationActive,
    NothingToRetry,
    UnsupportedProvider,
    StorageNotFound,
    StorageDisabled,
    InvalidStorageConfiguration,
}

#[derive(Clone, Copy)]
pub struct DeleteAdmission<'a> {
    pub user_id: i64,
    pub request_id: &'a str,
    pub storage_id: &'a str,
    pub path: &'a str,
    pub paths: &'a [&'a str],
    pub identity: &'a serde_json::Value,
    pub accepting: bool,
}

pub async fn submit_delete(
    pool: &SqlitePool,
    request: DeleteAdmission<'_>,
) -> Result<String, StorageRecoveryError> {
    Ok(admit_delete(pool, request, None, None).await?.0)
}

async fn admit_delete(
    pool: &SqlitePool,
    request: DeleteAdmission<'_>,
    parent: Option<&RetryParent>,
    remote_id: Option<i64>,
) -> Result<(String, bool), StorageRecoveryError> {
    use StorageRecoveryError as Error;
    let invalid = Error::InvalidRequest;
    let id_shape = request.request_id.as_bytes();
    if id_shape.len() != 36
        || !id_shape.iter().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
        || (request.storage_id != "local"
            && !request
                .storage_id
                .parse::<u64>()
                .is_ok_and(|n| n > 0 && n.to_string() == request.storage_id))
    {
        return Err(invalid);
    }
    let request_id = request.request_id.to_ascii_lowercase();
    let path = relative_path(request.path, true).ok_or(Error::InvalidRequest)?;
    let paths = normalize_delete_selection(request.paths.iter().copied())?;
    let parent_id = parent.map(|parent| parent.id.as_str());
    let payload = delete_payload(request.storage_id, &path, &paths, parent_id);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if recovery_pending_in(&mut *tx)
        .await
        .map_err(Error::Activation)?
    {
        return Err(Error::RecoveryPending);
    }
    if let Some(id) = matching_delete(&mut tx, request.user_id, &request_id, &payload).await? {
        tx.rollback().await?;
        return Ok((id, false));
    }
    if !request.accepting {
        return Err(Error::ShuttingDown);
    }
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM storage_operations WHERE state IN ('queued','running')",
    )
    .fetch_one(&mut *tx)
    .await?;
    if active >= 32 {
        return Err(Error::QueueFull);
    }
    if !request.identity.is_object()
        || parent.is_some_and(|parent| &parent.identity != request.identity)
    {
        return Err(Error::IdentityChanged);
    }
    if let Some(id) = remote_id
        && !matches_remote_configuration(&mut tx, id, request.identity).await?
    {
        return Err(Error::IdentityChanged);
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| Error::Randomness)?;
    nonce[6] = (nonce[6] & 0x0f) | 0x40;
    nonce[8] = (nonce[8] & 0x3f) | 0x80;
    let hex: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let id = format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    );
    sqlx::query("INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,parent_id,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at)
                 VALUES(?,?,?,?,?,?,'storage_delete',?,?,'queued','queued',?,0,0,0,0,0,0,strftime('%Y-%m-%d %H:%M:%f','now'),strftime('%Y-%m-%d %H:%M:%f','now'))")
        .bind(&id).bind(request.user_id).bind(&request_id).bind(payload.to_string()).bind(request.identity.to_string()).bind(parent_id)
        .bind(request.storage_id).bind(&path).bind(paths.len() as i64).execute(&mut *tx).await?;
    for path in paths {
        sqlx::query(
            "INSERT INTO storage_operation_items(operation_id,path,state) VALUES(?,?,'pending')",
        )
        .bind(&id)
        .bind(path)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok((id, true))
}

pub(crate) fn normalize_delete_selection<'a>(
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<String>, StorageRecoveryError> {
    use StorageRecoveryError::InvalidRequest;
    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if path.trim().is_empty() || path.chars().count() > 1024 {
            return Err(InvalidRequest);
        }
        // Invalid lexical paths remain itemized, but are never sent to a provider.
        let path = relative_path(path, false).unwrap_or_else(|| path.to_owned());
        if seen.insert(path.clone()) {
            selected.push(path);
        }
        if selected.len() > 1000 {
            return Err(InvalidRequest);
        }
    }
    if selected.is_empty() {
        return Err(InvalidRequest);
    }
    Ok(selected)
}

macro_rules! snapshot_sql {
    ($suffix:literal) => { concat!("SELECT json_object(
            'id',op.id,'action',op.action,'path',op.path,'state',op.state,'stage',op.stage,
            'total',op.total,'completed',op.completed,'succeeded',op.succeeded,
            'failed',op.failed,'skipped',op.skipped,'unknown',op.unknown,
            'cancel_requested',json(CASE WHEN op.cancel_requested THEN 'true' ELSE 'false' END),
            'parent_id',op.parent_id,
            'storage_id',CASE WHEN op.storage_id='local' THEN 'local' ELSE CAST(op.storage_id AS INTEGER) END,
            'created_at',op.created_at,'updated_at',op.updated_at,'finished_at',op.finished_at,
            'items',json((SELECT json_group_array(json_object('path',path,'state',state,'error',error))
                     FROM (SELECT path,state,error FROM storage_operation_items WHERE operation_id=op.id ORDER BY id))))
         FROM storage_operations AS op WHERE op.user_id=? AND op.action='storage_delete'", $suffix) };
}

/// An owner-scoped snapshot; the headline counters and items are read together.
pub async fn storage_operation_detail(
    pool: &SqlitePool,
    user_id: i64,
    operation_id: &str,
) -> Result<Option<serde_json::Value>, StorageRecoveryError> {
    let row: Option<String> = sqlx::query_scalar(snapshot_sql!(" AND op.id=?"))
        .bind(user_id)
        .bind(operation_id)
        .fetch_optional(pool)
        .await?;
    row.map(decode_snapshot).transpose()
}

fn delete_payload(
    storage: &str,
    path: &str,
    paths: &[String],
    parent: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({"action":"storage_delete", "storage_id":storage, "path":path, "paths":paths, "parent_id":parent})
}

async fn matching_delete(
    connection: &mut sqlx::SqliteConnection,
    user_id: i64,
    request_id: &str,
    payload: &serde_json::Value,
) -> Result<Option<String>, StorageRecoveryError> {
    let existing: Option<(String, String)> = sqlx::query_as(
        "SELECT id,payload FROM storage_operations WHERE user_id=? AND request_id=?",
    )
    .bind(user_id)
    .bind(request_id)
    .fetch_optional(connection)
    .await?;
    match existing {
        None => Ok(None),
        Some((id, saved))
            if serde_json::from_str::<serde_json::Value>(&saved)
                .ok()
                .as_ref()
                == Some(payload) =>
        {
            Ok(Some(id))
        }
        Some(_) => Err(StorageRecoveryError::RequestConflict),
    }
}

pub(crate) async fn replay_remote_delete(
    pool: &SqlitePool,
    user_id: i64,
    id: i64,
    request_id: &str,
    path: &str,
    paths: &[String],
) -> Result<Option<String>, StorageRecoveryError> {
    use StorageRecoveryError as Error;
    let normalized = normalize_delete_selection(paths.iter().map(String::as_str))?;
    let path = relative_path(path, true).ok_or(Error::InvalidRequest)?;
    let payload = delete_payload(&id.to_string(), &path, &normalized, None);
    let mut tx = pool.begin().await?;
    if recovery_pending_in(&mut *tx)
        .await
        .map_err(Error::Activation)?
    {
        return Err(Error::RecoveryPending);
    }
    if let Some(operation) = matching_delete(&mut tx, user_id, request_id, &payload).await? {
        return Ok(Some(operation));
    }
    let supported: Option<(bool, String)> = sqlx::query_as(
        "SELECT COALESCE(enabled != 0, 0),storage_type FROM remote_storages WHERE id=?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    Err(match supported {
        None => Error::StorageNotFound,
        Some((false, _)) => Error::StorageDisabled,
        Some((true, kind)) if matches!(kind.as_str(), "local" | "webdav" | "ftp" | "s3") => {
            return Ok(None);
        }
        Some((true, _)) => Error::UnsupportedProvider,
    })
}

/// Produce the published Python storage configuration digest before provider dispatch.
pub async fn remote_storage_identity(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<serde_json::Value>, StorageRecoveryError> {
    remote_row(pool, id)
        .await?
        .as_ref()
        .map(remote_identity)
        .transpose()
}

async fn remote_row<'e>(
    executor: impl sqlx::SqliteExecutor<'e>,
    id: i64,
) -> Result<Option<sqlx::sqlite::SqliteRow>, sqlx::Error> {
    sqlx::query("SELECT base_path,created_at,enabled,host,id,name,password,port,rclone_remote,s3_access_key,s3_bucket,s3_endpoint_url,s3_region,s3_secret_key,ssh_key_path,storage_type,updated_at,username,webdav_url FROM remote_storages WHERE id=?")
        .bind(id).fetch_optional(executor).await
}

async fn matches_remote_configuration(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    expected: &serde_json::Value,
) -> Result<bool, StorageRecoveryError> {
    let Some(row) = remote_row(connection, id).await? else {
        return Ok(false);
    };
    match remote_identity(&row) {
        Ok(value) => Ok(value["config"] == expected["config"]),
        Err(StorageRecoveryError::IdentityChanged) => Ok(false),
        Err(error) => Err(error),
    }
}

fn remote_identity(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<serde_json::Value, StorageRecoveryError> {
    use dockervault_core::{encoding::python_string, timestamp::Timestamp};
    use sha2::{Digest, Sha256};
    use sqlx::Row;

    let mut entries = Vec::new();
    // The select list is sorted to match json.dumps(sort_keys=True).
    for column in row.columns() {
        use sqlx::Column;
        let name = column.name();
        let value = match name {
            "enabled" => row
                .try_get::<Option<bool>, _>(name)?
                .map(|b| if b { "True" } else { "False" }.to_owned()),
            "port" | "id" => row.try_get::<Option<i64>, _>(name)?.map(|v| v.to_string()),
            "created_at" | "updated_at" => row
                .try_get::<Option<String>, _>(name)?
                .map(|raw| {
                    Timestamp::parse(&raw)
                        .map(|timestamp| timestamp.api().replacen('T', " ", 1))
                        .map_err(|_| StorageRecoveryError::IdentityChanged)
                })
                .transpose()?,
            _ => row.try_get::<Option<String>, _>(name)?,
        };
        entries.push(format!(
            "{}: {}",
            python_string(name),
            python_string(value.as_deref().unwrap_or("None"))
        ));
    }
    let encoded = format!("{{{}}}", entries.join(", "));
    let digest: String = Sha256::digest(encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(serde_json::json!({"config":digest}))
}

pub async fn storage_operation_list(
    pool: &SqlitePool,
    user_id: i64,
    storage: Option<&str>,
    limit: i64,
) -> Result<Vec<serde_json::Value>, StorageRecoveryError> {
    if !(1..=100).contains(&limit) {
        return Err(StorageRecoveryError::InvalidRequest);
    }
    let rows: Vec<String> = sqlx::query_scalar(snapshot_sql!(
        " AND (? IS NULL OR op.storage_id=?)
        ORDER BY (op.state IN ('queued','running')) DESC, op.created_at DESC, op.id DESC LIMIT ?"
    ))
    .bind(user_id)
    .bind(storage)
    .bind(storage)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(decode_snapshot).collect()
}

fn decode_snapshot(row: String) -> Result<serde_json::Value, StorageRecoveryError> {
    use dockervault_core::timestamp::Timestamp;
    let mut snapshot: serde_json::Value =
        serde_json::from_str(&row).map_err(|_| StorageRecoveryError::InvalidRequest)?;
    for field in ["created_at", "updated_at", "finished_at"] {
        if let Some(raw) = snapshot[field].as_str() {
            let timestamp =
                Timestamp::parse(raw).map_err(|_| StorageRecoveryError::InvalidRequest)?;
            snapshot[field] = format!("{}Z", timestamp.api()).into();
        }
    }
    Ok(snapshot)
}

#[derive(Debug)]
pub struct ClaimedItem {
    pub id: i64,
    pub path: String,
}

/// Commit a single owned delete claim before touching the provider. The caller
/// must hold the verified storage handle through the effect and settlement.
pub async fn claim_next(
    pool: &SqlitePool,
    user_id: i64,
    operation_id: &str,
    current_identity: &serde_json::Value,
) -> Result<Option<ClaimedItem>, StorageRecoveryError> {
    claim_delete(pool, user_id, operation_id, current_identity, None).await
}

async fn claim_delete(
    pool: &SqlitePool,
    user_id: i64,
    operation_id: &str,
    current_identity: &serde_json::Value,
    remote_id: Option<i64>,
) -> Result<Option<ClaimedItem>, StorageRecoveryError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if recovery_pending_in(&mut *tx)
        .await
        .map_err(StorageRecoveryError::Activation)?
    {
        tx.rollback().await?;
        return Ok(None);
    }
    let identity: Option<String> = sqlx::query_scalar(
        "SELECT storage_identity FROM storage_operations WHERE id=? AND user_id=?
         AND action='storage_delete' AND state IN ('queued','running') AND cancel_requested=0",
    )
    .bind(operation_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(identity) = identity else {
        tx.rollback().await?;
        return Ok(None);
    };
    if !current_identity.is_object()
        || serde_json::from_str::<serde_json::Value>(&identity)
            .as_ref()
            .ok()
            != Some(current_identity)
    {
        tx.rollback().await?;
        return Err(StorageRecoveryError::IdentityChanged);
    }
    if let Some(id) = remote_id
        && !matches_remote_configuration(&mut tx, id, current_identity).await?
    {
        sqlx::query("UPDATE storage_operation_items SET state='failed',error='Storage identity changed or unavailable; target not deleted' WHERE operation_id=? AND state='pending'")
            .bind(operation_id).execute(&mut *tx).await?;
        refresh_settled(&mut tx, operation_id).await?;
        tx.commit().await?;
        return Ok(None);
    }
    let item: Option<(i64, String)> = sqlx::query_as(
        "UPDATE storage_operation_items SET state='running'
         WHERE id=(SELECT id FROM storage_operation_items WHERE operation_id=? AND state='pending' ORDER BY id LIMIT 1)
           AND NOT EXISTS(SELECT 1 FROM storage_operation_items WHERE operation_id=? AND state='running')
         RETURNING id,path",
    ).bind(operation_id).bind(operation_id).fetch_optional(&mut *tx).await?;
    if item.is_some() {
        sqlx::query(
            "UPDATE storage_operations SET state='running',stage='deleting',
                     updated_at=strftime('%Y-%m-%d %H:%M:%f','now') WHERE id=?",
        )
        .bind(operation_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(item.map(|(id, path)| ClaimedItem { id, path }))
}

#[derive(Debug, Clone, Copy)]
pub enum DeleteOutcome {
    Succeeded,
    Failed,
    InvalidPath,
    NotFound,
    PermissionDenied,
    Unknown,
}

/// Record the settled physical outcome once. An unconfirmed provider response
/// is unknown even when the user has requested cancellation.
pub async fn record_settled(
    pool: &SqlitePool,
    user_id: i64,
    operation_id: &str,
    item_id: i64,
    outcome: DeleteOutcome,
) -> Result<bool, StorageRecoveryError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (state, message) = match outcome {
        DeleteOutcome::Succeeded => ("succeeded", None),
        DeleteOutcome::Failed => ("failed", Some("Delete failed")),
        DeleteOutcome::InvalidPath => ("failed", Some("Invalid file path")),
        DeleteOutcome::NotFound => ("failed", Some("Target not found")),
        DeleteOutcome::PermissionDenied => ("failed", Some("Permission denied")),
        DeleteOutcome::Unknown => (
            "unknown",
            Some("Delete not confirmed; inspect the actual target"),
        ),
    };
    let updated = sqlx::query(
        "UPDATE storage_operation_items SET state=?,error=? WHERE id=? AND operation_id=? AND state='running'
         AND EXISTS(SELECT 1 FROM storage_operations WHERE id=? AND user_id=? AND action='storage_delete' AND state='running')",
    ).bind(state).bind(message).bind(item_id).bind(operation_id).bind(operation_id).bind(user_id)
        .execute(&mut *tx).await?.rows_affected();
    if updated == 0 {
        tx.rollback().await?;
        return Ok(false);
    }
    refresh_settled(&mut tx, operation_id).await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn request_cancel(
    pool: &SqlitePool,
    user_id: i64,
    operation_id: &str,
) -> Result<bool, StorageRecoveryError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let state: Option<String> = sqlx::query_scalar(
        "SELECT state FROM storage_operations WHERE id=? AND user_id=? AND action='storage_delete'",
    )
    .bind(operation_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(state) = &state
        && matches!(state.as_str(), "queued" | "running")
    {
        sqlx::query("UPDATE storage_operations SET cancel_requested=1,updated_at=strftime('%Y-%m-%d %H:%M:%f','now') WHERE id=?")
            .bind(operation_id).execute(&mut *tx).await?;
        let running: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM storage_operation_items WHERE operation_id=? AND state='running')")
            .bind(operation_id).fetch_one(&mut *tx).await?;
        if !running {
            refresh_settled(&mut tx, operation_id).await?;
        }
    }
    tx.commit().await?;
    Ok(state.is_some())
}

async fn refresh_settled(
    tx: &mut sqlx::SqliteConnection,
    operation_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE storage_operation_items SET state='skipped',error='Not started; cancellation requested'
         WHERE operation_id=? AND state='pending'
           AND EXISTS(SELECT 1 FROM storage_operations WHERE id=? AND cancel_requested=1)",
    ).bind(operation_id).bind(operation_id).execute(&mut *tx).await?;
    let (succeeded, failed, skipped, unknown, active): (i64,i64,i64,i64,i64) = sqlx::query_as(
        "SELECT count(CASE WHEN state='succeeded' THEN 1 END), count(CASE WHEN state='failed' THEN 1 END),
                count(CASE WHEN state='skipped' THEN 1 END), count(CASE WHEN state='unknown' THEN 1 END),
                count(CASE WHEN state IN ('pending','running') THEN 1 END)
         FROM storage_operation_items WHERE operation_id=?",
    ).bind(operation_id).fetch_one(&mut *tx).await?;
    let next = if active != 0 {
        "running"
    } else if failed != 0 || unknown != 0 {
        "failed"
    } else if skipped != 0 {
        "cancelled"
    } else {
        "completed"
    };
    sqlx::query(
        "UPDATE storage_operations SET succeeded=?,failed=?,skipped=?,unknown=?,completed=?,state=?,
           stage=CASE WHEN ?='running' THEN 'deleting' ELSE 'finished' END,
           finished_at=CASE WHEN ?='running' THEN NULL ELSE strftime('%Y-%m-%d %H:%M:%f','now') END,
           updated_at=strftime('%Y-%m-%d %H:%M:%f','now') WHERE id=?",
    ).bind(succeeded).bind(failed).bind(skipped).bind(unknown).bind(succeeded+failed+skipped+unknown)
        .bind(next).bind(next).bind(next).bind(operation_id).execute(&mut *tx).await?;
    Ok(())
}

/// Binds a durable claim to a supervised SFTP effect. Caller supplies the
/// endpoint and identity from the same pinned storage configuration.
pub struct SftpDeleteWorker {
    pool: SqlitePool,
    jobs: Owner,
    processes: ProcessOwner,
    transport: Owner,
}

impl SftpDeleteWorker {
    pub fn new(pool: SqlitePool, jobs: Owner, processes: ProcessOwner, transport: Owner) -> Self {
        Self {
            pool,
            jobs,
            processes,
            transport,
        }
    }

    pub fn start(
        &self,
        endpoint: SftpEndpoint,
        user_id: i64,
        operation_id: String,
        identity: serde_json::Value,
    ) -> Result<Work<Result<bool, StorageRecoveryError>>, WorkError> {
        let pool = self.pool.clone();
        let processes = self.processes.clone();
        let transport = self.transport.clone();
        self.jobs.submit_async(move |cancel| async move {
            if cancel.is_cancelled() {
                return Ok(false);
            }
            let Some(item) = claim_next(&pool, user_id, &operation_id, &identity).await? else {
                return Ok(false);
            };
            // Once claimed, do not relay caller cancellation to a physical mutation.
            let outcome = match relative_path(&item.path, false) {
                Some(path) => match endpoint.delete(&processes, &transport, &path) {
                    Ok(effect) => match effect.wait().await {
                        Ok(Ok(())) => DeleteOutcome::Succeeded,
                        Ok(Err(SftpDownloadError::MutationUnverified)) | Err(_) => {
                            DeleteOutcome::Unknown
                        }
                        Ok(Err(_)) => DeleteOutcome::Failed,
                    },
                    Err(_) => DeleteOutcome::Failed,
                },
                None => DeleteOutcome::InvalidPath,
            };
            record_settled(&pool, user_id, &operation_id, item.id, outcome).await
        })
    }
}

pub fn start_local_delete(
    pool: &SqlitePool,
    jobs: &Owner,
    local: LocalStorage,
    user_id: i64,
    operation_id: String,
    identity: serde_json::Value,
) -> Result<Work<Result<bool, StorageRecoveryError>>, WorkError> {
    let pool = pool.clone();
    jobs.submit_async(move |cancel| async move {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        delete_next_local(&pool, &local, user_id, &operation_id, &identity, None).await
    })
}

async fn delete_next_local(
    pool: &SqlitePool,
    local: &LocalStorage,
    user_id: i64,
    operation_id: &str,
    identity: &serde_json::Value,
    remote_id: Option<i64>,
) -> Result<bool, StorageRecoveryError> {
    let expected_root = serde_json::from_value::<(u64, u64)>(identity["root_inode"].clone())
        .map_err(|_| StorageRecoveryError::IdentityChanged)?;
    let Some(item) = claim_delete(pool, user_id, operation_id, identity, remote_id).await? else {
        return Ok(false);
    };
    let outcome = match relative_path(&item.path, false) {
        Some(path) => match local.delete_target(path.into(), expected_root) {
            Ok(effect) => match effect.wait().await {
                Ok(Ok(())) => DeleteOutcome::Succeeded,
                Ok(Err(TransferError::PublicationUncertain(_))) | Err(_) => DeleteOutcome::Unknown,
                Ok(Err(TransferError::Io(error))) => match error.kind() {
                    std::io::ErrorKind::NotFound => DeleteOutcome::NotFound,
                    std::io::ErrorKind::PermissionDenied => DeleteOutcome::PermissionDenied,
                    std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotADirectory => {
                        DeleteOutcome::InvalidPath
                    }
                    _ => DeleteOutcome::Failed,
                },
            },
            Err(_) => DeleteOutcome::Failed,
        },
        None => DeleteOutcome::InvalidPath,
    };
    record_settled(pool, user_id, operation_id, item.id, outcome).await
}

pub(crate) struct LocalDeleteJobs {
    jobs: Owner,
    io: Owner,
    tail: Mutex<Option<oneshot::Receiver<()>>>,
    stopped: CancellationToken,
}

pub(crate) struct LocalSelection {
    pub user_id: i64,
    pub request_id: String,
    pub path: String,
    pub paths: Vec<String>,
    pub remote_id: Option<i64>,
    pub parent: Option<RetryParent>,
}

enum DeleteTarget {
    Local(LocalStorage),
    Dav(WebDav),
    Ftp(Ftp),
    S3(S3),
}

impl DeleteTarget {
    async fn next(
        &self,
        pool: &SqlitePool,
        user: i64,
        operation: &str,
        identity: &serde_json::Value,
        remote_id: Option<i64>,
    ) -> Result<bool, StorageRecoveryError> {
        if let Self::Local(local) = self {
            return delete_next_local(pool, local, user, operation, identity, remote_id).await;
        }
        let Some(item) = claim_delete(pool, user, operation, identity, remote_id).await? else {
            return Ok(false);
        };
        let outcome = if let Some(path) = relative_path(&item.path, false) {
            match self {
                Self::S3(s3) => match s3.delete(path) {
                    Ok(effect) => match effect.wait().await {
                        Ok(Ok(())) => DeleteOutcome::Succeeded,
                        Ok(Err(error))
                            if matches!(error.http_status(), Some(400 | 401 | 403 | 404)) =>
                        {
                            DeleteOutcome::Failed
                        }
                        _ => DeleteOutcome::Unknown,
                    },
                    Err(_) => DeleteOutcome::Failed,
                },
                Self::Ftp(ftp) => match ftp.delete(path) {
                    Ok(effect) => match effect.wait().await {
                        Ok(Ok(())) => DeleteOutcome::Succeeded,
                        Ok(Err(FtpError::Reply(_) | FtpError::Config)) => DeleteOutcome::Failed,
                        _ => DeleteOutcome::Unknown,
                    },
                    Err(_) => DeleteOutcome::Failed,
                },
                Self::Dav(dav) => match dav.delete(path) {
                    Ok(effect) => match effect.wait().await {
                        Ok(Ok(())) => DeleteOutcome::Succeeded,
                        Ok(Err(
                            DavError::Transport | DavError::Cancelled | DavError::Http(500..=599),
                        ))
                        | Err(_) => DeleteOutcome::Unknown,
                        Ok(Err(_)) => DeleteOutcome::Failed,
                    },
                    Err(_) => DeleteOutcome::Failed,
                },
                Self::Local(_) => unreachable!(),
            }
        } else {
            DeleteOutcome::InvalidPath
        };
        record_settled(pool, user, operation, item.id, outcome).await
    }
}

pub(crate) struct RetryParent {
    id: String,
    identity: serde_json::Value,
}

impl LocalSelection {
    pub async fn retry(
        pool: &SqlitePool,
        user_id: i64,
        id: String,
        request_id: String,
    ) -> Result<Self, StorageRecoveryError> {
        use StorageRecoveryError as Error;
        let Some((storage, state, path, identity)) = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT storage_id,state,path,storage_identity FROM storage_operations WHERE id=? AND user_id=? AND action='storage_delete'",
        ).bind(&id).bind(user_id).fetch_optional(pool).await? else {
            return Err(Error::OperationNotFound);
        };
        if matches!(state.as_str(), "queued" | "running") {
            return Err(Error::OperationActive);
        }
        // Terminal items are immutable; an unknown outcome must never be retried.
        let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM storage_operation_items WHERE operation_id=? AND state='failed' ORDER BY id LIMIT 1001")
            .bind(&id).fetch_all(pool).await?;
        if paths.is_empty() {
            return Err(Error::NothingToRetry);
        }
        if storage != "local" {
            return Err(Error::UnsupportedProvider);
        }
        let identity = serde_json::from_str(&identity).map_err(|_| Error::IdentityChanged)?;
        Ok(Self {
            user_id,
            request_id,
            path,
            paths,
            remote_id: None,
            parent: Some(RetryParent { id, identity }),
        })
    }
}

impl LocalDeleteJobs {
    async fn pin_local(
        &self,
        path: String,
        identity: &mut serde_json::Value,
    ) -> Option<DeleteTarget> {
        let io = self.io.clone();
        let result = match self
            .io
            .submit_blocking(move |_| LocalStorage::pin_configured(std::path::Path::new(&path), io))
        {
            Ok(work) => work.wait().await.ok().and_then(Result::ok),
            Err(_) => None,
        };
        if let Some((local, inode)) = result {
            identity["root_inode"] = serde_json::json!(inode);
            Some(DeleteTarget::Local(local))
        } else {
            *identity = serde_json::Value::Null;
            None
        }
    }

    pub fn new() -> Self {
        Self {
            jobs: Owner::new(64).expect("bounded storage admission"),
            io: Owner::new(2).expect("one root check and one effect"),
            tail: Mutex::new(None),
            stopped: CancellationToken::new(),
        }
    }

    pub async fn submit(
        self: &Arc<Self>,
        pool: SqlitePool,
        config: Arc<crate::config::Config>,
        selection: LocalSelection,
        draining: CancellationToken,
        notify: impl Fn(i64, &str) + Send + Sync + 'static,
    ) -> Result<String, StorageRecoveryError> {
        use StorageRecoveryError as Error;
        let this = self.clone();
        let (accepted, response) = oneshot::channel();
        // Once admitted, neither a lost HTTP reply nor a retry owns the job.
        let worker = self
            .jobs
            .submit_async(move |_| async move {
                let mut tail = this.tail.lock().await;
                let io = this.io.clone();
                let root = &config.backup_base_path;
                let prepared = async {
                    use sqlx::Row;
                    if let Some(id) = selection.remote_id {
                        let row = remote_row(&pool, id).await?.ok_or(Error::StorageNotFound)?;
                        if !row.try_get::<Option<bool>, _>("enabled")?.unwrap_or(false) {
                            return Err(Error::StorageDisabled);
                        }
                        let mut identity = remote_identity(&row)?;
                        let kind: String = row.try_get("storage_type")?;
                        let base: Option<String> = row.try_get("base_path")?;
                        if matches!(kind.as_str(), "webdav" | "ftp" | "s3") {
                            let password: Option<String> = row.try_get(if kind == "s3" {
                                "s3_secret_key"
                            } else {
                                "password"
                            })?;
                            let password = match password {
                                Some(value) if value.starts_with("fernet:") => {
                                    let config = config.clone();
                                    let work = io
                                        .submit_blocking(move |cancel| {
                                            load_delete_cipher(
                                                std::path::Path::new("/app/data/.credential_key"),
                                                config.credential_encryption_key.expose(),
                                                || cancel.is_cancelled(),
                                            )?
                                            .decrypt_value(&value)
                                            .map_err(|_| Error::InvalidStorageConfiguration)
                                        })
                                        .map_err(|_| Error::InvalidStorageConfiguration)?;
                                    Some(
                                        work.wait()
                                            .await
                                            .map_err(|_| Error::InvalidStorageConfiguration)??,
                                    )
                                }
                                value => value,
                            };
                            if kind == "s3" {
                                let s3 = S3::for_claimed_delete(
                                    S3Config {
                                        endpoint_url: row.try_get("s3_endpoint_url")?,
                                        region: row
                                            .try_get::<Option<String>, _>("s3_region")?
                                            .unwrap_or_default(),
                                        bucket: row
                                            .try_get::<Option<String>, _>("s3_bucket")?
                                            .ok_or(Error::InvalidStorageConfiguration)?,
                                        base_path: base.unwrap_or_default(),
                                        access_key: row
                                            .try_get::<Option<String>, _>("s3_access_key")?
                                            .ok_or(Error::InvalidStorageConfiguration)?,
                                        secret_key: password
                                            .ok_or(Error::InvalidStorageConfiguration)?,
                                    },
                                    io.clone(),
                                )
                                .map_err(|_| Error::InvalidStorageConfiguration)?;
                                return Ok((Some(DeleteTarget::S3(s3)), identity));
                            }
                            if kind == "ftp" {
                                let port = row
                                    .try_get::<Option<i64>, _>("port")?
                                    .map(|port| {
                                        u16::try_from(port)
                                            .ok()
                                            .filter(|port| *port > 0)
                                            .ok_or(Error::InvalidStorageConfiguration)
                                    })
                                    .transpose()?;
                                let ftp = Ftp::new(
                                    FtpConfig {
                                        host: row
                                            .try_get::<Option<String>, _>("host")?
                                            .ok_or(Error::InvalidStorageConfiguration)?,
                                        port,
                                        username: row
                                            .try_get::<Option<String>, _>("username")?
                                            .unwrap_or_default(),
                                        password: password.unwrap_or_default(),
                                        base_path: base.unwrap_or_default(),
                                    },
                                    io.clone(),
                                )
                                .map_err(|_| Error::InvalidStorageConfiguration)?;
                                return Ok((Some(DeleteTarget::Ftp(ftp)), identity));
                            }
                            let dav = WebDav::new(
                                DavConfig {
                                    url: row
                                        .try_get::<Option<String>, _>("webdav_url")?
                                        .ok_or(Error::InvalidStorageConfiguration)?,
                                    base_path: base.unwrap_or_default(),
                                    username: row.try_get("username")?,
                                    password,
                                    max_attempts: 1,
                                    verify_head: false,
                                },
                                io.clone(),
                                io.clone(),
                            )
                            .map_err(|_| Error::InvalidStorageConfiguration)?;
                            return Ok((Some(DeleteTarget::Dav(dav)), identity));
                        }
                        if kind != "local" {
                            return Err(Error::UnsupportedProvider);
                        }
                        let root = base
                            .filter(|path| !path.is_empty())
                            .ok_or(Error::IdentityChanged)?;
                        let local = this.pin_local(root, &mut identity).await;
                        Ok((local, identity))
                    } else {
                        let mut identity = serde_json::json!({"root":root});
                        let local = this.pin_local(root.clone(), &mut identity).await;
                        Ok((local, identity))
                    }
                }
                .await;
                let (target, identity) = match prepared {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let _ = accepted.send(Err(error));
                        return;
                    }
                };
                let storage_id = selection
                    .remote_id
                    .map_or_else(|| "local".to_owned(), |id| id.to_string());
                let paths: Vec<&str> = selection.paths.iter().map(String::as_str).collect();
                let admission = admit_delete(
                    &pool,
                    DeleteAdmission {
                        user_id: selection.user_id,
                        request_id: &selection.request_id,
                        storage_id: &storage_id,
                        path: &selection.path,
                        paths: &paths,
                        identity: &identity,
                        accepting: !this.stopped.is_cancelled() && !draining.is_cancelled(),
                    },
                    selection.parent.as_ref(),
                    selection.remote_id,
                )
                .await;
                let (id, target) = match (admission, target) {
                    (Ok((id, true)), Some(target)) => (id, target),
                    (Ok((id, false)), _) => {
                        let _ = accepted.send(Ok(id));
                        return;
                    }
                    (Err(error), _) => {
                        let _ = accepted.send(Err(error));
                        return;
                    }
                    _ => {
                        let _ = accepted.send(Err(Error::IdentityChanged));
                        return;
                    }
                };
                // Link the next turn while under the admission lock: commit order is wire order.
                let (done, turn) = oneshot::channel();
                let previous = tail.replace(turn);
                drop(tail);
                let _ = accepted.send(Ok(id.clone()));
                if let Some(previous) = previous
                    && previous.await.is_err()
                {
                    this.stopped.cancel();
                }
                let result = async {
                    let user = selection.user_id;
                    loop {
                        if this.stopped.is_cancelled() {
                            break;
                        }
                        if draining.is_cancelled() {
                            request_cancel(&pool, user, &id).await?;
                            break;
                        }
                        let more = target
                            .next(&pool, user, &id, &identity, selection.remote_id)
                            .await?;
                        notify(user, &id);
                        if !more {
                            break;
                        }
                    }
                    Result::<(), Error>::Ok(())
                }
                .await;
                if result.is_err() {
                    this.stopped.cancel();
                    eprintln!("Storage worker settlement failed; new deletion admission disabled");
                }
                let _ = done.send(());
            })
            .map_err(|error| match error {
                WorkError::Busy => Error::QueueFull,
                _ => Error::ShuttingDown,
            })?;
        let result = response.await.map_err(|_| Error::ShuttingDown)?;
        drop(worker);
        result
    }

    pub async fn shutdown(&self) {
        self.jobs.shutdown(None).await;
        self.io.shutdown(None).await;
    }
}

fn load_delete_cipher(
    path: &std::path::Path,
    configured: &str,
    cancelled: impl Fn() -> bool,
) -> Result<dockervault_core::crypto::CredentialCipher, StorageRecoveryError> {
    use dockervault_core::crypto::CredentialCipher;
    use nix::fcntl::{OFlag, openat};
    use std::os::unix::fs::MetadataExt;
    let invalid = || StorageRecoveryError::InvalidStorageConfiguration;
    if cancelled() {
        return Err(invalid());
    }
    if !configured.is_empty() {
        return CredentialCipher::new(configured).map_err(|_| invalid());
    }
    let dir = crate::storage::local::open_root(path.parent().ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    let file = std::fs::File::from(
        openat(
            &dir,
            path.file_name().ok_or_else(invalid)?,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::empty(),
        )
        .map_err(|_| invalid())?,
    );
    let meta = file.metadata().map_err(|_| invalid())?;
    if !meta.is_file()
        || meta.mode() & 0o777 != 0o600
        || meta.nlink() != 1
        || meta.uid() != nix::unistd::geteuid().as_raw()
    {
        return Err(invalid());
    }
    CredentialCipher::read_key(file, cancelled).map_err(|_| invalid())
}

#[cfg(test)]
#[test]
fn configured_key_precedes_existing_file_and_unsafe_files_are_not_followed() {
    use dockervault_core::crypto::CredentialCipher;
    use std::os::unix::fs::{OpenOptionsExt, symlink};

    use std::io::Write;
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).unwrap();
    let dir = std::env::temp_dir().join(format!("dv-delete-key-{nonce:02x?}"));
    std::fs::create_dir(&dir).unwrap();
    let key = CredentialCipher::generate_key();
    let path = dir.join("key");
    let wrong = CredentialCipher::generate_key();
    let encrypted = CredentialCipher::new(&key)
        .unwrap()
        .encrypt_value("synthetic");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    writeln!(file, "{}", key.as_str()).unwrap();
    drop(file);
    assert_eq!(
        load_delete_cipher(&path, "", || false)
            .unwrap()
            .decrypt_value(&encrypted)
            .unwrap(),
        "synthetic"
    );
    assert!(
        load_delete_cipher(&path, &wrong, || false)
            .unwrap()
            .decrypt_value(&encrypted)
            .is_err()
    );
    assert!(load_delete_cipher(&path, "invalid", || false).is_err());
    symlink(&path, dir.join("link")).unwrap();
    assert!(load_delete_cipher(&dir.join("link"), "", || false).is_err());
    assert!(load_delete_cipher(&dir.join("missing"), "", || false).is_err());
    assert!(!dir.join("missing").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

impl From<sqlx::Error> for StorageRecoveryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

/// Only call after fencing the previous writer and before admitting storage work.
/// Nothing is retried: an interrupted remote operation may already have taken effect.
pub async fn settle_interrupted(pool: &SqlitePool) -> Result<u64, StorageRecoveryError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if recovery_pending_in(&mut *tx)
        .await
        .map_err(StorageRecoveryError::Activation)?
    {
        tx.rollback().await?;
        return Ok(0);
    }
    sqlx::query(
        "UPDATE storage_operation_items
         SET error=CASE state WHEN 'running' THEN 'Interrupted; inspect the actual target before taking further action'
                              ELSE 'Not started before interruption' END,
             state=CASE state WHEN 'running' THEN 'unknown' ELSE 'skipped' END
         WHERE state IN ('running','pending') AND operation_id IN
               (SELECT id FROM storage_operations WHERE state IN ('queued','running'))",
    )
    .execute(&mut *tx)
    .await?;
    let result = sqlx::query(
        "UPDATE storage_operations AS op
         SET state='failed',stage='interrupted',
             finished_at=strftime('%Y-%m-%d %H:%M:%f','now'), updated_at=strftime('%Y-%m-%d %H:%M:%f','now'),
             succeeded=(SELECT COUNT(*) FROM storage_operation_items WHERE operation_id=op.id AND state='succeeded'),
             failed=(SELECT COUNT(*) FROM storage_operation_items WHERE operation_id=op.id AND state='failed'),
             skipped=(SELECT COUNT(*) FROM storage_operation_items WHERE operation_id=op.id AND state='skipped'),
             unknown=(SELECT COUNT(*) FROM storage_operation_items WHERE operation_id=op.id AND state='unknown'),
             completed=(SELECT COUNT(*) FROM storage_operation_items WHERE operation_id=op.id AND state IN ('succeeded','failed','skipped','unknown'))
         WHERE op.state IN ('queued','running')",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected())
}
