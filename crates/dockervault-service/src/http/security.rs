use super::HttpState;
use axum::{
    Json,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

pub fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            if name.trim() != "session_token" {
                return None;
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            Some(value.to_owned())
        })
        .next_back()
        .filter(|value| !value.is_empty())
}
pub fn bearer(headers: &HeaderMap, case_sensitive: bool) -> Option<String> {
    let (scheme, token) = headers
        .get("authorization")?
        .to_str()
        .ok()?
        .split_once(' ')?;
    if token.is_empty()
        || if case_sensitive {
            scheme != "Bearer"
        } else {
            !scheme.eq_ignore_ascii_case("Bearer")
        }
    {
        return None;
    }
    Some(token.to_owned())
}
pub fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> IpAddr {
    let peer = peer
        .map(|peer| peer.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    if peer.is_loopback() {
        for name in ["x-real-ip", "x-forwarded-for"] {
            if let Some(ip) = headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .and_then(|value| value.trim().parse().ok())
            {
                return ip;
            }
        }
    }
    peer
}
struct Window {
    start: Instant,
    count: u8,
}
#[derive(Default)]
pub(super) struct RateLimits {
    windows: Mutex<HashMap<(IpAddr, &'static str), Window>>,
}
impl RateLimits {
    pub(super) fn check(&self, ip: IpAddr, route: &'static str) -> bool {
        let Ok(mut windows) = self.windows.lock() else {
            return false;
        };
        let now = Instant::now();
        windows.retain(|_, window| now.duration_since(window.start) < Duration::from_secs(60));
        if !windows.contains_key(&(ip, route)) && windows.len() >= 4096 {
            return false;
        }
        let window = windows.entry((ip, route)).or_insert(Window {
            start: now,
            count: 0,
        });
        if window.count >= 5 {
            return false;
        }
        window.count += 1;
        true
    }
}
pub(super) fn rate_limit(
    state: &HttpState,
    request: &Request,
    route: &'static str,
) -> Result<(), Box<Response>> {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0);
    if state
        .limits
        .check(client_ip(request.headers(), peer), route)
    {
        Ok(())
    } else {
        Err(Box::new(
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error":"Rate limit exceeded: 5 per 1 minute"})),
            )
                .into_response(),
        ))
    }
}
fn valid_origin(origin: &str) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    if !["http", "https"].contains(&scheme)
        || authority.is_empty()
        || authority.contains(['@', '/', '?', '#'])
    {
        return false;
    }
    let Ok(authority) = authority.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let has_port = if authority.as_str().starts_with('[') {
        authority.as_str().contains("]:")
    } else {
        authority.as_str().contains(':')
    };
    !authority.as_str().ends_with(':') && (!has_port || authority.port().is_some())
}
fn explicit_origin(state: &HttpState, origin: &str) -> bool {
    valid_origin(origin)
        && state
            .config
            .cors_origins
            .split(',')
            .map(str::trim)
            .any(|allowed| allowed != "*" && allowed == origin)
}
fn allowed_origin(
    state: &HttpState,
    headers: &HeaderMap,
    uri: &Uri,
    peer: Option<SocketAddr>,
    origin: &str,
) -> bool {
    if !valid_origin(origin) {
        return false;
    }
    if explicit_origin(state, origin) {
        return true;
    }
    let Some(host) = headers.get("host").and_then(|host| host.to_str().ok()) else {
        return false;
    };
    let mut scheme = uri.scheme_str().unwrap_or("http");
    if peer.is_some_and(|peer| peer.ip().is_loopback())
        && let Some(forwarded) = headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .filter(|value| ["http", "https"].contains(value))
    {
        scheme = forwarded;
    }
    if origin == format!("{scheme}://{host}") {
        return true;
    }
    // Browser-generated Fetch Metadata can retain TLS origin across an HTTP proxy.
    headers
        .get("sec-fetch-site")
        .is_some_and(|value| value == "same-origin")
        && origin
            .split_once("://")
            .is_some_and(|(_, authority)| authority == host)
}
fn cors(response: &mut Response, origin: &str) {
    if let Ok(origin) = HeaderValue::from_str(origin) {
        response
            .headers_mut()
            .insert("access-control-allow-origin", origin);
        response.headers_mut().insert(
            "access-control-allow-credentials",
            HeaderValue::from_static("true"),
        );
        response
            .headers_mut()
            .insert("vary", HeaderValue::from_static("Origin"));
    }
}
pub(super) async fn guard(
    State(state): State<HttpState>,
    request: Request,
    next: Next,
) -> Response {
    let origin = request
        .headers()
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|value| value.0);
    let allowed = origin.as_deref().is_some_and(|origin| {
        allowed_origin(&state, request.headers(), request.uri(), peer, origin)
    });
    if request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key("access-control-request-method")
        && origin.is_some()
    {
        let method = request
            .headers()
            .get("access-control-request-method")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let methods = ["GET", "POST", "PUT", "DELETE", "OPTIONS"];
        let headers_ok = request
            .headers()
            .get("access-control-request-headers")
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| {
                value.split(',').all(|name| {
                    [
                        "accept",
                        "accept-language",
                        "authorization",
                        "content-language",
                        "content-type",
                    ]
                    .contains(&name.trim().to_ascii_lowercase().as_str())
                })
            });
        let mut response = (
            if allowed && methods.contains(&method) && headers_ok {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            },
            if allowed {
                "OK"
            } else {
                "Disallowed CORS origin"
            },
        )
            .into_response();
        response.headers_mut().insert(
            "access-control-allow-methods",
            HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"),
        );
        response.headers_mut().insert(
            "access-control-allow-headers",
            HeaderValue::from_static(
                "Accept, Accept-Language, Authorization, Content-Language, Content-Type",
            ),
        );
        response
            .headers_mut()
            .insert("access-control-max-age", HeaderValue::from_static("600"));
        if allowed {
            cors(&mut response, origin.as_deref().unwrap_or_default());
        }
        return response;
    }
    let mutating = ![Method::GET, Method::HEAD, Method::OPTIONS].contains(request.method());
    if mutating
        && ((request.headers().contains_key("origin") && !allowed)
            || (origin.is_none()
                && request
                    .headers()
                    .get("sec-fetch-site")
                    .is_some_and(|value| value == "cross-site")))
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"detail":"Origin not allowed"})),
        )
            .into_response();
    }
    let path = request.uri().path();
    let public = [
        "/health",
        "/api/v1/auth/status",
        "/api/v1/auth/setup",
        "/api/v1/auth/login",
    ]
    .contains(&path)
        || path.starts_with("/ws/")
        || state.config.enable_docs && ["/docs", "/redoc", "/openapi.json"].contains(&path);
    let mut response = if public {
        next.run(request).await
    } else {
        match state.auth.setup_complete().await {
            Err(error) => super::auth::error(error),
            Ok(false) => (
                StatusCode::FORBIDDEN,
                Json(json!({"detail":"Setup required","setup_required":true})),
            )
                .into_response(),
            Ok(true) => {
                let token = cookie(request.headers()).or_else(|| bearer(request.headers(), true));
                match token {
                    None => (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"detail":"Not authenticated"})),
                    )
                        .into_response(),
                    Some(token) => match state
                        .auth
                        .current(&crate::auth::SessionHash::from_token(&token))
                        .await
                    {
                        Err(error) => super::auth::error(error),
                        Ok(None) => (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({"detail":"Invalid or expired session"})),
                        )
                            .into_response(),
                        Ok(Some(_)) => next.run(request).await,
                    },
                }
            }
        }
    };
    if allowed {
        cors(&mut response, origin.as_deref().unwrap_or_default());
    }
    response
}
