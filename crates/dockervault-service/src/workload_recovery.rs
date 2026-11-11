//! Reconcile the published workload journal only; never replay archives or uploads.
use crate::{
    docker::DockerClient,
    journal::{Journal, JournalError},
    ownership::{Owner, WorkError},
};
use std::{collections::HashSet, time::Duration};
#[derive(Debug)]
pub enum RecoveryError {
    Journal(JournalError),
    Owner(WorkError),
    Manual(Vec<String>),
}
impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Journal(error) => std::fmt::Display::fmt(error, f),
            Self::Owner(error) => std::fmt::Display::fmt(error, f),
            Self::Manual(errors) => f.write_str(&errors.join("; ")),
        }
    }
}
impl std::error::Error for RecoveryError {}
impl From<JournalError> for RecoveryError {
    fn from(error: JournalError) -> Self {
        Self::Journal(error)
    }
}
impl From<WorkError> for RecoveryError {
    fn from(error: WorkError) -> Self {
        Self::Owner(error)
    }
}
#[derive(Clone)]
pub struct Recovery {
    journal: Journal,
    docker: DockerClient,
    owner: Owner,
}
impl Recovery {
    pub fn new(journal: Journal, docker: DockerClient) -> Result<Self, RecoveryError> {
        Ok(Self {
            journal,
            docker,
            owner: Owner::new(1)?,
        })
    }
    pub async fn shutdown(&self, grace: Option<Duration>) {
        self.owner.shutdown(grace).await;
    }
    async fn start_running(&self, id: &str) -> bool {
        for attempt in 0..3 {
            if self.docker.state(id).await.as_deref() == Ok("running") {
                return true;
            }
            if self.docker.start(id).await.is_ok() {
                for check in 0..3 {
                    if self.docker.state(id).await.as_deref() == Ok("running") {
                        return true;
                    }
                    if check < 2 {
                        tokio::time::sleep(Duration::from_millis(100 * (attempt + 1))).await;
                    }
                }
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(100 * (attempt + 1))).await;
            }
        }
        false
    }
    pub async fn reconcile(&self) -> Result<(), RecoveryError> {
        let service = self.clone();
        self.owner
            .submit_async(move |_| async move {
                let mut after = 0;
                let mut errors = Vec::new();
                loop {
                    let batch = service.journal.unfinished_after(after).await?;
                    if batch.is_empty() {
                        break;
                    }
                    for operation in batch {
                        after = operation.id;
                        let mut failures = Vec::new();
                        let mut ready = HashSet::new();
                        for (identity, value) in &operation.resources {
                            if value.original != "running" || !value.requested || value.restored {
                                continue;
                            }
                            let state = service.docker.state(identity).await.ok();
                            if !matches!(state.as_deref(), Some("running" | "exited" | "created")) {
                                failures.push(identity.clone());
                                continue;
                            }
                            if !value.stopped {
                                if state.as_deref() == Some("running")
                                    && service.docker.stop(identity).await.is_err()
                                {
                                    failures.push(identity.clone());
                                    continue;
                                }
                                service.journal.stop(operation.id, identity, true).await?;
                            }
                            ready.insert(identity.clone());
                        }
                        // Settle every old stop intent before starting any dependency again.
                        for identity in operation.restart_order() {
                            if ready.contains(&identity) && !service.start_running(&identity).await
                            {
                                failures.push(identity);
                            }
                        }
                        service
                            .journal
                            .record_recovery(operation.id, failures.clone())
                            .await?;
                        if !failures.is_empty() {
                            errors.push(format!(
                                "Manual recovery required for workload IDs: {}",
                                failures.join(", ")
                            ));
                        }
                    }
                }
                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(RecoveryError::Manual(errors))
                }
            })?
            .wait()
            .await?
    }
}
