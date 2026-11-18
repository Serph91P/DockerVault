//! Atomic restart settlement for interrupted user-owned storage operations.
use crate::recovery_activation::{JournalError, recovery_pending_in};
use sqlx::SqlitePool;

#[derive(Debug)]
pub enum StorageRecoveryError {
    Activation(JournalError),
    Database(sqlx::Error),
    IdentityChanged,
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
         AND action='delete' AND state IN ('queued','running') AND cancel_requested=0",
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
