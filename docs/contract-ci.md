# Real contract / roundtrip CI

The test suite already encodes cross-boundary regression gates that run in the `Tests` workflow, not just in-memory unit mocks:

- **Authenticated API → archive → catalog → byte-verified restore** across independent process lifetimes: `backend/tests/test_sync_restart.py` drives `runtime_sync_probe.py` (`sync`/`retry`/`recover`) with real setup/login, a real file-backed SQLite catalog and real local-provider copies, byte-comparing the restored archive and key sidecar after remote-only recovery.
- **Standalone encrypted recovery** runs the published age/OpenSSL commands via `runtime_recovery_instructions.py` in the `integration-tests` job against the built image.
- **Restart, retry, cancellation, catalog and migration invariants** are covered by `test_restart_recovery.py`, `test_docker_cancellation.py`, `test_catalog_integrity.py`, `test_migrations.py` and the other owned-fixture regression suites.
- **Frontend fixtures match backend responses**: MSW handlers reflect real route behavior (notably `DELETE /backups/{id}` returns `200` with `{"status": "deleted"}`, not `204`), and the API-layer tests assert that contract.
- **Security/dependency failures are surfaced, not silently swallowed**: `pip-audit` is a hard gate; the npm-audit `continue-on-error` remains only as a documented reachability exception for pre-existing frontend advisories tracked outside this backend scope.

New jobs use owned, uniquely-named disposable fixtures, never production access or a fixed-name host cleanup, and keep bounded runtime with deterministic cleanup.
