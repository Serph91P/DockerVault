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
// These scenarios share the production process-wide, single-slot recovery KDF.
static RECOVERY_KDF_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
struct Server {
    state: http::HttpState,
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
    async fn configured(config: Config) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-http-auth-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("backups")).unwrap();
        Self::open_at(config, path).await
    }
    async fn open_at(mut config: Config, path: PathBuf) -> Self {
        let backups = path.join("backups");
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
        http::recover_copy_removals(&state).await.unwrap();
        http::recover_s3_copies(&state).await.unwrap();
        let notifications = state.notifications.clone();
        let app = http::router(state.clone());
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
            state,
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
        std::fs::remove_dir_all(self.stop_preserving_files().await).unwrap();
    }
    async fn stop_preserving_files(self) -> PathBuf {
        let _ = self.stop.send(());
        self.task.await.unwrap();
        self.state.settle_storage().await.unwrap();
        self.auth.shutdown(None).await;
        self.db.close().await;
        self.path
    }
}

#[tokio::test]
async fn schedule_reads_and_preview_preserve_the_existing_database() {
    let config = Config::from_pairs([("TZ", "Europe/Berlin")]).unwrap();
    let server = Server::configured(config.clone()).await;
    let cookie = server.setup().await;
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO schedules (id,name,cron_expression,enabled,created_at,updated_at) VALUES
        (10,'Shared','0 3 13 * 4',0,'2025-01-01 00:00:00.123456','2025-01-02 00:00:00.000000'),
        (11,'Invalid','@daily',1,'2025-01-01 00:00:00.000000','2025-01-02 00:00:00.000000');
        INSERT INTO backup_targets (id,name,target_type,schedule_id,schedule_cron,enabled,removed_at) VALUES
        (20,'Assigned','path',10,'0 8 * * 0',1,NULL),
        (21,'Removed','path',10,'0 8 * * 0',1,'2025-01-02 00:00:00.000000'),
        (22,'Invalid legacy','path',NULL,'@daily',1,NULL),
        (23,'Invalid shared overrides legacy','path',11,'0 8 * * sun',1,NULL);")
        .execute(db).await.unwrap();
    let server = Server::open_at(config, server.stop_preserving_files().await).await;
    let db = server.db.pool();
    let snapshot = || async {
        sqlx::query_scalar::<_, String>("SELECT json_array(
          (SELECT json_group_array(json_array(id,name,cron_expression,enabled,created_at,updated_at)) FROM schedules),
          (SELECT json_group_array(json_array(id,schedule_id,schedule_cron,enabled,removed_at)) FROM backup_targets),
          (SELECT count(*) FROM backups), (SELECT count(*) FROM workload_operations),
          (SELECT count(*) FROM storage_operations), (SELECT count(*) FROM backup_schedules),
          (SELECT json_group_array(version) FROM (SELECT version FROM schema_migrations ORDER BY version)))")
            .fetch_one(db).await.unwrap()
    };
    let before = snapshot().await;
    for (method, path) in [
        ("POST", "/api/v1/schedules/preview"),
        ("GET", "/api/v1/schedules"),
        ("GET", "/api/v1/schedules/10"),
        ("GET", "/api/v1/schedules/compatibility/inventory"),
        ("GET", "/api/v1/schedules/legacy/by-target"),
        ("GET", "/api/v1/schedules/cron-help"),
    ] {
        assert_eq!(server.request(method, path, &[], None).await.0, 401);
    }
    let headers = [("cookie", cookie.as_str())];
    let preview = "/api/v1/schedules/preview";
    let (status, _, body) = server
        .request(
            "POST",
            preview,
            &headers,
            Some(json!({
                "cron_expression":"30 2 * * *", "base_time":"2025-03-29T03:00:00+01:00"
            })),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let contract = "Five fields: minute hour day month weekday. Monday=0, Sunday=6; prefer mon..sun. Day-of-month AND weekday must both match. Times use the server timezone and native APScheduler wall-clock/DST behavior; macros are not supported.";
    assert_eq!(
        body,
        json!({"next_runs":["2025-03-30T02:30:00+01:00","2025-03-31T02:30:00+02:00","2025-04-01T02:30:00+02:00"],"timezone":"Europe/Berlin","contract":contract,"warnings":[]})
    );
    for (input, error) in [
        ("@daily", "Wrong number of fields; got 1, expected 5"),
        ("0 0 31 2 *", "Cron expression has no future fire time"),
        (
            "60 0 * * *",
            "Error validating expression '60': the last value (60) is higher than the maximum value (59)",
        ),
        ("*/0 * * * *", "Increment must be higher than 0"),
        ("0 0 * * mON-foo", "Invalid weekday name \"foo\""),
        (
            "0 0 * * sun-mon",
            "The minimum value in a range must not be higher than the maximum",
        ),
        (
            "0x * * * *",
            "Unrecognized expression \"0x\" for field \"minute\"",
        ),
    ] {
        let (status, _, body) = server
            .request(
                "POST",
                preview,
                &headers,
                Some(json!({"cron_expression":input})),
            )
            .await;
        assert_eq!(
            (status, body),
            (
                400,
                json!({"detail":format!("Invalid cron expression: {error}")})
            )
        );
    }
    let (status, _, body) = server.request("POST", preview, &headers, Some(json!({"cron_expression":"* * * * *", "base_time":"2025-03-29T03:00:00", "count":11}))).await;
    assert_eq!(status, 422);
    assert_eq!(
        body,
        json!({"detail":"Validation error","errors":[{"field":"body.base_time","message":"Input should have timezone info"},{"field":"body.count","message":"Input should be less than or equal to 10"}]})
    );
    let (status, _, body) = server
        .request(
            "POST",
            preview,
            &headers,
            Some(json!({
                "cron_expression":"30 2 * * *", "base_time":"2025-10-25T03:00:00+02:00"
            })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body["next_runs"],
        json!([
            "2025-10-26T02:30:00+02:00",
            "2025-10-26T02:30:00+01:00",
            "2025-10-27T02:30:00+01:00"
        ])
    );

    let (_, _, list) = server
        .request("GET", "/api/v1/schedules", &headers, None)
        .await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    let mut shared = json!({"id":10,"name":"Shared","cron_expression":"0 3 13 * 4","description":null,"enabled":false,"target_count":1,"next_run":null,"created_at":"2025-01-01T00:00:00.123456","updated_at":"2025-01-02T00:00:00","cron_warnings":["Numeric weekdays retain existing execution: Monday=0, Sunday=6 (not Unix). Review intent; prefer mon..sun. No automatic conversion was performed.","Day-of-month AND weekday must both match, not Unix OR. Existing execution is unchanged; review intent."]});
    assert_eq!(list[0], shared);
    assert_eq!(list[1]["next_run"], Value::Null);
    assert!(
        list[1]["cron_warnings"][0]
            .as_str()
            .unwrap()
            .contains("Wrong number of fields")
    );
    shared["targets"] = json!([{"id":20,"name":"Assigned","target_type":"path","enabled":true}]);
    assert_eq!(
        server
            .request("GET", "/api/v1/schedules/10", &headers, None)
            .await
            .2,
        shared
    );
    let (_, _, legacy) = server
        .request("GET", "/api/v1/schedules/legacy/by-target", &headers, None)
        .await;
    assert_eq!(
        legacy,
        json!([
            {"id":20,"target_id":20,"target_name":"Assigned","cron_expression":"0 3 13 * 4","next_run":null,"last_run":null,"enabled":false},
            {"id":22,"target_id":22,"target_name":"Invalid legacy","cron_expression":"@daily","next_run":null,"last_run":null,"enabled":true},
            {"id":23,"target_id":23,"target_name":"Invalid shared overrides legacy","cron_expression":"@daily","next_run":null,"last_run":null,"enabled":true}
        ])
    );
    let (_, _, inventory) = server
        .request(
            "GET",
            "/api/v1/schedules/compatibility/inventory",
            &headers,
            None,
        )
        .await;
    assert_eq!(inventory["timezone"], "Europe/Berlin");
    assert_eq!(inventory["items"].as_array().unwrap().len(), 5);
    assert_eq!(inventory["items"][3]["kind"], "target_legacy");
    assert_eq!(inventory["items"][3]["id"], 21);
    let (_, _, help) = server
        .request("GET", "/api/v1/schedules/cron-help", &headers, None)
        .await;
    assert_eq!(help["contract"], contract);
    assert_eq!(help["special"]["*/n"], "Every n units");
    assert_eq!(snapshot().await, before);
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(db)
            .await
            .unwrap(),
        "ok"
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(db)
            .await
            .unwrap()
            .is_empty()
    );
    server.close().await;
}

#[tokio::test]
async fn schedule_preview_count_matches_python_integer_coercion() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    for (count, accepted) in [
        ("2e0", false),
        ("2.00000000000001", false),
        ("2.", false),
        ("2.0", true),
        (" 2 ", true),
        ("0_2", true),
    ] {
        let (status, _, body) = server.request("POST", "/api/v1/schedules/preview", &[("cookie", cookie.as_str())], Some(json!({"cron_expression":"0 3 * * *","base_time":"2025-01-01T00:00:00Z","count":count}))).await;
        if accepted {
            assert_eq!(status, 200, "{count:?}: {body}");
            assert_eq!(body["next_runs"].as_array().unwrap().len(), 2);
        } else {
            assert_eq!(
                (status, body),
                (
                    422,
                    json!({"detail":"Validation error","errors":[{"field":"body.count","message":"Input should be a valid integer, unable to parse string as an integer"}]})
                ),
                "{count:?}"
            );
        }
    }
    server.close().await;
}

#[tokio::test]
async fn schedule_estimate_uses_recent_completed_history_and_live_conflicts() {
    let server = Server::configured(Config::from_pairs([("TZ", "UTC")]).unwrap()).await;
    let cookie = server.setup().await;
    let headers = [("cookie", cookie.as_str())];
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO backup_targets(id,name,target_type,schedule_cron,enabled) VALUES (20,'Fixture','path','0 0 1 1 *',1),(21,'No history','path',NULL,0);
        INSERT INTO backups(target_id,backup_type,status,duration_seconds,created_at) VALUES
        (20,'FULL','COMPLETED',2000,'2025-01-01'),(20,'FULL','COMPLETED',20,'2025-01-02'),
        (20,'FULL','COMPLETED',25,'2025-01-03'),(20,'FULL','COMPLETED',30,'2025-01-04'),
        (20,'FULL','COMPLETED',35,'2025-01-05'),(20,'FULL','COMPLETED',40,'2025-01-06'),
        (20,'FULL','FAILED',2000,'2025-01-07'),(20,'FULL','COMPLETED',NULL,'2025-01-08');")
        .execute(db).await.unwrap();
    assert_eq!(
        server
            .request("POST", "/api/v1/schedules/reload", &headers, None)
            .await
            .0,
        200
    );
    let route = "/api/v1/schedules/estimate";
    let request = |id| Some(json!({"target_id":id,"cron_expression":"0 0 1 1 *"}));
    assert_eq!(server.request("POST", route, &[], request(20)).await.0, 401);
    let (status, _, estimate) = server.request("POST", route, &headers, request(20)).await;
    assert_eq!(status, 200);
    assert_eq!(estimate["duration_seconds"], 36);
    let start = estimate["start_time"].as_str().unwrap();
    assert!(start.ends_with("-01-01T00:00:00+00:00"));
    assert_eq!(
        estimate["estimated_end_time"],
        format!("{}-01-01T00:00:36+00:00", &start[..4])
    );
    assert_eq!(
        estimate["conflicts"],
        json!([{"job_id":"backup_target_20","job_name":"Backup: Fixture","scheduled_time":start}])
    );
    let (status, _, default) = server.request("POST", route, &headers, request(21)).await;
    assert_eq!((status, &default["duration_seconds"]), (200, &json!(60)));
    let (status, _, missing) = server.request("POST", route, &headers, request(99)).await;
    assert_eq!(
        (status, missing),
        (404, json!({"detail":"Target not found"}))
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM backups")
            .fetch_one(db)
            .await
            .unwrap(),
        8
    );
    server.close().await;
}

#[tokio::test]
async fn live_schedule_reload_exposes_only_installed_jobs_and_repairs_failures() {
    let server = Server::configured(Config::from_pairs([("TZ", "UTC")]).unwrap()).await;
    let cookie = server.setup().await;
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO schedules (id,name,cron_expression,enabled) VALUES (7,'Disabled','0 3 * * *',0);
        INSERT INTO backup_targets(id,name,target_type,schedule_id,schedule_cron,enabled,removed_at) VALUES
        (20,'Legacy','path',NULL,'0 3 * * *',1,NULL),(21,'Invalid','path',NULL,'@daily',1,NULL),
        (22,'Disabled shared','path',7,'0 3 * * *',1,NULL),(23,'Removed','path',NULL,'0 3 * * *',1,'2025-01-01 00:00:00.000000');")
        .execute(db).await.unwrap();
    let headers = [("cookie", cookie.as_str())];
    let reload = "/api/v1/schedules/reload";
    let jobs = "/api/v1/schedules/jobs";
    assert_eq!(server.request("GET", jobs, &[], None).await.0, 401);
    assert_eq!(server.request("POST", reload, &[], None).await.0, 401);
    let (status, _, error) = server.request("POST", reload, &headers, None).await;
    assert_eq!(
        (status, error),
        (
            503,
            json!({"detail":"Configuration saved; scheduler reconciliation failed. Retry POST /api/v1/schedules/reload."})
        )
    );
    let (status, _, initial) = server.request("GET", jobs, &headers, None).await;
    assert_eq!(status, 200);
    assert_eq!(initial.as_array().unwrap().len(), 1);
    assert_eq!(initial[0]["id"], "backup_target_20");
    assert_eq!(initial[0]["name"], "Backup: Legacy");
    assert!(initial[0]["next_run"].as_str().unwrap().ends_with("+00:00"));
    sqlx::raw_sql("UPDATE backup_targets SET enabled=0 WHERE id=20; UPDATE backup_targets SET schedule_cron='0 3 * * mon' WHERE id=21;").execute(db).await.unwrap();
    for _ in 0..2 {
        let (status, _, body) = server.request("POST", reload, &headers, None).await;
        assert_eq!((status, body), (200, json!({"status":"reloaded"})));
        let (status, _, body) = server.request("GET", jobs, &headers, None).await;
        assert_eq!(status, 200);
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["id"], "backup_target_21");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM backups")
            .fetch_one(db)
            .await
            .unwrap(),
        0
    );
    server.close().await;
}

#[tokio::test]
async fn schedule_crud_reports_saved_reconcile_failure_and_unlinks_removed_targets() {
    let server = Server::configured(Config::from_pairs([("TZ", "UTC")]).unwrap()).await;
    let cookie = server.setup().await;
    let headers = [("cookie", cookie.as_str())];
    let db = server.db.pool();
    let root = "/api/v1/schedules";
    let (status, _, body) = server
        .request(
            "POST",
            root,
            &headers,
            Some(json!({"name":"Fixture","cron_expression":"@daily"})),
        )
        .await;
    assert_eq!(
        (status, body),
        (
            400,
            json!({"detail":"Invalid cron expression: Wrong number of fields; got 1, expected 5"})
        )
    );
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,schedule_cron,enabled) VALUES (30,'Broken legacy','path','@daily',1)").execute(db).await.unwrap();
    let (status, _, body) = server
        .request(
            "POST",
            root,
            &headers,
            Some(json!({"name":"Fixture","cron_expression":"0 3 * * mon","description":"keep"})),
        )
        .await;
    assert_eq!(
        (status, body),
        (
            503,
            json!({"detail":"Configuration saved; scheduler reconciliation failed. Retry POST /api/v1/schedules/reload."})
        )
    );
    let id: i64 = sqlx::query_scalar("SELECT id FROM schedules WHERE name='Fixture'")
        .fetch_one(db)
        .await
        .unwrap();
    sqlx::raw_sql("UPDATE backup_targets SET enabled=0 WHERE id=30;")
        .execute(db)
        .await
        .unwrap();
    assert_eq!(
        server
            .request("POST", "/api/v1/schedules/reload", &headers, None)
            .await
            .0,
        200
    );
    let path = format!("{root}/{id}");
    let (status, _, body) = server
        .request(
            "PUT",
            &path,
            &headers,
            Some(json!({"enabled":false,"description":null,"cron_expression":null})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        (
            body["enabled"].clone(),
            body["description"].clone(),
            body["cron_expression"].clone(),
            body["next_run"].clone()
        ),
        (
            json!(false),
            json!("keep"),
            json!("0 3 * * mon"),
            Value::Null
        )
    );
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,schedule_id,enabled,removed_at) VALUES(31,'Removed','path',?,1,'2025-01-01 00:00:00.000000')").bind(id).execute(db).await.unwrap();
    let (status, _, body) = server.request("DELETE", &path, &headers, None).await;
    assert_eq!(
        (status, body),
        (200, json!({"status":"deleted","unlinked_targets":1}))
    );
    let row: (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT schedule_id,removed_at FROM backup_targets WHERE id=31")
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!(row, (None, Some("2025-01-01 00:00:00.000000".into())));
    assert_eq!(server.request("GET", &path, &headers, None).await.0, 404);
    server.close().await;
}

