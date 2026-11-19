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

type Snapshot<T> = (File, u64, T);

impl S3 {
    /// Recovery can prove an already written object without replay or a local source.
    pub fn verify_object(
        &self,
        remote: String,
        length: u64,
        digest: [u8; 32],
    ) -> Result<Work<Result<bool, S3Error>>, WorkError> {
        self.download_with(remote, move |mut reader, declared, cancel| {
            if declared != length {
                return Ok(false);
            }
            Ok(crate::storage::hash_reader(&mut reader, cancel)? == digest)
        })
    }

    pub fn download(
        &self,
        remote: String,
        root: File,
        path: PathBuf,
    ) -> Result<Work<Result<Downloaded, S3Error>>, WorkError> {
        self.download_with(remote, move |reader, expected, cancel| {
            receive_download(root, &path, reader, Some(expected), cancel)
        })
    }

    pub(crate) fn download_snapshot<T: Send + 'static>(
        &self,
        remote: String,
        held: T,
    ) -> Result<Work<Result<Snapshot<T>, S3Error>>, WorkError> {
        self.download_with(remote, move |reader, expected, cancel| {
            let (file, length) =
                crate::storage::local::receive_snapshot(reader, Some(expected), cancel)?;
            Ok((file, length, held))
        })
    }

    fn download_with<T: Send + 'static>(
        &self,
        remote: String,
        receive: impl FnOnce(&mut dyn std::io::Read, u64, &dyn Fn() -> bool) -> Result<T, TransferError>
        + Send
        + 'static,
    ) -> Result<Work<Result<T, S3Error>>, WorkError> {
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
                .send();
            let response = tokio::select! { biased;
                _ = cancel.cancelled() => return Err(S3Error::Cancelled),
                response = response => response,
            };
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let response = response.map_err(super::request_error)?;
            let expected = response
                .content_length()
                .and_then(|length| u64::try_from(length).ok())
                .ok_or(S3Error::Verification)?;
            let parent_cancel = cancel.clone();
            let written = backend
                .io
                .submit_blocking(move |cancel| {
                    let reader = body_reader(response.body, parent_cancel.clone(), cancel.clone());
                    receive(&mut SyncIoBridge::new(reader), expected, &|| {
                        cancel.is_cancelled() || parent_cancel.is_cancelled()
                    })
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
