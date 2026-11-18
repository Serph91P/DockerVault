//! The client talks plain FTP to owned control sockets; no production provider is touched.
use dockervault_service::{
    ownership::Owner,
    storage::ftp::{Ftp, FtpConfig, FtpError},
};
use sha2::Digest;
use std::time::Duration;
use std::{
    fs::{self, File},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-ftp-data-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

async fn ready_for_retr(control: &mut BufReader<TcpStream>, use_epsv: bool) -> TcpStream {
    control.write_all(b"220 data fixture\r\n").await.unwrap();
    assert_eq!(read_command(control).await, "USER owned\r\n");
    control
        .write_all(b"230 logged in without PASS\r\n")
        .await
        .unwrap();
    assert_eq!(read_command(control).await, "TYPE I\r\n");
    control.write_all(b"200 binary\r\n").await.unwrap();
    assert_eq!(read_command(control).await, "EPSV\r\n");
    let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = data.local_addr().unwrap().port();
    if use_epsv {
        control
            .write_all(format!("229 entering (|||{port}|)\r\n").as_bytes())
            .await
            .unwrap();
    } else {
        control.write_all(b"502 unavailable\r\n").await.unwrap();
        assert_eq!(read_command(control).await, "PASV\r\n");
        // This address is syntactically valid but not the authorized control peer.
        control
            .write_all(
                format!("227 entering (192,0,2,6,{},{})\r\n", port / 256, port % 256).as_bytes(),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        read_command(control).await,
        "RETR /backups/nested/archive.tar\r\n"
    );
    control
        .write_all(b"150 open data connection\r\n")
        .await
        .unwrap();
    data.accept().await.unwrap().0
}

#[tokio::test]
async fn ftp_download_requires_the_terminal_reply_before_replacing_an_existing_file() {
    let payload: Vec<u8> = (0..200_003).map(|i| (i % 251) as u8).collect();
    let digest: String = sha2::Sha256::digest(&payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    for success in [true, false] {
        let dir = Directory::new();
        let path = dir.0.join("archive.tar");
        fs::write(&path, b"existing recovery point").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let owner = Owner::new(1).unwrap();
        let io = Owner::new(1).unwrap();
        let ftp = backend(&listener, &owner, "owned", "synthetic");
        let (body_sent, body_seen) = tokio::sync::oneshot::channel();
        let (release, reply_allowed) = tokio::sync::oneshot::channel();
        let data_bytes = payload.clone();
        let server = tokio::spawn(async move {
            let mut control = BufReader::new(listener.accept().await.unwrap().0);
            let mut data = ready_for_retr(&mut control, success).await;
            data.write_all(&data_bytes).await.unwrap();
            drop(data);
            body_sent.send(()).unwrap();
            reply_allowed.await.unwrap();
            if success {
                control
                    .write_all(b"226 transfer complete\r\n")
                    .await
                    .unwrap();
                assert_eq!(read_command(&mut control).await, "QUIT\r\n");
                control.write_all(b"221 goodbye\r\n").await.unwrap();
            } else {
                control
                    .write_all(b"426 transfer interrupted\r\n")
                    .await
                    .unwrap();
            }
            assert!(read_command(&mut control).await.is_empty());
        });
        let work = ftp
            .download(
                "nested/archive.tar".into(),
                File::open(&dir.0).unwrap(),
                "archive.tar".into(),
                &io,
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), body_seen)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"existing recovery point");
        assert_eq!(owner.active(), 1);
        release.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), work.wait())
            .await
            .unwrap()
            .unwrap();
        if success {
            let receipt = result.unwrap();
            assert_eq!(receipt.len(), payload.len() as u64);
            assert_eq!(receipt.sha256_hex(), digest);
            assert_eq!(fs::read(&path).unwrap(), payload);
        } else {
            assert!(matches!(result, Err(FtpError::Reply(426))));
            assert_eq!(fs::read(&path).unwrap(), b"existing recovery point");
        }
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        assert_eq!(owner.active(), 0);
        assert_eq!(io.active(), 0);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn ftp_download_cancel_joins_writer_and_preserves_abort_obligation_until_ack() {
    use tokio::io::AsyncReadExt;
    for acknowledged in [true, false] {
        let dir = Directory::new();
        let path = dir.0.join("archive.tar");
        fs::write(&path, b"existing recovery point").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let owner = Owner::new(1).unwrap();
        let io = Owner::new(1).unwrap();
        let ftp = backend(&listener, &owner, "owned", "synthetic");
        let (started, running) = tokio::sync::oneshot::channel();
        let (abort_seen, abort_received) = tokio::sync::oneshot::channel();
        let (release_abort, abort_allowed) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut control = BufReader::new(listener.accept().await.unwrap().0);
            let mut data = ready_for_retr(&mut control, true).await;
            data.write_all(&vec![0x71; 128 * 1024]).await.unwrap();
            started.send(()).unwrap();
            assert_eq!(read_command(&mut control).await, "ABOR\r\n");
            match data.read(&mut [0; 1]).await {
                Ok(0) => (),
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => (),
                other => panic!("FTP data socket was not closed: {other:?}"),
            }
            if acknowledged {
                control
                    .write_all(b"426 transfer interrupted\r\n")
                    .await
                    .unwrap();
            }
            abort_seen.send(()).unwrap();
            abort_allowed.await.unwrap();
            if acknowledged {
                control.write_all(b"226 abort complete\r\n").await.unwrap();
                assert_eq!(read_command(&mut control).await, "QUIT\r\n");
                control.write_all(b"221 goodbye\r\n").await.unwrap();
            } else {
                control.write_all(b"550 abort rejected\r\n").await.unwrap();
            }
            assert!(read_command(&mut control).await.is_empty());
        });
        let work = ftp
            .download(
                "nested/archive.tar".into(),
                File::open(&dir.0).unwrap(),
                "archive.tar".into(),
                &io,
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if fs::read_dir(&dir.0).unwrap().any(|entry| {
                    let entry = entry.unwrap();
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".dockervault-transfer-")
                        && entry.metadata().unwrap().len() != 0
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(owner.active(), 1);
        assert_eq!(io.active(), 1);
        work.cancel();
        tokio::time::timeout(Duration::from_secs(2), abort_received)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owner.active(), 1);
        assert_eq!(io.active(), 0);
        assert!(ftp.test_connection().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"existing recovery point");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        release_abort.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), work.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.unwrap_err(),
            if acknowledged {
                FtpError::Cancelled
            } else {
                FtpError::AbortUnsettled
            }
        );
        assert_eq!(owner.active(), 0);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn ftp_upload_requires_independent_full_readback_before_reporting_success() {
    use dockervault_service::storage::VerifiedSource;
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;
    for corrupt in [false, true] {
        let dir = Directory::new();
        let bytes: Vec<_> = (0..200_003).map(|i| (i % 251) as u8).collect();
        let source = dir.0.join("source.tar");
        fs::write(&source, &bytes).unwrap();
        let source = Arc::new(VerifiedSource::open(&source, &|| false).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let owner = Owner::new(1).unwrap();
        let io = Owner::new(1).unwrap();
        let ftp = backend(&listener, &owner, "owned", "synthetic");
        let (readback_seen, readback_requested) = tokio::sync::oneshot::channel();
        let (release, readback_allowed) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut control = BufReader::new(listener.accept().await.unwrap().0);
            control.write_all(b"220 data fixture\r\n").await.unwrap();
            assert_eq!(read_command(&mut control).await, "USER owned\r\n");
            control.write_all(b"230 logged in\r\n").await.unwrap();
            for directory in ["/backups", "/backups/nested"] {
                assert_eq!(
                    read_command(&mut control).await,
                    format!("MKD {directory}\r\n")
                );
                control.write_all(b"257 created\r\n").await.unwrap();
            }
            let mut stored = Vec::new();
            for command in ["STOR", "RETR"] {
                assert_eq!(read_command(&mut control).await, "TYPE I\r\n");
                control.write_all(b"200 binary\r\n").await.unwrap();
                assert_eq!(read_command(&mut control).await, "EPSV\r\n");
                let data_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = data_listener.local_addr().unwrap().port();
                control
                    .write_all(format!("229 entering (|||{port}|)\r\n").as_bytes())
                    .await
                    .unwrap();
                assert_eq!(
                    read_command(&mut control).await,
                    format!("{command} /backups/nested/archive.tar\r\n")
                );
                control
                    .write_all(b"150 open data connection\r\n")
                    .await
                    .unwrap();
                let (mut data, _) = data_listener.accept().await.unwrap();
                if command == "STOR" {
                    data.read_to_end(&mut stored).await.unwrap();
                    assert_eq!(stored, bytes);
                } else {
                    readback_seen.send(()).unwrap();
                    readback_allowed.await.unwrap();
                    if corrupt {
                        stored[199_999] ^= 1;
                    }
                    data.write_all(&stored).await.unwrap();
                    drop(data);
                    control
                        .write_all(b"226 transfer complete\r\n")
                        .await
                        .unwrap();
                    break;
                }
                drop(data);
                control
                    .write_all(b"226 transfer complete\r\n")
                    .await
                    .unwrap();
            }
            assert_eq!(read_command(&mut control).await, "QUIT\r\n");
            control.write_all(b"221 goodbye\r\n").await.unwrap();
            assert!(read_command(&mut control).await.is_empty());
        });
        let work = ftp
            .upload(source, "nested/archive.tar".into(), &io)
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), readback_requested)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owner.active(), 1);
        assert!(ftp.test_connection().is_err());
        release.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), work.wait())
            .await
            .unwrap()
            .unwrap();
        if corrupt {
            assert_eq!(result, Err(FtpError::Verification));
        } else {
            assert_eq!(result, Ok(()));
        }
        assert_eq!(owner.active(), 0);
        assert_eq!(io.active(), 0);
        server.await.unwrap();
    }
}

