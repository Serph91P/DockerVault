//! Path handling shared by the SSH and SFTP transports.
use super::{
    VerifiedSource,
    local::{CliDownload, Downloaded, TransferError},
};
use crate::{
    ownership::{Owner, Work, WorkError},
    process::{ProcessError, ProcessOutput, ProcessOwner, ProcessSpec},
};
use std::{
    ffi::OsString,
    fs::File,
    io,
    net::Ipv6Addr,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub enum SftpDownloadError {
    Admission(WorkError),
    Process(ProcessError),
    Transfer(TransferError),
    ProviderRejected,
    InvalidListing,
    MutationUnverified,
}

#[derive(Debug, serde::Serialize)]
pub struct SftpEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub modified: f64,
}

#[derive(Clone)]
pub struct SftpEndpoint {
    pub host: String,
    pub username: String,
    pub port: u16,
    pub base: String,
    pub known_hosts: PathBuf,
    pub identity: Option<PathBuf>,
    pub password: Option<String>,
}

pub(crate) struct UploadStage {
    pub nonce: u128,
    pub identity: Option<File>,
    pub created: bool,
}

pub(crate) fn stage_dir(child: &str, nonce: u128) -> String {
    let leaf = format!(".dockervault-{nonce:032x}");
    match child.rsplit_once('/') {
        Some((parent, _)) => format!("{parent}/{leaf}"),
        None => leaf,
    }
}

pub(crate) fn key_file(path: &Path, allowed: &[PathBuf]) -> Option<File> {
    let path = path.canonicalize().ok()?;
    for root in allowed {
        let Ok(root) = root.canonicalize() else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let held = super::local::open_root(&root).ok()?;
        let mut parts = relative.components().peekable();
        let mut directory = held;
        while let Some(part) = parts.next() {
            let std::path::Component::Normal(name) = part else {
                return None;
            };
            use nix::fcntl::{OFlag, openat};
            let mut flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
            flags |= if parts.peek().is_some() {
                OFlag::O_DIRECTORY
            } else {
                OFlag::O_NONBLOCK
            };
            directory =
                File::from(openat(&directory, name, flags, nix::sys::stat::Mode::empty()).ok()?);
        }
        if directory.metadata().ok()?.is_file() {
            return Some(directory);
        }
    }
    None
}

impl SftpEndpoint {
    pub(crate) async fn install_public_key(
        &self,
        processes: &ProcessOwner,
        command: &str,
        public_key: Vec<u8>,
        cancel: &CancellationToken,
    ) -> Result<(), SftpDownloadError> {
        let (mut args, destination) = self
            .connection_args("-p")
            .map_err(SftpDownloadError::Process)?;
        args.extend([
            "-oNumberOfPasswordPrompts=1".into(),
            "--".into(),
            destination.into(),
            command.into(),
        ]);
        let mut spec = self.wrap_password("/usr/bin/ssh", args);
        spec.input = public_key;
        spec.timeout = Some(Duration::from_secs(60));
        spec.discard_output = true;
        run_batch(processes, spec, cancel).await.map(|_| ())
    }

    /// The process exit status, not command admission, reports the probe result.
    pub fn probe(
        &self,
        processes: &ProcessOwner,
    ) -> Result<Work<Result<ProcessOutput, ProcessError>>, ProcessError> {
        let directory = remote_path(&self.host, self.port, &self.base, "")
            .map_err(|_| ProcessError::InvalidSpec)?;
        let directory = batch_path(&directory);
        let mut spec = self.batch_spec()?;
        spec.input = format!("@cd {directory}\n@quit\n").into_bytes();
        spec.timeout = Some(Duration::from_secs(45));
        processes.start(spec)
    }

    /// API connection test: authenticate and enter the SFTP subsystem without changing directories.
    pub(crate) fn probe_handshake(
        &self,
        processes: &ProcessOwner,
        held_key: Option<File>,
    ) -> Result<Work<Result<ProcessOutput, ProcessError>>, ProcessError> {
        let mut spec = self.batch_spec()?;
        spec.input = b"@quit\n".to_vec();
        spec.timeout = Some(Duration::from_secs(45));
        spec.discard_output = true;
        processes.start_holding(spec, held_key)
    }

