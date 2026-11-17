use super::{S3, S3Error, download::body_reader};
use crate::{
    ownership::{Work, WorkError},
    storage::VerifiedSource,
};
use aws_sdk_s3::{
    primitives::{ByteStream, Length},
    types::{CompletedMultipartUpload, CompletedPart},
};
use std::{os::fd::AsRawFd, path::Path, sync::Arc, time::Duration};
use tokio_util::{io::SyncIoBridge, sync::CancellationToken};

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
    /// Caller must journal remote uncertainty BEFORE admission; failure is never a verified copy.
    pub fn upload(
        &self,
        source: Arc<VerifiedSource>,
        remote: String,
        policy: S3UploadPolicy,
    ) -> Result<Work<Result<(), S3Error>>, WorkError> {
        let backend = self.clone();
        self.owner.submit_async(move |cancel| async move {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let source_check = source.clone();
            let retained = backend
                .io
                .submit_blocking(move |_| source_check.reopen())
                .map_err(|_| S3Error::Capacity)?
                .wait()
                .await
                .map_err(|_| S3Error::Io)?
                .map_err(|_| S3Error::Io)?;
            let path = std::path::PathBuf::from(format!("/proc/self/fd/{}", retained.as_raw_fd()));
            let threshold = policy.multipart_threshold_bytes.max(5 * 1024 * 1024) as u64;
            let chunk = policy.multipart_chunk_size.max(5 * 1024 * 1024) as u64;
            let key = backend.key(&remote);
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            if source.len() < threshold {
                let body = file_body(&path, 0, source.len()).await?;
                // PUT has no protocol abort. Do not select/drop the request on cancellation.
                let result = backend
                    .client
                    .put_object()
                    .bucket(&backend.bucket)
                    .key(&key)
                    .body(body)
                    .send()
                    .await;
                if cancel.is_cancelled() {
                    return Err(S3Error::Cancelled);
                }
                result.map_err(|error| {
                    error.raw_response().map_or(S3Error::Request, |response| {
                        S3Error::Http(response.status().as_u16())
                    })
                })?;
            } else {
                if chunk > 5 * 1024 * 1024 * 1024 || source.len().div_ceil(chunk) > 10000 {
                    return Err(S3Error::Config);
                }
                let created = backend
                    .client
                    .create_multipart_upload()
                    .bucket(&backend.bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|error| {
                        error.raw_response().map_or(S3Error::Request, |response| {
                            S3Error::Http(response.status().as_u16())
                        })
                    })?;
                let upload_id = created
                    .upload_id()
                    .filter(|id| !id.is_empty())
                    .ok_or(S3Error::Request)?;
                let uploaded = backend
                    .upload_parts(&path, source.len(), chunk, &key, upload_id, &cancel)
                    .await;
                if uploaded.is_err() {
                    // Retain the ID for the durable caller if provider cleanup is not confirmed.
                    if !matches!(
                        tokio::time::timeout(
                            Duration::from_secs(10),
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
                        });
                    }
                    return uploaded;
                }
            }
            drop(retained);
            backend.verify_uploaded(source, &key, &cancel).await
        })
    }
    async fn upload_parts(
        &self,
        path: &Path,
        length: u64,
        chunk: u64,
        key: &str,
        upload_id: &str,
        cancel: &CancellationToken,
    ) -> Result<(), S3Error> {
        let mut parts = Vec::new();
        let mut offset = 0;
        while offset < length {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let amount = chunk.min(length - offset);
            let body = file_body(path, offset, amount).await?;
            let number = i32::try_from(parts.len() + 1).map_err(|_| S3Error::Config)?;
            let part = self
                .client
                .upload_part()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(upload_id)
                .part_number(number)
                .body(body)
                .send()
                .await
                .map_err(|error| {
                    error.raw_response().map_or(S3Error::Request, |response| {
                        S3Error::Http(response.status().as_u16())
                    })
                })?;
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
        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send()
            .await
            .map_err(|error| {
                error.raw_response().map_or(S3Error::Request, |response| {
                    S3Error::Http(response.status().as_u16())
                })
            })?;
        Ok(())
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
            .send()
            .await
            .map_err(|error| {
                error.raw_response().map_or(S3Error::Request, |response| {
                    S3Error::Http(response.status().as_u16())
                })
            })?;
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
async fn file_body(path: &Path, offset: u64, length: u64) -> Result<ByteStream, S3Error> {
    // A path to the HELD descriptor gives SDK retries independent offsets, never a new path occupant.
    ByteStream::read_from()
        .path(path)
        .offset(offset)
        .length(Length::Exact(length))
        .buffer_size(65536)
        .build()
        .await
        .map_err(|_| S3Error::Io)
}