#[tokio::test]
async fn started_scheduler_dispatches_and_rechecks_targets_before_admission() {
    let server = Server::configured(Config::from_pairs([("TZ", "UTC")]).unwrap()).await;
    let cookie = server.setup().await;
    let source = server.path.join("scheduled-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data.txt"), b"automatic backup fixture").unwrap();
    let db = server.db.pool();
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,host_path,schedule_cron,enabled) VALUES
        (61,'Active','path',?1,'* * * * *',1),(62,'Stale','path',?1,'* * * * *',1)",
    )
    .bind(source.to_str().unwrap())
    .execute(db)
    .await
    .unwrap();
    let expired = server.path.join("backups/scheduled-expired.tar");
    std::fs::write(&expired, b"expired backup").unwrap();
    sqlx::raw_sql("INSERT INTO retention_policies(id,name,keep_last,keep_daily,keep_weekly,keep_monthly,keep_yearly,max_age_days) VALUES(61,'Schedule fixture',1,0,0,0,0,1); UPDATE backup_targets SET retention_policy_id=61 WHERE id=61;").execute(db).await.unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path,created_at) VALUES(61,61,'FULL','COMPLETED',?,'2020-01-01T00:00:00')").bind(expired.to_str()).execute(db).await.unwrap();
    server.state.start_scheduler().await;
    let headers = [("cookie", cookie.as_str())];
    let (status, _, jobs) = server
        .request("GET", "/api/v1/schedules/jobs", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(jobs.as_array().unwrap().len(), 2);
    sqlx::query("UPDATE backup_targets SET enabled=0 WHERE id=62")
        .execute(db)
        .await
        .unwrap();
    let (completed, archive): (i64, String) =
        tokio::time::timeout(std::time::Duration::from_secs(75), async {
            loop {
                if let Some(row) = sqlx::query_as(
                    "SELECT target_id,file_path FROM backups WHERE status='COMPLETED' AND id!=61 AND NOT EXISTS(SELECT 1 FROM backups WHERE id=61)",
                )
                .fetch_optional(db)
                .await
                .unwrap()
                {
                    break row;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        })
        .await
        .unwrap();
    assert_eq!(completed, 61);
    assert!(!expired.exists());
    assert!(std::path::Path::new(&archive).is_file());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM backups WHERE target_id=62")
            .fetch_one(db)
            .await
            .unwrap(),
        0
    );
    server.close().await;
}

#[tokio::test]
async fn scheduler_projects_retained_plans_when_recovery_gate_reopens() {
    let server = Server::configured(Config::from_pairs([("TZ", "UTC")]).unwrap()).await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO app_settings(key,value) VALUES('recovery_activation','{}'); INSERT INTO backup_targets(id,name,target_type,enabled,schedule_cron) VALUES(191,'Recovered','path',1,'0 3 * * *');").execute(db).await.unwrap();
    server.state.start_scheduler().await;
    let route = "/api/v1/schedules/jobs";
    assert_eq!(
        server.request("GET", route, &headers, None).await.2,
        json!([])
    );
    sqlx::query("DELETE FROM app_settings WHERE key='recovery_activation'")
        .execute(db)
        .await
        .unwrap();
    let jobs = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (_, _, jobs) = server.request("GET", route, &headers, None).await;
            if jobs != json!([]) {
                break jobs;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(jobs.as_array().unwrap().len(), 1);
    assert_eq!(jobs[0]["id"], "backup_target_191");
    server.close().await;
}

#[tokio::test]
async fn overview_reads_retained_catalog_without_treating_a_stale_row_as_live() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO schedules(id,name,cron_expression,enabled) VALUES(40,'Paused','0 3 * * *',0); INSERT INTO backup_targets(id,name,target_type,enabled,schedule_id,schedule_cron) VALUES(40,'Paused','path',1,40,'0 4 * * *'),(41,'Manual','path',1,NULL,NULL); INSERT INTO backups(id,target_id,backup_type,status) VALUES(40,40,'FULL','RUNNING');").execute(db).await.unwrap();
    let route = "/api/v1/overview/summary";
    assert_eq!(server.request("GET", route, &[], None).await.0, 401);
    assert_eq!(
        server
            .request("GET", &format!("{route}?limit=0"), &headers, None)
            .await
            .0,
        422
    );
    let (status, _, body) = server
        .request("GET", &format!("{route}?limit=1"), &headers, None)
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["plans"]["automatic_enabled"], 0);
    assert_eq!(body["plans"]["automatic_paused"], 1);
    assert_eq!(body["plans"]["without_recorded_completed_run"]["total"], 2);
    assert_eq!(
        body["plans"]["without_recorded_completed_run"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        body["backup_activity"],
        json!({"recorded_pending":0,"recorded_running":1,"live":0,"total":1,"items":[{"backup_id":40,"plan_id":40,"name":"Paused","catalog_status":"running","live_worker":false,"stage":null}]})
    );
    server.close().await;
}

#[tokio::test]
async fn encryption_onboarding_status_distinguishes_deferral_from_key_custody() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let status = "/api/v1/encryption/status";
    assert_eq!(server.request("GET", status, &[], None).await.0, 401);
    let expected = |deferred| json!({"setup_completed":false,"encryption_enabled":false,"onboarding_deferred":deferred,"public_key":null,"key_created_at":null});
    assert_eq!(
        server.request("GET", status, &headers, None).await.2,
        expected(false)
    );
    for _ in 0..2 {
        assert_eq!(
            server
                .request("POST", "/api/v1/encryption/defer", &headers, None)
                .await
                .2,
            json!({"onboarding_deferred":true})
        );
    }
    assert_eq!(
        server.request("GET", status, &headers, None).await.2,
        expected(true)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM encryption_config")
            .fetch_one(server.db.pool())
            .await
            .unwrap(),
        0
    );
    server.close().await;
}

#[tokio::test]
async fn encryption_toggle_requires_confirmed_key_custody() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let confirm = "/api/v1/encryption/confirm-setup";
    let disable = "/api/v1/encryption/toggle?enabled=false";
    let db = server.db.pool();
    let (_, _, body) = server
        .request("POST", confirm, &headers, Some(json!({"confirmed":true})))
        .await;
    assert_eq!(
        body,
        json!({"detail":"Encryption not set up. Call /setup first."})
    );
    let (status, headers_out, issued) = server
        .request("POST", "/api/v1/encryption/setup", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert!(
        headers_out
            .lines()
            .any(|line| line.eq_ignore_ascii_case("cache-control: no-store"))
    );
    let public = issued["public_key"].as_str().unwrap();
    let identity: age::x25519::Identity = issued["private_key"].as_str().unwrap().parse().unwrap();
    assert_eq!(identity.to_public().to_string(), public);
    assert_eq!(
        server
            .request("POST", "/api/v1/encryption/setup", &headers, None)
            .await
            .0,
        409
    );
    assert_eq!(server.request("POST", disable, &headers, None).await.0, 400);
    assert_eq!(
        server
            .request("POST", confirm, &headers, Some(json!({"confirmed":false})))
            .await
            .0,
        400
    );
    assert_eq!(
        server
            .request("POST", confirm, &headers, Some(json!({"confirmed":true})))
            .await
            .0,
        200
    );
    assert_eq!(
        server.request("POST", disable, &headers, None).await.2,
        json!({"message":"Encryption disabled for new backups."})
    );
    let stored: (String, bool, bool) = sqlx::query_as(
        "SELECT public_key,setup_completed,encryption_enabled FROM encryption_config WHERE id=1",
    )
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(stored, (public.into(), true, false));
    let rotate = "/api/v1/encryption/regenerate";
    assert_eq!(
        server
            .request(
                "POST",
                rotate,
                &headers,
                Some(json!({"confirm_data_loss":false}))
            )
            .await
            .0,
        400
    );
    let (status, headers_out, regenerated) = server
        .request(
            "POST",
            rotate,
            &headers,
            Some(json!({"confirm_data_loss":true})),
        )
        .await;
    assert_eq!(status, 200);
    assert!(
        headers_out
            .lines()
            .any(|line| line.eq_ignore_ascii_case("cache-control: no-store"))
    );
    let new_public = regenerated["public_key"].as_str().unwrap();
    assert_ne!(new_public, public);
    let stored: (String, bool, bool) = sqlx::query_as(
        "SELECT public_key,setup_completed,encryption_enabled FROM encryption_config WHERE id=1",
    )
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(stored, (new_public.into(), false, false));
    server.close().await;
}

#[tokio::test]
async fn docker_health_is_distinct_from_application_health() {
    let server = Server::configured(
        Config::from_pairs([("DOCKER_SOCKET", "/nonexistent/dockervault-synthetic.sock")]).unwrap(),
    )
    .await;
    let cookie = server.setup().await;
    let route = "/api/v1/docker/health";
    assert_eq!(server.request("GET", route, &[], None).await.0, 401);
    let (status, _, body) = server
        .request("GET", route, &[("Cookie", &cookie)], None)
        .await;
    assert_eq!(
        (status, body),
        (503, json!({"detail":"Docker is not accessible"}))
    );
    assert_eq!(server.request("GET", "/health", &[], None).await.0, 200);
    let volumes = "/api/v1/docker/volumes";
    assert_eq!(server.request("GET", volumes, &[], None).await.0, 401);
    let (status, _, body) = server
        .request("GET", volumes, &[("Cookie", &cookie)], None)
        .await;
    assert_eq!(
        (status, body),
        (500, json!({"detail":"Internal server error"}))
    );
    server.close().await;
}

#[tokio::test]
async fn docker_lifecycle_requires_session_and_validates_stop_timeout() {
    let server = Server::configured(
        Config::from_pairs([("DOCKER_SOCKET", "/nonexistent/dockervault-synthetic.sock")]).unwrap(),
    )
    .await;
    let cookie = server.setup().await;
    let headers = [("cookie", cookie.as_str())];
    let stop = "/api/v1/docker/containers/fixture/stop";
    let start = "/api/v1/docker/containers/fixture/start";
    assert_eq!(server.request("POST", stop, &[], None).await.0, 401);
    for timeout in ["0", "301", "NaN"] {
        let (code, _, body) = server
            .request("POST", &format!("{stop}?timeout={timeout}"), &headers, None)
            .await;
        assert_eq!(code, 422);
        assert_eq!(body["errors"][0]["field"], "query.timeout");
    }
    for (route, reason) in [
        (stop, "Failed to stop container"),
        (start, "Failed to start container"),
    ] {
        let (code, _, body) = server.request("POST", route, &headers, None).await;
        assert_eq!((code, body), (500, json!({"detail":reason})));
    }
    server.close().await;
}

#[tokio::test]
async fn backup_prerequisite_validation_reports_missing_source_without_starting_backup() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("cookie", cookie.as_str())];
    let source = server.path.join("missing-source");
    let db = server.db.pool();
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,host_path) VALUES(91,'Fixture','path',?)",
    )
    .bind(source.to_str().unwrap())
    .execute(db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO backups(id,target_id,backup_type,status) VALUES(91,91,'FULL','COMPLETED')",
    )
    .execute(db)
    .await
    .unwrap();
    let route = "/api/v1/backups/91/validate";
    assert_eq!(server.request("POST", route, &[], None).await.0, 401);
    let (status, _, body) = server.request("POST", route, &headers, None).await;
    assert_eq!(
        (status, body),
        (
            200,
            json!({"valid":false,"issues":[format!("Path '{}' not found",source.display())]})
        )
    );
    std::fs::create_dir(&source).unwrap();
    let (status, _, body) = server.request("POST", route, &headers, None).await;
    assert_eq!((status, body), (200, json!({"valid":true,"issues":[]})));
    let (status, _, body) = server
        .request("POST", "/api/v1/backups/92/validate", &headers, None)
        .await;
    assert_eq!((status, body), (404, json!({"detail":"Backup not found"})));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM backups")
            .fetch_one(db)
            .await
            .unwrap(),
        1
    );
    server.close().await;
}

#[tokio::test]
async fn system_info_is_authenticated_and_redacts_configured_paths() {
    let server = Server::start("http://localhost:3000").await;
    let path = "/api/v1/settings/system-info";
    let cookie = server.setup().await;
    assert_eq!(server.request("GET", path, &[], None).await.0, 401);
    let (status, _, info) = server
        .request("GET", path, &[("Cookie", &cookie)], None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(info["backup_dir"], "[configured]");
    assert_eq!(info["database_path"], "[configured]");
    assert_eq!(info["disk_status"], "available");
    assert!(info["disk_total"].as_u64().unwrap() > 0);
    assert_eq!(info["backup_count"], 0);
    assert_eq!(info["target_count"], 0);
    assert_eq!(info["timezone"], server.state.config.tz);
    assert!(info["uptime_seconds"].is_u64());
    assert_eq!(info["app_version"], "1.0.0");
    server.close().await;
}

#[tokio::test]
async fn storage_quota_setting_is_authenticated_and_persists_false_explicitly() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let path = "/api/v1/settings/storage";
    assert_eq!(server.request("GET", path, &[], None).await.0, 401);
    assert_eq!(
        server.request("GET", path, &headers, None).await.2,
        json!({"check_quota_before_upload":false})
    );
    assert_eq!(
        server
            .request("PUT", path, &headers, Some(json!({})))
            .await
            .0,
        422
    );
    for enabled in [true, false] {
        let expected = json!({"check_quota_before_upload":enabled});
        let (status, _, body) = server
            .request("PUT", path, &headers, Some(expected.clone()))
            .await;
        assert_eq!((status, body), (200, expected.clone()));
        assert_eq!(
            server.request("GET", path, &headers, None).await.2,
            expected
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT value FROM app_settings WHERE key='storage_check_quota_before_upload'"
            )
            .fetch_one(server.db.pool())
            .await
            .unwrap(),
            enabled.to_string()
        );
    }
    server.close().await;
}