    pub fn download(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        root: File,
        destination: PathBuf,
    ) -> Result<Work<Result<Downloaded, SftpDownloadError>>, SftpDownloadError> {
        self.download_with(processes, workers, child, root, destination, false)
    }

    pub(crate) fn download_snapshot<T: Send + 'static>(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        held_key: Option<File>,
        held: T,
    ) -> Result<Work<Result<Snapshot<T>, SftpDownloadError>>, SftpDownloadError> {
        use std::os::fd::AsRawFd;
        let remote = remote_path(&self.host, self.port, &self.base, child)
            .map_err(|_| SftpDownloadError::Process(ProcessError::InvalidSpec))?;
        let remote = batch_path(&remote);
        let mut spec = self.batch_spec().map_err(SftpDownloadError::Process)?;
        spec.timeout = Some(Duration::from_secs(600));
        let processes = processes.clone();
        workers
            .submit_async(move |cancel| async move {
                let file = tokio::task::spawn_blocking(super::local::anonymous_download)
                    .await
                    .map_err(|_| SftpDownloadError::Admission(WorkError::WorkerFailed))?
                    .map_err(|error| SftpDownloadError::Transfer(error.into()))?;
                spec.input = format!(
                    "@get {remote} /proc/{}/fd/{}\n@quit\n",
                    std::process::id(),
                    file.as_raw_fd()
                )
                .into_bytes();
                run_batch_holding(&processes, spec, &cancel, held_key).await?;
                tokio::task::spawn_blocking(move || {
                    if cancel.is_cancelled() {
                        return Err(SftpDownloadError::Process(ProcessError::Cancelled));
                    }
                    let length = file
                        .metadata()
                        .map_err(|error| SftpDownloadError::Transfer(error.into()))?
                        .len();
                    Ok((file, length, held))
                })
                .await
                .map_err(|_| SftpDownloadError::Admission(WorkError::WorkerFailed))?
            })
            .map_err(SftpDownloadError::Admission)
    }

    pub fn download_rsync(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        root: File,
        destination: PathBuf,
    ) -> Result<Work<Result<Downloaded, SftpDownloadError>>, SftpDownloadError> {
        self.download_with(processes, workers, child, root, destination, true)
    }

    fn download_with(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        root: File,
        destination: PathBuf,
        rsync: bool,
    ) -> Result<Work<Result<Downloaded, SftpDownloadError>>, SftpDownloadError> {
        let invalid = || SftpDownloadError::Process(ProcessError::InvalidSpec);
        if matches!(child.rsplit('/').next(), None | Some("" | "." | "..")) {
            return Err(invalid());
        }
        let path = remote_path(&self.host, self.port, &self.base, child).map_err(|_| invalid())?;
        let (mut spec, remote) = if rsync {
            self.rsync_spec().map(|(spec, destination)| {
                (
                    spec,
                    format!(
                        "{destination}:{}{}",
                        if path.starts_with('/') { "" } else { "./" },
                        rsync_literal(&path)
                    ),
                )
            })
        } else {
            self.batch_spec().map(|spec| (spec, batch_path(&path)))
        }
        .map_err(SftpDownloadError::Process)?;
        let processes = processes.clone();
        workers
            .submit_async(move |cancel| async move {
                let staging =
                    tokio::task::spawn_blocking(move || CliDownload::prepare(root, &destination))
                        .await
                        .map_err(|_| SftpDownloadError::Admission(WorkError::WorkerFailed))?
                        .map_err(|error| SftpDownloadError::Transfer(error.into()))?;
                if cancel.is_cancelled() {
                    return Err(SftpDownloadError::Process(ProcessError::Cancelled));
                }
                if rsync {
                    spec.args.extend([
                        "--inplace".into(),
                        "--out-format=%i".into(),
                        "--".into(),
                        remote.into(),
                        staging.inplace_path().into(),
                    ]);
                    let result = run_batch(&processes, spec, &cancel).await?;
                    // An exit of zero can also mean a directory was skipped.
                    let item = result.stdout.strip_suffix(b"\n").unwrap_or_default();
                    if item.len() != 11 || !item.starts_with(b">f") {
                        return Err(SftpDownloadError::ProviderRejected);
                    }
                } else {
                    spec.input = format!("@get {remote} {}\n@quit\n", staging.descriptor_path())
                        .into_bytes();
                    run_batch(&processes, spec, &cancel).await?;
                }
                tokio::task::spawn_blocking(move || staging.finish(&|| cancel.is_cancelled()))
                    .await
                    .map_err(|_| SftpDownloadError::Admission(WorkError::WorkerFailed))?
                    .map_err(SftpDownloadError::Transfer)
            })
            .map_err(SftpDownloadError::Admission)
    }

    pub fn upload(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        source: Arc<VerifiedSource>,
        scratch: File,
    ) -> Result<Work<Result<(), SftpDownloadError>>, SftpDownloadError> {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| {
            SftpDownloadError::Transfer(
                io::Error::other("SFTP staging identity unavailable").into(),
            )
        })?;
        self.upload_staged(
            processes,
            workers,
            child,
            source,
            scratch,
            UploadStage {
                nonce: u128::from_be_bytes(nonce),
                identity: None,
                created: false,
            },
        )
    }

    pub(crate) fn upload_staged(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        source: Arc<VerifiedSource>,
        scratch: File,
        stage: UploadStage,
    ) -> Result<Work<Result<(), SftpDownloadError>>, SftpDownloadError> {
        self.upload_with(processes, workers, child, source, scratch, Some(stage))
    }

    pub fn upload_rsync(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        source: Arc<VerifiedSource>,
        scratch: File,
    ) -> Result<Work<Result<(), SftpDownloadError>>, SftpDownloadError> {
        self.upload_with(processes, workers, child, source, scratch, None)
    }

    fn upload_with(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        source: Arc<VerifiedSource>,
        scratch: File,
        stage: Option<UploadStage>,
    ) -> Result<Work<Result<(), SftpDownloadError>>, SftpDownloadError> {
        let invalid = || SftpDownloadError::Process(ProcessError::InvalidSpec);
        if matches!(child.rsplit('/').next(), None | Some("" | "." | "..")) {
            return Err(invalid());
        }
        let path = remote_path(&self.host, self.port, &self.base, child).map_err(|_| invalid())?;
        let remote = batch_path(&path);
        let mut get = self.batch_spec().map_err(SftpDownloadError::Process)?;
        let endpoint = self.clone();
        let child = child.to_owned();
        let processes = processes.clone();
        workers
            .submit_async(move |cancel| async move {
                let (_identity, staging) = stage.map_or((None, None), |stage| (stage.identity, Some((stage.nonce, stage.created))));
                let pinned = source.clone();
                let (input, verification) = tokio::task::spawn_blocking(move || {
                    Ok::<_, io::Error>((
                        pinned.reopen()?,
                        CliDownload::prepare(scratch, Path::new("unused"))?,
                    ))
                })
                .await
                .map_err(|_| SftpDownloadError::Admission(WorkError::WorkerFailed))?
                .map_err(|error| SftpDownloadError::Transfer(error.into()))?;
                if cancel.is_cancelled() {
                    return Err(SftpDownloadError::Process(ProcessError::Cancelled));
                }
                let local = format!("/proc/{}/fd/{}", std::process::id(), input.as_raw_fd());
                let put = if let Some((nonce, created)) = staging {
                    let mut spec = endpoint.batch_spec().map_err(SftpDownloadError::Process)?;
                    let stage = remote_path(&endpoint.host, endpoint.port, &endpoint.base, &stage_dir(&child, nonce)).map_err(|_| SftpDownloadError::Process(ProcessError::InvalidSpec))?;
                    let temporary = batch_path(&format!("{stage}/artifact"));
                    let stage = batch_path(&stage);
                    let batch = if created { String::new() } else { endpoint.mkdir_batch(&child, &stage)? };
                    // OpenSSH's plain rename uses posix-rename when advertised and can replace the destination.
                    spec.input = format!("{batch}@put {local} {temporary}\n@rename -l {temporary} {remote}\n@rmdir {stage}\n@quit\n").into_bytes();
                    spec.timeout = Some(Duration::from_secs(600));
                    spec
                } else {
                    endpoint
                        .rsync_put_spec(&path, &local)
                        .map_err(SftpDownloadError::Process)?
                };
                run_batch(&processes, put, &cancel)
                    .await
                    .map_err(|_| SftpDownloadError::MutationUnverified)?;
                drop(input);
                get.input = format!("@get {remote} {}\n@quit\n", verification.descriptor_path())
                    .into_bytes();
                get.timeout = Some(Duration::from_secs(600));
                run_batch(&processes, get, &cancel)
                    .await
                    .map_err(|_| SftpDownloadError::MutationUnverified)?;
                tokio::task::spawn_blocking(move || {
                    verification.verify(&source, &|| cancel.is_cancelled())
                })
                .await
                .map_err(|_| SftpDownloadError::MutationUnverified)?
                .map_err(|_| SftpDownloadError::MutationUnverified)
            })
            .map_err(SftpDownloadError::Admission)
    }

    fn mkdir_batch(&self, child: &str, stage: &str) -> Result<String, SftpDownloadError> {
        let mut batch = String::new();
        for (end, _) in child.match_indices('/').filter(|(end, _)| *end != 0) {
            let dir = remote_path(&self.host, self.port, &self.base, &child[..end])
                .map_err(|_| SftpDownloadError::Process(ProcessError::InvalidSpec))?;
            batch.push_str(&format!("@-mkdir {}\n", batch_path(&dir)));
        }
        batch.push_str(&format!("@mkdir {stage}\n"));
        Ok(batch)
    }

    pub(crate) async fn stage_directory(
        &self,
        processes: &ProcessOwner,
        child: &str,
        nonce: u128,
        cleanup: bool,
        cancel: &CancellationToken,
    ) -> Result<(), SftpDownloadError> {
        let dir = remote_path(&self.host, self.port, &self.base, &stage_dir(child, nonce))
            .map_err(|_| SftpDownloadError::Process(ProcessError::InvalidSpec))?;
        let stage = batch_path(&dir);
        let batch = if cleanup {
            format!(
                "@rm {}\n@rmdir {stage}\n",
                batch_path(&format!("{dir}/artifact"))
            )
        } else {
            self.mkdir_batch(child, &stage)?
        };
        let mut spec = self.batch_spec().map_err(SftpDownloadError::Process)?;
        spec.input = format!("{batch}@quit\n").into_bytes();
        spec.timeout = Some(Duration::from_secs(45));
        run_batch(processes, spec, cancel).await.map(|_| ())
    }

    pub fn delete(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
    ) -> Result<Work<Result<(), SftpDownloadError>>, SftpDownloadError> {
        let invalid = || SftpDownloadError::Process(ProcessError::InvalidSpec);
        if matches!(child.rsplit('/').next(), None | Some("" | "." | "..")) {
            return Err(invalid());
        }
        let remote =
            remote_path(&self.host, self.port, &self.base, child).map_err(|_| invalid())?;
        let mut spec = self.batch_spec().map_err(SftpDownloadError::Process)?;
        spec.input = format!("@rm {}\n@quit\n", batch_path(&remote)).into_bytes();
        let processes = processes.clone();
        workers
            .submit_async(move |cancel| async move {
                if cancel.is_cancelled() {
                    return Err(SftpDownloadError::Process(ProcessError::Cancelled));
                }
                run_batch(&processes, spec, &cancel)
                    .await
                    .map(|_| ())
                    .map_err(|_| SftpDownloadError::MutationUnverified)
            })
            .map_err(SftpDownloadError::Admission)
    }

    pub fn list(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
    ) -> Result<Work<Result<Vec<SftpEntry>, SftpDownloadError>>, SftpDownloadError> {
        self.list_holding(processes, workers, child, None)
    }

    pub(crate) fn list_holding(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        held_key: Option<std::fs::File>,
    ) -> Result<Work<Result<Vec<SftpEntry>, SftpDownloadError>>, SftpDownloadError> {
        let remote = remote_path(&self.host, self.port, &self.base, child)
            .map_err(|_| SftpDownloadError::Process(ProcessError::InvalidSpec))?;
        let mut spec = self.batch_spec().map_err(SftpDownloadError::Process)?;
        spec.input = format!("@cd {}\n@ls -lan\n@quit\n", batch_path(&remote)).into_bytes();
        spec.stdout_limit = 8 * 1024 * 1024;
        spec.timeout = Some(Duration::from_secs(45));
        spec.env.push(("TZ".into(), "UTC".into()));
        let processes = processes.clone();
        workers
            .submit_async(move |cancel| async move {
                let output = run_batch_holding(&processes, spec, &cancel, held_key).await?;
                parse_listing(&output.stdout)
            })
            .map_err(SftpDownloadError::Admission)
    }

    pub(crate) async fn quota_upper_bound(
        &self,
        processes: &ProcessOwner,
        cancel: &CancellationToken,
        key: Option<File>,
    ) -> Option<u64> {
        let remote = remote_path(&self.host, self.port, &self.base, "").ok()?;
        let mut spec = self.batch_spec().ok()?;
        spec.input = format!("@df {}\n@quit\n", batch_path(&remote)).into_bytes();
        spec.stdout_limit = 4096;
        spec.timeout = Some(Duration::from_secs(10));
        spec.env.push(("LC_ALL".into(), "C".into()));
        let output = run_batch_holding(processes, spec, cancel, key).await.ok()?;
        available_kib(&output.stdout)?
            .checked_mul(1024)?
            .checked_add(1023)
    }

    fn batch_spec(&self) -> Result<ProcessSpec, ProcessError> {
        let (mut args, destination) = self.connection_args("-P")?;
        args.extend(["-b".into(), "-".into(), "--".into(), destination.into()]);
        Ok(self.wrap_password("/usr/bin/sftp", args))
    }

    fn rsync_put_spec(&self, path: &str, local: &str) -> Result<ProcessSpec, ProcessError> {
        let (mut spec, destination) = self.rsync_spec()?;
        spec.args.extend([
            "-tL".into(),
            "--mkpath".into(),
            "--".into(),
            local.into(),
            format!("{destination}:{path}").into(),
        ]);
        Ok(spec)
    }

    fn rsync_spec(&self) -> Result<(ProcessSpec, String), ProcessError> {
        let (ssh, destination) = self.connection_args("-p")?;
        let mut command = "/usr/bin/ssh".to_owned();
        for arg in ssh {
            let arg = arg.to_str().ok_or(ProcessError::InvalidSpec)?;
            // rsync parses -e itself; doubling its delimiter quotes a literal quote.
            command.push_str(" '");
            command.push_str(&arg.replace('\'', "''"));
            command.push('\'');
        }
        Ok((
            self.wrap_password(
                "/usr/bin/rsync",
                vec![
                    "-z".into(),
                    "--ignore-times".into(),
                    "--secluded-args".into(),
                    "-e".into(),
                    command.into(),
                ],
            ),
            destination,
        ))
    }

    fn connection_args(&self, port_flag: &str) -> Result<(Vec<OsString>, String), ProcessError> {
        let host = self
            .host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(&self.host);
        let destination = if host.parse::<Ipv6Addr>().is_ok() {
            format!("{}@[{host}]", self.username)
        } else if valid_endpoint_atom(host, b".-_") {
            format!("{}@{host}", self.username)
        } else {
            return Err(ProcessError::InvalidSpec);
        };
        let known_hosts = self
            .known_hosts
            .to_str()
            .filter(|path| {
                self.known_hosts.is_absolute()
                    && !path.chars().any(|c| {
                        c.is_whitespace() || c.is_control() || matches!(c, '%' | '$' | '"' | '\\')
                    })
            })
            .ok_or(ProcessError::InvalidSpec)?;
        if self.port == 0 || !valid_endpoint_atom(&self.username, b"-_.") {
            return Err(ProcessError::InvalidSpec);
        }

        let mut args = vec![
            OsString::from("-F"),
            "/dev/null".into(),
            "-oStrictHostKeyChecking=accept-new".into(),
            format!("-oUserKnownHostsFile={known_hosts}").into(),
            "-oConnectTimeout=30".into(),
            "-oServerAliveInterval=15".into(),
            port_flag.into(),
            self.port.to_string().into(),
        ];
        if let Some(identity) = &self.identity {
            args.extend([
                "-oIdentitiesOnly=yes".into(),
                "-i".into(),
                identity.as_os_str().into(),
            ]);
        }
        if self.password_auth().is_some() {
            args.extend([
                "-oBatchMode=no".into(),
                "-oPreferredAuthentications=password".into(),
                "-oPubkeyAuthentication=no".into(),
            ]);
        }
        Ok((args, destination))
    }

    fn password_auth(&self) -> Option<&str> {
        if self.identity.is_none() {
            self.password.as_deref().filter(|value| !value.is_empty())
        } else {
            None
        }
    }

    fn wrap_password(&self, program: &str, args: Vec<OsString>) -> ProcessSpec {
        if let Some(password) = self.password_auth() {
            let mut wrapped = vec![OsString::from("-e"), program.into()];
            wrapped.extend(args);
            let mut spec = ProcessSpec::new("/usr/bin/sshpass", wrapped);
            spec.env.push(("SSHPASS".into(), password.into()));
            spec
        } else {
            ProcessSpec::new(program, args)
        }
    }
}

