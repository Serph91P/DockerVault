use dockervault_service::{
    auth::AuthService, config::Config, db::Database, http, ownership::Owner,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Server {
    path: PathBuf,
    address: std::net::SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
    auth: AuthService,
    db: Database,
    notifications: std::sync::Arc<http::ws::Notifications>,
}
impl Server {
    async fn start(cors: &str) -> Self {
        Self::configured(
            Config::from_pairs([("COOKIE_SECURE", "false"), ("CORS_ORIGINS", cors)]).unwrap(),
        )
        .await
    }
    async fn configured(mut config: Config) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-http-auth-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let backups = path.join("backups");
        std::fs::create_dir(&backups).unwrap();
        config.backup_base_path = backups.to_str().unwrap().into();
        let owner = Owner::new(1).unwrap();
        let db = Database::open(path.join("db.sqlite"), path.join("snapshots"), &owner)
            .await
            .unwrap();
        owner.shutdown(None).await;
        let auth = AuthService::new(db.pool().clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, receiver) = oneshot::channel();
        let state = http::HttpState::new(auth.clone(), config);
        let notifications = state.notifications.clone();
        let app = http::router(state);
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = receiver.await;
            })
            .await
            .unwrap();
        });
        Self {
            path,
            address,
            stop,
            task,
            auth,
            db,
            notifications,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (u16, String, Value) {
        let body = body.map(|value| value.to_string()).unwrap_or_default();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.address,
            body.len()
        );
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        {
            request.push_str("Content-Type: application/json\r\n");
        }
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(&body);
        let mut stream = TcpStream::connect(self.address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (
            status,
            headers.to_owned(),
            if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(body).unwrap_or_else(|_| Value::String(body.to_owned()))
            },
        )
    }
    async fn setup(&self) -> String {
        let (status,headers,body)=self.request("POST","/api/v1/auth/setup",&[],Some(json!({"username":"public-admin","password":"PUBLIC-password!","confirm_password":"PUBLIC-password!"}))).await;
        assert_eq!(status, 200);
        assert_eq!(body["message"], "Setup completed successfully");
        headers
            .lines()
            .find_map(|line| line.strip_prefix("set-cookie: "))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }
    async fn close(self) {
        let _ = self.stop.send(());
        self.task.await.unwrap();
        self.auth.shutdown(None).await;
        self.db.close().await;
        std::fs::remove_dir_all(self.path).unwrap();
    }
}

#[tokio::test]
async fn backup_history_accepts_legacy_datetime_filters_and_returns_python_errors() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    for date in ["2026-01-02T10:00", "2026-01-02T11:00:00%2B0100"] {
        let path = format!("/api/v1/backups/history?after={date}");
        assert_eq!(
            server.request("GET", &path, &headers, None).await.0,
            200,
            "{date}"
        );
    }
    let (status, _, error) = server
        .request(
            "GET",
            "/api/v1/backups/history?after=not-a-date",
            &headers,
            None,
        )
        .await;
    assert_eq!(status, 422);
    assert_eq!(
        error["errors"],
        json!([{"field":"query.after","message":"Input should be a valid datetime or date, invalid character in year"}])
    );
    server.close().await;
}

#[tokio::test]
async fn backup_catalog_reads_records_and_validates_filters() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    for (query, status, expected) in [
        ("", 200, json!([])),
        (
            "?limit=0",
            422,
            json!({"detail":"Validation error","errors":[{"field":"query.limit","message":"Input should be greater than or equal to 1"}]}),
        ),
    ] {
        let (actual, _, response) = server
            .request("GET", &format!("/api/v1/backups{query}"), &headers, None)
            .await;
        assert_eq!((actual, response), (status, expected));
    }
    assert_eq!(
        server
            .request("GET", "/api/v1/backups/1/restore-info", &headers, None)
            .await
            .2,
        json!({"detail":"Backup not found"})
    );
    let (_, _, empty) = server
        .request("GET", "/api/v1/backups/history", &headers, None)
        .await;
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["snapshot"], 0);
    assert_eq!(empty["items"], json!([]));
    assert_eq!(
        server.request("GET", "/api/v1/backups", &[], None).await.0,
        401
    );

    let archive = server.path.join("backups").join("fixture_.tar");
    std::fs::write(&archive, b"fixture").unwrap();
    let recorded = server.path.join("backups").join("fixture::__.tar");
    sqlx::raw_sql("INSERT INTO backup_targets(id,name,target_type,host_path,dependencies) VALUES
            (1,'Alpha % plan','path','/fixture','[\"fixture-dependency\"]'),(2,'Other plan','volume',NULL,NULL);
        INSERT INTO remote_storages(id,name,storage_type,enabled) VALUES (1,'Store','LOCAL',1);
        INSERT INTO backups(id,target_id,backup_type,status,created_at) VALUES
            (2,1,'FULL','FAILED','2026-01-02 10:00:00.000001'),
            (3,2,'INCREMENTAL','PENDING','2026-01-03 10:00:00.000000');
        INSERT INTO backup_logs(backup_id,step,message,created_at) VALUES(2,'failed','fixture','2026-01-02 10:01:00.000000');")
        .execute(server.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path,file_size,encrypted,created_at) VALUES
        (1,1,'FULL','COMPLETED',?,1024,1,'2026-01-01 10:00:00.000000')")
        .bind(recorded.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO backup_storage_syncs(backup_id,storage_id,remote_path,sync_status) VALUES(1,1,'remote/fixture.tar','completed')")
        .execute(server.db.pool()).await.unwrap();
    let (_, _, body) = server
        .request(
            "GET",
            "/api/v1/backups?target_id=1&status=completed&limit=1",
            &headers,
            None,
        )
        .await;
    assert_eq!(body[0]["id"], 1);
    assert_eq!(body[0]["local_available"], true);
    assert_eq!(body[0]["remote_synced"], true);
    assert_eq!(body[0]["file_size_human"], "1.00 KB");
    assert_eq!(body[0]["created_at"], "2026-01-01T10:00:00Z");
    let (_, _, body) = server
        .request("GET", "/api/v1/backups?limit=1&offset=1", &headers, None)
        .await;
    assert_eq!(body[0]["id"], 2);
    assert_eq!(body[0]["created_at"], "2026-01-02T10:00:00.000001Z");
    let (_, _, history) = server
        .request(
            "GET",
            "/api/v1/backups/history?q=%25&source=path&limit=1&sort=created&direction=asc",
            &headers,
            None,
        )
        .await;
    assert_eq!(history["total"], 2);
    assert_eq!(history["snapshot"], 3);
    assert_eq!(history["items"][0]["can_restore"], true);
    assert_eq!(
        history["items"][0]["remote_sync_details"][0]["availability"],
        "unknown"
    );
    let (_, _, history) = server
        .request(
            "GET",
            "/api/v1/backups/history?snapshot=2&limit=1",
            &headers,
            None,
        )
        .await;
    assert_eq!(history["total"], 2);
    assert_eq!(history["items"][0]["stage"], "failed");
    assert_eq!(history["items"][0]["archive_state"], "never_created");
    let (status, _, info) = server
        .request("GET", "/api/v1/backups/1/restore-info", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(info["backup"]["local_available"], true);
    assert_eq!(info["volume_discovery"], "unavailable");
    assert_eq!(info["containers_to_stop"], json!(["fixture-dependency"]));
    assert_eq!(
        info["remote_copies"][0]["remote_path"],
        "remote/fixture.tar"
    );
    assert_eq!(
        server
            .request("GET", "/api/v1/backups/2/restore-info", &headers, None)
            .await
            .0,
        400
    );
    server.close().await;
}

#[tokio::test]
async fn local_delete_submit_commits_before_effect_and_replay_does_not_delete_again() {
    const ROUTE: &str = "/api/v1/operations";
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let root = server.path.join("backups");
    std::fs::write(root.join("z.tar"), b"selected").unwrap();
    std::fs::write(root.join("a.tar"), b"also selected").unwrap();
    std::fs::write(root.join("keep.tar"), b"untouched").unwrap();
    let payload = json!({
        "action":"storage_delete", "storage_id":"local", "path":"",
        "paths":["z.tar", "a.tar", "z.tar", "missing.tar"],
        "request_id":"c8f37f38-8195-4d6e-9b23-166930276666"
    });
    let valid = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let foreign = [
        ("Cookie", cookie.as_str()),
        ("Origin", "https://unrelated.invalid"),
    ];
    let post = |headers, body| server.request("POST", ROUTE, headers, Some(body));
    assert_eq!(post(&[], payload.clone()).await.0, 401);
    assert_eq!(post(&foreign, payload.clone()).await.0, 403);
    let mut invalid = payload.clone();
    invalid["storage_id"] = json!(true);
    assert_eq!(post(&valid, invalid).await.0, 422);
    let mut remote = payload.clone();
    remote["storage_id"] = json!(41);
    let mut oversized_remote = remote.clone();
    oversized_remote["paths"] = json!((0..1001).map(|i| format!("item-{i}")).collect::<Vec<_>>());
    let oversized = post(&valid, oversized_remote).await;
    assert_eq!(
        (oversized.0, oversized.2),
        (
            422,
            json!({"detail":"Select between 1 and 1000 nonempty paths"})
        )
    );
    let missing = post(&valid, remote.clone()).await;
    assert_eq!(
        (missing.0, missing.2),
        (404, json!({"detail":"Storage not found"}))
    );
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled) VALUES (41,'disabled fixture','ssh',0)")
        .execute(server.db.pool()).await.unwrap();
    let mut blank_remote = remote.clone();
    blank_remote["paths"] = json!([" "]);
    let blank = post(&valid, blank_remote).await;
    assert_eq!(
        (blank.0, blank.2),
        (
            422,
            json!({"detail":"Select between 1 and 1000 nonempty paths"})
        )
    );
    let disabled = post(&valid, remote).await;
    assert_eq!(
        (disabled.0, disabled.2),
        (409, json!({"detail":"Storage is disabled"}))
    );
    let mut over_limit = payload.clone();
    over_limit["paths"] = json!((0..1001).map(|i| format!("item-{i}")).collect::<Vec<_>>());
    assert_eq!(post(&valid, over_limit).await.0, 422);
    let mut too_many_duplicates = payload.clone();
    too_many_duplicates["paths"] = json!(vec!["z.tar"; 10001]);
    let response = post(&valid, too_many_duplicates).await;
    assert_eq!(response.0, 422);
    assert_eq!(response.2["errors"][0]["field"], "body.paths");
    assert_eq!(std::fs::read(root.join("z.tar")).unwrap(), b"selected");

    // A rejected transaction must leave no operation/items and do no deletion.
    let pool = server.db.pool();
    sqlx::query("CREATE TRIGGER reject_admission BEFORE INSERT ON storage_operation_items BEGIN SELECT RAISE(ABORT, 'fixture rejects item commit'); END")
        .execute(pool).await.unwrap();
    assert_eq!(post(&valid, payload.clone()).await.0, 500);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM storage_operations")
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(std::fs::read(root.join("a.tar")).unwrap(), b"also selected");
    sqlx::query("DROP TRIGGER reject_admission")
        .execute(pool)
        .await
        .unwrap();

    let (first, second) =
        tokio::join!(post(&valid, payload.clone()), post(&valid, payload.clone()));
    assert_eq!(first.0, 202);
    assert_eq!(second.0, 202);
    assert_eq!(first.2, second.2);
    let id = first.2["operation_id"].as_str().unwrap();
    let detail = format!("{ROUTE}/{id}");
    let stored: (i64, i64) =
        sqlx::query_as("SELECT user_id,total FROM storage_operations WHERE id=?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(stored, (1, 3));
    let settled = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, snapshot) = server.request("GET", &detail, &valid, None).await;
            assert_eq!(status, 200);
            if snapshot["finished_at"].is_string() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(settled["state"], "failed");
    assert_eq!(settled["stage"], "finished");
    for (key, value) in [
        ("total", 3),
        ("completed", 3),
        ("succeeded", 2),
        ("failed", 1),
        ("skipped", 0),
        ("unknown", 0),
    ] {
        assert_eq!(settled[key], value, "{key}");
    }
    assert_eq!(
        settled["items"],
        json!([
            {"path":"z.tar","state":"succeeded","error":null},
            {"path":"a.tar","state":"succeeded","error":null},
            {"path":"missing.tar","state":"failed","error":"Target not found"}
        ])
    );
    assert!(!root.join("z.tar").exists());
    assert!(!root.join("a.tar").exists());
    assert_eq!(std::fs::read(root.join("keep.tar")).unwrap(), b"untouched");
    std::fs::write(root.join("z.tar"), b"recreated").unwrap();
    assert_eq!(post(&valid, payload.clone()).await.2, first.2);
    let mut conflict = payload.clone();
    conflict["paths"] = json!(["z.tar"]);
    assert_eq!(post(&valid, conflict).await.0, 409);
    assert_eq!(std::fs::read(root.join("z.tar")).unwrap(), b"recreated");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM storage_operations")
            .fetch_one(pool)
            .await
            .unwrap(),
        1
    );
    sqlx::query("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<32)
        INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at)
        SELECT 'queued-'||i,1,'synthetic-'||i,'{}','{}','storage_delete','local','','queued','queued',1,0,0,0,0,0,0,'2024-01-01','2024-01-01' FROM n")
        .execute(pool).await.unwrap();
    let mut another = payload.clone();
    another["request_id"] = json!("e8f37f38-8195-4d6e-9b23-166930276666");
    assert_eq!(post(&valid, another.clone()).await.0, 429);
    assert_eq!(post(&valid, payload.clone()).await.2, first.2);
    let renamed = server.path.join("old-root");
    std::fs::rename(&root, &renamed).unwrap();
    assert_eq!(post(&valid, payload).await.2, first.2);
    assert_eq!(post(&valid, another).await.0, 429);
    assert_eq!(std::fs::read(renamed.join("z.tar")).unwrap(), b"recreated");
    server.close().await;
}

