use dockervault_service::{
    ownership::Owner,
    storage::s3::{S3, S3Config},
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-s3-download-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
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
async fn headers(socket: &mut tokio::net::TcpStream) {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.push(socket.read_u8().await.unwrap());
        assert!(request.len() < 65536);
    }
    let request = String::from_utf8(request).unwrap();
    assert!(request.starts_with("GET /public-bucket/PUBLIC-prefix/archive"));
    assert!(request.contains("AWS4-HMAC-SHA256"));
}
#[tokio::test]
async fn s3_download_publishes_only_complete_bytes_and_computes_independent_sha256() {
    use sha2::{Digest, Sha256};
    for truncated in [false, true] {
        let fixture = Fixture::new();
        let bytes: Vec<_> = (0..200003).map(|i| (i % 251) as u8).collect();
        let expected: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        fs::write(fixture.0.join("archive"), b"existing destination").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tasks = Owner::new(1).unwrap();
        let io = Owner::new(1).unwrap();
        let backend = configured(listener.local_addr().unwrap(), tasks.clone(), io.clone());
        let remote = bytes.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            headers(&mut socket).await;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        remote.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let sent = if truncated {
                remote.len() - 7
            } else {
                remote.len()
            };
            socket.write_all(&remote[..sent]).await.unwrap();
        });
        let result = backend
            .download(
                "archive".to_owned(),
                fs::File::open(&fixture.0).unwrap(),
                PathBuf::from("archive"),
            )
            .unwrap()
            .wait()
            .await
            .unwrap();
        if truncated {
            assert!(result.is_err());
            assert_eq!(
                fs::read(fixture.0.join("archive")).unwrap(),
                b"existing destination"
            );
        } else {
            let receipt = result.unwrap();
            assert_eq!(receipt.len(), bytes.len() as u64);
            assert_eq!(receipt.sha256_hex(), expected);
            assert_eq!(fs::read(fixture.0.join("archive")).unwrap(), bytes);
        }
        server.await.unwrap();
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
        assert_eq!(tasks.active(), 0);
        assert_eq!(io.active(), 0);
        tasks.shutdown(None).await;
        io.shutdown(None).await;
    }
}
#[tokio::test]
async fn s3_cancel_before_get_headers_does_not_start_or_publish_a_file_writer() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("archive"), b"existing destination").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tasks = Owner::new(1).unwrap();
    let io = Owner::new(1).unwrap();
    let backend = configured(listener.local_addr().unwrap(), tasks.clone(), io.clone());
    let (received, ready) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        headers(&mut socket).await;
        received.send(()).unwrap();
        released.await.unwrap();
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
    });
    let work = backend
        .download(
            "archive".to_owned(),
            fs::File::open(&fixture.0).unwrap(),
            PathBuf::from("archive"),
        )
        .unwrap();
    ready.await.unwrap();
    assert_eq!(io.active(), 0);
    work.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), work.wait())
        .await
        .expect("GET cancellation must not wait for response headers")
        .unwrap();
    assert_eq!(
        result.unwrap_err().to_string(),
        "S3 operation cancelled; remote effect may exist"
    );
    release.send(()).unwrap();
    server.await.unwrap();
    assert_eq!(
        fs::read(fixture.0.join("archive")).unwrap(),
        b"existing destination"
    );
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    assert_eq!(tasks.active(), 0);
    assert_eq!(io.active(), 0);
    tasks.shutdown(None).await;
    io.shutdown(None).await;
}

#[tokio::test]
async fn s3_body_cancel_physically_settles_writer_without_publishing_or_leaking_stage() {
    for shutdown_io in [false, true] {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("archive"), b"existing destination").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tasks = Owner::new(1).unwrap();
        let io = Owner::new(1).unwrap();
        let backend = configured(listener.local_addr().unwrap(), tasks.clone(), io.clone());
        let (received, ready) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            headers(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 200003\r\nConnection: close\r\n\r\nPUBLIC prefix").await.unwrap();
            received.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
        });
        let work = backend
            .download(
                "archive".to_owned(),
                fs::File::open(&fixture.0).unwrap(),
                PathBuf::from("archive"),
            )
            .unwrap();
        ready.await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while io.active() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if shutdown_io {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                io.shutdown(Some(std::time::Duration::ZERO)),
            )
            .await
            .unwrap();
        } else {
            work.cancel();
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), work.wait())
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        server.await.unwrap();
        assert_eq!(
            fs::read(fixture.0.join("archive")).unwrap(),
            b"existing destination"
        );
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
        assert_eq!(tasks.active(), 0);
        assert_eq!(io.active(), 0);
        tasks.shutdown(None).await;
        io.shutdown(None).await;
    }
}
