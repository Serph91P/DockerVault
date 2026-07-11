# DockerVault Storage Recovery and Komodo Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Fix storage maintenance UX, add encrypted first-run recovery, and make Komodo a safe stack lifecycle authority.

**Architecture:** Deliver four serial PRs. Each implementation uses backend service boundaries, typed APIs, TDD, and a separate read-only review-and-merge card. Durable operations provide truthful progress and survive frontend reconnects.

**Tech Stack:** FastAPI, SQLAlchemy async, aiohttp, React, TypeScript, TanStack Query, Zustand/WebSocket, pytest, Vitest, Ruff, Docker.

---

## Global execution rules

- Read `docs/superpowers/specs/2026-07-11-storage-recovery-komodo-design.md` before work.
- Fetch origin before every phase. Branch from current `origin/develop`; PR target is `develop`.
- Use one isolated worktree per implementation and a separate read-only reviewer worktree.
- Write regression tests first and record RED then GREEN evidence.
- Never expose credentials, absolute sensitive paths, private keys, or recovery passwords.
- Do not merge implementation PRs from implementation cards.
- Run backend Ruff format/check and pytest plus frontend format/lint/typecheck/test/build in one local batch before push when touched.
- Run `git diff --check`, secret scanning, and changed-file U+2013/U+2014 scanning.

## Phase 1: Safe storage deletion and directory sizes

### Task 1: Reproduce local directory deletion

**Files:**
- Create: `backend/tests/test_api_storage.py`
- Inspect: `backend/app/api/storage.py`
- Inspect: `backend/app/remote_storage.py`

**Steps:**
1. Add a nested local backup directory fixture containing files and a symlink fixture.
2. Add a test showing the current DELETE endpoint fails for a directory.
3. Run the focused test and record the expected RED result.
4. Add traversal, storage-root deletion, and symlink-escape regression cases.

Run: `cd backend && pytest -o addopts='' -q tests/test_api_storage.py -k 'local and delete'`

### Task 2: Introduce a typed local deletion service

**Files:**
- Create: `backend/app/storage_operations.py`
- Modify: `backend/app/api/storage.py`
- Modify: `backend/tests/test_api_storage.py`

**Contract:**

```python
@dataclass(frozen=True)
class DeleteResult:
    path: str
    success: bool
    kind: Literal['file', 'directory', 'symlink']
    error: str | None = None


def resolve_local_target(root: Path, relative_path: str) -> Path: ...
def delete_local_target(root: Path, relative_path: str) -> DeleteResult: ...
```

1. Resolve only normalized relative paths below the configured root.
2. Reject root deletion and escapes.
3. Unlink files and symlinks; recursively delete directories without following external symlinks.
4. Return safe typed outcomes and map expected errors to 4xx responses.
5. Run focused tests until GREEN.

### Task 3: Add non-blocking directory size jobs

**Files:**
- Modify: `backend/app/storage_operations.py`
- Modify: `backend/app/api/storage.py`
- Modify: `backend/tests/test_api_storage.py`
- Modify: `frontend/src/api/index.ts`
- Modify: `frontend/src/components/StorageBrowser.tsx`
- Create: `frontend/src/components/StorageBrowser.test.tsx`

**Contract:**

```json
{"path":"actual-stack-backup","state":"completed","bytes":123456}
```

1. Add failing tests for recursive size, symlink avoidance, permission failure, and cancellation.
2. Keep normal listing fast and request directory sizes separately.
3. Cache results using storage identity, path, safe modification fingerprint, and bounded TTL.
4. Render loading, formatted size, and unavailable states per row.
5. Verify listing does not wait for recursive scans.

### Task 4: Phase 1 full gate

Run as one batch:

```bash
cd backend && ruff format --check app tests && ruff check app tests && pytest
cd ../frontend && npm ci && npm run format:check && npm run lint && npm run typecheck && npm test -- --run && npm run build
cd .. && git diff --check
```

Build the Docker image and run a containerized nested-directory delete smoke test. Commit, push, and open a PR to `develop` closing the storage deletion issue.

## Phase 2: Selection, durable operations, and progress UX

### Task 5: Add durable operation persistence and API

