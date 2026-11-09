use dockervault_service::{db::Database, ownership::Owner};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const LEGACY: &str = include_str!("../../../backend/tests/fixtures/legacy_pre_schedule.sql");
const GOLD: &str = include_str!("../../../docs/contracts/python-control-18d8c3d/sqlite-fresh.json");
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-db-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn db(&self) -> PathBuf {
        self.0.join("state.sqlite")
    }
    fn backups(&self) -> PathBuf {
        self.0.join("migration-backups")
    }
    fn legacy(&self) -> Connection {
        let conn = Connection::open(self.db()).unwrap();
        // Seed historical violations as the original pre-enforcement database did.
        // Production connections are separately required to enforce every FK.
        conn.pragma_update(None, "foreign_keys", false).unwrap();
        conn.execute_batch(LEGACY).unwrap();
        conn
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn schema(path: &Path) -> Vec<(String, String)> {
    let conn = Connection::open(path).unwrap();
    conn.prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn versions(conn: &Connection) -> Vec<i64> {
    conn.prepare("SELECT version FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn fresh_schema_and_published_ledger_match_the_independent_python_snapshot() {
    let fixture = Fixture::new();
    let owner = Owner::new(1).unwrap();
    let db = Database::open(fixture.db(), fixture.backups(), &owner)
        .await
        .unwrap();
    let gold: Value = serde_json::from_str(GOLD).unwrap();
    let mut expected: Vec<(String, String)> = gold["schema"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["sql"].is_string())
        .map(|item| {
            (
                item["name"].as_str().unwrap().into(),
                item["sql"].as_str().unwrap().into(),
            )
        })
        .collect();
    expected.sort();
    assert_eq!(schema(&fixture.db()), expected);
    let conn = Connection::open(fixture.db()).unwrap();
    assert_eq!(versions(&conn), vec![1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(rusqlite::version(), "3.54.1");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT sqlite_version()")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "3.54.1"
    );
    assert_eq!(db.migration().unwrap().from_version, 0);
    assert!(db.migration().unwrap().backup.is_none());
    db.close().await;
    owner.shutdown(None).await;
}

#[tokio::test]
async fn legacy_rows_extensions_objects_and_once_only_data_rewrite_survive() {
    let fixture = Fixture::new();
    let conn = fixture.legacy();
    conn.execute_batch("ALTER TABLE backup_targets ADD COLUMN legacy_extension TEXT;
        CREATE INDEX legacy_target_extension ON backup_targets(legacy_extension);
        CREATE TABLE legacy_events (target_id INTEGER);
        CREATE TRIGGER legacy_target_touch AFTER UPDATE ON backup_targets BEGIN INSERT INTO legacy_events VALUES (new.id); END;
        INSERT INTO remote_storages(id,name,storage_type,enabled,password,base_path) VALUES(7,'legacy','local',0,'opaque:legacy-ciphertext','/synthetic/remote');
        INSERT INTO backup_targets(id,name,target_type,enabled,sync_to_remote,remote_storage_ids,legacy_extension) VALUES(41,'legacy target','path',0,0,'[7]','unknown future value');
        INSERT INTO backups(id,target_id,backup_type,status,backup_metadata) VALUES(51,41,'FULL','COMPLETED','{\"unknown\":\"preserve\",\"parent_id\":19}');
        INSERT INTO backup_storage_syncs(id,backup_id,storage_id,remote_path,sync_status) VALUES(61,51,7,'original/archive.tar','completed'),(62,999,7,'orphan/archive.tar','completed');
        INSERT INTO backup_logs(id,backup_id,step,message) VALUES(71,51,'complete','original'),(72,999,'complete','orphan');").unwrap();
    drop(conn);
    let owner = Owner::new(1).unwrap();
    let db = Database::open(fixture.db(), fixture.backups(), &owner)
        .await
        .unwrap();
    let backup = db.migration().unwrap().backup.as_ref().unwrap();
    let before = Connection::open(backup).unwrap();
    assert_eq!(
        before
            .query_row(
                "SELECT sync_to_remote FROM backup_targets WHERE id=41",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let conn = Connection::open(fixture.db()).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT sync_to_remote,legacy_extension FROM backup_targets WHERE id=41",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        )
        .unwrap(),
        (1, "unknown future value".into())
    );
    assert_eq!(
        conn.query_row("SELECT password FROM remote_storages WHERE id=7", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "opaque:legacy-ciphertext"
    );
    assert_eq!(
        conn.query_row("SELECT backup_metadata FROM backups WHERE id=51", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "{\"unknown\":\"preserve\",\"parent_id\":19}"
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM catalog_quarantine", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    conn.execute("UPDATE backup_targets SET sync_to_remote=0 WHERE id=41", [])
        .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM legacy_events WHERE target_id=41",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    drop(conn);
    let snapshot = schema(&fixture.db());
    db.close().await;
    let second = Database::open(fixture.db(), fixture.backups(), &owner)
        .await
        .unwrap();
    assert_eq!(schema(&fixture.db()), snapshot);
    assert!(second.migration().unwrap().backup.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sync_to_remote FROM backup_targets WHERE id=41")
            .fetch_one(second.pool())
            .await
            .unwrap(),
        0
    );
    second.close().await;
    owner.shutdown(None).await;
}

#[tokio::test]
async fn unsupported_and_gapped_ledgers_refuse_startup_without_partial_ddl() {
    for ledger in [vec![2], vec![1, 3], vec![1, 2, 3, 4, 5, 6, 7, 8]] {
        let fixture = Fixture::new();
        let conn = fixture.legacy();
        conn.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)").unwrap();
        for version in &ledger {
            conn.execute(
                "INSERT INTO schema_migrations(version) VALUES(?)",
                [version],
            )
            .unwrap();
        }
        drop(conn);
        let before = schema(&fixture.db());
        let owner = Owner::new(1).unwrap();
        assert!(
            Database::open(fixture.db(), fixture.backups(), &owner)
                .await
                .is_err()
        );
        assert_eq!(schema(&fixture.db()), before);
        assert_eq!(versions(&Connection::open(fixture.db()).unwrap()), ledger);
        owner.shutdown(None).await;
    }
}

#[tokio::test]
async fn invalid_parent_rolls_back_ddl_data_quarantine_and_ledger_then_allows_retry() {
    let fixture = Fixture::new();
    let conn = fixture.legacy();
    conn.execute_batch("ALTER TABLE backup_targets ADD COLUMN schedule_id INTEGER;
        INSERT INTO backup_targets(id,name,target_type,schedule_id,sync_to_remote,remote_storage_ids) VALUES(41,'invalid','path',999,0,'[7]');").unwrap();
    drop(conn);
    let before = schema(&fixture.db());
    let owner = Owner::new(1).unwrap();
    assert!(
        Database::open(fixture.db(), fixture.backups(), &owner)
            .await
            .is_err()
    );
    assert_eq!(schema(&fixture.db()), before);
    let conn = Connection::open(fixture.db()).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT sync_to_remote FROM backup_targets WHERE id=41",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute("UPDATE backup_targets SET schedule_id=NULL WHERE id=41", [])
        .unwrap();
    drop(conn);
    let db = Database::open(fixture.db(), fixture.backups(), &owner)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sync_to_remote FROM backup_targets WHERE id=41")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    db.close().await;
    owner.shutdown(None).await;
}

#[tokio::test]
async fn every_pool_connection_enforces_foreign_keys_and_size_is_bounded() {
    let fixture = Fixture::new();
    let owner = Owner::new(1).unwrap();
    let db = Database::open(fixture.db(), fixture.backups(), &owner)
        .await
        .unwrap();
    let mut held = Vec::new();
    for _ in 0..4 {
        let mut connection = db.pool().acquire().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&mut *connection)
                .await
                .unwrap(),
            1
        );
        assert!(sqlx::query("INSERT INTO sessions(user_id,token,expires_at) VALUES(999,'synthetic','2099-01-01 00:00:00.000000')").execute(&mut *connection).await.is_err());
        held.push(connection);
    }
    assert!(db.pool().try_acquire().is_none());
    drop(held);
    db.close().await;
    owner.shutdown(None).await;
}

#[tokio::test]
async fn readonly_open_never_migrates_or_creates_a_database() {
    let fixture = Fixture::new();
    drop(fixture.legacy());
    let before = schema(&fixture.db());
    let db = Database::read_only(fixture.db()).await.unwrap();
    assert!(db.migration().is_none());
    assert!(
        sqlx::query("CREATE TABLE forbidden (id INTEGER)")
            .execute(db.pool())
            .await
            .is_err()
    );
    db.close().await;
    assert_eq!(schema(&fixture.db()), before);
    let missing = fixture.0.join("must-not-exist.sqlite");
    assert!(Database::read_only(&missing).await.is_err());
    assert!(!missing.exists());
}

#[tokio::test]
async fn migration_backup_includes_committed_wal_bytes_and_can_be_restored_independently() {
    let fixture = Fixture::new();
    let writer = fixture.legacy();
    writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        INSERT INTO users(id,username,password_hash) VALUES(77,'wal-user','public-synthetic-hash');").unwrap();
    assert!(
        std::fs::metadata(fixture.db().with_file_name("state.sqlite-wal"))
            .unwrap()
            .len()
            > 0
    );
    let owner = Owner::new(1).unwrap();
    let db = Database::open(fixture.db(), fixture.backups(), &owner)
        .await
        .unwrap();
    let backup = db.migration().unwrap().backup.as_ref().unwrap();
    let restored = fixture.0.join("restored.sqlite");
    std::fs::copy(backup, &restored).unwrap();
    let check = Connection::open(restored).unwrap();
    assert_eq!(
        check
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        check
            .query_row("SELECT username FROM users WHERE id=77", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "wal-user"
    );
    assert_eq!(
        check
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='schema_migrations'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    db.close().await;
    drop(writer);
    owner.shutdown(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_migration_waiter_cancels_backup_and_joins_rollback_before_release() {
    let fixture = Fixture::new();
    let conn = fixture.legacy();
    conn.execute_batch(
        "CREATE TABLE synthetic_payload(id INTEGER PRIMARY KEY,payload BLOB NOT NULL);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1024)
        INSERT INTO synthetic_payload SELECT x,randomblob(65536) FROM n;",
    )
    .unwrap();
    conn.close().unwrap();
    let original = std::fs::read(fixture.db()).unwrap();
    let owner = Owner::new(1).unwrap();
    let worker_owner = owner.clone();
    let path = fixture.db();
    let backups = fixture.backups();
    let waiter = tokio::spawn(async move { Database::open(path, backups, &worker_owner).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let copying = std::fs::read_dir(fixture.backups()).is_ok_and(|entries| {
                entries.filter_map(Result::ok).any(|entry| {
                    entry.path().extension().is_some_and(|ext| ext == "partial")
                        && entry.metadata().is_ok_and(|metadata| {
                            metadata.len() >= 256 * 1024 && metadata.len() < original.len() as u64
                        })
                })
            });
            if copying {
                break;
            }
            assert!(
                !waiter.is_finished(),
                "migration finished before its cancellation window was observed"
            );
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    waiter.abort();
    assert!(waiter.await.is_err_and(|error| error.is_cancelled()));
    tokio::time::timeout(std::time::Duration::from_secs(5), owner.shutdown(None))
        .await
        .unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(std::fs::read(fixture.db()).unwrap(), original);
    assert_eq!(
        std::fs::read_dir(fixture.backups()).unwrap().count(),
        0,
        "owned partial files survived cooperative settlement"
    );
    let conn = Connection::open(fixture.db()).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='schema_migrations'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn a_complete_ledger_with_missing_columns_or_constraints_is_not_readiness() {
    let fixture = Fixture::new();
    let conn = fixture.legacy();
    conn.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
        INSERT INTO schema_migrations(version) VALUES(1),(2),(3),(4),(5),(6),(7);").unwrap();
    drop(conn);
    let before = schema(&fixture.db());
    let owner = Owner::new(1).unwrap();
    assert!(
        Database::open(fixture.db(), fixture.backups(), &owner)
            .await
            .is_err()
    );
    assert_eq!(schema(&fixture.db()), before);
    assert!(!fixture.backups().exists());
    owner.shutdown(None).await;
}

#[tokio::test]
async fn a_failed_pre_migration_backup_cannot_commit_a_data_or_schema_change() {
    let fixture = Fixture::new();
    let conn = fixture.legacy();
    conn.execute_batch("INSERT INTO backup_targets(id,name,target_type,sync_to_remote,remote_storage_ids) VALUES(41,'target','path',0,'[7]');").unwrap();
    drop(conn);
    std::fs::write(fixture.backups(), b"existing unrelated file").unwrap();
    let before = schema(&fixture.db());
    let owner = Owner::new(1).unwrap();
    assert!(
        Database::open(fixture.db(), fixture.backups(), &owner)
            .await
            .is_err()
    );
    assert_eq!(schema(&fixture.db()), before);
    let conn = Connection::open(fixture.db()).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT sync_to_remote FROM backup_targets WHERE id=41",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        std::fs::read(fixture.backups()).unwrap(),
        b"existing unrelated file"
    );
    owner.shutdown(None).await;
}

#[tokio::test]
async fn target_rebuild_retains_the_autoincrement_high_water_mark_not_just_live_ids() {
    let fixture = Fixture::new();
    // This is the supported legacy declaration with the AUTOINCREMENT extension.
    let ddl = LEGACY.replace(
        "CREATE TABLE backup_targets (\n\tid INTEGER NOT NULL,",
        "CREATE TABLE backup_targets (\n\tid INTEGER PRIMARY KEY AUTOINCREMENT,",
    );
    let start = ddl.find("CREATE TABLE backup_targets").unwrap();
    let end = start + ddl[start..].find("CREATE TABLE sessions").unwrap();
    let target = ddl[start..end].replace(",\n\tPRIMARY KEY (id)", "");
    let ddl = format!("{}{}{}", &ddl[..start], target, &ddl[end..]);
    let conn = Connection::open(fixture.db()).unwrap();
    conn.execute_batch(&ddl).unwrap();
    conn.execute_batch("INSERT INTO backup_targets(id,name,target_type) VALUES(41,'retained','path'),(900,'removed','path'); DELETE FROM backup_targets WHERE id=900;").unwrap();
    drop(conn);
    let owner = Owner::new(1).unwrap();
    let db = Database::open(fixture.db(), fixture.backups(), &owner)
        .await
        .unwrap();
    let conn = Connection::open(fixture.db()).unwrap();
    conn.execute(
        "INSERT INTO backup_targets(name,target_type) VALUES('new','path')",
        [],
    )
    .unwrap();
    assert_eq!(conn.last_insert_rowid(), 901);
    assert_eq!(
        conn.query_row("SELECT name FROM backup_targets WHERE id=41", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "retained"
    );
    db.close().await;
    owner.shutdown(None).await;
}

#[tokio::test]
async fn already_upgraded_tables_accept_each_published_ledger_prefix_without_rewriting_user_choices()
 {
    for prefix in 1..=7 {
        let fixture = Fixture::new();
        let owner = Owner::new(1).unwrap();
        Database::open(fixture.db(), fixture.backups(), &owner)
            .await
            .unwrap()
            .close()
            .await;
        let conn = Connection::open(fixture.db()).unwrap();
        conn.execute("INSERT INTO backup_targets(id,name,target_type,sync_to_remote,remote_storage_ids) VALUES(41,'target','path',0,'[7]')", []).unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version>?", [prefix])
            .unwrap();
        drop(conn);
        let db = Database::open(fixture.db(), fixture.backups(), &owner)
            .await
            .unwrap();
        assert_eq!(db.migration().unwrap().from_version, prefix as usize);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT sync_to_remote FROM backup_targets WHERE id=41")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            versions(&Connection::open(fixture.db()).unwrap()),
            vec![1, 2, 3, 4, 5, 6, 7]
        );
        db.close().await;
        owner.shutdown(None).await;
    }
}
