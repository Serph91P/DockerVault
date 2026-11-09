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
binding (Rust 1.89, MIT, only the backup feature) to the **same** libsqlite3-sys.
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
