//! Admission check for the rehearsal-only rollback image, run under its writer fence.

use rusqlite::{Connection, OpenFlags};
use std::{env, os::unix::process::CommandExt, path::Path, process::Command};

fn check() -> Result<(), &'static str> {
    // The pinned legacy image resolves this URL with /app as its working directory.
    if env::var("DATABASE_URL").as_deref() != Ok("sqlite+aiosqlite:///./data/backup.db") {
        return Err("expected the default SQLite URL on the fenced data volume");
    }
    let path = Path::new("/app/data/backup.db");
    if !path.symlink_metadata().is_ok_and(|m| m.is_file()) {
        return Err("expected an existing regular database on the fenced data volume");
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "cannot open the fenced database read-only")?;
    db.execute_batch("PRAGMA trusted_schema=OFF; BEGIN")
        .map_err(|_| "cannot inspect the fenced database")?;
    let mut ledger = db
        .prepare("SELECT version FROM schema_migrations ORDER BY version LIMIT 8")
        .map_err(|_| "cannot read the migration ledger")?;
    let versions = ledger
        .query_map([], |row| row.get::<_, i64>(0))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|_| "cannot read the migration ledger")?;
    if versions != [1, 2, 3, 4, 5, 6, 7] {
        return Err("incompatible migration ledger");
    }
    for (query, reason) in [
        (
            "SELECT EXISTS(SELECT 1 FROM backups WHERE status IN ('PENDING','RUNNING') OR
               CASE WHEN backup_metadata IS NULL THEN 0 WHEN NOT json_valid(backup_metadata) THEN 1
               ELSE json_type(backup_metadata,'$.native_ephemeral_archive') IS NOT NULL
                 OR json_type(backup_metadata,'$.native_publication') IS NOT NULL
                 OR json_type(backup_metadata,'$.native_local_sync') IS NOT NULL
                 OR json_type(backup_metadata,'$.native_copy_removal') IS NOT NULL END)",
            "backup work or archive publication is unsettled",
        ),
        (
            "SELECT EXISTS(SELECT 1 FROM workload_operations WHERE state IS NOT 'finished')",
            "workload restart or recovery is unsettled",
        ),
        (
            "SELECT EXISTS(SELECT 1 FROM storage_operations WHERE state IN ('queued','running') OR unknown != 0)
                 OR EXISTS(SELECT 1 FROM storage_operation_items WHERE state IN ('pending','running','unknown'))",
            "storage work is unsettled or has unknown effects",
        ),
        (
            "SELECT EXISTS(SELECT 1 FROM backup_storage_syncs WHERE sync_status IS NULL OR sync_status NOT IN ('failed','completed'))",
            "remote sync is unsettled",
        ),
        (
            "SELECT EXISTS(SELECT 1 FROM app_settings WHERE key='native_ssh_key_change')",
            "SSH key publication is unsettled",
        ),
    ] {
        if db
            .query_row(query, [], |row| row.get::<_, bool>(0))
            .map_err(|_| "cannot inspect all required journals")?
        {
            return Err(reason);
        }
    }
    Ok(())
}

fn main() {
    if let Err(reason) = check() {
        eprintln!(
            "Python rollback refused: {reason}. Keep the current data and reconcile with Rust before retrying."
        );
        std::process::exit(74);
    }
    // exec keeps the inherited flock descriptor alive through legacy init.
    let error = Command::new("/entrypoint.sh").exec();
    eprintln!("Python rollback could not start legacy init: {error}");
    std::process::exit(74);
}