#[tokio::test]
async fn configured_webdav_delete_waits_for_the_provider_and_records_the_effect() {
    use base64::Engine;
    let key = dockervault_core::crypto::CredentialCipher::generate_key();
    let username = dockervault_core::crypto::CredentialCipher::generate_key();
    let password = dockervault_core::crypto::CredentialCipher::generate_key();
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!(
            "{}:{}",
            username.as_str(),
            password.as_str()
        ))
    );
    let ciphertext = dockervault_core::crypto::CredentialCipher::new(&key)
        .unwrap()
        .encrypt_value(&password);
    let config = Config::from_pairs([
        ("COOKIE_SECURE", "false"),
        ("CORS_ORIGINS", "http://localhost:3000"),
        ("CREDENTIAL_ENCRYPTION_KEY", key.as_str()),
    ])
    .unwrap();
    let server = Server::configured(config).await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/dav", listener.local_addr().unwrap());
    let remote = server.path.join("dav-fixture.tar");
    std::fs::write(&remote, b"selected bytes").unwrap();
    let target = remote.clone();
    let provider = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 4096);
            head.push(stream.read_u8().await.unwrap());
        }
        let head = String::from_utf8(head).unwrap();
        assert!(head.starts_with("DELETE /dav/backups/dav-fixture.tar HTTP/1.1\r\n"));
        assert!(
            head.lines()
                .filter_map(|line| line.split_once(':'))
                .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                    && value.trim() == expected)
        );
        std::fs::remove_file(target).unwrap();
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,webdav_url,username,password) VALUES (42,'fixture dav','webdav',1,'backups',?,?,?)")
        .bind(url).bind(username.as_str()).bind(ciphertext).execute(server.db.pool()).await.unwrap();
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let (status, _, result) = server
        .request(
            "POST",
            "/api/v1/operations",
            &headers,
            Some(json!({
                "action":"storage_delete", "storage_id":42, "path":"", "paths":["dav-fixture.tar"],
                "request_id":"2943eea3-0dd6-48b5-8d76-a01c54e40f99"
            })),
        )
        .await;
    assert_eq!(status, 202);
    tokio::time::timeout(std::time::Duration::from_secs(5), provider)
        .await
        .unwrap()
        .unwrap();
    let detail = format!(
        "/api/v1/operations/{}",
        result["operation_id"].as_str().unwrap()
    );
    let final_state = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, snapshot) = server.request("GET", &detail, &headers, None).await;
            assert_eq!(status, 200);
            if snapshot["finished_at"].is_string() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(final_state["state"], "completed");
    assert_eq!(final_state["succeeded"], 1);
    assert!(!remote.exists());
    server.close().await;
}

#[tokio::test]
async fn configured_webdav_retry_reexecutes_a_confirmed_rejection_once() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/dav", listener.local_addr().unwrap());
    let selected = server.path.join("retry.tar");
    std::fs::write(&selected, b"retry payload").unwrap();
    let remote = selected.clone();
    let provider = tokio::spawn(async move {
        for reply in [403, 204] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert!(line.starts_with("DELETE /dav/backups/retry.tar HTTP/1.1"));
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            let response = if reply == 403 {
                "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                std::fs::remove_file(&remote).unwrap();
                "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n"
            };
            stream
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap();
        }
        listener
    });
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,webdav_url) VALUES (42,'fixture dav','webdav',1,'backups',?)")
        .bind(url).execute(server.db.pool()).await.unwrap();
    let (status, _, first) = server.request("POST", "/api/v1/operations", &headers, Some(json!({
        "request_id":"b0598cda-877a-4f34-9173-c6762fa1d764", "action":"storage_delete", "storage_id":42, "path":"", "paths":["retry.tar"]
    }))).await;
    assert_eq!(status, 202);
    let server_ref = &server;
    let headers_ref = &headers;
    let settled = |detail: String| async move {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let (status, _, snapshot) =
                    server_ref.request("GET", &detail, headers_ref, None).await;
                assert_eq!(status, 200);
                if snapshot["finished_at"].is_string() {
                    break snapshot;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    };
    let parent = first["operation_id"].as_str().unwrap();
    let snapshot = settled(format!("/api/v1/operations/{parent}")).await;
    assert_eq!(snapshot["items"][0]["state"], "failed");
    assert_eq!(std::fs::read(&selected).unwrap(), b"retry payload");
    let retry = format!("/api/v1/operations/{parent}/retry");
    let body = json!({"request_id":"784b259d-59a5-424d-a81c-b2b450ebbdd5"});
    let (status, _, receipt) = server
        .request("POST", &retry, &headers, Some(body.clone()))
        .await;
    assert_eq!(status, 202);
    let child = settled(format!(
        "/api/v1/operations/{}",
        receipt["operation_id"].as_str().unwrap()
    ))
    .await;
    assert_eq!(child["parent_id"], parent);
    assert_eq!(
        child["items"],
        json!([{"path":"retry.tar","state":"succeeded","error":null}])
    );
    assert!(!selected.exists());
    std::fs::write(&selected, b"replacement").unwrap();
    let (status, _, replay) = server.request("POST", &retry, &headers, Some(body)).await;
    assert_eq!((status, replay), (202, receipt));
    let listener = tokio::time::timeout(std::time::Duration::from_secs(5), provider)
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&selected).unwrap(), b"replacement");
    server.close().await;
}

#[tokio::test]
async fn configured_ftp_delete_settles_after_the_control_session_closes() {
    configured_ftp_delete("221 bye\r\n").await;
}

#[tokio::test]
async fn confirmed_ftp_unlink_cannot_be_retried_after_quit_rejection() {
    configured_ftp_delete("500 quit not supported\r\n").await;
}

async fn configured_ftp_delete(quit_reply: &'static str) {
    use tokio::io::{AsyncBufReadExt, BufStream};
    let username = dockervault_core::crypto::CredentialCipher::generate_key();
    let password = dockervault_core::crypto::CredentialCipher::generate_key();
    let user_command = format!("USER {}\r\n", username.as_str());
    let pass_command = format!("PASS {}\r\n", password.as_str());
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let remote = server.path.join("ftp-fixture.tar");
    std::fs::write(&remote, b"selected bytes").unwrap();
    let target = remote.clone();
    let provider = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufStream::new(stream);
        stream.write_all(b"220 ready\r\n").await.unwrap();
        stream.flush().await.unwrap();
        let mut line = String::new();
        for (request, reply) in [
            (user_command.as_str(), "331 password\r\n"),
            (pass_command.as_str(), "230 logged in\r\n"),
            ("DELE /backups/ftp-fixture.tar\r\n", "250 deleted\r\n"),
            ("QUIT\r\n", quit_reply),
        ] {
            line.clear();
            stream.read_line(&mut line).await.unwrap();
            assert!(line == request, "unexpected FTP command");
            if request.starts_with("DELE") {
                std::fs::remove_file(&target).unwrap();
            }
            stream.write_all(reply.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();
        }
    });
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,host,port,username,password) VALUES (44,'fixture ftp','ftp',1,'/backups','127.0.0.1',?,?,?)")
        .bind(i64::from(port)).bind(username.as_str()).bind(password.as_str()).execute(server.db.pool()).await.unwrap();
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let (status, _, result) = server
        .request(
            "POST",
            "/api/v1/operations",
            &headers,
            Some(json!({
                "action":"storage_delete", "storage_id":44, "path":"", "paths":["ftp-fixture.tar"],
                "request_id":"4943eea3-0dd6-48b5-8d76-a01c54e40f99"
            })),
        )
        .await;
    assert_eq!(status, 202);
    tokio::time::timeout(std::time::Duration::from_secs(5), provider)
        .await
        .unwrap()
        .unwrap();
    let detail = format!(
        "/api/v1/operations/{}",
        result["operation_id"].as_str().unwrap()
    );
    let final_state = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, snapshot) = server.request("GET", &detail, &headers, None).await;
            assert_eq!(status, 200);
            if snapshot["finished_at"].is_string() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(final_state["state"], "completed");
    assert_eq!(final_state["succeeded"], 1);
    assert!(!remote.exists());
    server.close().await;
}

#[tokio::test]
async fn configured_s3_delete_settles_after_signed_provider_response() {
    configured_s3_delete(true).await;
}

#[tokio::test]
async fn configured_s3_delete_lost_response_is_unknown_and_not_sent_again() {
    configured_s3_delete(false).await;
}

async fn configured_s3_delete(acknowledge: bool) {
    let key = dockervault_core::crypto::CredentialCipher::generate_key();
    let cipher = dockervault_core::crypto::CredentialCipher::new(&key).unwrap();
    let server = Server::configured(
        Config::from_pairs([
            ("COOKIE_SECURE", "false"),
            ("CORS_ORIGINS", "http://localhost:3000"),
            ("CREDENTIAL_ENCRYPTION_KEY", key.as_str()),
        ])
        .unwrap(),
    )
    .await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let object = server.path.join("s3-fixture.tar");
    std::fs::write(&object, b"selected bytes").unwrap();
    let selected = object.clone();
    let access = dockervault_core::crypto::CredentialCipher::generate_key();
    let secret = dockervault_core::crypto::CredentialCipher::generate_key();
    let expected_access = format!("Credential={}/", access.as_str());
    let provider = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        assert!(line.starts_with(
            "DELETE /fixture-bucket/backups/s3-fixture.tar?x-id=DeleteObject HTTP/1.1"
        ));
        let mut signed = false;
        loop {
            line.clear();
            assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                signed |= name.eq_ignore_ascii_case("authorization")
                    && value.trim_start().starts_with("AWS4-HMAC-SHA256 ")
                    && value.contains(&expected_access);
            }
        }
        assert!(signed, "missing configured S3 signing identity");
        std::fs::remove_file(selected).unwrap();
        if acknowledge {
            stream
                .get_mut()
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        }
        listener
    });
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,s3_bucket,s3_endpoint_url,s3_access_key,s3_secret_key) VALUES (45,'fixture s3','s3',1,'backups','fixture-bucket',?,?,?)")
        .bind(url).bind(cipher.encrypt_value(&access)).bind(cipher.encrypt_value(&secret)).execute(server.db.pool()).await.unwrap();
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let (status, _, result) = server
        .request(
            "POST",
            "/api/v1/operations",
            &headers,
            Some(json!({
                "action":"storage_delete", "storage_id":45, "path":"", "paths":["s3-fixture.tar"],
                "request_id":"4ef7c55b-1394-4157-a577-f4f40b587ec5"
            })),
        )
        .await;
    assert_eq!(status, 202);
    let listener = tokio::time::timeout(std::time::Duration::from_secs(5), provider)
        .await
        .unwrap()
        .unwrap();
    let endpoint = format!(
        "/api/v1/operations/{}",
        result["operation_id"].as_str().unwrap()
    );
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, snapshot) = server.request("GET", &endpoint, &headers, None).await;
            assert_eq!(status, 200);
            if snapshot["finished_at"].is_string() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    if acknowledge {
        assert_eq!(snapshot["state"], "completed");
        assert_eq!(snapshot["succeeded"], 1);
    } else {
        assert_eq!(snapshot["state"], "failed");
        assert_eq!(snapshot["succeeded"], 0);
        assert_eq!(snapshot["items"][0]["state"], "unknown");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    }
    assert!(!object.exists());
    server.close().await;
}

#[tokio::test]
async fn configured_local_target_deletes_from_its_own_root() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let root = server.path.join("configured-local");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("same.tar"), b"selected").unwrap();
    let builtin = server.path.join("backups/same.tar");
    std::fs::write(&builtin, b"unselected").unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES (41,'fixture','local',1,?)")
        .bind(root.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let (status, _, response) = server
        .request(
            "POST",
            "/api/v1/operations",
            &headers,
            Some(json!({
                "action":"storage_delete", "storage_id":41, "path":"", "paths":["same.tar"],
                "request_id":"2943eea3-0dd6-48b5-8d76-a01c54e40f99"
            })),
        )
        .await;
    assert_eq!(status, 202);
    let id = response["operation_id"].as_str().unwrap();
    let detail = format!("/api/v1/operations/{id}");
    let final_state = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, snapshot) = server.request("GET", &detail, &headers, None).await;
            assert_eq!(status, 200);
            if snapshot["finished_at"].is_string() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(final_state["storage_id"], 41);
    assert_eq!(final_state["state"], "completed");
    assert_eq!(final_state["succeeded"], 1);
    assert!(!root.join("same.tar").exists());
    assert_eq!(std::fs::read(builtin).unwrap(), b"unselected");
    let text: String =
        sqlx::query_scalar("SELECT storage_identity FROM storage_operations WHERE id=?")
            .bind(id)
            .fetch_one(server.db.pool())
            .await
            .unwrap();
    let identity: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(identity["config"].as_str().unwrap().len(), 64);
    assert_eq!(identity["root_inode"].as_array().unwrap().len(), 2);
    assert!(identity.get("root").is_none());

    std::fs::write(root.join("same.tar"), b"must survive config drift").unwrap();
    // Change identity inside the committed admission, before a worker can claim the item.
    sqlx::query("CREATE TRIGGER drift_after_admission AFTER INSERT ON storage_operations WHEN NEW.storage_id='41' BEGIN UPDATE remote_storages SET name='reconfigured' WHERE id=41; END")
        .execute(server.db.pool()).await.unwrap();
    let (status, _, response) = server
        .request(
            "POST",
            "/api/v1/operations",
            &headers,
            Some(json!({
                "action":"storage_delete", "storage_id":41, "path":"", "paths":["same.tar"],
                "request_id":"3943eea3-0dd6-48b5-8d76-a01c54e40f99"
            })),
        )
        .await;
    assert_eq!(status, 202);
    let detail = format!(
        "/api/v1/operations/{}",
        response["operation_id"].as_str().unwrap()
    );
    let final_state = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, snapshot) = server.request("GET", &detail, &headers, None).await;
            assert_eq!(status, 200);
            if snapshot["finished_at"].is_string() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(final_state["state"], "failed");
    assert_eq!(
        final_state["items"][0]["error"],
        "Storage identity changed or unavailable; target not deleted"
    );
    assert_eq!(
        std::fs::read(root.join("same.tar")).unwrap(),
        b"must survive config drift"
    );
    let retry = format!("{detail}/retry");
    let body = json!({"request_id":"f45fb043-5463-4205-a666-d4b9db6356b2"});
    let (status, _, rejection) = server
        .request("POST", &retry, &headers, Some(body.clone()))
        .await;
    assert_eq!(
        (status, rejection),
        (
            409,
            json!({"detail":"Storage identity changed; inspect before starting a new operation"})
        )
    );
    sqlx::raw_sql("DROP TRIGGER drift_after_admission; UPDATE remote_storages SET name='fixture' WHERE id=41;")
        .execute(server.db.pool()).await.unwrap();
    let (status, _, receipt) = server
        .request("POST", &retry, &headers, Some(body.clone()))
        .await;
    assert_eq!(status, 202);
    let child = format!(
        "/api/v1/operations/{}",
        receipt["operation_id"].as_str().unwrap()
    );
    let child = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, snapshot) = server.request("GET", &child, &headers, None).await;
            assert_eq!(status, 200);
            if snapshot["finished_at"].is_string() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(child["parent_id"], response["operation_id"]);
    assert_eq!(child["storage_id"], 41);
    assert_eq!(
        child["items"],
        json!([{"path":"same.tar","state":"succeeded","error":null}])
    );
    assert!(!root.join("same.tar").exists());
    std::fs::write(root.join("same.tar"), b"replacement").unwrap();
    sqlx::query("DELETE FROM remote_storages WHERE id=41")
        .execute(server.db.pool())
        .await
        .unwrap();
    let (status, _, replay) = server.request("POST", &retry, &headers, Some(body)).await;
    assert_eq!((status, replay), (202, receipt));
    assert_eq!(
        std::fs::read(root.join("same.tar")).unwrap(),
        b"replacement"
    );
    server.close().await;
}

