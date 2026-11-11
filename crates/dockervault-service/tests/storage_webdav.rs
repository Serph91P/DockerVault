use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::StatusCode,
    response::Response,
    routing::any,
};
use dockervault_service::{
    ownership::Owner,
    storage::{
        VerifiedSource,
        webdav::{DavConfig, WebDav},
    },
};
use std::{
    collections::VecDeque,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::Notify;
static NEXT: AtomicU64 = AtomicU64::new(0);
type ObservedRequest = (
    String,
    String,
    Vec<u8>,
    Option<String>,
    Option<String>,
    bool,
);
#[derive(Default)]
struct Service {
    objects: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    requests: Mutex<Vec<ObservedRequest>>,
    put_status: Mutex<VecDeque<u16>>,
    corrupt_get: bool,
    hold_put: bool,
    propfind_status: u16,
    listing: Option<String>,
    oversized_listing: bool,
    received: Notify,
    release: Notify,
}
async fn endpoint(State(state): State<Arc<Service>>, request: Request) -> Response {
    let method = request.method().as_str().to_owned();
    let path = request.uri().to_string();
    let auth = request
        .headers()
        .get("authorization")
        .map(|h| h.to_str().unwrap().to_owned());
    let length = request
        .headers()
        .get("content-length")
        .map(|h| h.to_str().unwrap().to_owned());
    let chunked = request.headers().contains_key("transfer-encoding");
    let bytes = to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    state.requests.lock().unwrap().push((
        method.clone(),
        path.clone(),
        bytes.clone(),
        auth,
        length,
        chunked,
    ));
    let mut builder = Response::builder();
    if method == "PROPFIND" && state.oversized_listing {
        let chunks = (0..19).map(|index| {
            Ok::<_, std::convert::Infallible>(match index {
                0 => b"<multistatus xmlns=\"DAV:\"><!--".to_vec(),
                18 => b"--></multistatus>".to_vec(),
                _ => vec![b'x'; 1024 * 1024],
            })
        });
        return builder
            .status(207)
            .body(Body::from_stream(futures_util::stream::iter(chunks)))
            .unwrap();
    }
    let mut body = Vec::new();
    let status = match method.as_str() {
        "MKCOL" => 201,
        "PUT" => {
            state.objects.lock().unwrap().insert(path.clone(), bytes);
            state.received.notify_one();
            if state.hold_put {
                state.release.notified().await;
            }
            state.put_status.lock().unwrap().pop_front().unwrap_or(201)
        }
        "HEAD" => {
            let size = state.objects.lock().unwrap().get(&path).map(|b| b.len());
            if let Some(size) = size {
                builder = builder.header("Content-Length", size.to_string());
                200
            } else {
                404
            }
        }
        "GET" => {
            if let Some(bytes) = state.objects.lock().unwrap().get(&path) {
                body = bytes.clone();
                if state.corrupt_get && !body.is_empty() {
                    body[0] ^= 1;
                }
                200
            } else {
                404
            }
        }
        "DELETE" => {
            if state.objects.lock().unwrap().remove(&path).is_some() {
                204
            } else {
                404
            }
        }
        "PROPFIND" => {
            body = state
                .listing
                .as_deref()
                .unwrap_or("<multistatus xmlns=\"DAV:\"/>")
                .as_bytes()
                .to_vec();
            if state.propfind_status == 0 {
                207
            } else {
                state.propfind_status
            }
        }
        _ => 405,
    };
    builder
        .status(StatusCode::from_u16(status).unwrap())
        .body(Body::from(body))
        .unwrap()
}
struct Server {
    url: String,
    state: Arc<Service>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn new(service: Service) -> Self {
        let state = Arc::new(service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .fallback(any(endpoint))
            .with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, state, task }
    }
    fn backend(&self, attempts: u32) -> (WebDav, Owner, Owner) {
        let owner = Owner::new(2).unwrap();
        let io = Owner::new(2).unwrap();
        let backend = WebDav::new(
            DavConfig {
                url: format!("{}/dav%2Froot?tenant=one%26two", self.url),
                base_path: "/backups #?%".into(),
                username: Some("public-test-user".into()),
                password: Some("public-test-password".into()),
                max_attempts: attempts,
                verify_head: true,
            },
            owner.clone(),
            io.clone(),
        )
        .unwrap();
        (backend, owner, io)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.release.notify_waiters();
        self.task.abort();
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-dav-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("source"), b"complete independent protocol bytes").unwrap();
        Self(path)
    }
    fn source(&self) -> Arc<VerifiedSource> {
        Arc::new(VerifiedSource::open(&self.0.join("source"), &|| false).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[tokio::test]
async fn actual_put_and_independent_get_preserve_encoded_endpoint_and_filename_identity() {
    let server = Server::new(Service::default()).await;
    let (backend, owner, io) = server.backend(1);
    let fixture = Fixture::new();
    backend
        .upload(fixture.source(), "project%2F1/first#?%.tar".into())
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    let requests = server.state.requests.lock().unwrap();
    let put = requests.iter().find(|r| r.0 == "PUT").unwrap();
    assert_eq!(
        put.1,
        "/dav%2Froot/backups%20%23%3F%25/project%252F1/first%23%3F%25.tar?tenant=one%26two"
    );
    assert_eq!(put.2, b"complete independent protocol bytes");
    assert_eq!(put.4.as_deref(), Some("35"));
    assert!(!put.5);
    assert!(put.3.as_ref().unwrap().starts_with("Basic "));
    assert!(requests.iter().any(|r| r.0 == "GET" && r.1 == put.1));
    assert!(requests.iter().any(|r| r.0 == "HEAD" && r.1 == put.1));
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
}
#[tokio::test]
async fn retried_put_reopens_identical_bytes_but_permanent_unauthorized_never_retries() {
    for (statuses, attempts, success) in [(vec![500, 201], 2, true), (vec![401, 201], 3, false)] {
        let server = Server::new(Service {
            put_status: Mutex::new(statuses.into()),
            ..Default::default()
        })
        .await;
        let (backend, _, io) = server.backend(attempts);
        let fixture = Fixture::new();
        let result = backend
            .upload(fixture.source(), "archive".into())
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(result.is_ok(), success);
        let requests = server.state.requests.lock().unwrap();
        let puts: Vec<_> = requests.iter().filter(|r| r.0 == "PUT").collect();
        assert_eq!(puts.len(), if success { 2 } else { 1 });
        for put in puts {
            assert_eq!(put.2, b"complete independent protocol bytes");
        }
        assert_eq!(io.active(), 0);
    }
}
#[tokio::test]
async fn successful_put_and_head_cannot_hide_wrong_independent_download_bytes() {
    let server = Server::new(Service {
        corrupt_get: true,
        ..Default::default()
    })
    .await;
    let (backend, owner, io) = server.backend(1);
    let fixture = Fixture::new();
    assert!(
        backend
            .upload(fixture.source(), "archive".into())
            .unwrap()
            .wait()
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
}
#[tokio::test]
async fn cancelled_large_put_closes_the_actual_tcp_socket_and_joins_a_blocked_file_producer() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener_socket = tokio::net::TcpSocket::new_v4().unwrap();
    listener_socket.set_recv_buffer_size(65536).unwrap();
    listener_socket
        .bind("127.0.0.1:0".parse().unwrap())
        .unwrap();
    let listener = listener_socket.listen(16).unwrap();
    let address = listener.local_addr().unwrap();
    let received = Arc::new(Notify::new());
    let observed = received.clone();
    let drain = Arc::new(Notify::new());
    let can_drain = drain.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            header.push(byte[0]);
            assert!(header.len() < 65536);
        }
        assert!(String::from_utf8(header).unwrap().starts_with("PUT "));
        observed.notify_one();
        // Do not consume source bytes until cancellation has been requested by the client.
        can_drain.notified().await;
        socket.flush().await.unwrap();
        let mut count = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            match socket.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => count += n as u64,
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                Err(error) => panic!("socket read: {error}"),
            }
        }
        count
    });
    let owner = Owner::new(2).unwrap();
    let io = Owner::new(2).unwrap();
    let backend = WebDav::new(
        DavConfig {
            url: format!("http://{address}"),
            base_path: String::new(),
            username: None,
            password: None,
            max_attempts: 1,
            verify_head: false,
        },
        owner.clone(),
        io.clone(),
    )
    .unwrap();
    let fixture = Fixture::new();
    fs::File::create(fixture.0.join("source"))
        .unwrap()
        .set_len(64 * 1024 * 1024)
        .unwrap();
    let work = backend.upload(fixture.source(), "archive".into()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), received.notified())
        .await
        .unwrap();
    assert_eq!(
        io.active(),
        1,
        "Source producer must still own its physical I/O slot"
    );
    work.cancel();
    drain.notify_one();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), work.wait())
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    assert!(
        received < 64 * 1024 * 1024,
        "Cancelled upload must not finish sending source bytes"
    );
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
}

