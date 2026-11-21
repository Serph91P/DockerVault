use dockervault_service::{
    auth::{AuthError, AuthService},
    db::Database,
    ownership::Owner,
};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-auth-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    async fn open(&self) -> Database {
        let owner = Owner::new(1).unwrap();
        let db = Database::open(
            self.0.join("auth.sqlite"),
            self.0.join("migration-backups"),
            &owner,
        )
        .await
        .unwrap();
        owner.shutdown(None).await;
        db
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[tokio::test]
async fn setup_refuses_a_shared_live_preparation_claim_without_creating_an_admin_or_session() {
    use dockervault_service::recovery_activation::{ClaimKind, PreparationClaims};
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    let claims = PreparationClaims::new(Some(fixture.0.join("auth.sqlite")));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let result = auth
        .setup(
            "public-admin",
            "PUBLIC-test-password!",
            "PUBLIC-test-password!",
        )
        .await;
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(db.pool())
        .await
        .unwrap();
    drop(guard);
    auth.shutdown(None).await;
    db.close().await;
    assert_eq!(
        result.err(),
        Some(AuthError::Preparation(
            dockervault_service::recovery_activation::PreparationError::Busy
        ))
    );
    assert_eq!((users, sessions), (0, 0));
}

#[tokio::test]
async fn setup_atomically_creates_one_admin_and_only_a_hashed_seven_day_session() {
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    assert!(!auth.setup_complete().await.unwrap());
    let session = auth
        .setup(
            "public-admin",
            "PUBLIC-test-password!",
            "PUBLIC-test-password!",
        )
        .await
        .unwrap();
    assert!(auth.setup_complete().await.unwrap());
    assert!(session.user.is_admin);
    assert!(session.user.last_login.is_none());
    assert!(!session.user.created_at.ends_with("+00:00"));
    let stored: String = sqlx::query_scalar("SELECT token FROM sessions")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(stored, session.token.hash().as_str());
    assert_ne!(stored, session.token.expose());
    assert_eq!(session.token.expose().len(), 86);
    assert!(!format!("{:?}", session.token).contains(session.token.expose()));
    let lifetime: f64 = sqlx::query_scalar(
        "SELECT (julianday(expires_at)-julianday(created_at))*86400 FROM sessions",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!((lifetime - 604800.0).abs() < 0.01);
    assert_eq!(
        auth.current(&session.token.hash())
            .await
            .unwrap()
            .unwrap()
            .id,
        session.user.id
    );
    assert!(matches!(
        auth.setup(
            "another-admin",
            "PUBLIC-other-password!",
            "PUBLIC-other-password!"
        )
        .await,
        Err(AuthError::AlreadySetup)
    ));
    let cost: String = sqlx::query_scalar("SELECT password_hash FROM users")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(cost.starts_with("$2b$12$"));
    auth.shutdown(None).await;
    db.close().await;
}
#[tokio::test]
async fn failed_session_insert_rolls_back_the_admin_and_allows_a_real_retry() {
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    sqlx::raw_sql("CREATE TRIGGER synthetic_session_failure BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT,'PUBLIC synthetic failure'); END;").execute(db.pool()).await.unwrap();
    assert!(
        auth.setup(
            "public-admin",
            "PUBLIC-test-password!",
            "PUBLIC-test-password!"
        )
        .await
        .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    sqlx::raw_sql("DROP TRIGGER synthetic_session_failure")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        auth.setup(
            "public-admin",
            "PUBLIC-test-password!",
            "PUBLIC-test-password!"
        )
        .await
        .is_ok()
    );
    auth.shutdown(None).await;
    db.close().await;
}
#[tokio::test]
async fn concurrent_setup_claims_have_one_winner_and_no_extra_session() {
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    let (a, b) = tokio::join!(
        auth.setup("public-admin-a", "PUBLIC-password-a!", "PUBLIC-password-a!"),
        auth.setup("public-admin-b", "PUBLIC-password-b!", "PUBLIC-password-b!")
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    // Accepted fc1b339 returns the busy-claim error before the first admin commits;
    // an attempt after commit still returns AlreadySetup. Neither may create a session.
    let loser = a.as_ref().err().or_else(|| b.as_ref().err()).unwrap();
    assert!(matches!(
        loser,
        AuthError::AlreadySetup
            | AuthError::Preparation(
                dockervault_service::recovery_activation::PreparationError::Busy
            )
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    auth.shutdown(None).await;
    db.close().await;
}
#[tokio::test]
async fn existing_python_bcrypt_vectors_accept_exact_utf8_byte_prefixes_and_nul() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../docs/contracts/python-control-18d8c3d/bcrypt-vectors.json"
    ))
    .unwrap();
    for vector in vectors["vectors"].as_array().unwrap() {
        let fixture = Fixture::new();
        let db = fixture.open().await;
        let auth = AuthService::new(db.pool().clone());
        sqlx::query("INSERT INTO users(id,username,password_hash,is_admin,created_at,updated_at) VALUES(1,'public-admin',?,1,'2024-01-02 03:04:05.123456','2024-01-02 03:04:05.123456')").bind(vector["hash"].as_str().unwrap()).execute(db.pool()).await.unwrap();
        for key in ["password", "same_prefix_password"] {
            let login = auth
                .login("public-admin", vector[key].as_str().unwrap())
                .await
                .unwrap();
            assert_eq!(login.user.id, 1);
            assert!(login.user.last_login.unwrap().ends_with("+00:00"));
        }
        assert!(matches!(
            auth.login("public-admin", "PUBLIC-wrong-password").await,
            Err(AuthError::InvalidCredentials)
        ));
        assert!(matches!(
            auth.login("unknown-public-user", "PUBLIC-wrong-password")
                .await,
            Err(AuthError::InvalidCredentials)
        ));
        auth.shutdown(None).await;
        db.close().await;
    }
}
#[tokio::test]
async fn logout_waits_for_registered_connection_owners_and_never_keeps_a_raw_token() {
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    let session = auth
        .setup(
            "public-admin",
            "PUBLIC-test-password!",
            "PUBLIC-test-password!",
        )
        .await
        .unwrap();
    let identity = session.token.hash();
    let lease = auth
        .registry()
        .register(identity.clone(), session.user.id)
        .unwrap();
    let worker = auth.clone();
    let token = session.token.expose().to_owned();
    let logout = tokio::spawn(async move { worker.logout(Some(token)).await });
    tokio::time::timeout(Duration::from_secs(1), lease.cancelled())
        .await
        .unwrap();
    assert!(
        !logout.is_finished(),
        "revocation returned while its registered owner was still active"
    );
    assert!(auth.current(&identity).await.unwrap().is_none());
    drop(lease);
    logout.await.unwrap().unwrap();
    auth.shutdown(None).await;
    db.close().await;
}
#[tokio::test]
async fn dropping_the_logout_waiter_does_not_lose_its_committed_revocation_obligation() {
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    let session = auth
        .setup(
            "public-admin",
            "PUBLIC-test-password!",
            "PUBLIC-test-password!",
        )
        .await
        .unwrap();
    let identity = session.token.hash();
    let lease = auth
        .registry()
        .register(identity.clone(), session.user.id)
        .unwrap();
    let worker = auth.clone();
    let token = session.token.expose().to_owned();
    let waiter = tokio::spawn(async move { worker.logout(Some(token)).await });
    tokio::time::timeout(Duration::from_secs(1), lease.cancelled())
        .await
        .unwrap();
    waiter.abort();
    assert!(waiter.await.is_err_and(|error| error.is_cancelled()));
    drop(lease);
    auth.shutdown(None).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    db.close().await;
}
#[tokio::test]
async fn password_change_revokes_old_sessions_and_issues_only_one_replacement() {
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    let first = auth
        .setup(
            "public-admin",
            "PUBLIC-old-password!",
            "PUBLIC-old-password!",
        )
        .await
        .unwrap();
    let other = auth
        .login("public-admin", "PUBLIC-old-password!")
        .await
        .unwrap();
    assert!(matches!(
        auth.change_password(
            first.user.id,
            "PUBLIC-wrong-password!",
            "PUBLIC-new-password!",
            "PUBLIC-new-password!"
        )
        .await,
        Err(AuthError::CurrentPasswordIncorrect)
    ));
    let replacement = auth
        .change_password(
            first.user.id,
            "PUBLIC-old-password!",
            "PUBLIC-new-password!",
            "PUBLIC-new-password!",
        )
        .await
        .unwrap();
    assert!(auth.current(&first.token.hash()).await.unwrap().is_none());
    assert!(auth.current(&other.token.hash()).await.unwrap().is_none());
    assert!(
        auth.current(&replacement.token.hash())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    assert!(matches!(
        auth.login("public-admin", "PUBLIC-old-password!").await,
        Err(AuthError::InvalidCredentials)
    ));
    assert!(
        auth.login("public-admin", "PUBLIC-new-password!")
            .await
            .is_ok()
    );
    auth.shutdown(None).await;
    db.close().await;
}
#[tokio::test]
async fn missing_and_expired_session_identities_are_not_authenticated() {
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    assert!(matches!(
        auth.login("public-admin", "PUBLIC-password!").await,
        Err(AuthError::SetupNotComplete)
    ));
    let session = auth
        .setup(
            "public-admin",
            "PUBLIC-test-password!",
            "PUBLIC-test-password!",
        )
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET expires_at='2020-01-01 00:00:00.000000'")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(auth.current(&session.token.hash()).await.unwrap().is_none());
    auth.shutdown(None).await;
    db.close().await;
}

#[tokio::test]
async fn duplicate_session_identity_with_an_absent_parent_is_fail_closed() {
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    let session = auth
        .setup(
            "public-admin",
            "PUBLIC-test-password!",
            "PUBLIC-test-password!",
        )
        .await
        .unwrap();
    let identity = session.token.hash();
    let mut connection = db.pool().acquire().await.unwrap();
    sqlx::raw_sql("ALTER TABLE sessions RENAME TO original_sessions; CREATE TABLE sessions(id INTEGER PRIMARY KEY,user_id INTEGER,token TEXT,expires_at DATETIME,created_at DATETIME); INSERT INTO sessions SELECT * FROM original_sessions;").execute(&mut *connection).await.unwrap();
    sqlx::query("INSERT INTO sessions(user_id,token,expires_at,created_at) VALUES(999999,?,'9999-01-01 00:00:00.000000','2024-01-01 00:00:00.000000')").bind(identity.as_str()).execute(&mut *connection).await.unwrap();
    drop(connection);
    assert!(
        matches!(auth.current(&identity).await, Err(AuthError::Database)),
        "duplicate session identity must not be accepted merely because one row has an absent parent"
    );
    auth.shutdown(None).await;
    db.close().await;
}

#[tokio::test]
async fn registry_shutdown_closes_admission_and_registration_is_bounded() {
    use dockervault_service::auth::{SessionHash, SessionRegistry};
    let registry = SessionRegistry::default();
    let mut leases = Vec::new();
    for _ in 0..512 {
        leases.push(
            registry
                .register(SessionHash::from_token("PUBLIC-test-token"), 1)
                .unwrap(),
        );
    }
    assert!(matches!(
        registry.register(SessionHash::from_token("PUBLIC-test-token"), 1),
        Err(AuthError::Busy)
    ));
    drop(leases);
    registry.shutdown().await.unwrap();
    assert!(matches!(
        registry.register(SessionHash::from_token("PUBLIC-test-token"), 1),
        Err(AuthError::Busy)
    ));
}

#[tokio::test]
async fn native_password_writes_preserve_partial_utf8_byte_truncation() {
    let fixture = Fixture::new();
    let db = fixture.open().await;
    let auth = AuthService::new(db.pool().clone());
    let password = format!("{}äX", "A".repeat(71));
    let equivalent = format!("{}ãY", "A".repeat(71));
    auth.setup("public-admin", &password, &password)
        .await
        .unwrap();
    assert!(auth.login("public-admin", &equivalent).await.is_ok());
    auth.shutdown(None).await;
    db.close().await;
}
