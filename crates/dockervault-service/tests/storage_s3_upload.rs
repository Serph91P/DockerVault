//! Actual signed HTTP exchanges, replayable file bodies and independent GET verification.
use dockervault_service::{
    ownership::Owner,
    storage::{
        VerifiedSource,
        s3::{S3, S3Config, S3UploadPolicy},
    },
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-s3-upload-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn source(&self, bytes: &[u8]) -> Arc<VerifiedSource> {
        let path = self.0.join("source");
        fs::write(&path, bytes).unwrap();
        Arc::new(VerifiedSource::open(&path, &|| false).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn configured(address: std::net::SocketAddr, owner: Owner, io: Owner) -> S3 {
    S3::new(
        S3Config {
            endpoint_url: Some(format!("http://{address}")),
            region: "us-east-1".to_owned(),
            bucket: "public-bucket".to_owned(),
            base_path: "PUBLIC-prefix".to_owned(),
            access_key: "PUBLIC_TEST_ACCESS".to_owned(),
            secret_key: "PUBLIC synthetic secret".to_owned(),
        },
        owner,
        io,
    )
    .unwrap()
}
async fn line(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n") {
        bytes.push(socket.read_u8().await.unwrap());
        assert!(bytes.len() < 65536);
    }
    bytes
}
async fn request(socket: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        headers.extend(line(socket).await);
        assert!(headers.len() < 65536);
    }
    let headers = String::from_utf8(headers).unwrap();
    assert!(headers.contains("AWS4-HMAC-SHA256"));
    assert!(!headers.contains("PUBLIC synthetic secret"));
    let lower = headers.to_ascii_lowercase();
    if lower.contains("expect: 100-continue") {
        socket
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .await
            .unwrap();
    }
    let mut body = Vec::new();
    if lower.contains("transfer-encoding: chunked") {
        loop {
            let size = line(socket).await;
            let size = usize::from_str_radix(
                std::str::from_utf8(&size)
                    .unwrap()
                    .trim()
                    .split(';')
                    .next()
                    .unwrap(),
                16,
            )
            .unwrap();
            if size == 0 {
                while line(socket).await != b"\r\n" {}
                break;
            }
            assert!(body.len() + size < 8 * 1024 * 1024);
            let start = body.len();
            body.resize(start + size, 0);
            socket.read_exact(&mut body[start..]).await.unwrap();
            assert_eq!(line(socket).await, b"\r\n");
        }
    } else if let Some(size) = lower
        .lines()
        .find_map(|line| line.strip_prefix("content-length: "))
    {
        let size: usize = size.trim().parse().unwrap();
        assert!(size < 8 * 1024 * 1024);
        body.resize(size, 0);
        socket.read_exact(&mut body).await.unwrap();
    }
    if lower.contains("content-encoding: aws-chunked") {
        let mut rest = body.as_slice();
        let mut decoded = Vec::new();
        loop {
            let newline = rest.windows(2).position(|part| part == b"\r\n").unwrap();
            let size = usize::from_str_radix(
                std::str::from_utf8(&rest[..newline])
                    .unwrap()
                    .split(';')
                    .next()
                    .unwrap(),
                16,
            )
            .unwrap();
            rest = &rest[newline + 2..];
            if size == 0 {
                break;
            }
            decoded.extend_from_slice(&rest[..size]);
            assert_eq!(&rest[size..size + 2], b"\r\n");
            rest = &rest[size + 2..];
        }
        body = decoded;
    }
    (headers, body)
}
async fn respond(socket: &mut tokio::net::TcpStream, body: &[u8], extra: &str) {
    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    socket.write_all(body).await.unwrap();
}
#[tokio::test]
async fn s3_put_is_not_success_until_full_independent_get_hash_matches() {
    for corrupt in [false, true] {
        let fixture = Fixture::new();
        let bytes: Vec<_> = (0..200003).map(|i| (i % 251) as u8).collect();
        let source = fixture.source(&bytes);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let owner = Owner::new(1).unwrap();
        let io = Owner::new(1).unwrap();
        let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, mut stored) = request(&mut socket).await;
            assert!(headers.starts_with("PUT /public-bucket/PUBLIC-prefix/archive"));
            assert_eq!(stored, bytes);
            respond(&mut socket, &[], "ETag: \"PUBLIC-not-a-checksum\"\r\n").await;
            drop(socket);
            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, _) = request(&mut socket).await;
            assert!(headers.starts_with("GET /public-bucket/PUBLIC-prefix/archive"));
            if corrupt {
                stored[17000] ^= 1;
            }
            respond(&mut socket, &stored, "").await;
        });
        let result = backend
            .upload(source, "archive".to_owned(), S3UploadPolicy::default())
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(result.is_ok(), !corrupt);
        if corrupt {
            assert_eq!(
                result.unwrap_err().to_string(),
                "S3 independent byte verification failed"
            );
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owner.active(), 0);
        assert_eq!(io.active(), 0);
        owner.shutdown(None).await;
        io.shutdown(None).await;
    }
}
#[tokio::test]
async fn s3_retry_reopens_the_held_file_not_a_replaced_source_name() {
    for replaced in [false, true] {
        let fixture = Fixture::new();
        let bytes: Vec<_> = (0..200003).map(|i| (i % 251) as u8).collect();
        let source = fixture.source(&bytes);
        let path = fixture.0.join("source");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let owner = Owner::new(1).unwrap();
        let io = Owner::new(1).unwrap();
        let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (headers, stored) = request(&mut socket).await;
                assert!(headers.starts_with("PUT "));
                assert_eq!(
                    stored, bytes,
                    "every retry must start at offset zero in the retained inode"
                );
                if attempt == 0 {
                    if replaced {
                        fs::rename(&path, path.with_extension("original")).unwrap();
                        fs::write(&path, vec![91; bytes.len()]).unwrap();
                    }
                    let body = b"<Error><Code>SlowDown</Code></Error>";
                    socket.write_all(format!("HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
                    socket.write_all(body).await.unwrap();
                } else {
                    respond(&mut socket, &[], "").await;
                }
            }
            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, _) = request(&mut socket).await;
            assert!(headers.starts_with("GET "));
            respond(&mut socket, &bytes, "").await;
        });
        let result = backend
            .upload(source, "archive".to_owned(), S3UploadPolicy::default())
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(result.is_ok(), !replaced);
        server.await.unwrap();
        if replaced {
            assert_eq!(
                fs::read(fixture.0.join("source")).unwrap(),
                vec![91; 200003]
            );
        }
        assert_eq!(owner.active(), 0);
        assert_eq!(io.active(), 0);
        owner.shutdown(None).await;
        io.shutdown(None).await;
    }
}

