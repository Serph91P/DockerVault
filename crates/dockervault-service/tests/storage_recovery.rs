use dockervault_service::{
    db::Database,
    ownership::Owner,
    process::ProcessOwner,
    storage::ssh::SftpEndpoint,
    storage_recovery::{
        DeleteOutcome, SftpDeleteWorker, claim_next, record_settled, settle_interrupted,
    },
};

#[tokio::test]
async fn restart_keeps_uncertain_effects_unknown_and_never_replays_unstarted_items() {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).unwrap();
    let dir = std::env::temp_dir().join(format!("dv-operation-recovery-{nonce:02x?}"));
    std::fs::create_dir(&dir).unwrap();
    let owner = Owner::new(1).unwrap();
    let path = dir.join("data.sqlite");
    let backups = dir.join("backups");
    let db = Database::open(path.clone(), backups.clone(), &owner)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO users(id,username,password_hash) VALUES(1,'fixture','synthetic');
        INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at) VALUES
        ('interrupted',1,'r1','{}','{}','delete','local','','queued','queued',4,2,1,1,0,0,0,'2024-01-01','2024-01-01'),
        ('finished',1,'r2','{}','{}','delete','local','','completed','done',1,1,1,0,0,0,0,'2024-01-01','2024-01-01');
        INSERT INTO storage_operation_items(operation_id,path,state,error) VALUES
        ('interrupted','a','succeeded',NULL),('interrupted','b','pending',NULL),
        ('interrupted','c','pending',NULL),('interrupted','d','failed','retained'),('finished','e','succeeded',NULL);")
        .execute(db.pool()).await.unwrap();
    let identity = serde_json::json!({});
    assert!(
        claim_next(db.pool(), 2, "interrupted", &identity)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        claim_next(
            db.pool(),
            1,
            "interrupted",
            &serde_json::json!({"replaced": true})
        )
        .await
        .is_err()
    );
    let claim = claim_next(db.pool(), 1, "interrupted", &identity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.path, "b");
    assert!(
        claim_next(db.pool(), 1, "interrupted", &identity)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::raw_sql("UPDATE storage_operations SET cancel_requested=1 WHERE id='interrupted';
                   INSERT INTO app_settings(key,value) VALUES ('recovery_activation','incomplete');")
        .execute(db.pool()).await.unwrap();
    db.close().await;
    let db = Database::open(path, backups, &owner).await.unwrap();
    assert_eq!(settle_interrupted(db.pool()).await.unwrap(), 0);
    let state: String =
        sqlx::query_scalar("SELECT state FROM storage_operations WHERE id='interrupted'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(state, "running");
    sqlx::query("DELETE FROM app_settings WHERE key='recovery_activation'")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(settle_interrupted(db.pool()).await.unwrap(), 1);
    assert_eq!(settle_interrupted(db.pool()).await.unwrap(), 0);
    let row: (String,String,i64,i64,i64,i64,i64,bool) = sqlx::query_as("SELECT state,stage,completed,succeeded,failed,skipped,unknown,finished_at IS NOT NULL FROM storage_operations WHERE id='interrupted'").fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        row,
        ("failed".into(), "interrupted".into(), 4, 1, 1, 1, 1, true)
    );
    let items: Vec<(String,String,Option<String>)> = sqlx::query_as("SELECT path,state,error FROM storage_operation_items WHERE operation_id='interrupted' ORDER BY id").fetch_all(db.pool()).await.unwrap();
    assert_eq!(
        items,
        vec![
            ("a".into(), "succeeded".into(), None),
            (
                "b".into(),
                "unknown".into(),
                Some("Interrupted; inspect the actual target before taking further action".into())
            ),
            (
                "c".into(),
                "skipped".into(),
                Some("Not started before interruption".into())
            ),
            ("d".into(), "failed".into(), Some("retained".into()))
        ]
    );
    let untouched: (String, String) =
        sqlx::query_as("SELECT state,stage FROM storage_operations WHERE id='finished'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(untouched, ("completed".into(), "done".into()));
    sqlx::raw_sql("INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at) VALUES
        ('settled',1,'r3','{}','{}','delete','local','','queued','queued',2,0,0,0,0,0,0,'2024-01-01','2024-01-01');
        INSERT INTO storage_operation_items(operation_id,path,state) VALUES ('settled','first','pending'),('settled','next','pending');")
        .execute(db.pool()).await.unwrap();
    let claim = claim_next(db.pool(), 1, "settled", &identity)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE storage_operations SET cancel_requested=1 WHERE id='settled'")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        !record_settled(db.pool(), 2, "settled", claim.id, DeleteOutcome::Unknown)
            .await
            .unwrap()
    );
    assert!(
        record_settled(db.pool(), 1, "settled", claim.id, DeleteOutcome::Unknown)
            .await
            .unwrap()
    );
    assert!(
        !record_settled(db.pool(), 1, "settled", claim.id, DeleteOutcome::Succeeded)
            .await
            .unwrap()
    );
    let row: (String, String, i64, i64, i64) = sqlx::query_as(
        "SELECT state,stage,completed,unknown,skipped FROM storage_operations WHERE id='settled'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(row, ("failed".into(), "finished".into(), 2, 1, 1));
    sqlx::raw_sql("INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at) VALUES
        ('invalid-path',1,'r4','{}','{}','delete','local','','queued','queued',1,0,0,0,0,0,0,'2024-01-01','2024-01-01');
        INSERT INTO storage_operation_items(operation_id,path,state) VALUES ('invalid-path','../outside','pending');")
        .execute(db.pool()).await.unwrap();
    let processes = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let transport = Owner::new(1).unwrap();
    let endpoint = SftpEndpoint {
        host: "127.0.0.1".into(),
        username: "fixture".into(),
        port: 22,
        base: "/backups".into(),
        known_hosts: dir.join("known_hosts"),
        identity: None,
        password: None,
    };
    let worker = SftpDeleteWorker::new(
        db.pool().clone(),
        owner.clone(),
        processes.clone(),
        transport,
    );
    let effect = worker
        .start(endpoint, 1, "invalid-path".into(), identity.clone())
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    assert!(effect);
    let state: String = sqlx::query_scalar(
        "SELECT state FROM storage_operation_items WHERE operation_id='invalid-path'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(processes.active(), 0);
    db.close().await;
    owner.shutdown(None).await;
    std::fs::remove_dir_all(dir).unwrap();
}
