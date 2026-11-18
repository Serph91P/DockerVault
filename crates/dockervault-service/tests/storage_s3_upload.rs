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
async fn s3_single_put_cancel_keeps_the_owner_until_the_non_abortable_request_replies() {
    let fixture = Fixture::new();
    let bytes = b"public single-put fixture";
    let source = fixture.source(bytes);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let (stored, stored_seen) = tokio::sync::oneshot::channel();
    let (ack, ack_allowed) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, body) = request(&mut socket).await;
        let mut request_line = headers.lines().next().unwrap().split_ascii_whitespace();
        assert_eq!(request_line.next(), Some("PUT"));
        let target = request_line.next().unwrap();
        assert_eq!(
            target.split('?').next(),
            Some("/public-bucket/PUBLIC-prefix/archive")
        );
        assert_eq!(body, bytes);
        stored.send(()).unwrap();
        ack_allowed.await.unwrap();
        respond(&mut socket, &[], "").await;
        drop(socket);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "cancelled PUT must not replay, delete or verify"
        );
    });
    let work = backend
        .upload(source, "archive".into(), S3UploadPolicy::default())
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), stored_seen)
        .await
        .unwrap()
        .unwrap();
    work.cancel();
    let mut result = std::pin::pin!(work.wait());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut result)
            .await
            .is_err(),
        "cancellation returned while the provider still held the PUT reply"
    );
    assert_eq!(owner.active(), 1);
    assert!(backend.test_connection().is_err());
    ack.send(()).unwrap();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), result)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        outcome,
        Err(dockervault_service::storage::s3::S3Error::Cancelled)
    ));
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    server.await.unwrap();
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
async fn s3_retry_replays_from_zero_and_rejects_a_replaced_source_before_another_effect() {
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
            for attempt in 0..(if replaced { 1 } else { 2 }) {
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
            if !replaced {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (headers, _) = request(&mut socket).await;
                assert!(headers.starts_with("GET "));
                respond(&mut socket, &bytes, "").await;
            }
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
        S3Error::MultipartUnsettled { upload_id, primary } => {
            assert_eq!(upload_id, "PUBLIC-unsettled-id");
            assert!(matches!(*primary, S3Error::Cancelled));
        }
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
async fn s3_cancel_during_part_reply_stall_reaches_abort_without_waiting_the_read_timeout() {
    let fixture = Fixture::new();
    let source = fixture.source(&vec![0; 5 * 1024 * 1024]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let (part, part_seen) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        respond(&mut socket, b"<InitiateMultipartUploadResult><UploadId>PUBLIC-stall-id</UploadId></InitiateMultipartUploadResult>", "Content-Type: application/xml\r\n").await;
        drop(socket);
        let (mut part_socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut part_socket).await;
        assert!(headers.starts_with("PUT "));
        part.send(()).unwrap();
        let (mut abort_socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut abort_socket).await;
        assert!(headers.starts_with("DELETE ") && headers.contains("uploadId=PUBLIC-stall-id"));
        // Server settles the stalled part before acknowledging that abort is complete.
        let _ = part_socket
            .write_all(b"HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        drop(part_socket);
        respond(&mut abort_socket, &[], "").await;
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
    part_seen.await.unwrap();
    work.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), work.wait())
        .await
        .expect("cancel must reach multipart API/abort, not wait for the 300s read timeout")
        .unwrap();
    assert_eq!(
        result.unwrap_err().to_string(),
        "S3 operation cancelled; remote effect may exist"
    );
    server.await.unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    owner.shutdown(None).await;
    io.shutdown(None).await;
}

#[tokio::test]
async fn s3_cancel_during_complete_reply_stall_settles_abort_before_reporting_cancelled() {
    let fixture = Fixture::new();
    let source = fixture.source(&vec![0; 5 * 1024 * 1024]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let (complete, complete_seen) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        respond(&mut socket, b"<InitiateMultipartUploadResult><UploadId>PUBLIC-complete-stall-id</UploadId></InitiateMultipartUploadResult>", "Content-Type: application/xml\r\n").await;
        drop(socket);
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut socket).await;
        assert!(headers.starts_with("PUT "));
        respond(&mut socket, &[], "ETag: \"PUBLIC-part\"\r\n").await;
        drop(socket);
        let (mut complete_socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut complete_socket).await;
        assert!(
            headers.starts_with("POST ") && headers.contains("uploadId=PUBLIC-complete-stall-id")
        );
        complete.send(()).unwrap();
        let (mut abort_socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut abort_socket).await;
        assert!(
            headers.starts_with("DELETE ") && headers.contains("uploadId=PUBLIC-complete-stall-id")
        );
        let _ = complete_socket
            .write_all(b"HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        drop(complete_socket);
        respond(&mut abort_socket, &[], "").await;
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
    complete_seen.await.unwrap();
    assert_eq!(
        io.active(),
        0,
        "part producer must already have physically joined"
    );
    work.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), work.wait())
        .await
        .expect("complete cancellation must reach cleanup")
        .unwrap();
    assert_eq!(
        result.unwrap_err().to_string(),
        "S3 operation cancelled; remote effect may exist"
    );
    server.await.unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    owner.shutdown(None).await;
    io.shutdown(None).await;
}

#[tokio::test]
async fn s3_part_body_backpressure_cancel_joins_the_real_bounded_file_producer_before_abort_ack() {
    let fixture = Fixture::new();
    let path = fixture.0.join("source");
    fs::File::create(&path)
        .unwrap()
        .set_len(32 * 1024 * 1024)
        .unwrap();
    let source = Arc::new(VerifiedSource::open(&path, &|| false).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let (part, part_seen) = tokio::sync::oneshot::channel();
    let (aborting, abort_seen) = tokio::sync::oneshot::channel();
    let (release_abort, released_abort) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        respond(&mut socket, b"<InitiateMultipartUploadResult><UploadId>PUBLIC-body-stall-id</UploadId></InitiateMultipartUploadResult>", "Content-Type: application/xml\r\n").await;
        drop(socket);
        let (mut part_socket, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.extend(line(&mut part_socket).await);
            assert!(headers.len() < 65536);
        }
        assert!(String::from_utf8(headers).unwrap().starts_with("PUT "));
        part.send(()).unwrap();
        // Deliberately never read the 32 MiB body: real TCP backpressure must bound the producer.
        let (mut abort_socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut abort_socket).await;
        assert!(
            headers.starts_with("DELETE ") && headers.contains("uploadId=PUBLIC-body-stall-id")
        );
        drop(part_socket);
        aborting.send(()).unwrap();
        released_abort.await.unwrap();
        respond(&mut abort_socket, &[], "").await;
    });
    let work = backend
        .upload(
            source,
            "archive".to_owned(),
            S3UploadPolicy {
                multipart_threshold_bytes: 5 * 1024 * 1024,
                multipart_chunk_size: 32 * 1024 * 1024,
            },
        )
        .unwrap();
    part_seen.await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while io.active() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("streaming body must not be pre-collected into memory before HTTP headers");
    work.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), abort_seen)
        .await
        .expect("part body cancellation must reach actual abort")
        .unwrap();
    assert_eq!(
        io.active(),
        0,
        "blocking reader must physically join before remote abort acknowledgement"
    );
    assert_eq!(owner.active(), 1);
    release_abort.send(()).unwrap();
    assert_eq!(
        work.wait().await.unwrap().unwrap_err().to_string(),
        "S3 operation cancelled; remote effect may exist"
    );
    server.await.unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(fs::metadata(path).unwrap().len(), 32 * 1024 * 1024);
    owner.shutdown(None).await;
    io.shutdown(None).await;
}