#[tokio::test]
async fn target_reads_include_relationships_and_legacy_repair_warnings() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("Cookie", cookie.as_str())];
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO schedules(id,name,cron_expression,enabled) VALUES(70,'Week','0 3 * * 0',0); INSERT INTO retention_policies(id,name,keep_last,keep_daily,keep_weekly,keep_monthly,keep_yearly,max_age_days) VALUES(71,'GFS',2,0,1,0,0,365); INSERT INTO backup_targets(id,name,target_type,schedule_id,retention_policy_id,remote_storage_ids,created_at,enabled,stop_container,compression_enabled) VALUES(72,'Active','path',70,71,'[700,701]','2025-01-01 00:00:00.123456',0,0,0); INSERT INTO backup_targets(id,name,target_type,removed_at,enabled,stop_container,compression_enabled) VALUES(73,'Removed','path','2025-01-01 00:00:00',0,0,0);").execute(db).await.unwrap();
    assert_eq!(
        server.request("GET", "/api/v1/targets", &[], None).await.0,
        401
    );
    let (status, _, list) = server
        .request("GET", "/api/v1/targets", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let active = &list[0];
    assert_eq!(
        active["schedule"],
        json!({"id":70,"name":"Week","cron_expression":"0 3 * * 0"})
    );
    assert_eq!(
        active["retention_policy"],
        json!({"id":71,"name":"GFS","keep_last":2,"keep_daily":0,"keep_weekly":1,"keep_monthly":0})
    );
    assert_eq!(active["created_at"], "2025-01-01T00:00:00.123456Z");
    assert_eq!(
        active["validation_errors"],
        json!([
            "remote_storage_id 700 does not exist",
            "remote_storage_id 701 does not exist"
        ])
    );
    assert_eq!(active["cron_warnings"].as_array().unwrap().len(), 1);
    assert_eq!(active["dependencies"], json!([]));
    assert_eq!(active["per_volume_rules"], json!({}));
    assert_eq!(
        server
            .request("GET", "/api/v1/targets/72", &headers, None)
            .await
            .2,
        *active
    );
    let (status, _, listed) = server
        .request("GET", "/api/v1/targets?include_removed=yes", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(listed[1]["removed_at"], "2025-01-01T00:00:00Z");
    let (status, _, missing) = server
        .request("GET", "/api/v1/targets/73", &headers, None)
        .await;
    assert_eq!(
        (status, missing),
        (404, json!({"detail":"Target not found"}))
    );
    server.close().await;
}

#[tokio::test]
async fn deleting_a_plan_tombstones_it_without_deleting_recovery_points() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let db = server.db.pool();
    let archive = server.path.join("backups/historical.tar");
    std::fs::write(&archive, b"historical archive").unwrap();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,enabled,schedule_cron) VALUES(123,'History','path',1,'0 3 * * *')").execute(db).await.unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path) VALUES(123,123,'FULL','COMPLETED',?),(124,123,'FULL','PENDING',NULL)").bind(archive.to_str()).execute(db).await.unwrap();
    let route = "/api/v1/targets/123";
    assert_eq!(server.request("DELETE", route, &[], None).await.0, 401);
    assert_eq!(
        server.request("DELETE", route, &headers, None).await.2,
        json!({"status":"removed","copies":"retained"})
    );
    let stored: (String, bool) =
        sqlx::query_as("SELECT removed_at,enabled FROM backup_targets WHERE id=123")
            .fetch_one(db)
            .await
            .unwrap();
    assert!(!stored.1);
    assert_eq!(server.request("GET", route, &headers, None).await.0, 404);
    assert_eq!(
        server.request("DELETE", route, &headers, None).await.2,
        json!({"status":"already_removed","copies":"retained"})
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT removed_at FROM backup_targets WHERE id=123")
            .fetch_one(db)
            .await
            .unwrap(),
        stored.0
    );
    assert_eq!(
        sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT status,file_path FROM backups WHERE id=123"
        )
        .fetch_one(db)
        .await
        .unwrap(),
        ("COMPLETED".into(), Some(archive.to_str().unwrap().into()))
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM backups WHERE id=124")
            .fetch_one(db)
            .await
            .unwrap(),
        "FAILED"
    );
    assert_eq!(std::fs::read(archive).unwrap(), b"historical archive");
    server.close().await;
}

#[tokio::test]
async fn target_writes_validate_references_and_preserve_explicit_clears() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let dir = server.path.join("target-source");
    std::fs::create_dir(&dir).unwrap();
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO schedules(id,name,cron_expression,enabled) VALUES(177,'Shared','0 3 * * *',1); INSERT INTO retention_policies(id,name,keep_last,keep_daily,keep_weekly,keep_monthly,keep_yearly,max_age_days) VALUES(177,'Policy',2,1,0,0,0,30);").execute(db).await.unwrap();
    let root = "/api/v1/targets";
    let bad =
        json!({"name":"Rejected","target_type":"path","host_path":dir,"remote_storage_ids":[999]});
    let (status, _, body) = server.request("POST", root, &headers, Some(bad)).await;
    assert_eq!(
        (status, body),
        (
            400,
            json!({"detail":{"message":"Invalid target references","errors":["remote_storage_id 999 does not exist"]}})
        )
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM backup_targets")
            .fetch_one(db)
            .await
            .unwrap(),
        0
    );
    for (field, value, message) in [
        ("name", Value::Null, "Input should be a valid string"),
        ("name", json!(""), "String should have at least 1 character"),
        (
            "name",
            json!("x".repeat(256)),
            "String should have at most 255 characters",
        ),
        ("enabled", Value::Null, "Input should be a valid boolean"),
        ("include_paths", Value::Null, "Input should be a valid list"),
        (
            "per_volume_rules",
            Value::Null,
            "Input should be a valid dictionary",
        ),
    ] {
        let mut body = json!({"name":"Native","target_type":"path","host_path":dir});
        body[field] = value;
        let (status, _, response) = server.request("POST", root, &headers, Some(body)).await;
        assert_eq!(
            (status, response["errors"][0].clone()),
            (
                422,
                json!({"field":format!("body.{field}"),"message":message})
            )
        );
    }
    let (status, _, created) = server.request("POST",root,&headers,Some(json!({"name":"Native","target_type":"path","host_path":dir,"enabled":0.0,"schedule_id":177.0,"schedule_cron":" 0 4 * * * ","retention_policy_id":177,"include_paths":["one"]}))).await;
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["schedule_cron"], "0 4 * * *");
    assert_eq!(created["enabled"], false);
    let route = format!("{root}/{}", created["id"]);
    let (status, _, _) = server
        .request("PUT", &route, &headers, Some(json!({"enabled":null})))
        .await;
    assert_eq!(status, 422);
    let (status, _, edited) = server.request("PUT", &route, &headers, Some(json!({"schedule_id":null,"schedule_cron":null,"retention_policy_id":null,"include_paths":[],"compression_enabled":false}))).await;
    assert_eq!(status, 200, "{edited}");
    for key in ["schedule_id", "schedule_cron", "retention_policy_id"] {
        assert!(edited[key].is_null());
    }
    assert_eq!(edited["include_paths"], json!([]));
    assert_eq!(edited["compression_enabled"], false);
    assert_eq!(edited["enabled"], false);
    assert_eq!(
        server.request("GET", &route, &headers, None).await.2,
        edited
    );
    server.close().await;
}

#[tokio::test]
async fn orphan_cleanup_preserves_unknown_active_and_local_replica_archives() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let root = server.path.join("backups");
    let unknown = root.join("unregistered.tar");
    let active = root.join("active.tar.gz");
    let replica = root.join("replica.tar.gz");
    std::fs::write(&unknown, b"unowned").unwrap();
    std::fs::write(&active, b"reserved").unwrap();
    std::fs::write(&replica, b"local replica").unwrap();
    let db = server.db.pool();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type) VALUES(83,'Fixture','path')")
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,backup_metadata) VALUES(83,83,'FULL','RUNNING',?)")
        .bind(json!({"active_artifacts":[active]}).to_string()).execute(db).await.unwrap();
    sqlx::query(
        "INSERT INTO backups(id,target_id,backup_type,status) VALUES(84,83,'FULL','COMPLETED')",
    )
    .execute(db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES(84,'Fixture replica','local',1,?)")
        .bind(root.to_str().unwrap()).execute(db).await.unwrap();
    sqlx::query("INSERT INTO backup_storage_syncs(id,backup_id,storage_id,remote_path,sync_status) VALUES(84,84,84,'replica.tar.gz','completed')")
        .execute(db).await.unwrap();
    let route = "/api/v1/retention/cleanup-orphaned";
    for method in ["GET", "POST", "POST"] {
        assert_eq!(server.request(method, route, &[], None).await.0, 401);
        let (status, _, report) = server.request(method, route, &headers, None).await;
        assert_eq!(status, 200);
        assert_eq!(report["protected"], 3);
        assert_eq!(report["mode"], "scan_only");
        assert_eq!(report["deleted"], 0);
        assert_eq!(report["freed_bytes"], 0);
        assert_eq!(report["scan_complete"], true);
    }
    assert_eq!(std::fs::read(unknown).unwrap(), b"unowned");
    assert_eq!(std::fs::read(active).unwrap(), b"reserved");
    assert_eq!(std::fs::read(replica).unwrap(), b"local replica");
    let sync: (i64, i64, String, String) = sqlx::query_as(
        "SELECT backup_id,storage_id,remote_path,sync_status FROM backup_storage_syncs WHERE id=84",
    )
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(sync, (84, 84, "replica.tar.gz".into(), "completed".into()));
    server.close().await;
}

#[tokio::test]
async fn retention_reads_preserve_policy_and_catalog_wire_contracts() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO retention_policies (id,name,keep_last,keep_daily,keep_weekly,keep_monthly,keep_yearly,max_age_days,created_at,updated_at) VALUES
        (71,'fixture',3,7,0,6,2,365,'2025-01-01 00:00:00.123456','2025-01-02 00:00:00.000000');
        INSERT INTO backup_targets(id,name,target_type,retention_policy_id) VALUES (72,'fixture','path',71),(73,'other','path',71);
        INSERT INTO backups(id,target_id,backup_type,status,file_size,created_at) VALUES
        (81,72,'FULL','COMPLETED',1536,'2025-01-02 00:00:00.000000'),
        (82,72,'FULL','COMPLETED',NULL,'2025-01-01 00:00:00.123456'),
        (83,72,'FULL','FAILED',99999,'2025-01-03 00:00:00.000000'),
        (84,73,'FULL','COMPLETED',99999,'2025-01-03 00:00:00.000000');")
        .execute(db).await.unwrap();
    let headers = [("cookie", cookie.as_str())];
    let expected = json!({"id":71,"name":"fixture","keep_last":3,"keep_daily":7,"keep_weekly":0,"keep_monthly":6,"keep_yearly":2,"max_age_days":365,"created_at":"2025-01-01T00:00:00.123456","updated_at":"2025-01-02T00:00:00"});
    for (path, code, expected_body) in [
        ("/api/v1/retention/71", 200, expected.clone()),
        (
            "/api/v1/retention/99",
            404,
            json!({"detail":"Policy not found"}),
        ),
        (
            "/api/v1/retention/72/stats",
            200,
            json!({"total_backups":2,"total_size_bytes":1536,"total_size_human":"1.50 KB","oldest_backup":"2025-01-01T00:00:00.123456","newest_backup":"2025-01-02T00:00:00"}),
        ),
        (
            "/api/v1/retention/99/stats",
            200,
            json!({"total_backups":0,"total_size_bytes":0,"oldest_backup":null,"newest_backup":null}),
        ),
    ] {
        assert_eq!(
            server.request("GET", path, &[], None).await.0,
            401,
            "{path}"
        );
        let (status, _, body) = server.request("GET", path, &headers, None).await;
        assert_eq!((status, body), (code, expected_body), "{path}");
    }
    let (status, _, list) = server
        .request("GET", "/api/v1/retention", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert!(list.as_array().unwrap().contains(&expected));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM backups")
            .fetch_one(db)
            .await
            .unwrap(),
        4
    );
    server.close().await;
}

#[tokio::test]
async fn retention_policy_writes_preserve_null_zero_and_removed_references() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [("cookie", cookie.as_str())];
    let db = server.db.pool();
    let path = "/api/v1/retention";
    assert_eq!(
        server
            .request("POST", path, &[], Some(json!({"name":"fixture"})))
            .await
            .0,
        401
    );
    let (status, _, invalid) = server
        .request(
            "POST",
            path,
            &headers,
            Some(json!({"name":"fixture","keep_last":0})),
        )
        .await;
    assert_eq!(
        (status, invalid),
        (
            422,
            json!({"detail":"Validation error","errors":[{"field":"body.keep_last","message":"Input should be greater than or equal to 1"}]})
        )
    );
    let (status, _, created) = server
        .request(
            "POST",
            path,
            &headers,
            Some(json!({"name":"fixture","keep_daily":0})),
        )
        .await;
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["keep_daily"], 0);
    assert_eq!(created["keep_last"], 3);
    let id = created["id"].as_i64().unwrap();
    let (status, _, duplicate) = server
        .request("POST", path, &headers, Some(json!({"name":"fixture"})))
        .await;
    assert_eq!(
        (status, duplicate),
        (
            400,
            json!({"detail":"Policy with this name already exists"})
        )
    );
    let path = format!("{path}/{id}");
    let (status, _, changed) = server
        .request(
            "PUT",
            &path,
            &headers,
            Some(json!({"name":null,"keep_last":null,"keep_weekly":0})),
        )
        .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["name"], "fixture");
    assert_eq!(changed["keep_last"], 3);
    assert_eq!(changed["keep_weekly"], 0);
    assert_eq!(changed["created_at"], created["created_at"]);
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,retention_policy_id,removed_at) VALUES (72,'Removed','path',?,'2025-01-01 00:00:00.000000')").bind(id).execute(db).await.unwrap();
    let (status, _, protected) = server.request("DELETE", &path, &headers, None).await;
    assert_eq!(
        (status, protected),
        (
            409,
            json!({"detail":"Policy is still referenced by a plan, including removed plans"})
        )
    );
    let reference: i64 =
        sqlx::query_scalar("SELECT retention_policy_id FROM backup_targets WHERE id=72")
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!(reference, id);
    sqlx::query("UPDATE backup_targets SET retention_policy_id=NULL WHERE id=72")
        .execute(db)
        .await
        .unwrap();
    let (status, _, deleted) = server.request("DELETE", &path, &headers, None).await;
    assert_eq!((status, deleted), (200, json!({"status":"deleted"})));
    assert_eq!(server.request("GET", &path, &headers, None).await.0, 404);
    server.close().await;
}

#[tokio::test]
async fn schedule_target_trigger_admits_only_an_enabled_non_removed_plan() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let db = server.db.pool();
    let source = server.path.join("trigger-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data.txt"), b"scheduled fixture").unwrap();
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,host_path,enabled,removed_at) VALUES
        (21,'Enabled','path',?1,1,NULL),(22,'Disabled','path',?1,0,NULL),
        (23,'Removed','path',?1,1,'2025-01-01 00:00:00.000000')",
    )
    .bind(source.to_str().unwrap())
    .execute(db)
    .await
    .unwrap();
    let headers = [("cookie", cookie.as_str())];
    let endpoint = |id| format!("/api/v1/schedules/target/{id}/trigger");
    assert_eq!(
        server.request("POST", &endpoint(21), &[], None).await.0,
        401
    );
    for (id, status, detail) in [
        (22, 409, "Target is disabled"),
        (23, 404, "Target not found"),
        (99, 404, "Target not found"),
    ] {
        let (actual, _, body) = server.request("POST", &endpoint(id), &headers, None).await;
        assert_eq!((actual, body), (status, json!({"detail":detail})), "{id}");
    }
    let (status, _, body) = server
        .request(
            "POST",
            "/api/v1/schedules/target/22.0/trigger",
            &headers,
            None,
        )
        .await;
    assert_eq!(
        (status, body),
        (409, json!({"detail":"Target is disabled"}))
    );
    let (status, _, body) = server.request("POST", &endpoint(21), &headers, None).await;
    assert_eq!((status, body), (200, json!({"status":"triggered"})));
    let results = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let rows: Vec<(i64, String, Option<String>)> =
                sqlx::query_as("SELECT target_id,status,file_path FROM backups")
                    .fetch_all(db)
                    .await
                    .unwrap();
            if rows
                .iter()
                .all(|(_, state, _)| !matches!(state.as_str(), "RUNNING" | "PENDING"))
            {
                break rows;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(results.len(), 1);
    let (id, status, archive) = &results[0];
    assert_eq!((*id, status.as_str()), (21, "COMPLETED"));
    assert!(std::path::Path::new(archive.as_ref().unwrap()).is_file());
    server.close().await;
}

#[tokio::test]
async fn ssh_public_key_download_requires_auth_and_checks_the_key_name() {
    let server = Server::start("http://localhost:3000").await;
    let missing = "/api/v1/storage/ssh-keys/_missing_fixture_/public";
    let cookie = server.setup().await;
    assert_eq!(server.request("GET", missing, &[], None).await.0, 401);
    let headers = [("cookie", cookie.as_str())];
    let (status, _, body) = server.request("GET", missing, &headers, None).await;
    assert_eq!(
        (status, body),
        (404, json!({"detail":"Public key not found"}))
    );
    let (status, _, body) = server
        .request(
            "GET",
            "/api/v1/storage/ssh-keys/bad.name/public",
            &headers,
            None,
        )
        .await;
    assert_eq!(
        (status, body),
        (
            400,
            json!({"detail":"Key name must be 1-40 chars: letters, digits, '_' or '-'."})
        )
    );
    server.close().await;
}

