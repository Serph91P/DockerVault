//! Owned, offline relational staging only; no live database, deployment key or
//! filesystem setup. Semantic recovery/manifest/path/cron checks are separate.
use crate::{db::schema, ownership::Owner};
use dockervault_core::{
    lossless_json::PythonJson,
    model::{BackupStatus, BackupType},
    portable_records::{PortableTables, TABLE_ORDER},
    timestamp::Timestamp,
};
use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};
use serde_json::{Value, value::RawValue};
use std::collections::BTreeMap;
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
pub(crate) fn check(
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
    let serialized = serde_json::to_string(records).map_err(|_| RecordValidationError::Invalid)?;
    let tables: BTreeMap<String, Vec<BTreeMap<String, &RawValue>>> =
        serde_json::from_str(&serialized).map_err(|_| RecordValidationError::Invalid)?;
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
        for row in &tables[table] {
            if cancel.is_cancelled() {
                return Err(RecordValidationError::Cancelled);
            }
            if row.len() != columns.len() {
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
                        cancel,
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
    value: &RawValue,
    cancel: &CancellationToken,
) -> Result<SqlValue, RecordValidationError> {
    let invalid = RecordValidationError::Invalid;
    if kind == "JSON" {
        let json = PythonJson::parse(value.get().as_bytes()).map_err(|_| invalid)?;
        return json
            .python(&|| cancel.is_cancelled())
            .map(SqlValue::Text)
            .map_err(|_| {
                if cancel.is_cancelled() {
                    RecordValidationError::Cancelled
                } else {
                    invalid
                }
            });
    }
    let value: Value = serde_json::from_str(value.get()).map_err(|_| invalid)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    #[test]
    fn actual_sqlite_json_binding_preserves_all_source_opaque_values_including_null_and_surrogates()
    {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../dockervault-core/tests/fixtures/recovery-raw-state.TEST_ONLY.json"
        ))
        .unwrap();
        let cancel = CancellationToken::new();
        for case in fixture["cases"].as_array().unwrap() {
            let bytes = STANDARD
                .decode(case["payload_json_base64"].as_str().unwrap())
                .unwrap();
            let root: BTreeMap<String, &RawValue> = serde_json::from_slice(&bytes).unwrap();
            let tables: BTreeMap<String, Vec<BTreeMap<String, &RawValue>>> =
                serde_json::from_str(root["tables"].get()).unwrap();
            let table = case["table"].as_str().unwrap();
            let column = case["column"].as_str().unwrap();
            let bound =
                sql_value(table, column, "JSON", tables[table][0][column], &cancel).unwrap();
            let expected = String::from_utf8(
                STANDARD
                    .decode(case["sql_json_base64"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(bound, SqlValue::Text(expected.clone()), "{}", case["name"]);
            let connection = Connection::open_in_memory().unwrap();
            connection
                .execute_batch("CREATE TABLE opaque(value JSON NOT NULL)")
                .unwrap();
            connection
                .execute(
                    "INSERT INTO opaque(value) VALUES(?)",
                    rusqlite::params![bound],
                )
                .unwrap();
            // JSON has NUMERIC affinity: SQLite may store scalar 0 as INTEGER.
            // Compare the independent Source storage effect, not a TEXT assumption.
            let (actual_type, actual_quote): (String, String) = connection
                .query_row("SELECT typeof(value),quote(value) FROM opaque", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap();
            assert_eq!(
                actual_type,
                case["sqlite_type"].as_str().unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(
                actual_quote,
                case["sqlite_quote"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
}
