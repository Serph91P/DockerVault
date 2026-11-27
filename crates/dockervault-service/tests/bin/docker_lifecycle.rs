//! Explicit native Docker smoke runner; creates/removes only its own returned container ID.
use bollard::{
    API_DEFAULT_VERSION, Docker,
    models::{ContainerCreateBody, HostConfig},
    query_parameters::RemoveContainerOptions,
};
use dockervault_service::{
    db::Database,
    docker::DockerClient,
    journal::{Journal, Kind},
    ownership::Owner,
    workload_recovery::Recovery,
};
use std::{collections::HashMap, error::Error, path::Path, time::Duration};
async fn child(
    mode: &str,
    socket: &str,
    root: &Path,
    id: &str,
    operation: i64,
    marker: &Path,
) -> Result<(), Box<dyn Error>> {
    let owner = Owner::new(1)?;
    let db = Database::open(root.join("state.sqlite"), root.join("backups"), &owner).await?;
    let journal = Journal::new(db.pool().clone())?;
    let docker = DockerClient::connect(socket).await?;
    if mode == "--crash-unconfirmed" {
        journal.stop(operation, id, false).await?;
        docker.stop(id).await?;
        let mut file = std::fs::File::create(marker)?;
        std::io::Write::write_all(&mut file, b"actual-stop-finished-before-confirm")?;
        file.sync_all()?;
        std::future::pending::<()>().await;
    } else {
        let recovery = Recovery::new(journal.clone(), docker.clone())?;
        recovery.reconcile().await?;
        recovery.shutdown(None).await;
    }
    docker.shutdown(None).await;
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
    Ok(())
}
async fn process_loss(socket: &str, id: &str) -> Result<(), Box<dyn Error>> {
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|_| "nonce unavailable")?;
    let root = std::env::temp_dir().join(format!(
        "dv-journal-crash-{:032x}",
        u128::from_be_bytes(nonce)
    ));
    std::fs::create_dir(&root)?;
    let result: Result<(), Box<dyn Error>> = async {
        let owner = Owner::new(1)?;
        let db = Database::open(root.join("state.sqlite"), root.join("backups"), &owner).await?;
        let journal = Journal::new(db.pool().clone())?;
        let operation = journal
            .begin(
                None,
                Kind::Backup,
                [(id.to_owned(), Some("running".into()))]
                    .into_iter()
                    .collect(),
                None,
            )
            .await?;
        journal.shutdown(None).await;
        db.close().await;
        owner.shutdown(None).await;
        let marker = root.join("stopped.marker");
        let exe = std::env::current_exe()?;
        let op = operation.to_string();
        let args = [
            socket,
            root.to_str().ok_or("non UTF8 test directory")?,
            id,
            &op,
            marker.to_str().ok_or("non UTF8 marker")?,
        ];
        let mut stopped = tokio::process::Command::new(&exe)
            .arg("--crash-unconfirmed")
            .args(args)
            .kill_on_drop(true)
            .spawn()?;
        let reached = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if marker.exists() {
                    return Ok::<_, Box<dyn Error>>(());
                }
                if stopped.try_wait()?.is_some() {
                    return Err("crash child exited before its actual stop".into());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if reached.is_err() {
            stopped.kill().await?;
            return Err("actual stop marker timed out".into());
        }
        reached??;
        stopped.kill().await?;
        let status = tokio::process::Command::new(&exe)
            .arg("--reconcile")
            .args(args)
            .kill_on_drop(true)
            .status()
            .await?;
        if !status.success() {
            return Err("fresh reconciliation process failed".into());
        }
        let reader = Database::read_only(root.join("state.sqlite")).await?;
        let journal = Journal::new(reader.pool().clone())?;
        let saved = journal.load(operation).await?;
        let valid = saved.state == "finished"
            && saved
                .resources
                .get(id)
                .is_some_and(|resource| resource.stopped && resource.restored);
        journal.shutdown(None).await;
        reader.close().await;
        if !valid {
            return Err("fresh process lost workload acknowledgment".into());
        }
        Ok(())
    }
    .await;
    std::fs::remove_dir_all(root)?;
    result
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments = std::env::args().collect::<Vec<_>>();
    if matches!(
        arguments.get(1).map(String::as_str),
        Some("--crash-unconfirmed" | "--reconcile")
    ) {
        if arguments.len() != 7 {
            return Err("invalid child test arguments".into());
        }
        return child(
            &arguments[1],
            &arguments[2],
            Path::new(&arguments[3]),
            &arguments[4],
            arguments[5].parse()?,
            Path::new(&arguments[6]),
        )
        .await;
    }
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
            process_loss(&socket, &id).await?;
            if managed.state(&id).await? != "running" {
                return Err("fresh process failed to restart original immutable ID".into());
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
        "PASS: native Docker lifecycle -> real stop with unconfirmed intent -> SIGKILL/reap -> fresh process journal reconciliation/running -> stop/exited -> owned removal"
    );
    Ok(())
}
