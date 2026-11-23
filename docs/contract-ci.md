# Real contract / roundtrip CI

The `Rust foundations` and `Tests` workflows run distinct gates. Check the exact candidate revision and individual job steps before using a run as evidence.

## Native candidate package

[`Rust foundations`](../.github/workflows/rust-native.yml) builds the default application image on native Linux amd64 and arm64 runners. Its package checks use synthetic fixtures in isolated containers and do not publish an image or deploy it.

- **Runtime and persisted effects:** the default image starts with s6, runs the Rust service as UID 1000, and serves the frontend and health check through nginx. The queued roundtrip checks real HTTP backup and restore. The separate S3 check tests plain and encrypted backups restored from isolated S3 storage after a restart.
- **Offline recovery:** Chromium downloads the tool and the generated instructions from the packaged frontend. The test checks the download against its manifest and uses the downloaded binary, with the app stopped and networking disabled, to run the instructions from the README-linked guide, key export, and frontend dialog. The standalone fixture check is separate and compares restored v1 and legacy archive bytes with the originals. These jobs cover Linux amd64 and arm64 only.
- **Fenced rollback:** the native and pinned Python images exercise exclusive writer startup and plain/encrypted roundtrips. Chromium also checks the recovery download from the fenced Python package. These checks do not prove recovery of a restore interrupted after a persistent write.
- **Checks and artifacts:** the workflow runs locked offline Cargo tests, fmt and clippy, checks advisories and licenses, and exports unpublished OCI layouts in a separate job. A green build alone does not establish runtime, platform or performance parity.

## Python compatibility checks

The `Tests` workflow still runs the maintained Python test suite during the transition. The following Python tools remain available as comparison oracles. Running one does not establish native parity, and the pytest coverage percentage is not a Rust coverage result.

- **Real HTTP app backup and restore, no seeded catalog**: `python3 backend/tests/run_backup_roundtrip.py dockervault:test` expects a compatible Python image, not the default Rust image. It starts a uniquely named `--rm` container with no host Docker socket or published ports. `runtime_backup_roundtrip.py` uses nginx, setup/login, the target and backup APIs, a real archive and catalog, source mutation, and the restore API before comparing bytes. It also checks age/OpenSSL encryption and storage-only restore through the Python local provider after restarting the supervised backend. The original source layout, directory symlink and temporary cleanup are part of this oracle.
- **Provider/catalog restart contracts, not archive creation or restore**: `test_sync_restart.py` drives `runtime_sync_probe.py` with intentionally synthetic opaque bytes to test manual sync/retry, exact sidecar identity and file-backed catalog recovery across process lifetimes. Those tests do **not** exercise app backup creation, tar/crypto correctness or the restore API; the HTTP image gate above does.
- **Standalone encrypted recovery**: `runtime_recovery_instructions.py` executes the Python-era age/OpenSSL commands against the Python image, independently of app restore.
- **Interruption/migration and bounded browsing**: native Docker cancellation, crash/restart, repeated/interrupted SQLite migrations, full provider roundtrips and large plain/encrypted/local/remote archive paging remain separate canonical regressions. Browsing retains at most one bounded member window; preparation, worker settlement and cleanup share capacity.
- **Frontend caller contracts**: MSW reflects real API behavior, including DELETE 200, explicit clears and bounded plain/encrypted archive page parameters/headers. The actual browser can move forward and back beyond the default 2000 entries without accumulating all pages.

## Dependency scan policy

`pip-audit` is blocking. Npm audit is also blocking on execution/parse/schema errors and every **new or changed high/critical** finding, including a new advisory on an already-excepted package. There is no blanket `continue-on-error`. The `dockervault-npm-audit` binary runs the same `npm audit --json`, preserves the full raw report as a CI artifact and matches only `docs/npm-audit-exceptions.json`: exact installed versions/nodes, advisory URLs, ranges and severities, tracking issue, reason and expiry. Removal of known findings is allowed; expansion is not. The previous Python wrapper and tests remain as the historical comparison. Policy tests inject unknown high and critical findings, change a known advisory/range/severity/version, expire an exception, and inject an audit error; all must fail closed.

For the findings still present, use the raw report attached to the exact run. The exception file records the expiry and tracker for reviewed findings, not the current finding count. **A passing audit can still contain deferred known debt.** Browser advisories may remain reachable; build-time packages can still affect CI.

Mypy and the existing advisory Trivy/SARIF steps remain nonblocking and must be reported separately from successful mandatory tests. No scan's `continue-on-error` means security success.
