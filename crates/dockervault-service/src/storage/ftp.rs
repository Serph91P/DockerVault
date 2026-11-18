//! Plain FTP control sessions. No TLS, clear-text credential logging or detached session tasks.
use crate::ownership::{Owner, Work, WorkError};
mod download;
pub(super) mod listing;
mod upload;
pub use listing::FtpEntry;
use std::{fmt, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    time::timeout,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REPLY_BYTES: usize = 64 * 1024;

pub struct FtpConfig {
    pub host: String,
    pub port: Option<u16>,
    pub username: String,
    pub password: String,
    pub base_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtpError {
    Config,
    Cancelled,
    Timeout,
    Io,
    Protocol,
    Reply(u16),
    AbortUnsettled,
    DeleteUnsettled,
    Verification,
    PublicationUncertain,
}
impl fmt::Display for FtpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config => f.write_str("Invalid FTP configuration"),
            Self::Cancelled => f.write_str("FTP operation cancelled; remote effect may exist"),
            Self::DeleteUnsettled => f.write_str("FTP delete effect is unconfirmed"),
            Self::Timeout => f.write_str("FTP operation timed out"),
            Self::Io => f.write_str("FTP socket I/O failed"),
            Self::Protocol => f.write_str("Invalid FTP reply"),
            Self::Reply(status) => write!(f, "FTP server returned status {status}"),
            Self::AbortUnsettled => f.write_str("FTP transfer abort is unconfirmed"),
            Self::Verification => f.write_str("FTP independent byte verification failed"),
            Self::PublicationUncertain => {
                f.write_str("FTP download publication durability is uncertain")
            }
        }
    }
}
impl std::error::Error for FtpError {}

#[derive(Clone)]
pub struct Ftp {
    config: Arc<FtpConfig>,
    owner: Owner,
}
impl Ftp {
    pub fn new(config: FtpConfig, owner: Owner) -> Result<Self, FtpError> {
        if config.host.is_empty()
            || ![
                &config.host,
                &config.username,
                &config.password,
                &config.base_path,
            ]
            .into_iter()
            .all(|value| valid_argument(value))
        {
            return Err(FtpError::Config);
        }
        Ok(Self {
            config: Arc::new(config),
            owner,
        })
    }
    pub fn test_connection(&self) -> Result<Work<Result<(), FtpError>>, WorkError> {
        let config = self.config.clone();
        self.owner.submit_async(move |cancel| async move {
            let mut control = Control::connect(&config, &cancel).await?;
            tokio::select! { biased;
                _ = cancel.cancelled() => Err(FtpError::Cancelled),
                result = control.quit() => result,
            }
        })
    }
}

impl Ftp {
    /// Caller owns the durable delete intent, including an unconfirmed server reply.
    pub fn delete(&self, remote: String) -> Result<Work<Result<(), FtpError>>, WorkError> {
        let config = self.config.clone();
        self.owner.submit_async(move |cancel| async move {
            if !valid_argument(&remote)
                || remote
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))
            {
                return Err(FtpError::Config);
            }
            let remote = format!("{}/{}", config.base_path.trim_end_matches('/'), remote);
            if !valid_argument(&remote) {
                return Err(FtpError::Config);
            }
            let mut control = Control::connect(&config, &cancel).await?;
            if cancel.is_cancelled() {
                return Err(FtpError::Cancelled);
            }
            // DELE cannot be recalled after the server receives it.
            let result = control.command("DELE", Some(&remote)).await;
            expect(result.map_err(|_| FtpError::DeleteUnsettled)?, &[250])?;
            // The socket closes on drop; a QUIT error cannot undo acknowledged DELE.
            let _ = control.quit().await;
            if cancel.is_cancelled() {
                Err(FtpError::Cancelled)
            } else {
                Ok(())
            }
        })
    }
}

fn valid_argument(value: &str) -> bool {
    value.len() < MAX_REPLY_BYTES && !value.bytes().any(|b| matches!(b, 0 | b'\r' | b'\n'))
}
fn expect(reply: u16, accepted: &[u16]) -> Result<(), FtpError> {
    if accepted.contains(&reply) {
        Ok(())
    } else {
        Err(FtpError::Reply(reply))
    }
}