fn backend(listener: &TcpListener, owner: &Owner, user: &str, password: &str) -> Ftp {
    let address = listener.local_addr().unwrap();
    Ftp::new(
        FtpConfig {
            host: address.ip().to_string(),
            port: Some(address.port()),
            username: user.into(),
            password: password.into(),
            base_path: "/backups".into(),
        },
        owner.clone(),
    )
    .unwrap()
}

async fn read_command(socket: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    line
}

#[tokio::test]
async fn plain_ftp_handshakes_use_matching_multiline_replies_and_quit_each_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let explicit = backend(&listener, &owner, "owned", "synthetic");
    let anonymous = backend(&listener, &owner, "", "unused");
    let server = tokio::spawn(async move {
        for (user, pass) in [("owned", "synthetic"), ("anonymous", "anon@")] {
            let mut client = BufReader::new(listener.accept().await.unwrap().0);
            client
                .write_all(b"220-Owned test service\r\n331 not a terminator\r\n220 ready\r\n")
                .await
                .unwrap();
            assert_eq!(read_command(&mut client).await, format!("USER {user}\r\n"));
            client
                .write_all(b"331 password required\r\n")
                .await
                .unwrap();
            assert_eq!(read_command(&mut client).await, format!("PASS {pass}\r\n"));
            client.write_all(b"230 logged in\r\n").await.unwrap();
            assert_eq!(read_command(&mut client).await, "QUIT\r\n");
            client.write_all(b"221 goodbye\r\n").await.unwrap();
            assert!(read_command(&mut client).await.is_empty());
        }
    });
    for ftp in [explicit, anonymous] {
        tokio::time::timeout(
            Duration::from_secs(2),
            ftp.test_connection().unwrap().wait(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(owner.active(), 0);
    }
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn ftp_bad_login_closes_control_connection_and_reports_only_provider_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let ftp = backend(&listener, &owner, "owned", "synthetic");
    let server = tokio::spawn(async move {
        let mut client = BufReader::new(listener.accept().await.unwrap().0);
        client.write_all(b"220 ready\r\n").await.unwrap();
        assert_eq!(read_command(&mut client).await, "USER owned\r\n");
        client
            .write_all(b"331 password required\r\n")
            .await
            .unwrap();
        assert_eq!(read_command(&mut client).await, "PASS synthetic\r\n");
        client
            .write_all(b"530 provider echo synthetic\r\n")
            .await
            .unwrap();
        assert!(read_command(&mut client).await.is_empty());
    });
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        ftp.test_connection().unwrap().wait(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap_err();
    assert_eq!(error, FtpError::Reply(530));
    assert_eq!(error.to_string(), "FTP server returned status 530");
    assert!(!format!("{error:?}").contains("synthetic"));
    assert_eq!(owner.active(), 0);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn ftp_greeting_timeout_and_login_cancel_close_real_sockets() {
    for cancel_login in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let owner = Owner::new(1).unwrap();
        let ftp = backend(&listener, &owner, "owned", "synthetic");
        let (entered, reply_held) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut client = BufReader::new(listener.accept().await.unwrap().0);
            if cancel_login {
                client.write_all(b"220 ready\r\n").await.unwrap();
                assert_eq!(read_command(&mut client).await, "USER owned\r\n");
            }
            entered.send(()).unwrap();
            assert!(read_command(&mut client).await.is_empty());
        });
        let work = ftp.test_connection().unwrap();
        tokio::time::timeout(Duration::from_secs(2), reply_held)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owner.active(), 1);
        let expected = if cancel_login {
            work.cancel();
            FtpError::Cancelled
        } else {
            // Only the clock is virtual. A real TCP peer has already accepted the connection.
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(61)).await;
            FtpError::Timeout
        };
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), work.wait())
                .await
                .unwrap()
                .unwrap(),
            Err(expected)
        );
        assert_eq!(owner.active(), 0);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
        if !cancel_login {
            tokio::time::resume();
        }
    }
}

#[tokio::test]
async fn ftp_rejects_unterminated_oversized_reply_and_unsafe_command_arguments() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = Owner::new(1).unwrap();
    let address = listener.local_addr().unwrap();
    let bad = Ftp::new(
        FtpConfig {
            host: address.ip().to_string(),
            port: Some(address.port()),
            username: "not\nvalid".into(),
            password: "synthetic".into(),
            base_path: "/backups".into(),
        },
        owner.clone(),
    );
    assert!(matches!(bad, Err(FtpError::Config)));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
    let ftp = backend(&listener, &owner, "owned", "synthetic");
    let (release, held) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut client = listener.accept().await.unwrap().0;
        client.write_all(b"220-").await.unwrap();
        let _ = client.write_all(&vec![b'a'; 128 * 1024]).await;
        held.await.unwrap();
    });
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        ftp.test_connection().unwrap().wait(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap_err();
    assert_eq!(error, FtpError::Protocol);
    assert_eq!(owner.active(), 0);
    release.send(()).unwrap();
    server.await.unwrap();
}
