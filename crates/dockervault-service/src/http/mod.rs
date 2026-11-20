//! HTTP contracts; protocol state is shared, never retained in request-owned jobs.
mod auth;
mod backup_metrics;
mod backup_restore;
mod backup_submit;
mod backup_validate;
mod backups;
mod copy_removal;
mod docker_inventory;
mod docker_lifecycle;
mod encryption;
pub use copy_removal::recover_copy_removals;
mod ftp_sync;
mod local_browser_delete;
mod manual_sync;
mod operations;
mod overview;
mod recovery_export;
mod recovery_preview;
mod recovery_restore;
mod recovery_status;
mod retention;
mod s3_sync;
mod schedules;
pub mod security;
mod settings;
mod sftp_sync;
mod ssh_keys;
pub use ssh_keys::recover_key_change;
mod storage;
mod target_write;
mod targets;
pub use s3_sync::recover_s3_copies;
mod webdav_sync;
pub mod ws;
use crate::{auth::AuthService, config::Config, storage_recovery::LocalDeleteJobs};
use axum::{
    Json, Router, middleware,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::json;
use std::sync::Arc;
#[derive(Clone)]
pub struct HttpState {
    pub auth: AuthService,
    pub config: Arc<Config>,
    started: std::time::Instant,
    limits: Arc<security::RateLimits>,
    pub notifications: Arc<ws::Notifications>,
    draining: tokio_util::sync::CancellationToken,
    local_delete: Arc<LocalDeleteJobs>,
    probe_processes: Option<crate::process::ProcessOwner>,
    probe_jobs: crate::ownership::Owner,
    download_slots: Arc<tokio::sync::Semaphore>,
    download_jobs: crate::ownership::Owner,
    backup_jobs: crate::ownership::Owner,
    scheduler: Arc<schedules::LiveScheduler>,
    backup_metrics: Arc<std::sync::Mutex<backup_metrics::Metrics>>,
    live_backups: Arc<std::sync::Mutex<std::collections::HashMap<i64, &'static str>>>,
    removal_previews: Arc<std::sync::Mutex<copy_removal::Previews>>,
    backup_claims: crate::jobs::Claims,
    ssh_key_write: Arc<tokio::sync::Mutex<()>>,
}
impl HttpState {
    pub fn new(auth: AuthService, config: Config) -> Self {
        Self {
            auth,
            config: Arc::new(config),
            started: std::time::Instant::now(),
            limits: Arc::new(security::RateLimits::default()),
            notifications: Arc::new(ws::Notifications::default()),
            draining: tokio_util::sync::CancellationToken::new(),
            local_delete: Arc::new(LocalDeleteJobs::new()),
            probe_processes: crate::process::ProcessOwner::new(
                4,
                "/usr/local/libexec/dockervault-process-guard",
            )
            .ok(),
            probe_jobs: crate::ownership::Owner::new(4).expect("positive probe capacity"),
            download_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            download_jobs: crate::ownership::Owner::new(4).expect("positive download capacity"),
            backup_jobs: crate::ownership::Owner::new(4).expect("positive backup capacity"),
            scheduler: Arc::default(),
            backup_metrics: Arc::default(),
            live_backups: Arc::default(),
            removal_previews: Arc::default(),
            backup_claims: crate::jobs::Claims::default(),
            ssh_key_write: Arc::default(),
        }
    }

    pub fn begin_shutdown(&self) {
        self.draining.cancel();
    }

    pub async fn start_scheduler(&self) {
        schedules::start(self).await;
    }

    pub async fn settle_storage(
        &self,
    ) -> Result<(), crate::storage_recovery::StorageRecoveryError> {
        self.begin_shutdown();
        schedules::stop(self).await;
        self.backup_jobs
            .shutdown(self.config.shutdown_limit())
            .await;
        self.local_delete.shutdown().await;
        self.download_jobs.shutdown(None).await;
        self.probe_jobs.shutdown(None).await;
        if let Some(processes) = &self.probe_processes {
            processes.shutdown(None).await;
        }
        crate::storage_recovery::settle_interrupted(self.auth.pool()).await?;
        Ok(())
    }
}
pub fn router(state: HttpState) -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"status":"healthy"})) }),
        )
        .route("/api/v1/auth/status", get(auth::status))
        .route("/api/v1/auth/setup", post(auth::setup))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/me", get(auth::me))
        .route("/api/v1/recovery/preview", post(recovery_preview::preview))
        .route("/api/v1/recovery/restore", post(recovery_restore::restore))
        .route("/api/v1/recovery/export", post(recovery_export::export))
        .route("/api/v1/overview/summary", get(overview::summary))
        .route("/api/v1/docker/health", get(docker_inventory::health))
        .route(
            "/api/v1/docker/containers/{id}/start",
            post(docker_lifecycle::start),
        )
        .route(
            "/api/v1/docker/containers/{id}/stop",
            post(docker_lifecycle::stop),
        )
        .route("/api/v1/docker/volumes", get(docker_inventory::volumes))
        .route("/api/v1/docker/stacks", get(docker_inventory::stacks))
        .route(
            "/api/v1/docker/stacks/{name}/dependencies",
            get(docker_inventory::stack_dependencies),
        )
        .route(
            "/api/v1/docker/containers",
            get(docker_inventory::containers),
        )
        .route(
            "/api/v1/docker/containers/{id}",
            get(docker_inventory::container),
        )
        .route("/api/v1/encryption/status", get(encryption::status))
        .route("/api/v1/encryption/defer", post(encryption::defer))
        .route("/api/v1/encryption/setup", post(encryption::setup))
        .route(
            "/api/v1/encryption/regenerate",
            post(encryption::regenerate),
        )
        .route(
            "/api/v1/encryption/recovery-instructions",
            get(encryption::instructions),
        )
        .route(
            "/api/v1/encryption/confirm-setup",
            post(encryption::confirm),
        )
        .route("/api/v1/encryption/toggle", post(encryption::toggle))
        .route("/api/v1/auth/change-password", post(auth::change_password))
        .route(
            "/api/v1/retention",
            get(retention::list).post(retention::create),
        )
        .route(
            "/api/v1/retention/{id}",
            get(retention::detail)
                .put(retention::update)
                .delete(retention::delete),
        )
        .route("/api/v1/retention/{id}/stats", get(retention::stats))
        .route(
            "/api/v1/retention/cleanup-orphaned",
            post(retention::cleanup_orphaned).get(retention::cleanup_orphaned),
        )
        .route("/api/v1/settings/system-info", get(settings::system_info))
        .route(
            "/api/v1/settings/storage",
            get(settings::storage).put(settings::update_storage),
        )
        .route(
            "/api/v1/targets",
            get(targets::list).post(target_write::create),
        )
        .route(
            "/api/v1/targets/{id}",
            get(targets::detail)
                .put(target_write::update)
                .delete(targets::delete),
        )
        .route("/api/v1/retention/{id}/apply", post(retention::apply))
        .route(
            "/api/v1/schedules",
            get(schedules::list).post(schedules::create),
        )
        .route(
            "/api/v1/schedules/{id}",
            get(schedules::detail)
                .put(schedules::update)
                .delete(schedules::delete),
        )
        .route("/api/v1/schedules/preview", post(schedules::preview))
        .route("/api/v1/schedules/estimate", post(schedules::estimate))
        .route(
            "/api/v1/schedules/compatibility/inventory",
            get(schedules::inventory),
        )
        .route("/api/v1/schedules/legacy/by-target", get(schedules::legacy))
        .route("/api/v1/schedules/cron-help", get(schedules::help))
        .route("/api/v1/schedules/jobs", get(schedules::jobs))
        .route("/api/v1/schedules/reload", post(schedules::reload))
        .route(
            "/api/v1/schedules/target/{id}/trigger",
            post(schedules::trigger_target),
        )
        .route(
            "/api/v1/schedules/{id}/trigger-all",
            post(schedules::trigger_all),
        )
        .route(
            "/api/v1/backups",
            get(backups::list)
                .post(backup_submit::submit)
                .delete(backups::reject_unconfirmed_bulk_delete),
        )
        .route("/api/v1/backups/history", get(backups::history))
        .route("/api/v1/backups/{id}/retry-sync", post(s3_sync::retry))
        .route("/api/v1/backups/history/{id}", get(backups::history_detail))
        .route(
            "/api/v1/backups/{id}",
            get(backups::get).delete(copy_removal::legacy_delete),
        )
        .route("/api/v1/backups/{id}/logs", get(backups::logs))
        .route(
            "/api/v1/backups/{id}/validate",
            post(backup_validate::validate),
        )
        .route(
            "/api/v1/backups/removal/preview",
            post(copy_removal::preview),
        )
        .route(
            "/api/v1/backups/removal/execute",
            post(copy_removal::execute),
        )
        .route(
            "/api/v1/backups/{id}/files",
            get(backups::files).post(backups::encrypted_files),
        )
        .route(
            "/api/v1/backups/{id}/files/{*name}",
            get(backups::file_member).post(backups::encrypted_member),
        )
        .route("/api/v1/backups/{id}/stats", get(backups::stats))
        .route(
            "/api/v1/backups/metrics/summary",
            get(backup_metrics::summary),
        )
        .route(
            "/api/v1/backups/metrics/target/{id}",
            get(backup_metrics::target),
        )
        .route("/api/v1/backups/{id}/download", get(backups::download))
        .route(
            "/api/v1/backups/{id}/restore-review",
            post(backup_restore::review),
        )
        .route(
            "/api/v1/backups/{id}/restore",
            post(backup_restore::restore),
        )
        .route(
            "/api/v1/backups/{id}/restore-info",
            get(backups::restore_info),
        )
        .route(
            "/api/v1/operations",
            get(operations::list).post(operations::submit),
        )
        .route("/api/v1/operations/{id}", get(operations::detail))
        .route("/api/v1/operations/{id}/cancel", post(operations::cancel))
        .route("/api/v1/operations/{id}/retry", post(operations::retry))
        .route("/api/v1/recovery/status", get(recovery_status::status))
        .route("/api/v1/storage/types", get(storage::types))
        .route(
            "/api/v1/storage/ssh-keys",
            get(ssh_keys::list).post(ssh_keys::generate),
        )
        .route(
            "/api/v1/storage/ssh-keys/{name}",
            get(ssh_keys::detail).delete(ssh_keys::delete),
        )
        .route(
            "/api/v1/storage/ssh-keys/{name}/public",
            get(ssh_keys::download_public),
        )
        .route(
            "/api/v1/storage/ssh-keys/{name}/install",
            post(ssh_keys::install),
        )
        .route(
            "/api/v1/storage/local/files",
            get(storage::list_local_files).delete(local_browser_delete::single),
        )
        .route(
            "/api/v1/storage/local/files/bulk-delete",
            post(local_browser_delete::bulk),
        )
        .route("/api/v1/storage/local/files/size", get(storage::local_size))
        .route(
            "/api/v1/storage/local/files/download",
            get(storage::local_download),
        )
        .route(
            "/api/v1/storage/local/files/bulk-download",
            post(storage::bulk_local_download),
        )
        .route(
            "/api/v1/storage/{storage_id}/files/bulk-download",
            post(storage::bulk_configured_download),
        )
        .route(
            "/api/v1/storage/{storage_id}/files",
            get(storage::list_configured_files).delete(local_browser_delete::single),
        )
        .route(
            "/api/v1/storage/{storage_id}/files/bulk-delete",
            post(local_browser_delete::bulk),
        )
        .route(
            "/api/v1/storage/{storage_id}/files/size",
            get(storage::configured_size),
        )
        .route(
            "/api/v1/storage/{storage_id}/files/download",
            get(storage::configured_download),
        )
        .route("/api/v1/storage/test-config", post(storage::test_config))
        .route(
            "/api/v1/storage/{storage_id}/sync/{backup_id}",
            post(manual_sync::sync),
        )
        .route(
            "/api/v1/storage/{storage_id}/test",
            post(storage::test_saved),
        )
        .route("/api/v1/storage", get(storage::list).post(storage::create))
        .route(
            "/api/v1/storage/{storage_id}",
            get(storage::detail)
                .put(storage::update)
                .delete(storage::delete),
        )
        .route("/ws/updates", get(ws::updates))
        .fallback(|| async {
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"detail":"Not Found"})),
            )
                .into_response()
        })
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::guard,
        ))
        .layer(middleware::from_fn(security::recovery_privacy))
        .with_state(state)
}
