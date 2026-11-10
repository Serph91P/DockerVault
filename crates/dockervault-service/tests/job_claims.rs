#![cfg(target_os = "linux")]
use dockervault_service::{
    jobs::{ClaimError, Claims, Resource},
    ownership::Owner,
};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dv-claims-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn realpath_aliases_and_ancestor_sources_exclude_each_other_but_siblings_do_not() {
    let root = Scratch::new();
    std::fs::create_dir_all(root.path().join("parent/child")).unwrap();
    std::fs::create_dir(root.path().join("parent-other")).unwrap();
    std::os::unix::fs::symlink(root.path().join("parent"), root.path().join("alias")).unwrap();
    let claims = Claims::default();
    let cancel = CancellationToken::new();
    let parent = claims
        .acquire(
            [Resource::existing_path(&root.path().join("parent")).unwrap()],
            &cancel,
        )
        .await
        .unwrap();
    let alias = claims.acquire(
        [Resource::existing_path(&root.path().join("alias")).unwrap()],
        &cancel,
    );
    let child = claims.acquire(
        [Resource::existing_path(&root.path().join("parent/child")).unwrap()],
        &cancel,
    );
    tokio::pin!(alias, child);
    assert!(futures_util::poll!(alias.as_mut()).is_pending());
    assert!(futures_util::poll!(child.as_mut()).is_pending());
    let sibling = claims.acquire(
        [Resource::existing_path(&root.path().join("parent-other")).unwrap()],
        &cancel,
    );
    tokio::pin!(sibling);
    let sibling = match futures_util::poll!(sibling.as_mut()) {
        std::task::Poll::Ready(Ok(lease)) => lease,
        _ => panic!("independent source was blocked"),
    };
    drop(parent);
    let alias = tokio::time::timeout(Duration::from_secs(2), alias)
        .await
        .unwrap()
        .unwrap();
    assert!(futures_util::poll!(child.as_mut()).is_pending());
    drop(alias);
    let _child = tokio::time::timeout(Duration::from_secs(2), child)
        .await
        .unwrap()
        .unwrap();
    drop(sibling);
}

#[tokio::test]
async fn inverse_partial_sets_cancel_without_leaked_claims() {
    let claims = Claims::default();
    let cancel = CancellationToken::new();
    let a = Resource::container("immutable-a").unwrap();
    let b = Resource::volume("named-volume").unwrap();
    let current = claims
        .acquire([a.clone(), b.clone(), a.clone()], &cancel)
        .await
        .unwrap();
    let blocked = CancellationToken::new();
    let waiting = claims.acquire([b.clone(), Resource::target(17), a.clone()], &blocked);
    tokio::pin!(waiting);
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    blocked.cancel();
    blocked.cancel();
    assert!(matches!(waiting.await, Err(ClaimError::Cancelled)));
    let other = claims.acquire([Resource::target(17)], &cancel);
    tokio::pin!(other);
    let other = match futures_util::poll!(other.as_mut()) {
        std::task::Poll::Ready(Ok(lease)) => lease,
        _ => panic!("cancelled set leaked a claim"),
    };
    drop(other);
    drop(current);
    let _next = claims.acquire([b, a], &cancel).await.unwrap();
}

#[tokio::test]
async fn shutdown_rejects_new_claims_and_wakes_waiters() {
    let claims = Claims::default();
    let cancel = CancellationToken::new();
    let key = Resource::target(7);
    let active = claims.acquire([key.clone()], &cancel).await.unwrap();
    let waiting = claims.acquire([key.clone()], &cancel);
    tokio::pin!(waiting);
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    claims.close();
    assert!(matches!(waiting.await, Err(ClaimError::Closed)));
    assert!(matches!(
        claims.acquire([key], &cancel).await,
        Err(ClaimError::Closed)
    ));
    drop(active);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_http_work_handle_does_not_release_claims_while_real_io_is_still_blocked() {
    let claims = Claims::default();
    let cancel = CancellationToken::new();
    let key = Resource::container("owned-immutable-id").unwrap();
    let lease = claims.acquire([key.clone()], &cancel).await.unwrap();
    let (mut reader, mut writer) = UnixStream::pair().unwrap();
    let owner = Owner::new(1).unwrap();
    let (started, entered) = tokio::sync::oneshot::channel();
    let work = owner
        .submit_blocking(move |_| {
            let _lease = lease;
            started.send(()).unwrap();
            let mut byte = [0];
            reader.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [23]);
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), entered)
        .await
        .unwrap()
        .unwrap();
    work.cancel();
    work.cancel();
    drop(work);
    let next = claims.acquire([key], &cancel);
    tokio::pin!(next);
    assert!(futures_util::poll!(next.as_mut()).is_pending());
    assert_eq!(owner.active(), 1);
    writer.write_all(&[23]).unwrap();
    let _next = tokio::time::timeout(Duration::from_secs(2), next)
        .await
        .unwrap()
        .unwrap();
    owner.shutdown(None).await;
    assert_eq!(owner.active(), 0);
}

#[tokio::test]
async fn pre_cancelled_requests_cannot_claim_and_resource_namespaces_do_not_alias() {
    let claims = Claims::default();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        claims.acquire([Resource::target(0)], &cancelled).await,
        Err(ClaimError::Cancelled)
    ));
    assert!(Resource::container("").is_err());
    assert!(Resource::volume("bad\0name").is_err());
    let cancel = CancellationToken::new();
    let _container = claims
        .acquire([Resource::container("same-value").unwrap()], &cancel)
        .await
        .unwrap();
    let volume = claims.acquire([Resource::volume("same-value").unwrap()], &cancel);
    tokio::pin!(volume);
    assert!(matches!(
        futures_util::poll!(volume.as_mut()),
        std::task::Poll::Ready(Ok(_))
    ));
}