#[tokio::test]
async fn python_remote_receipt_can_be_replayed_after_its_target_was_removed() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let root = server.path.join("backups");
    std::fs::write(root.join("fixture.tar"), b"replacement").unwrap();
    const REQUEST: &str = "9843eea3-0dd6-48b5-8d76-a01c54e40f99";
    let payload = json!({"action":"storage_delete","storage_id":"41","path":"","paths":["fixture.tar"],"parent_id":null});
    sqlx::query("INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at) VALUES ('legacy-remote',1,?,?,'{}','storage_delete','41','','completed','finished',1,1,1,0,0,0,0,'2024-01-01','2024-01-01')")
        .bind(REQUEST).bind(payload.to_string()).execute(server.db.pool()).await.unwrap();
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let mut body = json!({"action":"storage_delete","storage_id":41,"path":".","paths":["./fixture.tar","fixture.tar"],"request_id":REQUEST});
    let (status, _, receipt) = server
        .request("POST", "/api/v1/operations", &headers, Some(body.clone()))
        .await;
    assert_eq!(
        (status, receipt),
        (202, json!({"operation_id":"legacy-remote"}))
    );
    body["paths"] = json!(["different.tar"]);
    assert_eq!(
        server
            .request("POST", "/api/v1/operations", &headers, Some(body))
            .await
            .0,
        409
    );
    assert_eq!(
        std::fs::read(root.join("fixture.tar")).unwrap(),
        b"replacement"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM storage_operations")
            .fetch_one(server.db.pool())
            .await
            .unwrap(),
        1
    );
    server.close().await;
}

#[tokio::test]
async fn local_delete_removes_a_selected_tree_without_following_links() {
    use std::os::unix::fs::symlink;
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let root = server.path.join("backups");
    let outside = server.path.join("outside");
    std::fs::create_dir_all(root.join("selected/nested")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("selected/nested/data"), b"selected").unwrap();
    std::fs::write(outside.join("keep"), b"outside").unwrap();
    std::fs::write(root.join("neighbor"), b"untouched").unwrap();
    symlink(&outside, root.join("selected/nested/link")).unwrap();
    let route = "/api/v1/operations";
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let (status, _, body) = server
        .request(
            "POST",
            route,
            &headers,
            Some(json!({
                "action":"storage_delete", "storage_id":"local", "path":"", "paths":["selected"],
                "request_id":"1b5124a4-55f0-46d5-b6ab-3251c8827a2f"
            })),
        )
        .await;
    assert_eq!(status, 202);
    let detail = format!("{route}/{}", body["operation_id"].as_str().unwrap());
    let settled = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, snapshot) = server.request("GET", &detail, &headers, None).await;
            assert_eq!(status, 200);
            if snapshot["finished_at"].is_string() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(settled["state"], "completed");
    assert_eq!(
        settled["items"],
        json!([{"path":"selected","state":"succeeded","error":null}])
    );
    assert!(!root.join("selected").exists());
    assert_eq!(std::fs::read(outside.join("keep")).unwrap(), b"outside");
    assert_eq!(std::fs::read(root.join("neighbor")).unwrap(), b"untouched");
    server.close().await;
}

#[tokio::test]
async fn retry_links_only_failed_items_and_replay_does_not_touch_recreated_bytes() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let root = server.path.join("backups");
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    use std::os::unix::fs::MetadataExt;
    let info = std::fs::metadata(&root).unwrap();
    let identity = json!({"root": root, "root_inode":[info.dev(),info.ino()]}).to_string();
    sqlx::query("INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at)
        VALUES ('retry-parent',1,'original','{}',?,'storage_delete','local','','failed','finished',3,3,1,1,0,1,0,'2024-01-01','2024-01-01')")
        .bind(identity).execute(server.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO storage_operation_items(operation_id,path,state) VALUES ('retry-parent','success.tar','succeeded'),('retry-parent','failed.tar','failed'),('retry-parent','uncertain.tar','unknown')")
        .execute(server.db.pool()).await.unwrap();
    for name in ["success.tar", "failed.tar", "uncertain.tar"] {
        std::fs::write(root.join(name), name.as_bytes()).unwrap();
    }
    let body = json!({"request_id":"1a5124a4-55f0-46d5-b6ab-3251c8827a2f"});
    let (status, _, missing) = server
        .request(
            "POST",
            "/api/v1/operations/not-mine/retry",
            &headers,
            Some(body.clone()),
        )
        .await;
    assert_eq!(
        (status, missing),
        (404, json!({"detail":"Operation not found"}))
    );
    let (status, _, result) = server
        .request(
            "POST",
            "/api/v1/operations/retry-parent/retry",
            &headers,
            Some(body.clone()),
        )
        .await;
    assert_eq!(status, 202);
    let id = result["operation_id"].as_str().unwrap();
    assert_ne!(id, "retry-parent");
    let detail = format!("/api/v1/operations/{id}");
    let settled = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, snapshot) = server.request("GET", &detail, &headers, None).await;
            assert_eq!(status, 200);
            if snapshot["finished_at"].is_string() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(settled["parent_id"], "retry-parent");
    assert_eq!(settled["state"], "completed");
    assert_eq!(
        settled["items"],
        json!([{"path":"failed.tar","state":"succeeded","error":null}])
    );
    assert!(!root.join("failed.tar").exists());
    std::fs::write(root.join("failed.tar"), b"recreated").unwrap();
    let (status, _, replay) = server
        .request(
            "POST",
            "/api/v1/operations/retry-parent/retry",
            &headers,
            Some(body),
        )
        .await;
    assert_eq!(status, 202);
    assert_eq!(replay, result);
    assert_eq!(
        std::fs::read(root.join("failed.tar")).unwrap(),
        b"recreated"
    );
    for name in ["success.tar", "uncertain.tar"] {
        assert_eq!(std::fs::read(root.join(name)).unwrap(), name.as_bytes());
    }
    let original = server.path.join("original-root");
    std::fs::rename(&root, &original).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("failed.tar"), b"replacement root").unwrap();
    let (status, _, rejected) = server
        .request(
            "POST",
            "/api/v1/operations/retry-parent/retry",
            &headers,
            Some(json!({"request_id":"2a5124a4-55f0-46d5-b6ab-3251c8827a2f"})),
        )
        .await;
    assert_eq!(
        (status, rejected),
        (
            409,
            json!({"detail":"Storage identity changed; inspect before starting a new operation"})
        )
    );
    assert_eq!(
        std::fs::read(root.join("failed.tar")).unwrap(),
        b"replacement root"
    );
    assert_eq!(
        std::fs::read(original.join("failed.tar")).unwrap(),
        b"recreated"
    );
    server.close().await;
}

#[tokio::test]
async fn storage_types_requires_session_and_returns_current_provider_contract() {
    let server = Server::start("http://localhost:3000").await;
    let path = "/api/v1/storage/types";
    let (status, _, body) = server.request("GET", path, &[], None).await;
    assert_eq!(
        (status, body),
        (
            403,
            json!({"detail":"Setup required","setup_required":true})
        )
    );
    let cookie = server.setup().await;
    let (status, _, _) = server.request("GET", path, &[], None).await;
    assert_eq!(status, 401);
    let (status, _, types) = server
        .request("GET", path, &[("Cookie", cookie.as_str())], None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(types.as_object().unwrap().len(), 6);
    assert_eq!(
        types["ftp"],
        json!({"name":"FTP (unencrypted)","description":"Plain FTP server; use SSH/SFTP for secure transport. FTPS is not supported.","required":["host","username","password","base_path"],"optional":["port"]})
    );
    assert_eq!(
        types["ssh"]["optional"],
        json!(["port", "ssh_key_path", "password"])
    );
    assert_eq!(
        types["s3"]["required"],
        json!(["s3_bucket", "s3_access_key", "s3_secret_key"])
    );
    server.close().await;
}

#[tokio::test]
async fn storage_inventory_and_detail_keep_legacy_repair_hints_without_credentials() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let fields = ["password", "s3_access_key", "s3_secret_key"];
    let synthesized = dockervault_core::crypto::CredentialCipher::generate_key();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,webdav_url,password,s3_access_key,s3_secret_key,created_at,updated_at) VALUES (44,'fixture','webdav',0,'backups','not a URL',?,?,?,'2024-02-01 01:02:03.000000','2024-02-02 01:02:03.120000')")
        .bind(synthesized.as_str()).bind(synthesized.as_str()).bind(synthesized.as_str()).execute(server.db.pool()).await.unwrap();
    let headers = [("Cookie", cookie.as_str())];
    let (status, _, list) = server
        .request("GET", "/api/v1/storage", &headers, None)
        .await;
    assert_eq!(status, 200);
    let (_, _, detail) = server
        .request("GET", "/api/v1/storage/44", &headers, None)
        .await;
    assert!(
        fields
            .iter()
            .all(|field| detail.get(field).is_none() && list[0].get(field).is_none())
    );
    assert_eq!(list[0], detail);
    for path in ["/api/v1/storage/44.0", "/api/v1/storage/%2044%20"] {
        let (status, _, equivalent) = server.request("GET", path, &headers, None).await;
        assert_eq!((status, equivalent), (200, detail.clone()));
    }
    assert_eq!(detail["id"], 44);
    assert_eq!(detail["enabled"], false);
    assert_eq!(detail["created_at"], "2024-02-01T01:02:03");
    assert_eq!(detail["updated_at"], "2024-02-02T01:02:03.120000");
    assert_eq!(
        detail["validation_errors"],
        json!(["webdav_url must be an http(s) URL with a valid hostname and port"])
    );
    let (status, _, missing) = server
        .request("GET", "/api/v1/storage/45", &headers, None)
        .await;
    assert_eq!(
        (status, missing),
        (404, json!({"detail":"Storage not found"}))
    );
    server.close().await;
}

#[tokio::test]
async fn storage_create_persists_provider_fields_and_encrypts_credentials() {
    let key = dockervault_core::crypto::CredentialCipher::generate_key();
    let cipher = dockervault_core::crypto::CredentialCipher::new(&key).unwrap();
    let server = Server::configured(
        Config::from_pairs([
            ("COOKIE_SECURE", "false"),
            ("CORS_ORIGINS", "http://localhost:3000"),
            ("CREDENTIAL_ENCRYPTION_KEY", key.as_str()),
        ])
        .unwrap(),
    )
    .await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let access = dockervault_core::crypto::CredentialCipher::generate_key();
    let secret = dockervault_core::crypto::CredentialCipher::generate_key();
    let (status, _, view) = server
        .request(
            "POST",
            "/api/v1/storage",
            &headers,
            Some(json!({
                "name":"Synthetic", "storage_type":"s3", "s3_bucket":"fixture-bucket",
                "s3_access_key":access.as_str(), "s3_secret_key":secret.as_str()
            })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(view["base_path"], "/backups");
    assert_eq!(view["enabled"], true);
    assert_eq!(view["validation_errors"], json!([]));
    assert!(view.get("s3_access_key").is_none() && view.get("s3_secret_key").is_none());
    let (saved_access, saved_secret): (String, String) =
        sqlx::query_as("SELECT s3_access_key,s3_secret_key FROM remote_storages WHERE id=?")
            .bind(view["id"].as_i64().unwrap())
            .fetch_one(server.db.pool())
            .await
            .unwrap();
    assert!(saved_access.starts_with("fernet:") && saved_secret.starts_with("fernet:"));
    assert!(cipher.decrypt_value(&saved_access).unwrap() == access.as_str());
    assert!(cipher.decrypt_value(&saved_secret).unwrap() == secret.as_str());
    let endpoint = format!("/api/v1/storage/{}", view["id"].as_i64().unwrap());
    let (status, _, changed) = server
        .request(
            "PUT",
            &endpoint,
            &headers,
            Some(json!({"enabled":false,"s3_access_key":""})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(changed["enabled"], false);
    let saved: String = sqlx::query_scalar("SELECT s3_access_key FROM remote_storages WHERE id=?")
        .bind(view["id"].as_i64().unwrap())
        .fetch_one(server.db.pool())
        .await
        .unwrap();
    assert!(saved == saved_access);
    let (status, _, _) = server
        .request(
            "PUT",
            &endpoint,
            &headers,
            Some(json!({"s3_access_key":null})),
        )
        .await;
    assert_eq!(status, 400);
    let committed: String =
        sqlx::query_scalar("SELECT s3_access_key FROM remote_storages WHERE id=?")
            .bind(view["id"].as_i64().unwrap())
            .fetch_one(server.db.pool())
            .await
            .unwrap();
    assert!(committed == saved_access);
    server.close().await;
}

#[tokio::test]
async fn configured_rclone_listing_rejects_an_invalid_path() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,rclone_remote) VALUES(44,'fixture','rclone',1,'/base','fixture')")
        .execute(server.db.pool()).await.unwrap();
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/44/files?path=..%2Foutside",
            &[("Cookie", cookie.as_str())],
            None,
        )
        .await;
    assert_eq!((status, body), (400, json!({"detail":"Invalid file path"})));
    server.close().await;
}

#[tokio::test]
async fn configured_sftp_listing_rejects_an_invalid_path_before_opening_a_connection() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,host,port,username) VALUES(44,'fixture','sftp',1,'/base','127.0.0.1',?,'fixture')")
        .bind(listener.local_addr().unwrap().port()).execute(server.db.pool()).await.unwrap();
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/44/files?path=..%2Foutside",
            &[("Cookie", cookie.as_str())],
            None,
        )
        .await;
    assert_eq!((status, body), (400, json!({"detail":"Invalid file path"})));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    server.close().await;
}

#[tokio::test]
async fn configured_s3_listing_preserves_nested_keys_and_reports_rejection() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,s3_bucket,s3_endpoint_url,s3_access_key,s3_secret_key) VALUES (44,'fixture','s3',1,'base','fixture-bucket',?,'fixture-access','fixture-secret')")
        .bind(endpoint).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        let success = "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>base/folder/sub/archive.tar</Key><LastModified>2024-01-01T00:00:00Z</LastModified><Size>7</Size></Contents></ListBucketResult>";
        for (status, body) in [
            ("200 OK", success),
            ("403 Forbidden", "<Error><Code>AccessDenied</Code></Error>"),
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert!(line.starts_with("GET /fixture-bucket"));
            assert!(line.contains("list-type=2"));
            assert!(line.contains("prefix=base%2Ffolder%2F"));
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            stream.get_mut().write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    let path = "/api/v1/storage/44/files?path=folder";
    let (status, _, body) = server.request("GET", path, &headers, None).await;
    assert_eq!(
        (status, body),
        (
            200,
            json!({"path":"folder","files":[{"name":"sub/archive.tar","size":7,"is_dir":false,"modified":1704067200.0}]})
        )
    );
    let (status, _, body) = server.request("GET", path, &headers, None).await;
    assert_eq!(
        (status, body),
        (502, json!({"detail":"Remote listing failed"}))
    );
    provider.await.unwrap();
    server.close().await;
}

