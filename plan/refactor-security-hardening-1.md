---
goal: Security Hardening — Remediate all critical and important findings from full OWASP security review
version: 2.0
date_created: 2026-02-24
last_updated: 2026-02-24
owner: DockerVault maintainers
status: 'Planned'
tags: [security, refactor, owasp, hardening, bug]
---

# Introduction

![Status: Planned](https://img.shields.io/badge/status-Planned-blue)

Full OWASP security review of the DockerVault workspace identified **3 critical**, **5 important**, and **4 suggestion-level** findings. This plan remediates all critical and important issues and addresses suggestions as low-priority improvements. The review covered secrets detection, authentication/session security, path traversal, injection vectors, Docker socket access, CORS policy, encryption implementation, and Dockerfile/infrastructure configuration.

## 1. Requirements & Constraints

- **REQ-001**: All remote storage credentials (`password`, `s3_secret_key`, `s3_access_key`) must be encrypted at rest in the SQLite database.
- **REQ-002**: CORS policy must not combine wildcard origins with credentials; must restrict to known frontend origin(s).
- **REQ-003**: Pre/post backup hook commands must be restricted to prevent arbitrary code execution.
- **REQ-004**: WebSocket endpoint must authenticate connections before granting access to backup event streams.
- **REQ-005**: Session cookie `secure` flag must be configurable and default to `True`.
- **REQ-006**: Authentication endpoints must enforce rate limiting against brute-force attacks.
- **REQ-007**: Temp files from storage downloads must be cleaned up after response delivery.
- **REQ-008**: `tar.extractall` must use the `filter` parameter for defense-in-depth on Python 3.12+.
- **SEC-001**: No credentials, PII, or secrets may be stored in plaintext in the database or logs.
- **SEC-002**: All subprocess invocations must avoid `shell=True` and validate input arguments.
- **SEC-003**: Private key material must not persist on disk after use.
- **CON-001**: The container must run as root in docker-compose for Docker socket access — this is an accepted design constraint but must be documented.
- **CON-002**: Backward compatibility with existing SQLite databases must be maintained (migration required for encrypted credentials).
- **GUD-001**: Defense-in-depth — apply multiple layers of validation (e.g., both manual path checks and `tar` filter parameter).
- **GUD-002**: All security-sensitive configuration should be environment-variable-driven with safe defaults.
- **PAT-001**: Use `cryptography.fernet.Fernet` for symmetric credential encryption, keyed from an auto-generated server-side secret.
- **PAT-002**: Use FastAPI dependency injection for auth checks on WebSocket and API endpoints.

## 2. Implementation Steps

### Implementation Phase 1: Critical — Data Protection & Injection Prevention

- GOAL-001: Eliminate plaintext credential storage, fix CORS credential leak, and restrict hook command execution.
- COMPLETION CRITERIA: (a) `SELECT password, s3_secret_key, s3_access_key FROM remote_storage` returns only Fernet-prefixed ciphertext, never plaintext. (b) `app.add_middleware(CORSMiddleware, ...)` uses an explicit origins list from env var, not `["*"]` with `allow_credentials=True`. (c) `_run_hook()` rejects any binary not in `settings.ALLOWED_HOOK_COMMANDS`.
- TASK DEPENDENCIES: TASK-002 must complete before TASK-001 and TASK-003. TASK-005 must complete before TASK-006. TASK-004 has no dependencies.

| Task | Description | Completed | Date |
|------|-------------|-----------|------|
| TASK-002 | **Create credential encryption helper module.** Create new file `backend/app/credential_encryption.py`. Implement two functions: `encrypt_value(plaintext: str) -> str` and `decrypt_value(ciphertext: str) -> str` using `cryptography.fernet.Fernet`. The Fernet key is sourced from `settings.CREDENTIAL_ENCRYPTION_KEY` (added in TASK-004's config changes). If the env var is empty at startup, auto-generate a key via `Fernet.generate_key()`, write it to `/app/data/.credential_key` with permissions `0o600`, and load it. Encrypted values must be prefixed with `fernet:` to distinguish them from legacy plaintext. `decrypt_value()` must return the input unchanged if it does not start with `fernet:` (backward compatibility). Add `from app.config import settings` import. **Verification**: `cd backend && python -c "from app.credential_encryption import encrypt_value, decrypt_value; v = encrypt_value('test'); assert decrypt_value(v) == 'test'; assert v.startswith('fernet:'); print('PASS')"` | | |
| TASK-001 | **Add credential migration to database init.** In `backend/app/database.py` function `init_db()` (called at app startup), after existing `ALTER TABLE` migrations, add a migration block that: (1) queries all rows from `remote_storage`, (2) for each row where `password`, `s3_secret_key`, or `s3_access_key` is not `NULL` and does not start with `fernet:`, calls `encrypt_value()` on the value, (3) updates the row with the encrypted value. Import `encrypt_value` from `app.credential_encryption`. **Verification**: Create a test SQLite DB with a plaintext password, run `init_db()`, query the row, assert the password starts with `fernet:` and `decrypt_value()` returns the original. | | |
| TASK-003 | **Update storage API to encrypt/decrypt credentials.** In `backend/app/api/storage.py`: (a) In the `create_storage()` endpoint, after building the `RemoteStorageModel` instance, call `encrypt_value()` on `password`, `s3_secret_key`, and `s3_access_key` fields before `session.add()`. (b) In the `update_storage()` endpoint, encrypt any credential fields present in the update payload before `session.commit()`. (c) In `_db_to_config()` function, call `decrypt_value()` on `storage.password`, `storage.s3_secret_key`, and `storage.s3_access_key` before passing to `StorageConfig`. Import `encrypt_value, decrypt_value` from `app.credential_encryption`. **Verification**: `cd backend && python -m pytest tests/ -k "storage" -v` — all storage tests pass. Manual: create a storage via API, inspect SQLite DB, confirm credential columns contain `fernet:`-prefixed values. | | |
| TASK-004 | **Fix CORS policy and add new config fields.** (a) In `backend/app/config.py`, add these fields to the `Settings` class: `CORS_ORIGINS: str = "http://localhost"` (comma-separated string), `COOKIE_SECURE: bool = True`, `CREDENTIAL_ENCRYPTION_KEY: str = ""`, `ALLOWED_HOOK_COMMANDS: str = "pg_dump,pg_dumpall,mysqldump,mongodump,redis-cli,mariadb-dump"`. (b) In `backend/app/main.py` at line 113, replace `allow_origins=["*"]` with `allow_origins=settings.CORS_ORIGINS.split(",")`. Replace `allow_credentials=True` with `allow_credentials=True`. Remove the comment `# allow all origins since we run behind nginx`. Add `from app.config import settings` import if not present. **Verification**: `cd backend && CORS_ORIGINS="http://localhost:3000" python -c "from app.config import settings; assert settings.CORS_ORIGINS == 'http://localhost:3000'; print('PASS')"` and `grep -n 'allow_origins=\["\*"\]' backend/app/main.py` returns no matches. | | |
| TASK-005 | **Restrict hook commands with allowlist.** In `backend/app/backup_engine.py`, modify `_run_hook()` (line 817). After `args = shlex.split(command)`, add validation: `allowed = settings.ALLOWED_HOOK_COMMANDS.split(",")`, then `if args[0] not in allowed: raise Exception(f"Hook command '{args[0]}' is not in the allowed commands list: {allowed}")`. Add `from app.config import settings` import. Add `logger.info(f"Executing hook command: {args[0]} (full: {command})")` before `create_subprocess_exec`. **Verification**: `cd backend && python -c "from app.backup_engine import backup_engine; import asyncio; asyncio.run(backup_engine._run_hook('rm -rf /'))"` raises `Exception` containing "not in the allowed commands list". | | |
| TASK-006 | **Add hook command validation to targets API.** In `backend/app/api/targets.py`, add a shared Pydantic `field_validator` to both `TargetCreate` and `TargetUpdate` models for fields `pre_backup_command` and `post_backup_command`. The validator must: (1) if value is `None` or empty string, return `None`, (2) parse with `shlex.split(value)`, (3) check `args[0]` against `settings.ALLOWED_HOOK_COMMANDS.split(",")`, (4) if not found, raise `ValueError(f"Command '{args[0]}' is not allowed. Allowed: {allowed}")`. Import `shlex` and `from app.config import settings`. **Verification**: `cd backend && python -c "from app.api.targets import TargetCreate; TargetCreate(name='test', target_type='volume', volume_name='v', pre_backup_command='rm -rf /')"` raises `ValidationError`. | | |

### Implementation Phase 2: Important — Authentication & Session Hardening

- GOAL-002: Secure WebSocket authentication, harden session cookies, add rate limiting, and fix resource leaks.
- COMPLETION CRITERIA: (a) WebSocket connection to `/ws/updates` without a valid `token` query parameter returns close code `4001`. (b) Login response cookie has `secure=True` by default. (c) 6th POST to `/api/v1/auth/login` within 60 seconds from same IP returns HTTP 429. (d) No orphan temp directories remain in `/tmp` after storage file download. (e) `tar.extractall` call includes `filter='data'` keyword argument.
- TASK DEPENDENCIES: All tasks in this phase are independent and may execute in parallel. TASK-009 depends on DEP-002 (`slowapi`).

| Task | Description | Completed | Date |
|------|-------------|-----------|------|
| TASK-007 | **Add WebSocket authentication.** In `backend/app/websocket.py`, modify `websocket_endpoint()` (line 88). Before `await manager.connect(websocket)`, add: (1) `token = websocket.query_params.get("token")`, (2) if `token` is `None`, call `await websocket.close(code=4001, reason="Authentication required")` and `return`, (3) `async with async_session() as db: user = await get_session_user(token, db)`, (4) if `user` is `None`, call `await websocket.close(code=4001, reason="Invalid or expired session")` and `return`. Add imports: `from app.auth import get_session_user` and `from app.database import async_session`. In `backend/app/main.py` line 34, remove `"/ws"` from the `PUBLIC_PATHS` set. **Verification**: `cd backend && python -c "from app.main import PUBLIC_PATHS; assert '/ws' not in PUBLIC_PATHS; print('PASS')"` and WebSocket connection without `?token=...` receives close code `4001`. | | |
| TASK-008 | **Make session cookie `secure` flag configurable.** `COOKIE_SECURE` was added to `backend/app/config.py` in TASK-004. In `backend/app/api/auth.py` line 151, replace `secure=False,  # Set to True in production with HTTPS` with `secure=settings.COOKIE_SECURE,`. At line 207, make the same replacement. Add `from app.config import settings` import if not present. **Verification**: `grep -n 'secure=False' backend/app/api/auth.py` returns no matches. `grep -n 'secure=settings.COOKIE_SECURE' backend/app/api/auth.py` returns exactly 2 matches (lines ~151 and ~207). | | |
| TASK-009 | **Add rate limiting to auth endpoints.** (a) Add `slowapi>=0.1.9` to `backend/requirements.txt`. Run `pip install slowapi`. (b) In `backend/app/main.py`, add: `from slowapi import Limiter, _rate_limit_exceeded_handler` and `from slowapi.util import get_remote_address` and `from slowapi.errors import RateLimitExceeded`. Before `app = FastAPI(...)`, add `limiter = Limiter(key_func=get_remote_address)`. After `app = FastAPI(...)`, add `app.state.limiter = limiter` and `app.add_exception_handler(RateLimitExceeded, _rate_limit_exceeded_handler)`. (c) In `backend/app/api/auth.py`, add `from slowapi import Limiter` and `from app.main import limiter` (or pass limiter via app state). Add `@limiter.limit("5/minute")` decorator to the `login()`, `setup()`, and `change_password()` endpoint functions. Each decorated function must accept `request: Request` as its first parameter (add `from fastapi import Request` import). **Verification**: `cd backend && pip install slowapi && python -c "from app.main import app; print('PASS')"` succeeds. Start server, send 6 POST requests to `/api/v1/auth/login` within 60 seconds — 6th returns HTTP 429. | | |
| TASK-010 | **Fix temp file leak in storage downloads.** In `backend/app/api/storage.py` at the `download_file()` function (line 363), add import: `from starlette.background import BackgroundTask`. Define a cleanup function inside `download_file`: `def cleanup(): import shutil; shutil.rmtree(temp_dir, ignore_errors=True)`. Replace `background=None,` in the `FileResponse(...)` constructor with `background=BackgroundTask(cleanup),`. Remove the manual cleanup in the `except HTTPException` block (the BackgroundTask handles all cases). **Verification**: `ls /tmp/tmp* | wc -l` before and after downloading a file via the API — count does not increase after response completes. | | |
| TASK-011 | **Add `filter='data'` to `tar.extractall`.** In `backend/app/backup_engine.py` at the `_extract_tar()` method (line 987), add `import sys` at the top of the file if not present. Replace the line `tar.extractall(dest)` (line 1027) with: `if sys.version_info >= (3, 12): tar.extractall(dest, filter='data')` / `else: tar.extractall(dest)`. **Verification**: `cd backend && python -c "import sys; assert sys.version_info >= (3, 12); print('PASS')"` and `grep -n "filter='data'" backend/app/backup_engine.py` returns exactly 1 match. | | |

### Implementation Phase 3: Suggestions — Infrastructure Hardening

- GOAL-003: Harden nginx, supervisord, encryption temp file handling, and API consistency.
- COMPLETION CRITERIA: (a) `curl -sI http://localhost/ | grep -i content-security-policy` returns a CSP header. (b) `ps aux | grep uvicorn` shows user `dockervault`, not `root`. (c) No temp files containing private key material exist after `decrypt_dek()` completes. (d) `list_files` endpoint rejects path containing `..`. (e) `README.md` contains "Security Considerations" section.
- TASK DEPENDENCIES: All tasks in this phase are independent and may execute in parallel. TASK-013 depends on the `dockervault` user existing in the Docker image (already created in Dockerfile).

| Task | Description | Completed | Date |
|------|-------------|-----------|------|
| TASK-012 | **Add Content-Security-Policy header to nginx.** In `frontend/nginx.conf`, within the `server { }` block, after the existing `add_header X-XSS-Protection` line, add: `add_header Content-Security-Policy "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' ws: wss:; font-src 'self';" always;`. **Verification**: `grep -c 'Content-Security-Policy' frontend/nginx.conf` returns `1`. | | |
| TASK-013 | **Run backend as non-root in supervisord.** In `docker/supervisord.conf`, in the `[program:backend]` section, add `user=dockervault` on a new line after `directory=/app`. The `dockervault` user (UID 1000) is already created in the Dockerfile and added to the docker group in `docker/entrypoint.sh`. **Verification**: `grep 'user=dockervault' docker/supervisord.conf` returns `1` match. After container restart, `docker exec <container> ps aux | grep uvicorn` shows `dockervault` user. | | |
| TASK-014 | **Harden private key temp file in decrypt_dek.** In `backend/app/encryption.py` function `decrypt_dek()` (line 185), replace `with tempfile.NamedTemporaryFile(mode="w", suffix=".key", delete=False) as f:` with `with tempfile.NamedTemporaryFile(mode="w", suffix=".key", delete=True) as f:` and update `f.name` usage: assign `key_file = f.name` inside the `with` block, then set file permissions `os.chmod(key_file, 0o600)` before writing. Move the `age` subprocess call inside the `with` block so the file is auto-deleted when the block exits. Remove the `os.unlink(key_file)` in the `finally` block (now handled by `delete=True`). **Verification**: `grep -c 'delete=True' backend/app/encryption.py` returns at least `1`. `grep -c 'os.unlink(key_file)' backend/app/encryption.py` returns `0`. | | |
| TASK-015 | **Validate path in `list_files` endpoint.** In `backend/app/api/storage.py` function `list_files()` (line 337), after `if not storage:` check, add `safe_path = _validate_remote_path(path)` and replace `files = await backend.list_files(path)` with `files = await backend.list_files(safe_path)`. Also update the return to use `safe_path`: `return {"files": files, "path": safe_path}`. **Verification**: `cd backend && python -c "from app.api.storage import _validate_remote_path; _validate_remote_path('../etc/passwd')"` raises `HTTPException` with status 400. | | |
| TASK-016 | **Document root container security posture.** Add a `## Security Considerations` section to `README.md` before any existing `## Contributing` or `## License` section. Content must include 4 subsections: (a) `### Container Privileges` — explain that root is required for Docker socket and volume access. (b) `### Docker Socket Access` — explain that mounting `/var/run/docker.sock` grants full Docker API access; recommend restricting network exposure. (c) `### Recommended Deployment` — recommend deploying behind a reverse proxy with TLS termination; set `COOKIE_SECURE=true`. (d) `### Security Environment Variables` — table with columns `Variable`, `Default`, `Description` listing: `CORS_ORIGINS` (default `http://localhost`), `COOKIE_SECURE` (default `true`), `CREDENTIAL_ENCRYPTION_KEY` (default auto-generated), `ALLOWED_HOOK_COMMANDS` (default `pg_dump,pg_dumpall,mysqldump,mongodump,redis-cli,mariadb-dump`). **Verification**: `grep -c 'Security Considerations' README.md` returns `1`. `grep -c 'CORS_ORIGINS\|COOKIE_SECURE\|CREDENTIAL_ENCRYPTION_KEY\|ALLOWED_HOOK_COMMANDS' README.md` returns `>= 4`. | | |

## 3. Alternatives

- **ALT-001**: Use SQLCipher for full database encryption instead of per-field credential encryption. Rejected: adds significant complexity, requires a different SQLite driver (`pysqlcipher3`), and most DB fields do not contain sensitive data. Per-field encryption is surgical and backward-compatible.
- **ALT-002**: Use Docker secrets for credential storage instead of encrypted DB columns. Rejected: DockerVault may run outside Swarm mode, and the current SQLite-based architecture would require significant refactoring to use external secret stores.
- **ALT-003**: Disable hook commands entirely. Rejected: pre/post backup hooks are a valuable feature for database-consistent backups (e.g., `pg_dump` before volume snapshot). An allowlist provides a balanced approach.
- **ALT-004**: Use JWT tokens instead of session cookies. Rejected: the current session-based auth is simpler, already implemented, and sufficient for a single-user/few-user application. JWT adds token revocation complexity.
- **ALT-005**: Use WebSocket subprotocol for authentication instead of query parameter token. Rejected: subprotocol-based auth requires custom client-side logic and is less portable. Query parameter is simpler and used by most real-time frameworks.

## 4. Dependencies

- **DEP-001**: `cryptography>=42.0.0` Python package — Fernet symmetric encryption for credential at-rest protection. Add to `backend/requirements.txt`. Install: `pip install cryptography>=42.0.0`.
- **DEP-002**: `slowapi>=0.1.9` Python package — rate limiting middleware for FastAPI. Add to `backend/requirements.txt`. Install: `pip install slowapi>=0.1.9`.
- **DEP-003**: `age` CLI tool — already installed in Dockerfile (`RUN apt-get install -y age`). Required for envelope encryption features.
- **DEP-004**: Python >= 3.12 — required for `tar.extractall(filter='data')` support. Current Dockerfile uses Python 3.14 base image. Verified.

## 5. Files

- **FILE-001**: `backend/app/credential_encryption.py` — **NEW FILE**. Symmetric encryption utility: `encrypt_value()`, `decrypt_value()`, auto-key generation. ~60 lines.
- **FILE-002**: `backend/app/config.py` — **MODIFY**. Add 4 new settings fields: `CORS_ORIGINS`, `COOKIE_SECURE`, `CREDENTIAL_ENCRYPTION_KEY`, `ALLOWED_HOOK_COMMANDS`. ~8 lines added.
- **FILE-003**: `backend/app/database.py` — **MODIFY**. Add credential migration block in `init_db()` to encrypt existing plaintext values. ~20 lines added.
- **FILE-004**: `backend/app/api/storage.py` — **MODIFY**. (a) Encrypt credentials on create/update. (b) Decrypt in `_db_to_config()`. (c) Add path validation to `list_files()`. (d) Fix temp file cleanup in `download_file()`. ~15 lines changed.
- **FILE-005**: `backend/app/main.py` — **MODIFY**. (a) Replace CORS wildcard with `settings.CORS_ORIGINS`. (b) Remove `/ws` from `PUBLIC_PATHS`. (c) Add `slowapi` limiter initialization. ~12 lines changed.
- **FILE-006**: `backend/app/api/auth.py` — **MODIFY**. (a) Replace `secure=False` with `settings.COOKIE_SECURE` at 2 locations (lines 151, 207). (b) Add `@limiter.limit("5/minute")` to 3 endpoints. ~8 lines changed.
- **FILE-007**: `backend/app/backup_engine.py` — **MODIFY**. (a) Add allowlist check in `_run_hook()` (line 817). (b) Add `filter='data'` to `tar.extractall()` (line 1027). (c) Add `import sys`. ~8 lines changed.
- **FILE-008**: `backend/app/api/targets.py` — **MODIFY**. Add `field_validator` for `pre_backup_command` and `post_backup_command` on `TargetCreate` and `TargetUpdate`. ~15 lines added.
- **FILE-009**: `backend/app/websocket.py` — **MODIFY**. Add token auth check at start of `websocket_endpoint()` (line 88). ~10 lines added.
- **FILE-010**: `backend/app/encryption.py` — **MODIFY**. Refactor `decrypt_dek()` to use `delete=True` and `0o600` permissions. ~5 lines changed.
- **FILE-011**: `frontend/nginx.conf` — **MODIFY**. Add `Content-Security-Policy` header. 1 line added.
- **FILE-012**: `docker/supervisord.conf` — **MODIFY**. Add `user=dockervault` to `[program:backend]`. 1 line added.
- **FILE-013**: `backend/requirements.txt` — **MODIFY**. Add `cryptography>=42.0.0` and `slowapi>=0.1.9`. 2 lines added.
- **FILE-014**: `README.md` — **MODIFY**. Add "Security Considerations" section with 4 subsections and env var table. ~40 lines added.

## 6. Testing

- **TEST-001**: `backend/tests/test_credential_encryption.py` — **NEW FILE**. Unit test `encrypt_value` → `decrypt_value` round-trip for strings `"password123"`, `""`, `"special!@#$%"`. Assert `encrypt_value("x") != encrypt_value("x")` (Fernet uses random IV). Assert `decrypt_value("plaintext_without_prefix")` returns `"plaintext_without_prefix"` (backward compat). Assert `decrypt_value("fernet:invalid")` raises `InvalidToken`.
- **TEST-002**: `backend/tests/test_api_storage_security.py` — **NEW FILE**. Integration test: create `RemoteStorage` via API with `password="secret"`, query SQLite directly, assert `password` column starts with `fernet:`. Call `_db_to_config()`, assert returned password is `"secret"`.
- **TEST-003**: `backend/tests/test_cors.py` — **NEW FILE**. Unit test: import `app` from `app.main`, assert CORS middleware `allow_origins` does not contain `"*"`. Set `CORS_ORIGINS="http://a.com,http://b.com"`, assert middleware origins list equals `["http://a.com", "http://b.com"]`.
- **TEST-004**: `backend/tests/test_hook_allowlist.py` — **NEW FILE**. Unit test: set `ALLOWED_HOOK_COMMANDS="pg_dump,mysqldump"`. Call `_run_hook("pg_dump --help")` — no exception. Call `_run_hook("rm -rf /")` — raises `Exception` with "not in the allowed commands list". Call `_run_hook("curl http://evil.com")` — raises `Exception`.
- **TEST-005**: `backend/tests/test_websocket_auth.py` — **NEW FILE**. Integration test using `TestClient` WebSocket: connect to `/ws/updates` without `token` param — assert close code is `4001`. Connect with `token=invalid_token` — assert close code is `4001`. Create a valid session, connect with `token=valid_session_token` — assert connection succeeds and receives `{"type": "connected"}` message.
- **TEST-006**: `backend/tests/test_rate_limiting.py` — **NEW FILE**. Integration test: send 5 POST requests to `/api/v1/auth/login` with invalid credentials — all return 401. Send 6th request — returns 429 with `Retry-After` header.
- **TEST-007**: In existing `backend/tests/test_backup_engine.py`, add test `test_extract_tar_uses_filter`: mock `tarfile.TarFile.extractall`, call `_extract_tar()`, assert `extractall` was called with `filter='data'` keyword argument.
- **TEST-008**: `backend/tests/test_storage_download_cleanup.py` — **NEW FILE**. Unit test: mock `FileResponse` and `BackgroundTask`, call `download_file()`, assert `BackgroundTask` was created with a cleanup callable. Execute the cleanup callable, assert temp directory no longer exists.
- **TEST-009**: In existing `backend/tests/test_api_backups.py` or new file, add test: call `list_files` endpoint with `path=../../etc/passwd` — assert HTTP 400 response with "Invalid file path" detail.
- **TEST-010**: `backend/tests/test_cookie_secure.py` — **NEW FILE**. Unit test: set `COOKIE_SECURE=True` in settings, call login endpoint, assert response `Set-Cookie` header contains `Secure`. Set `COOKIE_SECURE=False`, call login, assert `Secure` is absent.

## 7. Risks & Assumptions

- **RISK-001**: Encrypting existing credentials in a migration requires the encryption key to be available at startup. If the key is lost, all stored remote storage credentials become unrecoverable. **Mitigation**: Auto-generate and persist key to `/app/data/.credential_key`; log warning on first-boot generation; document key backup in README Security Considerations section.
- **RISK-002**: Adding rate limiting may affect automated API consumers (e.g., CI/CD scripts that authenticate). **Mitigation**: Rate limits are IP-based and generous (5/min on auth endpoints only); document the limit in README.
- **RISK-003**: Hook command allowlist may break existing user workflows if they use unlisted commands. **Mitigation**: Allowlist is configurable via `ALLOWED_HOOK_COMMANDS` env var; document the change in release notes; provide a permissive default list including common DB dump tools.
- **RISK-004**: WebSocket auth via query parameter exposes the token in server access logs and browser history. **Mitigation**: Nginx WebSocket proxy config does not log query parameters by default. Document that WebSocket URLs should not be logged in production. Future improvement: switch to first-message auth protocol.
- **RISK-005**: `slowapi` adds a new dependency and may conflict with existing middleware ordering. **Mitigation**: Initialize limiter before CORS middleware; test middleware chain in integration tests.
- **ASSUMPTION-001**: The application runs behind a reverse proxy (nginx in-container or external) that terminates TLS in production deployments.
- **ASSUMPTION-002**: The Docker socket mount is required and the root-in-compose design is intentional and accepted.
- **ASSUMPTION-003**: Python >= 3.12 is the target runtime. Current Dockerfile uses `python:3.14-slim` base image.
- **ASSUMPTION-004**: The `age` CLI tool version installed (via `apt-get install age`) supports `--version`, `-r`, `-d`, `-i`, and `-a` flags as used in `backend/app/encryption.py`.

## 8. Related Specifications / Further Reading

- [OWASP Top 10 (2021)](https://owasp.org/www-project-top-ten/)
- [OWASP CORS Misconfiguration](https://owasp.org/www-project-web-security-testing-guide/latest/4-Web_Application_Security_Testing/11-Client-side_Testing/07-Testing_Cross_Origin_Resource_Sharing)
- [CVE-2007-4559 — Python tarfile path traversal](https://nvd.nist.gov/vuln/detail/CVE-2007-4559)
- [Python 3.12 tarfile filter documentation](https://docs.python.org/3.12/library/tarfile.html#tarfile.TarFile.extractall)
- [age encryption tool](https://github.com/FiloSottile/age)
- [FastAPI CORS documentation](https://fastapi.tiangolo.com/tutorial/cors/)
- [slowapi rate limiting](https://github.com/laurentS/slowapi)
- [Fernet symmetric encryption (cryptography)](https://cryptography.io/en/latest/fernet/)
- [OWASP Rate Limiting Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Denial_of_Service_Cheat_Sheet.html)
- [Starlette BackgroundTask documentation](https://www.starlette.io/background/)
