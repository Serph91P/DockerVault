# WebSocket notifications

`/ws/updates` keeps its existing event envelopes and reconnect contract.
Notifications are best-effort hints, not a business-operation acknowledgement.
Use the authenticated backup API for authoritative progress/terminal state.

Each connection has one writer and at most 32 queued frames (plus its in-flight
frame). When full, the oldest obsolete notification is dropped in favor of the
latest. Connected/pong/subscription/error control frames are preserved; a saturated
control-only queue disconnects once rather than spawning unbounded cleanup tasks.
A slow peer cannot hold the connection registry lock or delay a backup,
cleanup, another client's writer, or a connect/disconnect operation. Network
accept/send/close calls have a five-second deadline; stalled writers are removed
and closed with retryable code 1013 when possible. No broker is involved.

Application shutdown drains/cancels its owned writer tasks and bounds close
handshakes. Queueing a terminal notification is not proof that a browser received
it; the API remains the source of truth after a missed event or reconnect.

## Session and Origin policy

Each application socket is associated with its existing hashed session identity
and authenticated user. Logout/password-change detach the exact revoked sessions
before waiting for close handshakes, discard their queued events, and close live
sockets with code 4001. The existing frontend stops reconnecting on that live auth
closure. Validation also runs before every outbound frame and every second while
idle, so expiry or out-of-process deletion cannot retain a protected subscription.
Session-validation failures close fail-closed; normal close/send deadlines still
apply. Raw credentials are not added to notification state or logs.

Browser Origins must be an exact configured `CORS_ORIGINS` entry or the public
same-origin URL inferred from Host and TLS/`COOKIE_SECURE`. Configure the explicit
public origin if a reverse proxy does not preserve Host. Wildcards, `null`, paths,
credentials and non-HTTP(S) Origins are not accepted. Missing Origin is allowed
only for the existing explicit query-token programmatic contract, not cookie-only
authentication; an invalid explicit token cannot fall back to a cookie.

Initial policy/auth denial stays before WebSocket acceptance: ASGI returns HTTP
403, which browser WebSocket APIs expose as code 1006, not an application 4001 or
4003 close frame. This is distinct from revoking an already authenticated socket.
The TLS Chromium acceptance fixture independently checks Secure/HttpOnly/Lax
cookies, same-origin WSS, foreign same-site/different-port requests that actually
carry the cookie, initial denial, and real logout/password-change closure. It uses
self-signed test TLS and the real built frontend/API, not a production TLS or nginx
configuration claim.