#[tokio::test]
async fn independent_get_downloads_to_private_atomic_destination_and_preserves_old_bytes_on_truncation()
 {
    let server = Server::new(Service::default()).await;
    let (backend, owner, io) = server.backend(1);
    let fixture = Fixture::new();
    let source = fixture.source();
    let expected = source.sha256_hex();
    backend
        .upload(source, "archive".into())
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    let receipt = backend
        .download(
            "archive".into(),
            fs::File::open(&fixture.0).unwrap(),
            "download".into(),
        )
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.sha256_hex(), expected);
    assert_eq!(
        fs::read(fixture.0.join("download")).unwrap(),
        b"complete independent protocol bytes"
    );
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            stream.read_exact(&mut b).await.unwrap();
            request.push(b[0]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort")
            .await
            .unwrap();
    });
    let bad = WebDav::new(
        DavConfig {
            url: format!("http://{address}"),
            base_path: String::new(),
            username: None,
            password: None,
            max_attempts: 1,
            verify_head: false,
        },
        owner.clone(),
        io.clone(),
    )
    .unwrap();
    let result = bad
        .download(
            "archive".into(),
            fs::File::open(&fixture.0).unwrap(),
            "download".into(),
        )
        .unwrap()
        .wait()
        .await
        .unwrap();
    peer.await.unwrap();
    assert!(result.is_err());
    assert_eq!(
        fs::read(fixture.0.join("download")).unwrap(),
        b"complete independent protocol bytes"
    );
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 2);
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
}

