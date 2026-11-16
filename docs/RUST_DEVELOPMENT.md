# Native development and dependency policy

Work in progress for #248. The Rust workspace does not yet replace the product.
The Python control remains unchanged while the native implementation is tested.
No R0–R14 package is complete merely because the workspace compiles.

## Standard container build

The toolchain/MSRV is pinned to Rust 1.99.1 in `rust-toolchain.toml` and the
workspace manifests. The official Debian Rust image is digest-pinned; no host
Rust installation or host-path mount is needed:

```sh
docker build -f docker/rust-dev.Dockerfile -t dockervault-rust-dev .
docker run --rm --network none --cpus 2 --memory 6g dockervault-rust-dev
docker run --rm --network none dockervault-rust-dev cargo fmt --all -- --check
docker run --rm --network none dockervault-rust-dev \
  cargo clippy --workspace --all-targets --locked --offline -- -D warnings
```

The development image fetches the locked dependencies during its build; tests
then run offline. It is **not** a production image or an offline-recovery target.
The development-only audit tools are pinned to cargo-audit 0.22.2 and cargo-deny
0.20.2. Advisory checks require access to the public RustSec database:

```sh
docker run --rm dockervault-rust-dev cargo audit
docker run --rm dockervault-rust-dev cargo deny check advisories licenses sources
```

## Workspace boundary

- `dockervault-core`: domain/offline algorithms; no HTTP, SQL or Tokio dependency.
- `dockervault-service`: configuration, supervised work and process ownership;
  application and protocol adapters are added here as the port progresses.
- `dockervault-tools`: minimal archive/recovery targets depend on core, never the
  service crate. No downloadable native target is claimed complete yet.

Use `cargo tree -p dockervault-core --edges normal` and the corresponding tools
command to verify those dependency boundaries; module naming alone is not proof.

## Owned process execution

Native process owners require the **unprivileged** `dockervault-process-guard`
binary alongside their runtime. Pass its explicit absolute path to `ProcessOwner::new(capacity, guard)`
and `Hooks::new(config, guard)`; do not search an ambient PATH or fall back to an
unbound process. The isolated archive test image installs it in
`/usr/local/libexec` next to the archive worker, but grants sudo ONLY to the fixed
archive worker. Never mark the exec guard setuid or add it to sudoers.

The guard applies the kernel parent-death signal and verifies the expected parent
before safe `CommandExt::exec`. It preserves literal arguments, descriptors and the
explicitly provided environment. This avoids unsafe `pre_exec` in a threaded service;
workspace `unsafe_code = "forbid"` remains unchanged. Explicit working directories
are absolute. Regular process cancellation still drains pipes, terminates/escalates
the process group and reaps it before completion. Kernel owner-loss evidence is for
the directly executed unprivileged child, not a blanket claim for escaped descendants
or privilege-changing execs; complete workload/privileged crash gates remain required.

Both async and blocking owners physically join before releasing their admission
permit, then notify completion. A caller's wakeup can immediately reenter admission
on another thread; announcing completion before releasing the slot is a race, even
when a later shutdown would eventually drain it. Panic replies follow the same join
and release boundary.

## Initial selected dependencies

Registry metadata and primary documentation were checked for the actual selected
versions. Direct versions and minimal required features are pinned in Cargo.toml;
Cargo.lock pins transitive versions.

| Dependency | Version | Declared minimum Rust | License |
| --- | --- | --- | --- |
| Axum | 0.8.9 | 1.80 | MIT |
| Tokio | 1.54.1 | 1.85 | MIT |
| tokio-util | 0.7.20 | 1.85 | MIT |
| dotenvy | 0.15.7 | 1.56.1 | MIT |
| Serde | 1.0.229 | 1.56 | MIT OR Apache-2.0 |
| serde_json | 1.0.154 | 1.71 | MIT OR Apache-2.0 |
| nix | 0.31.3 | 1.69 | MIT |
| tracing | 0.1.44 | 1.65.0 | MIT |
| tracing-subscriber | 0.3.23 | 1.65.0 | MIT |
| Chrono | 0.4.45 | 1.62.0 | MIT OR Apache-2.0 |

