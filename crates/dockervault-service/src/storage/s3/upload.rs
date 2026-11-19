use super::{S3, S3Error, download::body_reader, request_error};
use crate::{
    ownership::{Work, WorkError},
    storage::VerifiedSource,
};
use aws_sdk_s3::{
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart},
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio_util::{io::SyncIoBridge, sync::CancellationToken};
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct S3UploadPolicy {
    pub multipart_threshold_bytes: i64,
    pub multipart_chunk_size: i64,
}
impl Default for S3UploadPolicy {
    fn default() -> Self {
        Self {
            multipart_threshold_bytes: 64 * 1024 * 1024,
            multipart_chunk_size: 16 * 1024 * 1024,
        }
    }
}
impl S3 {
    pub(crate) fn settle_multipart(
        &self,
        remote: String,
        upload_id: String,
    ) -> Result<Work<Result<(), S3Error>>, WorkError> {
        let backend = self.clone();
        self.owner.submit_async(move |_| async move {
            let result = tokio::time::timeout(
                CLEANUP_TIMEOUT,
                backend
                    .client
                    .abort_multipart_upload()
                    .bucket(&backend.bucket)
                    .key(backend.key(&remote))
                    .upload_id(upload_id)
                    .send(),
            )
            .await
            .map_err(|_| S3Error::Request)?;
            match result {
                Ok(_) => Ok(()),
                Err(error)
                    if error
                        .raw_response()
                        .is_some_and(|r| r.status().as_u16() == 404)
                        && aws_sdk_s3::error::ProvideErrorMetadata::code(&error)
                            == Some("NoSuchUpload") =>
                {
                    Ok(())
                }
                Err(error) => Err(request_error(error)),
            }
        })
    }

    /// Caller must journal remote uncertainty BEFORE admission; failure is never a verified copy.
    pub fn upload(
        &self,
        source: Arc<VerifiedSource>,
        remote: String,
        policy: S3UploadPolicy,
    ) -> Result<Work<Result<(), S3Error>>, WorkError> {
        self.upload_checkpointed(source, remote, policy, |_| async { Ok(()) })
    }

