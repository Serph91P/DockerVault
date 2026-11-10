//! Shared accepted-provider claims and owned abandoned preparation cleanup.
mod cleanup;
use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg},
};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy)]
pub enum ClaimKind {
    Setup,
    Crypto,
}
impl ClaimKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Setup => ".setup.lock",
            Self::Crypto => ".recovery-crypto.lock",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationError {
    Busy,
    Io,
    InvalidJournal,
    UnsafeGeneration,
    UnsafeKeyPreparation,
}
impl std::fmt::Display for PreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "Setup is already in progress",
            Self::Io => "Unable to acquire recovery claim",
            Self::InvalidJournal => "Recovery preparation journal needs repair",
            Self::UnsafeGeneration => "Unsafe recovery generation",
            Self::UnsafeKeyPreparation => "Unsafe instance key preparation",
        })
    }
}
impl std::error::Error for PreparationError {}
#[derive(Default)]
struct Memory {
    setup: AtomicBool,
    crypto: AtomicBool,
}
impl Memory {
    fn gate(&self, kind: ClaimKind) -> &AtomicBool {
        match kind {
            ClaimKind::Setup => &self.setup,
            ClaimKind::Crypto => &self.crypto,
        }
    }
}
#[derive(Clone)]
enum Source {
    File(PathBuf),
    Memory(Arc<Memory>),
}
#[derive(Clone)]
pub struct PreparationClaims {
    source: Source,
}
pub struct PreparationGuard {
    _file: Option<Flock<File>>,
    memory: Option<(Arc<Memory>, ClaimKind)>,
    database: Option<PathBuf>,
    kind: ClaimKind,
}
impl PreparationGuard {
    /// Run in the owning blocking task while this guard remains held. On failure the
    /// preparation record remains durable; callers must not create an administrator.
    pub fn discard_incomplete(&self, key_file: &std::path::Path) -> Result<bool, PreparationError> {
        if !matches!(self.kind, ClaimKind::Setup) {
            return Err(PreparationError::InvalidJournal);
        }
        match &self.database {
            Some(database) => cleanup::discard(database, key_file),
            None => Ok(false),
        }
    }
}
impl Drop for PreparationGuard {
    fn drop(&mut self) {
        if let Some((memory, kind)) = &self.memory {
            memory.gate(*kind).store(false, Ordering::Release);
        }
    }
}
impl PreparationClaims {
    /// None is only an explicitly in-memory database, never a fallback after file errors.
    pub fn new(database: Option<PathBuf>) -> Self {
        Self {
            source: match database {
                Some(path) => Source::File(path),
                None => Source::Memory(Arc::new(Memory::default())),
            },
        }
    }
    /// Synchronous filesystem work: acquire inside the tracked physical owner and retain the
    /// guard through mutation/child settlement, not in the disposable HTTP waiter.
    pub fn try_claim(&self, kind: ClaimKind) -> Result<PreparationGuard, PreparationError> {
        match &self.source {
            Source::Memory(memory) => {
                memory
                    .gate(kind)
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .map_err(|_| PreparationError::Busy)?;
                Ok(PreparationGuard {
                    _file: None,
                    memory: Some((memory.clone(), kind)),
                    database: None,
                    kind,
                })
            }
            Source::File(database) => {
                // Match pathlib.absolute + suffix; canonicalizing the DB symlink here would
                // silently choose a different marker from the published Python provider.
                let database = std::path::absolute(database).map_err(|_| PreparationError::Io)?;
                let mut path = database.clone().into_os_string();
                path.push(kind.suffix());
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .mode(0o600)
                    .custom_flags(nix::libc::O_NOFOLLOW)
                    .open(PathBuf::from(path))
                    .map_err(|_| PreparationError::Io)?;
                let file = Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(
                    |(_file, error)| {
                        if error == Errno::EWOULDBLOCK {
                            PreparationError::Busy
                        } else {
                            PreparationError::Io
                        }
                    },
                )?;
                Ok(PreparationGuard {
                    _file: Some(file),
                    memory: None,
                    database: Some(database),
                    kind,
                })
            }
        }
    }
}