type Snapshot<T> = (File, u64, T);

async fn run_batch(
    processes: &ProcessOwner,
    spec: ProcessSpec,
    cancel: &CancellationToken,
) -> Result<ProcessOutput, SftpDownloadError> {
    run_batch_holding(processes, spec, cancel, None).await
}

async fn run_batch_holding(
    processes: &ProcessOwner,
    spec: ProcessSpec,
    cancel: &CancellationToken,
    held_key: Option<std::fs::File>,
) -> Result<ProcessOutput, SftpDownloadError> {
    if cancel.is_cancelled() {
        return Err(SftpDownloadError::Process(ProcessError::Cancelled));
    }
    let process = processes
        .start_holding(spec, held_key)
        .map_err(SftpDownloadError::Process)?;
    let process_cancel = process.cancellation_token();
    let mut reply = std::pin::pin!(process.wait());
    let settled = tokio::select! { biased;
        _ = cancel.cancelled() => {
            process_cancel.cancel();
            reply.await
        }
        result = &mut reply => result,
    };
    let output = settled
        .map_err(SftpDownloadError::Admission)?
        .map_err(SftpDownloadError::Process)?;
    if !output.status.success() {
        return Err(SftpDownloadError::ProviderRejected);
    }
    Ok(output)
}

fn rsync_literal(path: &str) -> String {
    if !path.contains(['[', '*', '?']) {
        return path.into();
    }
    let mut escaped = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '\\' | '[' | '*' | '?') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn batch_path(path: &str) -> String {
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    if path.starts_with('-') {
        quoted.push_str("./");
    }
    for c in path.chars() {
        // OpenSSH handles glob characters within quoted paths itself.
        if matches!(c, '\\' | '"') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

fn parse_listing(bytes: &[u8]) -> Result<Vec<SftpEntry>, SftpDownloadError> {
    use super::ftp::listing::unix_listing_time;
    let invalid = || SftpDownloadError::InvalidListing;
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut entries = Vec::new();
    for line in text.split_inclusive('\n') {
        let mut rest = line.strip_suffix('\n').ok_or_else(invalid)?;
        let mut next = || {
            let (token, tail) = rest
                .trim_start_matches(' ')
                .split_once(' ')
                .ok_or_else(invalid)?;
            rest = tail;
            Ok::<_, SftpDownloadError>(token)
        };
        let kind = next()?.as_bytes();
        if kind.len() != 10 || !matches!(kind[0], b'-' | b'd' | b'l' | b'b' | b'c' | b'p' | b's') {
            return Err(invalid());
        }
        for _ in 0..3 {
            next()?;
        }
        let size = next()?.parse().map_err(|_| invalid())?;
        let month = next()?;
        let day = next()?.parse().map_err(|_| invalid())?;
        let clock = next()?;
        let modified = unix_listing_time(month, day, clock).map_err(|_| invalid())? as f64;
        if matches!(rest, "." | "..") {
            continue;
        }
        if rest.is_empty() || rest.contains('/') || rest.chars().any(char::is_control) {
            return Err(invalid());
        }
        entries.push(SftpEntry {
            name: rest.into(),
            size,
            is_dir: kind[0] == b'd',
            modified,
        });
    }
    Ok(entries)
}

fn valid_endpoint_atom(value: &str, punctuation: &[u8]) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || punctuation.contains(&b))
}

