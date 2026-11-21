use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn owned_generation_cleanup_refuses_a_replaced_name_and_symlink_parent() {
    let root = std::env::temp_dir().join(format!("dv-generation-identity-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("generation");
    let generation = Generation::create(path.clone()).unwrap();
    std::fs::write(path.join("owned"), b"PUBLIC-owned").unwrap();
    std::fs::rename(&path, root.join("moved")).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("foreign"), b"PUBLIC-foreign").unwrap();
    assert_eq!(generation.finish(false), Err(Invalid));
    assert_eq!(
        std::fs::read(path.join("foreign")).unwrap(),
        b"PUBLIC-foreign"
    );
    assert_eq!(
        std::fs::read(root.join("moved/owned")).unwrap(),
        b"PUBLIC-owned"
    );
    let link = root.join("link");
    std::os::unix::fs::symlink(root.join("moved"), &link).unwrap();
    assert!(Generation::create(link.join("new")).is_err());
    assert!(!root.join("moved/new").exists());
    drop(generation);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn private_unix_rpc_rejects_error_truncation_and_overflow_without_disclosing_body() {
    let root = std::env::temp_dir().join(format!("dv-private-rpc-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let socket = root.join("rc");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let client = reqwest::Client::builder()
        .unix_socket(socket.as_path())
        .no_proxy()
        .build()
        .unwrap();
    let mut exact = b"[]".to_vec();
    exact.resize(OUTPUT_LIMIT, b' ');
    let mut overflow = exact.clone();
    overflow.push(b' ');
    let cases = vec![
        (200, b"{}".to_vec(), 2, true),
        (500, b"PUBLIC secret response".to_vec(), 22, false),
        (200, b"{".to_vec(), 1, false),
        (200, b"{}".to_vec(), 3, false),
        (200, exact, OUTPUT_LIMIT, true),
        (200, overflow, OUTPUT_LIMIT + 1, false),
    ];
    for (status, body, length, accepted) in cases {
        let cancel = CancellationToken::new();
        let data = json!({});
        let (result, ()) = tokio::join!(rpc(&client, "rc/noop", &data, &cancel), async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"POST /rc/noop HTTP/1.1\r\n"));
            let _=stream.write_all(format!("HTTP/1.1 {status} Status\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n").as_bytes()).await;
            let _ = stream.write_all(&body).await;
        });
        assert_eq!(result.is_ok(), accepted);
        if !accepted {
            assert_eq!(result, Err(Invalid));
            assert!(!format!("{result:?}").contains("PUBLIC"));
        }
    }
    drop(listener);
    std::fs::remove_dir_all(root).unwrap();
}