**Files:**
- Modify: `backend/app/database.py`
- Create: `backend/app/operations.py`
- Create: `backend/app/api/operations.py`
- Modify: `backend/app/api/__init__.py`
- Modify: `backend/app/main.py`
- Create: `backend/tests/test_operations.py`

**Model skeleton:**

```python
class OperationStatus(enum.Enum):
    QUEUED = 'queued'
    RUNNING = 'running'
    COMPLETED = 'completed'
    FAILED = 'failed'
    CANCELLED = 'cancelled'
```

1. Test valid state transitions, bounded per-item errors, cancellation, and reconnect lookup.
2. Persist operation counters and redacted current-stage data.
3. Provide create/get/list/cancel endpoints with authorization.
4. Publish operation updates through the existing WebSocket transport.
5. Use bounded polling as a frontend fallback.

### Task 6: Move deletion to operation jobs

**Files:**
- Modify: `backend/app/api/storage.py`
- Modify: `backend/app/storage_operations.py`
- Modify: `backend/tests/test_api_storage.py`
- Modify: `backend/tests/test_operations.py`

1. Add RED tests for operation ID response, mixed bulk result, item progress, and cancellation.
2. Execute each selected item through the shared deletion service.
3. Report authoritative item totals and completed/succeeded/failed counts.
4. Use indeterminate state for recursive work without a reliable denominator.
5. Preserve retryable failed-item details.

### Task 7: Improve StorageBrowser UX

**Files:**
- Modify: `frontend/src/components/StorageBrowser.tsx`
- Modify: `frontend/src/components/StorageBrowser.test.tsx`
- Modify: `frontend/src/api/index.ts`
- Modify: `frontend/src/store/websocket.ts`

1. Write failing tests for visible Select All, partial selection, Clear Selection, and selected count.
2. Add an in-panel operation progress section with determinate and indeterminate variants.
3. Keep target, path, mode, and scroll context after completion or failure.
4. Refresh only affected queries and show a final per-item summary.
5. Do not call `onClose` from operation success/error paths.
6. Add retry for failed items only.

### Task 8: Phase 2 full gate

Run complete backend/frontend checks, Docker build, browser component tests, and a reconnect smoke test. Commit, push, and open a PR to `develop` closing the storage UX/progress issue.

## Phase 3: Encrypted recovery export and first-run import

### Task 9: Define versioned recovery bundle codec

**Files:**
- Create: `backend/app/recovery_bundle.py`
- Create: `backend/tests/test_recovery_bundle.py`
- Modify: `backend/requirements.txt`

**Header skeleton:**

```python
class RecoveryHeader(BaseModel):
    format: Literal['dockervault-recovery']
    envelope_version: Literal[1]
    kdf: Argon2Parameters
    aead: AesGcmParameters


class RecoveryEnvelope(RecoveryHeader):
    ciphertext_b64: str
```

1. Add RED round-trip, wrong-password, tamper, truncation, and unsupported-version tests.
2. Validate the versioned header, bounded Argon2id parameters, and Base64 lengths before deriving a direct 256-bit key from the password and fresh salt.
3. Encrypt and authenticate the complete payload once with AES-256-GCM, a fresh 12-byte payload nonce, and the canonical header as additional authenticated data.
4. Store ciphertext and the 16-byte authentication tag in separate explicit fields; reject unknown or duplicate fields and never reuse a salt/nonce pair.
5. Keep only format, envelope version, cryptographic parameters, salt, payload nonce, payload tag, and ciphertext outside the encrypted payload.
6. Prove known plaintext secrets never appear in output bytes or exceptions.

### Task 10: Serialize and validate portable state

**Files:**
- Create: `backend/app/recovery_service.py`
- Modify: `backend/app/database.py` only if explicit recovery metadata is required
- Create: `backend/tests/test_recovery_service.py`

1. Test every included table and credential-bearing field.
2. Exclude archives, transient operations, sessions, logs, environment dumps, and private SSH keys.
3. Add relational and compatibility validation before import.
4. Mark SSH-key-backed targets as `credentials_required` when key material is unavailable.
5. Produce a redacted import summary.

### Task 11: Add authenticated export and setup import APIs

