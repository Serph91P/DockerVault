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
