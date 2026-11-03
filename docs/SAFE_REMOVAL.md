# Safe plan and archive-copy removal

## Removing automation is not deleting recovery points

`DELETE /api/v1/targets/{id}` removes the active protection plan. It retains the
historical target row (including its name and original restore destination),
archives, encryption-key sidecars, sync catalog and logs. It disables future
scheduled/manual runs and automatic retention for that plan. Queued, not-yet-started
runs become failed with an explicit reason. Running work is allowed to settle under
the existing target claim before acknowledgement. Repeating removal returns
`already_removed`; no file cleanup is performed.

Active target lists and editors exclude removed plans. History can explicitly use
`GET /api/v1/targets?include_removed=true`; `removed_at` distinguishes historical
sources. Backups renders those sources as retained history, without editing,
re-enabling or starting automation. Browse and supported restore still work.

Migration 6 adds only the nullable `removed_at` column. Existing catalog migrations
and foreign-key/recovery rules remain unchanged.

## Permanent copy deletion

1. `POST /api/v1/backups/removal/preview` with `scope: "local" | "remote" | "all"`.
   Select either `backup_ids: [...]` or optional `target_id`/`status` filters. An
   unfiltered request explicitly reviews the whole catalog, not a list-page sample.
2. Read `archive_count`, `copy_count`, every selected identity and retained scope.
   The server freezes exact archive/copy identities and returns an opaque `token`.
3. After confirmation, `POST /api/v1/backups/removal/execute` with that token only.
   Execution never reevaluates a filter. New archives cannot join the selection.

Previews expire after 15 minutes or application restart. They are short-lived,
process-local confirmations, not durable jobs. An expired, evicted or unknown token
returns 409 and requires a fresh review. At most 100 confirmations are retained;
settled results may be evicted at capacity. Normal repeated execution returns the
cached result; eviction never causes reexecution. The application uses its existing
single-process runtime and target/remote-object claims.

Execution revalidates archive identity, file identity, sync row, storage destination
and unresolved operation state. A changed/missing/shared identity is skipped and
requires review again, never silently replaced or widened. Active/recovery-required
work fails safely. Files and sidecars that could not be removed keep their catalog
and retry information. Local encryption keys stay while remote recovery copies
remain. In an all-copy request, deferred key cleanup remains in the failed scope
until confirmed remote deletion succeeds. A provider's failure is not a deletion.

Responses report `completed` or `incomplete`, per-copy `succeeded`, `failed`,
`skipped` and unselected `retained` items. Retained means intentionally not selected
for mutation, not a new remote availability probe. HTTP 200 plus `warning` is an
incomplete result, never total success. `retry_token` includes only failed confirmed
copy/substep scope; successful removals are not replayed. There is no Undo.

The old filter-based `DELETE /backups` now returns 409, directing callers to the
confirmation contract. Single-ID DELETE remains a compatibility entry point through
the same validated copy-removal service and structured results. New UI callers use
preview/execute in both Backups and the legacy Targets component.

## Executable verification

- `cd backend && python -m pytest` includes real SQLite/disk/local-provider copy
  removal, frozen >500-archive filtering, replacement/not-found, partial sidecar
  failure, cancellation/claims, recovery guards and migration tests.
- `cd frontend && npm run lint && npx tsc --noEmit && npm run test:coverage -- --run && npm run build`
- Build the actual runtime: `docker build -t dockervault:removal-candidate .`
- Compile the intentionally unrouted legacy component's test-only mount:
  `cd frontend && node --input-type=module -e 'import {build} from "vite"; await build({build:{outDir:"/absolute/owned/legacy-assets",emptyOutDir:true,rollupOptions:{input:"tests/removal.html"}}})'`
- `cd backend && python tests/run_removal_browser.py dockervault:removal-candidate PLAYWRIGHT_RUNNER_IMAGE /absolute/owned/legacy-assets`
  The existing runner image must provide Playwright in `/runner`. This tests actual
  rendered Backups and Targets with authenticated API, isolated SQLite, real age
  encryption, local remote-provider files, an application-process restart, browse,
  byte-checked restores, keyboard/cancel/double-click and partial-failure retries.
  Owned containers use no host ports, production volumes or Docker socket and are
  removed afterwards. The fixture-only key/control endpoints are never mounted by
  the product entry point. Do not substitute the repository's production Compose
  configuration or an unrelated published image for this candidate.