#[tokio::test]
async fn configured_ftp_listing_waits_for_mlsd_completion() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,host,port,username) VALUES (44,'fixture','ftp',1,'/base','127.0.0.1',?,'fixture')")
        .bind(listener.local_addr().unwrap().port()).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        let passive = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut control = BufReader::new(stream);
        control.get_mut().write_all(b"220 Ready\r\n").await.unwrap();
        for (expected, reply) in [
            ("USER fixture", "230 Authorized".to_owned()),
            ("TYPE I", "200 Binary".to_owned()),
            (
                "EPSV",
                format!(
                    "229 Entering (|||{}|)",
                    passive.local_addr().unwrap().port()
                ),
            ),
            ("MLSD /base/child", "150 Opening".to_owned()),
        ] {
            let mut command = String::new();
            control.read_line(&mut command).await.unwrap();
            assert!(command.trim_end() == expected);
            control
                .get_mut()
                .write_all(format!("{reply}\r\n").as_bytes())
                .await
                .unwrap();
        }
        let (mut data, _) = passive.accept().await.unwrap();
        data.write_all(b"type=file;size=7;modify=20240101000000; archive.tar\r\n")
            .await
            .unwrap();
        drop(data);
        control
            .get_mut()
            .write_all(b"226 Settled\r\n")
            .await
            .unwrap();
        let mut command = String::new();
        control.read_line(&mut command).await.unwrap();
        assert_eq!(command.trim_end(), "QUIT");
        control.get_mut().write_all(b"221 Bye\r\n").await.unwrap();
    });
    let (status, _, result) = server
        .request(
            "GET",
            "/api/v1/storage/44/files?path=child",
            &[("Cookie", cookie.as_str())],
            None,
        )
        .await;
    assert_eq!(
        (status, result),
        (
            200,
            json!({"path":"child","files":[{"name":"archive.tar","size":7,"is_dir":false,"modified":1704067200.0}]})
        )
    );
    provider.await.unwrap();
    server.close().await;
}

#[tokio::test]
async fn configured_webdav_listing_reports_provider_failure_not_empty_success() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/dav", listener.local_addr().unwrap());
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,webdav_url) VALUES (44,'fixture','webdav',1,'base',?)")
        .bind(url).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let document = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/base/folder/archive.tar</d:href><d:propstat><d:prop><d:getcontentlength>7</d:getcontentlength><d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
        for (status, body) in [
            ("207 Multi-Status", document),
            ("403 Forbidden", "private fixture response"),
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(line.trim_end(), "PROPFIND /dav/base/folder/ HTTP/1.1");
            let mut depth = false;
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                depth |= line.trim_end().eq_ignore_ascii_case("depth: 1");
            }
            assert!(depth);
            stream.get_mut().write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    let path = "/api/v1/storage/44/files?path=folder";
    let (status, _, result) = server.request("GET", path, &headers, None).await;
    assert_eq!(status, 200);
    assert_eq!(result["path"], "folder");
    assert_eq!(result["files"].as_array().unwrap().len(), 1);
    assert_eq!(result["files"][0]["name"], "archive.tar");
    assert_eq!(result["files"][0]["size"], 7);
    let (status, _, result) = server.request("GET", path, &headers, None).await;
    assert_eq!(
        (status, result),
        (502, json!({"detail":"Remote listing failed"}))
    );
    provider.await.unwrap();
    server.close().await;
}

#[tokio::test]
async fn webdav_bulk_download_stages_success_and_skips_missing_objects() {
    http_bulk_download_stages_success_and_skips_missing_objects(false).await;
}

#[tokio::test]
async fn s3_bulk_download_stages_success_and_skips_missing_objects() {
    http_bulk_download_stages_success_and_skips_missing_objects(true).await;
}

async fn http_bulk_download_stages_success_and_skips_missing_objects(s3: bool) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    if s3 {
        sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,s3_bucket,s3_endpoint_url,s3_access_key,s3_secret_key) VALUES(44,'fixture','s3',1,'base','fixture-bucket',?,'fixture-access','fixture-secret')")
            .bind(endpoint).execute(server.db.pool()).await.unwrap();
    } else {
        sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,webdav_url) VALUES(44,'fixture','webdav',1,'/base',?)")
            .bind(endpoint).execute(server.db.pool()).await.unwrap();
    }
    let provider = tokio::spawn(async move {
        for (name, status, body) in [
            ("archive.tar", "200 OK", "fixture"),
            (
                "missing.tar",
                "404 Not Found",
                if s3 {
                    "<Error><Code>NoSuchKey</Code></Error>"
                } else {
                    ""
                },
            ),
            ("short.tar", "200 OK", "short"),
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let path = if s3 {
                format!("/fixture-bucket/base/{name}?x-id=GetObject")
            } else {
                format!("/base/{name}")
            };
            assert_eq!(line, format!("GET {path} HTTP/1.1\r\n"));
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            let expected = if name == "short.tar" { 20 } else { body.len() };
            stream.get_mut().write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {expected}\r\nConnection: close\r\n\r\n{body}").as_bytes()).await.unwrap();
        }
    });
    let client = reqwest::Client::new();
    let route = "/api/v1/storage/44/files/bulk-download";
    let response = client
        .post(format!("http://{}{route}", server.address))
        .header("Cookie", &cookie)
        .header("Content-Type", "application/json")
        .body(r#"{"paths":["archive.tar","missing.tar"]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/zip");
    let mut archive = async_zip::base::read1::seek::ZipArchiveReader::open(
        tokio_util::compat::TokioAsyncReadCompatExt::compat(std::io::Cursor::new(
            response.bytes().await.unwrap(),
        )),
    )
    .await
    .unwrap();
    assert_eq!(archive.cdrs().len(), 1);
    assert_eq!(archive.find(b"archive.tar").unwrap().count(), 1);
    let mut contents = Vec::new();
    tokio_util::compat::FuturesAsyncReadCompatExt::compat(archive.file(0).await.unwrap())
        .read_to_end(&mut contents)
        .await
        .unwrap();
    assert_eq!(contents, b"fixture");
    let (status, _, body) = server
        .request(
            "POST",
            route,
            &[("Cookie", cookie.as_str())],
            Some(json!({"paths":["short.tar"]})),
        )
        .await;
    assert_eq!(
        (status, body),
        (500, json!({"detail":"Bulk download failed"}))
    );
    provider.await.unwrap();
    server.close().await;
}

#[tokio::test]
async fn configured_local_bulk_download_reports_missing_storage() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let (status, _, body) = server
        .request(
            "POST",
            "/api/v1/storage/45/files/bulk-download",
            &[("Cookie", cookie.as_str())],
            Some(json!({"paths": ["archive.tar"]})),
        )
        .await;
    assert_eq!((status, body), (404, json!({"detail":"Storage not found"})));
    server.close().await;
}

#[tokio::test]
async fn configured_bulk_download_aggregates_invalid_id_and_body() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let (status, _, body) = server
        .request(
            "POST",
            "/api/v1/storage/invalid/files/bulk-download",
            &[("Cookie", cookie.as_str())],
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (status, body),
        (
            422,
            json!({"detail":"Validation error","errors":[{"field":"path.storage_id","message":"Input should be a valid integer, unable to parse string as an integer"},{"field":"body.paths","message":"Field required"}]})
        )
    );
    server.close().await;
}

#[tokio::test]
async fn local_bulk_download_rejects_empty_selection() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let (status, _, body) = server
        .request(
            "POST",
            "/api/v1/storage/local/files/bulk-download",
            &[("Cookie", cookie.as_str())],
            Some(json!({"paths": []})),
        )
        .await;
    assert_eq!(
        (status, body),
        (400, json!({"detail":"No files specified"}))
    );
    server.close().await;
}

#[tokio::test]
async fn local_bulk_download_contains_only_existing_selected_file_bytes() {
    use async_zip::base::read1::seek::ZipArchiveReader;
    use std::io::Cursor;
    use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let dir = server.path.join("backups/folder");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("archive.tar"), b"\0archive\xff").unwrap();
    let other = server.path.join("backups/other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("archive.tar"), b"second").unwrap();
    let url = format!(
        "http://{}/api/v1/storage/local/files/bulk-download",
        server.address
    );
    let response = reqwest::Client::new()
        .post(url)
        .header("Cookie", cookie)
        .header("Content-Type", "application/json")
        .body(
            r#"{"paths":["folder/archive.tar","folder/missing.tar","folder","other/archive.tar"]}"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"folder.zip\""
    );
    assert_eq!(response.headers()["content-type"], "application/zip");
    let mut archive = ZipArchiveReader::open(Cursor::new(response.bytes().await.unwrap()).compat())
        .await
        .unwrap();
    assert_eq!(archive.cdrs().len(), 2);
    assert_eq!(archive.find(b"archive.tar").unwrap().count(), 2);
    for (index, expected) in [b"\0archive\xff".as_slice(), b"second".as_slice()]
        .into_iter()
        .enumerate()
    {
        let mut bytes = Vec::new();
        archive
            .file(index)
            .await
            .unwrap()
            .compat()
            .read_to_end(&mut bytes)
            .await
            .unwrap();
        assert_eq!(bytes, expected);
    }
    server.close().await;
}

#[tokio::test]
async fn backup_archive_download_returns_exact_ciphertext_sidecar_and_ranges() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let auth = [("Cookie", cookie.as_str())];
    let root = server.path.join("backups");
    std::fs::write(root.join("old_name.tar"), b"synthetic archive").unwrap();
    std::fs::write(root.join("archive.tar.enc"), b"ciphertext fixture").unwrap();
    std::fs::write(root.join("custom.key"), b"wrapped key fixture").unwrap();
    std::fs::write(server.path.join("outside.key"), b"outside root").unwrap();
    std::os::unix::fs::symlink(server.path.join("outside.key"), root.join("unsafe.key")).unwrap();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type) VALUES(1,'fixture','path')")
        .execute(server.db.pool())
        .await
        .unwrap();
    for (id, status, name, encrypted, key) in [
        (1, "COMPLETED", "old::name.tar", false, None),
        (2, "COMPLETED", "archive.tar.enc", true, Some("custom.key")),
        (3, "FAILED", "archive.tar.enc", true, None),
        (4, "COMPLETED", "missing.tar", false, None),
        (5, "COMPLETED", "archive.tar.enc", true, Some("unsafe.key")),
    ] {
        sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path,encrypted,encryption_key_path) VALUES(?,1,'FULL',?,?,?,?)")
            .bind(id).bind(status).bind(root.join(name).to_string_lossy().into_owned()).bind(encrypted)
            .bind(key.map(|path| root.join(path).to_string_lossy().into_owned())).execute(server.db.pool()).await.unwrap();
    }
    for (url, name, bytes) in [
        ("1/download", "old_name.tar", "synthetic archive"),
        ("2/download", "archive.tar.enc", "ciphertext fixture"),
        (
            "2/download?sidecar=true",
            "custom.key",
            "wrapped key fixture",
        ),
    ] {
        let (status, headers, body) = server
            .request("GET", &format!("/api/v1/backups/{url}"), &auth, None)
            .await;
        assert_eq!((status, body), (200, json!(bytes)));
        for header in [
            "content-type: application/octet-stream",
            "cache-control: no-store",
            "accept-ranges: bytes",
            &format!("content-disposition: attachment; filename=\"{name}\""),
        ] {
            assert!(headers.contains(header), "missing {header}");
        }
    }
    let (status, headers, body) = server
        .request(
            "GET",
            "/api/v1/backups/2/download",
            &[("Cookie", cookie.as_str()), ("Range", "bytes=0-5")],
            None,
        )
        .await;
    assert_eq!((status, body), (206, json!("cipher")));
    assert!(headers.contains("content-range: bytes 0-5/18"));
    for (url, status, detail) in [
        (
            "1/download?sidecar=true",
            404,
            "Archive has no encryption sidecar",
        ),
        ("3/download", 409, "Run has no completed archive"),
        ("4/download", 404, "Archive content unavailable"),
        (
            "5/download?sidecar=true",
            404,
            "Archive content unavailable",
        ),
        ("999/download", 404, "Archive not found"),
    ] {
        let (actual, _, body) = server
            .request("GET", &format!("/api/v1/backups/{url}"), &auth, None)
            .await;
        assert_eq!((actual, body), (status, json!({"detail":detail})));
    }
    assert_eq!(
        server
            .request(
                "GET",
                "/api/v1/backups/1/download?sidecar=invalid",
                &auth,
                None
            )
            .await
            .0,
        422
    );
    assert_eq!(
        server
            .request("GET", "/api/v1/backups/1/download", &[], None)
            .await
            .0,
        401
    );
    server.close().await;
}

#[tokio::test]
async fn local_download_serves_attachment_bytes_and_rejects_symlinks() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let root = server.path.join("backups");
    std::fs::write(root.join("an archive.tar"), b"archive bytes").unwrap();
    let outside = server.path.join("outside");
    std::fs::write(&outside, b"outside").unwrap();
    std::os::unix::fs::symlink(outside, root.join("alias.tar")).unwrap();
    let url = "/api/v1/storage/local/files/download";
    let (status, response, body) = server
        .request(
            "GET",
            &format!("{url}?path=an%20archive.tar"),
            &headers,
            None,
        )
        .await;
    assert_eq!((status, body), (200, json!("archive bytes")));
    assert!(response.contains("content-type: application/octet-stream"));
    assert!(
        response.contains("content-disposition: attachment; filename*=utf-8''an%20archive.tar")
    );
    assert!(response.contains("content-length: 13"));
    let (status, _, body) = server
        .request("GET", &format!("{url}?path=alias.tar"), &headers, None)
        .await;
    assert_eq!((status, body), (500, json!({"detail":"Download failed"})));
    server.close().await;
}