    /// The caller's durable checkpoint must acknowledge the provider upload ID before any part.
    pub fn upload_checkpointed<F, Fut>(
        &self,
        source: Arc<VerifiedSource>,
        remote: String,
        policy: S3UploadPolicy,
        checkpoint: F,
    ) -> Result<Work<Result<(), S3Error>>, WorkError>
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), S3Error>> + Send + 'static,
    {
        let backend = self.clone();
        self.owner.submit_async(move |cancel| async move {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let source_check = source.clone();
            backend
                .io
                .submit_blocking(move |_| source_check.check_unchanged())
                .map_err(|_| S3Error::Capacity)?
                .wait()
                .await
                .map_err(|_| S3Error::Io)?
                .map_err(|_| S3Error::Io)?;

            let threshold = policy.multipart_threshold_bytes.max(5 * 1024 * 1024) as u64;
            let chunk = policy.multipart_chunk_size.max(5 * 1024 * 1024) as u64;
            let key = backend.key(&remote);
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            if source.len() < threshold {
                // PUT has no protocol abort. Do not select/drop the request on cancellation.
                backend
                    .send_file(
                        source.clone(),
                        0,
                        source.len(),
                        &cancel,
                        false,
                        |body| async {
                            backend
                                .client
                                .put_object()
                                .bucket(&backend.bucket)
                                .key(&key)
                                .if_none_match("*")
                                .content_length(
                                    i64::try_from(source.len()).map_err(|_| S3Error::Config)?,
                                )
                                .body(body)
                                .customize()
                                .config_override(single_attempt())
                                .send()
                                .await
                                .map_err(request_error)
                        },
                    )
                    .await?;
            } else {
                if chunk > 5 * 1024 * 1024 * 1024 || source.len().div_ceil(chunk) > 10000 {
                    return Err(S3Error::Config);
                }
                let creation = backend
                    .client
                    .create_multipart_upload()
                    .bucket(&backend.bucket)
                    .key(&key)
                    .customize()
                    .config_override(single_attempt())
                    .send();
                tokio::pin!(creation);
                let created = tokio::select! {
                    result = &mut creation => result,
                    _ = cancel.cancelled() => {
                        // Obtain the in-flight creation's ID before cleanup; never detach it.
                        tokio::time::timeout(CLEANUP_TIMEOUT, &mut creation).await.map_err(|_| S3Error::CreationUnsettled { primary: Box::new(S3Error::Cancelled) })?
                    }
                }.map_err(|error| S3Error::CreationUnsettled { primary: Box::new(request_error(error)) })?;
                let upload_id = created
                    .upload_id()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| S3Error::CreationUnsettled { primary: Box::new(S3Error::Request) })?;
                let uploaded = match checkpoint(upload_id.to_owned()).await {
                    Ok(()) => backend.upload_parts(source.clone(), chunk, &key, upload_id, &cancel).await,
                    Err(error) => Err(error),
                };
                if let Err(primary) = uploaded {
                    // Retain the ID for the durable caller if provider cleanup is not confirmed.
                    if !matches!(
                        tokio::time::timeout(
                            CLEANUP_TIMEOUT,
                            backend
                                .client
                                .abort_multipart_upload()
                                .bucket(&backend.bucket)
                                .key(&key)
                                .upload_id(upload_id)
                                .send()
                        )
                        .await,
                        Ok(Ok(_))
                    ) {
                        return Err(S3Error::MultipartUnsettled {
                            upload_id: upload_id.to_owned(),
                            primary: Box::new(primary),
                        });
                    }
                    return Err(primary);
                }
            }
            backend.verify_uploaded(source, &key, &cancel).await
        })
    }
    async fn upload_parts(
        &self,
        source: Arc<VerifiedSource>,
        chunk: u64,
        key: &str,
        upload_id: &str,
        cancel: &CancellationToken,
    ) -> Result<(), S3Error> {
        let mut parts = Vec::new();
        let mut offset = 0;
        let length = source.len();
        while offset < length {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let amount = chunk.min(length - offset);
            let number = i32::try_from(parts.len() + 1).map_err(|_| S3Error::Config)?;
            let part = self
                .send_file(source.clone(), offset, amount, cancel, true, |body| async {
                    self.client
                        .upload_part()
                        .bucket(&self.bucket)
                        .key(key)
                        .upload_id(upload_id)
                        .part_number(number)
                        .content_length(amount as i64)
                        .body(body)
                        .customize()
                        .config_override(single_attempt())
                        .send()
                        .await
                        .map_err(request_error)
                })
                .await?;
            parts.push(
                CompletedPart::builder()
                    .e_tag(part.e_tag.ok_or(S3Error::Request)?)
                    .part_number(number)
                    .set_checksum_crc32(part.checksum_crc32)
                    .set_checksum_crc32_c(part.checksum_crc32_c)
                    .set_checksum_crc64_nvme(part.checksum_crc64_nvme)
                    .set_checksum_sha1(part.checksum_sha1)
                    .set_checksum_sha256(part.checksum_sha256)
                    .set_checksum_sha512(part.checksum_sha512)
                    .set_checksum_md5(part.checksum_md5)
                    .set_checksum_xxhash64(part.checksum_xxhash64)
                    .set_checksum_xxhash3(part.checksum_xxhash3)
                    .set_checksum_xxhash128(part.checksum_xxhash128)
                    .build(),
            );
            offset += amount;
        }
        if cancel.is_cancelled() {
            return Err(S3Error::Cancelled);
        }
        let completion = self
            .client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .if_none_match("*")
            .upload_id(upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send();
        let result = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(S3Error::Cancelled),
            result = completion => result,
        };
        result.map_err(request_error)?;
        Ok(())
    }
    async fn send_file<T, F, Fut>(
        &self,
        source: Arc<VerifiedSource>,
        offset: u64,
        length: u64,
        cancel: &CancellationToken,
        abort_on_cancel: bool,
        send: F,
    ) -> Result<T, S3Error>
    where
        F: Fn(ByteStream) -> Fut,
        Fut: Future<Output = Result<T, S3Error>>,
    {
        for attempt in 0..4 {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let source_check = source.clone();
            self.io
                .submit_blocking(move |_| source_check.check_unchanged())
                .map_err(|_| S3Error::Capacity)?
                .wait()
                .await
                .map_err(|_| S3Error::Io)?
                .map_err(|_| S3Error::Io)?;
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let (body, producer) =
                crate::storage::upload::source_body(source.clone(), &self.io, offset, length)
                    .map_err(|_| S3Error::Capacity)?;
            let producer_cancel = producer.cancellation_token();
            let body = ByteStream::new(aws_smithy_types::body::SdkBody::from_body_1_x(body));
            let request = send(body);
            let result = if abort_on_cancel {
                tokio::select! { biased;
                    _ = cancel.cancelled() => Err(S3Error::Cancelled),
                    _ = producer_cancel.cancelled() => Err(S3Error::Io),
                    result = request => result,
                }
            } else {
                request.await
            };
            // Closing an HTTP future cannot settle a blocking read. Join its owner on EVERY path.
            producer.cancel();
            let produced = producer.wait().await.map_err(|_| S3Error::Io)?;
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            match result {
                Ok(output) => {
                    produced.map_err(|_| S3Error::Io)?;
                    return Ok(output);
                }
                Err(error) => {
                    if attempt == 3
                        || !(matches!(&error, S3Error::Request)
                            || matches!(error.http_status(), Some(408 | 429 | 500..=599)))
                    {
                        return Err(error);
                    }
                    let mut random = [0u8; 8];
                    getrandom::fill(&mut random).map_err(|_| S3Error::Request)?;
                    let delay = Duration::from_secs_f64(
                        (1u64 << attempt) as f64
                            * (u64::from_le_bytes(random) as f64 / u64::MAX as f64),
                    );
                    tokio::select! { _ = cancel.cancelled() => return Err(S3Error::Cancelled), _ = tokio::time::sleep(delay) => {} }
                }
            }
        }
        unreachable!("four bounded upload attempts return their result")
    }

    async fn verify_uploaded(
        &self,
        source: Arc<VerifiedSource>,
        key: &str,
        cancel: &CancellationToken,
    ) -> Result<(), S3Error> {
        if cancel.is_cancelled() {
            return Err(S3Error::Cancelled);
        }
        let response = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send();
        let response = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(S3Error::Cancelled),
            response = response => response,
        };
        let response = response.map_err(request_error)?;
        if cancel.is_cancelled() {
            return Err(S3Error::Cancelled);
        }
        if response
            .content_length()
            .and_then(|length| u64::try_from(length).ok())
            != Some(source.len())
        {
            return Err(S3Error::Verification);
        }
        let job_cancel = cancel.clone();
        let result = self
            .io
            .submit_blocking(move |cancel| {
                let reader = body_reader(response.body, job_cancel.clone(), cancel.clone());
                source.verify_download(&mut SyncIoBridge::new(reader), &|| {
                    cancel.is_cancelled() || job_cancel.is_cancelled()
                })
            })
            .map_err(|_| S3Error::Capacity)?
            .wait()
            .await
            .map_err(|_| S3Error::Verification)?;
        if cancel.is_cancelled() {
            return Err(S3Error::Cancelled);
        }
        result.map_err(|_| S3Error::Verification)
    }
}
fn single_attempt() -> aws_sdk_s3::config::Builder {
    aws_sdk_s3::config::Builder::new()
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::standard().with_max_attempts(1))
}
