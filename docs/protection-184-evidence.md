# Protection workspace #184: implementation and verification

Issue: #184 (part of #179); related consumer correction: #183. Base: `develop@773679fca82813cc7d8c9f4ed1442a8a86f17c84`. One branch, `feat/184-protection-workspace`, one shared editor. Backend configuration and completed effects remain the source of truth. No merge, issue close, release or production deployment is part of this implementation.

## Acceptance matrix

| #184 group | Evidence |
|---|---|
| A1: four source types, create/reload/edit/reload, all supported fields | **PASS**: authenticated product browser creates container/stack/volume/path plans, compares every serialized payload field with real GET, reloads and changes each through the same editor, then compares PUT and reloaded GET. Immutable edit source is visible. Model tests cover legacy cron, false/null/empty values and volume-rule inheritance. |
| A2: schedule/retention/both-hook clears, scheduler, new → manual | **PASS**: all four browser plans clear both scheduling fields, explicit retention and private hooks. GET confirms clears and the real scheduler job disappears. New-policy → manual survives group remount. Null retention explicitly means default-policy fallback, not retention disabled. |
| A3: Save vs Run, real result, disabled/unavailable | **PASS**: all four saves create no backup row; separate Run actions reach actual backend completion. Real API returns409 for a disabled plan. Missing-source discovery is explicitly transport-injected; UI blocks Run without hiding retained history. Acceptance is never labelled completion. |
| A4: URL filters/sort, direct/detail/edit/Back/reload, missing/removed resources and retained archives | **PASS**: real list/detail reload and browser Back/Forward preserve the search/type/status/sort return context. Real404 preserves a safe return and retained-history link. Plan-only removal through the detail route retains real readable archive members; subsequent plan GET is unavailable. Notification `?plan=ID`, invalid IDs and unsafe return URLs have route tests. |
| A5: safe drafts, revision, resources, failures and sessions | **PASS**: owner+intent+revision-bound v1 drafts are allowlisted and debounced. Fake-clock tests prove timer invalidation, TTL and owner/intent isolation. Real browser restores a draft only after explicit changed-revision comparison; successful Save/reload cannot revive it. Logout invalidates queued writes. Hook bodies are absent from persistence/URL and shared caches; private load/save/cancel/session boundaries have caller tests. Discard clears unsaved private hooks or privately reloads the stored hooks. Legacy nonempty subsets migrate as subset, never all. |
| A6: advanced scope, dependencies, copies, removal intent and hooks | **PASS**: per-volume overrides use Docker identity different from archive roots; actual members and bytes exclude configured paths. Dependencies traverse real engine control flow with declared lifecycle doubles. Multiple real local-provider copies match archive bytes. Delete-local intent is saved without running; removing the last destination clears it. Owned allowlisted touch hooks execute real processes and leave independently checked markers. Empty subset prevents PUT and focuses the error. |
| A7: actual tar/gzip, partial writes/unknown results, cleanup | **PASS**: the UI selects gzip for four original runs and uncompressed TAR for a fifth run. Independent SQLite/file check requires exactly five COMPLETED rows, gzip magic + explicit `r:gz` for the first four, non-gzip magic + explicit `r:` for the last, and exact members/copy/restore bytes. A disguised-gzip negative control confirms `r:` rejects it. Lost policy POST commits once, requires readback + explicit selection, and its ID survives a real target rejection. Lost/503 target POST never auto-adopts or replays. Committed PUT503 uses GET comparison + real scheduler reload. |
| A8: narrow layouts and keyboard/error/save | **PASS**:102 product-browser assertions include320/390/768/1024/1440, zero scoped axe A/AA violations, actual Tab/Enter activation, field Enter/Escape/shortcut isolation,320px error correction and keyboard Save confirmed by real GET, browser reduced-motion preference, and1280→640 CSS-pixel reflow equivalent to200%. No physical-device/native-AT certification. |

## Runtime boundaries

`backend/tests/runtime_protection_app.py` doubles only Docker discovery/identity/lifecycle using owned container-layer directories. It has no Docker socket, host mount, Komodo connection or external-provider contact. Authentication, HTTP/API validation, SQLite, native scheduler, engine/worker/fallback filtering, actual archive bytes, local-provider copies and restore use production code. Touch is explicitly added to the disposable fixture's hook allowlist, never product defaults. Lost responses,503 and disappearing discovery are explicit transport faults after real commits, not synthetic success rows. Disabled SFTP is created/updated/listed through the real API and independently found in SQLite and both Storage/Protection UI; this is not SFTP reachability evidence.