#[tokio::test]
async fn local_download_slots_are_shared_and_returned_on_disconnect() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let root = server.path.join("backups");
    std::fs::File::create(root.join("large.tar"))
        .unwrap()
        .set_len(16 * 1024 * 1024)
        .unwrap();
    std::fs::write(root.join("small.tar"), b"released").unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES(44,'fixture','local',1,?)")
        .bind(root.to_str().unwrap()).execute(server.db.pool()).await.unwrap();

    let mut readers = Vec::new();
    for provider in ["local", "44", "local", "44"] {
        let mut stream = BufReader::new(TcpStream::connect(server.address).await.unwrap());
        stream.get_mut().write_all(format!("GET /api/v1/storage/{provider}/files/download?path=large.tar HTTP/1.1\r\nHost: {}\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n", server.address).as_bytes()).await.unwrap();
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("HTTP/1.1 200"), "{line}");
        readers.push(stream);
    }
    let short = "/api/v1/storage/44/files/download?path=small.tar";
    let headers = [("Cookie", cookie.as_str())];
    assert_eq!(server.request("GET", short, &headers, None).await.0, 503);
    assert_eq!(server.request("GET", "/health", &[], None).await.0, 200);
    drop(readers);
    let recovered = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, body) = server.request("GET", short, &headers, None).await;
            if status == 200 {
                break body;
            }
            assert_eq!(status, 503);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(recovered, json!("released"));
    server.close().await;
}

#[tokio::test]
async fn local_download_honors_ranges_and_file_validators() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let path = server.path.join("backups/archive.tar");
    std::fs::write(&path, b"0123456789abcdef").unwrap();
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1704067200);
    std::fs::File::open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
    let url = "/api/v1/storage/local/files/download?path=archive.tar";
    let cookie = ("Cookie", cookie.as_str());
    let (status, headers, body) = server
        .request("GET", url, &[cookie, ("Range", "bytes=5-11")], None)
        .await;
    assert_eq!((status, body), (206, json!("56789ab")));
    assert!(headers.contains("content-range: bytes 5-11/16"));
    assert!(headers.contains("accept-ranges: bytes"));
    assert!(headers.contains("last-modified: Mon, 01 Jan 2024 00:00:00 GMT"));
    let etag = headers
        .lines()
        .find_map(|line| line.strip_prefix("etag: "))
        .unwrap();
    let (status, _, body) = server
        .request(
            "GET",
            url,
            &[cookie, ("Range", "bytes=-2"), ("If-Range", etag)],
            None,
        )
        .await;
    assert_eq!((status, body), (206, json!("ef")));
    let (status, _, body) = server
        .request(
            "GET",
            url,
            &[cookie, ("Range", "bytes=5-11"), ("If-Range", "\"older\"")],
            None,
        )
        .await;
    assert_eq!((status, body), (200, json!("0123456789abcdef")));
    let (status, headers, _) = server
        .request("GET", url, &[cookie, ("Range", "bytes=99-100")], None)
        .await;
    assert_eq!(status, 416);
    assert!(headers.contains("content-range: bytes */16"));
    let (status, headers, body) = server
        .request("GET", url, &[cookie, ("Range", "bytes=0-1,14-15")], None)
        .await;
    assert_eq!(status, 206);
    assert!(headers.contains("content-type: multipart/byteranges; boundary="));
    let body = body.as_str().unwrap();
    assert!(body.contains("Content-Range: bytes 0-1/16\r\n\r\n01\r\n"));
    assert!(body.contains("Content-Range: bytes 14-15/16\r\n\r\nef\r\n"));
    server.close().await;
}

#[tokio::test]
async fn configured_rclone_download_rejects_invalid_path() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,rclone_remote) VALUES(44,'fixture','rclone',1,'/base','fixture')").execute(server.db.pool()).await.unwrap();
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/44/files/download?path=..%2Foutside",
            &[("Cookie", cookie.as_str())],
            None,
        )
        .await;
    assert_eq!((status, body), (400, json!({"detail":"Invalid file path"})));
    server.close().await;
}

#[tokio::test]
async fn configured_sftp_bulk_download_rejects_invalid_selection_before_contact() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,host,port,username) VALUES(44,'fixture','sftp',1,'/base','127.0.0.1',?,'fixture')")
        .bind(listener.local_addr().unwrap().port()).execute(server.db.pool()).await.unwrap();
    let result = server
        .request(
            "POST",
            "/api/v1/storage/44/files/bulk-download",
            &[("Cookie", cookie.as_str())],
            Some(json!({"paths":["archive.tar","../outside"]})),
        )
        .await;
    assert_eq!(
        (result.0, result.2),
        (400, json!({"detail":"Invalid file path"}))
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    server.close().await;
}

#[tokio::test]
async fn configured_sftp_download_rejects_invalid_path_before_contact() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,host,port,username) VALUES(44,'fixture','sftp',1,'/base','127.0.0.1',?,'fixture')")
        .bind(listener.local_addr().unwrap().port()).execute(server.db.pool()).await.unwrap();
    let result = server
        .request(
            "GET",
            "/api/v1/storage/44/files/download?path=..%2Foutside",
            &[("Cookie", cookie.as_str())],
            None,
        )
        .await;
    assert_eq!(
        (result.0, result.2),
        (400, json!({"detail":"Invalid file path"}))
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    server.close().await;
}

#[tokio::test]
async fn configured_ftp_download_requires_successful_quit_after_data_eof() {
    ftp_download_requires_successful_quit(false).await;
}

#[tokio::test]
async fn configured_ftp_bulk_download_requires_successful_quit_after_data_eof() {
    ftp_download_requires_successful_quit(true).await;
}

async fn ftp_download_requires_successful_quit(bulk: bool) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,host,port,username) VALUES (44,'fixture','ftp',1,'/base','127.0.0.1',?,'fixture')")
        .bind(listener.local_addr().unwrap().port()).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        for quit in ["221 Bye", "500 Rejected"] {
            let passive = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut control = BufReader::new(stream);
            control.get_mut().write_all(b"220 Ready\r\n").await.unwrap();
            for (expected, reply) in [
                ("USER fixture", "230 Authorized".to_owned()),
                ("TYPE I", "200 Binary".to_owned()),
                (
                    "EPSV",
                    format!(
                        "229 Entering (|||{}|)",
                        passive.local_addr().unwrap().port()
                    ),
                ),
                ("RETR /base/archive.tar", "150 Opening".to_owned()),
            ] {
                let mut command = String::new();
                control.read_line(&mut command).await.unwrap();
                assert_eq!(command.trim_end(), expected);
                control
                    .get_mut()
                    .write_all(format!("{reply}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            let (mut data, _) = passive.accept().await.unwrap();
            data.write_all(b"archive bytes").await.unwrap();
            drop(data);
            control
                .get_mut()
                .write_all(b"226 Settled\r\n")
                .await
                .unwrap();
            let mut command = String::new();
            control.read_line(&mut command).await.unwrap();
            assert_eq!(command.trim_end(), "QUIT");
            control
                .get_mut()
                .write_all(format!("{quit}\r\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let headers = [("Cookie", cookie.as_str())];
    let url = "/api/v1/storage/44/files/download?path=archive.tar";
    if bulk {
        use async_zip::base::read1::seek::ZipArchiveReader;
        use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
        let route = "/api/v1/storage/44/files/bulk-download";
        let response = reqwest::Client::new()
            .post(format!("http://{}{route}", server.address))
            .header("Cookie", &cookie)
            .header("Content-Type", "application/json")
            .body(r#"{"paths":["archive.tar"]}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut archive =
            ZipArchiveReader::open(std::io::Cursor::new(response.bytes().await.unwrap()).compat())
                .await
                .unwrap();
        assert_eq!(archive.find(b"archive.tar").unwrap().count(), 1);
        let mut bytes = Vec::new();
        archive
            .file(0)
            .await
            .unwrap()
            .compat()
            .read_to_end(&mut bytes)
            .await
            .unwrap();
        assert_eq!(bytes, b"archive bytes");
        let (status, _, body) = server
            .request(
                "POST",
                route,
                &headers,
                Some(json!({"paths":["archive.tar"]})),
            )
            .await;
        assert_eq!(
            (status, body),
            (500, json!({"detail":"Bulk download failed"}))
        );
    } else {
        let (status, _, body) = server.request("GET", url, &headers, None).await;
        assert_eq!((status, body), (200, json!("archive bytes")));
        let (status, _, body) = server.request("GET", url, &headers, None).await;
        assert_eq!((status, body), (500, json!({"detail":"Download failed"})));
    }
    provider.await.unwrap();
    server.close().await;
}

#[tokio::test]
async fn configured_s3_download_verifies_complete_object_before_serving_ranges() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,s3_bucket,s3_endpoint_url,s3_access_key,s3_secret_key) VALUES(44,'fixture','s3',1,'base','fixture-bucket',?,'fixture-access','fixture-secret')")
        .bind(endpoint).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        for body in ["archive bytes", "incomplete"] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(
                line,
                "GET /fixture-bucket/base/archive.tar?x-id=GetObject HTTP/1.1\r\n"
            );
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                assert!(!line.to_ascii_lowercase().starts_with("range:"));
            }
            stream
                .get_mut()
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\n{body}"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let headers = [("Cookie", cookie.as_str()), ("Range", "bytes=8-12")];
    let url = "/api/v1/storage/44/files/download?path=archive.tar";
    let (status, response, body) = server.request("GET", url, &headers, None).await;
    assert_eq!((status, body), (206, json!("bytes")));
    assert!(response.contains("content-range: bytes 8-12/13"));
    let (status, _, body) = server.request("GET", url, &headers, None).await;
    assert_eq!((status, body), (500, json!({"detail":"Download failed"})));
    provider.await.unwrap();
    server.close().await;
}

#[tokio::test]
async fn backup_remote_download_uses_verified_enabled_catalog_copy_and_sidecar() {
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    sqlx::raw_sql(
        "INSERT INTO backup_targets(id,name,target_type) VALUES(1,'fixture','path');
        INSERT INTO backups(id,target_id,backup_type,status,encrypted,file_size) VALUES
        (1,1,'FULL','COMPLETED',1,13),(2,1,'FULL','COMPLETED',0,13),(3,1,'FULL','COMPLETED',1,13);",
    )
    .execute(server.db.pool())
    .await
    .unwrap();
    // Python hashes the plaintext archive before encryption; the downloaded object is ciphertext.
    sqlx::query("UPDATE backups SET checksum=? WHERE id IN (1,2)")
        .bind(
            Sha256::digest(b"expected data")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        )
        .execute(server.db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,webdav_url) VALUES(44,'fixture','webdav',1,'/base',?),(45,'disabled','webdav',0,'/base',?)")
        .bind(&endpoint).bind(&endpoint).execute(server.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO backup_storage_syncs(backup_id,storage_id,remote_path,sync_status) VALUES
        (1,44,'job/archive.tar.enc','completed'),(2,44,'job/wrong.tar','completed'),(3,45,'job/disabled.tar.enc','completed')")
        .execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        for (path, body) in [
            ("archive.tar.enc", "archive bytes"),
            ("archive.tar.enc", "archive bytes"),
            ("archive.tar.key", "wrapped key"),
            ("wrong.tar", "substitutions"),
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(line, format!("GET /base/job/{path} HTTP/1.1\r\n"));
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            stream
                .get_mut()
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        listener
    });
    for (route, status, expected) in [
        ("1/download", 200, json!("archive bytes")),
        ("1/download?sidecar=true", 200, json!("wrapped key")),
        (
            "2/download",
            404,
            json!({"detail":"Archive content unavailable"}),
        ),
        (
            "3/download",
            404,
            json!({"detail":"Archive content unavailable"}),
        ),
    ] {
        let (actual, _, body) = server
            .request("GET", &format!("/api/v1/backups/{route}"), &headers, None)
            .await;
        assert_eq!((actual, body), (status, expected));
    }
    let listener = provider.await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(server.path.join("backups"))
            .unwrap()
            .count(),
        0
    );
    server.close().await;
}

#[tokio::test]
async fn configured_webdav_download_does_not_publish_incomplete_bodies() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,webdav_url) VALUES(44,'fixture','webdav',1,'/base',?)")
        .bind(endpoint).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        for body in ["archive bytes", "incomplete"] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(line, "GET /base/archive.tar HTTP/1.1\r\n");
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            stream
                .get_mut()
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\n{body}"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let url = "/api/v1/storage/44/files/download?path=archive.tar";
    let (status, _, body) = server.request("GET", url, &headers, None).await;
    assert_eq!((status, body), (200, json!("archive bytes")));
    let (status, _, body) = server.request("GET", url, &headers, None).await;
    assert_eq!((status, body), (500, json!({"detail":"Download failed"})));
    provider.await.unwrap();
    server.close().await;
}

#[tokio::test]
async fn configured_local_download_only_uses_the_selected_root() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let stored = server.path.join("stored");
    std::fs::create_dir(&stored).unwrap();
    std::fs::write(stored.join("backup.tar"), b"configured bytes").unwrap();
    std::fs::write(server.path.join("backups/backup.tar"), b"default bytes").unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES(44,'fixture','local',0,?)")
        .bind(stored.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/44/files/download?path=backup.tar",
            &headers,
            None,
        )
        .await;
    assert_eq!((status, body), (200, json!("configured bytes")));
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/45/files/download?path=backup.tar",
            &headers,
            None,
        )
        .await;
    assert_eq!((status, body), (404, json!({"detail":"Storage not found"})));
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/not-an-id/files/download",
            &headers,
            None,
        )
        .await;
    assert_eq!(
        (status, body),
        (
            422,
            json!({"detail":"Validation error","errors":[{"field":"path.storage_id","message":"Input should be a valid integer, unable to parse string as an integer"},{"field":"query.path","message":"Field required"}]})
        )
    );
    server.close().await;
}

#[tokio::test]
async fn configured_storage_browsing_uses_only_the_persisted_root() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let configured = server.path.join("configured");
    std::fs::create_dir_all(configured.join("child")).unwrap();
    std::fs::write(configured.join("child/from-saved"), b"fixture").unwrap();
    std::fs::write(
        server.path.join("backups/from-default"),
        b"not the configured root",
    )
    .unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES(44,'Saved','local',0,?),(45,'Remote','webdav',1,'/')")
        .bind(configured.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    let (status, _, result) = server
        .request("GET", "/api/v1/storage/44/files?path=child", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(result["path"], "child");
    assert_eq!(result["files"].as_array().unwrap().len(), 1);
    assert_eq!(result["files"][0]["name"], "from-saved");
    for (id, status, body) in [
        (
            44,
            200,
            json!({"path":"child","state":"completed","bytes":7}),
        ),
        (
            45,
            200,
            json!({"path":"child","state":"unavailable","bytes":null}),
        ),
        (46, 404, json!({"detail":"Storage not found"})),
    ] {
        let (actual, _, result) = server
            .request(
                "GET",
                &format!("/api/v1/storage/{id}/files/size?path=child"),
                &headers,
                None,
            )
            .await;
        assert_eq!((actual, result), (status, body));
    }
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/not-an-id/files/size",
            &headers,
            None,
        )
        .await;
    assert_eq!(
        (status, body),
        (
            422,
            json!({"detail":"Validation error","errors":[{"field":"path.storage_id","message":"Input should be a valid integer, unable to parse string as an integer"},{"field":"query.path","message":"Field required"}]})
        )
    );
    server.close().await;
}

