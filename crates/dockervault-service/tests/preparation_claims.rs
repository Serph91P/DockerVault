#![cfg(target_os = "linux")]
use dockervault_service::recovery_activation::{ClaimKind, PreparationClaims, PreparationError};
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dv-preparation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn separate_contexts_share_the_exact_file_claim_without_truncating_or_unlinking_its_marker() {
    let root = Fixture::new();
    let path = root.0.join("backup.db");
    let a = PreparationClaims::new(Some(path.clone()));
    let b = PreparationClaims::new(Some(path));
    let held = a.try_claim(ClaimKind::Setup).unwrap();
    assert!(matches!(
        b.try_claim(ClaimKind::Setup),
        Err(PreparationError::Busy)
    ));
    let crypto = b.try_claim(ClaimKind::Crypto).unwrap();
    assert!(matches!(
        a.try_claim(ClaimKind::Crypto),
        Err(PreparationError::Busy)
    ));
    let marker = root.0.join("backup.db.setup.lock");
    let inode = std::fs::metadata(&marker).unwrap().ino();
    assert_eq!(
        std::fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::write(&marker, b"unchanged marker").unwrap();
    drop(held);
    let _again = b.try_claim(ClaimKind::Setup).unwrap();
    assert_eq!(std::fs::read(&marker).unwrap(), b"unchanged marker");
    assert_eq!(std::fs::metadata(&marker).unwrap().ino(), inode);
    drop(crypto);
}
#[test]
fn marker_symlinks_and_missing_parents_fail_without_memory_fallback_or_foreign_writes() {
    let root = Fixture::new();
    let foreign = root.0.join("foreign");
    std::fs::write(&foreign, b"do not change").unwrap();
    std::os::unix::fs::symlink(&foreign, root.0.join("backup.db.setup.lock")).unwrap();
    let claims = PreparationClaims::new(Some(root.0.join("backup.db")));
    assert!(matches!(
        claims.try_claim(ClaimKind::Setup),
        Err(PreparationError::Io)
    ));
    assert_eq!(std::fs::read(&foreign).unwrap(), b"do not change");
    let missing = PreparationClaims::new(Some(root.0.join("missing/backup.db")));
    assert!(matches!(
        missing.try_claim(ClaimKind::Setup),
        Err(PreparationError::Io)
    ));
    assert!(!root.0.join("missing").exists());
}
#[tokio::test]
async fn abandoned_preparation_removes_only_its_generation_and_owned_key_then_clears_the_journal() {
    use dockervault_service::{db::Database, ownership::Owner};
    let root = Fixture::new();
    let database = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&database, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let generation = "0123456789abcdef0123456789abcdef";
    let dir = root.0.join(".recovery").join(generation);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rclone.conf"), b"PUBLIC temporary config").unwrap();
    let key = root.0.join(".credential_key");
    let stage = root.0.join(format!(".dv-recovery-key-{generation}"));
    std::fs::write(&stage, b"PUBLIC interrupted key material").unwrap();
    std::fs::hard_link(&stage, &key).unwrap();
    let record=serde_json::json!({"state":"preparing","generation":generation,"instance_key_preparing":true}).to_string();
    sqlx::query("INSERT INTO app_settings(key,value) VALUES('recovery_activation',?)")
        .bind(record)
        .execute(db.pool())
        .await
        .unwrap();
    let claims = PreparationClaims::new(Some(database));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    assert!(guard.discard_incomplete(&key).unwrap());
    assert!(!dir.exists());
    assert!(!key.exists());
    assert!(!stage.exists());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert!(!guard.discard_incomplete(&key).unwrap());
    drop(guard);
    db.close().await;
}

#[tokio::test]
async fn malformed_preparation_is_fenced_and_foreign_key_occupants_are_never_deleted() {
    use dockervault_service::{db::Database, ownership::Owner};
    let root = Fixture::new();
    let database = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&database, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let generation = "abcdef0123456789abcdef0123456789";
    let key = root.0.join(".credential_key");
    let stage = root.0.join(format!(".dv-recovery-key-{generation}"));
    std::fs::write(&stage, b"PUBLIC abandoned staging").unwrap();
    std::fs::write(&key, b"PUBLIC foreign occupant").unwrap();
    let record=serde_json::json!({"state":"preparing","generation":generation,"instance_key_preparing":true}).to_string();
    sqlx::query("INSERT INTO app_settings(key,value) VALUES('recovery_activation',?)")
        .bind(record)
        .execute(db.pool())
        .await
        .unwrap();
    let claims = PreparationClaims::new(Some(database));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    assert!(guard.discard_incomplete(&key).unwrap());
    assert_eq!(std::fs::read(&key).unwrap(), b"PUBLIC foreign occupant");
    assert!(!stage.exists());
    sqlx::query("INSERT INTO app_settings(key,value) VALUES('recovery_activation',?)")
        .bind(r#"{"state":"preparing","generation":"../outside"}"#)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(guard.discard_incomplete(&key).is_err());
    assert_eq!(std::fs::read(&key).unwrap(), b"PUBLIC foreign occupant");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
    drop(guard);
    db.close().await;
}

#[tokio::test]
async fn key_without_staging_requires_both_recorded_identity_and_digest() {
    use dockervault_service::{db::Database, ownership::Owner};
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::MetadataExt;
    for changed in [false, true] {
        let root = Fixture::new();
        let database = root.0.join("backup.db");
        let owner = Owner::new(1).unwrap();
        let db = Database::open(&database, root.0.join("snapshots"), &owner)
            .await
            .unwrap();
        owner.shutdown(None).await;
        let key = root.0.join(".credential_key");
        let original = b"PUBLIC originally owned partial key";
        std::fs::write(&key, original).unwrap();
        let info = std::fs::metadata(&key).unwrap();
        let digest: String = Sha256::digest(original)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let record=serde_json::json!({"state":"preparing","generation":"00000000000000000000000000000000","instance_key_preparing":true,"key_digest":digest,"key_inode":info.ino(),"key_device":info.dev()}).to_string();
        sqlx::query("INSERT INTO app_settings(key,value) VALUES('recovery_activation',?)")
            .bind(record)
            .execute(db.pool())
            .await
            .unwrap();
        if changed {
            std::fs::write(&key, b"PUBLIC changed occupant with same inode").unwrap();
            assert_eq!(std::fs::metadata(&key).unwrap().ino(), info.ino());
        }
        let claims = PreparationClaims::new(Some(database));
        let guard = claims.try_claim(ClaimKind::Setup).unwrap();
        assert!(guard.discard_incomplete(&key).unwrap());
        if changed {
            assert_eq!(
                std::fs::read(&key).unwrap(),
                b"PUBLIC changed occupant with same inode"
            );
        } else {
            assert!(!key.exists());
        }
        drop(guard);
        db.close().await;
    }
}

#[tokio::test]
async fn generation_symlink_never_removes_the_foreign_tree_or_clears_the_pending_fence() {
    use dockervault_service::{db::Database, ownership::Owner};
    let root = Fixture::new();
    let database = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&database, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let generation = "00000000000000000000000000000000";
    let foreign = root.0.join("outside");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("keep"), b"PUBLIC preserved").unwrap();
    std::fs::create_dir(root.0.join(".recovery")).unwrap();
    std::os::unix::fs::symlink(&foreign, root.0.join(".recovery").join(generation)).unwrap();
    sqlx::query("INSERT INTO app_settings(key,value) VALUES('recovery_activation',?)")
        .bind(serde_json::json!({"state":"preparing","generation":generation}).to_string())
        .execute(db.pool())
        .await
        .unwrap();
    let claims = PreparationClaims::new(Some(database));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    assert_eq!(
        guard.discard_incomplete(&root.0.join(".credential_key")),
        Err(PreparationError::UnsafeGeneration)
    );
    assert_eq!(
        std::fs::read(foreign.join("keep")).unwrap(),
        b"PUBLIC preserved"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
    drop(guard);
    db.close().await;
}

#[tokio::test]
async fn a_committed_administrator_keeps_its_generation_key_and_activation_record() {
    use dockervault_service::{auth::AuthService, db::Database, ownership::Owner};
    let root = Fixture::new();
    let database = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&database, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let auth = AuthService::new(db.pool().clone());
    auth.setup("public-admin", "PUBLIC-password!", "PUBLIC-password!")
        .await
        .unwrap();
    let generation = "00000000000000000000000000000000";
    let dir = root.0.join(".recovery").join(generation);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("keep"), b"PUBLIC committed config").unwrap();
    let key = root.0.join(".credential_key");
    std::fs::write(&key, b"PUBLIC committed key").unwrap();
    let record =
        serde_json::json!({"state":"active","generation":generation,"instance_key_preparing":true})
            .to_string();
    sqlx::query("INSERT INTO app_settings(key,value) VALUES('recovery_activation',?)")
        .bind(&record)
        .execute(db.pool())
        .await
        .unwrap();
    let claims = PreparationClaims::new(Some(database));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    assert!(!guard.discard_incomplete(&key).unwrap());
    assert_eq!(
        std::fs::read(dir.join("keep")).unwrap(),
        b"PUBLIC committed config"
    );
    assert_eq!(std::fs::read(&key).unwrap(), b"PUBLIC committed key");
    let retained: String =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(retained, record);
    drop(guard);
    auth.shutdown(None).await;
    db.close().await;
}

#[test]
fn explicit_memory_database_context_clones_share_claims_and_independent_databases_do_not() {
    let a = PreparationClaims::new(None);
    let b = a.clone();
    let independent = PreparationClaims::new(None);
    let held = a.try_claim(ClaimKind::Setup).unwrap();
    assert!(matches!(
        b.try_claim(ClaimKind::Setup),
        Err(PreparationError::Busy)
    ));
    let _other = independent.try_claim(ClaimKind::Setup).unwrap();
    let _crypto = b.try_claim(ClaimKind::Crypto).unwrap();
    drop(held);
    let _again = b.try_claim(ClaimKind::Setup).unwrap();
}

#[tokio::test]
async fn pending_fence_matches_actual_source_sql_null_duplicates_and_opaque_unused_fields() {
    use dockervault_service::{
        db::Database,
        ownership::Owner,
        recovery_activation::{JournalError, recovery_pending, require_recovery_ready},
    };
    let root = Fixture::new();
    let owner = Owner::new(1).unwrap();
    let db = Database::open(root.0.join("backup.db"), root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let source: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/activation-pending.TEST_ONLY.json")).unwrap();
    for case in source["cases"].as_array().unwrap() {
        sqlx::query("DELETE FROM app_settings WHERE key='recovery_activation'")
            .execute(db.pool())
            .await
            .unwrap();
        if !case["missing"].as_bool().unwrap() {
            sqlx::query("INSERT INTO app_settings(key,value) VALUES('recovery_activation',?)")
                .bind(case["value"].as_str())
                .execute(db.pool())
                .await
                .unwrap();
        }
        let pending = case["pending"].as_bool().unwrap();
        assert_eq!(
            recovery_pending(db.pool()).await.unwrap(),
            pending,
            "{}",
            case["name"]
        );
        let ready = require_recovery_ready(db.pool()).await;
        if pending {
            assert!(matches!(ready, Err(JournalError::Pending)));
            assert_eq!(
                ready.err().unwrap().to_string(),
                case["error"].as_str().unwrap()
            );
        } else {
            assert!(ready.is_ok());
        }
    }
    let pool = db.pool().clone();
    db.close().await;
    assert!(matches!(
        recovery_pending(&pool).await,
        Err(JournalError::Io)
    ));
}

#[tokio::test]
async fn begin_journals_before_files_and_restart_remains_fenced_until_owned_discard() {
    use dockervault_service::{
        db::Database,
        ownership::Owner,
        recovery_activation::{JournalError, recovery_pending},
    };
    let root = Fixture::new();
    let path = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&path, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let claims = PreparationClaims::new(Some(path.clone()));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let generation = guard.begin_recovery().unwrap();
    let name = generation.identifier().to_owned();
    assert_eq!(name.len(), 32);
    assert!(
        name.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(name.as_bytes()[12], b'4');
    assert!(b"89ab".contains(&name.as_bytes()[16]));
    assert_eq!(
        generation.directory().unwrap(),
        root.0.join(".recovery").join(&name)
    );
    assert!(!root.0.join(".recovery").exists());
    let value: String =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&value).unwrap(),
        serde_json::json!({"state":"preparing","generation":name})
    );
    assert!(recovery_pending(db.pool()).await.unwrap());
    assert!(matches!(
        guard.begin_recovery(),
        Err(JournalError::InvalidJournal)
    ));
    drop(generation);
    drop(guard);
    db.close().await;
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&path, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    assert!(recovery_pending(db.pool()).await.unwrap());
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    assert!(guard.discard_incomplete(&root.0.join("key")).unwrap());
    assert!(!recovery_pending(db.pool()).await.unwrap());
    let next = guard.begin_recovery().unwrap();
    assert_ne!(name, next.identifier());
    drop(next);
    assert!(guard.discard_incomplete(&root.0.join("key")).unwrap());
    drop(guard);
    db.close().await;
}

#[tokio::test]
async fn journal_begin_refuses_existing_users_wrong_claim_memory_and_missing_database() {
    use dockervault_service::{db::Database, ownership::Owner, recovery_activation::JournalError};
    let root = Fixture::new();
    let path = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&path, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    sqlx::query("INSERT INTO users(username,password_hash,is_admin) VALUES('PUBLIC-existing','PUBLIC-not-a-login-hash',1)").execute(db.pool()).await.unwrap();
    let claims = PreparationClaims::new(Some(path));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    assert!(matches!(
        guard.begin_recovery(),
        Err(JournalError::AlreadySetup)
    ));
    drop(guard);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
    let crypto = claims.try_claim(ClaimKind::Crypto).unwrap();
    assert!(matches!(
        crypto.begin_recovery(),
        Err(JournalError::WrongClaim)
    ));
    drop(crypto);
    let memory = PreparationClaims::new(None);
    let guard = memory.try_claim(ClaimKind::Setup).unwrap();
    assert!(matches!(
        guard.begin_recovery(),
        Err(JournalError::NotDurable)
    ));
    drop(guard);
    let absent = root.0.join("absent.db");
    let missing = PreparationClaims::new(Some(absent.clone()));
    let guard = missing.try_claim(ClaimKind::Setup).unwrap();
    assert!(matches!(guard.begin_recovery(), Err(JournalError::Io)));
    assert!(!absent.exists());
    drop(guard);
    db.close().await;
}

#[tokio::test]
async fn journal_generation_path_refuses_symlink_root_without_creating_foreign_material() {
    use dockervault_service::{db::Database, ownership::Owner, recovery_activation::JournalError};
    let root = Fixture::new();
    let path = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&path, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let foreign = root.0.join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    std::os::unix::fs::symlink(&foreign, root.0.join(".recovery")).unwrap();
    let claims = PreparationClaims::new(Some(path));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let generation = guard.begin_recovery().unwrap();
    assert!(matches!(
        generation.directory(),
        Err(JournalError::UnsafeGeneration)
    ));
    assert_eq!(std::fs::read_dir(&foreign).unwrap().count(), 0);
    drop(generation);
    drop(guard);
    db.close().await;
}

#[tokio::test]
async fn prepared_instance_key_is_private_valid_journal_bound_and_removed_only_while_owned() {
    use dockervault_core::crypto::CredentialCipher;
    use dockervault_service::{
        db::Database, ownership::Owner, recovery_activation::KeyPreparation,
    };
    use sha2::{Digest, Sha256};
    let root = Fixture::new();
    let path = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&path, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let claims = PreparationClaims::new(Some(path));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let generation = guard.begin_recovery().unwrap();
    let key = root.0.join("private/nested/credential_key");
    let cancel = tokio_util::sync::CancellationToken::new();
    assert_eq!(
        generation
            .prepare_instance_key(&key, false, &cancel)
            .unwrap(),
        KeyPreparation::Created
    );
    let metadata = std::fs::symlink_metadata(&key).unwrap();
    assert!(metadata.is_file());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    let bytes = std::fs::read(&key).unwrap();
    assert!(CredentialCipher::new(std::str::from_utf8(&bytes).unwrap()).is_ok());
    assert!(
        !key.parent()
            .unwrap()
            .join(format!(".dv-recovery-key-{}", generation.identifier()))
            .exists()
    );
    let record: String =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let record: serde_json::Value = serde_json::from_str(&record).unwrap();
    assert_eq!(record["instance_key_preparing"], true);
    assert_eq!(record["key_device"], metadata.dev());
    assert_eq!(record["key_inode"], metadata.ino());
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert!(record["key_digest"].as_str() == Some(&digest));
    assert!(!root.0.join(".recovery").exists());
    drop(generation);
    let reused = key.parent().unwrap().join(format!(
        ".dv-recovery-key-{}",
        record["generation"].as_str().unwrap()
    ));
    std::fs::write(&reused, b"PUBLIC unrelated reused stage name").unwrap();
    assert!(matches!(
        guard.discard_incomplete(&key),
        Err(PreparationError::UnsafeKeyPreparation)
    ));
    assert_eq!(
        std::fs::read(&reused).unwrap(),
        b"PUBLIC unrelated reused stage name"
    );
    assert!(key.exists());
    std::fs::remove_file(&reused).unwrap();
    assert!(guard.discard_incomplete(&key).unwrap());
    assert!(!key.exists());
    drop(guard);
    db.close().await;
}

#[tokio::test]
async fn configured_or_existing_keys_are_not_read_replaced_or_claimed_for_rollback() {
    use dockervault_service::{
        db::Database, ownership::Owner, recovery_activation::KeyPreparation,
    };
    let root = Fixture::new();
    let path = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&path, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let claims = PreparationClaims::new(Some(path));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let generation = guard.begin_recovery().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let key = root.0.join("missing/key");
    assert_eq!(
        generation
            .prepare_instance_key(&key, true, &cancel)
            .unwrap(),
        KeyPreparation::Configured
    );
    assert!(!root.0.join("missing").exists());
    let existing = root.0.join("existing");
    std::fs::write(
        &existing,
        b"PUBLIC foreign existing key, deliberately not parsed",
    )
    .unwrap();
    let before = std::fs::metadata(&existing).unwrap();
    assert_eq!(
        generation
            .prepare_instance_key(&existing, false, &cancel)
            .unwrap(),
        KeyPreparation::Existing
    );
    let value: String =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(&value)
            .unwrap()
            .get("instance_key_preparing")
            .is_none()
    );
    drop(generation);
    assert!(guard.discard_incomplete(&existing).unwrap());
    assert_eq!(std::fs::metadata(&existing).unwrap().ino(), before.ino());
    assert_eq!(
        std::fs::read(&existing).unwrap(),
        b"PUBLIC foreign existing key, deliberately not parsed"
    );
    drop(guard);
    db.close().await;
}

