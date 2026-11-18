use super::{Control, Ftp, FtpError, SOCKET_TIMEOUT, expect, valid_argument};
use crate::{
    ownership::{Owner, Work, WorkError},
    storage::{VerifiedSource, upload::source_chunks},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

impl Ftp {
    /// The caller records the remote mutation intent before scheduling this transfer.
    pub fn upload(
        &self,
        source: Arc<VerifiedSource>,
        remote: String,
        io: &Owner,
    ) -> Result<Work<Result<(), FtpError>>, WorkError> {
        let config = self.config.clone();
        let io = io.clone();
        self.owner.submit_async(move |cancel| async move {
            if !valid_argument(&remote) || remote.split('/').any(|part| matches!(part, "" | "." | "..")) {
                return Err(FtpError::Config);
            }
            let remote = format!("{}/{}", config.base_path.trim_end_matches('/'), remote);
            super::upload::check_source(&io, source.clone()).await?;
            let mut control = Control::connect(&config, &cancel).await?;
            for (end, _) in remote.match_indices('/').skip(1) {
                if cancel.is_cancelled() { return Err(FtpError::Cancelled); }
                // An existing directory may return 550; STOR is still authoritative.
                expect(control.command("MKD", Some(&remote[..end])).await?, &[250, 257, 550])?;
            }
            let mut data = begin(&mut control, "STOR", &remote, &cancel).await?;
            let (mut chunks, producer) = match source_chunks(source.clone(), &io, 0, source.len()) {
                Ok(work) => work,
                Err(_) => {
                    drop(data);
                    control.abort_transfer().await?;
                    return Err(FtpError::Io);
                }
            };
            let producer_cancel = producer.cancellation_token();
            let sent = tokio::select! { biased;
                _ = cancel.cancelled() => Err(FtpError::Cancelled),
                _ = producer_cancel.cancelled() => Err(FtpError::Io),
                result = async {
                    while let Some(chunk) = chunks.recv().await {
                        timeout(SOCKET_TIMEOUT, data.write_all(&chunk)).await.map_err(|_| FtpError::Timeout)?.map_err(|_| FtpError::Io)?;
                    }
                    Ok(())
                } => result,
            };
            if sent.is_err() { producer.cancel(); }
            drop(chunks);
            let produced = producer.wait().await.map_err(|_| FtpError::Io).and_then(|r| r.map_err(|_| FtpError::Io));
            if let Err(error) = sent.and(produced) {
                drop(data);
                control.abort_transfer().await?;
                return Err(error);
            }
            drop(data);
            complete(&mut control).await?;
            if cancel.is_cancelled() { return Err(FtpError::Cancelled); }
            let mut data = begin(&mut control, "RETR", &remote, &cancel).await?;
            let readback = tokio::select! { biased;
                _ = cancel.cancelled() => Err(FtpError::Cancelled),
                result = async {
                    let mut buf = [0; 64 * 1024];
                    let mut sha = Sha256::new();
                    loop {
                        let read = timeout(SOCKET_TIMEOUT, data.read(&mut buf)).await.map_err(|_| FtpError::Timeout)?.map_err(|_| FtpError::Io)?;
                        if read == 0 { break; }
                        sha.update(&buf[..read]);
                    }
                    Ok(<[u8; 32]>::from(sha.finalize()))
                } => result,
            };
            drop(data);
            let hash = match readback {
                Ok(hash) => hash,
                Err(error) => {
                    control.abort_transfer().await?;
                    return Err(error);
                }
            };
            complete(&mut control).await?;
            control.quit().await?;
            check_source(&io, source.clone()).await?;
            if cancel.is_cancelled() { return Err(FtpError::Cancelled); }
            if hash != source.digest { return Err(FtpError::Verification); }
            Ok(())
        })
    }
}

async fn check_source(io: &Owner, source: Arc<VerifiedSource>) -> Result<(), FtpError> {
    io.submit_blocking(move |_| source.check_unchanged())
        .map_err(|_| FtpError::Io)?
        .wait()
        .await
        .map_err(|_| FtpError::Io)?
        .map_err(|_| FtpError::Io)
}

async fn begin(
    control: &mut Control,
    verb: &'static str,
    remote: &str,
    cancel: &CancellationToken,
) -> Result<tokio::net::TcpStream, FtpError> {
    let data = tokio::select! { biased;
        _ = cancel.cancelled() => return Err(FtpError::Cancelled),
        result = control.passive() => result?,
    };
    if cancel.is_cancelled() {
        return Err(FtpError::Cancelled);
    }
    expect(control.command(verb, Some(remote)).await?, &[125, 150])?;
    Ok(data)
}

async fn complete(control: &mut Control) -> Result<(), FtpError> {
    expect(control.reply().await?, &[226, 250])
}