#[tokio::test]
async fn ssh_key_metadata_routes_use_the_legacy_empty_and_missing_replies() {
    let server = Server::start("http://localhost:3000").await;
    let root = "/api/v1/storage/ssh-keys";
    let cookie = server.setup().await;
    let headers = [("cookie", cookie.as_str())];
    assert_eq!(server.request("GET", root, &[], None).await.0, 401);
    let (status, _, body) = server.request("GET", root, &headers, None).await;
    assert_eq!((status, body), (200, json!([])));
    let (status, _, body) = server
        .request("GET", &format!("{root}/_missing_fixture_"), &headers, None)
        .await;
    assert_eq!((status, body), (404, json!({"detail":"SSH key not found"})));
    server.close().await;
}

#[tokio::test]
async fn recovery_status_reads_the_committed_legacy_result_without_writing() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let path = "/api/v1/recovery/status";
    let headers = [("cookie", cookie.as_str())];
    assert_eq!(server.request("GET", path, &[], None).await.0, 401);
    let (status, _, result) = server.request("GET", path, &headers, None).await;
    assert_eq!((status, result), (200, Value::Null));
    let result = json!({
        "counts":{"backup_targets":1,"schedules":0,"retention_policies":0,"remote_storages":1,"encryption_config":0,"app_settings":0,"backups":1,"backup_schedules":0,"backup_storage_syncs":1},
        "storages":[{"id":2,"type":"sftp","state":"credentials_required"}],
        "catalog":[{"id":1,"local":"unavailable","copies":[{"storage_id":2,"state":"credentials_required"}]}],
        "archives_modified":false,
        "ssh_public_keys":[]
    });
    let refs = json!([{"name":"original","public_key":"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIC7G8BfNXfsH4jQXzdy06W3u7i3BPlMrZ1E9QXuTLfPb"}]);
    let pool = server.db.pool();
    let write = "INSERT INTO app_settings(key,value) VALUES(?,?)";
    sqlx::query(write)
        .bind("recovery_result")
        .bind(result.to_string())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(write)
        .bind("recovery_ssh_public_keys")
        .bind(refs.to_string())
        .execute(pool)
        .await
        .unwrap();
    let mut expected = result.clone();
    expected["ssh_public_keys"] = refs;
    let (status, _, actual) = server.request("GET", path, &headers, None).await;
    assert_eq!((status, actual), (200, expected));
    let saved: String =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_result'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(saved, result.to_string());
    sqlx::query("UPDATE app_settings SET value=? WHERE key='recovery_ssh_public_keys'")
        .bind(r#"[{"name":"original","public_key":"TEST_ONLY_NON_PUBLIC"}]"#)
        .execute(pool)
        .await
        .unwrap();
    let (status, _, invalid) = server.request("GET", path, &headers, None).await;
    assert_eq!(
        (status, invalid),
        (500, json!({"detail":"Internal server error"}))
    );
    server.close().await;
}

#[tokio::test]
async fn schedule_batch_runs_enabled_targets_sequentially() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let db = server.db.pool();
    let source = server.path.join("batch-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data.txt"), b"batch fixture").unwrap();
    sqlx::raw_sql("INSERT INTO schedules (id,name,cron_expression,enabled) VALUES(31,'Manual','@daily',0);
        INSERT INTO backup_targets(id,name,target_type,schedule_id,enabled,removed_at) VALUES
        (21,'First','path',31,1,NULL),(22,'Disabled','path',31,0,NULL),
        (23,'Removed','path',31,1,'2025-01-01 00:00:00.000000'),(24,'Second','path',31,1,NULL);
        CREATE TRIGGER no_batch_overlap BEFORE UPDATE OF status ON backups
        WHEN NEW.status='RUNNING' AND NEW.target_id=24 AND NOT EXISTS (SELECT 1 FROM backups WHERE target_id=21 AND status='COMPLETED')
        BEGIN SELECT RAISE(ABORT, 'batch overlap'); END;")
        .execute(db).await.unwrap();
    sqlx::query("UPDATE backup_targets SET host_path=? WHERE schedule_id=31")
        .bind(source.to_str().unwrap())
        .execute(db)
        .await
        .unwrap();
    let headers = [("cookie", cookie.as_str())];
    let endpoint = "/api/v1/schedules/31/trigger-all";
    assert_eq!(server.request("POST", endpoint, &[], None).await.0, 401);
    let (status, _, body) = server
        .request("POST", "/api/v1/schedules/99.0/trigger-all", &headers, None)
        .await;
    assert_eq!(
        (status, body),
        (404, json!({"detail":"Schedule not found"}))
    );
    let (status, _, body) = server.request("POST", endpoint, &headers, None).await;
    assert_eq!(
        (status, body),
        (
            200,
            json!({"status":"triggered","schedule_id":31,"triggered_targets":[21,24],"skipped_targets":[22]})
        )
    );
    let results = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let rows: Vec<(i64, String, Option<String>)> =
                sqlx::query_as("SELECT target_id,status,file_path FROM backups ORDER BY id")
                    .fetch_all(db)
                    .await
                    .unwrap();
            if rows
                .iter()
                .all(|(_, state, _)| !matches!(state.as_str(), "RUNNING" | "PENDING"))
            {
                break rows;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        results
            .iter()
            .map(|(id, status, _)| (*id, status.as_str()))
            .collect::<Vec<_>>(),
        vec![(21, "COMPLETED"), (24, "COMPLETED")]
    );
    assert!(
        results
            .iter()
            .all(|(_, _, path)| std::path::Path::new(path.as_ref().unwrap()).is_file())
    );
    server.close().await;
}

