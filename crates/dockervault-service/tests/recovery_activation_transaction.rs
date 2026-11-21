use dockervault_service::{
    db::Database,
    ownership::Owner,
    recovery_activation::{ClaimKind, JournalError, PreparationClaims, recovery_pending},
};

#[tokio::test]
async fn late_result_failure_rolls_back_staged_user_and_retains_the_preparation_marker() {
    let root =
        std::env::temp_dir().join(format!("dv-activation-transaction-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&path, root.join("snapshots"), &owner)
        .await
        .unwrap();
    let observer = rusqlite::Connection::open(&path).unwrap();
    observer.execute_batch("CREATE TRIGGER synthetic_late_result BEFORE INSERT ON app_settings WHEN NEW.key='recovery_result' AND (SELECT count(*) FROM users)=1 AND (SELECT count(*) FROM sessions)=1 BEGIN SELECT RAISE(ABORT, 'synthetic late failure'); END").unwrap();
    let claims = PreparationClaims::new(Some(path));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let generation = guard.begin_recovery().unwrap();
    let write = |tx: &rusqlite::Transaction<'_>| {
        tx.execute("INSERT INTO users(username,password_hash,is_admin) VALUES('PUBLIC-fixture','PUBLIC-not-a-login-hash',1)", []).map_err(|_| JournalError::Io)?;
        tx.execute("INSERT INTO sessions(user_id,token,created_at,expires_at) VALUES(?,'PUBLIC-synthetic-token','2026-11-20T00:00:00','2026-11-21T00:00:00')", [tx.last_insert_rowid()]).map_err(|_| JournalError::Io)?;
        tx.execute(
            "INSERT INTO app_settings(key,value) VALUES('recovery_result','{}')",
            [],
        )
        .map_err(|_| JournalError::Io)?;
        Ok(())
    };
    assert_eq!(generation.commit_activation(write), Err(JournalError::Io));
    let count = |query: &str| {
        observer
            .query_row(query, [], |row| row.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(count("SELECT count(*) FROM users"), 0);
    assert_eq!(count("SELECT count(*) FROM sessions"), 0);
    assert_eq!(
        count("SELECT count(*) FROM app_settings WHERE key='recovery_result'"),
        0
    );
    assert!(recovery_pending(db.pool()).await.unwrap());
    observer
        .execute_batch("DROP TRIGGER synthetic_late_result")
        .unwrap();
    generation.commit_activation(write).unwrap();
    assert_eq!(count("SELECT count(*) FROM users"), 1);
    assert_eq!(count("SELECT count(*) FROM sessions"), 1);
    assert_eq!(
        count("SELECT count(*) FROM app_settings WHERE key='recovery_result'"),
        1
    );
    assert!(!recovery_pending(db.pool()).await.unwrap());
    assert_eq!(
        generation.commit_activation(write),
        Err(JournalError::AlreadySetup)
    );
    db.close().await;
    owner.shutdown(None).await;
    drop(observer);
    std::fs::remove_dir_all(root).unwrap();
}
