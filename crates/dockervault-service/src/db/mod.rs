//! SQLite persistence; opening a reader never runs migrations or recovery.
mod migrations;
pub(crate) mod schema;

use crate::ownership::{Owner, WorkError};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbError {
    Database,
    Busy,
    InvalidLedger,
    InvalidSchema,
    Integrity,
    ForeignKey,
    Cancelled,
    Io,
    Owner,
}
impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SQLite operation failed: {self:?}")
    }
}
impl std::error::Error for DbError {}
impl From<rusqlite::Error> for DbError {
    fn from(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::Busy
            }
            Some(rusqlite::ErrorCode::OperationInterrupted) => Self::Cancelled,
            _ => Self::Database,
        }
    }
}
impl From<sqlx::Error> for DbError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<std::io::Error> for DbError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}
impl From<WorkError> for DbError {
    fn from(_: WorkError) -> Self {
        Self::Owner
    }
}

#[derive(Debug)]
pub struct MigrationReport {
    pub from_version: usize,
    pub to_version: usize,
    pub backup: Option<PathBuf>,
}

pub struct Database {
    pool: SqlitePool,
    migration: Option<MigrationReport>,
}
impl Database {
    pub async fn open(
        path: impl AsRef<Path>,
        backup_dir: impl AsRef<Path>,
        owner: &Owner,
    ) -> Result<Self, DbError> {
        let path = path.as_ref().to_path_buf();
        let source = path.clone();
        let backup_dir = backup_dir.as_ref().to_path_buf();
        let report = owner
            .submit_blocking(move |cancel| migrations::run(&source, &backup_dir, &cancel))?
            .wait()
            .await??;
        let pool = connect(&path, false).await?;
        Ok(Self {
            pool,
            migration: Some(report),
        })
    }
    pub async fn read_only(path: impl AsRef<Path>) -> Result<Self, DbError> {
        Ok(Self {
            pool: connect(path.as_ref(), true).await?,
            migration: None,
        })
    }
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
    pub fn migration(&self) -> Option<&MigrationReport> {
        self.migration.as_ref()
    }
    pub async fn close(self) {
        self.pool.close().await;
    }
}
async fn connect(path: &Path, read_only: bool) -> Result<SqlitePool, DbError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(read_only)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    Ok(SqlitePoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(5))
        .after_connect(|connection, _| {
            Box::pin(async move {
                let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                    .fetch_one(connection)
                    .await?;
                if enabled != 1 {
                    return Err(sqlx::Error::Protocol("Foreign keys must be enabled".into()));
                }
                Ok(())
            })
        })
        .connect_with(options)
        .await?)
}