/// Resolve a child relative to the configured remote directory. A leading slash
/// on a child does not replace the configured base.
pub fn remote_path(host: &str, port: u16, base: &str, child: &str) -> io::Result<String> {
    if [base, child]
        .iter()
        .any(|path| path.chars().any(char::is_control))
        || child.split('/').any(|component| component == "..")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid SSH path",
        ));
    }
    let mut base = base.trim();
    if port == 23 && host.to_ascii_lowercase().ends_with(".your-storagebox.de") {
        base = base.trim_end_matches('/');
        base = match base {
            "" | "home" | "/home" | "backups" | "/backups" => ".",
            _ => base
                .strip_prefix("/home/")
                .unwrap_or_else(|| base.trim_start_matches('/')),
        };
    }
    let child = child.trim_start_matches('/');
    match (base, child) {
        ("", "") => Ok(".".into()),
        (_, "") => Ok(base.into()),
        ("" | ".", _) => Ok(child.into()),
        _ => Ok(format!("{}/{child}", base.trim_end_matches('/'))),
    }
}

fn available_kib(output: &[u8]) -> Option<u64> {
    let mut lines = std::str::from_utf8(output)
        .ok()?
        .lines()
        .filter(|line| !line.trim().is_empty());
    if !lines
        .next()?
        .split_ascii_whitespace()
        .eq(["Size", "Used", "Avail", "(root)", "%Capacity"])
    {
        return None;
    }
    let values: Vec<_> = lines.next()?.split_ascii_whitespace().collect();
    if lines.next().is_some()
        || values.len() != 5
        || !values[4].ends_with('%')
        || !values[..4]
            .iter()
            .all(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    values[2].parse().ok()
}

#[cfg(test)]
mod listing_tests {
    use super::*;

    #[test]
    fn quota_only_accepts_the_user_available_column_in_openssh_output() {
        assert_eq!(
            available_kib(b"Size Used Avail (root) %Capacity\n100 98 1 2 98%\n"),
            Some(1)
        );
        assert_eq!(
            available_kib(b"Size Used Avail (root) %Capacity\n100 98 1 2 98%\nunexpected\n"),
            None
        );
        assert_eq!(available_kib(b"unsupported extension\n"), None);
    }

    #[test]
    fn rsync_upload_keeps_remote_path_as_one_protected_argument() {
        let endpoint = SftpEndpoint {
            host: "::1".into(),
            username: "backup".into(),
            port: 23,
            base: "/backups".into(),
            known_hosts: "/app/data/ssh_known_hosts".into(),
            identity: Some("/app/data/keys/a key".into()),
            password: Some("synthetic".into()),
        };
        let spec = endpoint
            .rsync_put_spec("/backups/dir [1]/archive.tar.gz", "/proc/123/fd/4")
            .unwrap();
        assert_eq!(spec.program, PathBuf::from("/usr/bin/rsync"));
        assert_eq!(
            spec.args.last().unwrap(),
            "backup@[::1]:/backups/dir [1]/archive.tar.gz"
        );
        assert!(spec.args.iter().any(|arg| arg == "--secluded-args"));
        assert!(spec.args.iter().any(|arg| arg == "--mkpath"));
        assert!(spec.env.is_empty());
    }

    #[test]
    fn numeric_listing_keeps_literal_names_and_rejects_partial_inventory() {
        let bytes = b"drwxr-xr-x    ? root root 4096 Nov 18 2020 .\n-rw-r--r--    ? root root 17 Nov 18 2020  archive [1].key \nlrwxrwxrwx    ? root root 4 Nov 18 2020 link\n";
        let entries = parse_listing(bytes).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, " archive [1].key ");
        assert_eq!(entries[0].size, 17);
        assert_eq!(entries[0].modified, 1_605_657_600.0);
        assert_eq!(entries[1].name, "link");
        assert!(parse_listing(&bytes[..bytes.len() - 1]).is_err());
        assert!(parse_listing(b"unrecognized provider text\n").is_err());
    }
}
