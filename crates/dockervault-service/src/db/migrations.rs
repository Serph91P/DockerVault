use super::{DbError, MigrationReport, schema::TABLES};
use dockervault_core::encoding::{python_float, python_string as ascii_json};
use rusqlite::{
    Connection, OptionalExtension, TransactionBehavior,
    backup::{Backup, StepResult},
    types::ValueRef,
};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

const LAST_VERSION: usize = 7;
static NEXT_BACKUP: AtomicU64 = AtomicU64::new(0);

fn check_cancel(cancel: &CancellationToken) -> Result<(), DbError> {
    if cancel.is_cancelled() {
        Err(DbError::Cancelled)
    } else {
        Ok(())
    }
}
fn observe_cancel(conn: &mut Connection, cancel: &CancellationToken) -> Result<(), DbError> {
    let cancel = cancel.clone();
    // Long SQL statements must cooperate too, not only the outer migration loop.
    conn.progress_handler(1000, Some(move || cancel.is_cancelled()))?;
    Ok(())
}
fn table_exists(conn: &Connection, name: &str) -> Result<bool, DbError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
        [name],
        |row| row.get(0),
    )?)
}
fn create_table(conn: &Connection, name: &str) -> Result<(), DbError> {
    if !table_exists(conn, name)? {
        let (_, ddl, indices) = TABLES
            .iter()
            .find(|(table, _, _)| *table == name)
            .ok_or(DbError::InvalidSchema)?;
        conn.execute_batch(ddl)?;
        for index in *indices {
            conn.execute_batch(index)?;
        }
    }
    Ok(())
}
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn columns(conn: &Connection, table: &str) -> Result<Vec<String>, DbError> {
    Ok(conn
        .prepare(&format!("PRAGMA table_info({})", quoted(table)))?
        .query_map([], |row| row.get(1))?
        .collect::<Result<_, _>>()?)
}
fn add_column(
    conn: &Connection,
    table: &str,
    name: &str,
    declaration: &str,
) -> Result<(), DbError> {
    if !columns(conn, table)?.iter().any(|column| column == name) {
        // Table/column/declaration come only from the published constants below.
        conn.execute_batch(&format!(
            "ALTER TABLE {} ADD COLUMN {} {declaration}",
            quoted(table),
            quoted(name)
        ))?;
    }
    Ok(())
}
fn upgrade_legacy(conn: &Connection) -> Result<(), DbError> {
    for (name, _, _) in TABLES {
        if !matches!(*name, "schema_migrations" | "catalog_quarantine") {
            create_table(conn, name)?;
        }
    }
    for (table, name, declaration) in [
        ("backups", "encrypted", "BOOLEAN DEFAULT 0"),
        ("backups", "encryption_key_path", "VARCHAR(1024)"),
        ("backup_targets", "schedule_id", "INTEGER"),
        ("backup_targets", "retention_policy_id", "INTEGER"),
        ("backup_targets", "selected_volumes", "JSON DEFAULT '[]'"),
        ("backup_targets", "include_paths", "JSON DEFAULT '[]'"),
        ("backup_targets", "exclude_paths", "JSON DEFAULT '[]'"),
        ("backup_targets", "per_volume_rules", "JSON DEFAULT '{}'"),
        ("backup_targets", "sync_to_remote", "BOOLEAN DEFAULT 0"),
        ("backup_targets", "remote_storage_ids", "JSON DEFAULT '[]'"),
        (
            "backup_targets",
            "delete_local_after_sync",
            "BOOLEAN DEFAULT 0",
        ),
        ("retention_policies", "keep_last", "INTEGER DEFAULT 0"),
    ] {
        add_column(conn, table, name, declaration)?;
    }
    conn.execute("UPDATE backup_targets SET sync_to_remote=1 WHERE remote_storage_ids IS NOT NULL AND remote_storage_ids!='[]' AND remote_storage_ids!='' AND sync_to_remote=0", [])?;
    Ok(())
}
fn json_scalar(value: ValueRef<'_>) -> Result<String, DbError> {
    Ok(match value {
        ValueRef::Null => "null".into(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => python_float(value),
        ValueRef::Text(bytes) => {
            ascii_json(std::str::from_utf8(bytes).map_err(|_| DbError::InvalidSchema)?)
        }
        // The published Python migration also rejects non-JSON binary extension values.
        ValueRef::Blob(_) => return Err(DbError::InvalidSchema),
    })
}
fn quarantine(conn: &Connection, cancel: &CancellationToken) -> Result<(), DbError> {
    create_table(conn, "catalog_quarantine")?;
    for (table, query) in [
        (
            "backup_logs",
            "SELECT * FROM backup_logs WHERE backup_id NOT IN (SELECT id FROM backups)",
        ),
        (
            "backup_storage_syncs",
            "SELECT * FROM backup_storage_syncs WHERE backup_id NOT IN (SELECT id FROM backups) OR storage_id NOT IN (SELECT id FROM remote_storages)",
        ),
    ] {
        let mut statement = conn.prepare(query)?;
        let names: Vec<String> = statement.column_names().map(str::to_owned).collect();
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            check_cancel(cancel)?;
            let id: i64 = row.get("id")?;
            let fields = names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    Ok(format!(
                        "{}: {}",
                        ascii_json(name),
                        json_scalar(row.get_ref(index)?)?
                    ))
                })
                .collect::<Result<Vec<_>, DbError>>()?;
            let json = format!("{{{}}}", fields.join(", "));
            conn.execute("INSERT OR IGNORE INTO catalog_quarantine(table_name,row_id,row_json,reason) VALUES(?,?,?,'missing parent reference')", (table, id, json))?;
            conn.execute(&format!("DELETE FROM {table} WHERE id=?"), [id])?;
        }
    }
    validate_foreign_keys(conn)
}
fn ensure_target_foreign_keys(conn: &Connection) -> Result<(), DbError> {
    let existing = conn
        .prepare("PRAGMA foreign_key_list(backup_targets)")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(3)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let missing: Vec<_> = [
        ("schedule_id", "schedules"),
        ("retention_policy_id", "retention_policies"),
    ]
    .into_iter()
    .filter(|(column, parent)| {
        !existing
            .iter()
            .any(|(c, p, id)| c == column && p == parent && id == "id")
    })
    .collect();
    if missing.is_empty() {
        return Ok(());
    }
    for (column, parent) in &missing {
        if conn.query_row(&format!("SELECT EXISTS(SELECT 1 FROM backup_targets WHERE {column} IS NOT NULL AND {column} NOT IN (SELECT id FROM {parent}))"), [], |row| row.get::<_,bool>(0))? { return Err(DbError::ForeignKey); }
    }
    let original: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='backup_targets'",
        [],
        |row| row.get(0),
    )?;
    if !original
        .get(..12)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("CREATE TABLE"))
    {
        return Err(DbError::InvalidSchema);
    }
    let rest = original[12..].trim_start();
    let suffix = [
        "\"backup_targets\"",
        "`backup_targets`",
        "[backup_targets]",
        "backup_targets",
    ]
    .iter()
    .find_map(|name| rest.strip_prefix(name))
    .ok_or(DbError::InvalidSchema)?;
    if !suffix.trim_start().starts_with('(') {
        return Err(DbError::InvalidSchema);
    }
    let mut replacement = format!("CREATE TABLE \"backup_targets__fk_rebuild\"{suffix}");
    let closing = replacement.rfind(')').ok_or(DbError::InvalidSchema)?;
    let constraints = missing
        .iter()
        .map(|(column, parent)| format!("FOREIGN KEY (\"{column}\") REFERENCES \"{parent}\" (id)"))
        .collect::<Vec<_>>()
        .join(", ");
    replacement.insert_str(closing, &format!(", {constraints}"));
    let objects = conn.prepare("SELECT sql FROM sqlite_master WHERE tbl_name='backup_targets' AND type IN ('index','trigger') AND sql IS NOT NULL")?.query_map([], |row| row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    let sequence: Option<i64> = if table_exists(conn, "sqlite_sequence")? {
        conn.query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='backup_targets'",
            [],
            |row| row.get(0),
        )
        .optional()?
    } else {
        None
    };
    let names = columns(conn, "backup_targets")?
        .iter()
        .map(|name| quoted(name))
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute_batch(&replacement)?;
    conn.execute(
        &format!(
            "INSERT INTO backup_targets__fk_rebuild ({names}) SELECT {names} FROM backup_targets"
        ),
        [],
    )?;
    conn.execute_batch("DROP TABLE backup_targets; ALTER TABLE backup_targets__fk_rebuild RENAME TO backup_targets;")?;
    if let Some(sequence) = sequence {
        let changed = conn.execute(
            "UPDATE sqlite_sequence SET seq=MAX(seq,?) WHERE name='backup_targets'",
            [sequence],
        )?;
        if changed == 0 {
            conn.execute(
                "INSERT INTO sqlite_sequence(name,seq) VALUES('backup_targets',?)",
                [sequence],
            )?;
        }
    }
    for object in objects {
        conn.execute_batch(&object)?;
    }
    Ok(())
}
fn validate_schema(conn: &Connection) -> Result<(), DbError> {
    // Let SQLite parse the published declarations rather than maintaining a
    // second hand-written SQL parser or a drifting list of required columns.
    let expected = Connection::open_in_memory()?;
    for (_, ddl, _) in TABLES {
        expected.execute_batch(ddl)?;
    }
    for (table, _, _) in TABLES {
        let actual_columns = columns(conn, table)?;
        if columns(&expected, table)?
            .iter()
            .any(|column| !actual_columns.contains(column))
        {
            return Err(DbError::InvalidSchema);
        }
        let keys = |connection: &Connection| -> Result<Vec<[String; 5]>, DbError> {
            Ok(connection
                .prepare(&format!("PRAGMA foreign_key_list({})", quoted(table)))?
                .query_map([], |row| {
                    Ok([
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ])
                })?
                .collect::<Result<_, _>>()?)
        };
        let actual_keys = keys(conn)?;
        if keys(&expected)?
            .iter()
            .any(|key| !actual_keys.contains(key))
        {
            return Err(DbError::InvalidSchema);
        }
    }
    expected
        .close()
        .map_err(|(_, error)| DbError::from(error))?;
    Ok(())
}

