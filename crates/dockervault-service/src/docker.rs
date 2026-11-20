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
#[derive(serde::Serialize)]
pub struct VolumeInventory {
    name: String,
    driver: String,
    mountpoint: String,
    labels: std::collections::HashMap<String, String>,
    created_at: bollard::models::BollardDate,
    used_by: Vec<String>,
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
    async fn volumes(&self) -> Result<Vec<bollard::models::Volume>, DockerError> {
        Ok(self
            .request(|api| async move {
                api.list_volumes(None::<bollard::query_parameters::ListVolumesOptions>)
                    .await
            })
            .await?
            .volumes
            .unwrap_or_default())
    }
    pub async fn volume_names(&self) -> Result<Vec<String>, DockerError> {
        let mut names: Vec<_> = self
            .volumes()
            .await?
            .into_iter()
            .map(|volume| volume.name)
            .collect();
        names.sort();
        Ok(names)
    }
    pub async fn volume_inventory(&self) -> Result<Vec<VolumeInventory>, DockerError> {
        use bollard::{errors::Error, query_parameters::ListContainersOptionsBuilder};
        let volumes = self.volumes().await?;
        let mut usage = self
            .request(|api| async move {
                let mut usage = std::collections::HashMap::<String, Vec<String>>::new();
                let containers = api
                    .list_containers(Some(
                        ListContainersOptionsBuilder::default().all(true).build(),
                    ))
                    .await?;
                for item in containers {
                    let Some(id) = item.id else { continue };
                    let container = match api.inspect_container(&id, None).await {
                        Ok(container) => container,
                        Err(Error::DockerResponseServerError {
                            status_code: 404, ..
                        }) => continue,
                        Err(error) => return Err(error),
                    };
                    let Some(name) = container.name else { continue };
                    let name = name.trim_start_matches('/');
                    for mount in container.mounts.unwrap_or_default() {
                        if mount.typ.as_deref() == Some("volume")
                            && let Some(volume) = mount.name.filter(|name| !name.is_empty())
                        {
                            usage.entry(volume).or_default().push(name.to_owned());
                        }
                    }
                }
                Ok(usage)
            })
            .await?;
        Ok(volumes
            .into_iter()
            .map(|volume| VolumeInventory {
                used_by: usage.remove(&volume.name).unwrap_or_default(),
                name: volume.name,
                driver: volume.driver,
                mountpoint: volume.mountpoint,
                labels: volume.labels,
                created_at: volume.created_at.unwrap_or_default(),
            })
            .collect())
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
