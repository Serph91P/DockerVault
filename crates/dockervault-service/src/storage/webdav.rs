//! Native DAV PUT and mandatory independent readback, with bounded owned file producers.
use super::{VerifiedSource, check};
use crate::ownership::{Owner, Work, WorkError};
use reqwest::{Body, Client, Method, RequestBuilder, Response, Url};
use std::{
    io::{self, Read},
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct DavConfig {
    pub url: String,
    pub base_path: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub max_attempts: u32,
    pub verify_head: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavError {
    Config,
    Cancelled,
    Capacity,
    Transport,
    Http(u16),
    Source,
    Io,
    PublicationUncertain,
    Verification,
}
impl std::fmt::Display for DavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(status) => write!(f, "WebDAV HTTP {status}"),
            other => f.write_str(match other {
                Self::Config => "Invalid WebDAV configuration",
                Self::Cancelled => "WebDAV cancelled; remote effect may exist",
                Self::Capacity => "WebDAV I/O capacity exhausted",
                Self::Transport => "WebDAV protocol failed; remote effect may exist",
                Self::Source => "WebDAV source changed or unreadable",
                Self::Io => "WebDAV local destination I/O failed",
                Self::PublicationUncertain => {
                    "WebDAV local download publication durability is uncertain"
                }
                Self::Verification => "WebDAV independent byte verification failed",
                Self::Http(_) => unreachable!(),
            }),
        }
    }
}
impl std::error::Error for DavError {}
#[derive(Clone)]
pub struct WebDav {
    client: Client,
    endpoint: Url,
    config: Arc<DavConfig>,
    owner: Owner,
    io: Owner,
}
impl WebDav {
    pub fn new(config: DavConfig, owner: Owner, io: Owner) -> Result<Self, DavError> {
        let mut endpoint = Url::parse(&config.url).map_err(|_| DavError::Config)?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
        {
            return Err(DavError::Config);
        }
        endpoint.set_fragment(None);
        let client = Client::builder()
            .use_native_tls()
            .no_proxy()
            .http1_only()
            .pool_max_idle_per_host(0)
            .connect_timeout(Duration::from_secs(30))
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| DavError::Config)?;
        let result = Self {
            client,
            endpoint,
            config: Arc::new(config),
            owner,
            io,
        };
        result.url("")?;
        Ok(result)
    }
    pub fn url(&self, remote: &str) -> Result<Url, DavError> {
        let mut url = self.endpoint.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|_| DavError::Config)?;
            segments.pop_if_empty();
            for path in [&self.config.base_path[..], remote] {
                for segment in path.split('/').filter(|part| !part.is_empty()) {
                    if matches!(segment, "." | "..") || segment.contains('\0') {
                        return Err(DavError::Config);
                    }
                    segments.push(segment);
                }
            }
        }
        Ok(url)
    }
    fn request(&self, method: Method, url: Url, timeout: Duration) -> RequestBuilder {
        let request = self.client.request(method, url).timeout(timeout);
        match (&self.config.username, &self.config.password) {
            (Some(user), Some(password)) if !user.is_empty() && !password.is_empty() => {
                request.basic_auth(user, Some(password))
            }
            _ => request,
        }
    }
    async fn send(
        &self,
        request: RequestBuilder,
        cancel: &CancellationToken,
    ) -> Result<Response, DavError> {
        tokio::select! {biased;_=cancel.cancelled()=>Err(DavError::Cancelled),response=request.send()=>response.map_err(|_|DavError::Transport)}
    }
    async fn create_dirs(&self, path: &str, cancel: &CancellationToken) -> Result<(), DavError> {
        let Some((parent, _)) = path.rsplit_once('/') else {
            return Ok(());
        };
        let mut current = String::new();
        for part in parent.split('/').filter(|p| !p.is_empty()) {
            current.push('/');
            current.push_str(part);
            let response = self
                .send(
                    self.request(
                        Method::from_bytes(b"MKCOL").unwrap(),
                        self.url(&current)?,
                        Duration::from_secs(300),
                    ),
                    cancel,
                )
                .await?;
            if !matches!(response.status().as_u16(), 200 | 201 | 204 | 405) {
                return Err(DavError::Http(response.status().as_u16()));
            }
        }
        Ok(())
    }
    pub fn download(
        &self,
        remote: String,
        root: std::fs::File,
        path: std::path::PathBuf,
    ) -> Result<Work<Result<super::local::Downloaded, DavError>>, WorkError> {
        let backend = self.clone();
        self.owner.submit_async(move |cancel| async move {
            let response = backend
                .send(
                    backend.request(Method::GET, backend.url(&remote)?, Duration::from_secs(600)),
                    &cancel,
                )
                .await?;
            if response.status().as_u16() != 200 {
                return Err(DavError::Http(response.status().as_u16()));
            }
            let expected = response
                .headers()
                .get("Content-Length")
                .map(|value| {
                    value
                        .to_str()
                        .ok()
                        .and_then(|s| s.parse::<u64>().ok())
                        .ok_or(DavError::Verification)
                })
                .transpose()?;
            let (tx, rx) = mpsc::channel(8);
            let job_cancel = cancel.clone();
            let writer = backend
                .io
                .submit_blocking(move |cancel| {
                    super::local::receive_download(
                        root,
                        &path,
                        &mut Readback {
                            rx,
                            part: std::io::Cursor::new(Vec::new()),
                            ended: false,
                        },
                        expected,
                        &|| cancel.is_cancelled() || job_cancel.is_cancelled(),
                    )
                })
                .map_err(|_| DavError::Capacity)?;
            let transferred = feed_response(response, tx, &cancel).await;
            if transferred.is_err() {
                writer.cancel();
            }
            let written = writer.wait().await.map_err(|_| DavError::Io)?;
            transferred?;
            written.map_err(|error| match error {
                super::local::TransferError::PublicationUncertain(_) => {
                    DavError::PublicationUncertain
                }
                super::local::TransferError::Io(_) => DavError::Io,
            })
        })
    }

    /// A failure/cancellation may follow an actual remote PUT. Callers must persist uncertainty.
    pub fn upload(
        &self,
        source: Arc<VerifiedSource>,
        remote: String,
    ) -> Result<Work<Result<(), DavError>>, WorkError> {
        let backend = self.clone();
        self.owner.submit_async(move|cancel|async move{
            let url=backend.url(&remote)?;
            let source_check=source.clone();backend.io.submit_blocking(move|_|source_check.check_unchanged()).map_err(|_|DavError::Capacity)?.wait().await.map_err(|_|DavError::Source)?.map_err(|_|DavError::Source)?;
            let mut last=DavError::Transport;
            for attempt in 0..backend.config.max_attempts.max(1){
                if cancel.is_cancelled(){return Err(DavError::Cancelled);}
                let result=backend.upload_once(source.clone(),&remote,url.clone(),&cancel).await;
                match result {Ok(())=>return Ok(()),Err(error)=>{last=error;if !retryable(error){return Err(error);}}}
                if attempt+1<backend.config.max_attempts.max(1){
                    let mut random=[0u8;8];getrandom::fill(&mut random).map_err(|_|DavError::Transport)?;
                    let upper=(2u64.saturating_pow(attempt.saturating_add(1))).min(60) as f64;
                    let duration=Duration::from_secs_f64(upper*(u64::from_le_bytes(random)as f64/u64::MAX as f64));
                    tokio::select!{_=cancel.cancelled()=>return Err(DavError::Cancelled),_=tokio::time::sleep(duration)=>{}}
                }
            }
            Err(last)
        })
    }
    async fn upload_once(
        &self,
        source: Arc<VerifiedSource>,
        remote: &str,
        url: Url,
        cancel: &CancellationToken,
    ) -> Result<(), DavError> {
        self.create_dirs(remote, cancel).await?;
        let (body, producer) = source_body(source.clone(), &self.io)?;
        let seconds = (source.len() / (100 * 1024 * 1024))
            .saturating_mul(60)
            .saturating_add(120)
            .max(300);
        let result = self
            .send(
                self.request(Method::PUT, url.clone(), Duration::from_secs(seconds))
                    .header("Content-Length", source.len())
                    .header("Content-Type", "application/octet-stream")
                    .body(body),
                cancel,
            )
            .await;
        // Also terminates a producer after an early response that did not consume the body.
        producer.cancel();
        let produced = producer.wait().await.map_err(|_| DavError::Source)?;
        let response = result?;
        let status = response.status().as_u16();
        drop(response);
        if !matches!(status, 200 | 201 | 204) {
            return Err(DavError::Http(status));
        }
        produced.map_err(|_| DavError::Source)?;
        if self.config.verify_head {
            let response = self
                .send(
                    self.request(Method::HEAD, url.clone(), Duration::from_secs(300)),
                    cancel,
                )
                .await?;
            if !matches!(response.status().as_u16(), 200 | 204) {
                return Err(DavError::Http(response.status().as_u16()));
            }
            if let Some(length) = response.headers().get("Content-Length") {
                let length = length
                    .to_str()
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or(DavError::Verification)?;
                if length != source.len() {
                    return Err(DavError::Verification);
                }
            }
        }
        let response = self
            .send(
                self.request(Method::GET, url, Duration::from_secs(600)),
                cancel,
            )
            .await?;
        if response.status().as_u16() != 200 {
            return Err(DavError::Http(response.status().as_u16()));
        }
        verify_response(response, source, &self.io, cancel).await
    }
}
fn retryable(error: DavError) -> bool {
    match error {
        DavError::Http(code) => !matches!(code, 400 | 401 | 403 | 405 | 409 | 413 | 422 | 507),
        DavError::Transport | DavError::Verification => true,
        _ => false,
    }
}
fn source_body(
    source: Arc<VerifiedSource>,
    owner: &Owner,
) -> Result<(Body, Work<io::Result<()>>), DavError> {
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, io::Error>>(8);
    let runtime = tokio::runtime::Handle::current();
    let work=owner.submit_blocking(move|cancel|{
        let mut file=source.reopen()?;let mut buffer=[0u8;65536];
        loop{check(&||cancel.is_cancelled())?;let count=file.read(&mut buffer)?;if count==0{break;}let part=buffer[..count].to_vec();runtime.block_on(async{tokio::select!{_=cancel.cancelled()=>Err(io::Error::new(io::ErrorKind::Interrupted,"Upload cancelled")),sent=tx.send(Ok(part))=>sent.map_err(|_|io::Error::new(io::ErrorKind::BrokenPipe,"Upload body closed"))}})?;}
        source.check_unchanged()
    }).map_err(|_|DavError::Capacity)?;
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Ok((Body::wrap_stream(stream), work))
}
enum ReadPart {
    Bytes(Vec<u8>),
    End,
}
struct Readback {
    rx: mpsc::Receiver<ReadPart>,
    part: std::io::Cursor<Vec<u8>>,
    ended: bool,
}
impl Read for Readback {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            let count = self.part.read(buffer)?;
            if count > 0 || buffer.is_empty() {
                return Ok(count);
            }
            if self.ended {
                return Ok(0);
            }
            match self.rx.blocking_recv() {
                Some(ReadPart::Bytes(bytes)) => self.part = std::io::Cursor::new(bytes),
                Some(ReadPart::End) => self.ended = true,
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Remote body incomplete",
                    ));
                }
            }
        }
    }
}
async fn feed_response(
    mut response: Response,
    tx: mpsc::Sender<ReadPart>,
    cancel: &CancellationToken,
) -> Result<(), DavError> {
    loop {
        let chunk = tokio::select! {biased;_=cancel.cancelled()=>return Err(DavError::Cancelled),chunk=response.chunk()=>chunk.map_err(|_|DavError::Transport)?};
        let Some(chunk) = chunk else { break };
        for bytes in chunk.chunks(65536) {
            tokio::select! {biased;_=cancel.cancelled()=>return Err(DavError::Cancelled),sent=tx.send(ReadPart::Bytes(bytes.to_vec()))=>sent.map_err(|_|DavError::Verification)?};
        }
    }
    tokio::select! {biased;_=cancel.cancelled()=>return Err(DavError::Cancelled),sent=tx.send(ReadPart::End)=>sent.map_err(|_|DavError::Verification)?};
    Ok(())
}

async fn verify_response(
    response: Response,
    source: Arc<VerifiedSource>,
    owner: &Owner,
    cancel: &CancellationToken,
) -> Result<(), DavError> {
    let (tx, rx) = mpsc::channel(8);
    let verifier = owner
        .submit_blocking(move |cancel| {
            source.verify_download(
                &mut Readback {
                    rx,
                    part: std::io::Cursor::new(Vec::new()),
                    ended: false,
                },
                &|| cancel.is_cancelled(),
            )
        })
        .map_err(|_| DavError::Capacity)?;
    let transfer = feed_response(response, tx, cancel).await;
    if transfer.is_err() {
        verifier.cancel();
    }
    let verified = verifier.wait().await.map_err(|_| DavError::Verification)?;
    transfer?;
    verified.map_err(|_| DavError::Verification)
}