fn validate_foreign_keys(conn: &Connection) -> Result<(), DbError> {
    if conn
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        return Err(DbError::ForeignKey);
    }
    Ok(())
}
fn validate_integrity(conn: &Connection) -> Result<(), DbError> {
    let mut statement = conn.prepare("PRAGMA integrity_check")?;
    let mut rows = statement.query([])?;
    if rows
        .next()?
        .ok_or(DbError::Integrity)?
        .get::<_, String>(0)?
        != "ok"
        || rows.next()?.is_some()
    {
        return Err(DbError::Integrity);
    }
    Ok(())
}

fn consistent_backup(
    path: &Path,
    directory: &Path,
    cancel: &CancellationToken,
) -> Result<PathBuf, DbError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DbError::Io)?
        .as_nanos();
    let destination = directory.join(format!(
        "pre-migration-{stamp}-{}-{}.sqlite",
        std::process::id(),
        NEXT_BACKUP.fetch_add(1, Ordering::Relaxed)
    ));
    let staging = destination.with_extension("sqlite.partial");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)?;
    drop(file);
    let copied = (|| {
        let source = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut target =
            Connection::open_with_flags(&staging, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        observe_cancel(&mut target, cancel)?;
        {
            let backup = Backup::new(&source, &mut target)?;
            let mut busy_since: Option<Instant> = None;
            loop {
                check_cancel(cancel)?;
                match backup.step(64)? {
                    StepResult::Done => break,
                    StepResult::More => {
                        busy_since = None;
                    }
                    StepResult::Busy | StepResult::Locked => {
                        if busy_since.get_or_insert_with(Instant::now).elapsed()
                            >= Duration::from_secs(5)
                        {
                            return Err(DbError::Busy);
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    _ => return Err(DbError::Database),
                }
            }
        }
        validate_integrity(&target)?;
        target.close().map_err(|(_, error)| DbError::from(error))?;
        source.close().map_err(|(_, error)| DbError::from(error))?;
        File::open(&staging)?.sync_all()?;
        check_cancel(cancel)?;
        // Same-directory hard-link promotion is atomic and never overwrites a
        // prior backup, unlike rename() without RENAME_NOREPLACE.
        fs::hard_link(&staging, &destination)?;
        fs::remove_file(&staging)?;
        File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = copied {
        // All SQLite handles are settled before the incomplete owned file is removed.
        match fs::remove_file(&staging) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        return Err(error);
    }
    Ok(destination)
}

pub(super) fn run(
    path: &Path,
    backup_dir: &Path,
    cancel: &CancellationToken,
) -> Result<MigrationReport, DbError> {
    check_cancel(cancel)?;
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    observe_cancel(&mut conn, cancel)?;
    conn.pragma_update(None, "foreign_keys", false)?;
    // Fence all other writers before snapshotting, and hold it through commit.
    // The backup reads through a separate connection, so the source backup API
    // is never called on a connection with an active write transaction.
    let txn = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_cancel(cancel)?;
    let completed: Vec<i64> = if table_exists(&txn, "schema_migrations")? {
        txn.prepare("SELECT version FROM schema_migrations ORDER BY version")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?
    } else {
        Vec::new()
    };
    if completed.len() > LAST_VERSION
        || completed
            .iter()
            .enumerate()
            .any(|(index, version)| *version != (index + 1) as i64)
    {
        return Err(DbError::InvalidLedger);
    }
    validate_integrity(&txn)?;
    let nonempty: bool = txn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%')", [], |row| row.get(0))?;
    let backup = if completed.len() < LAST_VERSION && nonempty {
        Some(consistent_backup(path, backup_dir, cancel)?)
    } else {
        None
    };
    create_table(&txn, "schema_migrations")?;
    for version in completed.len() + 1..=LAST_VERSION {
        check_cancel(cancel)?;
        match version {
            1 => upgrade_legacy(&txn)?,
            2 => quarantine(&txn, cancel)?,
            3 | 5 => create_table(&txn, "workload_operations")?,
            4 => ensure_target_foreign_keys(&txn)?,
            6 => add_column(&txn, "backup_targets", "removed_at", "DATETIME")?,
            7 => {
                create_table(&txn, "storage_operations")?;
                create_table(&txn, "storage_operation_items")?;
            }
            _ => return Err(DbError::InvalidLedger),
        }
        txn.execute(
            "INSERT INTO schema_migrations(version) VALUES(?)",
            [version as i64],
        )?;
    }
    validate_schema(&txn)?;
    validate_foreign_keys(&txn)?;
    validate_integrity(&txn)?;
    check_cancel(cancel)?;
    txn.commit()?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.close().map_err(|(_, error)| DbError::from(error))?;
    Ok(MigrationReport {
        from_version: completed.len(),
        to_version: LAST_VERSION,
        backup,
    })
}
