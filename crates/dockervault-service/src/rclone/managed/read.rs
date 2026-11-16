//! Read the actual refreshable config under the same cross-process lock as Python.
mod run;
use super::*;
use nix::fcntl::{Flock, FlockArg};
pub use run::ManagedCommandOutput;

struct LockedConfig {
    generation: Generation,
    _lock: Flock<File>,
    unlock: zeroize::Zeroizing<String>,
    state: Value,
}
impl LockedConfig {
    fn acquire(
        state: Value,
        cipher: &CredentialCipher,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        if cancel.is_cancelled() {
            return Err(Cancelled);
        }
        let path = PathBuf::from(
            state
                .get("directory")
                .and_then(Value::as_str)
                .ok_or(Invalid)?,
        );
        if !path.is_absolute() {
            return Err(Invalid);
        }
        let parent = directory(path.parent().ok_or(Invalid)?).map_err(|_| Invalid)?;
        let name = path.file_name().ok_or(Invalid)?.to_owned();
        let held = File::from(
            nix::fcntl::openat(
                &parent,
                Path::new(&name),
                nix::fcntl::OFlag::O_RDONLY
                    | nix::fcntl::OFlag::O_DIRECTORY
                    | nix::fcntl::OFlag::O_NOFOLLOW
                    | nix::fcntl::OFlag::O_CLOEXEC,
                nix::sys::stat::Mode::empty(),
            )
            .map_err(|_| Invalid)?,
        );
        let generation = Generation {
            directory: path,
            parent,
            held,
            name,
        };
        let config = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(generation.path().join("rclone.conf"))
            .map_err(|_| Invalid)?;
        if !config.metadata().map_err(|_| Invalid)?.is_file() {
            return Err(Invalid);
        }
        let mut lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(generation.path().join(".lock"))
            .map_err(|_| Invalid)?;
        if !lock.metadata().map_err(|_| Invalid)?.is_file() {
            return Err(Invalid);
        }
        let lock = loop {
            if cancel.is_cancelled() {
                return Err(Cancelled);
            }
            match Flock::lock(lock, FlockArg::LockExclusiveNonblock) {
                Ok(lock) => break lock,
                Err((file, nix::errno::Errno::EWOULDBLOCK)) => {
                    lock = file;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return Err(Invalid),
            }
        };
        let unlock = zeroize::Zeroizing::new(
            cipher
                .decrypt_value(state.get("unlock").and_then(Value::as_str).ok_or(Invalid)?)
                .map_err(|_| Invalid)?,
        );
        Ok(Self {
            generation,
            _lock: lock,
            unlock,
            state,
        })
    }
    fn restore(
        &self,
        cipher: &CredentialCipher,
        mut remotes: Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let values = remotes.as_object_mut().ok_or(Invalid)?;
        let text = zeroize::Zeroizing::new(
            cipher
                .decrypt_value(
                    self.state
                        .get("overrides")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                )
                .map_err(|_| Invalid)?,
        );
        let overrides: Value = serde_json::from_str(if text.is_empty() { "{}" } else { &text })
            .map_err(|_| Invalid)?;
        for (name, fields) in overrides.as_object().ok_or(Invalid)? {
            if cancel.is_cancelled() {
                return Err(Cancelled);
            }
            let remote = values
                .get_mut(name)
                .and_then(Value::as_object_mut)
                .ok_or(Invalid)?;
            for (field, value) in fields.as_object().ok_or(Invalid)? {
                if remote.get(field).is_some_and(|old| old != value) {
                    return Err(Invalid);
                }
                remote.insert(field.clone(), value.clone());
            }
        }
        for (name, fields) in self
            .state
            .get("file_refs")
            .and_then(Value::as_object)
            .ok_or(Invalid)?
        {
            if cancel.is_cancelled() {
                return Err(Cancelled);
            }
            let remote = values
                .get_mut(name)
                .and_then(Value::as_object_mut)
                .ok_or(Invalid)?;
            for (field, reference) in fields.as_object().ok_or(Invalid)? {
                let reference = reference.as_str().ok_or(Invalid)?;
                if remote.get(field).and_then(Value::as_str)
                    != Some(format!("dv-recovery-file:{reference}").as_str())
                {
                    return Err(Invalid);
                }
                remote.insert(field.clone(), json!({"file_ref":reference}));
            }
        }
        Ok(remotes)
    }
}
impl ManagedConfigBuilder {
    /// Raw normalized remotes for export packaging, not a portable-package validator.
    /// Caller retains its crypto/setup claim through this physically settled work.
    pub fn start_read_config(
        &self,
        state: Value,
        cipher: Arc<CredentialCipher>,
    ) -> std::result::Result<Work<Result<Value>>, WorkError> {
        let this = self.clone();
        self.tasks.submit_async(move |cancel| async move {
            this.read_config(state, cipher, cancel).await
        })
    }
    /// Complete managed portable package from live config and authenticated metadata.
    /// External/unmanaged config merging remains a separate caller operation.
    pub fn start_export_managed(
        &self,
        state: Value,
        cipher: Arc<CredentialCipher>,
        storages: Vec<dockervault_core::portable_records::RemoteStorage>,
        options: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    ) -> std::result::Result<Work<Result<Value>>, WorkError> {
        let this = self.clone();
        self.tasks.submit_async(move |cancel| async move {
            let remotes = this
                .read_config(state.clone(), cipher.clone(), cancel.clone())
                .await?;
            this.blocking
                .submit_blocking(move |physical| {
                    if cancel.is_cancelled() || physical.is_cancelled() {
                        return Err(Cancelled);
                    }
                    let text = zeroize::Zeroizing::new(
                        cipher
                            .decrypt_value(
                                state.get("files").and_then(Value::as_str).ok_or(Invalid)?,
                            )
                            .map_err(|_| Invalid)?,
                    );
                    let files: Value = serde_json::from_str(&text).map_err(|_| Invalid)?;
                    let package = dockervault_core::rclone_package::assemble_canonical(
                        &remotes,
                        &files,
                        &storages,
                        state.get("roots").ok_or(Invalid)?,
                        &options,
                        &|| cancel.is_cancelled() || physical.is_cancelled(),
                    )?;
                    dockervault_core::rclone_package::validate(
                        &package,
                        &storages,
                        &options,
                        &|| cancel.is_cancelled() || physical.is_cancelled(),
                    )?;
                    Ok(package)
                })
                .map_err(|_| Invalid)?
                .wait()
                .await
                .map_err(|_| Invalid)?
        })
    }
    async fn read_config(
        &self,
        state: Value,
        cipher: Arc<CredentialCipher>,
        cancel: CancellationToken,
    ) -> Result<Value> {
        let this = self;
        let check = cancel.clone();
        let key = cipher.clone();
        let locked = this
            .blocking
            .submit_blocking(move |physical| {
                if physical.is_cancelled() {
                    return Err(Cancelled);
                }
                LockedConfig::acquire(state, &key, &check)
            })
            .map_err(|_| Invalid)?
            .wait()
            .await
            .map_err(|_| Invalid)??;
        let result = this
            .capture(
                this.spec(&locked.generation, &locked.unlock, ["config", "dump"]),
                &cancel,
            )
            .await;
        this.blocking
            .submit_blocking(move |_| {
                let out = result?;
                let remotes = serde_json::from_slice(&out.stdout).map_err(|_| Invalid)?;
                locked.restore(&cipher, remotes, &cancel)
            })
            .map_err(|_| Invalid)?
            .wait()
            .await
            .map_err(|_| Invalid)?
    }
}
