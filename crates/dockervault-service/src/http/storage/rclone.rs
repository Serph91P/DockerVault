use super::*;
use crate::{process::ProcessSpec, rclone::ManagedConfigBuilder};
use std::{path::Path, sync::Arc, time::Duration};

pub(super) async fn probe(state: &HttpState, config: &Value) -> Option<()> {
    let remote = config["rclone_remote"]
        .as_str()
        .filter(|name| dockervault_core::rclone_path::valid_name(name))?;
    let processes = state.probe_processes.as_ref()?;
    let target = format!("{remote}:");
    let args = ["lsd", "--", &target];
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
                let output = runner
                    .start_run_managed(
                        generation,
                        Arc::new(cipher),
                        remote.to_owned(),
                        args.into_iter().map(Into::into).collect(),
                        timeout,
                        dirs,
                    )
                    .ok()?
                    .wait()
                    .await
                    .ok()?
                    .ok()?;
                return (output.code == 0).then_some(());
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
    spec.discard_output = true;
    if !state.config.rclone_config_pass.expose().is_empty() {
        spec.env.push((
            "RCLONE_CONFIG_PASS".into(),
            state.config.rclone_config_pass.expose().into(),
        ));
    }
    processes
        .start(spec)
        .ok()?
        .wait()
        .await
        .ok()?
        .ok()?
        .status
        .success()
        .then_some(())
}
