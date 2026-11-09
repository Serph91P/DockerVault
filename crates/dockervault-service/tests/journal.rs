use dockervault_service::{
    db::Database,
    journal::{Journal, Kind},
    ownership::Owner,
};
use indexmap::IndexMap;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    async fn open() -> (Self, Database, Owner, Journal) {
        let path = std::env::temp_dir().join(format!(
            "dv-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let owner = Owner::new(1).unwrap();
        let db = Database::open(path.join("state.sqlite"), path.join("backups"), &owner)
            .await
            .unwrap();
        let journal = Journal::new(db.pool().clone()).unwrap();
        (Self(path), db, owner, journal)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn originals() -> IndexMap<String, Option<String>> {
    [
        ("web", Some("running")),
        ("db", Some("exited")),
        ("cache", Some("running")),
    ]
    .into_iter()
    .map(|(id, state)| (id.into(), state.map(str::to_owned)))
    .collect()
}

#[tokio::test]
async fn durable_begin_preserves_insertion_order_and_defaults_to_reverse_restart_order() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    let id = journal
        .begin(None, Kind::Backup, originals(), None)
        .await
        .unwrap();
    let row = journal.load(id).await.unwrap();
    assert_eq!(row.state, "active");
    assert_eq!(row.phase, "claimed");
    assert_eq!(
        row.resources.keys().map(String::as_str).collect::<Vec<_>>(),
        ["web", "db", "cache"]
    );
    assert_eq!(row.resources["web"].start_index, Some(2));
    assert_eq!(row.resources["cache"].start_index, Some(0));
    assert!(row.resources["db"].restored);
    assert!(!row.resources["web"].restored);
    let raw: String = sqlx::query_scalar("SELECT resources FROM workload_operations WHERE id=?")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(raw.starts_with(
        "{\"web\": {\"original\": \"running\", \"start_index\": 2, \"requested\": false"
    ));
    journal.finish(Some(id)).await.unwrap();
    assert_eq!(journal.load(id).await.unwrap().state, "finished");
    assert!(journal.unfinished_after(0).await.unwrap().is_empty());
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn unknown_original_state_refuses_even_the_first_journal_effect() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    let mut states = originals();
    states.insert("unknown".into(), None);
    assert!(
        journal
            .begin(None, Kind::Restore, states, None)
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM workload_operations")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    journal.finish(None).await.unwrap();
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn unconfirmed_stop_retains_restart_obligation_until_recovered_and_finished() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    let id = journal
        .begin(
            None,
            Kind::Restore,
            originals(),
            Some(vec!["web".into(), "web".into()]),
        )
        .await
        .unwrap();
    journal.stop(id, "web", false).await.unwrap();
    journal.stop(id, "cache", true).await.unwrap();
    let row = journal.load(id).await.unwrap();
    assert_eq!(row.restart_order(), ["web", "cache"]);
    assert!(!row.resources["web"].stopped);
    journal.finish(Some(id)).await.unwrap();
    let row = journal.load(id).await.unwrap();
    assert_eq!(row.state, "recovery_required");
    assert_eq!(
        row.error.as_deref(),
        Some("Workload recovery incomplete; retry reconciliation before new jobs")
    );
    assert_eq!(journal.unfinished_after(0).await.unwrap().len(), 1);
    journal.recovered(id, vec!["web".into()]).await.unwrap();
    journal.finish(Some(id)).await.unwrap();
    assert_eq!(journal.load(id).await.unwrap().state, "recovery_required");
    journal.recovered(id, vec![]).await.unwrap();
    journal.finish(Some(id)).await.unwrap();
    assert_eq!(journal.load(id).await.unwrap().state, "finished");
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn historical_missing_start_ranks_and_extension_fields_survive_mutation() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    let id = journal
        .begin(None, Kind::Backup, IndexMap::new(), None)
        .await
        .unwrap();
    let legacy = r#"{"z":{"original":"running","requested":true,"stopped":false,"restored":false,"extension":{"empty":[],"enabled":false}},"a":{"original":"running","requested":true,"stopped":true,"restored":false}}"#;
    sqlx::query("UPDATE workload_operations SET resources=? WHERE id=?")
        .bind(legacy)
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(journal.load(id).await.unwrap().restart_order(), ["a", "z"]);
    journal.stop(id, "z", true).await.unwrap();
    let row = journal.load(id).await.unwrap();
    assert_eq!(row.restart_order(), ["a", "z"]);
    assert_eq!(
        row.resources["z"].extra["extension"],
        serde_json::json!({"empty":[],"enabled":false})
    );
    assert_eq!(row.resources["z"].start_index, None);
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn concurrent_resource_updates_do_not_lose_committed_intents() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    let id = journal
        .begin(None, Kind::Backup, originals(), None)
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        journal.stop(id, "web", true),
        journal.stop(id, "cache", false)
    );
    left.unwrap();
    right.unwrap();
    let row = journal.load(id).await.unwrap();
    assert!(row.resources["web"].stopped);
    assert!(row.resources["cache"].requested);
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn dropping_a_db_waiter_does_not_abandon_an_admitted_intent_write() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    let id = journal
        .begin(None, Kind::Backup, originals(), None)
        .await
        .unwrap();
    let locked = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let mut pending = Box::pin(journal.stop(id, "web", true));
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(pending);
    locked.commit().await.unwrap();
    journal.shutdown(None).await;
    assert!(journal.load(id).await.unwrap().resources["web"].stopped);
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn parent_deletion_keeps_the_persisted_workload_obligation() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    sqlx::query("INSERT INTO backup_targets(id,name,target_type) VALUES(1,'PUBLIC','DIRECTORY')")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO backups(id,target_id) VALUES(1,1)")
        .execute(db.pool())
        .await
        .unwrap();
    let id = journal
        .begin(Some(1), Kind::Backup, originals(), None)
        .await
        .unwrap();
    journal.stop(id, "web", false).await.unwrap();
    sqlx::query("DELETE FROM backups WHERE id=1")
        .execute(db.pool())
        .await
        .unwrap();
    let row = journal.load(id).await.unwrap();
    assert_eq!(row.backup_id, None);
    assert!(row.resources["web"].requested);
    assert_eq!(row.restart_order(), ["web"]);
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn recovery_result_fails_a_backup_but_preserves_a_completed_restore_parent() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    sqlx::query("INSERT INTO backup_targets(id,name,target_type) VALUES(1,'PUBLIC','VOLUME')")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,status) VALUES(1,1,'RUNNING'),(2,1,'COMPLETED')")
        .execute(db.pool())
        .await
        .unwrap();
    for (backup, kind) in [(1, Kind::Backup), (2, Kind::Restore)] {
        let id = journal
            .begin(Some(backup), kind, originals(), None)
            .await
            .unwrap();
        journal.stop(id, "web", true).await.unwrap();
        journal
            .record_recovery(id, vec!["web".into()])
            .await
            .unwrap();
        let row = journal.load(id).await.unwrap();
        assert_eq!(row.state, "recovery_required");
        assert!(!row.resources["web"].restored);
        assert_eq!(
            row.error.as_deref(),
            Some("Manual recovery required for workload IDs: web")
        );
        journal.record_recovery(id, vec![]).await.unwrap();
        let row = journal.load(id).await.unwrap();
        assert_eq!(row.state, "finished");
        assert!(row.resources["web"].restored);
    }
    let states: Vec<String> = sqlx::query_scalar("SELECT status FROM backups ORDER BY id")
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert_eq!(states, ["FAILED", "COMPLETED"]);
    let logs: Vec<(String, String)> =
        sqlx::query_as("SELECT level,step FROM backup_logs ORDER BY id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(logs, vec![("ERROR".into(), "recovery".into()); 4]);
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn failed_recovery_log_rolls_back_parent_status_and_obligation_acknowledgment() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    sqlx::query("INSERT INTO backup_targets(id,name,target_type) VALUES(1,'PUBLIC','VOLUME')")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,status) VALUES(1,1,'RUNNING')")
        .execute(db.pool())
        .await
        .unwrap();
    let id = journal
        .begin(Some(1), Kind::Backup, originals(), None)
        .await
        .unwrap();
    journal.stop(id, "web", true).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_recovery_log BEFORE INSERT ON backup_logs BEGIN SELECT RAISE(ABORT,'PUBLIC failure'); END").execute(db.pool()).await.unwrap();
    assert!(journal.record_recovery(id, vec![]).await.is_err());
    let row = journal.load(id).await.unwrap();
    assert_eq!(row.state, "active");
    assert!(!row.resources["web"].restored);
    let status: String = sqlx::query_scalar("SELECT status FROM backups WHERE id=1")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(status, "RUNNING");
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn failed_commit_physically_rolls_back_before_releasing_the_writer() {
    let (fixture, db, owner, journal) = Fixture::open().await;
    let id = journal
        .begin(None, Kind::Backup, originals(), None)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE deferred_probe(parent INTEGER REFERENCES backups(id) DEFERRABLE INITIALLY DEFERRED);
        CREATE TABLE spill_probe(id INTEGER PRIMARY KEY,payload BLOB);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<512) INSERT INTO spill_probe SELECT x,zeroblob(32768) FROM n;
        CREATE TRIGGER fail_commit AFTER UPDATE ON workload_operations BEGIN UPDATE spill_probe SET payload=randomblob(32768); INSERT INTO deferred_probe VALUES(999); END;").execute(db.pool()).await.unwrap();
    assert!(journal.stop(id, "web", true).await.is_err());
    let independent = rusqlite::Connection::open(fixture.0.join("state.sqlite")).unwrap();
    independent.busy_timeout(std::time::Duration::ZERO).unwrap();
    independent
        .execute_batch("BEGIN IMMEDIATE; ROLLBACK")
        .expect("returned mutation must not leave a physical rollback writer");
    assert!(!journal.load(id).await.unwrap().resources["web"].requested);
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
#[tokio::test]
async fn failed_update_rolls_back_and_does_not_poison_the_next_writer() {
    let (_fixture, db, owner, journal) = Fixture::open().await;
    let id = journal
        .begin(None, Kind::Backup, originals(), None)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_journal BEFORE UPDATE ON workload_operations BEGIN SELECT RAISE(ABORT,'PUBLIC test failure'); END").execute(db.pool()).await.unwrap();
    assert!(journal.stop(id, "web", true).await.is_err());
    assert!(!journal.load(id).await.unwrap().resources["web"].requested);
    sqlx::query("DROP TRIGGER reject_journal")
        .execute(db.pool())
        .await
        .unwrap();
    journal.phase(id, "archiving").await.unwrap();
    assert_eq!(journal.load(id).await.unwrap().phase, "archiving");
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
}
