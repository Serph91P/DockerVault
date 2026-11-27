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
        Self::connect_with_timeout(socket, 60).await
    }
    pub(crate) async fn connect_with_timeout(
        socket: &str,
        timeout: u64,
    ) -> Result<Self, DockerError> {
        let client = Self {
            api: Docker::connect_with_unix(socket, timeout, API_DEFAULT_VERSION)
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
        let volumes = self.volumes().await?;
        let mut usage = std::collections::HashMap::<String, Vec<String>>::new();
        for container in self.containers(true).await? {
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
    async fn containers(
        &self,
        all: bool,
    ) -> Result<Vec<bollard::models::ContainerInspectResponse>, DockerError> {
        use bollard::{errors::Error, query_parameters::ListContainersOptionsBuilder};
        self.request(move |api| async move {
            let mut result = vec![];
            let containers = api
                .list_containers(Some(
                    ListContainersOptionsBuilder::default().all(all).build(),
                ))
                .await?;
            for item in containers {
                let Some(id) = item.id else { continue };
                match api.inspect_container(&id, None).await {
                    Ok(container) => result.push(container),
                    Err(Error::DockerResponseServerError {
                        status_code: 404, ..
                    }) => continue,
                    Err(error) => return Err(error),
                }
            }
            Ok(result)
        })
        .await
    }
    pub async fn container_inventory(
        &self,
        all: bool,
    ) -> Result<Vec<serde_json::Value>, DockerError> {
        use serde_json::json;
        self.containers(all).await?.into_iter().map(|container| {
            let id = container.id.ok_or(DockerError::InvalidResponse)?;
            let name = container.name.as_deref().unwrap_or_default().trim_start_matches('/');
            let status = container.state.and_then(|state| state.status).map(|status| status.to_string()).unwrap_or_default();
            let config = container.config.unwrap_or_default();
            let labels = config.labels.unwrap_or_default();
            let depends_on: Vec<_> = labels.get("backup.depends_on").into_iter().flat_map(|value| value.split(',')).map(str::trim).filter(|value| !value.is_empty()).collect();
            let mut networks: Vec<_> = container.network_settings.and_then(|value| value.networks).unwrap_or_default().into_keys().collect();
            networks.sort();
            let mounts: Vec<_> = container.mounts.unwrap_or_default().into_iter().map(|mount| json!({"type":mount.typ,"source":mount.source,"destination":mount.destination,"name":mount.name,"mode":mount.mode,"rw":mount.rw})).collect();
            Ok(json!({"id":id,"name":name,"image":config.image.unwrap_or_default(),"status":status,"state":status,
                "created":container.created.unwrap_or_default(),"labels":labels,"mounts":mounts,"networks":networks,
                "compose_project":labels.get("com.docker.compose.project"),"compose_service":labels.get("com.docker.compose.service"),"depends_on":depends_on}))
        }).collect()
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
        self.stop_with_timeout(id, 30).await
    }
    pub(crate) async fn stop_with_timeout(
        &self,
        id: &str,
        timeout: i32,
    ) -> Result<(), DockerError> {
        let id = id.to_owned();
        self.request(move |api| async move {
            api.stop_container(
                &id,
                Some(StopContainerOptions {
                    t: Some(timeout),
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
