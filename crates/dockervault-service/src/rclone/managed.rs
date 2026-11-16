//! Preparation and serialized reads; journal/claim owner activates descriptors.
mod read;
use super::{OUTPUT_LIMIT, RcloneError};
use crate::{
    ownership::{Owner, Work, WorkError},
    process::{ProcessOwner, ProcessSpec},
    recovery_state::ValidatedState,
};
use RcloneError::{Cancelled, Invalid};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use dockervault_core::crypto::CredentialCipher;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
type Result<T> = std::result::Result<T, RcloneError>;

#[derive(Clone)]
pub struct ManagedConfigBuilder {
    processes: ProcessOwner,
    blocking: Owner,
    tasks: Owner,
    binary: PathBuf,
}
impl ManagedConfigBuilder {
    pub fn new(processes: ProcessOwner, blocking: Owner, tasks: Owner, binary: PathBuf) -> Self {
        Self {
            processes,
            blocking,
            tasks,
            binary,
        }
    }
    /// Caller must hold the durable preparation claim/journal until settlement.
    pub fn start(
        &self,
        state: Arc<ValidatedState>,
        directory: PathBuf,
        cipher: Arc<CredentialCipher>,
    ) -> std::result::Result<Work<Result<Value>>, WorkError> {
        let this = self.clone();
        self.tasks.submit_async(move |cancel| async move {
            this.prepare(state, directory, cipher, cancel).await
        })
    }
    async fn prepare(
        &self,
        state: Arc<ValidatedState>,
        directory: PathBuf,
        cipher: Arc<CredentialCipher>,
        cancel: CancellationToken,
    ) -> Result<Value> {
        let check = cancel.clone();
        let (payload, generation) = self
            .blocking
            .submit_blocking(move |physical| {
                if physical.is_cancelled() || check.is_cancelled() {
                    return Err(Cancelled);
                }
                // Deserialize only this scalar-valued field: opaque imported DB JSON may
                // contain Python lone-surrogate escapes and must not be decoded here.
                #[derive(serde::Deserialize)]
                struct Packet {
                    rclone: Value,
                }
                let payload = serde_json::from_str::<Packet>(state.payload_json().as_str())
                    .map_err(|_| Invalid)?
                    .rclone;
                Ok((payload, Arc::new(Generation::create(directory)?)))
            })
            .map_err(|_| Invalid)?
            .wait()
            .await
            .map_err(|_| Invalid)??;
        let result = self.populate(&payload, &generation, &cipher, &cancel).await;
        let success = result.is_ok();
        self.blocking
            .submit_blocking(move |_| generation.finish(success))
            .map_err(|_| Invalid)?
            .wait()
            .await
            .map_err(|_| Invalid)??;
        result
    }
    fn spec(
        &self,
        generation: &Generation,
        unlock: &str,
        args: impl IntoIterator<Item = impl Into<std::ffi::OsString>>,
    ) -> ProcessSpec {
        let mut spec = ProcessSpec::new(
            &self.binary,
            [
                "--config".into(),
                generation.child_path().join("rclone.conf").into_os_string(),
                "--ask-password=false".into(),
                "--log-level".into(),
                "ERROR".into(),
                "--password-command".into(),
                "/usr/bin/printenv DV_RECOVERY_RCLONE_UNLOCK".into(),
            ],
        );
        spec.args.extend(args.into_iter().map(Into::into));
        spec.env = vec![
            ("PATH".into(), "/bin:/usr/bin:/usr/local/bin".into()),
            ("HOME".into(), "/nonexistent".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("DV_RECOVERY_RCLONE_UNLOCK".into(), unlock.into()),
        ];
        spec.stdout_limit = OUTPUT_LIMIT;
        spec.timeout = Some(Duration::from_secs(30));
        spec
    }
    async fn command(&self, spec: ProcessSpec, cancel: &CancellationToken) -> Result<()> {
        self.capture(spec, cancel).await.map(|_| ())
    }
    async fn capture(
        &self,
        spec: ProcessSpec,
        cancel: &CancellationToken,
    ) -> Result<crate::process::ProcessOutput> {
        if cancel.is_cancelled() {
            return Err(Cancelled);
        }
        let work = self.processes.start(spec).map_err(|_| Invalid)?;
        let stop = work.cancellation_token();
        let waiting = work.wait();
        tokio::pin!(waiting);
        let out = tokio::select! {biased;_=cancel.cancelled()=>{stop.cancel();let _=waiting.await;return Err(Cancelled);},out=&mut waiting=>out.map_err(|_|Invalid)?.map_err(|_|Invalid)?};
        if out.status.success() {
            Ok(out)
        } else {
            Err(Invalid)
        }
    }
    async fn populate(
        &self,
        payload: &Value,
        generation: &Generation,
        cipher: &CredentialCipher,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let mut random = [0u8; 48];
        getrandom::fill(&mut random).map_err(|_| Invalid)?;
        let unlock = zeroize::Zeroizing::new(URL_SAFE_NO_PAD.encode(random));
        self.command(self.spec(generation, &unlock, ["config", "touch"]), cancel)
            .await?;
        self.command(
            self.spec(generation, &unlock, ["config", "encryption", "set"]),
            cancel,
        )
        .await?;
        let mut daemon = self.spec(
            generation,
            &unlock,
            ["rcd", "--rc-addr", "unix://rc", "--rc-no-auth"],
        );
        daemon.working_directory = Some(generation.child_path());
        daemon.timeout = None;
        let daemon = self.processes.start(daemon).map_err(|_| Invalid)?;
        let stop = daemon.cancellation_token();
        let waiting = daemon.wait();
        tokio::pin!(waiting);
        let client = reqwest::Client::builder()
            .unix_socket(generation.path().join("rc"))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Invalid);
        let work = async {
            let client = client?;
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if cancel.is_cancelled() {
                        return Err(Cancelled);
                    }
                    if rpc(&client, "rc/noop", &json!({}), cancel).await.is_ok() {
                        return Ok(());
                    }
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(Cancelled),
                        _ = tokio::time::sleep(Duration::from_millis(20)) => {},
                    }
                }
            })
            .await
            .map_err(|_| Invalid)??;
            let mut refs = json!({});
            let mut overrides = json!({});
            let mut caches = Vec::new();
            for (name, remote) in payload["remotes"].as_object().ok_or(Invalid)? {
                if cancel.is_cancelled() {
                    return Err(Cancelled);
                }
                let mut parameters = remote.clone();
                let parameters = parameters.as_object_mut().ok_or(Invalid)?;
                if parameters.get("type").and_then(Value::as_str) == Some("cache") {
                    caches.push(name.clone());
                }
                let mut retained = json!({});
                for (field, value) in parameters.iter_mut() {
                    if let Some(reference) = value
                        .get("file_ref")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                    {
                        refs[name][field] = json!(reference);
                        *value = json!(format!("dv-recovery-file:{reference}"));
                    }
                    if field.starts_with("config_") {
                        retained[field] = value.clone();
                    }
                }
                overrides[name] = retained;
                rpc(&client,"config/create",&json!({"name":name,"type":"memory","parameters":parameters,"opt":{"nonInteractive":true,"noOutput":true,"noObscure":true}}),cancel).await?;
            }
            Ok((refs, overrides, caches))
        };
        tokio::pin!(work);
        let (result, settled) = tokio::select! {
            biased;
            _ = cancel.cancelled() => (Err(Cancelled), false),
            _ = &mut waiting => (Err(Invalid), true),
            result = &mut work => (result, false),
        };
        stop.cancel();
        if !settled {
            let _ = waiting.await;
        }
        let (refs, overrides, caches) = result?;
        self.command(
            self.spec(generation, &unlock, ["config", "encryption", "check"]),
            cancel,
        )
        .await?;
        Ok(
            json!({"directory":generation.directory,"unlock":cipher.encrypt_value(&unlock),"file_refs":refs,"cache_remotes":caches,"files":cipher.encrypt_value(&serde_json::to_string(&payload["files"]).map_err(|_|Invalid)?),"overrides":cipher.encrypt_value(&serde_json::to_string(&overrides).map_err(|_|Invalid)?),"roots":payload["roots"]}),
        )
    }
}
async fn rpc(
    client: &reqwest::Client,
    endpoint: &'static str,
    data: &Value,
    cancel: &CancellationToken,
) -> Result<Value> {
    let work = async {
        let mut response = client
            .post(format!("http://localhost/{endpoint}"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(data).map_err(|_| Invalid)?)
            .send()
            .await
            .map_err(|_| Invalid)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Invalid);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Invalid)? {
            if bytes.len().saturating_add(chunk.len()) > OUTPUT_LIMIT {
                return Err(Invalid);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Invalid)
    };
    tokio::select! {biased;_=cancel.cancelled()=>Err(Cancelled),result=work=>result}
}
struct Generation {
    directory: PathBuf,
    parent: File,
    held: File,
    name: std::ffi::OsString,
}
fn directory(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW)
        .open(path)
}
impl Generation {
    fn create(path: PathBuf) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Invalid);
        }
        let parent = directory(path.parent().ok_or(Invalid)?).map_err(|_| Invalid)?;
        let name = path.file_name().ok_or(Invalid)?.to_owned();
        nix::sys::stat::mkdirat(&parent, Path::new(&name), nix::sys::stat::Mode::S_IRWXU)
            .map_err(|_| Invalid)?;
        let held = nix::fcntl::openat(
            &parent,
            Path::new(&name),
            nix::fcntl::OFlag::O_RDONLY
                | nix::fcntl::OFlag::O_DIRECTORY
                | nix::fcntl::OFlag::O_NOFOLLOW
                | nix::fcntl::OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::empty(),
        )
        .map(File::from)
        .map_err(|_| Invalid)?;
        parent.sync_all().map_err(|_| Invalid)?;
        Ok(Self {
            directory: path,
            parent,
            held,
            name,
        })
    }
    fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.held.as_raw_fd()))
    }
    fn child_path(&self) -> PathBuf {
        PathBuf::from(format!(
            "/proc/{}/fd/{}",
            std::process::id(),
            self.held.as_raw_fd()
        ))
    }
    fn finish(&self, success: bool) -> Result<()> {
        let anchored =
            PathBuf::from(format!("/proc/self/fd/{}", self.parent.as_raw_fd())).join(&self.name);
        let actual = std::fs::symlink_metadata(&anchored).map_err(|_| Invalid)?;
        let held = self.held.metadata().map_err(|_| Invalid)?;
        if actual.dev() != held.dev() || actual.ino() != held.ino() || !actual.is_dir() {
            return Err(Invalid);
        }
        if success {
            let config = OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(self.path().join("rclone.conf"))
                .map_err(|_| Invalid)?;
            if !config.metadata().map_err(|_| Invalid)?.is_file() {
                return Err(Invalid);
            }
            config
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| Invalid)?;
            config.sync_all().map_err(|_| Invalid)?;
            match std::fs::remove_file(self.path().join("rc")) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Invalid),
            }
            self.held.sync_all().map_err(|_| Invalid)?;
        } else {
            std::fs::remove_dir_all(anchored).map_err(|_| Invalid)?;
        }
        self.parent.sync_all().map_err(|_| Invalid)
    }
}

#[cfg(test)]
mod tests;
