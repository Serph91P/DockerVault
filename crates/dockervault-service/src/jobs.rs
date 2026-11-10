//! Process-local job exclusion. Leases belong in the tracked physical worker, never its HTTP waiter.
//! Durable admission/recovery and the instance fence remain separate journal responsibilities.
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Resource(Key);
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Target(i64),
    Container(String),
    Volume(String),
    Path(PathBuf),
}

impl Resource {
    pub fn target(id: i64) -> Self {
        Self(Key::Target(id))
    }
    /// Identity must already be the daemon's resolved immutable id, not a display name.
    pub fn container(identity: impl Into<String>) -> Result<Self, ClaimError> {
        let identity = identity.into();
        validate_name(&identity)?;
        Ok(Self(Key::Container(identity)))
    }
    pub fn volume(name: impl Into<String>) -> Result<Self, ClaimError> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self(Key::Volume(name)))
    }
    /// Synchronous filesystem resolution: call in the source resolver's blocking owner.
    /// Missing restore destinations require the separately validated ancestor/dirfd resolver.
    pub fn existing_path(path: &Path) -> io::Result<Self> {
        Ok(Self(Key::Path(std::fs::canonicalize(path)?)))
    }
    fn conflicts(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Key::Path(a), Key::Path(b)) => a.starts_with(b) || b.starts_with(a),
            _ => self == other,
        }
    }
}
fn validate_name(name: &str) -> Result<(), ClaimError> {
    if name.is_empty() || name.contains('\0') {
        Err(ClaimError::InvalidResource)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimError {
    Cancelled,
    Closed,
    InvalidResource,
}
impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "Claim acquisition cancelled",
            Self::Closed => "Claim owner closed",
            Self::InvalidResource => "Invalid claim identity",
        })
    }
}
impl std::error::Error for ClaimError {}

#[derive(Clone, Default)]
pub struct Claims {
    inner: Arc<Inner>,
}
#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    changed: Notify,
}
#[derive(Default)]
struct State {
    closed: bool,
    next: u64,
    active: BTreeMap<u64, BTreeSet<Resource>>,
}

/// Not clonable: transfer this lease into the owned task that physically settles all source I/O.
/// Drop only after source settlement; the target lease and global permit outlive remote finalization.
pub struct Claim {
    inner: Arc<Inner>,
    id: u64,
}
impl Drop for Claim {
    fn drop(&mut self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .remove(&self.id);
        self.inner.changed.notify_waiters();
    }
}
impl Claims {
    pub async fn acquire(
        &self,
        resources: impl IntoIterator<Item = Resource>,
        cancel: &CancellationToken,
    ) -> Result<Claim, ClaimError> {
        let resources: BTreeSet<_> = resources.into_iter().collect();
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            // Register before checking the condition, including on a multi-threaded runtime.
            changed.as_mut().enable();
            if cancel.is_cancelled() {
                return Err(ClaimError::Cancelled);
            }
            {
                let mut state = self.inner.state.lock().map_err(|_| ClaimError::Closed)?;
                if state.closed {
                    return Err(ClaimError::Closed);
                }
                let conflict = state.active.values().any(|held| {
                    held.iter()
                        .any(|a| resources.iter().any(|b| a.conflicts(b)))
                });
                if !conflict {
                    if cancel.is_cancelled() {
                        return Err(ClaimError::Cancelled);
                    }
                    let id = state.next.checked_add(1).ok_or(ClaimError::Closed)?;
                    state.next = id;
                    state.active.insert(id, resources);
                    return Ok(Claim {
                        inner: self.inner.clone(),
                        id,
                    });
                }
            }
            tokio::select! {biased;_ = cancel.cancelled()=>return Err(ClaimError::Cancelled),_ = &mut changed=>{}}
        }
    }
    pub fn close(&self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
        self.inner.changed.notify_waiters();
    }
}
