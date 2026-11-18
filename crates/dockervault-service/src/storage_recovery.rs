//! Atomic restart settlement for interrupted user-owned storage operations.
use crate::recovery_activation::{JournalError, recovery_pending_in};
use sqlx::SqlitePool;

#[derive(Debug)]
pub enum StorageRecoveryError {
    Activation(JournalError),
    Database(sqlx::Error),
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
