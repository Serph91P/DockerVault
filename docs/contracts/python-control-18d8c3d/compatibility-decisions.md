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

## Not decisions and not completed gates

- No new cryptography, key rotation, archive encoding or database system.
- Missing-ID/empty-body HTTP snapshots are not successful domain journeys.
- Socket-free Docker error responses do not validate Docker/Komodo lifecycle.
- The new Python performance controls are not Rust measurements; a recorded
  health timeout cannot be erased or explained away by historical results.
- Additional fixture, HTTP/WS, provider, resource, native-architecture and
  rollback gates in R0–R14 remain open.
