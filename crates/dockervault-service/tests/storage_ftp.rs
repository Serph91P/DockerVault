//! The client talks plain FTP to owned control sockets; no production provider is touched.
use dockervault_service::{
    ownership::Owner,
    storage::ftp::{Ftp, FtpConfig, FtpError},
};
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};

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