## Executed local gates

- Backend native full suite: **699 passed**,27 warnings, **74.30%** coverage; original34% floor retained. Own age tools were extracted from the disposable product image. A first final invocation timed out at240s in inherited WebSocket tests; a fresh complete invocation passed in179.99s. No timeout, assertion or product polling setting was relaxed.
- Frontend locked install and Node24.11.0/npm11.6.2 native gates: **366 passed /47 files**; coverage69.66% statements /65.83% branches /60.80% functions /71.59% lines; lint and TypeScript pass; native **production** build passes. Dependencies and lockfiles are unchanged.
- Product Protection browser: **102/102**, then independent SQLite/five-archive/format/copy/hook/restore assertions pass.
- Existing storage-operation suite: **44 controls pass**, including real cancellation/restart/unknown outcomes. Three recovery scenarios pass; they also passed twice on the earlier candidate image. Existing plain + encrypted local and remote-only-after-restart byte-roundtrips pass.
- Production Ruff check/format and new Python tests/fixtures pass. Advisory mypy remains432 errors in20 files. An exploratory all-backend Ruff run finds23 pre-existing findings in untouched test/fixture files; these are outside the declared production Ruff gate.
- The existing214-state/61-real-run browser remains a required release gate. Its final invocation and exact-head GitHub checks are recorded in the PR/task handoff, not inferred from this document. No CI waiver is introduced.

Commands (use the project's installed Python environment, a Playwright runner image and axe script):

```sh
cd backend
PATH="$OWN_AGE_BIN:$PATH" python -m pytest --cov=app --cov-report=xml --cov-report=html
ruff check app/
ruff format --check app/
mypy app/ # existing advisory gate
cd ../frontend
npm ci
npm run test:coverage -- --run
npm run lint
npx tsc --noEmit
npm run build
cd ..
docker build --build-arg BUILD_ENV=development -t "$PRODUCT_IMAGE" .
python backend/tests/run_protection_browser.py "$PRODUCT_IMAGE" "$BROWSER_IMAGE" "$AXE_JS"
python backend/tests/run_truthful_states_browser.py "$PRODUCT_IMAGE" "$BROWSER_IMAGE" "$AXE_JS"
python backend/tests/run_storage_jobs_browser.py "$PRODUCT_IMAGE" "$BROWSER_IMAGE"
python backend/tests/run_storage_jobs_recovery_browser.py "$PRODUCT_IMAGE" "$BROWSER_IMAGE"
python backend/tests/run_backup_roundtrip.py "$PRODUCT_IMAGE"
```

## Provenance and self-review

The tested development image is `sha256:ab7eda562926424074927054abcdad1efe5b6f362fb5bfdc714d0857288c4483`. Read-only binding compares all38 backend Python sources and119 frontend builder source files with this checkout, all4 Docker-builder dist files with the shipped image, and the exported recovery script. Docker build path `/app` and `BUILD_ENV=development` are identical. The separate native production build is **not** claimed byte-identical: development JSX embeds build paths. Raw logs and hash manifests are retained in the task evidence attachment.

Static self-review traced source→model→serializer→API→readback, private hook ownership/cancellation, all policy/target retry branches, draft timer/session guards, legacy scopes and claimed archive roots. RED→GREEN findings corrected confirmed-create source locking, private-hook discard, legacy-subset migration, ambiguous/overlapping roots and strict enum types. Central SFTP correction accepts all seven existing backend types and rejects malformed arrays at all five enum boundaries. Schedule-job validation also accepts the real maintenance jobs, not only backup jobs.

Gate review preserves every existing behavioral assertion: the retired modal-only editor cases are replaced by nonmodal route/caller tests; the old two editors now share one implementation. Existing214-browser selectors follow the route editor and await URL commitment without longer timeouts. Recovery holds every pre-acceptance empty snapshot (`requests.length === 0`) as required by the owner, rather than only the first GET. Static validation copy replaces raw server error echo; privacy remains enforced. No checks are skipped, disabled or weakened.

Ruling: preserve incumbent #182 design/primitives and existing API/query/private-editor infrastructure. No new product library, alias/discovery engine, codec, policy-compensation/exactly-once API or Komodo lifecycle implementation. Local/Chromium synthetic evidence is not production/provider/native-AT evidence; physical devices/native AT remain #191. Inherited npm audit reports28 advisories; Browserslist and large-bundle warnings remain disclosed. Independent same-card review and owner integration remain required.
