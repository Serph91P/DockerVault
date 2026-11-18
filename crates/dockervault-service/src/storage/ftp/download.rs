use super::{CONNECT_TIMEOUT, Control, Ftp, FtpError, SOCKET_TIMEOUT, expect, valid_argument};
use crate::{
    ownership::{Owner, Work, WorkError},
    storage::local::{Downloaded, TransferError, receive_download},
};
use std::{fs::File, io, path::PathBuf, time::Duration};
use tokio::{io::AsyncReadExt, net::TcpStream, sync::mpsc, time::timeout};
use tokio_util::{
    bytes::Bytes,
    io::{StreamReader, SyncIoBridge},
};

impl Ftp {
    pub fn download(
        &self,
        remote: String,
        root: File,
        path: PathBuf,
        io_owner: &Owner,
    ) -> Result<Work<Result<Downloaded, FtpError>>, WorkError> {
        let config = self.config.clone();
        let io_owner = io_owner.clone();
        self.owner.submit_async(move |cancel| async move {
            if !valid_argument(&remote)
                || remote
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))
            {
                return Err(FtpError::Config);
            }
            let remote = format!("{}/{}", config.base_path.trim_end_matches('/'), remote);
            let mut control = Control::connect(&config, &cancel).await?;
            let mut data = tokio::select! { biased;
                _ = cancel.cancelled() => return Err(FtpError::Cancelled),
                result = control.passive() => result?,
            };
            // Do not abandon a partially consumed RETR reply on cancellation.
            let start = control.command("RETR", Some(&remote)).await;
            match start {
                Ok(status) => expect(status, &[125, 150])?,
                Err(_) if cancel.is_cancelled() => return Err(FtpError::AbortUnsettled),
                Err(error) => return Err(error),
            }
            let (send, receive) = mpsc::channel::<Option<Bytes>>(8);
            let parent_cancel = cancel.clone();
            let writer = match io_owner.submit_blocking(move |io_cancel| {
                let stream = futures_util::stream::unfold(Some(receive), |state| async move {
                    let mut receive = state?;
                    match receive.recv().await {
                        Some(Some(chunk)) => Some((Ok(chunk), Some(receive))),
                        Some(None) => None, // EOF is explicit, emitted only after the final ACK.
                        None => Some((
                            Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "FTP transfer not confirmed",
                            )),
                            None,
                        )),
                    }
                });
                let mut reader = SyncIoBridge::new(StreamReader::new(Box::pin(stream)));
                receive_download(root, &path, &mut reader, None, &|| {
                    parent_cancel.is_cancelled() || io_cancel.is_cancelled()
                })
            }) {
                Ok(writer) => writer,
                Err(_) => {
                    drop(data);
                    control.abort_transfer().await?;
                    return Err(FtpError::Io);
                }
            };
            let io_cancel = writer.cancellation_token();
            let network = tokio::select! { biased;
                _ = cancel.cancelled() => Err(FtpError::Cancelled),
                _ = io_cancel.cancelled() => Err(FtpError::Io),
                result = pump(&mut data, &send) => result,
            };
            drop(data);
            let result = match network {
                Err(error) => {
                    // Closing the producer makes the writer roll back, not publish.
                    drop(send);
                    let joined = writer.wait().await;
                    control.abort_transfer().await?;
                    let _ = joined.map_err(|_| FtpError::Io)?;
                    return Err(error);
                }
                Ok(()) => {
                    let reply = control.reply().await;
                    match reply {
                        Err(_) if cancel.is_cancelled() => Err(FtpError::AbortUnsettled),
                        reply => {
                            let completed = reply.and_then(|status| expect(status, &[226, 250]));
                            if let Err(error) = completed {
                                Err(error)
                            } else if cancel.is_cancelled() {
                                Err(FtpError::Cancelled)
                            } else {
                                control.quit().await
                            }
                        }
                    }
                }
            };
            let result = match result {
                Ok(()) => send.send(None).await.map_err(|_| FtpError::Io),
                Err(error) => Err(error),
            };
            drop(send);
            let written = writer.wait().await;
            result?;
            written
                .map_err(|_| FtpError::Io)?
                .map_err(|error| match error {
                    TransferError::Io(_) if cancel.is_cancelled() => FtpError::Cancelled,
                    TransferError::Io(_) => FtpError::Io,
                    TransferError::PublicationUncertain(_) => FtpError::PublicationUncertain,
                })
        })
    }
}

