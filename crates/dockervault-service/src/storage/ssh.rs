//! Path handling shared by the SSH and SFTP transports.
use crate::{
    ownership::Work,
    process::{ProcessError, ProcessOutput, ProcessOwner, ProcessSpec},
};
use std::{ffi::OsString, io, net::Ipv6Addr, path::PathBuf, time::Duration};

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
        let mut spec = if let Some(password) = password {
            let mut wrapped = vec![OsString::from("-e"), "/usr/bin/sftp".into()];
            wrapped.extend(args);
            let mut spec = ProcessSpec::new("/usr/bin/sshpass", wrapped);
            spec.env.push(("SSHPASS".into(), password.into()));
            spec
        } else {
            ProcessSpec::new("/usr/bin/sftp", args)
        };
        let directory = if directory.starts_with('-') {
            format!("./{directory}")
        } else {
            directory
        };
        let mut quoted = String::with_capacity(directory.len());
        for c in directory.chars() {
            // OpenSSH's quoted-argument parser handles glob characters for cd.
            if matches!(c, '\\' | '"') {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        spec.input = format!("@cd \"{quoted}\"\n@quit\n").into_bytes();
        spec.timeout = Some(Duration::from_secs(45));
        processes.start(spec)
    }
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
