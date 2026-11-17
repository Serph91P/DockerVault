//! Async network body, fixed-size synchronous filesystem work, one physical owner per side.
use super::{S3, S3Error};
use crate::{
    ownership::{Work, WorkError},
    storage::local::{Downloaded, TransferError, receive_download},
};
use std::{fs::File, path::PathBuf};
use tokio_util::{
    io::{StreamReader, SyncIoBridge},
    sync::CancellationToken,
};

impl S3 {
    pub fn download(
        &self,
        remote: String,
        root: File,
        path: PathBuf,
    ) -> Result<Work<Result<Downloaded, S3Error>>, WorkError> {
        let backend = self.clone();
        self.owner.submit_async(move |cancel| async move {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let response = backend
                .client
                .get_object()
                .bucket(&backend.bucket)
                .key(backend.key(&remote))
                .send()
                .await;
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let response = response.map_err(|error| {
                error.raw_response().map_or(S3Error::Request, |response| {
                    S3Error::Http(response.status().as_u16())
                })
            })?;
            let expected = response
                .content_length()
                .and_then(|length| u64::try_from(length).ok())
                .ok_or(S3Error::Verification)?;
            let parent_cancel = cancel.clone();
            let written = backend
                .io
                .submit_blocking(move |cancel| {
                    let reader = body_reader(response.body, parent_cancel.clone(), cancel.clone());
                    receive_download(
                        root,
                        &path,
                        &mut SyncIoBridge::new(reader),
                        Some(expected),
                        &|| cancel.is_cancelled() || parent_cancel.is_cancelled(),
                    )
                })
                .map_err(|_| S3Error::Capacity)?
                .wait()
                .await
                .map_err(|_| S3Error::Io)?;
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            written.map_err(|error| match error {
                TransferError::Io(_) => S3Error::Io,
                TransferError::PublicationUncertain(_) => S3Error::PublicationUncertain,
            })
        })
    }
}

pub(super) fn body_reader(
    body: aws_sdk_s3::primitives::ByteStream,
    cancel: CancellationToken,
    io_cancel: CancellationToken,
) -> impl tokio::io::AsyncRead + Unpin + Send {
    let stream = futures_util::stream::unfold(
        (body, cancel, io_cancel),
        |(mut body, cancel, io_cancel)| async move {
            let chunk = tokio::select! {
                biased;
                _ = io_cancel.cancelled() => Some(Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "S3 body cancelled"))),
                _ = cancel.cancelled() => Some(Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "S3 body cancelled"))),
                chunk = body.try_next() => match chunk {
                    Ok(Some(bytes)) => Some(Ok(bytes)),
                    Ok(None) => None,
                    Err(_) => Some(Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "S3 body incomplete"))),
                }
            };
            chunk.map(|chunk| (chunk, (body, cancel, io_cancel)))
        },
    );
    StreamReader::new(Box::pin(stream))
}
