# Compatibility decisions and observed asymmetries

Scope: unchanged Python control
`18d8c3d7f5d464195ac3fb3918048c794ef1079a`; implementation #248.
These observations are contracts to test, not evidence that Rust implements them.
No published format, key, migration history or safety assertion is changed here.

## D1 — SQLite enum names and temporal/JSON encoding

**Decision:** preserve published persisted names independently of wire values.
The executed ORM-to-SQLite control writes backup status `COMPLETED`, backup type
`FULL` and log level `INFO`. Wire enums may be lowercase; blindly deriving SQL
serialization from the wire enum is incompatible. The captured UTC datetime is
`2024-01-02 03:04:05.123456`, without an offset suffix. JSON null, false, zero,
empty collections and Unicode remain distinct. See `sqlite-values.json`.

**Cost of being wrong:** unreadable existing records, silent state changes, or
loss of scheduling and retention semantics. Rust must read and write the frozen
encodings; changing them requires the explicit reverse-conversion contract.

## D2 — The published migration ledger is not a framework implementation detail

**Decision:** retain `schema_migrations` and the published history 1–7. Both the
fresh control and static legacy fixture actually migrate to those seven entries,
with no FK violations. Do not substitute `_sqlx_migrations`, rewrite an old
migration, or assume that an additive ledger version is readable by old Python.
A schema change beyond this corpus is not approved by this decision.

**Cost of being wrong:** failed startup, non-atomic partial migration or rollback
that cannot read new state. R2 and R14 must execute forward/reverse rehearsals,
including after new writes rather than only restoring an old snapshot.

## D3 — Conflicting HTTP credentials have observable route asymmetries

**Observation:** real isolated HTTP requests with a valid synthetic cookie and an
invalid Bearer header yield:

| Route | Result |
| --- | --- |
| `GET /api/v1/auth/me` | 401, `Invalid or expired session` |
| `GET /api/v1/auth/status` | 200, authenticated user |
| `GET /api/v1/settings/storage` | 200, protected settings |

Global Python middleware chooses the cookie before the Bearer header. The auth
`get_current_user` dependency chooses the Bearer header before the cookie, and
`status` reads only the cookie. Logout also has its own cookie-only behavior;
bearer-only revocation must not be falsely described as existing support.

**Decision:** freeze these asymmetries explicitly; never let framework extractor
defaults silently choose a different identity. This initial corpus does **not**
approve an authentication/CSRF policy change. R3 must test all supported
credential combinations, revocation and origin boundaries, and either preserve
exact precedence or deliver an explicit separately reviewed security decision.

**Cost of being wrong:** authenticating the wrong session, breaking programmatic
clients, or falsely acknowledging a logout that leaves a session/WS usable.

## D4 — Storage-operation identity fingerprints encode Python string values

**Observation:** `StorageJobs._storage` hashes sorted JSON of **every persisted
RemoteStorage column converted with Python `str` first**. That includes creation
identity, `None`, `True`, empty strings, zero and the stored credential bytes.
Default JSON separators and ASCII escaping are part of the input bytes. It is
not equivalent to hashing a normal Rust/Serde row, decrypted configuration,
selected endpoint fields, or just the recyclable integer storage ID.

**Decision:** preserve the exact hash input encoding in R9; retain local root
inode/device pinning separately. `storage-identity.json` contains an actual
synthetic persisted-row control with the constant input bytes and expected hash.
R9 must consume it as a native assertion rather than relying on an inferred
equivalent serializer.

**Cost of being wrong:** destructive operation replay against a changed/reused
storage target, or incorrectly rejecting a legitimate unchanged operation.

## D5 — Auth mutation ownership and transaction boundaries

**Decision:** preserve bcrypt's first 72 UTF-8 **bytes**, cost 12 for newly
written passwords, 64 random session bytes encoded URL-safe without padding,
SHA-256 token storage, and seven-day UTC expiry. `bcrypt-vectors.json` is an
executed unchanged Python control including a prefix ending inside a UTF-8
codepoint and an embedded NUL; all credentials are explicitly public test data.
Raw tokens are redacted in Debug and cannot be registered as a WS identity:
registration requires the separate `SessionHash` type.

Setup retains its immediate SQLite singleton transaction. Auth mutations run
under the bounded service owner rather than their HTTP waiter's lifetime;
rollback/connection closure is awaited before failed work is released. Revocation
must cancel registered connection owners and await their physical ownership
leases, including after the HTTP waiter disappears. Socket loops must drop those
leases only after their actual I/O resources settle; domain lease tests alone
are not a WebSocket runtime receipt.

Login rechecks its verified password hash inside the write claim. A concurrent
password change therefore cannot issue a late session authenticated with a stale
hash. Password update, old-session deletion and replacement session publication
share a transaction. This deliberately prevents partial password/session effects
on replacement-insert failure, unlike the Python handler's separate commits;
no credential precedence, key rotation, schema or successful response changes
are implied. Login's last-login update and session insert are likewise atomic.

**Cost of being wrong:** resurrected sessions after revocation, an unusable
account after a failed setup/password change, or false settlement. HTTP/WS
runtime, independent-process setup, restart and P gates remain required.

## D6 — HTTP origin, CORS and proxy boundary

