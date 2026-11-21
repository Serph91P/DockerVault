//! Owned, offline relational staging only; no live database, deployment key or
//! filesystem setup. Semantic recovery/manifest/path/cron checks are separate.
use crate::{db::schema, ownership::Owner};
use dockervault_core::{
    lossless_json::PythonJson,
    model::{BackupStatus, BackupType},
    portable_records::{PortableTables, TABLE_ORDER},
    recovery_bundle::MAX_PAYLOAD,
    timestamp::Timestamp,
};
use rusqlite::{
    Connection, params_from_iter,
    types::{Value as SqlValue, ValueRef},
};
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
    let transaction = connection
        .transaction()
        .map_err(|_| RecordValidationError::Invalid)?;
    insert_records(&transaction, records, cancel)?;
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
pub(crate) fn insert_records(
    transaction: &rusqlite::Transaction<'_>,
    records: &PortableTables,
    cancel: &CancellationToken,
) -> Result<(), RecordValidationError> {
    let serialized = serde_json::to_string(records).map_err(|_| RecordValidationError::Invalid)?;
    let tables: BTreeMap<String, Vec<BTreeMap<String, &RawValue>>> =
        serde_json::from_str(&serialized).map_err(|_| RecordValidationError::Invalid)?;
    for table in TABLE_ORDER {
        let Some(first) = tables[table].first() else {
            continue;
        };
        let mut info = transaction
            .prepare(&format!("PRAGMA table_info({})", quote(table)))
            .map_err(|_| RecordValidationError::Invalid)?;
        let mut columns: Vec<(String, String)> = info
            .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
            .map_err(|_| RecordValidationError::Invalid)?
            .collect::<Result<_, _>>()
            .map_err(|_| RecordValidationError::Invalid)?;
        drop(info);
        // Like source Table.insert(values): imported columns only, so retained
        // deployment extension columns keep their own published defaults.
        columns.retain(|(name, _)| first.contains_key(name));
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
    Ok(())
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Read the portable catalog under a caller-owned read transaction.
/// Storage credentials are still in their persisted form; this is not a bundle.
pub fn snapshot_records(
    transaction: &rusqlite::Transaction<'_>,
    cancel: &CancellationToken,
) -> Result<PortableTables, RecordValidationError> {
    use RecordValidationError::{Cancelled, Invalid};
    let settled = || {
        if cancel.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    };
    settled()?;
    let reference = Connection::open_in_memory().map_err(|_| Invalid)?;
    let mut tables = BTreeMap::new();
    let mut remaining = MAX_PAYLOAD - 2;
    for table in TABLE_ORDER {
        settled()?;
        remaining = remaining.checked_sub(table.len() + 6).ok_or(Invalid)?;
        // The published schema, not SELECT *, defines which columns may leave.
        let (_, ddl, _) = schema::TABLES
            .iter()
            .find(|entry| entry.0 == table)
            .ok_or(Invalid)?;
        reference.execute_batch(ddl).map_err(|_| Invalid)?;
        let mut info = reference
            .prepare(&format!("PRAGMA table_info({})", quote(table)))
            .map_err(|_| Invalid)?;
        let columns: Vec<(String, String)> = info
            .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
            .map_err(|_| Invalid)?
            .collect::<Result<_, _>>()
            .map_err(|_| Invalid)?;
        let names = columns
            .iter()
            .map(|(name, _)| quote(name))
            .collect::<Vec<_>>()
            .join(",");
        let filter = if table == "app_settings" {
            "WHERE key IN ('storage_check_quota_before_upload','encryption_onboarding_deferred')"
        } else {
            ""
        };
        let mut statement = transaction
            .prepare(&format!(
                "SELECT {names} FROM {} {filter} ORDER BY id LIMIT 100001",
                quote(table)
            ))
            .map_err(|_| Invalid)?;
        let mut cursor = statement.query([]).map_err(|_| Invalid)?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().map_err(|_| Invalid)? {
            settled()?;
            if rows.len() >= 100000 {
                return Err(Invalid);
            }
            remaining = remaining.checked_sub(3).ok_or(Invalid)?;
            let mut object = BTreeMap::new();
            for (index, (name, kind)) in columns.iter().enumerate() {
                let value = row.get_ref(index).map_err(|_| Invalid)?;
                remaining = remaining.checked_sub(name.len() + 4).ok_or(Invalid)?;
                // Borrow the SQLite cell first; reject before copying a large value.
                if let ValueRef::Text(bytes) = value
                    && bytes.len() > remaining
                {
                    return Err(Invalid);
                }
                let value = portable_value(table, name, kind, value)?;
                remaining = remaining.checked_sub(value.get().len()).ok_or(Invalid)?;
                object.insert(name.clone(), value);
            }
            rows.push(object);
        }
        tables.insert(table, rows);
    }
    settled()?;
    PortableTables::parse_json(&serde_json::to_vec(&tables).map_err(|_| Invalid)?)
        .map_err(|_| Invalid)
}

fn portable_value(
    table: &str,
    name: &str,
    kind: &str,
    value: ValueRef<'_>,
) -> Result<Box<RawValue>, RecordValidationError> {
    use RecordValidationError::Invalid;
    let scalar = match (kind, value) {
        (_, ValueRef::Null) => Value::Null,
        ("JSON", ValueRef::Text(bytes)) => {
            return RawValue::from_string(String::from_utf8(bytes.to_vec()).map_err(|_| Invalid)?)
                .map_err(|_| Invalid);
        }
        ("JSON" | "INTEGER", ValueRef::Integer(number)) => Value::from(number),
        ("JSON", ValueRef::Real(number)) => {
            Value::Number(serde_json::Number::from_f64(number).ok_or(Invalid)?)
        }
        ("BOOLEAN", ValueRef::Integer(number)) => Value::Bool(number != 0),
        (_, ValueRef::Text(bytes)) => {
            let text = std::str::from_utf8(bytes).map_err(|_| Invalid)?;
            match (table, name, kind) {
                ("backups", "status", _) => {
                    serde_json::to_value(BackupStatus::from_db(text).map_err(|_| Invalid)?)
                        .map_err(|_| Invalid)?
                }
                ("backups", "backup_type", _) => {
                    serde_json::to_value(BackupType::from_db(text).map_err(|_| Invalid)?)
                        .map_err(|_| Invalid)?
                }
                (_, _, "DATETIME") => {
                    Value::String(Timestamp::parse(text).map_err(|_| Invalid)?.api())
                }
                _ => Value::String(text.to_owned()),
            }
        }
        _ => return Err(Invalid),
    };
    serde_json::value::to_raw_value(&scalar).map_err(|_| Invalid)
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
    fn snapshot_rejects_oversize_catalog_across_tables() {
        let mut conn = Connection::open_in_memory().unwrap();
        for (_, ddl, _) in schema::TABLES {
            conn.execute_batch(ddl).unwrap();
        }
        conn.execute_batch(
            "INSERT INTO retention_policies (id,name,description)
             VALUES (1,'fixture',hex(zeroblob(6291456)));",
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        assert!(snapshot_records(&tx, &CancellationToken::new()).is_ok());
        tx.rollback().unwrap();
        conn.execute_batch(
            "INSERT INTO app_settings (id,key,value)
             VALUES (1,'storage_check_quota_before_upload',hex(zeroblob(6291456)));",
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        assert!(matches!(
            snapshot_records(&tx, &CancellationToken::new()),
            Err(RecordValidationError::Invalid)
        ));
    }
    #[test]
    fn snapshot_uses_the_portable_schema_and_preserves_opaque_catalog_fields() {
        let mut conn = Connection::open_in_memory().unwrap();
        for (_, ddl, _) in schema::TABLES {
            conn.execute_batch(ddl).unwrap();
        }
        conn.execute_batch(
            "ALTER TABLE backups ADD COLUMN extension TEXT;
             INSERT INTO backup_targets (id,name,target_type) VALUES (1,'fixture','volume');
             INSERT INTO backups (id,target_id,status,backup_type,encrypted,created_at,backup_metadata,extension)
             VALUES (2,1,'COMPLETED','FULL',0,'2024-01-01 00:00:00.000000','{\"opaque\":\"\\ud800\"}','private extension');
             INSERT INTO backups (id,target_id,backup_metadata) VALUES (1,1,'42');
             INSERT INTO app_settings (id,key,value) VALUES
                (1,'komodo_api_secret','retired'),(2,'storage_check_quota_before_upload','true'),(3,'other','private');
             INSERT INTO users (id,username,password_hash) VALUES (1,'private user','private hash');",
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        let snapshot = snapshot_records(&tx, &CancellationToken::new()).unwrap();
        let value = serde_json::to_string(&snapshot).unwrap();
        let tables: BTreeMap<String, Vec<BTreeMap<String, &RawValue>>> =
            serde_json::from_str(&value).unwrap();
        assert_eq!(tables.len(), TABLE_ORDER.len());
        assert_eq!(tables["backups"][0]["id"].get(), "1");
        assert_eq!(tables["backups"][0]["backup_metadata"].get(), "42");
        let row = &tables["backups"][1];
        assert_eq!(row["id"].get(), "2");
        assert_eq!(row["status"].get(), "\"completed\"");
        assert_eq!(row["backup_type"].get(), "\"full\"");
        assert_eq!(row["encrypted"].get(), "false");
        assert_eq!(row["created_at"].get(), "\"2024-01-01T00:00:00\"");
        assert_eq!(row["backup_metadata"].get(), r#"{"opaque":"\ud800"}"#);
        assert!(!row.contains_key("extension"));
        assert_eq!(tables["app_settings"].len(), 1);
        assert_eq!(tables["app_settings"][0]["id"].get(), "2");
        assert!(!tables.contains_key("users"));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            snapshot_records(&tx, &cancel),
            Err(RecordValidationError::Cancelled)
        ));
    }
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