#[tokio::test]
async fn local_directory_size_lists_only_regular_bytes_and_reflects_nested_writes() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let root = server.path.join("backups");
    let nested = root.join("scan/nested");
    std::fs::create_dir_all(&nested).unwrap();
    let file = nested.join("item");
    std::fs::write(&file, b"123").unwrap();
    std::os::unix::fs::symlink(&file, nested.join("link")).unwrap();
    for (path, length) in [
        (".%2Fscan%2F", Some(3)),
        ("missing", None),
        ("%2E%2E", None),
    ] {
        let (status, _, body) = server
            .request(
                "GET",
                &format!("/api/v1/storage/local/files/size?path={path}"),
                &headers,
                None,
            )
            .await;
        let display_path = match path {
            ".%2Fscan%2F" => "scan",
            "%2E%2E" => "..",
            _ => path,
        };
        assert_eq!(
            (status, body),
            (
                200,
                json!({"path":display_path,"bytes":length,"state":if length.is_some(){"completed"}else{"unavailable"}})
            )
        );
    }
    std::fs::write(file, b"12345678").unwrap();
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/local/files/size?path=scan",
            &headers,
            None,
        )
        .await;
    assert_eq!(
        (status, body),
        (200, json!({"path":"scan","bytes":8,"state":"completed"}))
    );
    let (status, _, _) = server
        .request("GET", "/api/v1/storage/local/files/size", &headers, None)
        .await;
    assert_eq!(status, 422);
    server.close().await;
}

#[tokio::test]
async fn local_listing_reports_entries_without_following_symlinks() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let root = server.path.join("backups");
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let archive = nested.join("archive.tar");
    std::fs::write(&archive, b"bytes").unwrap();
    let target = server.path.join("outside");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, nested.join("link")).unwrap();
    let headers = [("Cookie", cookie.as_str())];
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/local/files?path=.%2Fnested%2F",
            &headers,
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["path"], "nested");
    let files = body["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    let archive = files.iter().find(|f| f["name"] == "archive.tar").unwrap();
    assert_eq!(archive["size"], 5);
    assert_eq!(archive["is_dir"], false);
    assert_eq!(archive["is_symlink"], false);
    assert!(archive["modified"].is_number());
    let link = files.iter().find(|f| f["name"] == "link").unwrap();
    assert_eq!(link["is_symlink"], true);
    assert_eq!(link["is_dir"], false);
    assert_eq!(link["size"], Value::Null);
    for (path, status, detail) in [
        ("nested%2Flink", 400, "Invalid file path"),
        ("nested%2Farchive.tar", 400, "Invalid file path"),
        ("nested%2F..", 400, "Invalid file path"),
        ("missing", 404, "Directory not found"),
    ] {
        let (actual, _, body) = server
            .request(
                "GET",
                &format!("/api/v1/storage/local/files?path={path}"),
                &headers,
                None,
            )
            .await;
        assert_eq!((actual, body), (status, json!({"detail":detail})));
    }
    assert_eq!(std::fs::read(nested.join("archive.tar")).unwrap(), b"bytes");
    server.close().await;
}

#[tokio::test]
async fn local_storage_drafts_probe_without_persisting_configuration() {
    const URL: &str = "/api/v1/storage/test-config";
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let stored = server.path.join("backups");
    let target = server.path.join("draft/nested");
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES (44,'Stored','local',1,?)")
        .bind(stored.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    let before: String = sqlx::query_scalar(
        "SELECT json_array(id,name,enabled,base_path,password,updated_at) FROM remote_storages",
    )
    .fetch_one(server.db.pool())
    .await
    .unwrap();

    for body in [
        json!({"mode":"create","config":{"name":"Draft","storage_type":"local","base_path":target,"password":"not-persisted"}}),
        json!({"mode":"edit","storage_id":44,"config":{"name":"Edited draft","base_path":target,"enabled":false}}),
    ] {
        let (status, _, body) = server.request("POST", URL, &headers, Some(body)).await;
        assert_eq!(
            (status, body),
            (
                200,
                json!({"success":true,"message":"Connection test succeeded"})
            )
        );
    }
    for body in [
        json!({"mode":"private-fixture-value","config":{}}),
        json!({"mode":"create","storage_id":44,"config":{"name":"Draft","storage_type":"local"}}),
        json!({"mode":"edit","storage_id":44,"config":{"storage_type":"s3"}}),
        json!({"mode":"edit","storage_id":0,"config":{}}),
        json!({"mode":"edit","storage_id":44,"config":{"port":65536}}),
    ] {
        let (status, _, body) = server.request("POST", URL, &headers, Some(body)).await;
        assert_eq!(
            (status, body),
            (422, json!({"detail":"Invalid storage draft configuration"}))
        );
    }
    let (status, _, body) = server
        .request(
            "POST",
            URL,
            &headers,
            Some(json!({"mode":"edit","storage_id":999,"config":{}})),
        )
        .await;
    assert_eq!((status, body), (404, json!({"detail":"Storage not found"})));
    let (status, _, body) = server
        .request(
            "POST",
            URL,
            &headers,
            Some(json!({"mode":"edit","storage_id":44,"config":{"base_path":null}})),
        )
        .await;
    assert_eq!(
        (status, body),
        (
            400,
            json!({"detail":{"message":"Invalid storage configuration","errors":["base_path is required for StorageType.LOCAL; edit this storage to repair it"]}})
        )
    );
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&stored).unwrap().count(), 0);
    let after: Vec<String> = sqlx::query_scalar(
        "SELECT json_array(id,name,enabled,base_path,password,updated_at) FROM remote_storages",
    )
    .fetch_all(server.db.pool())
    .await
    .unwrap();
    assert_eq!(after, [before]);
    server.close().await;
}

#[tokio::test]
async fn webdav_draft_credentials_are_applied_only_to_the_probe() {
    use dockervault_core::crypto::CredentialCipher;
    use tokio::io::{AsyncBufReadExt, BufReader};
    let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let config = Config::from_pairs([
        ("COOKIE_SECURE", "false"),
        ("CORS_ORIGINS", "http://localhost:3000"),
        ("CREDENTIAL_ENCRYPTION_KEY", key),
    ])
    .unwrap();
    let server = Server::configured(config).await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/dav", listener.local_addr().unwrap());
    let stored = CredentialCipher::new(key).unwrap().encrypt_value("saved");
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,webdav_url,username,password) VALUES (44,'fixture','webdav',1,'base path',?,'user',?)")
        .bind(&url).bind(&stored).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        for (path, auth) in [
            ("/dav/draft%20path", Some("Basic dXNlcjpmZXJuZXQ6ZHJhZnQ=")),
            ("/dav/base%20path", Some("Basic dXNlcjpzYXZlZA==")),
            ("/dav/base%20path", Some("Basic dXNlcjpzYXZlZA==")),
            ("/dav/base%20path", None),
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(line.trim_end(), format!("PROPFIND {path} HTTP/1.1"));
            let mut authorization = None;
            let mut depth = None;
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.trim_end().split_once(':') {
                    if name.eq_ignore_ascii_case("authorization") {
                        authorization = Some(value.trim().to_owned());
                    } else if name.eq_ignore_ascii_case("depth") {
                        depth = Some(value.trim().to_owned());
                    }
                }
            }
            assert!(authorization.as_deref() == auth);
            assert_eq!(depth.as_deref(), Some("0"));
            let status = if auth.is_some() {
                "207 Multi-Status"
            } else {
                "403 Forbidden"
            };
            stream
                .get_mut()
                .write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    for (body, success) in [
        (
            json!({"mode":"create","config":{"name":"Draft","storage_type":"webdav","webdav_url":url,"base_path":"draft path","username":"user","password":"fernet:draft"}}),
            true,
        ),
        (json!({"mode":"edit","storage_id":44,"config":{}}), true),
        (
            json!({"mode":"edit","storage_id":44,"config":{"password":""}}),
            true,
        ),
        (
            json!({"mode":"edit","storage_id":44,"config":{"password":null}}),
            false,
        ),
    ] {
        let (status, _, result) = server
            .request("POST", "/api/v1/storage/test-config", &headers, Some(body))
            .await;
        assert_eq!(
            (status, result),
            (
                200,
                json!({"success":success,"message":if success { "Connection test succeeded" } else { "Connection test failed" }})
            )
        );
    }
    provider.await.unwrap();
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT base_path,password FROM remote_storages")
            .fetch_all(server.db.pool())
            .await
            .unwrap();
    assert!(rows == [("base path".to_owned(), stored)]);
    server.close().await;
}

#[tokio::test]
async fn ssh_draft_rejects_a_key_outside_the_configured_roots_before_connecting() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let key = server.path.join("outside.key");
    std::fs::write(&key, b"synthetic fixture outside allowed directories").unwrap();
    let payload = json!({"mode":"create","config":{"name":"Draft","storage_type":"sftp","host":"127.0.0.1","port":listener.local_addr().unwrap().port(),"username":"fixture","ssh_key_path":key}});
    let (status, _, body) = server
        .request(
            "POST",
            "/api/v1/storage/test-config",
            &headers,
            Some(payload),
        )
        .await;
    assert_eq!(status, 400);
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .starts_with("SSH key path must be within allowed directories:")
    );
    assert!(!body.to_string().contains("outside.key"));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    server.close().await;
}

#[tokio::test]
async fn s3_probe_uses_effective_signed_configuration_without_saving_a_draft() {
    use dockervault_core::crypto::CredentialCipher;
    use tokio::io::{AsyncBufReadExt, BufReader};
    let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let config = Config::from_pairs([
        ("COOKIE_SECURE", "false"),
        ("CORS_ORIGINS", "http://localhost:3000"),
        ("CREDENTIAL_ENCRYPTION_KEY", key),
    ])
    .unwrap();
    let server = Server::configured(config).await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let cipher = CredentialCipher::new(key).unwrap();
    let access = cipher.encrypt_value("FIXTURE_SAVED_ACCESS");
    let secret = cipher.encrypt_value("fixture-saved-secret");
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,s3_bucket,s3_endpoint_url,s3_access_key,s3_secret_key) VALUES (44,'S3','s3',1,'prefix','fixture-bucket',?,?,?)")
        .bind(endpoint).bind(&access).bind(&secret).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        for (id, bucket, status) in [
            ("FIXTURE_DRAFT_ACCESS", "draft-bucket", "200 OK"),
            ("FIXTURE_SAVED_ACCESS", "fixture-bucket", "403 Forbidden"),
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(line.trim_end(), format!("HEAD /{bucket}/ HTTP/1.1"));
            let mut seen = false;
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("authorization")
                {
                    assert!(value.contains(&format!("Credential={id}/")));
                    assert!(value.contains("/us-east-1/s3/aws4_request"));
                    seen = true;
                }
            }
            assert!(seen);
            stream
                .get_mut()
                .write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let cleared = json!({"mode":"edit","storage_id":44,"config":{"s3_secret_key":null}});
    let (status, _, body) = server
        .request(
            "POST",
            "/api/v1/storage/test-config",
            &headers,
            Some(cleared),
        )
        .await;
    assert_eq!(
        (status, body),
        (
            400,
            json!({"detail":{"message":"Invalid storage configuration","errors":["s3_secret_key is required for StorageType.S3; edit this storage to repair it"]}})
        )
    );
    let draft = json!({"mode":"edit","storage_id":44,"config":{"s3_bucket":"draft-bucket","s3_access_key":"FIXTURE_DRAFT_ACCESS","s3_secret_key":""}});
    let (status, _, body) = server
        .request("POST", "/api/v1/storage/test-config", &headers, Some(draft))
        .await;
    assert_eq!(
        (status, body),
        (
            200,
            json!({"success":true,"message":"Connection test succeeded"})
        )
    );
    let (status, _, body) = server
        .request("POST", "/api/v1/storage/44/test", &headers, None)
        .await;
    assert_eq!(
        (status, body),
        (
            200,
            json!({"success":false,"message":"Connection test failed"})
        )
    );
    provider.await.unwrap();
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT s3_bucket,s3_access_key,s3_secret_key FROM remote_storages")
            .fetch_all(server.db.pool())
            .await
            .unwrap();
    assert!(rows == [("fixture-bucket".to_owned(), access, secret)]);
    server.close().await;
}

#[tokio::test]
async fn ftp_probe_reports_only_the_settled_login_and_quit_result() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,host,port,base_path,username,password) VALUES (44,'FTP','ftp',1,'127.0.0.1',?,'/fixture','fixture-user','synthetic-saved')")
        .bind(port).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        for (password, quit) in [
            ("synthetic-draft", "221 goodbye"),
            ("synthetic-saved", "500 rejected"),
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            stream.get_mut().write_all(b"220 hello\r\n").await.unwrap();
            for (command, reply) in [
                ("USER fixture-user".to_owned(), "331 need password"),
                (format!("PASS {password}"), "230 logged in"),
                ("QUIT".to_owned(), quit),
            ] {
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                assert!(line.trim_end() == command);
                stream
                    .get_mut()
                    .write_all(format!("{reply}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            let mut end = String::new();
            assert_eq!(stream.read_line(&mut end).await.unwrap(), 0);
        }
    });
    let input = json!({"mode":"edit","storage_id":44,"config":{"password":"synthetic-draft"}});
    let (status, _, result) = server
        .request("POST", "/api/v1/storage/test-config", &headers, Some(input))
        .await;
    assert_eq!(
        (status, result),
        (
            200,
            json!({"success":true,"message":"Connection test succeeded"})
        )
    );
    let (status, _, result) = server
        .request("POST", "/api/v1/storage/44/test", &headers, None)
        .await;
    assert_eq!(
        (status, result),
        (
            200,
            json!({"success":false,"message":"Connection test failed"})
        )
    );
    provider.await.unwrap();
    let credentials: Vec<String> = sqlx::query_scalar("SELECT password FROM remote_storages")
        .fetch_all(server.db.pool())
        .await
        .unwrap();
    assert!(credentials == ["synthetic-saved"]);
    server.close().await;
}