#[tokio::test]
async fn cancelled_get_after_actual_file_writes_closes_socket_and_removes_only_private_partial() {
    use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            assert!(request.len() < 65536);
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 67108864\r\n\r\n")
            .await
            .unwrap();
        socket.write_all(&vec![23; 1024 * 1024]).await.unwrap();
        let mut byte = [0];
        match socket.read(&mut byte).await {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("Expected actual client close, got {other:?}"),
        }
    });
    let fixture = Fixture::new();
    fs::write(fixture.0.join("download"), b"existing recovery point").unwrap();
    let watch = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC).unwrap();
    watch
        .add_watch(&fixture.0, AddWatchFlags::IN_MODIFY)
        .unwrap();
    let owner = Owner::new(2).unwrap();
    let io = Owner::new(2).unwrap();
    let backend = WebDav::new(
        DavConfig {
            url: format!("http://{address}"),
            base_path: String::new(),
            username: None,
            password: None,
            max_attempts: 1,
            verify_head: false,
        },
        owner.clone(),
        io.clone(),
    )
    .unwrap();
    let work = backend
        .download(
            "archive".into(),
            fs::File::open(&fixture.0).unwrap(),
            "download".into(),
        )
        .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let written = match watch.read_events() {
            Ok(events) => events.iter().any(|event| {
                event.name.as_ref().is_some_and(|name| {
                    name.to_string_lossy().starts_with(".dockervault-transfer-")
                })
            }),
            Err(nix::errno::Errno::EAGAIN) => false,
            Err(error) => panic!("inotify {error}"),
        };
        if written {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "No actual private download write"
        );
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert_eq!(io.active(), 1);
    work.cancel();
    work.cancel();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), work.wait())
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), peer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    assert_eq!(
        fs::read(fixture.0.join("download")).unwrap(),
        b"existing recovery point"
    );
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 2);
}

#[tokio::test]
async fn propfind_lists_namespaced_metadata_quota_and_skips_collection_itself() {
    let listing = r#"<s:multistatus xmlns:s="DAV:" xmlns:x="urn:not-dav">
<s:response><s:href>/dav%2Froot/backups%20%23%3F%25/folder/</s:href><s:propstat><s:prop><s:resourcetype><s:collection/></s:resourcetype></s:prop></s:propstat></s:response>
<s:response><s:href>/dav%2Froot/backups%20%23%3F%25/folder/first%23%3F%25.tar</s:href><s:propstat><s:prop><s:getcontentlength>35</s:getcontentlength><s:getlastmodified>Wed, 21 Oct 2015 07:28:00 GMT</s:getlastmodified><s:resourcetype><x:collection/></s:resourcetype><s:quota-available-bytes>96</s:quota-available-bytes></s:prop></s:propstat></s:response>
<s:response><s:href>/dav%2Froot/backups%20%23%3F%25/folder/child/</s:href><s:propstat><s:prop><s:resourcetype><s:collection/></s:resourcetype></s:prop></s:propstat></s:response>
<s:response><s:href>/dav%2Froot/backups%20%23%3F%25/folder/amp&amp;name</s:href><s:propstat><s:prop/></s:propstat></s:response>
</s:multistatus>"#;
    let server = Server::new(Service {
        listing: Some(listing.into()),
        ..Default::default()
    })
    .await;
    let (backend, owner, io) = server.backend(1);
    let entries = backend
        .list("folder".into())
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].name, "first#?%.tar");
    assert_eq!(entries[0].size, 35);
    assert!(!entries[0].is_dir);
    assert_eq!(entries[0].modified, 1_445_412_480.0);
    assert_eq!(entries[1].name, "child");
    assert!(entries[1].is_dir);
    assert_eq!(entries[2].name, "amp&name");
    assert_eq!(
        backend
            .quota_available()
            .unwrap()
            .wait()
            .await
            .unwrap()
            .unwrap(),
        Some(96)
    );
    backend
        .test_connection()
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
}

