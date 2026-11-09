#![cfg(target_os = "linux")]
use dockervault_service::{
    db::Database,
    docker::DockerClient,
    journal::{Journal, Kind},
    ownership::Owner,
    workload_recovery::Recovery,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    sync::Notify,
};
use tokio_util::sync::CancellationToken;
static NEXT: AtomicU64 = AtomicU64::new(0);
#[derive(Default)]
struct State {
    calls: Mutex<Vec<String>>,
    status: Mutex<String>,
    statuses: Mutex<std::collections::HashMap<String, String>>,
    delay_stop: AtomicBool,
    entered: Notify,
    release: Notify,
}
struct Server {
    path: PathBuf,
    state: Arc<State>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Server {
    async fn start() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "dv-docker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("api.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let state = Arc::new(State::default());
        *state.status.lock().unwrap() = "running".into();
        let cancel = CancellationToken::new();
        let stopping = cancel.clone();
        let shared = state.clone();
        let task = tokio::spawn(async move {
            let mut workers = tokio::task::JoinSet::new();
            loop {
                tokio::select! {biased;
                    _=stopping.cancelled()=>break,
                    Some(result)=workers.join_next(),if !workers.is_empty()=>{result.unwrap();},
                    accepted=listener.accept()=>{
                        let (mut stream,_)=accepted.unwrap();let shared=shared.clone();let stopping=stopping.clone();
                        workers.spawn(async move {
                            let mut data=Vec::new();let mut chunk=[0u8;1024];
                            while !data.windows(4).any(|w|w==b"\r\n\r\n") {let n=stream.read(&mut chunk).await.unwrap();if n==0{return}data.extend_from_slice(&chunk[..n]);assert!(data.len()<16384);}
                            let line=std::str::from_utf8(&data).unwrap().lines().next().unwrap();shared.calls.lock().unwrap().push(line.to_owned());
                            let path=line.split_whitespace().nth(1).unwrap();
                            let id=path.split("/containers/").nth(1).and_then(|suffix|suffix.split('/').next()).unwrap_or("");
                            let (status,body)=if path.ends_with("/version") {(200,r#"{"ApiVersion":"1.53","MinAPIVersion":"1.44","Version":"PUBLIC"}"#.into())}
                            else if path.contains("/missing/") {(404,r#"{"message":"No such container"}"#.into())}
                            else if path.contains("/json") {
                                let status=shared.statuses.lock().unwrap().get(id).cloned().unwrap_or_else(||shared.status.lock().unwrap().clone());
                                (200,serde_json::json!({"Id":"public-immutable-id","State":{"Status":status}}).to_string())
                            }
                            else if path.contains("/stop?") {
                                shared.entered.notify_one();if shared.delay_stop.load(Ordering::SeqCst) {tokio::select!{_=shared.release.notified()=>{},_=stopping.cancelled()=>{}}}
                                *shared.status.lock().unwrap()="exited".into();shared.statuses.lock().unwrap().insert(id.into(),"exited".into());(204,String::new())
                            }else if path.ends_with("/start") {*shared.status.lock().unwrap()="running".into();shared.statuses.lock().unwrap().insert(id.into(),"running".into());(204,String::new())}
                            else {(500,r#"{"message":"unexpected test request"}"#.into())};
                            let response=format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());let _=stream.write_all(response.as_bytes()).await;
                        });
                    }
                }
            }
            while let Some(result) = workers.join_next().await {
                result.unwrap();
            }
        });
        Self {
            path,
            state,
            cancel,
            task: Some(task),
        }
    }
    async fn close(mut self) {
        self.cancel.cancel();
        self.task.take().unwrap().await.unwrap();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
        std::fs::remove_dir_all(self.path.parent().unwrap()).unwrap();
    }
}
async fn journal(server: &Server) -> (Database, Owner, Journal) {
    let owner = Owner::new(1).unwrap();
    let root = server.path.parent().unwrap();
    let db = Database::open(root.join("state.sqlite"), root.join("backups"), &owner)
        .await
        .unwrap();
    let journal = Journal::new(db.pool().clone()).unwrap();
    (db, owner, journal)
}
#[tokio::test]
async fn recovery_settles_unconfirmed_stop_then_restarts_once_and_never_replays_finished_work() {
    let server = Server::start().await;
    let client = DockerClient::connect(server.path.to_str().unwrap())
        .await
        .unwrap();
    let (db, owner, journal) = journal(&server).await;
    let id = journal
        .begin(
            None,
            Kind::Backup,
            [("public-immutable-id".into(), Some("running".into()))]
                .into_iter()
                .collect(),
            None,
        )
        .await
        .unwrap();
    journal
        .stop(id, "public-immutable-id", false)
        .await
        .unwrap();
    let recovery = Recovery::new(journal.clone(), client.clone()).unwrap();
    recovery.reconcile().await.unwrap();
    let row = journal.load(id).await.unwrap();
    assert_eq!(row.state, "finished");
    assert!(row.resources["public-immutable-id"].stopped);
    assert!(row.resources["public-immutable-id"].restored);
    let changes = server
        .state
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.starts_with("POST"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(changes.len(), 2);
    assert!(changes[0].contains("/stop?"));
    assert!(changes[1].contains("/start"));
    let count = server.state.calls.lock().unwrap().len();
    recovery.reconcile().await.unwrap();
    assert_eq!(server.state.calls.lock().unwrap().len(), count);
    recovery.shutdown(None).await;
    client.shutdown(None).await;
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
    server.close().await;
}
#[tokio::test]
async fn every_pending_stop_settles_before_explicit_dependency_restart_order() {
    let server = Server::start().await;
    server.state.statuses.lock().unwrap().extend([
        ("first".into(), "running".into()),
        ("second".into(), "running".into()),
    ]);
    let client = DockerClient::connect(server.path.to_str().unwrap())
        .await
        .unwrap();
    let (db, owner, journal) = journal(&server).await;
    let id = journal
        .begin(
            None,
            Kind::Backup,
            [
                ("first".into(), Some("running".into())),
                ("second".into(), Some("running".into())),
            ]
            .into_iter()
            .collect(),
            Some(vec!["second".into(), "first".into()]),
        )
        .await
        .unwrap();
    journal.stop(id, "first", false).await.unwrap();
    journal.stop(id, "second", false).await.unwrap();
    let recovery = Recovery::new(journal.clone(), client.clone()).unwrap();
    recovery.reconcile().await.unwrap();
    let actions = server
        .state
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.starts_with("POST"))
        .map(|line| line.split_whitespace().nth(1).unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        actions,
        [
            "/containers/first/stop?t=30",
            "/containers/second/stop?t=30",
            "/containers/second/start",
            "/containers/first/start",
        ]
    );
    recovery.shutdown(None).await;
    client.shutdown(None).await;
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
    server.close().await;
}
#[tokio::test]
async fn unknown_workload_is_not_guessed_or_finished_and_remains_manual_recovery() {
    let server = Server::start().await;
    let client = DockerClient::connect(server.path.to_str().unwrap())
        .await
        .unwrap();
    let (db, owner, journal) = journal(&server).await;
    let id = journal
        .begin(
            None,
            Kind::Restore,
            [("missing".into(), Some("running".into()))]
                .into_iter()
                .collect(),
            None,
        )
        .await
        .unwrap();
    journal.stop(id, "missing", false).await.unwrap();
    let recovery = Recovery::new(journal.clone(), client.clone()).unwrap();
    assert!(recovery.reconcile().await.is_err());
    let row = journal.load(id).await.unwrap();
    assert_eq!(row.state, "recovery_required");
    assert!(!row.resources["missing"].restored);
    assert!(
        !server
            .state
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("POST"))
    );
    recovery.shutdown(None).await;
    client.shutdown(None).await;
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
    server.close().await;
}
#[tokio::test]
async fn originally_exited_workload_is_not_inspected_or_changed_by_recovery() {
    let server = Server::start().await;
    let client = DockerClient::connect(server.path.to_str().unwrap())
        .await
        .unwrap();
    let (db, owner, journal) = journal(&server).await;
    let id = journal
        .begin(
            None,
            Kind::Backup,
            [("public-immutable-id".into(), Some("exited".into()))]
                .into_iter()
                .collect(),
            None,
        )
        .await
        .unwrap();
    let recovery = Recovery::new(journal.clone(), client.clone()).unwrap();
    let before = server.state.calls.lock().unwrap().len();
    recovery.reconcile().await.unwrap();
    assert_eq!(journal.load(id).await.unwrap().state, "finished");
    assert_eq!(server.state.calls.lock().unwrap().len(), before);
    recovery.shutdown(None).await;
    client.shutdown(None).await;
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
    server.close().await;
}
#[tokio::test]
async fn recovery_caller_abort_cannot_release_ownership_before_stop_restart_and_journal_settlement()
{
    let server = Server::start().await;
    server.state.delay_stop.store(true, Ordering::SeqCst);
    let client = DockerClient::connect(server.path.to_str().unwrap())
        .await
        .unwrap();
    let (db, owner, journal) = journal(&server).await;
    let id = journal
        .begin(
            None,
            Kind::Backup,
            [("public-immutable-id".into(), Some("running".into()))]
                .into_iter()
                .collect(),
            None,
        )
        .await
        .unwrap();
    journal
        .stop(id, "public-immutable-id", false)
        .await
        .unwrap();
    let recovery = Recovery::new(journal.clone(), client.clone()).unwrap();
    let waiting = recovery.clone();
    let caller = tokio::spawn(async move { waiting.reconcile().await });
    tokio::time::timeout(Duration::from_secs(2), server.state.entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut shutdown = Box::pin(recovery.shutdown(None));
    std::future::poll_fn(|cx| {
        assert!(shutdown.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(journal.load(id).await.unwrap().state, "active");
    server.state.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), shutdown)
        .await
        .unwrap();
    assert_eq!(journal.load(id).await.unwrap().state, "finished");
    assert_eq!(*server.state.status.lock().unwrap(), "running");
    client.shutdown(None).await;
    journal.shutdown(None).await;
    db.close().await;
    owner.shutdown(None).await;
    server.close().await;
}
#[tokio::test]
async fn negotiated_real_unix_http_preserves_identity_and_stop_start_state() {
    let server = Server::start().await;
    let client = DockerClient::connect(server.path.to_str().unwrap())
        .await
        .unwrap();
    let id = client.identity("friendly").await.unwrap();
    assert_eq!(id, "public-immutable-id");
    assert_eq!(client.state(&id).await.unwrap(), "running");
    client.stop(&id).await.unwrap();
    assert_eq!(client.state(&id).await.unwrap(), "exited");
    client.start(&id).await.unwrap();
    assert_eq!(client.state(&id).await.unwrap(), "running");
    assert!(client.state("missing").await.is_err());
    assert!(
        server
            .state
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.contains("/public-immutable-id/stop?t=30"))
    );
    client.shutdown(None).await;
    server.close().await;
}
#[tokio::test]
async fn lost_caller_does_not_cancel_an_inflight_docker_stop_or_release_shutdown() {
    let server = Server::start().await;
    server.state.delay_stop.store(true, Ordering::SeqCst);
    let client = DockerClient::connect(server.path.to_str().unwrap())
        .await
        .unwrap();
    let waiting = client.clone();
    let caller = tokio::spawn(async move { waiting.stop("public-immutable-id").await });
    tokio::time::timeout(Duration::from_secs(2), server.state.entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut shutdown = Box::pin(client.shutdown(None));
    std::future::poll_fn(|cx| {
        assert!(shutdown.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    server.state.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), shutdown)
        .await
        .unwrap();
    assert_eq!(*server.state.status.lock().unwrap(), "exited");
    server.close().await;
}