#[tokio::test]
async fn s3_failed_abort_returns_the_exact_obligation_but_never_logs_provider_tokens() {
    use dockervault_service::storage::s3::S3Error;
    let fixture = Fixture::new();
    let source = fixture.source(&vec![0; 5 * 1024 * 1024]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let (created, creation_seen) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        created.send(()).unwrap();
        released.await.unwrap();
        respond(&mut socket, b"<InitiateMultipartUploadResult><UploadId>PUBLIC-unsettled-id</UploadId></InitiateMultipartUploadResult>", "Content-Type: application/xml\r\n").await;
        drop(socket);
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut socket).await;
        assert!(headers.starts_with("DELETE ") && headers.contains("uploadId=PUBLIC-unsettled-id"));
        let body = b"<Error><Code>AccessDenied</Code><Message>PUBLIC-provider-token-not-for-logs</Message></Error>";
        socket
            .write_all(
                format!(
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        socket.write_all(body).await.unwrap();
    });
    let work = backend
        .upload(
            source,
            "archive".to_owned(),
            S3UploadPolicy {
                multipart_threshold_bytes: 5 * 1024 * 1024,
                multipart_chunk_size: 5 * 1024 * 1024,
            },
        )
        .unwrap();
    creation_seen.await.unwrap();
    work.cancel();
    release.send(()).unwrap();
    let error = work.wait().await.unwrap().unwrap_err();
    assert!(!format!("{error:?} {error}").contains("PUBLIC-"));
    match error {
        S3Error::MultipartUnsettled { upload_id } => assert_eq!(upload_id, "PUBLIC-unsettled-id"),
        _ => panic!("lost provider cleanup obligation"),
    }
    server.await.unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    owner.shutdown(None).await;
    io.shutdown(None).await;
}

