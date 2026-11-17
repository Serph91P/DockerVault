use crate::ownership::{Owner, Work, WorkError};
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region, retry::RetryConfig, timeout::TimeoutConfig},
};
use std::time::Duration;

pub struct S3Config {
    pub endpoint_url: Option<String>,
    pub region: String,
    pub bucket: String,
    pub base_path: String,
    pub access_key: String,
    pub secret_key: String,
}

#[derive(Debug)]
pub enum S3Error {
    Config,
    Request,
    Http(u16),
    Cancelled,
}
impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config => f.write_str("invalid S3 configuration"),
            Self::Request => f.write_str("S3 request failed"),
            Self::Http(code) => write!(f, "S3 HTTP status {code}"),
            Self::Cancelled => f.write_str("S3 operation cancelled"),
        }
    }
}
impl std::error::Error for S3Error {}

#[derive(Clone)]
pub struct S3 {
    client: Client,
    bucket: String,
    prefix: String,
    owner: Owner,
}
impl S3 {
    pub fn new(config: S3Config, owner: Owner) -> Result<Self, S3Error> {
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
        })
    }

    pub fn key(&self, remote: &str) -> String {
        format!("{}/{remote}", self.prefix)
            .trim_start_matches('/')
            .to_owned()
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
            result.map(|_| ()).map_err(|error| {
                error.raw_response().map_or(S3Error::Request, |response| {
                    S3Error::Http(response.status().as_u16())
                })
            })
        })
    }
}