Axum's matchit 0.8.4 dependency requires **both MIT and BSD-3-Clause**, not an
MIT-only interpretation. The BSD attribution/non-endorsement conditions are
compatible with this workspace; retain both license notices in delivered
third-party notices. `deny.toml` explicitly permits the qualified MIT,
Apache-2.0, BSD-3-Clause, Unicode-3.0 and Zlib obligations, including development
and build dependencies. Zlib applies to foldhash: preserve the original license,
do not misrepresent authorship, and mark any altered source. New licenses require
qualification, not a blanket exception. There are no ignored advisories; yanked versions and unknown sources
are rejected. Final SBOM/notices/provenance and architecture execution remain R13
gates rather than claims made by this initial manifest.

Primary references:
[Axum](https://docs.rs/axum/0.8.9/axum/),
[Tokio process](https://docs.rs/tokio/1.54.1/tokio/process/),
[blocking work](https://docs.rs/tokio/1.54.1/tokio/task/fn.spawn_blocking.html),
[TaskTracker](https://docs.rs/tokio-util/0.7.20/tokio_util/task/struct.TaskTracker.html),
[Linux subreaper](https://docs.rs/nix/0.31.3/nix/sys/prctl/fn.set_child_subreaper.html),
[dotenv iterator](https://docs.rs/dotenvy/0.15.7/dotenvy/fn.from_path_iter.html),
[cargo-deny policy](https://embarkstudios.github.io/cargo-deny/checks/).

## SQLite dependency qualification

SQLx is fixed to upstream revision
`10462fa4414c6c7f1ed1d39bb31678e3b726878c` (version 0.9.0, Rust 1.94,
MIT OR Apache-2.0). The registry 0.9.0 release caps libsqlite3-sys below
0.38.0 and cannot select the security fix for
[RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459).
The pinned upstream commit widens the supported SQLite binding range; no local
fork, copied patch or ignored advisory is used. Only that upstream Git source
is allowed in addition to crates.io. The lockfile selects libsqlite3-sys 0.39.1
with patched bundled SQLite 3.54.1. Keep the lockfile; do not substitute a system
SQLite whose security patch level has not been qualified.

Migration and incremental `sqlite3_backup` access use the safe rusqlite 0.41.1
binding (Rust 1.89, MIT, backup plus cancellation progress hooks) to the **same** libsqlite3-sys.
SQLx does not expose a safe incremental backup API, and its serialization API
would allocate the entire database. This avoids custom unsafe FFI and keeps
backup work incremental and cancellation-aware. Application connections remain
SQLx, with four bounded connections, five-second busy/acquire limits and foreign
keys enabled on every connection. A disposable migration connection alone turns
FK enforcement off for SQLite's documented transactional table rebuild, then
checks schema, FKs and integrity before commit. Migration never performs remote
I/O. Readers neither create a missing DB nor run migrations/recovery.

The published ledger remains `schema_migrations` versions 1–7; SQLx's migration
feature is disabled. Existing enum names, opaque credentials, JSON strings,
identities, indices, triggers and extension columns are not rewritten casually.
Migration/startup is not full crash-recovery or pre/post-write rollback acceptance;
those must still be proved against the complete service and the old application.

Primary references:
[upstream fix](https://github.com/transact-rs/sqlx/commit/10462fa4414c6c7f1ed1d39bb31678e3b726878c),
[SQLite backup](https://sqlite.org/backup.html),
[rusqlite backup](https://docs.rs/rusqlite/0.41.1/rusqlite/backup/struct.Backup.html),
[SQLite patched release](https://sqlite.org/releaselog/3_54_1.html),
[Zlib license](https://spdx.org/licenses/Zlib.html).

## Persisted scalar compatibility

The native core distinguishes uppercase SQLAlchemy enum names from lowercase
wire values and rejects unknown names instead of silently inventing a status.
Timestamp encoding retains six SQL fractional digits and Python's conditional
six-digit ISO API fractions, including the existing aware/naive distinction.
Chrono currently enables only `std`; no ambient local timezone is inferred.

Persisted JSON uses the Python default separators, ASCII escaping and insertion
order. Storage identity can sort its already-stringified fields explicitly; the
native output is checked against every frozen identity byte. Serde JSON enables
`preserve_order`, `arbitrary_precision` and `float_roundtrip`: actual negative
controls found a large integer converted into a rounded float and a one-bit f64
parsing difference without the latter features. The frozen current-Python
controls include independently captured numeric literals/bits, Unicode controls,
large integers, signed zero and SQLAlchemy datetime encodings. Special legacy
SQLite floating extension values in quarantine retain Python's spelling; this
is not a promise to accept nonstandard JSON request bodies.

`docs/contracts/python-control-18d8c3d/scalar-values.json` was captured using the
unchanged Python control image (Python 3.14.10 / SQLAlchemy 2.1.6) with standard
library/dialect imports only, not the application lifespan. Native SQLite writes
are also compared field-for-field to the earlier independent ORM value snapshot.
These are encoding tests, not complete application/reverse-cutover acceptance.

Primary references: [Serde JSON features](https://docs.rs/crate/serde_json/1.0.154/features),
[Chrono](https://docs.rs/chrono/0.4.45/chrono/).

## Managed rclone preparation (partial accepted-143 port)

`ManagedConfigBuilder` accepts only the offline validator's `ValidatedState`,
explicit bounded async/blocking/process owners, a trusted rclone binary path and
an existing deployment `CredentialCipher`. Its caller must hold the durable
setup claim and preparation journal throughout settlement. The returned encrypted
state descriptor is **not activation**; atomic DB/admin/session activation,
managed export/execution and the four recovery HTTP routes remain separate work.

Preparation creates a new private generation without replacing an existing one,
anchors its files and local RC socket to held directory descriptors, encrypts the
empty native rclone config **before** supplying any imported credentials, then
imports through a private Unix RC endpoint using the accepted memory-bootstrap
protocol. `config_*` fields survive separately as encrypted overrides; file
references remain placeholders and no imported provider credential file is
materialized. No provider onboarding or remote connection is performed here.
A deployment Fernet cipher wraps the generated unlock secret, portable files and
overrides; this introduces no new archive/key format or crypto implementation.

The existing reqwest 0.13.5 Unix-socket client disables proxies/redirects, limits
RC responses to 8 MiB and reports only generic errors. Commands retain the source
30-second bound, RPC five seconds and RC readiness ten seconds. Cancellation and
early daemon exit physically reap the child before cleanup and permit release.
Cleanup refuses a substituted generation name. Existing process stdout stays
64 KiB by default; explicitly requested machine-readable stdout may be at most
8 MiB, without enlarging stderr. The already-locked zeroize dependency is reused
by the service; no dependency version or license policy changes.

Run both actual offline CLI probes with the same accepted rclone 1.72.1 binaries
and architecture-specific checksums used by the current product:

```sh
docker build -f docker/rust-dev.Dockerfile -t dockervault-rust-ci .
docker build -f docker/rclone-test.Dockerfile -t dockervault-rclone-ci .
docker run --rm --network none --cpus 2 --memory 6g --user 1000:1000 dockervault-rclone-ci
```

The native CI matrix runs these probes on its actual amd64/arm64 runners, not
cross-compilation. The capabilities probe compares all 65 actual provider option
catalogs to independent accepted-source controls. The generation probe checks
seven configurations through actual native `config dump`, file modes, encryption,
Fernet material recovery, file references and retained OCI overrides. These are
component proofs, not service restart/activation, provider transfer, rollback or
complete R0–R14 acceptance.

Logical `recovery_import::insert_state` likewise accepts only `ValidatedState`
and a caller-owned, FK-enabled SQLite transaction. It reuses the offline stage's
exact enum/datetime/lossless-JSON binder, protects only the three storage and two
Komodo secret fields with the deployment Fernet cipher, and relocates archive
and sidecar paths lexically to the configured root. It neither commits nor deletes
rows or creates files; late SQL failure/cancellation remains the caller's rollback
obligation. Retained deployment extension columns keep their defaults. Actual
accepted-provider SQL snapshots and all 78 independent opaque-JSON/SQL-affinity
cases are native assertions; full activation and pre/postwrite rollback remain open.

Primary references:
[reqwest Unix sockets](https://docs.rs/reqwest/0.13.5/reqwest/struct.ClientBuilder.html#method.unix_socket),
[rclone RC security and config/create](https://rclone.org/rc/#config-create-create-a-config-remote).

## Credential encoding compatibility

The existing Fernet primitive alone is stricter about Base64 framing than the
accepted Python 3.14.10 credential loader/decryptor. Actual unchanged-provider
controls reproduce accepted `!`/whitespace/Unicode noise, interior padding and
nonzero unused bits; source framing still requires sufficient terminal padding.
A private adapter canonicalizes only those encoded bytes before calling the
same Fernet implementation. It does not change the encryption/MAC, 32-byte key
requirement, UTF-8 plaintext, prefix/legacy behavior or output format, and cannot
bypass authentication. Strict bundle/key-sidecar/SSH formats are untouched.

The native regression consumes 2,478 actual-source transformation recipes for
both key construction and authenticated recovery, including accepted but wrong
keys that must fail authentication. Keys are derived from public bytes0..31 at
test time, never embedded as credential-shaped literals. Buffers containing key
encoding are zeroized. No dependency, key rotation or custom cryptography is added.
Use the current source's non-validating framing, not an older Base64 decoder's
stop-at-padding assumption.

`CredentialCipher::read_key` parses existing UTF-8 key text incrementally with
8 KiB reads, a single partial UTF-8 character and at most one encoded 32-byte
key. Large ignored text does not become an unbounded allocation. Invalid UTF-8,
incomplete padding and cancellation fail closed; no key is generated or rewritten.
`RecoveryGeneration::load_instance_cipher` adds the borrowed setup claim and
matching zero-user/preparing-journal fence, explicit configuration precedence
(with no invalid-key fallback), no-follow descriptor-bound regular-file access,
full raw-file SHA256 against journaled ownership when applicable, and durable
0600 permissions. It never replaces a missing/invalid key; its caller must first
use the existing journaled `prepare_instance_key` path. This is prepared-custody
loading, not full application startup, final activation or a key-rotation API.
Actual tests include sparse 64 MiB file read -> IN_ACCESS -> cancellation ->
physical owner settlement without changing key identity/mode or the journal.

Primary references:
[CPython 3.14 Base64](https://docs.python.org/3.14/library/base64.html),
[exact 3.14.10 binascii framing](https://github.com/python/cpython/blob/v3.14.10/Modules/binascii.c),
[Fernet authenticated encryption](https://cryptography.io/en/latest/fernet/).

## Ownership invariants

Dropping an HTTP wait future requests cooperative cancellation; it does not
abort owned work or release its permit. Started blocking work is joined. A
finite shutdown grace requests cancellation when it expires but still waits for
physical settlement. Zero configured grace means unlimited draining, not an
immediate timeout. Environment parsing preserves false, empty and zero values,
case-sensitive names, published boolean/numeric coercions and dotenv precedence;
it never mutates the process environment or dumps secret values.

The Linux process adapter uses absolute executable paths, explicit argv, a
minimal environment, bounded stdin/stdout/stderr, a process group and subreaper.
Cancellation/timeout/output overflow terminates, escalates and reaps before
returning. A successful direct-child exit does not legitimize a still-running
descendant. Command arguments and captured bytes are excluded from Debug output.
Privileged-worker protocol tests, complete application startup/recovery ordering,
health/observability, all providers and final end-to-end gates are still required.
