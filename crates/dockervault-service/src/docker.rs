//! Unix Docker API calls remain owned until their response settles, even after caller loss.
use crate::ownership::{Owner, WorkError};
use bollard::{API_DEFAULT_VERSION, Docker, query_parameters::StopContainerOptions};
use std::{future::Future, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerError {
    Unavailable,
    InvalidResponse,
    Owner,
}
impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "Docker API request failed",
            Self::InvalidResponse => "Docker API response lacks workload identity or state",
            Self::Owner => "Docker request owner unavailable",
        })
    }
}
impl std::error::Error for DockerError {}
impl From<WorkError> for DockerError {
    fn from(_: WorkError) -> Self {
        Self::Owner
    }
}
#[derive(Clone)]
pub struct DockerClient {
    api: Docker,
    owner: Owner,
}
impl DockerClient {
    pub async fn connect(socket: &str) -> Result<Self, DockerError> {
        let client = Self {
            api: Docker::connect_with_unix(socket, 60, API_DEFAULT_VERSION)
                .map_err(|_| DockerError::Unavailable)?,
            owner: Owner::new(16)?,
        };
        let api = client
            .request(|api| async move { api.negotiate_version().await })
            .await?;
        Ok(Self {
            api,
            owner: client.owner,
        })
    }
    async fn request<T, F, Fut>(&self, operation: F) -> Result<T, DockerError>
    where
        T: Send + 'static,
        F: FnOnce(Docker) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, bollard::errors::Error>> + Send + 'static,
    {
        let api = self.api.clone();
        self.owner
            .submit_async(move |_| async move {
                operation(api).await.map_err(|_| DockerError::Unavailable)
            })?
            .wait()
            .await?
    }
    pub async fn shutdown(&self, grace: Option<Duration>) {
        self.owner.shutdown(grace).await;
    }
    pub async fn ping(&self) -> Result<(), DockerError> {
        self.request(|api| async move { api.ping().await.map(|_| ()) })
            .await
    }
    pub async fn identity(&self, name: &str) -> Result<String, DockerError> {
        let name = name.to_owned();
        let found = self
            .request(move |api| async move { api.inspect_container(&name, None).await })
            .await?;
        found
            .id
            .filter(|id| !id.is_empty())
            .ok_or(DockerError::InvalidResponse)
    }
    pub async fn state(&self, id: &str) -> Result<String, DockerError> {
        let id = id.to_owned();
        let found = self
            .request(move |api| async move { api.inspect_container(&id, None).await })
            .await?;
        found
            .state
            .and_then(|state| state.status)
            .map(|state| state.to_string())
            .ok_or(DockerError::InvalidResponse)
    }
    pub async fn stop(&self, id: &str) -> Result<(), DockerError> {
        let id = id.to_owned();
        self.request(move |api| async move {
            api.stop_container(
                &id,
                Some(StopContainerOptions {
                    t: Some(30),
                    ..Default::default()
                }),
            )
            .await
        })
        .await
    }
    pub async fn start(&self, id: &str) -> Result<(), DockerError> {
        let id = id.to_owned();
        self.request(move |api| async move { api.start_container(&id, None).await })
            .await
    }
}
