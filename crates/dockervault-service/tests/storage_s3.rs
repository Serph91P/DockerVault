use dockervault_service::{
    ownership::Owner,
    storage::s3::{S3, S3Config},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
