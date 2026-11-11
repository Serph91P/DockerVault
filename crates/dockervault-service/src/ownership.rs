//! Bounded work whose lifetime is independent of its HTTP caller's future.
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkError {
    Busy,
    Closed,
    WorkerFailed,
    InvalidCapacity,
}

impl std::fmt::Display for WorkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "Work capacity exhausted",
            Self::Closed => "Work owner is closed",
            Self::WorkerFailed => "Owned worker failed",
            Self::InvalidCapacity => "Invalid work capacity",
        })
    }
}

impl std::error::Error for WorkError {}

#[derive(Clone)]
pub struct Owner {
    inner: Arc<Inner>,
}

struct Inner {
    closed: Mutex<bool>,
    slots: Arc<Semaphore>,
    capacity: usize,
    tracker: TaskTracker,
    cancel: CancellationToken,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.tracker.close();
    }
}

pub struct Work<T> {
    reply: Option<oneshot::Receiver<Result<T, WorkError>>>,
    cancel: CancellationToken,
}

impl<T> Work<T> {
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub async fn wait(mut self) -> Result<T, WorkError> {
        self.reply
            .take()
            .ok_or(WorkError::WorkerFailed)?
            .await
            .map_err(|_| WorkError::WorkerFailed)?
    }
}

impl<T> Drop for Work<T> {
    fn drop(&mut self) {
        // This requests cleanup; the tracked worker, not this handle, owns the permit.
        self.cancel.cancel();
    }
}

impl Owner {
    pub fn new(capacity: usize) -> Result<Self, WorkError> {
        if capacity == 0 || capacity > Semaphore::MAX_PERMITS {
            return Err(WorkError::InvalidCapacity);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                closed: Mutex::new(false),
                slots: Arc::new(Semaphore::new(capacity)),
                capacity,
                tracker: TaskTracker::new(),
                cancel: CancellationToken::new(),
            }),
        })
    }

    pub fn active(&self) -> usize {
        self.inner.capacity - self.inner.slots.available_permits()
    }

    pub fn submit_async<T, F, Fut>(&self, operation: F) -> Result<Work<T>, WorkError>
    where
        T: Send + 'static,
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        tokio::runtime::Handle::try_current().map_err(|_| WorkError::WorkerFailed)?;
        let closed = self.inner.closed.lock().map_err(|_| WorkError::Closed)?;
        if *closed {
            return Err(WorkError::Closed);
        }
        let permit = Arc::new(
            self.inner
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| WorkError::Busy)?,
        );
        let physical_permit = permit.clone();
        let cancel = self.inner.cancel.child_token();
        let worker_cancel = cancel.clone();
        let (sender, reply) = oneshot::channel();
        let worker = self.inner.tracker.spawn(async move {
            let _permit = physical_permit;
            operation(worker_cancel).await
        });
        self.inner.tracker.spawn(async move {
            let result = worker.await.map_err(|_| WorkError::WorkerFailed);
            // Completion can wake and immediately reenter admission on another
            // thread. Release only AFTER physical join, but BEFORE notifying it.
            drop(permit);
            let _ = sender.send(result);
        });
        drop(closed);
        Ok(Work {
            reply: Some(reply),
            cancel,
        })
    }

    pub fn submit_blocking<T, F>(&self, operation: F) -> Result<Work<T>, WorkError>
    where
        T: Send + 'static,
        F: FnOnce(CancellationToken) -> T + Send + 'static,
    {
        tokio::runtime::Handle::try_current().map_err(|_| WorkError::WorkerFailed)?;
        let closed = self.inner.closed.lock().map_err(|_| WorkError::Closed)?;
        if *closed {
            return Err(WorkError::Closed);
        }
        let permit: Arc<OwnedSemaphorePermit> = Arc::new(
            self.inner
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| WorkError::Busy)?,
        );
        let physical_permit = permit.clone();
        let cancel = self.inner.cancel.child_token();
        let worker_cancel = cancel.clone();
        let (sender, reply) = oneshot::channel();
        let worker = self.inner.tracker.spawn_blocking(move || {
            let _permit = physical_permit;
            operation(worker_cancel)
        });
        self.inner.tracker.spawn(async move {
            let result = worker.await.map_err(|_| WorkError::WorkerFailed);
            drop(permit);
            let _ = sender.send(result);
        });
        drop(closed);
        Ok(Work {
            reply: Some(reply),
            cancel,
        })
    }

    pub fn close(&self) {
        let mut closed = self
            .inner
            .closed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *closed = true;
        self.inner.tracker.close();
    }

    pub async fn shutdown(&self, grace: Option<Duration>) {
        self.close();
        if let Some(grace) = grace
            && tokio::time::timeout(grace, self.inner.tracker.wait())
                .await
                .is_err()
        {
            self.inner.cancel.cancel();
        }
        // Even an expired deadline cannot turn non-abortable I/O into settled work.
        self.inner.tracker.wait().await;
    }
}
