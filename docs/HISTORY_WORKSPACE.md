# History & Restore

`/history` lists runs; `/history?view=archives` lists recorded artifacts. Detail
addresses are `/history/runs/:id` and `/history/archives/:id`. Search, filters,
sort, page size, offset and the insert boundary live in the query string. Detail
and Storage return links keep that query context. Legacy `?run=`/`?archive=` links
redirect to details. Protection links use `target_id`, including removed plans.
The old grouped `Backups` component remains for compatibility; it is not the new
History route and is not evidence of an additional supported explorer.

## Server query

Authenticated `GET /api/v1/backups/history` returns
`{items,total,limit,offset,snapshot,dataset}`. `GET /backups/history/:id` returns one
history record or 404. Existing `GET /backups` remains an array; its Dashboard,
Protection and other consumers keep their existing contract.

Parameters:

- `view`: `runs` (all catalog run records) or `archives` (recorded file path,
  recorded size, or a completed remote copy with a recorded path).
- `q`: literal, case-insensitive search in plan name, archive path, run error or
  run ID. `%` and `_` are literal, not wildcards. Maximum 200 characters.
- `status`: native run status; `source`: container/stack/volume/path;
  `target_id`: positive plan ID, including removed plans.
- `after` / `before`: inclusive creation-time bounds. Offsetless values mean UTC;
  explicit offsets are converted to UTC. The UI labels its date controls UTC.
- `location`: `local` means a local path is recorded, `remote` means any remote
  sync record, or a positive storage ID means a record for that destination.
  A failed sync attempt counts as recorded placement, not a usable remote copy.
- `sort`: created/name/size; `direction`: asc/desc. The stable ID is the final
  tie-breaker. `limit`: 1–1000, default 50; `offset`: nonnegative.
- `snapshot`: nonnegative maximum ID. The first query supplies it. Paging reuses
  it so new insertions do not shift pages. Existing records, copy changes and
  removals remain live. **Include new runs / restart pagination**, filter changes,
  reset or a view change starts a new boundary; this is not an immutable history
  export or a database snapshot held across HTTP requests.

Count and page selection share one SQLite read transaction and the same SQL
predicates. They do not scan filesystem/provider availability. Returned records
have bounded, off-loop local observations and catalog remote status. An API error
has no fabricated zero total. A failed refetch keeps explicitly dated stale data.
Location inventory failure does not hide independently confirmed history rows.

## Truth and supported actions

A run outcome is not an archive. Failed/pending/cancelled runs without artifact
evidence say **Archive never created**; a failed sync row alone is not artifact
evidence. Missing identity on other states stays unknown. Native browse/restore
requires a completed run and a potentially usable local or enabled remote copy;
access may still fail and is retryable. No restore-test timestamp is invented.

Local copies distinguish available, missing at a recorded path, inaccessible,
no recorded local path, and confirmed deletion. Canonical preview/token removal
records `local_copy_removed` in existing backup metadata after successful local
removal; no historical deletion is inferred from absence alone. Older removed or
imported copies without provenance remain missing/not recorded, not a fabricated
confirmed deletion. Remote copies display destination names, sync outcomes and
unknown live availability; disabled destinations are inaccessible. A catalog sync
success is explicitly not a current provider probe.

The detail page reuses the existing log/timeline, archive browser, RestoreWizard
and RemovalDialog. Restore retains the existing supported backend effects; the
full restore workspace remains #186. Copy removal retains #180 preview, frozen
scope, confirmation token, actual per-copy results and failed-scope retry. Sync
uses the canonical retry service for both failed copies and new destinations.

Archive paths are searched globally with bounded pages through the existing
#175 scanner/cache, not a second index. Tree expansion and downloads are named
keyboard actions. `GET /backups/:id/download` adapts the canonical local/remote
preparation to a resource-owned FileResponse; `?sidecar=true` exports the encrypted
DEK sidecar. Ciphertext stays encrypted. Export both matching filenames and keep
the separately held private key. Downloads retain errors until retry/success;
private keys remain component-owned, abortable and outside URLs/shared caches.
Storage links use a relative location/path and the canonical StorageBrowser, not
a second file explorer or host mount path.

## Reproducible verification

- `cd backend && pytest` retains the configured 34% coverage floor.
- `cd frontend && npm ci && npm run test:coverage -- --run && npm run lint && npx tsc --noEmit && npm run build`
- `python backend/tests/run_history_browser.py PRODUCT_IMAGE BROWSER_IMAGE AXE_JS OUTPUT_DIRECTORY`
  runs real cookie auth, native SQLite seeds (>500, not >500 engine runs), real
  TAR/age/local-provider bytes, route/reload/Back, failed downloads and five CSS
  widths. HTTP faults are explicitly injected. The runner records and removes
  only its own containers/anonymous volumes.
- Preserve Protection, reconciliation, TruthfulStates (61 actual engine runs,
  paced under the unchanged rate limit), Storage, recovery, policy and
  plain/encrypted restart/restore gates on the same candidate.

Chromium/Linux CSS widths and automated axe are not physical-mobile or native
assistive-technology certification. Those human checks remain #191. LocalStorage
provider tests are not external-provider or production-Docker lifecycle claims.
