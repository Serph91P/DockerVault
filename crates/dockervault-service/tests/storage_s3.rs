use dockervault_service::{
    ownership::Owner,
    storage::s3::{S3, S3Config},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn configured(address: std::net::SocketAddr, owner: Owner) -> S3 {
    S3::new(
        S3Config {
            endpoint_url: Some(format!("http://{address}")),
            region: String::new(),
            bucket: "public-bucket".to_owned(),
            base_path: "PUBLIC-prefix".to_owned(),
            access_key: "PUBLIC_TEST_ACCESS".to_owned(),
            secret_key: "PUBLIC synthetic secret".to_owned(),
        },
        owner,
    )
    .unwrap()
}

async fn read_headers(socket: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.push(socket.read_u8().await.unwrap());
        assert!(request.len() < 65536);
    }
    String::from_utf8(request).unwrap()
}

#[tokio::test]
async fn s3_delete_waits_for_remote_settlement_after_cancel() {
    use dockervault_service::storage::s3::S3Error;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tasks = Owner::new(1).unwrap();
    let storage = configured(listener.local_addr().unwrap(), tasks.clone());
    let (received, ready) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_headers(&mut socket).await;
        assert!(request.starts_with("DELETE /public-bucket/PUBLIC-prefix/sub/%C3%A9%20%25%3F.enc?x-id=DeleteObject HTTP/1.1"), "{request}");
        assert!(request.contains("AWS4-HMAC-SHA256"));
        received.send(()).unwrap();
        released.await.unwrap();
        socket
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let work = storage.delete("sub/é %?.enc".to_owned()).unwrap();
    ready.await.unwrap();
    work.cancel();
    assert_eq!(tasks.active(), 1);
    assert!(storage.test_connection().is_err());
    release.send(()).unwrap();
    assert!(matches!(
        work.wait().await.unwrap(),
        Err(S3Error::Cancelled)
    ));
    server.await.unwrap();
    assert_eq!(tasks.active(), 0);
    tasks.shutdown(None).await;
}

#[tokio::test]
async fn s3_listing_consumes_all_pages_and_preserves_wire_metadata() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tasks = Owner::new(1).unwrap();
    let storage = configured(listener.local_addr().unwrap(), tasks.clone());
    let server = tokio::spawn(async move {
        for index in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_headers(&mut socket).await;
            assert!(request.starts_with("GET /public-bucket/?"), "{request}");
            assert!(request.contains("list-type=2"));
            assert!(request.contains("prefix=PUBLIC-prefix%2Fsub%2F"));
            if index == 1 {
                assert!(
                    request.contains("continuation-token=PUBLIC%2B%2F%3D"),
                    "{request}"
                );
            }
            let body = if index == 0 {
                "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><IsTruncated>true</IsTruncated><NextContinuationToken>PUBLIC+/=</NextContinuationToken><Contents><Key>PUBLIC-prefix/sub/é.txt</Key><Size>17</Size><LastModified>2024-01-02T03:04:05.500Z</LastModified></Contents></ListBucketResult>"
            } else {
                "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><IsTruncated>false</IsTruncated><Contents><Key>PUBLIC-prefix/sub/folder/</Key><Size>0</Size></Contents></ListBucketResult>"
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/xml\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
    });
    let entries = storage
        .list_files("sub".to_owned())
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "é.txt");
    assert_eq!(entries[0].size, 17);
    assert_eq!(entries[0].modified, 1704164645.5);
    assert!(!entries[0].is_dir);
    assert_eq!(entries[1].name, "folder/");
    assert_eq!(entries[1].modified, 0.0);
    assert!(!entries[1].is_dir);
    server.await.unwrap();
    assert_eq!(tasks.active(), 0);
    tasks.shutdown(None).await;
}
#[tokio::test]
async fn s3_listing_rejects_missing_and_repeated_continuations_without_partial_success() {
    use dockervault_service::storage::s3::S3Error;
    for repeated in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tasks = Owner::new(1).unwrap();
        let storage = configured(listener.local_addr().unwrap(), tasks.clone());
        let server = tokio::spawn(async move {
            let pages = if repeated { 2 } else { 1 };
            for _ in 0..pages {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_headers(&mut socket).await;
                let token = if repeated {
                    "<NextContinuationToken>PUBLIC-repeated</NextContinuationToken>"
                } else {
                    ""
                };
                let body = format!(
                    "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><IsTruncated>true</IsTruncated>{token}<Contents><Key>PUBLIC-prefix/partial.txt</Key><Size>1</Size></Contents></ListBucketResult>"
                );
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/xml\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        assert!(matches!(
            storage
                .list_files(String::new())
                .unwrap()
                .wait()
                .await
                .unwrap(),
            Err(S3Error::Listing)
        ));
        server.await.unwrap();
        assert_eq!(tasks.active(), 0);
        tasks.shutdown(None).await;
    }
}

#[tokio::test]
async fn s3_delete_denial_preserves_status_without_disclosing_response_secrets() {
    use dockervault_service::storage::s3::S3Error;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tasks = Owner::new(1).unwrap();
    let storage = configured(listener.local_addr().unwrap(), tasks.clone());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_headers(&mut socket).await;
        let body = "<Error><Code>AccessDenied</Code><Message>PUBLIC-response-must-not-escape</Message></Error>";
        socket.write_all(format!("HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nContent-Type: application/xml\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    let error = storage
        .delete("file".to_owned())
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, S3Error::Http(403)));
    assert_eq!(error.to_string(), "S3 HTTP status 403");
    assert!(!format!("{error:?}").contains("PUBLIC-response"));
    server.await.unwrap();
    assert_eq!(tasks.active(), 0);
    tasks.shutdown(None).await;
}

#[tokio::test]
async fn s3_bucket_probe_uses_explicit_signed_configuration_on_real_tcp() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let byte = socket.read_u8().await.unwrap();
            request.push(byte);
            assert!(request.len() < 65536);
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("HEAD /public-bucket"));
        assert!(request.contains("AWS4-HMAC-SHA256"));
        assert!(request.contains("Credential=PUBLIC_TEST_ACCESS/"));
        assert!(request.contains("/us-east-1/s3/aws4_request"));
        assert!(!request.contains("PUBLIC synthetic secret"));
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let tasks = Owner::new(1).unwrap();
    let storage = S3::new(
        S3Config {
            endpoint_url: Some(format!("http://{address}")),
            region: "us-east-1".to_owned(),
            bucket: "public-bucket".to_owned(),
            base_path: "PUBLIC-prefix".to_owned(),
            access_key: "PUBLIC_TEST_ACCESS".to_owned(),
            secret_key: "PUBLIC synthetic secret".to_owned(),
        },
        tasks.clone(),
    )
    .unwrap();
    assert_eq!(storage.key("/sub/é %?.enc"), "PUBLIC-prefix//sub/é %?.enc");
    storage
        .test_connection()
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    assert_eq!(tasks.active(), 0);
    tasks.shutdown(None).await;
}