#[tokio::test]
async fn s3_upload_retry_budget_is_four_physical_requests_not_multiplied_by_sdk_retries() {
    let fixture = Fixture::new();
    let bytes = vec![7; 150003];
    let source = fixture.source(&bytes);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, stored) = request(&mut socket).await;
            assert!(headers.starts_with("PUT "));
            assert_eq!(stored, bytes);
            let body = b"<Error><Code>SlowDown</Code></Error>";
            socket.write_all(format!("HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
        }
    });
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        backend
            .upload(source, "archive".to_owned(), S3UploadPolicy::default())
            .unwrap()
            .wait(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap_err();
    assert_eq!(error.to_string(), "S3 HTTP status 503");
    server.await.unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    owner.shutdown(None).await;
    io.shutdown(None).await;
}

#[tokio::test]
async fn s3_abort_failure_keeps_primary_upload_code_and_status_in_the_retained_obligation() {
    use dockervault_service::storage::s3::S3Error;
    let fixture = Fixture::new();
    let source = fixture.source(&vec![0; 5 * 1024 * 1024]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        respond(&mut socket, b"<InitiateMultipartUploadResult><UploadId>PUBLIC-primary-error-id</UploadId></InitiateMultipartUploadResult>", "Content-Type: application/xml\r\n").await;
        drop(socket);
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut socket).await;
        assert!(headers.starts_with("PUT "));
        let body = b"<Error><Code>SourceRejected</Code><Message>PUBLIC-message-must-not-escape</Message></Error>";
        socket
            .write_all(
                format!(
                    "HTTP/1.1 409 Conflict\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        socket.write_all(body).await.unwrap();
        drop(socket);
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut socket).await;
        assert!(headers.starts_with("DELETE "));
        let body = b"<Error><Code>AccessDenied</Code></Error>";
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
    let error = backend
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
        .unwrap_err();
    assert_eq!(error.http_status(), Some(409));
    assert_eq!(error.provider_code(), Some("SourceRejected"));
    assert!(!format!("{error:?} {error}").contains("PUBLIC-"));
    match error {
        S3Error::MultipartUnsettled { upload_id, .. } => {
            assert_eq!(upload_id, "PUBLIC-primary-error-id")
        }
        _ => panic!("lost uncertain multipart obligation"),
    }
    server.await.unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    owner.shutdown(None).await;
    io.shutdown(None).await;
}

#[tokio::test]
async fn s3_unreturned_creation_id_is_bounded_but_not_falsely_reported_as_settled_cancel() {
    let fixture = Fixture::new();
    let source = fixture.source(&vec![0; 5 * 1024 * 1024]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), owner.clone(), io.clone());
    let (created, creation_seen) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let pending_uploads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let provider_uploads = pending_uploads.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = request(&mut socket).await;
        assert!(headers.contains("?uploads"));
        provider_uploads.lock().unwrap().push("PUBLIC-late-id");
        created.send(()).unwrap();
        released.await.unwrap();
        // Effect exists at the provider, but its ID reaches the client too late.
        let body = b"<InitiateMultipartUploadResult><UploadId>PUBLIC-late-id</UploadId></InitiateMultipartUploadResult>";
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await;
        let _ = socket.write_all(body).await;
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
    // Only the clock is virtual: the request above used the real SDK and TCP endpoint.
    tokio::time::pause();
    work.cancel();
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(11)).await;
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), work.wait()).await.expect("unreturned creation must retain uncertainty after the 10s cleanup budget, not wait for the 300s read timeout").unwrap();
    let error = result.unwrap_err();
    assert_eq!(
        error.to_string(),
        "S3 multipart creation is unconfirmed; retain its journal obligation"
    );
    assert!(matches!(
        error,
        dockervault_service::storage::s3::S3Error::CreationUnsettled { .. }
    ));
    assert_eq!(*pending_uploads.lock().unwrap(), ["PUBLIC-late-id"]);
    tokio::time::resume();
    release.send(()).unwrap();
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
