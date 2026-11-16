use dockervault_core::{
    cron::TimeZone, crypto::CredentialCipher, recovery_state::state_manifest, timestamp::Timestamp,
};
use dockervault_service::{
    db::Database,
    ownership::Owner,
    recovery_import::insert_state,
    recovery_state::{ValidationContext, validate_state},
};
use rusqlite::Connection;
use serde_json::{Value, json};

#[tokio::test]
async fn every_inserted_column_matches_the_unchanged_accepted_python_sql_snapshot() {
    let root = std::env::temp_dir().join(format!("dv-import-source-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let owner = Owner::new(1).unwrap();
    let db = Database::open(
        root.join("state.sqlite"),
        root.join("migration-backups"),
        &owner,
    )
    .await
    .unwrap();
    db.close().await;
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/insert-state-source.TEST_ONLY.json")).unwrap();
    let state = validate_state(
        fixture["payload"].clone(),
        ValidationContext {
            backup_root: root.join("backups"),
            now: Timestamp::parse("2024-01-01T00:00:00").unwrap(),
            timezone: TimeZone::utc(),
            rclone_options: Default::default(),
        },
        &owner,
    )
    .await
    .unwrap();
    let mut conn = Connection::open(root.join("state.sqlite")).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    let cipher = CredentialCipher::new(&CredentialCipher::generate_key()).unwrap();
    let tx = conn.transaction().unwrap();
    insert_state(
        &tx,
        &state,
        std::path::Path::new(fixture["configured_backup_root"].as_str().unwrap()),
        &cipher,
        &CancellationToken::new(),
    )
    .unwrap();
    for table in dockervault_core::portable_records::TABLE_ORDER {
        let mut statement = tx
            .prepare(&format!("SELECT * FROM {table} ORDER BY id"))
            .unwrap();
        let names = statement
            .column_names()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut rows = statement.query([]).unwrap();
        let mut actual = vec![];
        while let Some(row) = rows.next().unwrap() {
            let mut object = serde_json::Map::new();
            for (i, name) in names.iter().enumerate() {
                use rusqlite::types::ValueRef;
                let value = match row.get_ref(i).unwrap() {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Real(n) => json!(n),
                    ValueRef::Text(t) => json!(std::str::from_utf8(t).unwrap()),
                    ValueRef::Blob(_) => panic!("unexpected portable blob"),
                };
                object.insert(name.clone(), value);
            }
            let fields = if table == "remote_storages" {
                vec!["password", "s3_access_key", "s3_secret_key"]
            } else if table == "app_settings"
                && matches!(
                    object["key"].as_str(),
                    Some("komodo_api_key" | "komodo_api_secret")
                )
            {
                vec!["value"]
            } else {
                vec![]
            };
            for field in fields {
                if let Some(s) = object[field].as_str() {
                    object[field] = json!(cipher.decrypt_value(s).unwrap());
                }
            }
            actual.push(Value::Object(object));
        }
        assert_eq!(
            json!(actual),
            fixture["plaintext_sql_rows"][table],
            "{table}"
        );
    }
    tx.rollback().unwrap();
    drop(conn);
    owner.shutdown(None).await;
    std::fs::remove_dir_all(root).unwrap();
}

use base64::{Engine, engine::general_purpose::STANDARD};

#[tokio::test]
async fn insertion_keeps_every_independent_python_raw_json_and_sql_affinity_value() {
    let root = std::env::temp_dir().join(format!("dv-import-raw-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let owner = Owner::new(1).unwrap();
    let db = Database::open(
        root.join("state.sqlite"),
        root.join("migration-backups"),
        &owner,
    )
    .await
    .unwrap();
    db.close().await;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../dockervault-core/tests/fixtures/recovery-raw-state.TEST_ONLY.json"
    ))
    .unwrap();
    let mut conn = Connection::open(root.join("state.sqlite")).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    let cipher = CredentialCipher::new(&CredentialCipher::generate_key()).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let state = dockervault_service::recovery_state::validate_state_json(
            STANDARD
                .decode(case["payload_json_base64"].as_str().unwrap())
                .unwrap(),
            ValidationContext {
                backup_root: root.join("backups"),
                now: Timestamp::parse(fixture["now"].as_str().unwrap()).unwrap(),
                timezone: TimeZone::utc(),
                rclone_options: Default::default(),
            },
            &owner,
        )
        .await
        .unwrap();
        let tx = conn.transaction().unwrap();
        insert_state(
            &tx,
            &state,
            &root.join("backups"),
            &cipher,
            &CancellationToken::new(),
        )
        .unwrap();
        let table = case["table"].as_str().unwrap();
        let column = case["column"].as_str().unwrap();
        assert!(
            [
                "backups",
                "backup_targets",
                "backup_schedules",
                "backup_storage_syncs"
            ]
            .contains(&table)
        );
        assert!(column.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
        let actual: (String, String) = tx
            .query_row(
                &format!("SELECT typeof({column}),quote({column}) FROM {table}"),
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            actual.0,
            case["sqlite_type"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            actual.1,
            case["sqlite_quote"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        tx.rollback().unwrap();
    }
    drop(conn);
    owner.shutdown(None).await;
    std::fs::remove_dir_all(root).unwrap();
}
use tokio_util::sync::CancellationToken;
#[tokio::test]
async fn insertion_protects_credentials_rebases_paths_and_never_commits_or_creates_files() {
    let root = std::env::temp_dir().join(format!("dv-import-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let owner = Owner::new(1).unwrap();
    let db = Database::open(
        root.join("state.sqlite"),
        root.join("migration-backups"),
        &owner,
    )
    .await
    .unwrap();
    db.close().await;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../dockervault-core/tests/fixtures/recovery-state.TEST_ONLY.json"
    ))
    .unwrap();
    let mut payload = fixture["cases"][0]["payload"].clone();
    payload["tables"]["remote_storages"][0]["s3_access_key"] = json!("");
    payload["tables"]["app_settings"].as_array_mut().unwrap().push(json!({"id":2,"key":"komodo_api_secret","value":"PUBLIC literal credential","updated_at":null}));
    payload["tables"]["backups"][0]["file_path"] =
        json!("/PUBLIC/exported/backups/nested/./archive.tar.gz.enc");
    payload["tables"]["backups"][0]["encryption_key_path"] =
        json!("/PUBLIC/exported/backups/nested/archive.tar.gz.key");
    payload["tables"]["backups"][0]["encrypted"] = json!(true);
    payload["manifest"] = state_manifest(&payload).unwrap();
    let state = validate_state(
        payload.clone(),
        ValidationContext {
            backup_root: root.join("backups"),
            now: Timestamp::parse(fixture["now"].as_str().unwrap()).unwrap(),
            timezone: TimeZone::utc(),
            rclone_options: serde_json::from_value(fixture["options"].clone()).unwrap(),
        },
        &owner,
    )
    .await
    .unwrap();
    let cipher = CredentialCipher::new(&CredentialCipher::generate_key()).unwrap();
    let mut conn = Connection::open(root.join("state.sqlite")).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn.execute_batch(
        "ALTER TABLE remote_storages ADD COLUMN extension TEXT DEFAULT 'PUBLIC-extension';",
    )
    .unwrap();
    let target = root.join("not-created/../backups");
    let cancel = CancellationToken::new();
    {
        let tx = conn.transaction().unwrap();
        insert_state(&tx, &state, &target, &cipher, &cancel).unwrap();
        assert_eq!(
            tx.query_row("SELECT count(*) FROM backups", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let reader = Connection::open(root.join("state.sqlite")).unwrap();
        assert_eq!(
            reader
                .query_row("SELECT count(*) FROM backups", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        tx.rollback().unwrap();
    }
    assert_eq!(
        conn.query_row("SELECT count(*) FROM retention_policies", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let tx = conn.transaction().unwrap();
    insert_state(&tx, &state, &target, &cipher, &cancel).unwrap();
    tx.commit().unwrap();
    for table in dockervault_core::portable_records::TABLE_ORDER {
        assert_eq!(
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap() as usize,
            payload["tables"][table].as_array().unwrap().len()
        );
    }
    let password: String = conn
        .query_row("SELECT password FROM remote_storages", [], |r| r.get(0))
        .unwrap();
    assert!(password.starts_with("fernet:"));
    assert_eq!(
        cipher.decrypt_value(&password).unwrap(),
        payload["tables"]["remote_storages"][0]["password"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        conn.query_row("SELECT s3_access_key FROM remote_storages", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        ""
    );
    assert_eq!(
        conn.query_row("SELECT s3_secret_key FROM remote_storages", [], |r| r
            .get::<_, Option<String>>(0))
            .unwrap(),
        None
    );
    assert_eq!(
        conn.query_row("SELECT extension FROM remote_storages", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "PUBLIC-extension"
    );
    let secret: String = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key='komodo_api_secret'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(secret.starts_with("fernet:"));
    assert_eq!(
        cipher.decrypt_value(&secret).unwrap(),
        "PUBLIC literal credential"
    );
    assert_eq!(
        conn.query_row(
            "SELECT value FROM app_settings WHERE key='storage_check_quota_before_upload'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "false"
    );
    assert_eq!(
        conn.query_row("SELECT file_path FROM backups", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        format!("{}/nested/archive.tar.gz.enc", target.display())
    );
    assert_eq!(
        conn.query_row("SELECT encryption_key_path FROM backups", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        format!("{}/nested/archive.tar.gz.key", target.display())
    );
    assert_eq!(
        conn.query_row("SELECT status||':'||backup_type FROM backups", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "PENDING:FULL"
    );
    assert!(!root.join("not-created").exists());
    assert!(!root.join("backups").exists());
    let before_password = password.clone();
    conn.execute_batch("CREATE TEMP TRIGGER late_import_fault BEFORE INSERT ON backup_storage_syncs BEGIN SELECT RAISE(ABORT,'PUBLIC diagnostic must not escape'); END;").unwrap();
    let tx = conn.transaction().unwrap();
    for table in dockervault_core::portable_records::TABLE_ORDER
        .into_iter()
        .rev()
    {
        tx.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }
    let error = insert_state(&tx, &state, &target, &cipher, &cancel).unwrap_err();
    assert_eq!(
        error,
        dockervault_service::recovery_records::RecordValidationError::Invalid
    );
    assert!(!error.to_string().contains("PUBLIC"));
    tx.rollback().unwrap();
    assert_eq!(
        conn.query_row("SELECT password FROM remote_storages", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        before_password
    );
    conn.execute_batch("DROP TRIGGER late_import_fault;")
        .unwrap();
    let signal = cancel.clone();
    conn.update_hook(Some(move |action, _db: &str, table: &str, _id| {
        if action == rusqlite::hooks::Action::SQLITE_INSERT && table == "schedules" {
            signal.cancel();
        }
    }))
    .unwrap();
    let tx = conn.transaction().unwrap();
    for table in dockervault_core::portable_records::TABLE_ORDER
        .into_iter()
        .rev()
    {
        tx.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }
    assert_eq!(
        insert_state(&tx, &state, &target, &cipher, &cancel),
        Err(dockervault_service::recovery_records::RecordValidationError::Cancelled)
    );
    assert!(cancel.is_cancelled());
    tx.rollback().unwrap();
    assert_eq!(
        conn.query_row("SELECT password FROM remote_storages", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        before_password
    );
    let tx = conn.transaction().unwrap();
    assert_eq!(
        insert_state(&tx, &state, &target, &cipher, &cancel),
        Err(dockervault_service::recovery_records::RecordValidationError::Cancelled)
    );
    tx.rollback().unwrap();
    drop(conn);
    owner.shutdown(None).await;
    std::fs::remove_dir_all(root).unwrap();
}
