use super::*;
use crate::{process::ProcessSpec, rclone::ManagedConfigBuilder};
use std::{path::Path, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub(super) async fn probe(state: &HttpState, config: &Value) -> Option<()> {
    let remote = remote_name(config)?;
    let target = format!("{remote}:");
    run(state, config, remote, ["lsd", "--", &target], false, None).await?;
    Some(())
}

pub(super) async fn list(state: &HttpState, config: &Value, child: &str) -> Option<Vec<Value>> {
    let remote = remote_name(config)?;
    let base = config["base_path"].as_str().unwrap_or_default();
    let path = format!("{base}/{child}").replace("//", "/");
    let target = format!("{remote}:{path}");
    let output = run(state, config, remote, ["lsjson", "--", &target], true, None).await?;
    let items: Vec<Value> = serde_json::from_slice(&output).ok()?;
    items.into_iter().map(|item| {
        let modified = item["ModTime"].as_str().and_then(|value| {
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
        }).map_or(0.0, |value| value.unix_timestamp() as f64 + f64::from(value.microsecond()) / 1_000_000.0);
        Some(json!({"name":item["Name"].as_str()?,"size":item["Size"].as_i64().unwrap_or(0),"is_dir":item["IsDir"].as_bool()?,"modified":modified}))
    }).collect()
}

fn remote_name(config: &Value) -> Option<&str> {
    config["rclone_remote"]
        .as_str()
        .filter(|name| dockervault_core::rclone_path::valid_name(name))
}

pub(super) async fn download_snapshot<T: Send + 'static>(
    state: &HttpState,
    config: Value,
    child: String,
    slot: T,
) -> Option<(std::fs::File, u64, T)> {
    use std::os::fd::AsRawFd;
    let shared = state.clone();
    state
        .download_jobs
        .submit_async(move |cancel| async move {
            let remote = remote_name(&config)?;
            let base = config["base_path"].as_str().unwrap_or_default();
            let path = format!("{base}/{child}").replace("//", "/");
            let source = format!("{remote}:{path}");
            let file = tokio::task::spawn_blocking(crate::storage::local::anonymous_download)
                .await
                .ok()?
                .ok()?;
            // Follow the local destination descriptor, not links on the source.
            let destination = format!(
                ":local,copy_links=true:/proc/{}/fd/{}",
                std::process::id(),
                file.as_raw_fd()
            );
            run(
                &shared,
                &config,
                remote,
                [
                    "copyto",
                    "--inplace",
                    "--ignore-times",
                    "--",
                    &source,
                    &destination,
                ],
                false,
                Some(&cancel),
            )
            .await?;
            tokio::task::spawn_blocking(move || {
                let length = file.metadata().ok()?.len();
                Some((file, length, slot))
            })
            .await
            .ok()?
        })
        .ok()?
        .wait()
        .await
        .ok()?
}

pub(super) async fn settled<T>(
    work: crate::ownership::Work<T>,
    cancel: Option<&CancellationToken>,
) -> Option<T> {
    let Some(cancel) = cancel else {
        return work.wait().await.ok();
    };
    let child = work.cancellation_token();
    let mut reply = std::pin::pin!(work.wait());
    tokio::select! { biased;
        _ = cancel.cancelled() => { child.cancel(); let _ = reply.await; None }
        result = &mut reply => result.ok(),
    }
}

async fn run<const N: usize>(
    state: &HttpState,
    config: &Value,
    remote: &str,
    args: [&str; N],
    capture: bool,
    cancel: Option<&CancellationToken>,
) -> Option<Vec<u8>> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return None;
    }
    let processes = state.probe_processes.as_ref()?;
    let timeout = Some(Duration::from_secs(360));
    if let Some(id) = config["id"].as_i64() {
        let row = sqlx::query_scalar::<_, Option<String>>(
            "SELECT value FROM app_settings WHERE key='recovery_rclone'",
        )
        .fetch_optional(state.auth.pool())
        .await
        .ok()?
        .flatten();
        if let Some(raw) = row.filter(|raw| !raw.is_empty()) {
            let generation: Value = serde_json::from_str(&raw).ok()?;
            if let Some(root) = generation["roots"]
                .as_array()?
                .iter()
                .find(|root| root["storage_id"].as_i64() == Some(id))
            {
                if root["remote_name"].as_str() != Some(remote) {
                    return None;
                }
                let io = state.auth.blocking();
                let cfg = state.config.clone();
                let cipher = io
                    .submit_blocking(move |cancel| {
                        crate::storage_recovery::load_delete_cipher(
                            Path::new("/app/data/.credential_key"),
                            cfg.credential_encryption_key.expose(),
                            || cancel.is_cancelled(),
                        )
                    })
                    .ok()?
                    .wait()
                    .await
                    .ok()?
                    .ok()?;
                let dirs = state
                    .config
                    .allowed_ssh_key_dirs
                    .split(',')
                    .map(str::trim)
                    .filter(|dir| !dir.is_empty())
                    .map(Into::into)
                    .collect();
                let runner = ManagedConfigBuilder::new(
                    processes.clone(),
                    io.clone(),
                    state.probe_jobs.clone(),
                    "/usr/local/bin/rclone".into(),
                );
                let work = runner
                    .start_run_managed(
                        generation,
                        Arc::new(cipher),
                        remote.to_owned(),
                        args.into_iter().map(Into::into).collect(),
                        timeout,
                        dirs,
                    )
                    .ok()?;
                let output = settled(work, cancel).await?.ok()?;
                return (output.code == 0).then(|| output.stdout.into_bytes());
            }
        }
    }
    let mut spec = ProcessSpec::new(
        "/usr/local/bin/rclone",
        [
            "--config",
            state.config.rclone_config.as_str(),
            "--ask-password=false",
            "--timeout",
            "300s",
            "--contimeout",
            "30s",
        ]
        .into_iter()
        .chain(args),
    );
    spec.timeout = timeout;
    spec.discard_output = !capture;
    if capture {
        spec.stdout_limit = 8 * 1024 * 1024;
    }
    if !state.config.rclone_config_pass.expose().is_empty() {
        spec.env.push((
            "RCLONE_CONFIG_PASS".into(),
            state.config.rclone_config_pass.expose().into(),
        ));
    }
    let output = settled(processes.start(spec).ok()?, cancel).await?.ok()?;
    output.status.success().then_some(output.stdout)
}
