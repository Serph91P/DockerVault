//! Path handling shared by the SSH and SFTP transports.
use super::local::{CliDownload, Downloaded, TransferError};
use crate::{
    ownership::{Owner, Work, WorkError},
    process::{ProcessError, ProcessOutput, ProcessOwner, ProcessSpec},
};
use std::{ffi::OsString, fs::File, io, net::Ipv6Addr, path::PathBuf, time::Duration};

#[derive(Debug)]
pub enum SftpDownloadError {
    Admission(WorkError),
    Process(ProcessError),
    Transfer(TransferError),
    ProviderRejected,
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
                tokio::task::spawn_blocking(move || staging.finish(&|| cancel.is_cancelled()))
                    .await
                    .map_err(|_| SftpDownloadError::Admission(WorkError::WorkerFailed))?
                    .map_err(SftpDownloadError::Transfer)
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
