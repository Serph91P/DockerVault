//! One bounded queue and one physical socket owner per authenticated connection.
use super::{HttpState, security};
use crate::auth::{SessionHash, SessionLease};
use axum::{
    extract::{
        ConnectInfo, Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use dockervault_core::encoding::python_json;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::Notify;
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Default)]
struct Queue {
    messages: VecDeque<(bool, Arc<str>)>,
    closed: bool,
}
struct Client {
    user_id: i64,
    queue: Mutex<Queue>,
    notify: Notify,
}
impl Client {
    fn enqueue(&self, encoded: Arc<str>, control: bool) {
        if let Ok(mut queue) = self.queue.lock() {
            if queue.closed {
                return;
            }
            if encoded.len() > MAX_MESSAGE {
                queue.closed = true;
            } else if queue.messages.len() == 32 {
                if let Some(index) = queue.messages.iter().position(|entry| !entry.0) {
                    queue.messages.remove(index);
                } else {
                    queue.closed = true;
                }
            }
            if !queue.closed {
                queue.messages.push_back((control, encoded));
            }
        }
        self.notify.notify_one();
    }
    fn pop(&self) -> Result<Option<Arc<str>>, ()> {
        let mut queue = self.queue.lock().map_err(|_| ())?;
        if queue.closed {
            return Err(());
        }
        Ok(queue.messages.pop_front().map(|entry| entry.1))
    }
}
#[derive(Default)]
pub struct Notifications {
    clients: Mutex<Vec<Weak<Client>>>,
}
impl Notifications {
    fn subscribe(&self, user_id: i64) -> Arc<Client> {
        let client = Arc::new(Client {
            user_id,
            queue: Mutex::new(Queue::default()),
            notify: Notify::new(),
        });
        let mut clients = self
            .clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clients.retain(|entry| entry.strong_count() > 0);
        clients.push(Arc::downgrade(&client));
        client
    }
    fn publish(&self, user_id: Option<i64>, message: &Value) {
        let encoded: Arc<str> = python_json(message).into();
        let control = control(message);
        if let Ok(clients) = self.clients.lock() {
            for client in clients.iter().filter_map(Weak::upgrade) {
                if user_id.is_none_or(|user_id| client.user_id == user_id) {
                    client.enqueue(encoded.clone(), control);
                }
            }
        }
    }
    pub fn broadcast(&self, message: &Value) {
        self.publish(None, message)
    }
    pub fn send_to_user(&self, user_id: i64, message: &Value) {
        self.publish(Some(user_id), message)
    }
}
fn control(message: &Value) -> bool {
    message
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| ["connected", "pong", "subscribed", "error"].contains(&kind))
}
fn send(client: &Client, message: Value) {
    client.enqueue(python_json(&message).into(), control(&message));
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::String(value) => !value.is_empty(),
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
fn response(message: &Value) -> Option<Value> {
    match message.get("type").and_then(Value::as_str) {
        Some("ping") => Some(json!({"type":"pong"})),
        Some("subscribe") => message
            .get("backup_id")
            .filter(|value| truthy(value))
            .map(|id| json!({"type":"subscribed","backup_id":id})),
        _ => {
            let kind = match message.get("type") {
                None | Some(Value::Null) => "None".to_owned(),
                Some(Value::String(value)) => value.clone(),
                Some(Value::Bool(true)) => "True".to_owned(),
                Some(Value::Bool(false)) => "False".to_owned(),
                Some(other) => other.to_string(),
            };
            Some(json!({"type":"error","message":format!("Unknown message type: {kind}")}))
        }
    }
}
fn allowed_origin(
    state: &HttpState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    peer: Option<SocketAddr>,
    origin: &str,
) -> bool {
    if !security::valid_origin(origin) {
        return false;
    }
    let secure = state.config.cookie_secure
        || matches!(uri.scheme_str(), Some("https" | "wss"))
        || peer.is_some_and(|peer| peer.ip().is_loopback())
            && headers
                .get("x-forwarded-proto")
                .is_some_and(|value| value == "https");
    let scheme = if secure { "https" } else { "http" };
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    origin == format!("{scheme}://{host}")
        || state
            .config
            .cors_origins
            .split(',')
            .map(str::trim)
            .any(|allowed| allowed == origin)
}
pub(super) async fn updates(
    State(state): State<HttpState>,
    uri: axum::http::Uri,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let origin = headers.get("origin").and_then(|value| value.to_str().ok());
    let token = if origin.is_none() {
        query.get("token").cloned()
    } else {
        security::cookie(&headers).or_else(|| query.get("token").cloned())
    };
    let Some(token) = token.filter(|token| !token.is_empty()) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if headers.contains_key("origin")
        && origin.is_none_or(|origin| !allowed_origin(&state, &headers, &uri, Some(peer), origin))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let identity = SessionHash::from_token(&token);
    drop(token);
    let user = match state.auth.current(&identity).await {
        Ok(Some(user)) => user,
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
    let lease = match state.auth.registry().register(identity.clone(), user.id) {
        Ok(lease) => lease,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // Registration precedes the final DB check so a concurrent revocation is never lost.
    if !authorized(&state, &identity, user.id, &lease).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade
        .read_buffer_size(16 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(2 * MAX_MESSAGE)
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run(socket, state, identity, user.id, lease))
}
async fn authorized(
    state: &HttpState,
    identity: &SessionHash,
    user_id: i64,
    lease: &SessionLease,
) -> bool {
    tokio::select! {biased;
        _=lease.cancelled()=>false,
        result=tokio::time::timeout(SEND_TIMEOUT,state.auth.current(identity))=>matches!(result,Ok(Ok(Some(user))) if user.id==user_id),
    }
}
async fn run(
    mut socket: WebSocket,
    state: HttpState,
    identity: SessionHash,
    user_id: i64,
    lease: SessionLease,
) {
    let client = state.notifications.subscribe(user_id);
    send(
        &client,
        json!({"type":"connected","message":"Connected to backup manager"}),
    );
    let mut timer = tokio::time::interval(Duration::from_secs(1));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (mut code, mut reason) = (1000, "");
    loop {
        if lease.is_cancelled() {
            code = 4001;
            reason = "Session revoked";
            break;
        }
        match client.pop() {
            Err(()) => {
                code = 1013;
                reason = "Notification queue full";
                break;
            }
            Ok(Some(encoded)) => {
                if !authorized(&state, &identity, user_id, &lease).await {
                    code = 4001;
                    reason = "Session revoked or expired";
                    break;
                }
                let sent = tokio::select! {biased;
                    _=lease.cancelled()=>false,
                    result=tokio::time::timeout(SEND_TIMEOUT,socket.send(Message::Text(encoded.to_string().into())))=>matches!(result,Ok(Ok(()))),
                };
                if !sent {
                    if lease.is_cancelled() {
                        code = 4001;
                        reason = "Session revoked";
                    } else {
                        code = 1013;
                        reason = "Notification delivery stalled";
                    }
                    break;
                }
                continue;
            }
            Ok(None) => {}
        }
        tokio::select! {biased;
            _=lease.cancelled()=> {code=4001;reason="Session revoked";break},
            _=timer.tick()=> {if !authorized(&state,&identity,user_id,&lease).await {code=4001;reason="Session revoked or expired";break}},
            _=client.notify.notified()=>{},
            received=socket.recv()=>match received {
                Some(Ok(Message::Text(text)))=>match serde_json::from_str::<Value>(&text) {
                    Ok(message) if message.is_object()=> {if let Some(message)=response(&message) {send(&client,message);}},
                    Ok(_)=>break,
                    Err(_)=>send(&client,json!({"type":"error","message":"Invalid JSON"})),
                },
                Some(Ok(Message::Ping(_)|Message::Pong(_)))=>{},
                _=>break,
            },
        }
    }
    if state.auth.registry().is_closed() {
        code = 1001;
        reason = "Server shutdown";
    }
    let _ = tokio::time::timeout(
        SEND_TIMEOUT,
        socket.send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }))),
    )
    .await;
    // Revocation acknowledgment follows actual transport destruction, never waiter drop.
    drop(socket);
    drop(client);
    drop(lease);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_coalescing_preserves_initial_control_and_latest_terminal_message() {
        let hub = Notifications::default();
        let client = hub.subscribe(1);
        send(
            &client,
            json!({"type":"connected","message":"Connected to backup manager"}),
        );
        for sequence in 0..1000 {
            hub.broadcast(&json!({"type":"backup_progress","sequence":sequence}));
        }
        assert!(client.queue.lock().unwrap().messages.len() <= 32);
        let first: Value = serde_json::from_str(&client.pop().unwrap().unwrap()).unwrap();
        assert_eq!(first["type"], "connected");
        let mut last = Value::Null;
        while let Some(message) = client.pop().unwrap() {
            last = serde_json::from_str(&message).unwrap();
        }
        assert_eq!(last["sequence"], 999);
    }
    #[test]
    fn control_flood_and_oversize_messages_fail_closed_without_dispatching_tasks() {
        let hub = Notifications::default();
        let client = hub.subscribe(1);
        for _ in 0..1000 {
            hub.send_to_user(1, &json!({"type":"pong"}));
        }
        let queue = client.queue.lock().unwrap();
        assert!(queue.closed);
        assert_eq!(queue.messages.len(), 32);
        drop(queue);
        let other = hub.subscribe(2);
        hub.send_to_user(
            2,
            &json!({"type":"backup_progress","message":"x".repeat(MAX_MESSAGE)}),
        );
        assert!(other.pop().is_err());
    }
    #[test]
    fn user_targeted_notifications_do_not_publish_to_a_different_owner() {
        let hub = Notifications::default();
        let owner = hub.subscribe(1);
        let other = hub.subscribe(2);
        hub.send_to_user(1, &json!({"type":"backup_progress","backup_id":77}));
        assert!(owner.pop().unwrap().is_some());
        assert!(other.pop().unwrap().is_none());
    }
}