#[tokio::test]
async fn invalid_or_failed_propfind_cannot_be_reported_as_an_empty_successful_collection() {
    let deep = format!(
        "<multistatus xmlns=\"DAV:\">{}{}</multistatus>",
        "<nested>".repeat(70),
        "</nested>".repeat(70)
    );
    for xml in ["<multistatus xmlns=\"not-dav\"/>".to_owned(),"<multistatus xmlns=\"DAV:\"><response>".to_owned(),"<!DOCTYPE x [<!ENTITY payload SYSTEM 'file:///etc/passwd'>]><multistatus xmlns=\"DAV:\"/>".to_owned(),"<multistatus xmlns=\"DAV:\"><response><href>&undefined;</href></response></multistatus>".to_owned(),"<multistatus xmlns=\"DAV:\"/><![CDATA[orphan]]>".to_owned(),"<multistatus xmlns=\"DAV:\"/><multistatus xmlns=\"DAV:\"/>".to_owned(),"<multistatus xmlns=\"DAV:\"><response><href>/denied</href><status>HTTP/1.1 403 Forbidden</status></response></multistatus>".to_owned(),deep] {
        let server=Server::new(Service{listing:Some(xml.clone()),..Default::default()}).await;let(backend,_,_)=server.backend(1);
        assert!(backend.list(String::new()).unwrap().wait().await.unwrap().is_err(),"invalid XML must fail: {xml}");
    }
    let server = Server::new(Service {
        propfind_status: 503,
        ..Default::default()
    })
    .await;
    let (backend, _, _) = server.backend(1);
    assert!(
        backend
            .list(String::new())
            .unwrap()
            .wait()
            .await
            .unwrap()
            .is_err()
    );
    let server = Server::new(Service::default()).await;
    let (backend, _, _) = server.backend(1);
    assert!(
        backend
            .list(String::new())
            .unwrap()
            .wait()
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn document_boundaries_preserve_valid_xml_declarations_but_reject_orphan_references() {
    for xml in [
        "<multistatus xmlns=\"DAV:\"/>&amp;",
        "<multistatus xmlns=\"DAV:\"/><?xml version=\"1.0\"?>",
        "<?xml version=\"1.0\"?><?xml version=\"1.0\"?><multistatus xmlns=\"DAV:\"/>",
    ] {
        let server = Server::new(Service {
            listing: Some(xml.into()),
            ..Default::default()
        })
        .await;
        let (backend, _, _) = server.backend(1);
        assert!(
            backend
                .list(String::new())
                .unwrap()
                .wait()
                .await
                .unwrap()
                .is_err(),
            "invalid document {xml}"
        );
    }
    for xml in [
        "\u{feff}<?xml version=\"1.0\"?><multistatus xmlns=\"DAV:\"/>",
        "<?xml version=\"1.0\"?>\n<!-- comment --><multistatus xmlns=\"DAV:\"/>",
    ] {
        let server = Server::new(Service {
            listing: Some(xml.into()),
            ..Default::default()
        })
        .await;
        let (backend, _, _) = server.backend(1);
        assert!(
            backend
                .list(String::new())
                .unwrap()
                .wait()
                .await
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn chunked_valid_xml_is_bounded_before_it_can_become_an_empty_collection() {
    let server = Server::new(Service {
        oversized_listing: true,
        ..Default::default()
    })
    .await;
    let (backend, owner, io) = server.backend(1);
    assert!(
        backend
            .list(String::new())
            .unwrap()
            .wait()
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
}

#[tokio::test]
async fn delete_and_connection_use_real_protocol_status_without_inventing_objects() {
    let server = Server::new(Service::default()).await;
    let (backend, owner, io) = server.backend(1);
    let fixture = Fixture::new();
    backend
        .upload(fixture.source(), "archive".into())
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    backend
        .test_connection()
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    for _ in 0..2 {
        backend
            .delete("archive".into())
            .unwrap()
            .wait()
            .await
            .unwrap()
            .unwrap();
    }
    assert!(server.state.objects.lock().unwrap().is_empty());
    let requests = server.state.requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.0 == "DELETE").count(), 2);
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
}

#[tokio::test]
async fn cancel_after_remote_received_all_bytes_settles_local_body_producer_without_false_success()
{
    let server = Server::new(Service {
        hold_put: true,
        ..Default::default()
    })
    .await;
    let (backend, owner, io) = server.backend(1);
    let fixture = Fixture::new();
    let work = backend.upload(fixture.source(), "archive".into()).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        server.state.received.notified(),
    )
    .await
    .unwrap();
    work.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), work.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
    assert_eq!(owner.active(), 0);
    assert_eq!(io.active(), 0);
    assert_eq!(
        server
            .state
            .objects
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap(),
        b"complete independent protocol bytes"
    );
    server.state.release.notify_one();
}
