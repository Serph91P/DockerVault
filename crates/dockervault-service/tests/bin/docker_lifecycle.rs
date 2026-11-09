//! Explicit native Docker smoke runner; creates/removes only its own returned container ID.
use bollard::{
    API_DEFAULT_VERSION, Docker,
    models::{ContainerCreateBody, HostConfig},
    query_parameters::RemoveContainerOptions,
};
use dockervault_service::docker::DockerClient;
use std::{collections::HashMap, error::Error};
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let socket = std::env::args()
        .nth(1)
        .ok_or("usage: docker_lifecycle SOCKET")?;
    let api = Docker::connect_with_unix(&socket, 60, API_DEFAULT_VERSION)?
        .negotiate_version()
        .await?;
    let created = api
        .create_container(
            None,
            ContainerCreateBody {
                image: Some("dockervault-248:rust-dev-current".into()),
                user: Some("1000:1000".into()),
                cmd: Some(vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "trap 'exit 0' TERM; while :; do sleep 1 & wait $!; done".into(),
                ]),
                labels: Some(HashMap::from([
                    (
                        "hermes.task".into(),
                        std::env::var("HERMES_KANBAN_TASK")
                            .unwrap_or_else(|_| "native-lifecycle".into()),
                    ),
                    ("dockervault.test".into(), "native-lifecycle".into()),
                ])),
                host_config: Some(HostConfig {
                    network_mode: Some("none".into()),
                    readonly_rootfs: Some(true),
                    cap_drop: Some(vec!["ALL".into()]),
                    memory: Some(33554432),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await?;
    let id = created.id;
    let result: Result<(), Box<dyn Error>> = async {
        api.start_container(&id, None).await?;
        let managed = DockerClient::connect(&socket).await?;
        let result: Result<(), Box<dyn Error>> = async {
            if managed.identity(&id).await? != id || managed.state(&id).await? != "running" {
                return Err("initial immutable identity/running state mismatch".into());
            }
            managed.stop(&id).await?;
            if managed.state(&id).await? != "exited" {
                return Err("stop did not physically exit".into());
            }
            managed.start(&id).await?;
            if managed.state(&id).await? != "running" {
                return Err("start did not reach running".into());
            }
            managed.stop(&id).await?;
            if managed.state(&id).await? != "exited" {
                return Err("final stop did not physically exit".into());
            }
            Ok(())
        }
        .await;
        managed.shutdown(None).await;
        result
    }
    .await;
    api.remove_container(
        &id,
        Some(RemoveContainerOptions {
            force: true,
            v: true,
            ..Default::default()
        }),
    )
    .await?;
    assert!(
        api.inspect_container(&id, None).await.is_err(),
        "owned container must be physically absent"
    );
    result?;
    println!(
        "PASS: native Docker negotiate -> immutable ID -> stop/exited -> start/running -> stop/exited -> owned removal"
    );
    Ok(())
}