#[tokio::test]
async fn instance_key_prepare_rejects_symlinks_foreign_stage_cancel_and_stale_generation_without_effects()
 {
    use dockervault_service::{db::Database, ownership::Owner, recovery_activation::JournalError};
    let root = Fixture::new();
    let path = root.0.join("backup.db");
    let owner = Owner::new(1).unwrap();
    let db = Database::open(&path, root.0.join("snapshots"), &owner)
        .await
        .unwrap();
    owner.shutdown(None).await;
    let claims = PreparationClaims::new(Some(path));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let generation = guard.begin_recovery().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let foreign = root.0.join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("key"), b"PUBLIC untouched").unwrap();
    std::os::unix::fs::symlink(&foreign, root.0.join("alias")).unwrap();
    assert!(matches!(
        generation.prepare_instance_key(&root.0.join("alias/key"), false, &cancel),
        Err(JournalError::UnsafeKeyPreparation)
    ));
    std::os::unix::fs::symlink(foreign.join("key"), root.0.join("key-link")).unwrap();
    assert!(matches!(
        generation.prepare_instance_key(&root.0.join("key-link"), false, &cancel),
        Err(JournalError::UnsafeKeyPreparation)
    ));
    let stage = root
        .0
        .join(format!(".dv-recovery-key-{}", generation.identifier()));
    std::fs::write(&stage, b"PUBLIC unrelated stage occupant").unwrap();
    let key = root.0.join("key");
    assert!(matches!(
        generation.prepare_instance_key(&key, false, &cancel),
        Err(JournalError::UnsafeKeyPreparation)
    ));
    assert!(!key.exists());
    assert_eq!(
        std::fs::read(&stage).unwrap(),
        b"PUBLIC unrelated stage occupant"
    );
    cancel.cancel();
    assert!(matches!(
        generation.prepare_instance_key(&root.0.join("cancelled/new/key"), false, &cancel),
        Err(JournalError::Cancelled)
    ));
    assert!(!root.0.join("cancelled").exists());
    let value: String =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(&value)
            .unwrap()
            .get("instance_key_preparing")
            .is_none()
    );
    let stale =
        serde_json::json!({"state":"preparing","generation":"ffffffffffffffffffffffffffffffff"})
            .to_string();
    sqlx::query("UPDATE app_settings SET value=? WHERE key='recovery_activation'")
        .bind(&stale)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(matches!(
        generation.prepare_instance_key(
            &root.0.join("stale/new/key"),
            false,
            &tokio_util::sync::CancellationToken::new()
        ),
        Err(JournalError::InvalidJournal)
    ));
    assert!(!root.0.join("stale").exists());
    let actual: String =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(actual, stale);
    assert_eq!(
        std::fs::read(foreign.join("key")).unwrap(),
        b"PUBLIC untouched"
    );
    drop(generation);
    drop(guard);
    db.close().await;
}
