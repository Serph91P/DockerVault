//! Real process-loss probes, not a mock migration callback or production fault flag.
use dockervault_service::{db::Database, ownership::Owner};
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const LEGACY: &str = include_str!("../../../backend/tests/fixtures/legacy_pre_schedule.sql");
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self
            .0
            .try_wait()
            .expect("query the owned migration process")
            .is_none()
        {
            self.0
                .kill()
                .expect("kill the exact owned migration process");
            self.0
                .wait()
                .expect("reap the exact owned migration process");
        }
    }
}
fn spawn(source: &Path, backups: &Path, cooperative: bool) -> OwnedChild {
    OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .env_clear()
            .env("DV_SQLITE_CRASH_CHILD", source)
            .env("DV_SQLITE_BACKUPS", backups)
            .env(
                "DV_SQLITE_COOPERATIVE_CHILD",
                if cooperative { "1" } else { "0" },
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}
fn run_child() {
    let source = std::env::var_os("DV_SQLITE_CRASH_CHILD").unwrap();
    let backups = std::env::var_os("DV_SQLITE_BACKUPS").unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(2)
        .build()
        .unwrap()
        .block_on(async {
            let owner = Owner::new(1).unwrap();
            if std::env::var("DV_SQLITE_COOPERATIVE_CHILD").unwrap() == "1" {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .unwrap();
                let worker_owner = owner.clone();
                let waiter = tokio::spawn(async move {
                    Database::open(PathBuf::from(source), PathBuf::from(backups), &worker_owner)
                        .await
                });
                term.recv().await.unwrap();
                waiter.abort();
                assert!(waiter.await.is_err_and(|error| error.is_cancelled()));
            } else {
                Database::open(PathBuf::from(source), PathBuf::from(backups), &owner)
                    .await
                    .unwrap()
                    .close()
                    .await;
            }
            owner.shutdown(None).await;
        });
}
fn private_snapshot(path: &Path) -> bool {
    let name = path.to_string_lossy();
    // SQLite may keep private rollback/WAL bookkeeping alongside staging data.
    [
        ".partial",
        ".partial-journal",
        ".partial-wal",
        ".partial-shm",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}
fn schema_snapshot(conn: &Connection) -> Vec<(String, String)> {
    conn.prepare("SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn transaction_crash(root: &Path, cooperative: bool) {
    let directory = root.join(if cooperative {
        "cooperative"
    } else {
        "transaction"
    });
    fs::create_dir(&directory).unwrap();
    let source = directory.join("state.sqlite");
    let backups = directory.join("migration-backups");
    let connection = Connection::open(&source).unwrap();
    connection.execute_batch(LEGACY).unwrap();
    connection.execute_batch("CREATE TABLE migration_effect(payload BLOB NOT NULL);
        CREATE TRIGGER synthetic_slow_write AFTER UPDATE ON backup_targets BEGIN INSERT INTO migration_effect VALUES(randomblob(4096)); END;
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000)
        INSERT INTO backup_targets(id,name,target_type,sync_to_remote,remote_storage_ids) SELECT x,'target-'||x,'path',0,'[7]' FROM n;").unwrap();
    if cooperative {
        connection.execute_batch("CREATE TABLE migration_cost(value INTEGER);
            CREATE TRIGGER synthetic_slow_vm AFTER UPDATE ON backup_targets WHEN new.id>2000 BEGIN
                INSERT INTO migration_cost SELECT (WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<5000000) SELECT sum(x) FROM n);
            END;").unwrap();
    }
    let schema = schema_snapshot(&connection);
    connection.close().unwrap();
    let initial_length = fs::metadata(&source).unwrap().len();
    let journal = directory.join("state.sqlite-journal");
    let mut child = spawn(&source, &backups, cooperative);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Real page-cache spill proves that both DDL and row mutations reached
        // the filesystem, while the live rollback journal proves no commit.
        if fs::metadata(&source).unwrap().len() > initial_length + 4 * 1024 * 1024
            && journal.exists()
        {
            break;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "migration committed before observing its physical transaction window"
        );
        assert!(
            Instant::now() < deadline,
            "no physical transaction spill observed"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    if cooperative {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(child.0.id() as i32),
            nix::sys::signal::Signal::SIGTERM,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "cooperative cancellation failed to interrupt and settle the SQLite VM"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    } else {
        child.0.kill().unwrap();
        assert!(!child.0.wait().unwrap().success());
    }
    let completed: Vec<_> = fs::read_dir(&backups)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sqlite")
        })
        .collect();
    assert_eq!(
        completed.len(),
        1,
        "transaction began before one complete rollback snapshot existed"
    );
    let snapshot = Connection::open(&completed[0]).unwrap();
    assert_eq!(
        snapshot
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(schema_snapshot(&snapshot), schema);
    assert_eq!(
        snapshot
            .query_row(
                "SELECT count(*) FROM backup_targets WHERE sync_to_remote=0",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        10000
    );
    snapshot.close().unwrap();
    let check = Connection::open(&source).unwrap();
    assert_eq!(
        check
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        schema_snapshot(&check),
        schema,
        "process-loss recovery retained partial DDL or ledger"
    );
    assert_eq!(
        check
            .query_row(
                "SELECT count(*) FROM backup_targets WHERE sync_to_remote=0",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        10000
    );
    assert_eq!(
        check
            .query_row("SELECT count(*) FROM migration_effect", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    check.close().unwrap();
    if cooperative {
        println!(
            "migration-crash: cooperative SIGTERM -> bounded SQLite VM cancellation and complete rollback: PASS"
        );
        return;
    }
    let mut retry = spawn(&source, &backups, false);
    assert!(retry.0.wait().unwrap().success());
    let check = Connection::open(&source).unwrap();
    assert_eq!(
        check
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        check
            .query_row(
                "SELECT count(*) FROM backup_targets WHERE sync_to_remote=1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        10000
    );
    assert_eq!(
        check
            .query_row("SELECT count(*) FROM migration_effect", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        10000
    );
    check.close().unwrap();
    println!(
        "migration-crash: physical DDL/data spill SIGKILL -> complete rollback -> successful retry: PASS"
    );
}
fn main() {
    if std::env::var_os("DV_SQLITE_CRASH_CHILD").is_some() {
        run_child();
        return;
    }
    let root = std::env::temp_dir().join(format!("dv-migration-crash-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let fixture = Fixture(root);
    let source = fixture.0.join("state.sqlite");
    let backups = fixture.0.join("migration-backups");
    let connection = Connection::open(&source).unwrap();
    connection.execute_batch(LEGACY).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE synthetic_payload(id INTEGER PRIMARY KEY, payload BLOB NOT NULL);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1024)
        INSERT INTO synthetic_payload SELECT x, randomblob(65536) FROM n;",
        )
        .unwrap();
    connection.close().unwrap();
    let original = fs::read(&source).unwrap();
    let mut child = spawn(&source, &backups, false);
    let deadline = Instant::now() + Duration::from_secs(5);
    let partial = loop {
        if let Ok(entries) = fs::read_dir(&backups) {
            for entry in entries {
                let path = entry.unwrap().path();
                let Ok(metadata) = fs::metadata(&path) else {
                    continue;
                };
                let length = metadata.len();
                if length < original.len() as u64 {
                    assert!(
                        private_snapshot(&path),
                        "incomplete SQLite backup is exposed under a completed filename: {:?}",
                        path.file_name()
                    );
                    if length >= 256 * 1024 {
                        break;
                    }
                }
            }
            let found = fs::read_dir(&backups)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.to_string_lossy().ends_with(".partial")
                        && fs::metadata(path).is_ok_and(|meta| {
                            meta.len() >= 256 * 1024 && meta.len() < original.len() as u64
                        })
                });
            if let Some(path) = found {
                break path;
            }
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "worker exited before the real crash window was observed"
        );
        assert!(
            Instant::now() < deadline,
            "no observable incremental-backup crash window"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    assert!(partial.exists());
    assert_eq!(
        fs::read(&source).unwrap(),
        original,
        "process loss during backup modified the source database"
    );
    assert!(
        fs::read_dir(&backups)
            .unwrap()
            .all(|entry| private_snapshot(&entry.unwrap().path()))
    );
    let connection = Connection::open(&source).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='schema_migrations'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    connection.close().unwrap();
    drop(child);
    let mut retry = spawn(&source, &backups, false);
    assert!(retry.0.wait().unwrap().success());
    let connection = Connection::open(&source).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        7
    );
    connection.close().unwrap();
    println!(
        "migration-crash: backup SIGKILL -> unchanged source/integrity -> successful retry: PASS"
    );
    transaction_crash(&fixture.0, false);
    transaction_crash(&fixture.0, true);
}
