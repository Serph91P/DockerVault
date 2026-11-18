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

#[derive(Debug)]
pub struct SftpEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub modified: f64,
}

pub struct SftpEndpoint {
    pub host: String,
    pub username: String,
    pub port: u16,
    pub base: String,
    pub known_hosts: PathBuf,
    pub identity: Option<PathBuf>,
    pub password: Option<String>,
}

impl SftpEndpoint {
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

    pub fn download(
        &self,
        processes: &ProcessOwner,
        workers: &Owner,
        child: &str,
        root: File,
        destination: PathBuf,
    ) -> Result<Work<Result<Downloaded, SftpDownloadError>>, SftpDownloadError> {
        let invalid = || SftpDownloadError::Process(ProcessError::InvalidSpec);
        if child.trim_start_matches('/').is_empty() {
            return Err(invalid());
        }
        let path = remote_path(&self.host, self.port, &self.base, child).map_err(|_| invalid())?;
        let remote = batch_path(&path);
        let mut spec = self.batch_spec().map_err(SftpDownloadError::Process)?;
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
                spec.input =
                    format!("@get {remote} {}\n@quit\n", staging.descriptor_path()).into_bytes();
                run_batch(&processes, spec, &cancel).await?;
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
        let invalid = || SftpDownloadError::Process(ProcessError::InvalidSpec);
        if child.trim_matches('/').is_empty() || child.ends_with('/') {
            return Err(invalid());
        }
        let path = remote_path(&self.host, self.port, &self.base, child).map_err(|_| invalid())?;
        let remote = batch_path(&path);
        let mut put = self.batch_spec().map_err(SftpDownloadError::Process)?;
        let mut get = self.batch_spec().map_err(SftpDownloadError::Process)?;
        let processes = processes.clone();
        workers
            .submit_async(move |cancel| async move {
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
                put.input = format!(
                    "@put /proc/{}/fd/{} {remote}\n@quit\n",
                    std::process::id(),
                    input.as_raw_fd()
                )
                .into_bytes();
                run_batch(&processes, put, &cancel)
                    .await
                    .map_err(|_| SftpDownloadError::MutationUnverified)?;
                drop(input);
                get.input = format!("@get {remote} {}\n@quit\n", verification.descriptor_path())
                    .into_bytes();
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
                let output = run_batch(&processes, spec, &cancel).await?;
                parse_listing(&output.stdout)
            })
            .map_err(SftpDownloadError::Admission)
    }

    fn batch_spec(&self) -> Result<ProcessSpec, ProcessError> {
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
        let password = if self.identity.is_none() {
            self.password.as_deref().filter(|value| !value.is_empty())
        } else {
            None
        };
        let mut args = vec![
            OsString::from("-F"),
            "/dev/null".into(),
            "-oStrictHostKeyChecking=accept-new".into(),
            format!("-oUserKnownHostsFile={known_hosts}").into(),
            "-oConnectTimeout=30".into(),
            "-oServerAliveInterval=15".into(),
            "-P".into(),
            self.port.to_string().into(),
        ];
        if let Some(identity) = &self.identity {
            args.extend([
                "-oIdentitiesOnly=yes".into(),
                "-i".into(),
                identity.as_os_str().into(),
            ]);
        }
        if password.is_some() {
            args.extend([
                "-oBatchMode=no".into(),
                "-oPreferredAuthentications=password".into(),
                "-oPubkeyAuthentication=no".into(),
            ]);
        }
        args.extend(["-b".into(), "-".into(), "--".into(), destination.into()]);
        Ok(if let Some(password) = password {
            let mut wrapped = vec![OsString::from("-e"), "/usr/bin/sftp".into()];
            wrapped.extend(args);
            let mut spec = ProcessSpec::new("/usr/bin/sshpass", wrapped);
            spec.env.push(("SSHPASS".into(), password.into()));
            spec
        } else {
            ProcessSpec::new("/usr/bin/sftp", args)
        })
    }
}

async fn run_batch(
    processes: &ProcessOwner,
    spec: ProcessSpec,
    cancel: &CancellationToken,
) -> Result<ProcessOutput, SftpDownloadError> {
    if cancel.is_cancelled() {
        return Err(SftpDownloadError::Process(ProcessError::Cancelled));
    }
    let process = processes.start(spec).map_err(SftpDownloadError::Process)?;
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

#[cfg(test)]
mod listing_tests {
    use super::*;

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
