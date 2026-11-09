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
    async fn configured(config: Config) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-http-auth-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
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
