//! Atomic restart settlement for interrupted user-owned storage operations.
use crate::{
    ownership::{Owner, Work, WorkError},
    process::ProcessOwner,
    recovery_activation::{JournalError, recovery_pending_in},
    storage::{
        relative_path,
        ssh::{SftpDownloadError, SftpEndpoint},
    },
};
use sqlx::SqlitePool;

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
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &candidate in request.paths {
        if candidate.trim().is_empty() || candidate.chars().count() > 1024 {
            return Err(Error::InvalidRequest);
        }
        let candidate = relative_path(candidate, false).unwrap_or_else(|| candidate.to_owned());
        if seen.insert(candidate.clone()) {
            paths.push(candidate);
        }
        if paths.len() > 1000 {
            return Err(Error::InvalidRequest);
        }
    }
    if paths.is_empty() {
        return Err(Error::InvalidRequest);
    }
    let payload = serde_json::json!({"action":"storage_delete", "storage_id":request.storage_id, "path":path, "paths":paths, "parent_id":null});
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if recovery_pending_in(&mut *tx)
        .await
        .map_err(Error::Activation)?
    {
        return Err(Error::RecoveryPending);
    }
    let existing: Option<(String, String)> = sqlx::query_as(
        "SELECT id,payload FROM storage_operations WHERE user_id=? AND request_id=?",
    )
    .bind(request.user_id)
    .bind(&request_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((id, saved)) = existing {
        tx.rollback().await?;
        return if serde_json::from_str::<serde_json::Value>(&saved)
            .as_ref()
            .ok()
            == Some(&payload)
        {
            Ok(id)
        } else {
            Err(Error::RequestConflict)
        };
    }
    if !request.accepting {
        return Err(Error::ShuttingDown);
    }
    if !request.identity.is_object() {
        return Err(Error::IdentityChanged);
    }
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM storage_operations WHERE state IN ('queued','running')",
    )
    .fetch_one(&mut *tx)
    .await?;
    if active >= 32 {
        return Err(Error::QueueFull);
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
    sqlx::query("INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at)
                 VALUES(?,?,?,?,?,'storage_delete',?,?,'queued','queued',?,0,0,0,0,0,0,strftime('%Y-%m-%d %H:%M:%f','now'),strftime('%Y-%m-%d %H:%M:%f','now'))")
        .bind(&id).bind(request.user_id).bind(&request_id).bind(payload.to_string()).bind(request.identity.to_string())
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
    Ok(id)
}

macro_rules! snapshot_sql {
    ($suffix:literal) => { concat!("SELECT json_object(
            'id',op.id,'action',op.action,'path',op.path,'state',op.state,'stage',op.stage,
            'total',op.total,'completed',op.completed,'succeeded',op.succeeded,
            'failed',op.failed,'skipped',op.skipped,'unknown',op.unknown,
            'cancel_requested',json(CASE WHEN op.cancel_requested THEN 'true' ELSE 'false' END),
            'parent_id',op.parent_id,
            'storage_id',CASE WHEN op.storage_id='local' THEN 'local' ELSE CAST(op.storage_id AS INTEGER) END,
            'created_at',replace(op.created_at,' ','T')||'Z',
            'updated_at',replace(op.updated_at,' ','T')||'Z',
            'finished_at',replace(op.finished_at,' ','T')||'Z',
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
    serde_json::from_str(&row).map_err(|_| StorageRecoveryError::InvalidRequest)
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
