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
