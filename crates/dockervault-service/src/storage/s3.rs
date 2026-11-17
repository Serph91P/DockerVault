mod download;
mod upload;
use crate::ownership::{Owner, Work, WorkError};
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region, retry::RetryConfig, timeout::TimeoutConfig},
};
use std::time::Duration;
pub use upload::S3UploadPolicy;

pub struct S3Config {
    pub endpoint_url: Option<String>,
    pub region: String,
    pub bucket: String,
    pub base_path: String,
    pub access_key: String,
    pub secret_key: String,
}

pub enum S3Error {
    Config,
    Request,
    Http(u16),
    Provider {
        status: Option<u16>,
        code: String,
    },
    Cancelled,
    Listing,
    Capacity,
    Io,
    Verification,
    PublicationUncertain,
    MultipartUnsettled {
        upload_id: String,
        primary: Box<S3Error>,
    },
    CreationUnsettled {
        primary: Box<S3Error>,
    },
}
impl S3Error {
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Http(status) => Some(*status),
            Self::Provider { status, .. } => *status,
            Self::MultipartUnsettled { primary, .. } | Self::CreationUnsettled { primary } => {
                primary.http_status()
            }
            _ => None,
        }
    }
    pub fn provider_code(&self) -> Option<&str> {
        match self {
            Self::Provider { code, .. } => Some(code),
            Self::MultipartUnsettled { primary, .. } | Self::CreationUnsettled { primary } => {
                primary.provider_code()
            }
            _ => None,
        }
    }
}
impl std::fmt::Debug for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config => f.write_str("invalid S3 configuration"),
            Self::Request => f.write_str("S3 request failed"),
            Self::Http(code) => write!(f, "S3 HTTP status {code}"),
            Self::Provider {
                status: Some(status),
                ..
            } => write!(f, "S3 HTTP status {status}"),
            Self::Provider { status: None, .. } => f.write_str("S3 request failed"),
            Self::Cancelled => f.write_str("S3 operation cancelled; remote effect may exist"),
            Self::Listing => f.write_str("S3 listing is incomplete or invalid"),
            Self::MultipartUnsettled { .. } => {
                f.write_str("S3 multipart cleanup is unconfirmed; retain its journal obligation")
            }
            Self::CreationUnsettled { .. } => {
                f.write_str("S3 multipart creation is unconfirmed; retain its journal obligation")
            }
            Self::Capacity => f.write_str("S3 I/O capacity exhausted"),
            Self::Io => f.write_str("S3 local I/O failed"),
            Self::Verification => f.write_str("S3 independent byte verification failed"),
            Self::PublicationUncertain => {
                f.write_str("S3 local publication durability is uncertain")
            }
        }
    }
}
impl std::error::Error for S3Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MultipartUnsettled { primary, .. } | Self::CreationUnsettled { primary } => {
                Some(primary.as_ref())
            }
            _ => None,
        }
    }
}

fn request_error<E: aws_sdk_s3::error::ProvideErrorMetadata>(
    error: aws_sdk_s3::error::SdkError<E>,
) -> S3Error {
    let status = error
        .raw_response()
        .map(|response| response.status().as_u16());
    match error.as_service_error().and_then(|error| error.code()) {
        Some(code) => S3Error::Provider {
            status,
            code: code.chars().take(500).collect(),
        },
        None => status.map_or(S3Error::Request, S3Error::Http),
    }
}

#[derive(Clone)]
pub struct S3 {
    client: Client,
    bucket: String,
    prefix: String,
    owner: Owner,
    io: Owner,
}
impl S3 {
    pub fn new(config: S3Config, owner: Owner, io: Owner) -> Result<Self, S3Error> {
        if config.bucket.is_empty() || config.access_key.is_empty() || config.secret_key.is_empty()
        {
            return Err(S3Error::Config);
        }
        let region = if config.region.is_empty() {
            "us-east-1".to_owned()
        } else {
            config.region
        };
        let mut builder = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region))
            .credentials_provider(Credentials::new(
                config.access_key,
                config.secret_key,
                None,
                None,
                "explicit-storage",
            ))
            .retry_config(RetryConfig::standard().with_max_attempts(4))
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(30))
                    .read_timeout(Duration::from_secs(300))
                    .build(),
            );
        if let Some(endpoint) = config.endpoint_url {
            let url = reqwest::Url::parse(&endpoint).map_err(|_| S3Error::Config)?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
                || url.query().is_some()
            {
                return Err(S3Error::Config);
            }
            builder = builder.endpoint_url(endpoint);
        }
        Ok(Self {
            client: Client::from_conf(builder.build()),
            bucket: config.bucket,
            prefix: config.base_path,
            owner,
            io,
        })
    }

    pub fn key(&self, remote: &str) -> String {
        format!("{}/{remote}", self.prefix)
            .trim_start_matches('/')
            .to_owned()
    }

    pub fn delete(&self, remote: String) -> Result<Work<Result<(), S3Error>>, WorkError> {
        let backend = self.clone();
        self.owner.submit_async(move |cancel| async move {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let result = backend
                .client
                .delete_object()
                .bucket(&backend.bucket)
                .key(backend.key(&remote))
                .send()
                .await;
            // DELETE has no protocol abort: retain admission until the request settles.
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            result.map(|_| ()).map_err(request_error)
        })
    }

    pub fn list_files(
        &self,
        remote: String,
    ) -> Result<Work<Result<Vec<super::webdav::StorageEntry>, S3Error>>, WorkError> {
        let backend = self.clone();
        self.owner.submit_async(move |cancel| async move {
            let mut prefix = backend.key(&remote);
            if !prefix.is_empty() && !prefix.ends_with('/') {
                prefix.push('/');
            }
            let mut continuation = None;
            let mut seen = std::collections::HashSet::new();
            let mut files = Vec::new();
            loop {
                if cancel.is_cancelled() {
                    return Err(S3Error::Cancelled);
                }
                let page = backend
                    .client
                    .list_objects_v2()
                    .bucket(&backend.bucket)
                    .prefix(&prefix)
                    .set_continuation_token(continuation)
                    .send()
                    .await;
                if cancel.is_cancelled() {
                    return Err(S3Error::Cancelled);
                }
                let page = page.map_err(request_error)?;
                for object in page.contents() {
                    let key = object.key().ok_or(S3Error::Listing)?;
                    files.push(super::webdav::StorageEntry {
                        name: key.strip_prefix(&prefix).unwrap_or(key).to_owned(),
                        size: object.size().ok_or(S3Error::Listing)?,
                        is_dir: false,
                        modified: object
                            .last_modified()
                            .map_or(0.0, |date| date.as_secs_f64()),
                    });
                }
                if page.is_truncated() != Some(true) {
                    break;
                }
                let token = page
                    .next_continuation_token()
                    .filter(|token| !token.is_empty())
                    .ok_or(S3Error::Listing)?;
                if !seen.insert(token.to_owned()) {
                    return Err(S3Error::Listing);
                }
                continuation = Some(token.to_owned());
            }
            Ok(files)
        })
    }

    pub fn test_connection(&self) -> Result<Work<Result<(), S3Error>>, WorkError> {
        let backend = self.clone();
        self.owner.submit_async(move |cancel| async move {
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            let result = backend
                .client
                .head_bucket()
                .bucket(backend.bucket)
                .send()
                .await;
            if cancel.is_cancelled() {
                return Err(S3Error::Cancelled);
            }
            result.map(|_| ()).map_err(request_error)
        })
    }
}