#[tokio::test]
async fn saved_webdav_probe_waits_for_the_remote_propfind_result() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/dav", listener.local_addr().unwrap());
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path,webdav_url) VALUES (44,'fixture','webdav',1,'backups',?)")
        .bind(url).execute(server.db.pool()).await.unwrap();
    let provider = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        for status in [
            b"HTTP/1.1 207 Multi-Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .as_slice(),
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice(),
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert!(line.starts_with("PROPFIND /dav/backups"));
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            stream.get_mut().write_all(status).await.unwrap();
        }
    });
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    for success in [true, false] {
        let (status, _, result) = server
            .request("POST", "/api/v1/storage/44/test", &headers, None)
            .await;
        assert_eq!(
            (status, result),
            (
                200,
                json!({"success":success,"message":if success {"Connection test succeeded"} else {"Connection test failed"}})
            )
        );
    }
    provider.await.unwrap();
    server.close().await;
}

#[tokio::test]
async fn saved_local_storage_probe_creates_only_the_configured_directory() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let target = server.path.join("missing/nested");
    let outside = server.path.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = server.path.join("link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    for (id, path) in [
        (44, target.clone()),
        (45, link.join("untouched")),
        (46, server.path.join("unchecked/../unexpected")),
    ] {
        sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES(?,?,'local',1,?)")
            .bind(id).bind(format!("local fixture {id}")).bind(path.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    }
    assert!(!target.exists());
    for (id, success) in [(44, true), (44, true), (45, false), (46, false)] {
        let (status, _, result) = server
            .request(
                "POST",
                &format!("/api/v1/storage/{id}/test"),
                &headers,
                None,
            )
            .await;
        assert_eq!(
            (status, result),
            (
                200,
                json!({"success":success,"message":if success {"Connection test succeeded"} else {"Connection test failed"}})
            )
        );
    }
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert!(!server.path.join("unchecked").exists());
    assert!(!server.path.join("unexpected").exists());
    server.close().await;
}

#[tokio::test]
async fn saved_local_storage_probe_checks_writability_without_leaving_a_file() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let target = server.path.join("backups");
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES(44,'local fixture','local',1,?)")
        .bind(target.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    let (status, _, result) = server
        .request("POST", "/api/v1/storage/44/test", &headers, None)
        .await;
    assert_eq!(
        (status, result),
        (
            200,
            json!({"success":true,"message":"Connection test succeeded"})
        )
    );
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    let (status, _, result) = server
        .request("POST", "/api/v1/storage/99/test", &headers, None)
        .await;
    assert_eq!(
        (status, result),
        (404, json!({"detail":"Storage not found"}))
    );
    server.close().await;
}

#[tokio::test]
async fn storage_configuration_delete_preserves_catalog_references_and_archive_bytes() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let archive = server.path.join("backups/fixture.tar");
    std::fs::write(&archive, b"retain these bytes").unwrap();
    sqlx::raw_sql("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES (44,'referenced','local',1,'/backups'),(45,'unused','local',1,'/backups');
        INSERT INTO backup_targets(id,name,target_type,enabled,sync_to_remote) VALUES (41,'fixture target','path',0,0);
        INSERT INTO backups(id,target_id,backup_type,status) VALUES (51,41,'FULL','COMPLETED');
        INSERT INTO backup_storage_syncs(id,backup_id,storage_id,remote_path,sync_status) VALUES (61,51,44,'fixture.tar','completed');").execute(server.db.pool()).await.unwrap();
    let (status, _, referenced) = server
        .request("DELETE", "/api/v1/storage/44.0", &headers, None)
        .await;
    assert_eq!(
        (status, referenced),
        (
            409,
            json!({"detail":"Remove backup sync records before deleting their storage configuration"})
        )
    );
    let (status, _, deleted) = server
        .request("DELETE", "/api/v1/storage/45", &headers, None)
        .await;
    assert_eq!(
        (status, deleted),
        (200, json!({"message":"Storage deleted"}))
    );
    let (status, _, missing) = server
        .request("DELETE", "/api/v1/storage/45", &headers, None)
        .await;
    assert_eq!(
        (status, missing),
        (404, json!({"detail":"Storage not found"}))
    );
    let linked: Vec<(i64, i64)> = sqlx::query_as("SELECT remote_storages.id, backup_storage_syncs.backup_id FROM remote_storages JOIN backup_storage_syncs ON storage_id=remote_storages.id").fetch_all(server.db.pool()).await.unwrap();
    assert_eq!(linked, vec![(44, 51)]);
    assert_eq!(std::fs::read(archive).unwrap(), b"retain these bytes");
    server.close().await;
}

#[tokio::test]
async fn operation_detail_requires_authentication_and_ownership() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    sqlx::raw_sql("INSERT INTO users(id,username,password_hash) VALUES(200,'other','synthetic');
        INSERT INTO storage_operations(id,user_id,request_id,payload,storage_identity,action,storage_id,path,state,stage,total,completed,succeeded,failed,skipped,unknown,cancel_requested,created_at,updated_at) VALUES
        ('mine',1,'r1','{}','{}','storage_delete','local','','queued','queued',1,0,0,0,0,0,0,'2024-01-01 00:00:00.120','2024-01-01 00:00:00.000000'),
        ('theirs',200,'r2','{}','{}','storage_delete','local','','queued','queued',1,0,0,0,0,0,0,'2024-01-01 00:00:00','2024-01-01 00:00:00');
        INSERT INTO storage_operation_items(operation_id,path,state) VALUES ('mine','fixture.tar','pending'),('theirs','other.tar','pending');")
        .execute(server.db.pool()).await.unwrap();
    let (status, _, listing) = server
        .request(
            "GET",
            "/api/v1/operations?storage_id=local&limit=1",
            &[("Cookie", &cookie)],
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(listing.as_array().unwrap().len(), 1);
    assert_eq!(listing[0]["id"], "mine");
    assert_eq!(listing[0]["created_at"], "2024-01-01T00:00:00.120000Z");
    assert_eq!(listing[0]["updated_at"], "2024-01-01T00:00:00Z");
    assert_eq!(listing[0]["finished_at"], Value::Null);
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/operations?limit=101",
            &[("Cookie", &cookie)],
            None,
        )
        .await;
    assert_eq!(status, 422);
    assert_eq!(
        body,
        json!({"detail":"Validation error","errors":[{"field":"query.limit","message":"Input should be less than or equal to 100"}]})
    );
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/operations?limit=101&limit=1",
            &[("Cookie", &cookie)],
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body.as_array().unwrap().len(), 1);
    for (query, expected) in [
        ("limit=1.0", listing.clone()),
        ("limit=%201%20", listing.clone()),
        ("storage_id=1.0", json!([])),
    ] {
        let (status, _, body) = server
            .request(
                "GET",
                &format!("/api/v1/operations?{query}"),
                &[("Cookie", &cookie)],
                None,
            )
            .await;
        assert_eq!(status, 200, "{query}");
        assert_eq!(body, expected, "{query}");
    }
    for value in ["1.1", "1e0", "1.00000000000000001"] {
        let (status, _, body) = server
            .request(
                "GET",
                &format!("/api/v1/operations?limit={value}"),
                &[("Cookie", &cookie)],
                None,
            )
            .await;
        assert_eq!(status, 422, "{value}");
        assert_eq!(
            body,
            json!({"detail":"Validation error","errors":[{"field":"query.limit","message":"Input should be a valid integer, unable to parse string as an integer"}]})
        );
    }
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/operations?storage_id=0.0",
            &[("Cookie", &cookie)],
            None,
        )
        .await;
    assert_eq!(status, 422);
    assert_eq!(
        body,
        json!({"detail":"Validation error","errors":[{"field":"query.storage_id.literal['local']","message":"Input should be 'local'"},{"field":"query.storage_id.constrained-int","message":"Input should be greater than 0"}]})
    );
    let (status, _, _) = server
        .request("GET", "/api/v1/operations/mine", &[], None)
        .await;
    assert_eq!(status, 401);
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/operations/theirs",
            &[("Cookie", &cookie)],
            None,
        )
        .await;
    assert_eq!(
        (status, body),
        (404, json!({"detail":"Operation not found"}))
    );
    let (status, _, detail) = server
        .request(
            "GET",
            "/api/v1/operations/mine",
            &[("Cookie", &cookie)],
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        detail["items"],
        json!([{"path":"fixture.tar","state":"pending","error":null}])
    );
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let (status, _, body) = server
        .request("POST", "/api/v1/operations/theirs/cancel", &headers, None)
        .await;
    assert_eq!(
        (status, body),
        (404, json!({"detail":"Operation not found"}))
    );
    let (status, _, body) = server
        .request("POST", "/api/v1/operations/mine/cancel", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["cancel_requested"], true);
    assert_eq!(body["items"][0]["state"], "skipped");
    server.close().await;
}

#[tokio::test]
async fn setup_http_honors_shared_claim_repairs_abandoned_generation_and_fences_invalid_journal() {
    use dockervault_service::recovery_activation::{ClaimKind, PreparationClaims};
    let server = Server::start("http://localhost").await;
    let body = json!({"username":"public-admin","password":"PUBLIC-password!","confirm_password":"PUBLIC-password!"});
    let claims = PreparationClaims::new(Some(server.path.join("db.sqlite")));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let blocked = server
        .request("POST", "/api/v1/auth/setup", &[], Some(body.clone()))
        .await;
    assert_eq!(blocked.0, 400);
    assert_eq!(blocked.2, json!({"detail":"Setup is already in progress"}));
    assert!(!blocked.1.contains("set-cookie:"));
    drop(guard);
    let marker = r#"{"state":"preparing","generation":"../escape"}"#;
    sqlx::query("INSERT INTO app_settings(key,value) VALUES('recovery_activation',?)")
        .bind(marker)
        .execute(server.db.pool())
        .await
        .unwrap();
    let invalid = server
        .request("POST", "/api/v1/auth/setup", &[], Some(body.clone()))
        .await;
    assert_eq!(invalid.0, 400);
    assert_eq!(
        invalid.2,
        json!({"detail":"Recovery preparation journal needs repair"})
    );
    assert!(!invalid.1.contains("set-cookie:"));
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(server.db.pool())
        .await
        .unwrap();
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(server.db.pool())
        .await
        .unwrap();
    assert_eq!((users, sessions), (0, 0));
    let persisted: String =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(server.db.pool())
            .await
            .unwrap();
    assert_eq!(persisted, marker);
    let generation = "0123456789abcdef0123456789abcdef";
    let directory = server.path.join(".recovery").join(generation);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("rclone.conf"), b"PUBLIC abandoned config").unwrap();
    sqlx::query("UPDATE app_settings SET value=? WHERE key='recovery_activation'")
        .bind(json!({"state":"preparing","generation":generation}).to_string())
        .execute(server.db.pool())
        .await
        .unwrap();
    let success = server
        .request("POST", "/api/v1/auth/setup", &[], Some(body))
        .await;
    assert_eq!(success.0, 200);
    assert!(success.1.contains("set-cookie:"));
    assert!(!directory.exists());
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app_settings WHERE key='recovery_activation'")
            .fetch_one(server.db.pool())
            .await
            .unwrap();
    assert_eq!(rows, 0);
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(server.db.pool())
        .await
        .unwrap();
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(server.db.pool())
        .await
        .unwrap();
    assert_eq!((users, sessions), (1, 1));
    server.close().await;
}

#[tokio::test]
async fn real_http_setup_status_cookie_attributes_and_protected_router_match_the_control() {
    let server = Server::start("http://localhost").await;
    let before = server.request("GET", "/api/v1/targets", &[], None).await;
    assert_eq!(before.0, 403);
    assert_eq!(
        before.2,
        json!({"detail":"Setup required","setup_required":true})
    );
    let status = server
        .request("GET", "/api/v1/auth/status", &[], None)
        .await;
    assert_eq!(
        status.2,
        json!({"authenticated":false,"setup_complete":false,"user":null})
    );
    let cookie = server.setup().await;
    let status = server
        .request("GET", "/api/v1/auth/status", &[("Cookie", &cookie)], None)
        .await;
    assert_eq!(status.0, 200);
    assert_eq!(status.2["authenticated"], true);
    let unauthorized = server.request("GET", "/api/v1/targets", &[], None).await;
    assert_eq!(unauthorized.0, 401);
    assert_eq!(unauthorized.2["detail"], "Not authenticated");
    let login = server
        .request(
            "POST",
            "/api/v1/auth/login",
            &[],
            Some(json!({"username":"public-admin","password":"PUBLIC-password!"})),
        )
        .await;
    assert_eq!(login.0, 200);
    for attribute in ["HttpOnly", "Max-Age=604800", "Path=/", "SameSite=lax"] {
        assert!(login.1.contains(attribute));
    }
    assert!(!login.1.contains("; Secure"));
    assert_eq!(login.2["message"], "Login successful");
    assert!(
        login.2["user"]["last_login"]
            .as_str()
            .unwrap()
            .ends_with("+00:00")
    );
    server.close().await;
}
#[tokio::test]
async fn conflict_credential_precedence_and_bearer_only_logout_are_not_silently_changed() {
    let server = Server::start("http://localhost").await;
    let cookie = server.setup().await;
    let raw = cookie.strip_prefix("session_token=").unwrap();
    let bearer = format!("Bearer {raw}");
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Authorization", "Bearer invalid-public-token"),
    ];
    assert_eq!(
        server
            .request("GET", "/api/v1/auth/me", &headers, None)
            .await
            .0,
        401
    );
    assert_eq!(
        server
            .request("GET", "/api/v1/auth/status", &headers, None)
            .await
            .2["authenticated"],
        true
    );
    assert_eq!(
        server
            .request("GET", "/api/v1/unmapped-protected-contract", &headers, None)
            .await
            .0,
        404
    );
    let bad_cookie = [
        ("Cookie", "session_token=invalid-public-token"),
        ("Authorization", bearer.as_str()),
    ];
    assert_eq!(
        server
            .request("GET", "/api/v1/auth/me", &bad_cookie, None)
            .await
            .0,
        401
    );
    assert_eq!(
        server
            .request(
                "POST",
                "/api/v1/auth/logout",
                &[("Authorization", &bearer)],
                None
            )
            .await
            .0,
        200
    );
    assert_eq!(
        server
            .request(
                "GET",
                "/api/v1/auth/me",
                &[("Authorization", &bearer)],
                None
            )
            .await
            .0,
        200
    );
    let logout = server
        .request("POST", "/api/v1/auth/logout", &[("Cookie", &cookie)], None)
        .await;
    assert_eq!(logout.0, 200);
    assert!(logout.1.contains("Max-Age=0"));
    assert!(logout.1.contains("SameSite=lax"));
    assert_eq!(
        server
            .request(
                "GET",
                "/api/v1/auth/me",
                &[("Authorization", &bearer)],
                None
            )
            .await
            .0,
        401
    );
    server.close().await;
}
#[tokio::test]
async fn untrusted_and_null_origins_cannot_create_an_admin_even_with_wildcard_cors() {
    let server = Server::start("*").await;
    for origin in [
        "https://malicious.invalid",
        "null",
        "http://localhost@malicious.invalid",
        "http://localhost:invalid",
    ] {
        let result=server.request("POST","/api/v1/auth/setup",&[("Origin",origin)],Some(json!({"username":"public-admin","password":"PUBLIC-password!","confirm_password":"PUBLIC-password!"}))).await;
        assert_eq!(result.0, 403);
        assert_eq!(result.2["detail"], "Origin not allowed");
    }
    let preflight = server
        .request(
            "OPTIONS",
            "/api/v1/auth/setup",
            &[
                ("Origin", "https://malicious.invalid"),
                ("Access-Control-Request-Method", "POST"),
            ],
            None,
        )
        .await;
    assert_eq!(preflight.0, 400);
    assert!(!server.auth.setup_complete().await.unwrap());
    let origin = format!("http://{}", server.address);
    assert_eq!(server.request("POST","/api/v1/auth/setup",&[("Origin",&origin)],Some(json!({"username":"public-admin","password":"PUBLIC-password!","confirm_password":"PUBLIC-password!"}))).await.0,200);
    server.close().await;
}
#[tokio::test]
async fn trusted_cors_and_no_origin_programmatic_clients_keep_their_supported_paths() {
    let server = Server::start("https://trusted.invalid").await;
    let preflight = server
        .request(
            "OPTIONS",
            "/api/v1/auth/setup",
            &[
                ("Origin", "https://trusted.invalid"),
                ("Access-Control-Request-Method", "POST"),
                ("Access-Control-Request-Headers", "content-type"),
            ],
            None,
        )
        .await;
    assert_eq!(preflight.0, 200);
    assert!(
        preflight
            .1
            .contains("access-control-allow-origin: https://trusted.invalid")
    );
    assert!(
        preflight
            .1
            .contains("access-control-allow-credentials: true")
    );
    let setup=server.request("POST","/api/v1/auth/setup",&[("Origin","https://trusted.invalid"),("Sec-Fetch-Site","cross-site")],Some(json!({"username":"public-admin","password":"PUBLIC-password!","confirm_password":"PUBLIC-password!"}))).await;
    assert_eq!(setup.0, 200);
    assert_eq!(
        server
            .request(
                "POST",
                "/api/v1/auth/login",
                &[],
                Some(json!({"username":"public-admin","password":"PUBLIC-password!"}))
            )
            .await
            .0,
        200
    );
    server.close().await;
}
#[tokio::test]
async fn standard_json_media_type_case_and_quoted_spaced_cookie_are_supported() {
    let server = Server::start("http://localhost").await;
    let cookie = server.setup().await;
    let raw = cookie.strip_prefix("session_token=").unwrap();
    let quoted = format!("session_token = \"{raw}\"");
    let status = server
        .request("GET", "/api/v1/auth/status", &[("Cookie", &quoted)], None)
        .await;
    assert_eq!(status.2["authenticated"], true);
    let login = server
        .request(
            "POST",
            "/api/v1/auth/login",
            &[("Content-Type", "Application/Json; Charset=UTF-8")],
            Some(json!({"username":"public-admin","password":"PUBLIC-password!"})),
        )
        .await;
    assert_eq!(login.0, 200);
    assert_eq!(
        server
            .request(
                "POST",
                "/api/v1/auth/login",
                &[("Content-Type", "text/plain")],
                Some(json!({"username":"public-admin","password":"PUBLIC-password!"}))
            )
            .await
            .0,
        422
    );
    server.close().await;
}