async fn pump(data: &mut TcpStream, send: &mpsc::Sender<Option<Bytes>>) -> Result<(), FtpError> {
    loop {
        let mut chunk = vec![0; 64 * 1024];
        let count = timeout(SOCKET_TIMEOUT, data.read(&mut chunk))
            .await
            .map_err(|_| FtpError::Timeout)?
            .map_err(|_| FtpError::Io)?;
        if count == 0 {
            return Ok(());
        }
        chunk.truncate(count);
        send.send(Some(Bytes::from(chunk)))
            .await
            .map_err(|_| FtpError::Io)?;
    }
}

impl Control {
    pub(super) async fn passive(&mut self) -> Result<TcpStream, FtpError> {
        expect(self.command("TYPE", Some("I")).await?, &[200])?;
        self.send("EPSV", None).await?;
        let (reply, line) = self.reply_line().await?;
        let port = match reply {
            229 => passive_port(&line, true)?,
            500 | 501 | 502 | 504 | 522
                if self
                    .0
                    .get_ref()
                    .peer_addr()
                    .map_err(|_| FtpError::Io)?
                    .is_ipv4() =>
            {
                self.send("PASV", None).await?;
                let (reply, line) = self.reply_line().await?;
                expect(reply, &[227])?;
                passive_port(&line, false)?
            }
            response => return Err(FtpError::Reply(response)),
        };
        // The configured control peer determines the address, even for PASV.
        let mut peer = self.0.get_ref().peer_addr().map_err(|_| FtpError::Io)?;
        peer.set_port(port);
        timeout(CONNECT_TIMEOUT, TcpStream::connect(peer))
            .await
            .map_err(|_| FtpError::Timeout)?
            .map_err(|_| FtpError::Io)
    }
    pub(super) async fn abort_transfer(&mut self) -> Result<(), FtpError> {
        let abort = timeout(Duration::from_secs(10), async {
            let reply = self.command("ABOR", None).await?;
            let reply = if reply == 426 {
                self.reply().await?
            } else {
                reply
            };
            expect(reply, &[225, 226])?;
            self.send("QUIT", None).await?;
            // A 226 for the original transfer can arrive before the ABOR acknowledgement.
            let quit = self.reply().await?;
            let quit = if matches!(quit, 225 | 226) {
                self.reply().await?
            } else {
                quit
            };
            expect(quit, &[221])
        })
        .await;
        if matches!(abort, Ok(Ok(()))) {
            Ok(())
        } else {
            Err(FtpError::AbortUnsettled)
        }
    }
}

fn passive_port(reply: &[u8], extended: bool) -> Result<u16, FtpError> {
    let reply = std::str::from_utf8(reply).map_err(|_| FtpError::Protocol)?;
    let tuple = reply
        .split_once('(')
        .and_then(|(_, tail)| tail.split_once(')'))
        .map(|(tuple, _)| tuple)
        .ok_or(FtpError::Protocol)?;
    let port = if extended {
        let delimiter = *tuple.as_bytes().first().ok_or(FtpError::Protocol)?;
        if !(33..=126).contains(&delimiter) {
            return Err(FtpError::Protocol);
        }
        let parts: Vec<_> = tuple.split(char::from(delimiter)).collect();
        match parts.as_slice() {
            ["", "", "", port, ""] if port.bytes().all(|b| b.is_ascii_digit()) => {
                port.parse().map_err(|_| FtpError::Protocol)?
            }
            _ => return Err(FtpError::Protocol),
        }
    } else {
        let mut parts = [0u8; 6];
        let mut split = tuple.split(',');
        for part in &mut parts {
            let value = split.next().ok_or(FtpError::Protocol)?;
            if !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(FtpError::Protocol);
            }
            *part = value.parse().map_err(|_| FtpError::Protocol)?;
        }
        if split.next().is_some() {
            return Err(FtpError::Protocol);
        }
        u16::from_be_bytes([parts[4], parts[5]])
    };
    if port == 0 {
        Err(FtpError::Protocol)
    } else {
        Ok(port)
    }
}
