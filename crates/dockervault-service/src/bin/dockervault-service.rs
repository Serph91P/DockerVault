//! Native service process. Packaging and the cross-runtime writer handoff are separate.
use dockervault_service::{
    auth::AuthService,
    config::Config,
    db::Database,
    docker::DockerClient,
    http::{self, HttpState},
    journal::Journal,
    ownership::Owner,
    storage_recovery::settle_interrupted,
    workload_recovery::Recovery,
};
use nix::fcntl::{Flock, FlockArg};
use std::{
    fs::OpenOptions, net::SocketAddr, os::unix::fs::OpenOptionsExt, path::PathBuf,
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
    let database_path = std::path::absolute(path).map_err(|_| "database path")?;
    let parent = database_path.parent().ok_or("database parent")?;
    std::fs::create_dir_all(parent).map_err(|_| "database parent")?;
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
    let listener = TcpListener::bind(address).await.map_err(|_| "listener")?;
    let auth = AuthService::new(db.pool().clone());
    let router = http::router(HttpState::new(auth.clone(), config));
    let result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! { _ = terminate.recv() => {}, _ = interrupt.recv() => {} }
    })
    .await;
    auth.shutdown(grace).await;
    journal.shutdown(grace).await;
    owner.shutdown(grace).await;
    db.close().await;
    result.map_err(|_| "serving HTTP")
}