#[tokio::test]
async fn submitted_path_backup_publishes_a_verified_archive_before_completion() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let source = server.path.join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data.txt"), b"owned backup bytes").unwrap();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled,compression_enabled) VALUES(1,'Source','path',?,1,0)")
        .bind(source.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    let (status, _, accepted) = server
        .request(
            "POST",
            "/api/v1/backups",
            &[("Cookie", &cookie), ("Origin", "http://localhost:3000")],
            Some(json!({"target_id":1})),
        )
        .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["status"], "pending");
    let id = accepted["id"].as_i64().unwrap();
    let (path, checksum, length) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let row: (String, Option<String>, Option<String>, Option<i64>) = sqlx::query_as(
                "SELECT status,file_path,checksum,file_size FROM backups WHERE id=?",
            )
            .bind(id)
            .fetch_one(server.db.pool())
            .await
            .unwrap();
            match row {
                (state, Some(path), Some(checksum), Some(length)) if state == "COMPLETED" => {
                    break (path, checksum, length);
                }
                (state, _, _, _) if state == "PENDING" || state == "RUNNING" => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await
                }
                row => panic!("Unexpected settlement: {row:?}"),
            }
        }
    })
    .await
    .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(i64::try_from(bytes.len()).unwrap(), length);
    use sha2::{Digest, Sha256};
    assert_eq!(
        checksum,
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let mut reader = tar::Archive::new(bytes.as_slice());
    let mut entry = reader
        .entries()
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| entry.path().unwrap().as_ref() == std::path::Path::new("source/data.txt"))
        .unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
    assert_eq!(data, b"owned backup bytes");
    let browse = format!("/api/v1/backups/{id}/files");
    let browse_auth = [("Cookie", cookie.as_str())];
    assert_eq!(server.request("GET", &browse, &[], None).await.0, 401);
    for (suffix, path, more, total) in [
        ("?limit=1", "source", "true", "2"),
        ("?limit=1&offset=1", "source/data.txt", "false", "2"),
        ("?search=DATA.TXT", "source/data.txt", "false", "1"),
    ] {
        let (status, headers, entries) = server
            .request("GET", &format!("{browse}{suffix}"), &browse_auth, None)
            .await;
        assert_eq!(status, 200);
        assert!(
            headers
                .lines()
                .any(|line| line == format!("x-total-count: {total}"))
        );
        assert!(
            headers
                .lines()
                .any(|line| line == format!("x-has-more: {more}"))
        );
        assert_eq!(entries.as_array().unwrap().len(), 1);
        assert_eq!(entries[0]["path"], path);
    }
    assert_eq!(
        server
            .request("GET", &format!("{browse}?limit=2001"), &browse_auth, None)
            .await
            .0,
        422
    );
    let headers = [("Cookie", cookie.as_str())];
    let summary = "/api/v1/backups/metrics/summary";
    let member = format!("{browse}/source/data.txt");
    assert_eq!(server.request("GET", &member, &[], None).await.0, 401);
    let (status, member_headers, body) = server.request("GET", &member, &browse_auth, None).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!("owned backup bytes"));
    assert!(
        member_headers
            .lines()
            .any(|line| line == "cache-control: no-store")
    );
    assert_eq!(
        server
            .request("GET", &format!("{browse}/source"), &browse_auth, None)
            .await
            .0,
        400
    );
    assert_eq!(
        server
            .request("GET", &format!("{browse}/missing"), &browse_auth, None)
            .await
            .0,
        404
    );
    assert_eq!(server.request("GET", summary, &[], None).await.0, 401);
    let (status, _, payload) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let response = server.request("GET", summary, &headers, None).await;
            if response.0 != 200 || response.2["total_backups"] != 0 {
                break response;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status, 200);
    assert_eq!(payload["total_backups"], 1);
    assert_eq!(payload["successful_backups"], 1);
    assert_eq!(payload["failed_backups"], 0);
    assert_eq!(payload["success_rate"], 100.0);
    assert_eq!(payload["total_bytes_backed_up"], length);
    assert!(payload["last_backup_time"].is_string());
    let (status, _, payload) = server
        .request("GET", "/api/v1/backups/metrics/target/1", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(payload["average_size_bytes"], length);
    assert!(payload["average_duration_seconds"].as_f64().unwrap() >= 0.0);
    assert_eq!(payload["backup_count"], 1);
    let (status, _, payload) = server
        .request("GET", "/api/v1/backups/metrics/target/999", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        payload,
        json!({"target_id":999,"average_duration_seconds":null,"average_size_bytes":null,"average_size_human":null,"backup_count":0})
    );
    let (status, _, logs) = server
        .request("GET", &format!("/api/v1/backups/{id}/logs"), &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        logs.as_array()
            .unwrap()
            .iter()
            .map(|row| row["step"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["start", "archive", "archive", "complete"]
    );
    assert_eq!(logs[3]["details"]["file_size"], length);
    assert_eq!(
        server
            .request(
                "GET",
                &format!("/api/v1/backups/{id}/download"),
                &headers,
                None
            )
            .await
            .0,
        200
    );
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled) VALUES(2,'Missing','path',?,1)")
            .bind(source.join("missing-fixture").to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    let (status, _, failed) = server
        .request(
            "POST",
            "/api/v1/backups",
            &[("Cookie", &cookie), ("Origin", "http://localhost:3000")],
            Some(json!({"target_id":2})),
        )
        .await;
    assert_eq!(status, 200);
    let failed = failed["id"].as_i64().unwrap();
    let fail_stats = format!("/api/v1/backups/{failed}/stats");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (status, _, stats) = server.request("GET", &fail_stats, &headers, None).await;
            assert_eq!(status, 200);
            if stats["status"] == "failed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (status, _, logs) = server
        .request(
            "GET",
            &format!("/api/v1/backups/{failed}/logs"),
            &headers,
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        logs.as_array()
            .unwrap()
            .iter()
            .map(|row| row["step"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["start", "error"]
    );
    assert_eq!(logs[1]["level"], "error");
    std::fs::write(source.join("data.txt"), b"post-backup mutation").unwrap();
    std::fs::write(source.join("unrelated"), b"preserve me").unwrap();
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let review_route = format!("/api/v1/backups/{id}/restore-review");
    let restore_route = format!("/api/v1/backups/{id}/restore");
    let selected = json!({"destination_type":"original"});
    let (code, _, review) = server
        .request("POST", &review_route, &headers, Some(selected.clone()))
        .await;
    assert_eq!(code, 200, "{review}");
    assert_eq!(review["destination"]["path"], source.to_str().unwrap());
    assert_eq!(
        server
            .request("POST", &restore_route, &headers, Some(selected))
            .await
            .0,
        409
    );
    let payload = json!({"destination_type":"original","review_fingerprint":review["fingerprint"]});
    std::fs::rename(&source, server.path.join("previous")).unwrap();
    std::fs::create_dir(&source).unwrap();
    assert_eq!(
        server
            .request("POST", &restore_route, &headers, Some(payload))
            .await
            .0,
        409
    );
    assert!(!source.join("data.txt").exists());
    std::fs::rename(server.path.join("previous"), &source).unwrap();
    let (code, _, restored) = server
        .request(
            "POST",
            &restore_route,
            &headers,
            Some(json!({"destination_type":"original","review_fingerprint":review["fingerprint"]})),
        )
        .await;
    assert_eq!(code, 200, "{restored}");
    assert_eq!(restored["status"], "restored");
    assert_eq!(
        std::fs::read(source.join("data.txt")).unwrap(),
        b"owned backup bytes"
    );
    assert_eq!(
        std::fs::read(source.join("unrelated")).unwrap(),
        b"preserve me"
    );
    server.close().await;
}

#[tokio::test]
async fn sync_to_remote_creates_a_missing_local_root_and_records_a_verified_copy() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let source = server.path.join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data.txt"), b"owned backup bytes").unwrap();
    let remote = server.path.join("new-storage/remote-copies");
    assert!(!remote.exists());
    sqlx::query(
        "INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES(7,'LocalCopy','local',1,?)",
    )
    .bind(remote.to_str().unwrap())
    .execute(server.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,host_path,enabled,compression_enabled,sync_to_remote,remote_storage_ids) VALUES(1,'Source','path',?,1,0,1,'[7]')",
    )
    .bind(source.to_str().unwrap())
    .execute(server.db.pool())
    .await
    .unwrap();
    let (status, _, accepted) = server
        .request(
            "POST",
            "/api/v1/backups",
            &[("Cookie", &cookie), ("Origin", "http://localhost:3000")],
            Some(json!({"target_id":1})),
        )
        .await;
    assert_eq!(status, 200, "{accepted}");
    let id = accepted["id"].as_i64().unwrap();
    let (archive, remote_path) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let row: (String, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
                "SELECT b.status, b.file_path, s.sync_status, s.remote_path FROM backups b LEFT JOIN backup_storage_syncs s ON s.backup_id=b.id WHERE b.id=?",
            )
            .bind(id)
            .fetch_one(server.db.pool())
            .await
            .unwrap();
            assert_ne!(row.0, "FAILED", "LOCAL backup failed: {row:?}");
            if let (state, Some(file), Some(sync), Some(path)) = row
                && state == "COMPLETED" && sync == "completed"
            {
                break (file, path);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let local_bytes = std::fs::read(&archive).unwrap();
    let copied = std::fs::read(remote.join(&remote_path)).unwrap();
    assert_eq!(copied, local_bytes);
    let rows: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT backup_id,storage_id,sync_status FROM backup_storage_syncs WHERE backup_id=?",
    )
    .bind(id)
    .fetch_all(server.db.pool())
    .await
    .unwrap();
    assert_eq!(rows, vec![(id, 7, "completed".to_owned())]);
    server.close().await;
}

#[tokio::test]
async fn sync_to_remote_sqlite_faults_preserve_custody() {
    for reject_intent in [true, false] {
        let server = Server::start("http://localhost:3000").await;
        let cookie = server.setup().await;
        let source = server.path.join("source");
        let remote = server.path.join("remote-copies");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&remote).unwrap();
        std::fs::write(source.join("data.txt"), b"synthetic encrypted sync payload").unwrap();
        let pool = server.db.pool();
        let recipient = age::x25519::Identity::generate().to_public().to_string();
        sqlx::query("INSERT INTO encryption_config(id,public_key,encryption_enabled,setup_completed) VALUES(1,?,1,1)")
            .bind(recipient).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES(7,'LocalCopy','local',1,?)")
            .bind(remote.to_str().unwrap()).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled,compression_enabled,sync_to_remote,remote_storage_ids,delete_local_after_sync) VALUES(1,'Source','path',?,1,1,1,'[7]',1)")
            .bind(source.to_str().unwrap()).execute(pool).await.unwrap();
        let trigger = if reject_intent {
            "CREATE TRIGGER deny_sync BEFORE INSERT ON backup_storage_syncs BEGIN SELECT RAISE(ABORT,'intent unavailable'); END"
        } else {
            sqlx::query("CREATE TABLE sync_settlement_attempts (id INTEGER)")
                .execute(pool)
                .await
                .unwrap();
            "CREATE TRIGGER deny_sync BEFORE UPDATE ON backup_storage_syncs WHEN NEW.sync_status='completed' BEGIN INSERT INTO sync_settlement_attempts VALUES(NEW.backup_id); SELECT RAISE(FAIL,'settlement unavailable'); END"
        };
        sqlx::query(trigger).execute(pool).await.unwrap();
        let (status, _, result) = server
            .request(
                "POST",
                "/api/v1/backups",
                &[("Cookie", &cookie), ("Origin", "http://localhost:3000")],
                Some(json!({"target_id":1})),
            )
            .await;
        assert_eq!(status, 200, "{result}");
        let id = result["id"].as_i64().unwrap();
        let backups = server.path.join("backups");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let published = if reject_intent {
                    std::fs::read_dir(backups.join("Source"))
                        .is_ok_and(|entries| entries.count() == 2)
                } else {
                    sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM sync_settlement_attempts WHERE id=?)",
                    )
                    .bind(id)
                    .fetch_one(pool)
                    .await
                    .unwrap()
                };
                if published {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("archive and sidecar publication must precede the injected SQLite fault");
        server.state.settle_storage().await.unwrap();
        let sync: Option<String> =
            sqlx::query_scalar("SELECT sync_status FROM backup_storage_syncs WHERE backup_id=?")
                .bind(id)
                .fetch_optional(pool)
                .await
                .unwrap();
        if reject_intent {
            assert_eq!(sync, None);
            assert_eq!(std::fs::read_dir(&remote).unwrap().count(), 0);
        } else {
            assert_eq!(sync.as_deref(), Some("running"));
            for file in std::fs::read_dir(backups.join("Source"))
                .unwrap()
                .map(Result::unwrap)
            {
                assert_eq!(
                    std::fs::read(file.path()).unwrap(),
                    std::fs::read(remote.join("Source").join(file.file_name())).unwrap()
                );
            }
        }
        assert_eq!(
            std::fs::read_dir(backups.join("Source")).unwrap().count(),
            2
        );
        sqlx::query("DROP TRIGGER deny_sync")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("UPDATE backup_targets SET remote_storage_ids='[]' WHERE id=1")
            .execute(pool)
            .await
            .unwrap();
        let owner = Owner::new(1).unwrap();
        if !reject_intent {
            let archive: String = sqlx::query_scalar("SELECT file_path FROM backups WHERE id=?")
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
            let bytes = std::fs::read(&archive).unwrap();
            let mut wrong = bytes.clone();
            wrong[0] ^= 1;
            std::fs::write(&archive, wrong).unwrap();
            std::fs::remove_dir_all(remote.join("Source")).unwrap();
            dockervault_service::storage::reconcile_backup_storage(
                pool,
                backups.to_str().unwrap(),
                &owner,
            )
            .await
            .unwrap();
            let pending: String = sqlx::query_scalar(
                "SELECT sync_status FROM backup_storage_syncs WHERE backup_id=?",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
            assert_eq!(pending, "running");
            assert_eq!(
                std::fs::read_dir(&remote).unwrap().count(),
                0,
                "wrong ciphertext must not be recopied"
            );
            std::fs::write(archive, bytes).unwrap();
            // A reserved SQLite writer lets the retry read the old config and
            // publish the pair, but prevents its success write until we repoint it.
            let replacement = server.path.join("replacement");
            std::fs::create_dir(&replacement).unwrap();
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            let (recovered, ()) = tokio::join!(
                dockervault_service::storage::reconcile_backup_storage(
                    pool,
                    backups.to_str().unwrap(),
                    &owner
                ),
                async {
                    tokio::time::timeout(std::time::Duration::from_secs(4), async {
                        loop {
                            if std::fs::read_dir(remote.join("Source"))
                                .is_ok_and(|files| files.count() == 2)
                            {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .unwrap();
                    sqlx::query("UPDATE remote_storages SET base_path=? WHERE id=7")
                        .bind(replacement.to_str().unwrap())
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                    tx.commit().await.unwrap();
                }
            );
            recovered.unwrap();
            let pending: String = sqlx::query_scalar(
                "SELECT sync_status FROM backup_storage_syncs WHERE backup_id=?",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
            assert_eq!(
                pending, "running",
                "a copy to the old destination cannot complete the new destination"
            );
            assert_eq!(std::fs::read_dir(&replacement).unwrap().count(), 0);
            sqlx::query("UPDATE remote_storages SET base_path=? WHERE id=7")
                .bind(remote.to_str().unwrap())
                .execute(pool)
                .await
                .unwrap();
        }
        sqlx::query("CREATE TRIGGER deny_cleanup_receipt BEFORE UPDATE OF backup_metadata ON backups WHEN json_type(OLD.backup_metadata,'$.native_local_sync')='object' AND json_type(NEW.backup_metadata,'$.native_local_sync') IS NULL BEGIN SELECT RAISE(ABORT,'cleanup settlement unavailable'); END")
            .execute(pool).await.unwrap();
        let result = dockervault_service::storage::reconcile_backup_storage(
            pool,
            backups.to_str().unwrap(),
            &owner,
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cleanup settlement unavailable")
        );
        assert_eq!(
            std::fs::read_dir(backups.join("Source")).unwrap().count(),
            0
        );
        let retained: bool = sqlx::query_scalar("SELECT json_type(backup_metadata,'$.native_local_sync')='object' FROM backups WHERE id=?")
            .bind(id).fetch_one(pool).await.unwrap();
        assert!(
            retained,
            "cleanup cannot lose its obligation when SQLite rejects settlement"
        );
        sqlx::query("DROP TRIGGER deny_cleanup_receipt")
            .execute(pool)
            .await
            .unwrap();
        for _ in 0..2 {
            dockervault_service::storage::reconcile_backup_storage(
                pool,
                backups.to_str().unwrap(),
                &owner,
            )
            .await
            .unwrap();
            let rows: Vec<(i64, String)> = sqlx::query_as(
                "SELECT storage_id,sync_status FROM backup_storage_syncs WHERE backup_id=?",
            )
            .bind(id)
            .fetch_all(pool)
            .await
            .unwrap();
            assert_eq!(rows, [(7, "completed".into())]);
        }
        assert_eq!(std::fs::read_dir(remote.join("Source")).unwrap().count(), 2);
        let retained: bool = sqlx::query_scalar("SELECT json_type(backup_metadata,'$.native_local_sync') IS NOT NULL FROM backups WHERE id=?")
            .bind(id).fetch_one(pool).await.unwrap();
        assert!(
            !retained,
            "verified cleanup and SQLite settlement clear the obligation"
        );
        assert_eq!(
            std::fs::read_dir(backups.join("Source")).unwrap().count(),
            0
        );
        owner.shutdown(None).await;
        server.close().await;
    }
}

#[tokio::test]
#[ignore = "requires packaged OpenSSH and the installed process guard"]
async fn sftp_backup_durably_admits_the_copy_before_provider_io() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let source = server.path.join("sftp-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data"), b"synthetic SFTP admission fixture").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pool = server.db.pool();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,host,port,username,base_path) VALUES(7,'SFTP fixture','sftp',1,'127.0.0.1',?,'dvfixture','/copies')")
        .bind(listener.local_addr().unwrap().port()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled,sync_to_remote,remote_storage_ids) VALUES(1,'SftpTarget','path',?,1,1,'[7]')")
        .bind(source.to_str().unwrap()).execute(pool).await.unwrap();
    let (status, _, accepted) = server
        .request(
            "POST",
            "/api/v1/backups",
            &[("Cookie", &cookie), ("Origin", "http://localhost:3000")],
            Some(json!({"target_id":1})),
        )
        .await;
    assert_eq!(status, 200, "{accepted}");
    let id = accepted["id"].as_i64().unwrap();
    let (socket, _) = tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let intent: String = sqlx::query_scalar(
        "SELECT sync_status FROM backup_storage_syncs WHERE backup_id=? AND storage_id=7",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(intent, "running");
    drop(socket);
    drop(listener);
    server.close().await;
}

#[tokio::test]
async fn recovery_export_is_admin_only_and_returns_an_encrypted_read_only_snapshot() {
    let _kdf = RECOVERY_KDF_TEST.lock().await;
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", "http://localhost:3000"),
    ];
    let route = "/api/v1/recovery/export";
    let password = "PUBLIC_TEST_EXPORT_PASSWORD";
    let valid = json!({"password":password,"confirm_password":password});
    assert_eq!(
        server
            .request("POST", route, &[], Some(valid.clone()))
            .await
            .0,
        401
    );
    for (request, detail) in [
        (json!({"password":password}), "Invalid recovery request"),
        (
            json!({"password":password,"confirm_password":"different"}),
            "Enter and confirm the dedicated system-backup password",
        ),
    ] {
        let (status, _, body) = server.request("POST", route, &headers, Some(request)).await;
        assert_eq!((status, body), (400, json!({"detail":detail})));
    }
    let db = server.db.pool();
    sqlx::raw_sql(
        "INSERT INTO backup_targets (id,name,target_type) VALUES (1,'Export fixture','path');
        INSERT INTO backups (id,target_id,status,backup_type) VALUES (2,1,'COMPLETED','FULL');",
    )
    .execute(db)
    .await
    .unwrap();
    let (status, response, envelope) = server.request("POST", route, &headers, Some(valid)).await;
    assert_eq!(status, 200);
    for header in [
        "cache-control: no-store",
        "pragma: no-cache",
        "x-content-type-options: nosniff",
        "content-type: application/octet-stream",
        "content-disposition: attachment; filename=\"dockervault-system-backup.dockervault\"",
    ] {
        assert!(response.contains(header), "missing {header}");
    }
    let bytes = serde_json::to_vec(&envelope).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("Export fixture"));
    let catalog = dockervault_core::recovery_bundle::decode_bundle(&bytes, password).unwrap();
    assert_eq!(catalog["tables"]["backups"][0]["id"], 2);
    assert!(catalog["tables"].get("users").is_none());
    assert!(catalog["tables"].get("sessions").is_none());
    assert_eq!(
        catalog["manifest"],
        dockervault_core::recovery_state::state_manifest(&catalog).unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM backups WHERE id=2")
            .fetch_one(db)
            .await
            .unwrap(),
        "COMPLETED"
    );
    sqlx::query("UPDATE users SET is_admin=0")
        .execute(db)
        .await
        .unwrap();
    let (status, _, body) = server
        .request(
            "POST",
            route,
            &headers,
            Some(json!({"password":password,"confirm_password":password})),
        )
        .await;
    assert_eq!(status, 403);
    assert_eq!(body, json!({"detail":"Administrator access required"}));
    server.close().await;
}

#[tokio::test]
async fn recovery_preview_is_private_read_only_and_available_only_before_setup() {
    let _kdf = RECOVERY_KDF_TEST.lock().await;
    let server = Server::configured(Config::from_pairs([("TZ", "UTC")]).unwrap()).await;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../dockervault-core/tests/fixtures/recovery-state.TEST_ONLY.json"
    ))
    .unwrap();
    let baseline = &fixture["cases"][0];
    let password = "PUBLIC_TEST_RECOVERY_PASSWORD_NOT_A_CREDENTIAL";
    let encoded =
        dockervault_core::recovery_bundle::encode_bundle(baseline["payload"].clone(), password)
            .unwrap();
    let bundle = String::from_utf8(encoded).unwrap();
    let route = "/api/v1/recovery/preview";
    let (status, headers, body) = server
        .request(
            "POST",
            route,
            &[],
            Some(json!({"bundle":bundle,"password":password})),
        )
        .await;
    assert_eq!(status, 200);
    assert!(headers.contains("cache-control: no-store"));
    assert!(headers.contains("pragma: no-cache"));
    assert_eq!(body, baseline["preview"]);
    let (status, headers, body) = server
        .request(
            "POST",
            route,
            &[],
            Some(json!({"bundle":bundle,"password":"wrong public fixture"})),
        )
        .await;
    assert_eq!(status, 400);
    assert!(headers.contains("cache-control: no-store"));
    assert_eq!(
        body,
        json!({"detail":"Invalid or incompatible recovery bundle or password"})
    );
    let (status, _, body) = server
        .request("POST", route, &[], Some(json!({"bundle":bundle})))
        .await;
    assert_eq!(status, 400);
    assert_eq!(body, json!({"detail":"Invalid recovery request"}));
    for query in [
        "SELECT COUNT(*) FROM users",
        "SELECT COUNT(*) FROM backup_targets",
        "SELECT COUNT(*) FROM backups",
        "SELECT COUNT(*) FROM remote_storages",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(query)
                .fetch_one(server.db.pool())
                .await
                .unwrap(),
            0
        );
    }
    assert!(
        std::fs::read_dir(server.path.join("backups"))
            .unwrap()
            .next()
            .is_none()
    );
    server.setup().await;
    let (status, headers, body) = server.request("POST", route, &[], None).await;
    assert_eq!(status, 403);
    assert!(headers.contains("cache-control: no-store"));
    assert_eq!(
        body,
        json!({"detail":"Recovery is only available before initial setup"})
    );
    server.close().await;
}

#[tokio::test]
async fn legacy_bulk_backup_delete_requires_frozen_confirmation() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let db = server.db.pool();
    let archive = server.path.join("backups/legacy.tar");
    std::fs::write(&archive, b"retained fixture").unwrap();
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,enabled) VALUES(1,'Fixture','path',1)",
    )
    .execute(db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path) VALUES(1,1,'FULL','COMPLETED',?)")
        .bind(archive.to_str().unwrap()).execute(db).await.unwrap();
    let route = "/api/v1/backups";
    let (status, _, _) = server.request("DELETE", route, &[], None).await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let (first, _, body) = server.request("DELETE", route, &headers, None).await;
    assert_eq!(first, 409);
    assert_eq!(status, 401);
    assert_eq!(
        body,
        json!({"detail":"Review POST /backups/removal/preview, then execute its token; unconfirmed bulk deletion is disabled"})
    );
    let (status, _, body) = server
        .request(
            "DELETE",
            "/api/v1/backups?target_id=no&delete_local=maybe&delete_remote=off",
            &headers,
            None,
        )
        .await;
    assert_eq!(status, 422);
    assert_eq!(
        body,
        json!({"detail":"Validation error","errors":[{"field":"query.target_id","message":"Input should be a valid integer, unable to parse string as an integer"},{"field":"query.delete_local","message":"Input should be a valid boolean, unable to interpret input"}]})
    );
    for _ in 0..9 {
        assert_eq!(
            server
                .request(
                    "DELETE",
                    "/api/v1/backups?target_id=1.0&delete_local=OFF&delete_remote=Yes",
                    &headers,
                    None
                )
                .await
                .0,
            409
        );
    }
    let (status, _, body) = server.request("DELETE", route, &headers, None).await;
    assert_eq!(status, 429);
    assert_eq!(
        body,
        json!({"error":"Rate limit exceeded: 10 per 1 minute"})
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT file_path FROM backups WHERE id=1")
            .fetch_one(db)
            .await
            .unwrap(),
        archive.to_str().unwrap()
    );
    assert_eq!(std::fs::read(archive).unwrap(), b"retained fixture");
    server.close().await;
}

