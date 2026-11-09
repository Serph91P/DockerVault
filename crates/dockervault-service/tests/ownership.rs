use dockervault_service::ownership::{Owner, WorkError};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_or_repeatedly_cancelling_a_started_blocking_call_keeps_its_permit_until_joined() {
    let owner = Owner::new(1).unwrap();
    let finish = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = oneshot::channel();
    let worker_finish = finish.clone();
    let worker_completed = completed.clone();
    let work = owner
        .submit_blocking(move |_cancel| {
            entered_tx.send(()).unwrap();
            while !worker_finish.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            worker_completed.store(true, Ordering::SeqCst);
            17
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await
        .unwrap()
        .unwrap();
    work.cancel();
    work.cancel();
    drop(work);
    assert_eq!(owner.active(), 1);
    assert!(matches!(
        owner.submit_blocking(|_| 99),
        Err(WorkError::Busy)
    ));
    let draining = owner.shutdown(None);
    tokio::pin!(draining);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut draining)
            .await
            .is_err()
    );
    assert!(!completed.load(Ordering::SeqCst));
    finish.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(2), draining)
        .await
        .unwrap();
    assert!(completed.load(Ordering::SeqCst));
    assert_eq!(owner.active(), 0);
}

#[tokio::test]
async fn closed_owner_rejects_admission_but_unlimited_grace_does_not_cancel_running_work() {
    let owner = Owner::new(1).unwrap();
    let (finish_tx, finish_rx) = oneshot::channel();
    let work = owner
        .submit_async(move |cancel| async move {
            finish_rx.await.unwrap();
            cancel.is_cancelled()
        })
        .unwrap();
    owner.close();
    assert!(matches!(
        owner.submit_async(|_| async { 0 }),
        Err(WorkError::Closed)
    ));
    let draining = owner.shutdown(None);
    tokio::pin!(draining);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut draining)
            .await
            .is_err()
    );
    finish_tx.send(()).unwrap();
    draining.await;
    assert!(!work.wait().await.unwrap());
}

#[tokio::test]
async fn expired_grace_requests_cooperative_cancel_and_still_drains_every_task() {
    let owner = Owner::new(2).unwrap();
    let a = owner
        .submit_async(|cancel| async move {
            cancel.cancelled().await;
            11
        })
        .unwrap();
    let b = owner
        .submit_async(|cancel| async move {
            cancel.cancelled().await;
            13
        })
        .unwrap();
    assert!(matches!(
        owner.submit_async(|_| async { 0 }),
        Err(WorkError::Busy)
    ));
    owner.shutdown(Some(Duration::ZERO)).await;
    assert_eq!(a.wait().await.unwrap(), 11);
    assert_eq!(b.wait().await.unwrap(), 13);
    assert_eq!(owner.active(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_panic_is_reported_only_after_the_worker_has_exited() {
    let owner = Owner::new(1).unwrap();
    let work = owner
        .submit_blocking(|_| -> () {
            panic!("owned synthetic worker panic");
        })
        .unwrap();
    assert_eq!(work.wait().await.unwrap_err(), WorkError::WorkerFailed);
    owner.shutdown(None).await;
    assert_eq!(owner.active(), 0);
}

#[tokio::test]
async fn cancelling_the_waiter_requests_cancel_without_aborting_the_owned_async_cleanup() {
    let owner = Owner::new(1).unwrap();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let cleaned = Arc::new(AtomicBool::new(false));
    let worker_cleaned = cleaned.clone();
    let work = owner
        .submit_async(move |cancel| async move {
            entered_tx.send(()).unwrap();
            cancel.cancelled().await;
            finish_rx.await.unwrap();
            worker_cleaned.store(true, Ordering::SeqCst);
        })
        .unwrap();
    entered_rx.await.unwrap();
    let waiter = tokio::spawn(work.wait());
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert_eq!(owner.active(), 1);
    assert!(!cleaned.load(Ordering::SeqCst));
    finish_tx.send(()).unwrap();
    owner.shutdown(None).await;
    assert!(cleaned.load(Ordering::SeqCst));
    assert_eq!(owner.active(), 0);
}

#[test]
fn zero_capacity_fails_instead_of_creating_a_permanently_unserviceable_owner() {
    assert!(matches!(Owner::new(0), Err(WorkError::InvalidCapacity)));
}