**Files:**
- Create: `backend/app/api/recovery.py`
- Modify: `backend/app/api/__init__.py`
- Modify: `backend/app/main.py`
- Modify: `backend/app/auth.py`
- Create: `backend/tests/test_api_recovery.py`

1. Require authenticated admin for export.
2. Permit import only while setup is required.
3. Stream export with no-store headers and deterministic temporary cleanup.
4. Validate into an isolated temporary database.
5. Atomically activate imported state or roll back fully.
6. Reconcile catalog entries against reachable archives without deleting anything.

### Task 12: Add Settings export and SetupWizard restore flows

**Files:**
- Modify: `frontend/src/pages/Settings.tsx`
- Modify: `frontend/src/pages/SetupWizard.tsx`
- Modify: `frontend/src/api/index.ts`
- Create or modify corresponding Vitest files.

1. Test Create New versus Restore Backup setup choice.
2. Add file/password input, redacted summary, stage progress, and failure rollback UI.
3. Add System Backup export with password confirmation and explicit archive-exclusion warning.
4. Never persist passwords in browser storage or URLs.
5. Show post-import connection/reconciliation results.

### Task 13: Phase 3 security and integration gate

Run all standard checks plus recovery round trip from a populated fixture, clean first-run import smoke test, tamper test, secret scans over bundle bytes/logs/errors/temp paths, and Docker setup smoke test. Commit, push, and open a PR to `develop` closing the recovery issue.

## Phase 4: Komodo stack lifecycle integration

### Task 14: Add capability-negotiated Komodo stack client

**Files:**
- Modify: `backend/app/komodo.py`
- Modify: `backend/app/api/komodo.py`
- Create or modify: `backend/tests/test_komodo.py`
- Create or modify: `backend/tests/test_api_komodo.py`

1. Capture Komodo HTTP fixtures for supported versions and error responses.
2. Test server/stack/container discovery, status, stop, start, restart, and timeout.
3. Resolve actual API request types from current Komodo contracts, never guesses.
4. Expose only capabilities verified for the connected version.
5. Redact API credentials from requests, logs, and errors.

### Task 15: Add fail-closed stack backup orchestration

**Files:**
- Modify: `backend/app/backup_engine.py`
- Modify: `backend/app/database.py`
- Modify: `backend/tests/test_backup_engine.py`

1. Add RED tests for pre-state capture, failed stop, timeout, backup failure, and state restoration.
2. Associate targets with stable Komodo server and stack identities.
3. Stop and wait before archive creation when configured.
4. Never fall back to direct Docker when Komodo is lifecycle authority.
5. Restore only lifecycle state DockerVault changed.
6. Publish lifecycle stages through durable operations.

### Task 16: Add stack discovery and lifecycle UI

**Files:**
- Modify: `frontend/src/pages/Settings.tsx`
- Modify: `frontend/src/components/BackupWizard/StepTargetSelect.tsx`
- Modify: `frontend/src/pages/Targets.tsx`
- Modify: `frontend/src/api/index.ts`
- Add or modify corresponding tests.

1. Test discovery selector, server/stack status, refresh, and unsupported capabilities.
2. Add confirmed manual Stop, Start, and Restart actions.
3. Render operation progress and timeout/failure detail.
4. Disable unsupported actions with an explanation.
5. Preserve free-form legacy targets without silently converting them.

### Task 17: Phase 4 full gate

Run all standard checks plus mocked Komodo HTTP contract tests, lifecycle cancellation/failure tests, Docker build, and frontend interaction tests. Commit, push, and open a PR to `develop` closing the Komodo issue.

## Final acceptance and release readiness

### Task 18: Independent end-to-end gate

A read-only reviewer must verify all four merged PRs from current `origin/develop`:

1. Reproduce local nested directory deletion.
2. Verify Select All, persistent browser state, folder sizes, progress, mixed outcomes, and retry.
3. Export a populated encrypted recovery bundle and restore it in a clean setup container.
4. Prove wrong password and tampering leave active state unchanged.
5. Verify catalog reconciliation and credential-required states.
6. Exercise mocked supported and unsupported Komodo versions and fail-closed lifecycle ordering.
7. Run the complete CI-equivalent suite and Docker build.
8. Confirm every remote CI check is green before declaring release-ready.

No production deployment, release, or destructive live-data test occurs without explicit approval.