**Decision:** retain D3 credential precedence: cookie-first, case-sensitive
`Bearer ` in the global guard; bearer-first/case-insensitive scheme in me and
password change; status/logout cookie-only. The current global exception handler
omits WWW-Authenticate, so a native extractor must not invent it accidentally.

Mutating requests with Origin require a syntactically valid HTTP(S) origin that
is same-origin or explicitly listed in CORS_ORIGINS. `*` is not a credentialled
cross-origin authorization. Browser-generated `Sec-Fetch-Site: same-origin` can
confirm the same authority when a reverse proxy hides the external TLS scheme;
it never authorizes a different authority. No-Origin programmatic requests stay
supported, but explicit cross-site Fetch Metadata without an allowed Origin is
rejected. Malicious/null origins are rejected before any mutation. Explicitly
configured frontend origins may operate cross-origin, including credentialled
preflight. This implements the required CSRF policy; the old HTTP middleware
had no equivalent rejection. The current WS origin policy remains a separate
existing contract and must not be silently weakened to match HTTP.

Raw tokens are never included in validation errors. JSON bodies retain the
executed control's FastAPI strict Content-Type default (introduced upstream in
0.132): missing/empty/non-application JSON media types cannot bootstrap setup.
Media types are case-insensitive and application/*+json remains supported.

Only loopback peers (the existing same-container nginx-to-backend topology)
may supply forwarded IP/protocol. Other peers cannot choose rate-limit identity
through X-Real-IP/X-Forwarded-For. Only the three currently decorated auth
endpoints have five-per-minute buckets; malformed DTOs fail validation before
consuming those buckets. Per-IP/per-endpoint windows and admission state are
bounded, and exhaustion fails closed. No new RBAC gate is inferred from is_admin.
R13 must preserve the external Host authority including its port through nginx.

**Cost of being wrong:** CSRF/session swapping, spoofable brute-force limits,
or breaking a legitimate proxy/client. Real browser/proxy, malformed-header,
open-socket revocation, native-platform and P receipts are still mandatory;
the initial TCP auth journeys are not whole-product acceptance.

## D7 — WebSocket transport and notification ownership

**Decision:** retain the existing cookie/query precedence independently of HTTP:
without Origin only the explicit query token is used; with Origin a nonempty
cookie wins. Missing/foreign/null origin-cookie handshakes are denied before
acceptance. The executed control uses Uvicorn's websockets-sansio protocol and
its actual preaccept denial is HTTP403 with an empty body; do not mistake the
in-process close intent4001/4003 for an established WebSocket close frame.

Each accepted socket has a separate SHA256 identity and fixed authenticated
user ID. Register before the final DB check to prevent a lost-revocation race;
revalidate the same user before every output and each idle second. Logout or
password-change acknowledgment waits until owned socket transport destruction.
Expiry/deletion closes with4001; shutdown uses1001; notification pressure1013.
The queue remains32 messages with the existing protected control types; progress
coalescing retains the newest event without dropping an initial control frame.
No producer awaits a network send or spawns an unbounded per-event task.

The current control's actual ws_max_size is16MiB. Native input/output metadata
and write buffers are bounded; archive bytes do not travel through this queue.
Python json.dumps-compatible text encoding is shared with the frozen core codec.
Native TCP/WS tests retain the existing two-second acceptance/revocation bounds,
including broadcast-after-delete and idle validation; they are not browser,
slow-reader-with-backup, architecture or performance acceptance by themselves.

**Cost of being wrong:** an old identity receiving protected events, early revoke
success while a writer remains alive, lost control messages or resource growth.

## D8 — Hook argv and ignored-output compatibility

**Decision:** port Python shlex.split(posix=True, comments=False), not a subtly
different shell lexer. Actual installed Rust shlex2.0.1 qualification changes
literal # arguments, escaped newlines, double-quoted backslash-dollar, and CR
splitting. The shell-words documentation also explicitly describes incompatible
POSIX behavior. A minimal four-state lexer is therefore required; all595 actual
unchanged Python stdlib vectors in hook-argv.json are native assertions. Parsing
never implies expansion or shell execution. Allowlisting still compares argv0
exactly against the untrimmed CSV, and timeout0 still means immediate timeout.

Generic process capture remains bounded/fail-closed by default. Hooks explicitly
select discard-and-drain because the Python hook ignores both captured pipes;
a verbose successful hook must not become a failure merely for writing64KiB.
Hooks explicitly retain the parent environment and allowlisted PATH lookup,
without changing the generic process owner's clean-environment/absolute-program
defaults. Individual cancellation handles request cleanup while the hook still
awaits the actual process/pipe/group join. Arguments and discarded output are
not logged. Job/source claims must remain in the owning job until that join.

**Cost of being wrong:** changed database dump commands, unintended shell
execution, leaked argument secrets, or early workload restart during a hook.

## Not decisions and not completed gates

- No new cryptography, key rotation, archive encoding or database system.
- Missing-ID/empty-body HTTP snapshots are not successful domain journeys.
- Socket-free Docker error responses do not validate Docker/Komodo lifecycle.
- The new Python performance controls are not Rust measurements; a recorded
  health timeout cannot be erased or explained away by historical results.
- Additional fixture, HTTP/WS, provider, resource, native-architecture and
  rollback gates in R0–R14 remain open.