struct Control(BufReader<TcpStream>);
impl Control {
    async fn connect(
        config: &FtpConfig,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Self, FtpError> {
        if cancel.is_cancelled() {
            return Err(FtpError::Cancelled);
        }
        let port = config.port.filter(|port| *port != 0).unwrap_or(21);
        // Dropping Tokio's waiter cannot settle a blocking system lookup already running.
        let addresses = tokio::net::lookup_host((config.host.as_str(), port)).await;
        if cancel.is_cancelled() {
            return Err(FtpError::Cancelled);
        }
        let addresses = addresses.map_err(|_| FtpError::Io)?.collect::<Vec<_>>();
        tokio::select! { biased;
            _ = cancel.cancelled() => Err(FtpError::Cancelled),
            result = async {
                let stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(addresses.as_slice()))
                    .await.map_err(|_| FtpError::Timeout)?.map_err(|_| FtpError::Io)?;
                let mut control = Control(BufReader::new(stream));
                let greeting = control.reply().await?;
                let greeting = if greeting == 120 { control.reply().await? } else { greeting };
                expect(greeting, &[220])?;
                let (user, password) = if config.username.is_empty() {
                    ("anonymous", "anon@")
                } else {
                    (config.username.as_str(), config.password.as_str())
                };
                let login = control.command("USER", Some(user)).await?;
                if login == 331 {
                    expect(control.command("PASS", Some(password)).await?, &[230])?;
                } else {
                    expect(login, &[230])?;
                }
                Ok(control)
            } => result,
        }
    }

    async fn quit(&mut self) -> Result<(), FtpError> {
        expect(self.command("QUIT", None).await?, &[221])
    }

    async fn command(
        &mut self,
        verb: &'static str,
        argument: Option<&str>,
    ) -> Result<u16, FtpError> {
        self.send(verb, argument).await?;
        self.reply().await
    }

    async fn send(&mut self, verb: &'static str, argument: Option<&str>) -> Result<(), FtpError> {
        let line = match argument {
            Some(value) if valid_argument(value) => format!("{verb} {value}\r\n"),
            Some(_) => return Err(FtpError::Config),
            None => format!("{verb}\r\n"),
        };
        timeout(SOCKET_TIMEOUT, self.0.write_all(line.as_bytes()))
            .await
            .map_err(|_| FtpError::Timeout)?
            .map_err(|_| FtpError::Io)
    }
    async fn reply(&mut self) -> Result<u16, FtpError> {
        self.reply_line().await.map(|(status, _)| status)
    }
    async fn reply_line(&mut self) -> Result<(u16, Vec<u8>), FtpError> {
        timeout(SOCKET_TIMEOUT, async {
            let mut line = Vec::new();
            let mut budget = MAX_REPLY_BYTES;
            self.line(&mut line, &mut budget).await?;
            let prefix: [u8; 3] = line
                .get(..3)
                .ok_or(FtpError::Protocol)?
                .try_into()
                .map_err(|_| FtpError::Protocol)?;
            if !(b'1'..=b'5').contains(&prefix[0]) || !prefix.iter().all(u8::is_ascii_digit) {
                return Err(FtpError::Protocol);
            }
            match line.get(3) {
                Some(b' ') => {}
                Some(b'-') => loop {
                    self.line(&mut line, &mut budget).await?;
                    if line.starts_with(&prefix) && line.get(3) == Some(&b' ') {
                        break;
                    }
                },
                _ => return Err(FtpError::Protocol),
            }
            let status = u16::from(prefix[0] - b'0') * 100
                + u16::from(prefix[1] - b'0') * 10
                + u16::from(prefix[2] - b'0');
            Ok((status, line))
        })
        .await
        .map_err(|_| FtpError::Timeout)?
    }
    async fn line(&mut self, line: &mut Vec<u8>, budget: &mut usize) -> Result<(), FtpError> {
        line.clear();
        let read = (&mut self.0)
            .take(*budget as u64)
            .read_until(b'\n', line)
            .await
            .map_err(|_| FtpError::Io)?;
        if !line.ends_with(b"\r\n") {
            return Err(FtpError::Protocol);
        }
        *budget -= read;
        Ok(())
    }
}
