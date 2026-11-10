//! Existing bcrypt/session persistence with owned mutations and revocation settlement.
use crate::ownership::{Owner, WorkError};
use crate::recovery_activation::{ClaimKind, PreparationClaims, PreparationError};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use dockervault_core::timestamp::Timestamp;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqliteConnection, SqlitePool, pool::PoolConnection, sqlite::SqliteRow};
use std::{
    fmt,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    Preparation(PreparationError),
    AlreadySetup,
    SetupNotComplete,
    PasswordsMismatch,
    InvalidCredentials,
    CurrentPasswordIncorrect,
    NewPasswordsMismatch,
    UserNotFound,
    Database,
    Crypto,
    Busy,
    Cancelled,
}
impl From<sqlx::Error> for AuthError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<WorkError> for AuthError {
    fn from(_: WorkError) -> Self {
        Self::Busy
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionHash(String);
impl SessionHash {
    pub fn from_token(token: &str) -> Self {
        Self(
            Sha256::digest(token.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        )
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
pub struct SessionToken(String);
impl fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionToken([REDACTED])")
    }
}
impl SessionToken {
    fn generate() -> Result<Self, AuthError> {
        let mut bytes = [0u8; 64];
        getrandom::fill(&mut bytes).map_err(|_| AuthError::Crypto)?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn hash(&self) -> SessionHash {
        SessionHash::from_token(&self.0)
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub created_at: String,
    pub last_login: Option<String>,
}
pub struct AuthSession {
    pub user: User,
    pub token: SessionToken,
}
struct StoredUser {
    user: User,
    password_hash: String,
}
fn timestamp(value: DateTime<Utc>) -> Result<Timestamp, AuthError> {
    Timestamp::parse(&value.to_rfc3339()).map_err(|_| AuthError::Database)
}
fn user(row: &SqliteRow) -> Result<User, AuthError> {
    let created: String = row.try_get("created_at")?;
    let last: Option<String> = row.try_get("last_login")?;
    Ok(User {
        id: row.try_get("id")?,
        username: row.try_get("username")?,
        is_admin: row
            .try_get::<Option<bool>, _>("is_admin")?
            .ok_or(AuthError::Database)?,
        created_at: Timestamp::parse(&created)
            .map_err(|_| AuthError::Database)?
            .api(),
        last_login: last
            .map(|value| {
                Timestamp::parse(&value)
                    .map(|value| value.api())
                    .map_err(|_| AuthError::Database)
            })
            .transpose()?,
    })
}
fn stored(rows: Vec<SqliteRow>) -> Result<Option<StoredUser>, AuthError> {
    if rows.len() > 1 {
        return Err(AuthError::Database);
    }
    rows.first()
        .map(|row| {
            Ok(StoredUser {
                user: user(row)?,
                password_hash: row.try_get("password_hash")?,
            })
        })
        .transpose()
}

// The lease belongs to the actual connection owner, not its HTTP upgrade waiter.
struct Connection {
    identity: SessionHash,
    user_id: i64,
    cancel: CancellationToken,
    closed: watch::Sender<bool>,
}
pub struct SessionLease {
    connection: Arc<Connection>,
}
impl SessionLease {
    pub fn is_cancelled(&self) -> bool {
        self.connection.cancel.is_cancelled()
    }
    pub async fn cancelled(&self) {
        self.connection.cancel.cancelled().await
    }
}
impl Drop for SessionLease {
    fn drop(&mut self) {
        self.connection.closed.send_replace(true);
    }
}
#[derive(Default)]
struct RegistryState {
    closed: bool,
    connections: Vec<Weak<Connection>>,
}
#[derive(Default)]
pub struct SessionRegistry {
    connections: Mutex<RegistryState>,
}
impl SessionRegistry {
    pub fn is_closed(&self) -> bool {
        self.connections
            .lock()
            .map(|state| state.closed)
            .unwrap_or(true)
    }
    pub fn register(&self, identity: SessionHash, user_id: i64) -> Result<SessionLease, AuthError> {
        let mut state = self.connections.lock().map_err(|_| AuthError::Busy)?;
        if state.closed {
            return Err(AuthError::Busy);
        }
        let connections = &mut state.connections;
        connections.retain(|entry| entry.strong_count() > 0);
        if connections.len() >= 512 {
            return Err(AuthError::Busy);
        }
        let (closed, _) = watch::channel(false);
        let connection = Arc::new(Connection {
            identity,
            user_id,
            cancel: CancellationToken::new(),
            closed,
        });
        connections.push(Arc::downgrade(&connection));
        Ok(SessionLease { connection })
    }
    async fn revoke(&self, predicate: impl Fn(&Connection) -> bool) -> Result<(), AuthError> {
        let connections: Vec<_> = self
            .connections
            .lock()
            .map_err(|_| AuthError::Busy)?
            .connections
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|entry| predicate(entry))
            .collect();
        for connection in &connections {
            connection.cancel.cancel();
        }
        for connection in connections {
            let mut closed = connection.closed.subscribe();
            let _ = closed.wait_for(|closed| *closed).await;
        }
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<(), AuthError> {
        self.connections.lock().map_err(|_| AuthError::Busy)?.closed = true;
        self.revoke(|_| true).await
    }
}

#[derive(Clone)]
pub struct AuthService {
    inner: Arc<Inner>,
}
struct Inner {
    pool: SqlitePool,
    preparation: PreparationClaims,
    operations: Owner,
    crypto: Owner,
    registry: SessionRegistry,
}
impl AuthService {
    pub fn new(pool: SqlitePool) -> Self {
        let filename = pool.connect_options().get_filename().to_path_buf();
        let preparation = PreparationClaims::new(if filename == std::path::Path::new(":memory:") {
            None
        } else {
            Some(filename)
        });
        Self {
            inner: Arc::new(Inner {
                pool,
                preparation,
                operations: Owner::new(16).expect("constant nonzero capacity"),
                crypto: Owner::new(2).expect("constant nonzero capacity"),
                registry: SessionRegistry::default(),
            }),
        }
    }
    pub fn registry(&self) -> &SessionRegistry {
        &self.inner.registry
    }
    pub async fn shutdown(&self, grace: Option<Duration>) {
        self.inner.operations.shutdown(grace).await;
        self.inner.crypto.shutdown(grace).await;
        let _ = self.registry().shutdown().await;
    }
    pub async fn setup_complete(&self) -> Result<bool, AuthError> {
        Ok(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(&self.inner.pool)
            .await?
            > 0)
    }
    pub async fn current(&self, identity: &SessionHash) -> Result<Option<User>, AuthError> {
        let rows =
            sqlx::query("SELECT user_id FROM sessions WHERE token=? AND expires_at>? LIMIT 2")
                .bind(identity.as_str())
                .bind(timestamp(Utc::now())?.sql())
                .fetch_all(&self.inner.pool)
                .await?;
        if rows.len() > 1 {
            return Err(AuthError::Database);
        }
        let Some(session) = rows.first() else {
            return Ok(None);
        };
        let user_id: i64 = session.try_get("user_id")?;
        let row = sqlx::query("SELECT * FROM users WHERE id=?")
            .bind(user_id)
            .fetch_optional(&self.inner.pool)
            .await?;
        row.as_ref().map(user).transpose()
    }
    async fn begin(&self) -> Result<PoolConnection<Sqlite>, AuthError> {
        let mut connection = self.inner.pool.acquire().await?;
        if sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .is_err()
        {
            let _ = connection.close().await;
            return Err(AuthError::Database);
        }
        Ok(connection)
    }
    async fn finish<T>(
        mut connection: PoolConnection<Sqlite>,
        result: Result<T, AuthError>,
    ) -> Result<T, AuthError> {
        match result {
            Ok(value) => match sqlx::query("COMMIT").execute(&mut *connection).await {
                Ok(_) => Ok(value),
                Err(_) => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                    let _ = connection.close().await;
                    Err(AuthError::Database)
                }
            },
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                let _ = connection.close().await;
                Err(error)
            }
        }
    }
    async fn create_session(
        connection: &mut SqliteConnection,
        user_id: i64,
        token: &SessionToken,
    ) -> Result<(), AuthError> {
        let now = Utc::now();
        sqlx::query("INSERT INTO sessions(user_id,token,expires_at,created_at) VALUES(?,?,?,?)")
            .bind(user_id)
            .bind(token.hash().as_str())
            .bind(timestamp(now + TimeDelta::days(7))?.sql())
            .bind(timestamp(now)?.sql())
            .execute(connection)
            .await?;
        Ok(())
    }
    async fn credentials(
        &self,
        password: String,
        hash: Option<String>,
        cancel: CancellationToken,
    ) -> Result<(Option<String>, SessionToken), AuthError> {
        self.inner
            .crypto
            .submit_blocking(move |_| {
                if cancel.is_cancelled() {
                    return Err(AuthError::Cancelled);
                }
                let bytes = password.as_bytes();
                let bytes = &bytes[..bytes.len().min(72)];
                let result = match hash {
                    Some(hash) => {
                        if !bcrypt::verify(bytes, &hash).map_err(|_| AuthError::Crypto)? {
                            return Err(AuthError::InvalidCredentials);
                        }
                        None
                    }
                    None => Some(
                        bcrypt::hash(bytes, bcrypt::DEFAULT_COST).map_err(|_| AuthError::Crypto)?,
                    ),
                };
                if cancel.is_cancelled() {
                    return Err(AuthError::Cancelled);
                }
                Ok((result, SessionToken::generate()?))
            })?
            .wait()
            .await?
    }
    pub async fn setup(
        &self,
        username: &str,
        password: &str,
        confirmation: &str,
    ) -> Result<AuthSession, AuthError> {
        let service = self.clone();
        let username = username.to_owned();
        let password = password.to_owned();
        let confirmation = confirmation.to_owned();
        self.inner.operations.submit_async(move |cancel|async move {
            if password!=confirmation {return Err(AuthError::PasswordsMismatch)}
            if cancel.is_cancelled() {return Err(AuthError::Cancelled)}
            let claims=service.inner.preparation.clone();
            let _preparation=service.inner.crypto.submit_blocking(move |_| {
                let guard=claims.try_claim(ClaimKind::Setup)?;
                guard.discard_incomplete(std::path::Path::new("/app/data/.credential_key"))?;
                Ok::<_,PreparationError>(guard)
            })?.wait().await?.map_err(AuthError::Preparation)?;
            if cancel.is_cancelled() {return Err(AuthError::Cancelled)}
            let mut connection=service.begin().await?;
            let result=async {
                if sqlx::query_scalar::<_,i64>("SELECT count(*) FROM users").fetch_one(&mut *connection).await?>0 {return Err(AuthError::AlreadySetup)}
                if password!=confirmation {return Err(AuthError::PasswordsMismatch)}
                let (hash,token)=service.credentials(password,None,cancel.clone()).await?;
                if cancel.is_cancelled() {return Err(AuthError::Cancelled)}
                let now=timestamp(Utc::now())?.sql();
                let id=sqlx::query("INSERT INTO users(username,password_hash,is_admin,created_at,updated_at) VALUES(?,?,1,?,?)")
                    .bind(username).bind(hash.ok_or(AuthError::Crypto)?).bind(&now).bind(&now).execute(&mut *connection).await?.last_insert_rowid();
                Self::create_session(&mut connection,id,&token).await?;
                let row=sqlx::query("SELECT * FROM users WHERE id=?").bind(id).fetch_one(&mut *connection).await?;
                Ok(AuthSession {user:user(&row)?,token})
            }.await;
            Self::finish(connection,result).await
        })?.wait().await?
    }
    pub async fn login(&self, username: &str, password: &str) -> Result<AuthSession, AuthError> {
        let service = self.clone();
        let username = username.to_owned();
        let password = password.to_owned();
        self.inner
            .operations
            .submit_async(move |cancel| async move {
                if !service.setup_complete().await? {
                    return Err(AuthError::SetupNotComplete);
                }
                let found = stored(
                    sqlx::query("SELECT * FROM users WHERE username=? LIMIT 2")
                        .bind(username)
                        .fetch_all(&service.inner.pool)
                        .await?,
                )?;
                let expected = found
                    .as_ref()
                    .map(|found| found.password_hash.clone())
                    .unwrap_or_else(|| {
                        "$2b$12$LJ3m4ys3Lg2UsSEgDKr9ceULEiZoSUsVT2D0E.kFYRPQr0K3YBmiy".to_owned()
                    });
                let (_, token) = service
                    .credentials(password, Some(expected.clone()), cancel.clone())
                    .await?;
                let mut found = found.ok_or(AuthError::InvalidCredentials)?;
                if cancel.is_cancelled() {
                    return Err(AuthError::Cancelled);
                }
                let mut connection = service.begin().await?;
                let result = async {
                    let current: Option<String> =
                        sqlx::query_scalar("SELECT password_hash FROM users WHERE id=?")
                            .bind(found.user.id)
                            .fetch_optional(&mut *connection)
                            .await?;
                    if current.as_deref() != Some(expected.as_str()) {
                        return Err(AuthError::InvalidCredentials);
                    }
                    let now = timestamp(Utc::now())?;
                    sqlx::query("UPDATE users SET last_login=?,updated_at=? WHERE id=?")
                        .bind(now.sql())
                        .bind(now.sql())
                        .bind(found.user.id)
                        .execute(&mut *connection)
                        .await?;
                    Self::create_session(&mut connection, found.user.id, &token).await?;
                    found.user.last_login = Some(now.api());
                    Ok(AuthSession {
                        user: found.user,
                        token,
                    })
                }
                .await;
                Self::finish(connection, result).await
            })?
            .wait()
            .await?
    }
    pub async fn logout(&self, token: Option<String>) -> Result<(), AuthError> {
        let service = self.clone();
        self.inner
            .operations
            .submit_async(move |_| async move {
                if let Some(token) = token {
                    let identity = SessionHash::from_token(&token);
                    let mut connection = service.begin().await?;
                    let result = sqlx::query("DELETE FROM sessions WHERE token=?")
                        .bind(identity.as_str())
                        .execute(&mut *connection)
                        .await
                        .map(|_| ())
                        .map_err(AuthError::from);
                    Self::finish(connection, result).await?;
                    service
                        .registry()
                        .revoke(|connection| connection.identity == identity)
                        .await?;
                }
                Ok(())
            })?
            .wait()
            .await?
    }
    pub async fn change_password(
        &self,
        user_id: i64,
        current: &str,
        new: &str,
        confirmation: &str,
    ) -> Result<AuthSession, AuthError> {
        let service = self.clone();
        let current = current.to_owned();
        let new = new.to_owned();
        let confirmation = confirmation.to_owned();
        self.inner
            .operations
            .submit_async(move |cancel| async move {
                let found = stored(
                    sqlx::query("SELECT * FROM users WHERE id=? LIMIT 2")
                        .bind(user_id)
                        .fetch_all(&service.inner.pool)
                        .await?,
                )?
                .ok_or(AuthError::UserNotFound)?;
                match service
                    .credentials(current, Some(found.password_hash.clone()), cancel.clone())
                    .await
                {
                    Err(AuthError::InvalidCredentials) => {
                        return Err(AuthError::CurrentPasswordIncorrect);
                    }
                    other => {
                        other?;
                    }
                }
                if new != confirmation {
                    return Err(AuthError::NewPasswordsMismatch);
                }
                let (hash, token) = service.credentials(new, None, cancel.clone()).await?;
                if cancel.is_cancelled() {
                    return Err(AuthError::Cancelled);
                }
                let mut connection = service.begin().await?;
                let result = async {
                    let existing: Option<String> =
                        sqlx::query_scalar("SELECT password_hash FROM users WHERE id=?")
                            .bind(user_id)
                            .fetch_optional(&mut *connection)
                            .await?;
                    if existing.as_deref() != Some(found.password_hash.as_str()) {
                        return Err(AuthError::CurrentPasswordIncorrect);
                    }
                    sqlx::query("UPDATE users SET password_hash=?,updated_at=? WHERE id=?")
                        .bind(hash.ok_or(AuthError::Crypto)?)
                        .bind(timestamp(Utc::now())?.sql())
                        .bind(user_id)
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("DELETE FROM sessions WHERE user_id=?")
                        .bind(user_id)
                        .execute(&mut *connection)
                        .await?;
                    Self::create_session(&mut connection, user_id, &token).await?;
                    Ok(AuthSession {
                        user: found.user,
                        token,
                    })
                }
                .await;
                let session = Self::finish(connection, result).await?;
                service
                    .registry()
                    .revoke(|connection| connection.user_id == user_id)
                    .await?;
                Ok(session)
            })?
            .wait()
            .await?
    }
}