#[tokio::test]
async fn removal_preview_selects_all_matching_copies_without_writing() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,enabled) VALUES(1,'Fixture','path',1)",
    )
    .execute(server.db.pool())
    .await
    .unwrap();
    let backup_root = server.path.join("backups");
    let insert = |id| {
        let path = backup_root.join(format!("item-{id}.tar"));
        std::fs::write(&path, b"owned fixture").unwrap();
        sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path) VALUES(?,1,'FULL','COMPLETED',?)").bind(id).bind(path.to_str().unwrap().to_owned()).execute(server.db.pool())
    };
    for id in 1..=505 {
        insert(id).await.unwrap();
    }
    let route = "/api/v1/backups/removal/preview";
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let selection = json!({"scope":"local","target_id":1,"status":"completed"});
    assert_eq!(
        server
            .request("POST", route, &[], Some(selection.clone()))
            .await
            .0,
        401
    );
    let (status, _, value) = server
        .request("POST", route, &headers, Some(selection))
        .await;
    assert_eq!(status, 200);
    assert_eq!(value["archive_count"], 505);
    assert_eq!(value["copy_count"], 505);
    assert_eq!(value["expires_in_seconds"], 900);
    assert_eq!(value["items"].as_array().unwrap().len(), 505);
    assert_eq!(value["items"][504]["backup_id"], 505);
    assert_eq!(
        server
            .request(
                "POST",
                route,
                &headers,
                Some(json!({"scope":"local","target_id":1,"backup_ids":[1]}))
            )
            .await
            .0,
        422
    );
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM backups WHERE file_path IS NOT NULL")
            .fetch_one(server.db.pool())
            .await
            .unwrap();
    assert_eq!(remaining, 505);
    insert(506).await.unwrap();
    let body = json!({"token":value["token"]});
    let execute = "/api/v1/backups/removal/execute";
    let (status, _, result) = server
        .request("POST", execute, &headers, Some(body.clone()))
        .await;
    assert_eq!(status, 200);
    assert_eq!(result["status"], "completed");
    assert_eq!(result["succeeded"], 505);
    assert_eq!(
        server
            .request("POST", execute, &headers, Some(body))
            .await
            .2,
        result
    );
    let remaining: Vec<i64> = sqlx::query_scalar("SELECT id FROM backups ORDER BY id")
        .fetch_all(server.db.pool())
        .await
        .unwrap();
    assert_eq!(remaining, vec![506]);
    assert_eq!(
        std::fs::read(backup_root.join("item-506.tar")).unwrap(),
        b"owned fixture"
    );
    assert_eq!(std::fs::read_dir(backup_root).unwrap().count(), 1);
    server.close().await;
}

#[tokio::test]
async fn retention_applies_only_to_expired_local_copies_and_counts_settled_changes() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let endpoint = "/api/v1/retention/1/apply";
    assert_eq!(server.request("POST", endpoint, &[], None).await.0, 401);
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO retention_policies(id,name,keep_last,keep_daily,keep_weekly,keep_monthly,keep_yearly,max_age_days) VALUES(123,'GFS',2,7,4,6,2,1); INSERT INTO backup_targets(id,name,target_type,enabled,retention_policy_id) VALUES(1,'Fixture','path',1,123); INSERT INTO backup_targets(id,name,target_type,enabled,removed_at) VALUES(2,'Removed','path',0,'2020-01-01T00:00:00'); INSERT INTO remote_storages(id,name,storage_type,base_path) VALUES(123,'Replica','local','/unused');").execute(db).await.unwrap();
    let archive = |id| server.path.join(format!("backups/gfs-{id}.tar"));
    let key = |id| server.path.join(format!("backups/gfs-{id}.key"));
    for (id, target, status) in [
        (1, 1, "COMPLETED"),
        (2, 1, "COMPLETED"),
        (3, 1, "COMPLETED"),
        (4, 1, "COMPLETED"),
        (5, 1, "FAILED"),
        (6, 2, "COMPLETED"),
    ] {
        std::fs::write(archive(id), b"archive").unwrap();
        std::fs::write(key(id), b"sidecar").unwrap();
        sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path,encryption_key_path,created_at,encrypted) VALUES(?,?,'FULL',?,?,?,?,1)").bind(id).bind(target).bind(status).bind(archive(id).to_str()).bind(key(id).to_str()).bind(format!("2020-01-0{}T00:00:00",7-id)).execute(db).await.unwrap();
    }
    sqlx::query("INSERT INTO backup_storage_syncs(backup_id,storage_id,sync_status,remote_path) VALUES(4,123,'failed','replica.tar')").execute(db).await.unwrap();
    let shared = server.path.join("backups/shared-replica.tar");
    std::fs::write(&shared, b"another backup's replica").unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path,created_at) VALUES(7,1,'FULL','COMPLETED',?,'2020-01-01T00:00:00')")
        .bind(shared.to_str()).execute(db).await.unwrap();
    sqlx::query("INSERT INTO backup_storage_syncs(backup_id,storage_id,sync_status,remote_path) VALUES(1,123,'completed',?)")
        .bind(shared.to_str()).execute(db).await.unwrap();
    let (_, _, removed) = server
        .request("POST", "/api/v1/retention/2/apply", &headers, None)
        .await;
    assert_eq!(removed, json!({"detail":"Plan removed"}));
    sqlx::query("INSERT INTO app_settings(key,value) VALUES('recovery_activation','{}')")
        .execute(db)
        .await
        .unwrap();
    assert_eq!(
        server.request("POST", endpoint, &headers, None).await.0,
        503
    );
    assert!(archive(3).exists() && key(3).exists() && archive(4).exists());
    sqlx::query("DELETE FROM app_settings WHERE key='recovery_activation'")
        .execute(db)
        .await
        .unwrap();
    let (status, _, result) = server.request("POST", endpoint, &headers, None).await;
    assert_eq!((status, result), (200, json!({"kept":2,"deleted":2})));
    for id in [1, 2, 5, 6] {
        assert!(archive(id).exists() && key(id).exists());
    }
    assert!(!archive(3).exists() && !key(3).exists());
    assert!(!archive(4).exists() && key(4).exists());
    let rows: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id,file_path FROM backups ORDER BY id")
            .fetch_all(db)
            .await
            .unwrap();
    assert_eq!(
        rows.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![1, 2, 4, 5, 6, 7]
    );
    assert_eq!(std::fs::read(&shared).unwrap(), b"another backup's replica");
    assert_eq!(rows[2], (4, None));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT remote_path FROM backup_storage_syncs WHERE backup_id=4"
        )
        .fetch_one(db)
        .await
        .unwrap(),
        "replica.tar"
    );
    let (status, _, result) = server.request("POST", endpoint, &headers, None).await;
    assert_eq!((status, result), (200, json!({"kept":2,"deleted":0})));
    server.close().await;
}

#[tokio::test]
async fn legacy_delete_removes_only_the_selected_local_copy() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let selected = server.path.join("backups/selected.tar");
    let retained = server.path.join("backups/retained.tar");
    std::fs::write(&selected, b"disposable selected archive").unwrap();
    std::fs::write(&retained, b"separate retained archive").unwrap();
    let db = server.db.pool();
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,enabled) VALUES(1,'Fixture','path',1)",
    )
    .execute(db)
    .await
    .unwrap();
    for (id, path) in [(1, &selected), (2, &retained)] {
        sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path) VALUES(?,1,'FULL','COMPLETED',?)")
            .bind(id).bind(path.to_str().unwrap()).execute(db).await.unwrap();
    }
    let route = "/api/v1/backups/1?delete_local=true&delete_remote=false";
    assert_eq!(server.request("DELETE", route, &[], None).await.0, 401);
    assert!(selected.exists());
    let (status, _, result) = server.request("DELETE", route, &headers, None).await;
    assert_eq!(status, 200);
    assert_eq!(result["succeeded"], 1);
    assert_eq!(result["failed"], 0);
    assert!(!selected.exists());
    assert_eq!(
        std::fs::read(retained).unwrap(),
        b"separate retained archive"
    );
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM backups ORDER BY id")
        .fetch_all(db)
        .await
        .unwrap();
    assert_eq!(ids, [2]);
    assert_eq!(server.request("DELETE", route, &headers, None).await.0, 404);
    server.close().await;
}

#[tokio::test]
async fn removal_keeps_durable_intent_when_catalog_acknowledgment_fails() {
    removal_acknowledgment_failure(true, false).await;
}

#[tokio::test]
async fn removal_retry_resumes_durable_intent_once() {
    removal_acknowledgment_failure(false, false).await;
}

#[tokio::test]
async fn removal_does_not_report_success_for_an_ignored_primary_catalog_delete() {
    removal_acknowledgment_failure(true, true).await;
}

