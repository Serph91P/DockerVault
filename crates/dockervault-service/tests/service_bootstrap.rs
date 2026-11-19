use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use std::{
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn run(root: &Path, port: u16) -> Child {
    Command::new(env!("CARGO_BIN_EXE_dockervault-service"))
        .current_dir(root)
        .env_clear()
        .env("DATABASE_URL", "sqlite+aiosqlite:///./state.sqlite")
        .env("BACKUP_BASE_PATH", "./backups")
        .env("SHUTDOWN_TIMEOUT", "0")
        .env("DOCKER_SOCKET", "./absent-docker.sock")
        .arg(format!("127.0.0.1:{port}"))
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn standalone_service_migrates_serves_and_shuts_down_without_python() {
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).unwrap();
    let dir = std::env::temp_dir().join(format!("dv-bootstrap-{nonce:02x?}"));
    std::fs::create_dir(&dir).unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let mut process = run(&dir, port);
    let ready = || -> std::io::Result<bool> {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        let mut bytes = vec![];
        stream.read_to_end(&mut bytes)?;
        Ok(bytes.starts_with(b"HTTP/1.1 200 "))
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if ready().unwrap_or(false) {
            break;
        }
        if process.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let _ = process.kill();
            let _ = process.wait();
            panic!("native process never served health");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let other_port = other.local_addr().unwrap().port();
    drop(other);
    let mut duplicate = run(&dir, other_port);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = duplicate.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        if Instant::now() >= deadline {
            let _ = duplicate.kill();
            let _ = duplicate.wait();
            let _ = process.kill();
            let _ = process.wait();
            panic!("second native writer was not refused");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    std::os::unix::fs::symlink(dir.join("state.sqlite"), dir.join("alias.sqlite")).unwrap();
    let alias_status = Command::new(env!("CARGO_BIN_EXE_dockervault-service"))
        .current_dir(&dir)
        .env_clear()
        .env("DATABASE_URL", "sqlite+aiosqlite:///./alias.sqlite")
        .env("DOCKER_SOCKET", "./absent-docker.sock")
        .arg(format!("127.0.0.1:{other_port}"))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut alias_status = alias_status;
    let deadline = Instant::now() + Duration::from_secs(5);
    while alias_status.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = alias_status.kill();
            let _ = alias_status.wait();
            let _ = process.kill();
            let _ = process.wait();
            panic!("aliased database opened a second writer");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let alias_status = alias_status.wait_with_output().unwrap();
    assert!(!alias_status.status.success());
    assert!(
        String::from_utf8_lossy(&alias_status.stderr).contains("unsafe database file identity")
    );
    let db = rusqlite::Connection::open(dir.join("state.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM schema_migrations", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
    db.execute_batch("INSERT INTO users(id,username,password_hash) VALUES(1,'fixture','synthetic');
      INSERT INTO backup_targets(id,name,target_type) VALUES(1,'fixture','path');
      INSERT INTO backups(id,target_id,status,file_path) VALUES (1,1,'PENDING',NULL),(2,1,'RUNNING',NULL),(3,1,'COMPLETED','completed.tar');
      INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at) VALUES
      ('interrupted',1,'fixture-request','{}','{}','storage_delete','local','','running','deleting',2,0,0,0,0,0,0,'2024-01-01','2024-01-01');
      INSERT INTO storage_operation_items(operation_id,path,state) VALUES ('interrupted','possibly removed','running'),('interrupted','not started','pending');").unwrap();
    let neighbor = dir.join("not started");
    std::fs::write(&neighbor, b"fixture must not be replayed").unwrap();
    process.kill().unwrap();
    assert!(!process.wait().unwrap().success());
    let mut process = run(&dir, port);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready().unwrap_or(false) {
        if process.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let _ = process.kill();
            let _ = process.wait();
            panic!("restarted native process never served health");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let row = db
        .query_row(
            "SELECT state,stage,unknown,skipped FROM storage_operations WHERE id='interrupted'",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row, ("failed".into(), "interrupted".into(), 1, 1));
    assert_eq!(
        std::fs::read(neighbor).unwrap(),
        b"fixture must not be replayed"
    );
    // Hold an incomplete login body after the server starts polling it.
    use std::io::{Read, Write};
    let mut incomplete = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    incomplete
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    incomplete.write_all(b"POST /api/v1/auth/login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\nExpect: 100-continue\r\n\r\n").unwrap();
    let mut provisional = [0; 25];
    incomplete.read_exact(&mut provisional).unwrap();
    assert_eq!(&provisional, b"HTTP/1.1 100 Continue\r\n\r\n");
    kill(Pid::from_raw(process.id() as i32), Signal::SIGTERM).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = process.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            let _ = process.kill();
            let _ = process.wait();
            panic!("native process did not settle");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    type BackupState = (String, Option<String>, Option<String>, Option<String>);
    let states: Vec<BackupState> = db
        .prepare("SELECT status,completed_at,error_message,file_path FROM backups ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    db.execute_batch(
        "INSERT INTO app_settings(key,value) VALUES('recovery_activation','{}');
        INSERT INTO backups(id,target_id,status) VALUES(4,1,'PENDING');",
    )
    .unwrap();
    let mut process = run(&dir, port);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready().unwrap_or(false) {
        if process.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let _ = process.kill();
            let _ = process.wait();
            panic!("native process never served health with a pending recovery marker");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let pending: String = db
        .query_row("SELECT status FROM backups WHERE id=4", [], |row| {
            row.get(0)
        })
        .unwrap();
    process.kill().unwrap();
    process.wait().unwrap();
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        pending, "PENDING",
        "startup must not change backups during staged recovery activation"
    );
    for row in &states[..2] {
        assert_eq!(row.0, "FAILED");
        assert!(row.1.is_some());
        assert_eq!(
            row.2.as_deref(),
            Some(
                "Interrupted by restart; accepted job terminally failed. Legacy workload state may be unknown: verify workloads manually and retry."
            )
        );
        assert_eq!(row.3, None);
    }
    assert_eq!(
        states[2],
        ("COMPLETED".into(), None, None, Some("completed.tar".into()))
    );
}