#[tokio::test]
async fn s3_multipart_streams_each_part_then_verifies_the_complete_object() {
    let fixture = Fixture::new();
    let bytes: Vec<_> = (0..6 * 1024 * 1024 + 17).map(|i| (i % 251) as u8).collect();
    let source = fixture.source(&bytes);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut socket).await;
        assert!(headers.starts_with("POST /public-bucket/PUBLIC-prefix/archive?uploads"));
        respond(&mut socket, b"<InitiateMultipartUploadResult><UploadId>PUBLIC-id</UploadId></InitiateMultipartUploadResult>", "Content-Type: application/xml\r\n").await;
        drop(socket);
        let mut stored = Vec::new();
        let mut checksums = Vec::new();
        for part in 1..=2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, body) = request(&mut socket).await;
            assert!(headers.starts_with("PUT /public-bucket/PUBLIC-prefix/archive?"));
            assert!(headers.contains(&format!("partNumber={part}")));
            assert!(headers.contains("uploadId=PUBLIC-id"));
            use base64::Engine;
            use sha2::Digest;
            let checksum =
                base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(&body));
            checksums.push(checksum.clone());
            assert_eq!(
                body.len(),
                if part == 1 {
                    5 * 1024 * 1024
                } else {
                    1024 * 1024 + 17
                }
            );
            stored.extend(body);
            respond(
                &mut socket,
                &[],
                &format!("ETag: \"PUBLIC-part-{part}\"\r\nx-amz-checksum-sha256: {checksum}\r\n"),
            )
            .await;
        }
        assert_eq!(stored, bytes);
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, body) = request(&mut socket).await;
        assert!(
            headers.starts_with("POST /public-bucket/PUBLIC-prefix/archive?uploadId=PUBLIC-id")
        );
        let xml = String::from_utf8(body).unwrap();
        assert!(xml.contains("PUBLIC-part-1") && xml.contains("PUBLIC-part-2"));
        for checksum in checksums {
            assert!(
                xml.contains(&format!("<ChecksumSHA256>{checksum}</ChecksumSHA256>")),
                "provider part checksums must accompany ETags at completion"
            );
        }
        respond(&mut socket, b"<CompleteMultipartUploadResult><ETag>PUBLIC-not-a-checksum</ETag></CompleteMultipartUploadResult>", "Content-Type: application/xml\r\n").await;
        drop(socket);
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut socket).await;
        assert!(headers.starts_with("GET "));
        respond(&mut socket, &stored, "").await;
    });
    backend
        .upload(
            source,
            "archive".to_owned(),
            S3UploadPolicy {
                multipart_threshold_bytes: 5 * 1024 * 1024,
                multipart_chunk_size: 5 * 1024 * 1024,
            },
        )
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    owner.shutdown(None).await;
    io.shutdown(None).await;
}
#[tokio::test]
async fn s3_cancel_during_creation_waits_for_id_and_actual_abort_response() {
    let fixture = Fixture::new();
    let source = fixture.source(&vec![0; 5 * 1024 * 1024]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let (created, creation_seen) = tokio::sync::oneshot::channel();
    let (release_creation, released_creation) = tokio::sync::oneshot::channel();
    let (aborted, abort_seen) = tokio::sync::oneshot::channel();
    let (release_abort, released_abort) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut socket).await;
        assert!(headers.contains("?uploads"));
        created.send(()).unwrap();
        released_creation.await.unwrap();
        respond(&mut socket, b"<InitiateMultipartUploadResult><UploadId>PUBLIC-cancel-id</UploadId></InitiateMultipartUploadResult>", "Content-Type: application/xml\r\n").await;
        drop(socket);
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut socket).await;
        assert!(headers.starts_with("DELETE ") && headers.contains("uploadId=PUBLIC-cancel-id"));
        aborted.send(()).unwrap();
        released_abort.await.unwrap();
        respond(&mut socket, &[], "").await;
    });
    let work = backend
        .upload(
            source,
            "archive".to_owned(),
            S3UploadPolicy {
                multipart_threshold_bytes: 5 * 1024 * 1024,
                multipart_chunk_size: 5 * 1024 * 1024,
            },
        )
        .unwrap();
    creation_seen.await.unwrap();
    work.cancel();
    assert_eq!(owner.active(), 1);
    release_creation.send(()).unwrap();
    abort_seen.await.unwrap();
    assert_eq!(owner.active(), 1);
    assert!(backend.test_connection().is_err());
    release_abort.send(()).unwrap();
    assert_eq!(
        work.wait().await.unwrap().unwrap_err().to_string(),
        "S3 operation cancelled; remote effect may exist"
    );
    server.await.unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    owner.shutdown(None).await;
    io.shutdown(None).await;
}
