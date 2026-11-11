//! Owned, offline relational staging only; no live database, deployment key or
//! filesystem setup. Semantic recovery/manifest/path/cron checks are separate.
use crate::{db::schema, ownership::Owner};
use dockervault_core::{
    encoding::python_json,
    model::{BackupStatus, BackupType},
    portable_records::{PortableTables, TABLE_ORDER},
    timestamp::Timestamp,
};
use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordValidationError {
    Invalid,
    Cancelled,
    Owner,
}
impl std::fmt::Display for RecordValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "Invalid or incompatible recovery state",
            Self::Cancelled => "Operation cancelled",
            Self::Owner => "Service unavailable",
        })
    }
}
impl std::error::Error for RecordValidationError {}

pub async fn check_record_constraints(
    records: PortableTables,
    owner: &Owner,
) -> Result<(), RecordValidationError> {
    owner
        .submit_blocking(move |cancel| {
            check(&records, &cancel).map_err(|_| {
                if cancel.is_cancelled() {
                    RecordValidationError::Cancelled
                } else {
                    RecordValidationError::Invalid
                }
            })
        })
        .map_err(|_| RecordValidationError::Owner)?
        .wait()
        .await
        .map_err(|_| RecordValidationError::Owner)?
}
fn check(
    records: &PortableTables,
    cancel: &CancellationToken,
) -> Result<(), RecordValidationError> {
    if cancel.is_cancelled() {
        return Err(RecordValidationError::Cancelled);
    }
    let mut connection =
        Connection::open_in_memory().map_err(|_| RecordValidationError::Invalid)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| RecordValidationError::Invalid)?;
    connection
        .pragma_update(None, "temp_store", "MEMORY")
        .map_err(|_| RecordValidationError::Invalid)?;
    let progress = cancel.clone();
    connection
        .progress_handler(1000, Some(move || progress.is_cancelled()))
        .map_err(|_| RecordValidationError::Invalid)?;
    for (_, ddl, indexes) in schema::TABLES {
        connection
            .execute_batch(ddl)
            .map_err(|_| RecordValidationError::Invalid)?;
        for index in *indexes {
            connection
                .execute_batch(index)
                .map_err(|_| RecordValidationError::Invalid)?;
        }
    }
    let tables = serde_json::to_value(records).map_err(|_| RecordValidationError::Invalid)?;
    let transaction = connection
        .transaction()
        .map_err(|_| RecordValidationError::Invalid)?;
    for table in TABLE_ORDER {
        let mut info = transaction
            .prepare(&format!("PRAGMA table_info({})", quote(table)))
            .map_err(|_| RecordValidationError::Invalid)?;
        let columns: Vec<(String, String)> = info
            .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
            .map_err(|_| RecordValidationError::Invalid)?
            .collect::<Result<_, _>>()
            .map_err(|_| RecordValidationError::Invalid)?;
        drop(info);
        let names = columns
            .iter()
            .map(|(name, _)| quote(name))
            .collect::<Vec<_>>()
            .join(",");
        let placeholders = std::iter::repeat_n("?", columns.len())
            .collect::<Vec<_>>()
            .join(",");
        let mut insert = transaction
            .prepare(&format!(
                "INSERT INTO {} ({names}) VALUES ({placeholders})",
                quote(table)
            ))
            .map_err(|_| RecordValidationError::Invalid)?;
        for row in tables[table]
            .as_array()
            .ok_or(RecordValidationError::Invalid)?
        {
            if cancel.is_cancelled() {
                return Err(RecordValidationError::Cancelled);
            }
            if row.as_object().ok_or(RecordValidationError::Invalid)?.len() != columns.len() {
                return Err(RecordValidationError::Invalid);
            }
            let values = columns
                .iter()
                .map(|(name, kind)| {
                    sql_value(
                        table,
                        name,
                        kind,
                        row.get(name).ok_or(RecordValidationError::Invalid)?,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            insert
                .execute(params_from_iter(&values))
                .map_err(|_| RecordValidationError::Invalid)?;
        }
    }
    let mut foreign = transaction
        .prepare("PRAGMA foreign_key_check")
        .map_err(|_| RecordValidationError::Invalid)?;
    if foreign
        .query([])
        .map_err(|_| RecordValidationError::Invalid)?
        .next()
        .map_err(|_| RecordValidationError::Invalid)?
        .is_some()
    {
        return Err(RecordValidationError::Invalid);
    }
    drop(foreign);
    transaction
        .rollback()
        .map_err(|_| RecordValidationError::Invalid)?;
    Ok(())
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn sql_value(
    table: &str,
    column: &str,
    kind: &str,
    value: &Value,
) -> Result<SqlValue, RecordValidationError> {
    let invalid = RecordValidationError::Invalid;
    if kind == "JSON" {
        return Ok(SqlValue::Text(python_json(value)));
    }
    if value.is_null() {
        return Ok(SqlValue::Null);
    }
    if table == "backups" && column == "status" {
        let status: BackupStatus = serde_json::from_value(value.clone()).map_err(|_| invalid)?;
        return Ok(SqlValue::Text(status.db_name().to_owned()));
    }
    if table == "backups" && column == "backup_type" {
        let kind: BackupType = serde_json::from_value(value.clone()).map_err(|_| invalid)?;
        return Ok(SqlValue::Text(kind.db_name().to_owned()));
    }
    Ok(match kind {
        "INTEGER" => SqlValue::Integer(value.as_i64().ok_or(invalid)?),
        "BOOLEAN" => SqlValue::Integer(i64::from(value.as_bool().ok_or(invalid)?)),
        "DATETIME" => SqlValue::Text(
            Timestamp::parse(value.as_str().ok_or(invalid)?)
                .map_err(|_| invalid)?
                .sql(),
        ),
        _ => SqlValue::Text(value.as_str().ok_or(invalid)?.to_owned()),
    })
}
