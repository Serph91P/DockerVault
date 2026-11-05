# Durable storage deletion operations (#142)

## Contract

The existing storage browser now starts a durable `storage_delete` operation. It
keeps the panel, folder, selection mode and listing scroll position open. Select
All and Clear Selection apply to the current listing, including directories and
leaf links; the native checkbox exposes partial selection and a selected count.
Navigation clears selection. Explicit close remains available during work.

SQLite is authoritative. HTTP 202 means accepted after commit, not completed.
Partial failures have state `failed`, never a success notification. Outer entry
counts are measurable; recursive inner deletion is indeterminate. A disconnected
browser does not cancel a job, and cancellation does not undo completed effects.

## API

All routes are authenticated and scoped to the creating user:

- `POST /api/v1/operations`: `{action:"storage_delete", storage_id:"local"|number,
  path:string, paths:string[], request_id:UUID}` -> 202 `{operation_id:string}`.
  Paths are relative to the storage root. The current folder is context, not an
  absolute provider path. Dedupe/normalize valid targets; retain invalid target
  errors individually. A selection contains 1-1000 nonempty deduplicated targets,
  with a bounded raw request of at most 10000 entries and 1024 characters per path.
- A request UUID is unique per user. Repeating the same normalized request returns
  the same ID without another effect; a different payload with that UUID is 409.
- `GET /api/v1/operations?storage_id=local&limit=20`: active jobs first, then recent
  results (maximum 100). `GET /api/v1/operations/{id}` reads one durable detail.
  Detail includes original folder/storage, counters, timestamps, parent ID and
  `{path,state,error}` items; it excludes storage configuration and credentials.
- `POST /api/v1/operations/{id}/cancel`: persist a cancellation request, settle the
  current physical item, skip future items, then publish the actual counters.
  Repeated cancellation cannot change an already terminal outcome.
- `POST /api/v1/operations/{id}/retry`: `{request_id:UUID}` creates a child from
  recorded **failed** items only. Successful, skipped and unknown items are not
  silently retried. Original outcomes remain immutable. The UI directs further
  retries to an existing child and reuses a request UUID across ambiguous replies.

States: `queued`, `running`, `completed`, `failed`, `cancelled`. Item states:
`pending`, `running`, `succeeded`, `failed`, `skipped`, `unknown`. `completed` counts
all terminal items and equals succeeded + failed + skipped + unknown. Only all
verified successful entries produce operation state `completed`.

## Runtime and recovery

The existing single-process application owns one serial worker, with admission
bounded to 32 active operations. That also serializes overlapping paths/roots.
Shutdown stops future entries and joins current physical I/O and database work
before releasing ownership. Storage configuration/creation identity is checked
against the accepted snapshot. Local roots additionally keep an inode descriptor
pinned until settlement, and canonical fd-safe deletion checks root identity in
the same descriptor walk; replacement or inode recycling cannot redirect a job.

Startup marks interrupted active jobs failed: an in-flight item becomes unknown,
and an unstarted item becomes skipped. It **never** automatically replays a
storage deletion. Inspect unknown effects on the actual storage before taking a
new explicit action. A worker failure closes admission and preserves uncertainty.

Migration 7 adds `StorageOperation` and `StorageOperationItem` through the existing
transactional ledger, including fresh, previous-schema and repeat initialization.
`WorkloadOperation`, `OperationJournal` and workload restart recovery are unchanged.
The old synchronous file and bulk-delete endpoints remain compatible consumers
of the same canonical deletion/provider behavior.

Authorized WebSocket clients receive only `operation_update {operation_id}`
invalidation. The UI refetches authenticated snapshots, debounces realtime hints,
and polls every 2 seconds while an active operation is visible. Fetch retries are
bounded (two retries with backoff); failure shows a stale/error warning instead
of claiming a current success. Terminal work stops polling. Unmount, storage
change and session-generation change invalidate UI ownership and late replies.
Reopening a storage discovers operations from the server, not browser persistence.

## Verification and replay

Baseline: `7c5c0c6906283892cf835e5faf120edcced076ee` on `develop`.
No merge, release, deployment or issue closure is part of this implementation.

Linux checks run with Python 3.14.4, Node 24.11.0 and npm 11.6.2:

```sh
# From backend, using the repository virtual environment:
python -m pytest
python -m ruff check app tests/test_storage_jobs.py tests/run_storage_jobs_browser.py
python -m ruff format --check app tests/test_storage_jobs.py tests/run_storage_jobs_browser.py
uv pip check --python ../.venv/bin/python
PYTHONPATH=. python tests/runtime_smoke.py

# From frontend, with Node/npm matching package.json:
npm ci
npm run test:coverage -- --run
npm run lint
npx tsc --noEmit
npm run build

# From repository root (isolated --rm containers; no production socket/mounts):
docker build -t dockervault:storage-operations-test .
.venv/bin/python backend/tests/run_storage_jobs_browser.py dockervault:storage-operations-test dockervault:t539-browser-runner
.venv/bin/python backend/tests/run_backup_roundtrip.py dockervault:storage-operations-test
```

Results on the candidate source:

| Check | Result |
| --- | --- |
| Full backend suite, configured coverage gate >=34% | 666 passed; 74.12% coverage |
| New storage-job SQLite/API/filesystem tests | 27 passed in the full suite |
| Full frontend coverage suite | 186 passed in 27 files |
| Ruff app and new Python tests, format | pass |
| Frontend lint, TypeScript, production build | pass |
| Runtime import, repeated SQLite init, dependency consistency | pass |
| Production Docker build and nginx/backend health | pass |
| Product Chromium/cookie/SQLite/disk acceptance | 44 checks pass |
| Plain and encrypted HTTP backup, local restore, remote-only restart restore | both byte-identical |

The versioned product runner uses real application routes and physical deletion,
including a genuine permission-denied directory and a sentinel outside deletion
scope. It tests current/configured roots, native partial selection and keyboard
controls, 320/390px modal fit, retained path/mode/scroll, in-panel failures and
live-error focus, failed-only child retry, active reload, WebSocket loss with
polling fallback, settled cancellation and a real backend KILL in the
physical-effect-before-DB-outcome window. External test-only holds make that
window deterministic; they do not replace ordinary real product deletions and
are never included in product source or the production image. Final SQLite
counters and actual disk bytes are inspected before owned containers are removed.

TDD/self-review also reproduced and fixed a split counter/item snapshot race,
root inode lifetime gap, worker initial-read failure, ambiguous/fast retry request
identity reuse and missing native indeterminate progress. Migration assertions
advance the supported ledger from 6 to 7 (the unknown-version guard advances to
8); no gate is lowered and no old test is removed. The existing folder-size test
uses the new async acceptance/snapshot contract rather than the synchronous UI
route it replaced. No new runtime dependency or unrelated framework is added.

Evidence boundaries: browser-engine keyboard/ARIA/focus checks are not human
screenreader listening, physical-mobile testing, or full accessibility
certification. The new WebDAV case uses a real isolated HTTP server with synthetic
objects, not an external provider account. Existing provider/cancellation suites
remain in the full backend run. Existing advisory mypy reports 425 errors in 17
files; it is not presented as a passing mandatory gate. npm reports 28 dependency
advisories (1 low, 13 moderate, 14 high); the lockfile is unchanged. Existing
Browserslist-age and large-bundle build warnings remain visible. Exact-head CI and
independent same-card review are recorded on the PR/implementation handoff.
