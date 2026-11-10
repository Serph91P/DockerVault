//! Independent native server processes race through the real HTTP setup route.
use dockervault_service::{
    auth::AuthService,
    config::Config,
    db::Database,
    http::{self, HttpState},
    ownership::Owner,
};
use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}
fn child(database: PathBuf, ready: PathBuf) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(2)
        .build()
        .unwrap()
        .block_on(async {
            let owner = Owner::new(1).unwrap();
            let db = Database::open(database, ready.with_extension("snapshots"), &owner)
                .await
                .unwrap();
            owner.shutdown(None).await;
            let auth = AuthService::new(db.pool().clone());
            let app = http::router(HttpState::new(
                auth,
                Config::from_pairs([("COOKIE_SECURE", "false")]).unwrap(),
            ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            fs::write(ready, listener.local_addr().unwrap().to_string()).unwrap();
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
}
fn spawn(database: &Path, ready: &Path) -> OwnedChild {
    OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .arg("--child")
            .arg(database)
            .arg(ready)
            .env_clear()
            .env("TMPDIR", ready.parent().unwrap())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}
fn address(child: &mut OwnedChild, ready: &Path) -> SocketAddr {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "owned native setup server exited before listening"
        );
        if let Ok(value) = fs::read_to_string(ready)
            && let Ok(address) = value.parse()
        {
            return address;
        }
        assert!(
            Instant::now() < deadline,
            "native setup server did not listen"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn setup(address: SocketAddr, username: &str) -> u16 {
    let body=serde_json::json!({"username":username,"password":"PUBLIC-process-password!","confirm_password":"PUBLIC-process-password!"}).to_string();
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream,"POST /api/v1/auth/setup HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    #[cfg(target_os = "linux")]
    if args.get(1).is_some_and(|argument| argument == "--claim") {
        use dockervault_service::recovery_activation::{ClaimKind, PreparationClaims};
        let claims = PreparationClaims::new(Some(PathBuf::from(&args[2])));
        let _guard = claims.try_claim(ClaimKind::Setup).unwrap();
        fs::write(&args[3], b"claimed").unwrap();
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
        return;
    }
    if args.get(1).is_some_and(|argument| argument == "--child") {
        child(PathBuf::from(&args[2]), PathBuf::from(&args[3]));
        return;
    }
    let fixture =
        Fixture(std::env::temp_dir().join(format!("dv-auth-process-{}", std::process::id())));
    fs::create_dir(&fixture.0).unwrap();
    let database = fixture.0.join("db.sqlite");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let owner = Owner::new(1).unwrap();
            let db = Database::open(&database, fixture.0.join("snapshots"), &owner)
                .await
                .unwrap();
            owner.shutdown(None).await;
            db.close().await;
        });
    let ready_a = fixture.0.join("a.ready");
    let ready_b = fixture.0.join("b.ready");
    let mut a = spawn(&database, &ready_a);
    let mut b = spawn(&database, &ready_b);
    let a_address = address(&mut a, &ready_a);
    let b_address = address(&mut b, &ready_b);
    let first = std::thread::spawn(move || setup(a_address, "public-admin-a"));
    let second = std::thread::spawn(move || setup(b_address, "public-admin-b"));
    let mut statuses = [first.join().unwrap(), second.join().unwrap()];
    statuses.sort();
    assert_eq!(
        statuses,
        [200, 400],
        "independent setup claims must produce one success and one already-complete rejection"
    );
    drop(a);
    drop(b);
    let connection = rusqlite::Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM users", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM sessions", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert!(
        connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none()
    );
    drop(connection);
    #[cfg(target_os = "linux")]
    {
        use dockervault_service::recovery_activation::{
            ClaimKind, PreparationClaims, PreparationError,
        };
        let ready = fixture.0.join("claim.ready");
        let mut holder = OwnedChild(
            Command::new(std::env::current_exe().unwrap())
                .arg("--claim")
                .arg(&database)
                .arg(&ready)
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(holder.0.try_wait().unwrap().is_none());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let claims = PreparationClaims::new(Some(database.clone()));
        assert!(matches!(
            claims.try_claim(ClaimKind::Setup),
            Err(PreparationError::Busy)
        ));
        holder.0.kill().unwrap();
        holder.0.wait().unwrap();
        let _guard = claims.try_claim(ClaimKind::Setup).unwrap();
        println!(
            "preparation-process: real child lock -> Busy -> SIGKILL/reap -> same marker reacquired: PASS"
        );
    }
    println!(
        "auth-process: two real HTTP servers -> one administrator/session -> physical process settlement/integrity: PASS"
    );
}
