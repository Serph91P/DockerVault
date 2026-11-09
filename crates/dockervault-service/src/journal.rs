//! Durable workload intents; no archive/upload effect is replayed by the journal.
use crate::ownership::{Owner, WorkError};
use chrono::Utc;
use dockervault_core::{encoding::python_json, timestamp::Timestamp};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use std::time::Duration;

const INCOMPLETE: &str = "Workload recovery incomplete; retry reconciliation before new jobs";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalError {
    Database,
    InvalidRecord,
    Missing,
    UnknownOriginalState,
    Owner,
}
impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Database => "Workload journal database operation failed",
            Self::InvalidRecord => "Invalid workload journal record",
            Self::Missing => "Workload journal record missing",
            Self::UnknownOriginalState => "Unknown workload state; refusing external changes",
            Self::Owner => "Workload journal owner unavailable",
        })
    }
}
impl std::error::Error for JournalError {}
impl From<sqlx::Error> for JournalError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<WorkError> for JournalError {
    fn from(_: WorkError) -> Self {
        Self::Owner
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Backup,
    Restore,
}
impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Backup => "backup",
            Self::Restore => "restore",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub original: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_index: Option<usize>,
    pub requested: bool,
    pub stopped: bool,
    pub restored: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
pub struct Operation {
    pub id: i64,
    pub backup_id: Option<i64>,
    pub kind: Kind,
    pub phase: String,
    pub state: String,
    pub resources: IndexMap<String, Resource>,
    pub error: Option<String>,
}
impl Operation {
    pub fn restart_order(&self) -> Vec<String> {
        let mut ready = self
            .resources
            .iter()
            .enumerate()
            .filter(|(_, (_, value))| {
                value.original == "running" && value.requested && !value.restored
            })
            .collect::<Vec<_>>();
        ready.sort_by_key(|(index, (_, value))| {
            value
                .start_index
                .unwrap_or(self.resources.len() - index - 1)
        });
        ready
            .into_iter()
            .map(|(_, (identity, _))| identity.clone())
            .collect()
    }
}
fn decode(row: SqliteRow) -> Result<Operation, JournalError> {
    let kind: String = row.try_get("kind")?;
    let resources: String = row.try_get("resources")?;
    Ok(Operation {
        id: row.try_get("id")?,
        backup_id: row.try_get("backup_id")?,
        kind: match kind.as_str() {
            "backup" => Kind::Backup,
            "restore" => Kind::Restore,
            _ => return Err(JournalError::InvalidRecord),
        },
        phase: row.try_get("phase")?,
        state: row.try_get("state")?,
        resources: serde_json::from_str(&resources).map_err(|_| JournalError::InvalidRecord)?,
        error: row.try_get("error")?,
    })
}
#[derive(Clone)]
pub struct Journal {
    pool: SqlitePool,
    owner: Owner,
}
impl Journal {
    pub fn new(pool: SqlitePool) -> Result<Self, JournalError> {
        Ok(Self {
            pool,
            owner: Owner::new(16)?,
        })
    }
    pub async fn shutdown(&self, grace: Option<Duration>) {
        self.owner.shutdown(grace).await;
    }
    pub async fn load(&self, id: i64) -> Result<Operation, JournalError> {
        decode(
            sqlx::query("SELECT * FROM workload_operations WHERE id=?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(JournalError::Missing)?,
        )
    }
    pub async fn unfinished_after(&self, id: i64) -> Result<Vec<Operation>, JournalError> {
        sqlx::query("SELECT * FROM workload_operations WHERE state != 'finished' AND id>? ORDER BY id LIMIT 64").bind(id).fetch_all(&self.pool).await?.into_iter().map(decode).collect()
    }
    pub async fn begin(
        &self,
        backup_id: Option<i64>,
        kind: Kind,
        originals: IndexMap<String, Option<String>>,
        start_order: Option<Vec<String>>,
    ) -> Result<i64, JournalError> {
        if originals.values().any(Option::is_none) {
            return Err(JournalError::UnknownOriginalState);
        }
        let mut order = IndexMap::<String, usize>::new();
        for identity in start_order.unwrap_or_else(|| originals.keys().rev().cloned().collect()) {
            let rank = order.len();
            order.entry(identity).or_insert(rank);
        }
        for identity in originals.keys() {
            let rank = order.len();
            order.entry(identity.clone()).or_insert(rank);
        }
        let resources = originals
            .into_iter()
            .map(|(identity, state)| {
                let original = state.ok_or(JournalError::UnknownOriginalState)?;
                let resource = Resource {
                    restored: original != "running",
                    original,
                    start_index: Some(order[&identity]),
                    requested: false,
                    stopped: false,
                    extra: Map::new(),
                };
                Ok((identity, resource))
            })
            .collect::<Result<IndexMap<_, _>, JournalError>>()?;
        let resources =
            python_json(&serde_json::to_value(resources).map_err(|_| JournalError::InvalidRecord)?);
        let pool = self.pool.clone();
        self.owner.submit_async(move |_|async move {
            let now=Timestamp::parse(&Utc::now().to_rfc3339()).map_err(|_|JournalError::InvalidRecord)?.sql();
            let result=sqlx::query("INSERT INTO workload_operations(backup_id,kind,phase,state,resources,error,created_at) VALUES(?,?,'claimed','active',?,NULL,?)").bind(backup_id).bind(kind.as_str()).bind(resources).bind(now).execute(&pool).await?;
            Ok(result.last_insert_rowid())
        })?.wait().await?
    }
    async fn update<F>(&self, id: i64, change: F) -> Result<(), JournalError>
    where
        F: FnOnce(&mut Operation) -> Result<(), JournalError> + Send + 'static,
    {
        let pool = self.pool.clone();
        self.owner.submit_async(move |_|async move {
            let mut connection=pool.acquire().await?;
            let mut transaction=sqlx::Connection::begin_with(&mut *connection,"BEGIN IMMEDIATE").await?;
            let result=async {
                let row=sqlx::query("SELECT * FROM workload_operations WHERE id=?").bind(id).fetch_optional(&mut *transaction).await?.ok_or(JournalError::Missing)?;
                let mut operation=decode(row)?;change(&mut operation)?;
                let resources=python_json(&serde_json::to_value(&operation.resources).map_err(|_|JournalError::InvalidRecord)?);
                sqlx::query("UPDATE workload_operations SET phase=?,state=?,resources=?,error=? WHERE id=?").bind(&operation.phase).bind(&operation.state).bind(resources).bind(&operation.error).bind(id).execute(&mut *transaction).await?;Ok(())
            }.await;
            let settled=if result.is_ok() {transaction.commit().await}else{transaction.rollback().await};
            // A failed COMMIT only queues SQLx's drop rollback; close joins it before release.
            if settled.is_err() {connection.close().await?;}
            settled?;
            result
        })?.wait().await?
    }
    pub async fn phase(&self, id: i64, phase: &str) -> Result<(), JournalError> {
        let phase = phase.to_owned();
        self.update(id, move |operation| {
            operation.phase = phase;
            Ok(())
        })
        .await
    }
    pub async fn stop(&self, id: i64, identity: &str, confirmed: bool) -> Result<(), JournalError> {
        let identity = identity.to_owned();
        self.update(id, move |operation| {
            let resource = operation
                .resources
                .get_mut(&identity)
                .ok_or(JournalError::InvalidRecord)?;
            resource.requested = true;
            resource.stopped = confirmed;
            operation.phase = if confirmed { "stopped" } else { "stopping" }.into();
            Ok(())
        })
        .await
    }
    pub async fn recovered(&self, id: i64, failures: Vec<String>) -> Result<(), JournalError> {
        self.update(id, move |operation| {
            for (identity, resource) in &mut operation.resources {
                resource.restored = !failures.contains(identity);
            }
            Ok(())
        })
        .await
    }
    pub async fn record_recovery(
        &self,
        id: i64,
        failures: Vec<String>,
    ) -> Result<(), JournalError> {
        let pool = self.pool.clone();
        self.owner.submit_async(move |_|async move {
            let mut connection=pool.acquire().await?;
            let mut transaction=sqlx::Connection::begin_with(&mut *connection,"BEGIN IMMEDIATE").await?;
            let result=async {
                let row=sqlx::query("SELECT * FROM workload_operations WHERE id=?").bind(id).fetch_optional(&mut *transaction).await?.ok_or(JournalError::Missing)?;
                let mut operation=decode(row)?;
                for (identity,resource) in &mut operation.resources {resource.restored = !failures.contains(identity);}
                let state=if failures.is_empty() {"finished"}else{"recovery_required"};
                let message=if failures.is_empty() {"Interrupted operation reconciled; retry the job explicitly".into()}else{format!("Manual recovery required for workload IDs: {}",failures.join(", "))};
                let resources=python_json(&serde_json::to_value(&operation.resources).map_err(|_|JournalError::InvalidRecord)?);
                sqlx::query("UPDATE workload_operations SET state=?,error=?,resources=? WHERE id=?").bind(state).bind(&message).bind(resources).bind(id).execute(&mut *transaction).await?;
                if let Some(backup)=operation.backup_id {
                    let now=Timestamp::parse(&Utc::now().to_rfc3339()).map_err(|_|JournalError::InvalidRecord)?.sql();
                    if operation.kind==Kind::Backup {
                        sqlx::query("UPDATE backups SET status='FAILED',error_message=?,completed_at=? WHERE id=?").bind(&message).bind(&now).bind(backup).execute(&mut *transaction).await?;
                    }
                    sqlx::query("INSERT INTO backup_logs(backup_id,level,step,message,created_at) VALUES(?,'ERROR','recovery',?,?)").bind(backup).bind(&message).bind(now).execute(&mut *transaction).await?;
                }
                Ok(())
            }.await;
            let settled=if result.is_ok() {transaction.commit().await}else{transaction.rollback().await};
            // A failed COMMIT only queues SQLx's drop rollback; close joins it before release.
            if settled.is_err() {connection.close().await?;}
            settled?;
            result
        })?.wait().await?
    }
    pub async fn finish(&self, id: Option<i64>) -> Result<(), JournalError> {
        let Some(id) = id else { return Ok(()) };
        self.update(id, |operation| {
            let outstanding = operation
                .resources
                .values()
                .any(|value| value.requested && !value.restored);
            operation.state = if outstanding {
                "recovery_required"
            } else {
                "finished"
            }
            .into();
            if outstanding {
                operation.error = Some(INCOMPLETE.into());
            }
            Ok(())
        })
        .await
    }
}
