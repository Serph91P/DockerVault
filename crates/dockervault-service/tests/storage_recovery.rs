use dockervault_service::{
    db::Database,
    ownership::Owner,
    process::ProcessOwner,
    storage::ssh::SftpEndpoint,
    storage_recovery::{
        DeleteAdmission, DeleteOutcome, SftpDeleteWorker, claim_next, record_settled,
        request_cancel, settle_interrupted, storage_operation_detail, submit_delete,
    },
};

#[tokio::test]
async fn remote_storage_identity_matches_the_python_orm_digest() {
    use dockervault_service::storage_recovery::remote_storage_identity;
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).unwrap();
    let root = std::env::temp_dir().join(format!("dv-remote-identity-{nonce:02x?}"));
    std::fs::create_dir(&root).unwrap();
    let owner = Owner::new(1).unwrap();
    let db = Database::open(root.join("db"), root.join("backups"), &owner)
        .await
        .unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,host,port,username,password,base_path,created_at,updated_at) VALUES(41,?,'ssh',1,'example.invalid',22,'fixture','ciphertext-fixture','./remote','2024-01-01 01:02:03.120','2024-01-01 01:02:03')")
        .bind("Föö 🚀").execute(db.pool()).await.unwrap();
    let matched = remote_storage_identity(db.pool(), 41)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        matched,
        serde_json::json!({"config":"0c9afada890a8ceb2f8d5aac525c98daddd24d0471ca8528f0fd11a412fec486"})
    );
    assert_eq!(remote_storage_identity(db.pool(), 42).await.unwrap(), None);
    sqlx::query("UPDATE remote_storages SET created_at='2024-01-02 01:02:03.120' WHERE id=41")
        .execute(db.pool())
        .await
        .unwrap();
    assert_ne!(
        remote_storage_identity(db.pool(), 41)
            .await
            .unwrap()
            .unwrap(),
        matched
    );
    db.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_worker_settles_exactly_one_claim_and_does_not_replay_after_cancel() {
    use dockervault_service::{storage::local::LocalStorage, storage_recovery::start_local_delete};
    use std::{fs, os::unix::fs::MetadataExt};
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).unwrap();
    let dir = std::env::temp_dir().join(format!("dv-local-delete-claim-{nonce:02x?}"));
    let root = dir.join("remote");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("first"), b"delete").unwrap();
    fs::write(root.join("second"), b"preserve").unwrap();
    let owner = Owner::new(1).unwrap();
    let transport = Owner::new(1).unwrap();
    let db = Database::open(dir.join("db"), dir.join("backups"), &owner)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users(id,username,password_hash) VALUES(1,'fixture','synthetic')")
        .execute(db.pool())
        .await
        .unwrap();
    let meta = fs::metadata(&root).unwrap();
    let identity = serde_json::json!({"root":root,"root_inode":[meta.dev(),meta.ino()]});
    let id = submit_delete(
        db.pool(),
        DeleteAdmission {
            user_id: 1,
            request_id: "12345678-1234-1234-1234-123456789abc",
            storage_id: "local",
            path: "",
            paths: &["first", "second"],
            identity: &identity,
            accepting: true,
        },
    )
    .await
    .unwrap();
    let local = LocalStorage::new(fs::File::open(&root).unwrap(), transport.clone()).unwrap();
    let run = || {
        start_local_delete(
            db.pool(),
            &owner,
            local.clone(),
            1,
            id.clone(),
            identity.clone(),
        )
        .unwrap()
    };
    assert!(run().wait().await.unwrap().unwrap());
    assert!(!root.join("first").exists());
    assert!(request_cancel(db.pool(), 1, &id).await.unwrap());
    assert!(!run().wait().await.unwrap().unwrap());
    assert_eq!(fs::read(root.join("second")).unwrap(), b"preserve");
    let states: Vec<String> = sqlx::query_scalar(
        "SELECT state FROM storage_operation_items WHERE operation_id=? ORDER BY id",
    )
    .bind(&id)
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(states, ["succeeded", "skipped"]);
    assert_eq!((owner.active(), transport.active()), (0, 0));
    db.close().await;
    fs::remove_dir_all(dir).unwrap();
}

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
        ('interrupted',1,'r1','{}','{}','storage_delete','local','','queued','queued',4,2,1,1,0,0,0,'2024-01-01','2024-01-01'),
        ('finished',1,'r2','{}','{}','storage_delete','local','','completed','done',1,1,1,0,0,0,0,'2024-01-01','2024-01-01');
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
        ('settled',1,'r3','{}','{}','storage_delete','local','','queued','queued',2,0,0,0,0,0,0,'2024-01-01','2024-01-01');
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
        ('invalid-path',1,'r4','{}','{}','storage_delete','local','','queued','queued',1,0,0,0,0,0,0,'2024-01-01','2024-01-01');
        INSERT INTO storage_operation_items(operation_id,path,state) VALUES ('invalid-path','/outside','pending');")
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
    let state: (String, Option<String>) = sqlx::query_as(
        "SELECT state,error FROM storage_operation_items WHERE operation_id='invalid-path'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(state, ("failed".into(), Some("Invalid file path".into())));
    assert_eq!(processes.active(), 0);
    let admission = DeleteAdmission {
        user_id: 1,
        request_id: "00112233-4455-6677-8899-aabbccddeeff",
        storage_id: "local",
        path: "./",
        paths: &["a//b", "a/b", "../outside"],
        identity: &identity,
        accepting: true,
    };
    let id = submit_delete(db.pool(), admission).await.unwrap();
    let replay = submit_delete(
        db.pool(),
        DeleteAdmission {
            accepting: false,
            ..admission
        },
    )
    .await
    .unwrap();
    assert_eq!(replay, id);
    assert!(
        submit_delete(
            db.pool(),
            DeleteAdmission {
                paths: &["changed"],
                ..admission
            }
        )
        .await
        .is_err()
    );
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT path FROM storage_operation_items WHERE operation_id=? ORDER BY id",
    )
    .bind(&id)
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(names, ["a/b", "../outside"]);
    assert!(!request_cancel(db.pool(), 2, &id).await.unwrap());
    assert!(request_cancel(db.pool(), 1, &id).await.unwrap());
    let cancelled: (String, i64, i64) =
        sqlx::query_as("SELECT state,cancel_requested,skipped FROM storage_operations WHERE id=?")
            .bind(&id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(cancelled, ("cancelled".into(), 1, 2));
    let detail = storage_operation_detail(db.pool(), 1, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail["state"], "cancelled");
    assert_eq!(detail["cancel_requested"], true);
    assert_eq!(detail["storage_id"], "local");
    assert_eq!(detail["items"][0]["path"], "a/b");
    assert_eq!(detail["items"][0]["state"], "skipped");
    assert!(detail["finished_at"].as_str().unwrap().ends_with('Z'));
    assert!(
        storage_operation_detail(db.pool(), 2, &id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(detail.get("payload").is_none() && detail.get("storage_identity").is_none());
    let payload: String = sqlx::query_scalar("SELECT payload FROM storage_operations WHERE id=?")
        .bind(&id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&payload).unwrap(),
        serde_json::json!({
            "action":"storage_delete","storage_id":"local","path":"","paths":["a/b","../outside"],"parent_id":null
        })
    );
    db.close().await;
    owner.shutdown(None).await;
    std::fs::remove_dir_all(dir).unwrap();
}
