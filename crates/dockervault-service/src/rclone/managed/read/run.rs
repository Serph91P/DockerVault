//! Managed CLI lifetime: no inherited credentials, on-disk provider files or source caches.
#[cfg(test)]
mod tests;
use super::*;
use crate::rclone::managed::material::MaterializedFiles;
use dockervault_core::rclone_path::environment_name;
use std::{ffi::OsString, os::unix::process::ExitStatusExt};

pub struct ManagedCommandOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: &'static str,
}
impl std::fmt::Debug for ManagedCommandOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedCommandOutput")
            .field("code", &self.code)
            .field("stdout_bytes", &self.stdout.len())
            .finish()
    }
}
impl ManagedCommandOutput {
    fn unavailable() -> Self {
        Self {
            code: 1,
            stdout: String::new(),
            stderr: "Remote storage unavailable",
        }
    }
    fn credentials() -> Self {
        Self {
            code: 1,
            stdout: String::new(),
            stderr: "Storage credentials required",
        }
    }
}
struct Runtime {
    locked: LockedConfig,
    cache: Generation,
    _material: MaterializedFiles,
    _ssh: Vec<File>,
    environment: Vec<(OsString, OsString)>,
}
fn check(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Cancelled)
    } else {
        Ok(())
    }
}
fn open_beneath(mut directory: File, path: &Path, leaf: bool) -> Option<File> {
    let parts: Vec<_> = path
        .components()
        .filter_map(|part| match part {
            std::path::Component::RootDir | std::path::Component::CurDir => None,
            part => Some(part),
        })
        .collect();
    for (i, part) in parts.iter().enumerate() {
        let std::path::Component::Normal(name) = part else {
            return None;
        };
        let mut flags = nix::fcntl::OFlag::O_RDONLY
            | nix::fcntl::OFlag::O_NOFOLLOW
            | nix::fcntl::OFlag::O_CLOEXEC;
        if !leaf || i + 1 != parts.len() {
            flags |= nix::fcntl::OFlag::O_DIRECTORY;
        } else {
            flags |= nix::fcntl::OFlag::O_NONBLOCK;
        }
        directory = File::from(
            nix::fcntl::openat(&directory, *name, flags, nix::sys::stat::Mode::empty()).ok()?,
        );
    }
    Some(directory)
}
fn key_file(path: &Path, allowed: &[PathBuf]) -> Option<File> {
    let path = path.canonicalize().ok()?;
    for root in allowed {
        let Ok(root) = root.canonicalize() else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let system = directory(Path::new("/")).ok()?;
        let root = open_beneath(system, &root, false)?;
        let file = open_beneath(root, relative, true)?;
        if file.metadata().ok()?.is_file() {
            return Some(file);
        }
    }
    None
}
fn cache(parent: &Generation) -> Result<Generation> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Invalid)?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let name = OsString::from(format!("dvrc-cache-{id}"));
    nix::sys::stat::mkdirat(
        &parent.held,
        Path::new(&name),
        nix::sys::stat::Mode::S_IRWXU,
    )
    .map_err(|_| Invalid)?;
    let held = File::from(
        nix::fcntl::openat(
            &parent.held,
            Path::new(&name),
            nix::fcntl::OFlag::O_RDONLY
                | nix::fcntl::OFlag::O_DIRECTORY
                | nix::fcntl::OFlag::O_NOFOLLOW
                | nix::fcntl::OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::empty(),
        )
        .map_err(|_| Invalid)?,
    );
    Ok(Generation {
        directory: parent.directory.join(&name),
        parent: parent.held.try_clone().map_err(|_| Invalid)?,
        held,
        name,
    })
}
impl Runtime {
    fn prepare(
        state: Value,
        cipher: &CredentialCipher,
        remote: &str,
        allowed: &[PathBuf],
        cancel: &CancellationToken,
    ) -> Result<Option<Self>> {
        check(cancel)?;
        let root = state
            .get("roots")
            .and_then(Value::as_array)
            .ok_or(Invalid)?
            .iter()
            .find(|root| root.get("remote_name").and_then(Value::as_str) == Some(remote));
        let Some(root) = root else {
            return Ok(None);
        };
        let mut environment = Vec::new();
        let mut ssh = Vec::new();
        if root.get("reason").and_then(Value::as_str) == Some("ssh_private_key_required") {
            for requirement in root
                .get("ssh_requirements")
                .and_then(Value::as_array)
                .ok_or(Invalid)?
            {
                check(cancel)?;
                let Some(path) = requirement.get("key_path").and_then(Value::as_str) else {
                    return Ok(None);
                };
                let Some(file) = key_file(Path::new(path), allowed) else {
                    return Ok(None);
                };
                environment.push((
                    environment_name(
                        requirement
                            .get("remote")
                            .and_then(Value::as_str)
                            .ok_or(Invalid)?,
                        "key_file",
                    )
                    .into(),
                    format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd()).into(),
                ));
                ssh.push(file);
            }
            if ssh.is_empty() {
                return Ok(None);
            }
        } else if root.get("state").and_then(Value::as_str) != Some("portable") {
            return Ok(None);
        }
        let locked = LockedConfig::acquire(state, cipher, cancel)?;
        let text = zeroize::Zeroizing::new(
            cipher
                .decrypt_value(
                    locked
                        .state
                        .get("files")
                        .and_then(Value::as_str)
                        .ok_or(Invalid)?,
                )
                .map_err(|_| Invalid)?,
        );
        let files: Value = serde_json::from_str(&text).map_err(|_| Invalid)?;
        let material = MaterializedFiles::create(
            &files,
            locked.state.get("file_refs").ok_or(Invalid)?,
            cancel,
        )?;
        let text = zeroize::Zeroizing::new(
            cipher
                .decrypt_value(
                    locked
                        .state
                        .get("overrides")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                )
                .map_err(|_| Invalid)?,
        );
        let overrides: Value = serde_json::from_str(if text.is_empty() { "{}" } else { &text })
            .map_err(|_| Invalid)?;
        let mut override_env = Vec::new();
        for (name, fields) in overrides.as_object().ok_or(Invalid)? {
            for (field, value) in fields.as_object().ok_or(Invalid)? {
                override_env.push((
                    environment_name(name, field).into(),
                    value.as_str().ok_or(Invalid)?.into(),
                ));
            }
        }
        let caches: Vec<&str> = locked
            .state
            .get("cache_remotes")
            .map_or(Ok(Vec::new()), |v| {
                v.as_array()
                    .ok_or(Invalid)?
                    .iter()
                    .map(|v| v.as_str().ok_or(Invalid))
                    .collect()
            })?;
        check(cancel)?;
        let cache = cache(&locked.generation)?;
        let path = cache.child_path();
        for key in ["HOME", "XDG_CACHE_HOME", "TMPDIR"] {
            environment.push((key.into(), path.clone().into_os_string()));
        }
        environment.push(("AWS_EC2_METADATA_DISABLED".into(), "true".into()));
        environment.push(("AWS_CONFIG_FILE".into(), "/dev/null".into()));
        for name in caches {
            use sha2::{Digest, Sha256};
            let id: String = Sha256::digest(name.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let private = path.join(id);
            for (field, value) in [
                ("db_path", private.join("metadata").into_os_string()),
                ("chunk_path", private.join("chunks").into_os_string()),
                ("tmp_upload_path", OsString::new()),
            ] {
                environment.push((environment_name(name, field).into(), value));
            }
        }
        environment.extend(override_env);
        environment.extend(material.environment.iter().cloned());
        Ok(Some(Self {
            locked,
            cache,
            _material: material,
            _ssh: ssh,
            environment,
        }))
    }
}
impl ManagedConfigBuilder {
    /// Trusted internal fixed-argv storage operations only; caller owns durable
    /// operation/claims until this work settles (including errors/cancellation).
    pub fn start_run_managed(
        &self,
        state: Value,
        cipher: Arc<CredentialCipher>,
        remote: String,
        arguments: Vec<OsString>,
        timeout: Option<Duration>,
        allowed_ssh_key_dirs: Vec<PathBuf>,
    ) -> std::result::Result<Work<Result<ManagedCommandOutput>>, WorkError> {
        let this = self.clone();
        self.tasks.submit_async(move |cancel| async move {
            let check = cancel.clone();
            let context = this
                .blocking
                .submit_blocking(move |physical| {
                    if physical.is_cancelled() {
                        return Err(Cancelled);
                    }
                    Runtime::prepare(state, &cipher, &remote, &allowed_ssh_key_dirs, &check)
                })
                .map_err(|_| Invalid)?
                .wait()
                .await
                .map_err(|_| Invalid)??;
            let Some(context) = context else {
                return Ok(ManagedCommandOutput::credentials());
            };
            let mut spec = this.spec(
                &context.locked.generation,
                &context.locked.unlock,
                ["--timeout", "300s", "--contimeout", "30s"],
            );
            spec.args.extend(arguments);
            spec.timeout = timeout;
            spec.env.extend(context.environment.iter().cloned());
            let result = this.capture_raw(spec, &cancel).await;
            this.blocking
                .submit_blocking(move |_| {
                    context.cache.finish(false)?;
                    let output = match result {
                        Err(Cancelled) => return Err(Cancelled),
                        Err(_) => ManagedCommandOutput::unavailable(),
                        Ok(output) if output.status.success() => {
                            match String::from_utf8(output.stdout) {
                                Ok(stdout) => ManagedCommandOutput {
                                    code: 0,
                                    stdout,
                                    stderr: "",
                                },
                                Err(_) => ManagedCommandOutput::unavailable(),
                            }
                        }
                        Ok(output) => ManagedCommandOutput {
                            code: output
                                .status
                                .code()
                                .unwrap_or_else(|| -output.status.signal().unwrap_or(1)),
                            stdout: String::new(),
                            stderr: "Remote storage unavailable",
                        },
                    };
                    Ok(output)
                })
                .map_err(|_| Invalid)?
                .wait()
                .await
                .map_err(|_| Invalid)?
        })
    }
}
