use dockervault_service::{
    db::Database,
    ownership::Owner,
    storage_recovery::{claim_next, settle_interrupted},
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
    db.close().await;
    owner.shutdown(None).await;
    std::fs::remove_dir_all(dir).unwrap();
}
