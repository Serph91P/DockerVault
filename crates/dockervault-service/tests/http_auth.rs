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
    let missing = post(&valid, remote.clone()).await;
    assert_eq!(
        (missing.0, missing.2),
        (404, json!({"detail":"Storage not found"}))
    );
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled) VALUES (41,'disabled fixture','ssh',0)")
        .execute(server.db.pool()).await.unwrap();
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
