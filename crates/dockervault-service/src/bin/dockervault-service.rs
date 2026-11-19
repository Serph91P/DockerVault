//! Native service process. Packaging and the cross-runtime writer handoff are separate.
use dockervault_service::{
    auth::AuthService,
    config::Config,
    db::Database,
    docker::DockerClient,
    http::{self, HttpState},
    journal::Journal,
    ownership::Owner,
    recovery_activation::recovery_pending,
    storage_recovery::settle_interrupted,
    workload_recovery::Recovery,
};
use nix::fcntl::{Flock, FlockArg};
use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    net::SocketAddr,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    process::ExitCode,
};
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(4)
        .max_blocking_threads(16)
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return ExitCode::FAILURE,
    };
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(stage) => {
            eprintln!("DockerVault native startup or shutdown failed: {stage}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), &'static str> {
    let mut args = std::env::args().skip(1);
    let address: SocketAddr = args
        .next()
        .ok_or("expected listener address")?
        .parse()
        .map_err(|_| "invalid listener address")?;
    if args.next().is_some() {
        return Err("unexpected argument");
    }
    let config = Config::load().map_err(|_| "configuration")?;
    let path = config
        .database_url
        .strip_prefix("sqlite+aiosqlite:///")
        .or_else(|| config.database_url.strip_prefix("sqlite:///"))
        .filter(|path| !path.is_empty() && *path != ":memory:")
        .ok_or("database URL")?;
    let path = std::path::absolute(path).map_err(|_| "database path")?;
    let name = path.file_name().ok_or("database filename")?;
    let parent = path.parent().ok_or("database parent")?;
    fs::create_dir_all(parent).map_err(|_| "database parent")?;
    let parent = parent.canonicalize().map_err(|_| "database parent")?;
    let database_path = parent.join(name);
    // SQLite sidecars and the writer lock require one name in a trusted state directory.
    match fs::symlink_metadata(&database_path) {
        Ok(meta) if meta.is_file() && meta.nlink() == 1 => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        _ => return Err("unsafe database file identity"),
    }
    let mut lock = database_path.clone().into_os_string();
    lock.push(".native-writer.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .mode(0o600)
        .open(PathBuf::from(lock))
        .map_err(|_| "native writer lock")?;
    if !lock.metadata().map_err(|_| "native writer lock")?.is_file() {
        return Err("native writer lock");
    }
    let _writer = Flock::lock(lock, FlockArg::LockExclusiveNonblock)
        .map_err(|_| "another native writer is active")?;
    let mut terminate = signal(SignalKind::terminate()).map_err(|_| "SIGTERM handler")?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(|_| "SIGINT handler")?;
    let owner = Owner::new(1).map_err(|_| "startup owner")?;
    let db = Database::open(&database_path, parent.join("migration-backups"), &owner)
        .await
        .map_err(|_| "database migration")?;
    settle_interrupted(db.pool())
        .await
        .map_err(|_| "storage recovery")?;
    let journal = Journal::new(db.pool().clone()).map_err(|_| "workload journal")?;
    let grace = config.shutdown_limit();
    let activation_pending = recovery_pending(db.pool())
        .await
        .map_err(|_| "recovery activation")?;
    if !journal
        .unfinished_after(0)
        .await
        .map_err(|_| "workload journal")?
        .is_empty()
    {
        let docker = DockerClient::connect(&config.docker_socket)
            .await
            .map_err(|_| "workload recovery requires Docker")?;
        let recovery = Recovery::new(journal.clone(), docker).map_err(|_| "workload recovery")?;
        recovery
            .reconcile()
            .await
            .map_err(|_| "workload recovery")?;
        recovery.shutdown(grace).await;
    }
    // Preserve staged activation and any specific result from journal reconciliation.
    if !activation_pending {
        dockervault_service::storage::reconcile_backup_storage(
            db.pool(),
            &config.backup_base_path,
            &owner,
        )
        .await
        .map_err(|_| "archive reservation recovery")?;
        sqlx::query(
        "UPDATE backups SET status='FAILED', error_message=?,
         completed_at=strftime('%Y-%m-%d %H:%M:%f000','now')
         WHERE status IN ('PENDING','RUNNING')",
    )
    .bind("Interrupted by restart; accepted job terminally failed. Legacy workload state may be unknown: verify workloads manually and retry.")
    .execute(db.pool())
    .await
    .map_err(|_| "interrupted backup recovery")?;
    }
    let listener = TcpListener::bind(address).await.map_err(|_| "listener")?;
    let auth = AuthService::new(db.pool().clone());
    let state = HttpState::new(auth.clone(), config);
    http::recover_s3_copies(&state).await.map_err(|error| {
        tracing::error!(%error, "S3 sync journal recovery failed");
        "storage recovery"
    })?;
    let router = http::router(state.clone());
    let shutdown = state.clone();
    let result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! { _ = terminate.recv() => {}, _ = interrupt.recv() => {} }
        shutdown.begin_shutdown();
    })
    .await;
    let storage = state.settle_storage().await;
    auth.shutdown(grace).await;
    journal.shutdown(grace).await;
    owner.shutdown(grace).await;
    db.close().await;
    storage.map_err(|_| "storage settlement")?;
    result.map_err(|_| "serving HTTP")
}