#[tokio::test]
async fn malicious_origin_cannot_logout_or_change_an_existing_users_password() {
    let server = Server::start("*").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "https://malicious.invalid"),
    ];
    assert_eq!(
        server
            .request("POST", "/api/v1/auth/logout", &headers, None)
            .await
            .0,
        403
    );
    assert_eq!(server.request("POST","/api/v1/auth/change-password",&headers,Some(json!({"current_password":"PUBLIC-password!","new_password":"PUBLIC-new-password!","confirm_password":"PUBLIC-new-password!"}))).await.0,403);
    assert_eq!(
        server
            .request(
                "POST",
                "/api/v1/auth/logout",
                &[("Cookie", &cookie), ("Sec-Fetch-Site", "cross-site")],
                None
            )
            .await
            .0,
        403
    );
    assert_eq!(
        server
            .request("GET", "/api/v1/auth/me", &[("Cookie", &cookie)], None)
            .await
            .0,
        200
    );
    assert_eq!(
        server
            .request(
                "POST",
                "/api/v1/auth/login",
                &[],
                Some(json!({"username":"public-admin","password":"PUBLIC-password!"}))
            )
            .await
            .0,
        200
    );
    server.close().await;
}

#[test]
fn only_the_actual_loopback_proxy_peer_can_supply_rate_limit_identity() {
    use axum::http::{HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();
    headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.20"));
    headers.insert(
        "x-forwarded-for",
        HeaderValue::from_static("198.51.100.21, 198.51.100.22"),
    );
    assert_eq!(
        http::security::client_ip(&headers, Some("203.0.113.10:8000".parse().unwrap())),
        "203.0.113.10".parse::<std::net::IpAddr>().unwrap()
    );
    assert_eq!(
        http::security::client_ip(&headers, Some("127.0.0.1:8000".parse().unwrap())),
        "198.51.100.20".parse::<std::net::IpAddr>().unwrap()
    );
}

#[tokio::test]
async fn is_admin_remains_data_not_a_new_authorization_role() {
    let server = Server::start("http://localhost").await;
    let cookie = server.setup().await;
    sqlx::query("UPDATE users SET is_admin=0")
        .execute(server.db.pool())
        .await
        .unwrap();
    let me = server
        .request("GET", "/api/v1/auth/me", &[("Cookie", &cookie)], None)
        .await;
    assert_eq!(me.0, 200);
    assert_eq!(me.2["is_admin"], false);
    assert_eq!(
        server
            .request(
                "GET",
                "/api/v1/unmapped-protected-contract",
                &[("Cookie", &cookie)],
                None
            )
            .await
            .0,
        404
    );
    server.close().await;
}

#[tokio::test]
async fn a_body_without_content_type_cannot_bootstrap_an_account() {
    let server = Server::start("http://localhost").await;
    let body=json!({"username":"public-admin","password":"PUBLIC-password!","confirm_password":"PUBLIC-password!"}).to_string();
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    stream.write_all(format!("POST /api/v1/auth/setup HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",server.address,body.len()).as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    assert!(
        String::from_utf8(response)
            .unwrap()
            .starts_with("HTTP/1.1 422")
    );
    assert!(!server.auth.setup_complete().await.unwrap());
    server.close().await;
}

#[tokio::test]
async fn enabled_docs_do_not_bypass_the_existing_global_auth_guard() {
    let server = Server::configured(
        Config::from_pairs([("COOKIE_SECURE", "false"), ("ENABLE_DOCS", "true")]).unwrap(),
    )
    .await;
    for path in ["/docs", "/redoc", "/openapi.json"] {
        assert_eq!(server.request("GET", path, &[], None).await.0, 403);
    }
    let _ = server.setup().await;
    for path in ["/docs", "/redoc", "/openapi.json"] {
        assert_eq!(server.request("GET", path, &[], None).await.0, 401);
    }
    server.close().await;
}

#[tokio::test]
async fn secure_cookie_creation_and_deletion_have_matching_transport_attributes() {
    let server = Server::configured(Config::from_pairs([("COOKIE_SECURE", "true")]).unwrap()).await;
    let cookie = server.setup().await;
    let login = server
        .request(
            "POST",
            "/api/v1/auth/login",
            &[],
            Some(json!({"username":"public-admin","password":"PUBLIC-password!"})),
        )
        .await;
    assert_eq!(login.0, 200);
    assert!(login.1.contains("; Secure"));
    let logout = server
        .request("POST", "/api/v1/auth/logout", &[("Cookie", &cookie)], None)
        .await;
    for attribute in [
        "; Secure",
        "HttpOnly",
        "SameSite=lax",
        "Path=/",
        "Max-Age=0",
    ] {
        assert!(logout.1.contains(attribute));
    }
    server.close().await;
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
async fn socket(
    server: &Server,
    cookie: Option<&str>,
    origin: Option<&str>,
    query: Option<&str>,
) -> Result<Socket, tokio_tungstenite::tungstenite::Error> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let url = format!(
        "ws://{}/ws/updates{}",
        server.address,
        query
            .map(|token| format!("?token={token}"))
            .unwrap_or_default()
    );
    let mut request = url.into_client_request().unwrap();
    if let Some(cookie) = cookie {
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
    }
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
    }
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(socket, _)| socket)
}
async fn frame(socket: &mut Socket) -> tokio_tungstenite::tungstenite::Message {
    use futures_util::StreamExt;
    tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn actual_websocket_origin_matrix_retains_cookie_and_query_precedence() {
    let server = Server::start("https://trusted.invalid").await;
    let cookie = server.setup().await;
    let raw = cookie.strip_prefix("session_token=").unwrap();
    let same = format!("http://{}", server.address);
    let cases = [
        (Some(cookie.as_str()), Some(same.as_str()), None, true),
        (
            Some(cookie.as_str()),
            Some("https://trusted.invalid"),
            None,
            true,
        ),
        (
            Some(cookie.as_str()),
            Some("https://foreign.invalid"),
            None,
            false,
        ),
        (Some(cookie.as_str()), Some("null"), None, false),
        (Some(cookie.as_str()), None, None, false),
        (None, None, Some(raw), true),
        (
            Some(cookie.as_str()),
            None,
            Some("invalid-public-token"),
            false,
        ),
        (None, Some(same.as_str()), None, false),
        (
            Some("session_token=invalid-public-token"),
            Some(same.as_str()),
            Some(raw),
            false,
        ),
    ];
    for (cookie, origin, query, allowed) in cases {
        let connected = socket(&server, cookie, origin, query).await;
        if allowed {
            let mut connected = connected.unwrap();
            assert_eq!(
                frame(&mut connected).await.into_text().unwrap(),
                "{\"type\": \"connected\", \"message\": \"Connected to backup manager\"}"
            );
            connected.close(None).await.unwrap();
        } else {
            assert!(
                matches!(connected,Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status().as_u16()==403)
            );
        }
    }
    server.close().await;
}
#[tokio::test]
async fn actual_websocket_preserves_control_messages_and_broadcast_bytes() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    let server = Server::start("http://localhost").await;
    let cookie = server.setup().await;
    let origin = format!("http://{}", server.address);
    let mut connected = socket(&server, Some(&cookie), Some(&origin), None)
        .await
        .unwrap();
    let _ = frame(&mut connected).await;
    connected
        .send(Message::Text("{\"type\":\"ping\"}".into()))
        .await
        .unwrap();
    assert_eq!(
        frame(&mut connected).await.into_text().unwrap(),
        "{\"type\": \"pong\"}"
    );
    connected
        .send(Message::Text(
            "{\"type\":\"subscribe\",\"backup_id\":77}".into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        frame(&mut connected).await.into_text().unwrap(),
        "{\"type\": \"subscribed\", \"backup_id\": 77}"
    );
    connected
        .send(Message::Text("not-json".into()))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&frame(&mut connected).await.into_text().unwrap()).unwrap(),
        json!({"type":"error","message":"Invalid JSON"})
    );
    server.notifications.broadcast(&json!({"type":"backup_progress","backup_id":77,"progress":50,"message":"public protected payload"}));
    assert_eq!(
        serde_json::from_str::<Value>(&frame(&mut connected).await.into_text().unwrap()).unwrap()["backup_id"],
        77
    );
    connected.close(None).await.unwrap();
    server.close().await;
}
#[tokio::test]
async fn actual_open_websockets_revoke_on_logout_password_expiry_and_deleted_rows() {
    for (cause, idle) in [
        ("logout", false),
        ("password", false),
        ("expiry", false),
        ("delete", false),
        ("expiry", true),
        ("delete", true),
    ] {
        let server = Server::start("http://localhost").await;
        let cookie = server.setup().await;
        let origin = format!("http://{}", server.address);
        let mut connected = socket(&server, Some(&cookie), Some(&origin), None)
            .await
            .unwrap();
        let _ = frame(&mut connected).await;
        match cause {
            "logout"=>assert_eq!(server.request("POST","/api/v1/auth/logout",&[("Cookie",&cookie)],None).await.0,200),
            "password"=>assert_eq!(server.request("POST","/api/v1/auth/change-password",&[("Cookie",&cookie)],Some(json!({"current_password":"PUBLIC-password!","new_password":"PUBLIC-new-password!","confirm_password":"PUBLIC-new-password!"}))).await.0,200),
            "expiry"=> {sqlx::query("UPDATE sessions SET expires_at='2020-01-01 00:00:00.000000'").execute(server.db.pool()).await.unwrap();},
            "delete"=> {sqlx::query("DELETE FROM sessions").execute(server.db.pool()).await.unwrap();},_=>unreachable!(),
        }
        if !idle {
            server.notifications.broadcast(&json!({"type":"backup_progress","backup_id":77,"progress":50,"message":"protected"}));
        }
        let closed = frame(&mut connected).await;
        assert!(
            matches!(closed,tokio_tungstenite::tungstenite::Message::Close(Some(close)) if u16::from(close.code)==4001),
            "revoked socket must close without publishing the protected event"
        );
        drop(connected);
        server.close().await;
    }
}

#[tokio::test]
async fn schema_validation_is_nonleaking_and_does_not_consume_the_login_bruteforce_budget() {
    let server = Server::start("http://localhost").await;
    let _ = server.setup().await;
    for _ in 0..6 {
        let invalid = server
            .request("POST", "/api/v1/auth/login", &[], Some(json!({})))
            .await;
        assert_eq!(invalid.0, 422);
        assert_eq!(
            invalid.2,
            json!({"detail":"Validation error","errors":[{"field":"body.username","message":"Field required"},{"field":"body.password","message":"Field required"}]})
        );
    }
    for _ in 0..5 {
        assert_eq!(
            server
                .request(
                    "POST",
                    "/api/v1/auth/login",
                    &[],
                    Some(json!({"username":"public-admin","password":"PUBLIC-wrong-password!"}))
                )
                .await
                .0,
            401
        );
    }
    let limited = server
        .request(
            "POST",
            "/api/v1/auth/login",
            &[],
            Some(json!({"username":"public-admin","password":"PUBLIC-wrong-password!"})),
        )
        .await;
    assert_eq!(limited.0, 429);
    assert_eq!(
        limited.2,
        json!({"error":"Rate limit exceeded: 5 per 1 minute"})
    );
    server.close().await;
}