async fn removal_acknowledgment_failure(restart: bool, ignore: bool) {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let path = server.path.join("backups/disposable.tar");
    std::fs::write(&path, b"discardable fixture").unwrap();
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,enabled) VALUES(1,'Fixture','path',1)",
    )
    .execute(server.db.pool())
    .await
    .unwrap();
    let fixture = if ignore {
        "CREATE TRIGGER refuse_removal BEFORE DELETE ON backups BEGIN SELECT RAISE(IGNORE); END"
    } else {
        "CREATE TRIGGER refuse_removal BEFORE DELETE ON backups BEGIN SELECT RAISE(ABORT,'fixture refused catalog acknowledgment'); END"
    };
    sqlx::query(fixture)
        .execute(server.db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path) VALUES(1,1,'FULL','COMPLETED',?)").bind(path.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    let (status, _, preview) = server
        .request(
            "POST",
            "/api/v1/backups/removal/preview",
            &headers,
            Some(json!({"scope":"local","backup_ids":[1]})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, _, result) = server
        .request(
            "POST",
            "/api/v1/backups/removal/execute",
            &headers,
            Some(json!({"token":preview["token"]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(result["failed"], 1);
    assert!(result["retry_token"].is_string());
    let pending: bool=sqlx::query_scalar("SELECT json_type(backup_metadata,'$.native_copy_removal')='object' FROM backups WHERE id=1").fetch_one(server.db.pool()).await.unwrap();
    assert!(pending);
    assert!(!path.exists());
    sqlx::query("DROP TRIGGER refuse_removal")
        .execute(server.db.pool())
        .await
        .unwrap();
    let reopened = if restart {
        let config = (*server.state.config).clone();
        let dir = server.stop_preserving_files().await;
        Server::open_at(config, dir).await
    } else {
        let execute = "/api/v1/backups/removal/execute";
        let retry = json!({"token":result["retry_token"]});
        let (status, _, settled) = server
            .request("POST", execute, &headers, Some(retry.clone()))
            .await;
        assert_eq!(status, 200);
        assert_eq!(settled["succeeded"], 1);
        assert_eq!(
            server
                .request("POST", execute, &headers, Some(retry))
                .await
                .2,
            settled
        );
        assert_eq!(
            server
                .request(
                    "POST",
                    execute,
                    &headers,
                    Some(json!({"token":preview["token"]}))
                )
                .await
                .2,
            result
        );
        server
    };
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM backups")
        .fetch_one(reopened.db.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0);
    assert!(!path.exists());
    reopened.close().await;
}

#[tokio::test]
async fn remote_removal_checks_frozen_destination_before_provider_io() {
    replica_removal(ReplicaDrift::Destination).await;
}

#[tokio::test]
async fn remote_removal_checks_frozen_sync_path_before_provider_io() {
    replica_removal(ReplicaDrift::Path).await;
}

#[tokio::test]
async fn remote_removal_checks_frozen_encryption_before_provider_io() {
    replica_removal(ReplicaDrift::Encryption).await;
}

enum ReplicaDrift {
    Destination,
    Path,
    Encryption,
    SharedSidecar,
}

#[tokio::test]
async fn remote_removal_retains_a_sidecar_shared_with_another_catalog_entry() {
    replica_removal(ReplicaDrift::SharedSidecar).await;
}

async fn replica_removal(drift: ReplicaDrift) {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let copy = server.path.join("copy");
    let replacement = server.path.join("replacement");
    std::fs::create_dir(&copy).unwrap();
    std::fs::create_dir(&replacement).unwrap();
    std::fs::write(copy.join("fixture.tar"), b"copy").unwrap();
    std::fs::write(copy.join("replacement.tar"), b"new catalog path").unwrap();
    std::fs::write(replacement.join("fixture.tar"), b"other copy").unwrap();
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO backup_targets(id,name,target_type,enabled) VALUES(1,'Fixture','path',1); INSERT INTO backups(id,target_id,backup_type,status) VALUES(1,1,'FULL','COMPLETED');").execute(db).await.unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,base_path,enabled) VALUES(1,'Replica','local',?,1)").bind(copy.to_str().unwrap()).execute(db).await.unwrap();
    sqlx::query("INSERT INTO backup_storage_syncs(id,backup_id,storage_id,remote_path,sync_status) VALUES(1,1,1,'fixture.tar','completed')").execute(db).await.unwrap();
    let shared = matches!(drift, ReplicaDrift::SharedSidecar);
    let sidecar = "fixture.tar.key";
    if shared {
        sqlx::query("UPDATE backups SET encrypted=1 WHERE id=1")
            .execute(db)
            .await
            .unwrap();
        std::fs::write(copy.join(sidecar), b"shared bytes").unwrap();
    }
    let (status, _, preview) = server
        .request(
            "POST",
            "/api/v1/backups/removal/preview",
            &headers,
            Some(json!({"scope":"remote","backup_ids":[1]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(preview["copy_count"], 1);
    match drift {
        ReplicaDrift::Destination => {
            sqlx::query("UPDATE remote_storages SET base_path=? WHERE id=1")
                .bind(replacement.to_str().unwrap())
                .execute(db)
                .await
                .unwrap()
        }
        ReplicaDrift::Path => {
            sqlx::query("UPDATE backup_storage_syncs SET remote_path='replacement.tar' WHERE id=1")
                .execute(db)
                .await
                .unwrap()
        }
        ReplicaDrift::Encryption => sqlx::query("UPDATE backups SET encrypted=1 WHERE id=1")
            .execute(db)
            .await
            .unwrap(),
        ReplicaDrift::SharedSidecar => {
            sqlx::query("INSERT INTO backups(id,target_id,backup_type,status) VALUES(2,1,'FULL','COMPLETED')").execute(db).await.unwrap();
            sqlx::query("INSERT INTO backup_storage_syncs(id,backup_id,storage_id,remote_path,sync_status) VALUES(2,2,1,?,'completed')")
                .bind(sidecar).execute(db).await.unwrap()
        }
    };
    let (status, _, result) = server
        .request(
            "POST",
            "/api/v1/backups/removal/execute",
            &headers,
            Some(json!({"token":preview["token"]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(result["skipped"], 1);
    assert_eq!(result["succeeded"], 0);
    assert!(result["retry_token"].is_null());
    assert_eq!(
        std::fs::read(replacement.join("fixture.tar")).unwrap(),
        b"other copy"
    );
    assert_eq!(std::fs::read(copy.join("fixture.tar")).unwrap(), b"copy");
    assert_eq!(
        std::fs::read(copy.join("replacement.tar")).unwrap(),
        b"new catalog path"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_storage_syncs")
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!(count, if shared { 2 } else { 1 });
    if shared {
        assert_eq!(std::fs::read(copy.join(sidecar)).unwrap(), b"shared bytes");
    }
    server.close().await;
}

#[tokio::test]
async fn encrypted_local_replica_removal_settles_files_before_catalog_ack() {
    replica_removal_with_catalog_rejection(
        "CREATE TRIGGER reject_ack BEFORE DELETE ON backup_storage_syncs BEGIN SELECT RAISE(ABORT,'fixture'); END",
        true,
    )
    .await;
}

#[tokio::test]
async fn encrypted_local_replica_removal_requires_a_deleted_sync_row() {
    replica_removal_with_catalog_rejection(
        "CREATE TRIGGER reject_ack BEFORE DELETE ON backup_storage_syncs BEGIN SELECT RAISE(IGNORE); END",
        true,
    )
    .await;
}

#[tokio::test]
async fn encrypted_local_replica_removal_requires_a_cleared_intent() {
    replica_removal_with_catalog_rejection("CREATE TRIGGER reject_ack BEFORE UPDATE OF backup_metadata ON backups WHEN json_type(OLD.backup_metadata,'$.native_copy_removal')='object' BEGIN SELECT RAISE(IGNORE); END", true).await;
}

#[tokio::test]
async fn encrypted_local_replica_removal_requires_a_durable_intent_before_unlink() {
    replica_removal_with_catalog_rejection("CREATE TRIGGER reject_ack BEFORE UPDATE OF backup_metadata ON backups WHEN json_type(NEW.backup_metadata,'$.native_copy_removal')='object' BEGIN SELECT RAISE(IGNORE); END", false).await;
}

async fn replica_removal_with_catalog_rejection(trigger: &'static str, unlinked: bool) {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let replica = server.path.join("replica");
    std::fs::create_dir(&replica).unwrap();
    let primary = server.path.join("backups");
    for (root, data) in [(&replica, b"replica"), (&primary, b"primary")] {
        for name in ["fixture.tar.enc", "fixture.tar.key"] {
            std::fs::write(root.join(name), data).unwrap();
        }
    }
    let db = server.db.pool();
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,enabled) VALUES(1,'Fixture','path',1)",
    )
    .execute(db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,encrypted,file_path,encryption_key_path) VALUES(1,1,'FULL','COMPLETED',1,?,?)")
        .bind(primary.join("fixture.tar.enc").to_str().unwrap()).bind(primary.join("fixture.tar.key").to_str().unwrap()).execute(db).await.unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,base_path,enabled) VALUES(1,'Replica','local',?,1)").bind(replica.to_str().unwrap()).execute(db).await.unwrap();
    sqlx::query("INSERT INTO backup_storage_syncs(id,backup_id,storage_id,remote_path,sync_status) VALUES(1,1,1,'fixture.tar.enc','completed')").execute(db).await.unwrap();
    sqlx::query(trigger).execute(db).await.unwrap();
    let (status, _, preview) = server
        .request(
            "POST",
            "/api/v1/backups/removal/preview",
            &headers,
            Some(json!({"scope":"remote","backup_ids":[1]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(preview["copy_count"], 1);
    let (status, _, result) = server
        .request(
            "POST",
            "/api/v1/backups/removal/execute",
            &headers,
            Some(json!({"token":preview["token"]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(result["status"], "incomplete");
    assert_eq!(result["succeeded"], 0);
    for name in ["fixture.tar.enc", "fixture.tar.key"] {
        assert_eq!(
            !replica.join(name).exists(),
            unlinked,
            "replica member {name}"
        );
        assert_eq!(std::fs::read(primary.join(name)).unwrap(), b"primary");
    }
    let count =
        || sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM backup_storage_syncs").fetch_one(db);
    assert_eq!(count().await.unwrap(), 1);
    let pending: bool = sqlx::query_scalar("SELECT coalesce(json_type(backup_metadata,'$.native_copy_removal')='object',0) FROM backups WHERE id=1").fetch_one(db).await.unwrap();
    assert_eq!(pending, unlinked);
    assert!(result["retry_token"].is_string());
    sqlx::query("DROP TRIGGER reject_ack")
        .execute(db)
        .await
        .unwrap();
    let (status, _, retry) = server
        .request(
            "POST",
            "/api/v1/backups/removal/execute",
            &headers,
            Some(json!({"token":result["retry_token"]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(retry["succeeded"], 1);
    assert_eq!(count().await.unwrap(), 0);
    let (archive, key): (String, String) =
        sqlx::query_as("SELECT file_path,encryption_key_path FROM backups WHERE id=1")
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!(archive, primary.join("fixture.tar.enc").to_str().unwrap());
    assert_eq!(key, primary.join("fixture.tar.key").to_str().unwrap());
    for name in ["fixture.tar.enc", "fixture.tar.key"] {
        assert_eq!(std::fs::read(primary.join(name)).unwrap(), b"primary");
    }
    server.close().await;
}

#[tokio::test]
async fn removal_does_not_unlink_replaced_or_shared_archives() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let root = server.path.join("backups");
    let changed = root.join("changed.tar");
    let shared = root.join("shared.tar");
    std::fs::write(&changed, b"old").unwrap();
    std::fs::write(&shared, b"both").unwrap();
    sqlx::query(
        "INSERT INTO backup_targets(id,name,target_type,enabled) VALUES(1,'Fixture','path',1)",
    )
    .execute(server.db.pool())
    .await
    .unwrap();
    for (id, path) in [(1, &changed), (2, &shared), (3, &shared)] {
        sqlx::query("INSERT INTO backups(id,target_id,backup_type,status,file_path) VALUES(?,1,'FULL','COMPLETED',?)").bind(id).bind(path.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    }
    let (status, _, preview) = server
        .request(
            "POST",
            "/api/v1/backups/removal/preview",
            &headers,
            Some(json!({"scope":"local","backup_ids":[1,2]})),
        )
        .await;
    assert_eq!(status, 200);
    std::fs::write(root.join("replacement"), b"new").unwrap();
    std::fs::rename(root.join("replacement"), &changed).unwrap();
    let (status, _, result) = server
        .request(
            "POST",
            "/api/v1/backups/removal/execute",
            &headers,
            Some(json!({"token":preview["token"]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(result["status"], "incomplete");
    assert_eq!(result["skipped"], 2);
    assert_eq!(std::fs::read(changed).unwrap(), b"new");
    assert_eq!(std::fs::read(shared).unwrap(), b"both");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM backups")
        .fetch_one(server.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 3);
    server.close().await;
}

#[tokio::test]
async fn backup_logs_and_stats_preserve_legacy_catalog_contract() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let db = server.db.pool();
    sqlx::raw_sql("INSERT INTO backup_targets(id,name,target_type,enabled) VALUES(1,'Fixture','path',1); INSERT INTO backups(id,target_id,backup_type,status,file_size,duration_seconds) VALUES(3,1,'FULL','COMPLETED',2048,2),(4,1,'FULL','PENDING',NULL,NULL); INSERT INTO backup_logs(id,backup_id,level,step,message,details,created_at) VALUES(10,3,'WARNING','upload','second',NULL,'2026-01-02 10:05:00'),(11,3,'INFO','archive','first','{\"count\":2}','2026-01-02 10:00:00');").execute(db).await.unwrap();
    let headers = [("Cookie", cookie.as_str())];
    let (status, _, _) = server
        .request("GET", "/api/v1/backups/3/logs", &[], None)
        .await;
    assert_eq!(status, 401);
    let (status, _, logs) = server
        .request("GET", "/api/v1/backups/3/logs", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        logs,
        json!([{"id":11,"level":"info","step":"archive","message":"first","details":{"count":2},"created_at":"2026-01-02T10:00:00"},{"id":10,"level":"warning","step":"upload","message":"second","details":null,"created_at":"2026-01-02T10:05:00"}])
    );
    let (status, _, stats) = server
        .request("GET", "/api/v1/backups/3/stats", &headers, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        stats,
        json!({"backup_id":3,"file_size":2048,"file_size_human":"2.00 KB","duration_seconds":2,"status":"completed"})
    );
    let (status, _, logs) = server
        .request("GET", "/api/v1/backups/4/logs", &headers, None)
        .await;
    assert_eq!((status, logs), (200, json!([])));
    for path in ["/api/v1/backups/99/stats", "/api/v1/backups/99/logs"] {
        let (status, _, body) = server.request("GET", path, &headers, None).await;
        assert_eq!((status, body), (404, json!({"detail":"Backup not found"})));
    }
    server.close().await;
}

#[tokio::test]
async fn ftp_backup_durably_admits_unique_catalog_path_before_provider_io() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let source = server.path.join("ftp-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data"), b"synthetic FTP admission fixture").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pool = server.db.pool();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,host,port,username,base_path) VALUES(7,'FTP fixture','ftp',1,'127.0.0.1',?,'dvfixture','/copies')")
        .bind(listener.local_addr().unwrap().port()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled,sync_to_remote,remote_storage_ids) VALUES(1,'FtpTarget','path',?,1,1,'[7]')")
        .bind(source.to_str().unwrap()).execute(pool).await.unwrap();
    let (status, _, accepted) = server
        .request(
            "POST",
            "/api/v1/backups",
            &[("Cookie", &cookie), ("Origin", "http://localhost:3000")],
            Some(json!({"target_id":1})),
        )
        .await;
    assert_eq!(status, 200, "{accepted}");
    let id = accepted["id"].as_i64().unwrap();
    let (socket, _) = tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let (status, path): (String, String) = sqlx::query_as("SELECT sync_status,remote_path FROM backup_storage_syncs WHERE backup_id=? AND storage_id=7")
        .bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(status, "running");
    let (claim, archive) = path.split_once('/').unwrap();
    assert_eq!(claim.strip_prefix("dockervault-").unwrap().len(), 32);
    assert!(!archive.contains('/') && archive.contains(".tar"));
    drop(socket);
    drop(listener);
    server.close().await;
}

#[tokio::test]
async fn webdav_backup_durably_admits_the_copy_before_provider_io() {
    webdav_admission(false).await;
}

#[tokio::test]
async fn webdav_known_insufficient_quota_prevents_upload() {
    webdav_admission(true).await;
}

async fn webdav_admission(check_quota: bool) {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let source = server.path.join("dav-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data"), b"synthetic WebDAV admission fixture").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pool = server.db.pool();
    if check_quota {
        sqlx::query("INSERT INTO app_settings(key,value) VALUES('storage_check_quota_before_upload','true')").execute(pool).await.unwrap();
    }
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,webdav_url,base_path) VALUES(7,'DAV fixture','webdav',1,?,'/copies')")
        .bind(format!("http://{}/dav", listener.local_addr().unwrap())).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled,sync_to_remote,remote_storage_ids) VALUES(1,'DavTarget','path',?,1,1,'[7]')")
        .bind(source.to_str().unwrap()).execute(pool).await.unwrap();
    let (status, _, accepted) = server
        .request(
            "POST",
            "/api/v1/backups",
            &[("Cookie", &cookie), ("Origin", "http://localhost:3000")],
            Some(json!({"target_id":1})),
        )
        .await;
    assert_eq!(status, 200, "{accepted}");
    let id = accepted["id"].as_i64().unwrap();
    let (socket, _) = tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let intent: String = sqlx::query_scalar(
        "SELECT sync_status FROM backup_storage_syncs WHERE backup_id=? AND storage_id=7",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(intent, "running");
    if check_quota {
        let mut socket = socket;
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            assert!(request.len() < 8192);
            request.push(socket.read_u8().await.unwrap());
        }
        assert!(
            request.starts_with(b"PROPFIND "),
            "{}",
            String::from_utf8_lossy(&request)
        );
        let body = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/copies/</d:href><d:propstat><d:prop><d:quota-available-bytes>0</d:quota-available-bytes></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
        socket.write_all(format!("HTTP/1.1 207 Multi-Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(4),async {
            loop {
                let (status,archive,uncertain):(String,String,bool) = sqlx::query_as("SELECT s.sync_status,b.file_path,json_type(b.backup_metadata,'$.native_webdav_effects') IS NOT NULL FROM backup_storage_syncs s JOIN backups b ON b.id=s.backup_id WHERE s.backup_id=?").bind(id).fetch_one(pool).await.unwrap();
                if status == "failed" {
                    assert!(std::path::Path::new(&archive).is_file());
                    assert!(!uncertain);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }).await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept())
                .await
                .is_err()
        );
    } else {
        drop(socket);
    }
    drop(listener);
    server.close().await;
}

#[tokio::test]
async fn s3_backup_submission_accepts_only_after_configured_native_admission() {
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let source = server.path.join("s3-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data"), b"synthetic admission fixture").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pool = server.db.pool();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,s3_bucket,s3_region,s3_endpoint_url,s3_access_key,s3_secret_key) VALUES(7,'S3 test','s3',1,'fixture-bucket','us-east-1',?,'PUBLIC fixture access','PUBLIC fixture secret')")
        .bind(format!("http://{}", listener.local_addr().unwrap())).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled,sync_to_remote,remote_storage_ids) VALUES(1,'S3Target','path',?,1,1,'[7]')")
        .bind(source.to_str().unwrap()).execute(pool).await.unwrap();
    let (status, _, accepted) = server
        .request(
            "POST",
            "/api/v1/backups",
            &[("Cookie", &cookie), ("Origin", "http://localhost:3000")],
            Some(json!({"target_id":1})),
        )
        .await;
    assert_eq!(status, 200, "{accepted}");
    let id = accepted["id"].as_i64().unwrap();
    let (mut socket, _) =
        tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
    let first_line = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut data = Vec::new();
        while !data.ends_with(b"\r\n") {
            assert!(data.len() < 4096);
            data.push(socket.read_u8().await.unwrap());
        }
        String::from_utf8(data).unwrap()
    })
    .await
    .unwrap();
    let (intent, remote, effect): (String, String, Option<String>) = sqlx::query_as(
        "SELECT s.sync_status,s.remote_path,json_type(b.backup_metadata,'$.native_s3_effects.destination_7.artifact_0') FROM backup_storage_syncs s JOIN backups b ON b.id=s.backup_id WHERE s.backup_id=? AND s.storage_id=7",
    ).bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(intent, "running");
    assert_eq!(effect.as_deref(), Some("object"));
    let mut request = first_line.split_whitespace();
    assert_eq!(request.next(), Some("PUT"));
    assert_eq!(
        request.next().unwrap().split('?').next().unwrap(),
        format!("/fixture-bucket/{remote}")
    );
    drop(socket);
    drop(listener);
    let endpoint = format!("/api/v1/backups/{id}/retry-sync");
    let denied = server
        .request(
            "POST",
            &endpoint,
            &[("Origin", "http://localhost:3000")],
            Some(json!({})),
        )
        .await;
    assert_eq!(denied.0, 401);
    let authenticated = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let unknown = server
        .request("POST", &endpoint, &authenticated, Some(json!({})))
        .await;
    assert_eq!(unknown.0, 409);
    assert_eq!(
        unknown.2["detail"],
        "Remote sync has an unsettled provider effect"
    );
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT json_type(backup_metadata,'$.native_s3_effects.destination_7.artifact_0') FROM backups WHERE id=?").bind(id).fetch_one(pool).await.unwrap(), "object");
    server.state.settle_storage().await.unwrap();
    let (state, archive): (String, String) = sqlx::query_as("SELECT s.sync_status,b.file_path FROM backup_storage_syncs s JOIN backups b ON b.id=s.backup_id WHERE s.backup_id=? AND s.storage_id=7").bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(
        state, "running",
        "lost PUT response must not settle as success"
    );
    assert!(std::path::Path::new(&archive).is_file());
    server.close().await;
}

#[tokio::test]
async fn encrypted_s3_copy_recovers_rejected_ack_and_restores_after_reopen() {
    encrypted_remote_copy_recovers_rejected_ack(false).await;
}

#[tokio::test]
async fn encrypted_webdav_copy_recovers_rejected_ack_and_restores_after_reopen() {
    encrypted_remote_copy_recovers_rejected_ack(true).await;
}

async fn encrypted_remote_copy_recovers_rejected_ack(dav: bool) {
    use age::secrecy::ExposeSecret;
    use axum::{
        body::Bytes,
        http::{HeaderMap, Method, StatusCode, Uri},
        response::IntoResponse,
    };
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
        time::Duration,
    };
    let config = Config::from_pairs([
        ("COOKIE_SECURE", "false"),
        ("CORS_ORIGINS", "http://localhost:3000"),
    ])
    .unwrap();
    let server = Server::configured(config.clone()).await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let files = Arc::new(Mutex::new(BTreeMap::<String, Bytes>::new()));
    let fixture = files.clone();
    let (stop, stopped) = oneshot::channel::<()>();
    let provider = tokio::spawn(async move {
        let app = axum::Router::new().fallback(
            move |method: Method, uri: Uri, headers: HeaderMap, mut body: Bytes| {
                let fixture = fixture.clone();
                async move {
                    let mut files = fixture.lock().unwrap();
                    let key = uri.path().to_owned();
                    match method {
                        Method::PUT => match files.entry(key) {
                            std::collections::btree_map::Entry::Vacant(slot) => {
                                if headers
                                    .get("content-encoding")
                                    .is_some_and(|value| value == "aws-chunked")
                                {
                                    let mut bytes = Vec::new();
                                    let mut remaining = body.as_ref();
                                    loop {
                                        let end = remaining
                                            .windows(2)
                                            .position(|pair| pair == b"\r\n")
                                            .unwrap();
                                        let size = std::str::from_utf8(&remaining[..end])
                                            .unwrap()
                                            .split(';')
                                            .next()
                                            .unwrap();
                                        let size = usize::from_str_radix(size, 16).unwrap();
                                        remaining = &remaining[end + 2..];
                                        if size == 0 {
                                            break;
                                        }
                                        bytes.extend_from_slice(&remaining[..size]);
                                        assert_eq!(&remaining[size..size + 2], b"\r\n");
                                        remaining = &remaining[size + 2..];
                                    }
                                    body = bytes.into();
                                }
                                slot.insert(body);
                                StatusCode::OK.into_response()
                            }
                            std::collections::btree_map::Entry::Occupied(_) => {
                                StatusCode::PRECONDITION_FAILED.into_response()
                            }
                        },
                        Method::GET => match files.get(&key) {
                            Some(data) => data.clone().into_response(),
                            None => StatusCode::NOT_FOUND.into_response(),
                        },
                        _ if dav && method.as_str() == "MKCOL" => {
                            StatusCode::CREATED.into_response()
                        }
                        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
                    }
                }
            },
        );
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let origin = server.path.join("s3-cipher");
    std::fs::create_dir(&origin).unwrap();
    std::fs::write(origin.join("data"), b"original S3 plaintext fixture").unwrap();
    let identity = age::x25519::Identity::generate();
    let pool = server.db.pool();
    sqlx::query("INSERT INTO encryption_config(id,public_key,encryption_enabled,setup_completed) VALUES(1,?,1,1)").bind(identity.to_public().to_string()).execute(pool).await.unwrap();
    if dav {
        sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,webdav_url,base_path) VALUES(7,'DAV fixture','webdav',1,?,'/fixture-bucket')").bind(url).execute(pool).await.unwrap();
    } else {
        sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,s3_bucket,s3_region,s3_endpoint_url,s3_access_key,s3_secret_key) VALUES(7,'S3 fixture','s3',1,'fixture-bucket','us-east-1',?,'PUBLIC fixture access','PUBLIC fixture secret')").bind(url).execute(pool).await.unwrap();
    }
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled,sync_to_remote,remote_storage_ids,delete_local_after_sync) VALUES(1,'S3Cipher','path',?,1,1,'[7]',1)").bind(origin.to_str().unwrap()).execute(pool).await.unwrap();
    sqlx::query("CREATE TABLE test_sync_ack_attempts (backup_id INTEGER NOT NULL)")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_completed_sync BEFORE UPDATE OF sync_status ON backup_storage_syncs WHEN NEW.sync_status='completed' BEGIN INSERT INTO test_sync_ack_attempts VALUES(NEW.backup_id); SELECT RAISE(FAIL,'synthetic failed acknowledgement'); END").execute(pool).await.unwrap();
    let (status, _, body) = server
        .request(
            "POST",
            "/api/v1/backups",
            &headers,
            Some(json!({"target_id":1})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let id = body["id"].as_i64().unwrap();
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let attempted: Option<i64> =
                sqlx::query_scalar("SELECT backup_id FROM test_sync_ack_attempts")
                    .fetch_optional(pool)
                    .await
                    .unwrap();
            if attempted == Some(id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "S3 sync did not reach acknowledgement; {} fixture objects written",
            files.lock().unwrap().len()
        )
    });
    let (archive, key, remote, status): (String,String,String,String) = sqlx::query_as("SELECT b.file_path,b.encryption_key_path,s.remote_path,s.sync_status FROM backups b JOIN backup_storage_syncs s ON b.id=s.backup_id WHERE b.id=?").bind(id).fetch_one(pool).await.unwrap();
    let key_remote = dockervault_core::crypto::key_sidecar_path(&remote);
    let before = files.lock().unwrap().clone();
    for (local, remote) in [(&archive, &remote), (&key, &key_remote)] {
        assert_eq!(
            before[&format!("/fixture-bucket/{remote}")],
            std::fs::read(local).unwrap()
        );
    }
    assert_eq!(status, "running");
    let path = server.stop_preserving_files().await;
    assert!(std::path::Path::new(&archive).is_file());
    assert!(std::path::Path::new(&key).is_file());
    // Remove the injected fault with the application stopped, then reopen the same SQLite file.
    let owner = Owner::new(1).unwrap();
    let db = Database::open(path.join("db.sqlite"), path.join("snapshots"), &owner)
        .await
        .unwrap();
    sqlx::query("DROP TRIGGER reject_completed_sync")
        .execute(db.pool())
        .await
        .unwrap();
    db.close().await;
    owner.shutdown(None).await;
    let reopened = Server::open_at(config, path).await;
    let status: String =
        sqlx::query_scalar("SELECT sync_status FROM backup_storage_syncs WHERE backup_id=?")
            .bind(id)
            .fetch_one(reopened.db.pool())
            .await
            .unwrap();
    assert_eq!(status, "completed");
    assert_eq!(*files.lock().unwrap(), before);

    assert!(!std::path::Path::new(&archive).exists());
    assert!(!std::path::Path::new(&key).exists());
    std::fs::write(origin.join("data"), b"mutated fixture").unwrap();
    let review = reopened
        .request(
            "POST",
            &format!("/api/v1/backups/{id}/restore-review"),
            &headers,
            Some(json!({"destination_type":"original"})),
        )
        .await;
    assert_eq!(review.0, 200, "{:?}", review.2);
    let response = reopened.request("POST", &format!("/api/v1/backups/{id}/restore"), &headers, Some(json!({"destination_type":"original","review_fingerprint":review.2["fingerprint"],"private_key":identity.to_string().expose_secret()}))).await;
    assert_eq!(response.0, 200, "{:?}", response.2);
    assert_eq!(
        std::fs::read(origin.join("data")).unwrap(),
        b"original S3 plaintext fixture"
    );
    reopened.close().await;
    let _ = stop.send(());
    provider.await.unwrap();
}

#[tokio::test]
async fn native_encrypted_backup_has_recoverable_key_and_no_named_plaintext() {
    use dockervault_core::crypto::{decrypt_dek, decrypt_file};
    use sha2::{Digest, Sha256};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let source = server.path.join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data"), b"encrypted backup payload").unwrap();
    let identity = age::x25519::Identity::generate();
    sqlx::query("INSERT INTO encryption_config(id,public_key,encryption_enabled,setup_completed) VALUES(1,?,1,1)")
        .bind(identity.to_public().to_string()).execute(server.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled,compression_enabled) VALUES(1,'Cipher','path',?,1,1)")
        .bind(source.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    let (status, _, body) = server
        .request(
            "POST",
            "/api/v1/backups",
            &headers,
            Some(json!({"target_id":1})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let id = body["id"].as_i64().unwrap();
    let (archive,key,size,checksum)=tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let row=sqlx::query_as::<_,(String,Option<String>,Option<String>,Option<i64>,Option<String>,bool)>("SELECT status,file_path,encryption_key_path,file_size,checksum,encrypted FROM backups WHERE id=?").bind(id).fetch_one(server.db.pool()).await.unwrap();
            match row {
                (s,Some(a),Some(k),Some(n),Some(hash),true) if s=="COMPLETED"=>break (a,k,n,hash),
                (s,..) if s=="PENDING" || s=="RUNNING"=>tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                row=>panic!("unexpected backup result: {row:?}"),
            }
        }
    }).await.unwrap();
    assert_eq!(std::fs::metadata(&archive).unwrap().len() as i64, size);
    assert_eq!(
        std::fs::read_dir(server.path.join("backups/Cipher"))
            .unwrap()
            .count(),
        2
    );
    use age::secrecy::ExposeSecret;
    let material = decrypt_dek(
        std::fs::File::open(&key).unwrap(),
        identity.to_string().expose_secret(),
    )
    .unwrap();
    let decoded = server.path.join("decoded.gz");
    decrypt_file(std::path::Path::new(&archive), &decoded, &material, &|| {
        false
    })
    .unwrap();
    let bytes = std::fs::read(decoded).unwrap();
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(digest, checksum);
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    let mut member = tar
        .entries()
        .unwrap()
        .map(Result::unwrap)
        .find(|member| member.path().unwrap().as_ref() == std::path::Path::new("source/data"))
        .unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(&mut member, &mut data).unwrap();
    assert_eq!(data, b"encrypted backup payload");
    let route = format!("/api/v1/backups/{id}/files");
    let key_body = || json!({"private_key":identity.to_string().expose_secret()});
    let (status, _, rows) = server
        .request(
            "POST",
            &format!("{route}?search=data"),
            &headers,
            Some(key_body()),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(rows[0]["path"], "source/data");
    let (status, _, data) = server
        .request(
            "POST",
            &format!("{route}/source/data"),
            &headers,
            Some(key_body()),
        )
        .await;
    assert_eq!((status, data), (200, json!("encrypted backup payload")));
    for path in [&route, &format!("{route}/source/data")] {
        assert_eq!(
            server
                .request(
                    "POST",
                    path,
                    &headers,
                    Some(json!({"private_key":"synthetic-invalid"}))
                )
                .await
                .0,
            400
        );
        assert_eq!(
            server
                .request("POST", path, &headers, Some(json!({})))
                .await
                .0,
            422
        );
    }
    server.close().await;
}

#[tokio::test]
async fn encrypted_restore_authenticates_before_writing_the_destination() {
    #[path = "../../dockervault-core/tests/common/mod.rs"]
    mod public_fixture;
    use dockervault_core::crypto::{decrypt_dek, encrypt_file};
    use sha2::{Digest, Sha256};
    let server = Server::start("http://localhost:3000").await;
    let cookie = server.setup().await;
    let headers = [
        ("Cookie", cookie.as_str()),
        ("Origin", "http://localhost:3000"),
    ];
    let source = server.path.join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data"), b"mutated").unwrap();
    let plain = server.path.join("fixture.tar");
    let mut tar = tar::Builder::new(std::fs::File::create(&plain).unwrap());
    let original = b"verified bytes";
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o640);
    header.set_size(original.len() as u64);
    tar.append_data(&mut header, "source/data", &original[..])
        .unwrap();
    drop(tar.into_inner().unwrap());
    let cipher = server.path.join("backups/fixture.tar.enc");
    let sidecar = server.path.join("backups/fixture.tar.key");
    let wrapped =
        include_bytes!("../../dockervault-core/tests/fixtures/pax-links.tar.key.TEST_ONLY");
    std::fs::write(&sidecar, wrapped).unwrap();
    let private = public_fixture::public_identity(false);
    let material = decrypt_dek(&wrapped[..], &private).unwrap();
    encrypt_file(&plain, &cipher, &material, &|| false).unwrap();
    let checksum: String = Sha256::digest(std::fs::read(plain).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    sqlx::query("INSERT INTO backup_targets(id,name,target_type,host_path,enabled) VALUES(1,'Fixture','path',?,1)").bind(source.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO backups(id,target_id,status,encrypted,file_path,encryption_key_path,file_size,checksum) VALUES(1,1,'COMPLETED',1,?,?,?,?)")
        .bind(cipher.to_str().unwrap()).bind(sidecar.to_str().unwrap()).bind(std::fs::metadata(&cipher).unwrap().len() as i64).bind(checksum).execute(server.db.pool()).await.unwrap();
    let review = server
        .request(
            "POST",
            "/api/v1/backups/1/restore-review",
            &headers,
            Some(json!({"destination_type":"original"})),
        )
        .await;
    assert_eq!(review.0, 200);
    assert_eq!(review.2["key_required"], true);
    let mut selection =
        json!({"destination_type":"original","review_fingerprint":review.2["fingerprint"]});
    let submit = |body| server.request("POST", "/api/v1/backups/1/restore", &headers, Some(body));
    assert_eq!(submit(selection.clone()).await.0, 400);
    selection["private_key"] = json!(&private);
    let saved = std::fs::read(&cipher).unwrap();
    let mut tampered = saved.clone();
    *tampered.last_mut().unwrap() ^= 1;
    std::fs::write(&cipher, tampered).unwrap();
    assert_ne!(submit(selection.clone()).await.0, 200);
    assert_eq!(std::fs::read(source.join("data")).unwrap(), b"mutated");
    std::fs::write(&cipher, saved).unwrap();
    assert_eq!(submit(selection.clone()).await.0, 200);
    assert_eq!(std::fs::read(source.join("data")).unwrap(), original);
    let remote = server.path.join("configured-local");
    std::fs::create_dir(&remote).unwrap();
    std::fs::rename(&cipher, remote.join("fixture.tar.enc")).unwrap();
    std::fs::rename(&sidecar, remote.join("fixture.tar.key")).unwrap();
    sqlx::query("INSERT INTO remote_storages(id,name,storage_type,enabled,base_path) VALUES(77,'fixture-copy','local',1,?)")
        .bind(remote.to_str().unwrap()).execute(server.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO backup_storage_syncs(backup_id,storage_id,remote_path,sync_status) VALUES(1,77,'fixture.tar.enc','completed')")
        .execute(server.db.pool()).await.unwrap();
    std::fs::write(source.join("data"), b"mutated again").unwrap();
    let mut without_key = selection.clone();
    without_key.as_object_mut().unwrap().remove("private_key");
    assert_eq!(submit(without_key).await.0, 400);
    let key_path = remote.join("fixture.tar.key");
    std::fs::remove_file(&key_path).unwrap();
    assert_eq!(submit(selection.clone()).await.0, 400);
    assert_eq!(
        std::fs::read(source.join("data")).unwrap(),
        b"mutated again"
    );
    std::fs::write(&key_path, wrapped).unwrap();
    assert_eq!(submit(selection).await.0, 200);
    assert_eq!(std::fs::read(source.join("data")).unwrap(), original);
    assert_eq!(
        std::fs::read_dir(server.path.join("backups"))
            .unwrap()
            .count(),
        0
    );
    server.close().await;
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
async fn stalled_websocket_does_not_hold_healthy_delivery_or_http_health() {
    use futures_util::StreamExt;
    use std::time::Duration;
    use tokio_tungstenite::{MaybeTlsStream, tungstenite::client::IntoClientRequest};

    let server = Server::start("http://localhost").await;
    let cookie = server.setup().await;
    let origin = format!("http://{}", server.address);
    let mut request = format!("ws://{}/ws/updates", server.address)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    request
        .headers_mut()
        .insert("origin", origin.parse().unwrap());
    let tcp = tokio::net::TcpSocket::new_v4().unwrap();
    tcp.set_recv_buffer_size(8192).unwrap();
    let tcp = tcp.connect(server.address).await.unwrap();
    let (mut slow, _) = tokio_tungstenite::client_async(request, MaybeTlsStream::Plain(tcp))
        .await
        .unwrap();
    assert!(
        frame(&mut slow)
            .await
            .into_text()
            .unwrap()
            .contains("connected")
    );

    // An ordinary consumer that does not read: one bounded synthetic notification
    // cannot fit in its TCP receive window. Do not poll this WebSocket yet.
    server.notifications.broadcast(&json!({
        "type": "backup_progress", "progress": 50, "message": "x".repeat(8 * 1024 * 1024)
    }));
    let MaybeTlsStream::Plain(tcp) = slow.get_ref() else {
        unreachable!()
    };
    tokio::time::timeout(Duration::from_secs(2), tcp.readable())
        .await
        .unwrap()
        .unwrap();

    let mut healthy = socket(&server, Some(&cookie), Some(&origin), None)
        .await
        .unwrap();
    let _ = frame(&mut healthy).await;
    server
        .notifications
        .broadcast(&json!({"type":"backup_progress","progress":100}));
    let health = tokio::time::timeout(
        Duration::from_secs(1),
        server.request("GET", "/health", &[], None),
    );
    let (health, terminal) = tokio::join!(health, frame(&mut healthy));
    assert_eq!(health.unwrap().0, 200);
    assert_eq!(
        terminal.into_text().unwrap(),
        "{\"type\": \"backup_progress\", \"progress\": 100}"
    );

    // The socket stays unread for the real send deadline. A partial frame may
    // prevent delivery of the close code, but it must never see terminal progress.
    tokio::time::sleep(Duration::from_secs(6)).await;
    tokio::time::timeout(Duration::from_secs(8), async {
        while let Some(Ok(message)) = slow.next().await {
            if let Ok(text) = message.into_text() {
                assert_ne!(text, "{\"type\": \"backup_progress\", \"progress\": 100}");
            }
        }
    })
    .await
    .unwrap();
    healthy.close(None).await.unwrap();
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
