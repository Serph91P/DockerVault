# Protection workspace #184: implementation and verification

Issue: #184 (part of #179); related consumer correction: #183. Base: `develop@773679fca82813cc7d8c9f4ed1442a8a86f17c84`. One branch, `feat/184-protection-workspace`, one shared editor. Backend configuration and completed effects remain the source of truth. No merge, issue close, release or production deployment is part of this implementation.

The sole correction round additionally fixes typed/intent-bound candidate reconciliation (B1), whitespace-only private-hook clearing (S1), consumption of the native global422 envelope, and the owner-confirmed SQLite cursor-cancellation merge blocker. These are verified below; earlier successful normal paths did not disprove those defects.

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

- Backend native full suite: **731 passed**,27 warnings, **74.37%** coverage; original34% floor retained,213.86s. Own age tools were extracted from the disposable product image. One preceding731-case invocation failed the inherited S3 provider cancellation fixture's5s entry wait during concurrent verification (730 passed); unchanged focused provider/API cases and the fresh entire suite subsequently pass. This is disclosed, not a claim of universal flake freedom. No timeout, assertion or product polling setting was relaxed.
- Frontend locked install and Node24.11.0/npm11.6.2 native gates: **403 passed /49 files**; coverage70.52% statements /66.75% branches /61.63% functions /72.57% lines; lint and TypeScript pass; native **production** build passes. Dependencies and lockfiles are unchanged.
- Product Protection browser: **five consecutive102/102 runs**, each with independent SQLite/five-archive/format/copy/hook/restore assertions. The additional reconciliation runner passes36 checks plus SQLite, including real lost committed POSTs, colliding IDs, Discard/late readback, both private hook cases and a native non-intercepted422.
- Existing storage-operation suite: **44 controls pass**, including real cancellation/restart/unknown outcomes. Three recovery scenarios pass; they also passed twice on the earlier candidate image. Existing plain + encrypted local and remote-only-after-restart byte-roundtrips pass.
- Production Ruff check/format and new Python tests/fixtures pass. Advisory native `mypy app/` remains432 errors in20 files. An exploratory all-backend Ruff run finds23 pre-existing findings in untouched test/fixture files; these are outside the declared production Ruff gate.
- Existing214-state browser and61 real COMPLETED runs pass, including SQLite, physical deletion, real SSH key files and preserved source bytes. The final run took861s with native rate-limit pacing. An earlier pre-backend invocation was terminated at240s and rerun without changing its pacing/assertions. Exact-head GitHub checks are recorded in the PR/task handoff; no CI waiver is introduced.

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
python backend/tests/run_protection_reconciliation_browser.py "$PRODUCT_IMAGE" "$BROWSER_IMAGE"
python backend/tests/run_truthful_states_browser.py "$PRODUCT_IMAGE" "$BROWSER_IMAGE" "$AXE_JS"
python backend/tests/run_storage_jobs_browser.py "$PRODUCT_IMAGE" "$BROWSER_IMAGE"
python backend/tests/run_storage_jobs_recovery_browser.py "$PRODUCT_IMAGE" "$BROWSER_IMAGE"
python backend/tests/run_backup_roundtrip.py "$PRODUCT_IMAGE"
```

## Provenance and self-review

The correction's tested immutable development image is `sha256:97b043d4a44c11833015156600919fc189ed07fdfdbfce6c7b47f7534522b8bd`. It derives from the originalab7 runtime with a fresh native Docker frontend-builder dist and the corrected backend sources, not a claimed fresh complete Dockerfile build. Read-only binding compares all38 backend Python sources and121 frontend builder source files with this checkout, all4 Docker-builder dist files with the shipped image, and the exported recovery script. Docker build path `/app` and `BUILD_ENV=development` are identical. The separate native production build is **not** claimed byte-identical: development JSX embeds build paths. Raw logs, Docker derivation inputs and hash manifests are retained in the task evidence attachment.

## Sole correction round

- **B1:** candidate rows, completion/error and selection are bound to resource kind and current generation. Model/private-hook changes, checkpoint/new POST, resolution, Discard/Restore/Cancel/session/unmount invalidate them. Old IDs cannot be reinterpreted for the next table, even after a delayed read.12 component cases include sequential Schedule→Retention→Target and Schedule→Target collisions; causal old-editor replay has9 failures/3 controls, current403-case suite passes. Real36-check API/SQLite replay confirms correct policy IDs, six intended/orphan-disclosed target rows, exactly two schedules/two retentions, zero backup runs, retained draft before correct selection and no duplicate POSTs.
- **S1:** only whitespace-only hook bodies become null before the private write/readback comparison; other bodies remain byte-for-byte unchanged, private and absent from browser persistence. Real existing-plan edits confirm both cases.
- **Native422:** only actual-string, exact allowlisted `body.<field>` names from global `errors` are mapped to static messages. Raw message/msg/input/ctx never enter Error/cache/UI/callback state. Compatible detail-array validation remains supported with strict types.24 adapter cases and both caller envelopes cover malformed/nested/prototype/private-marker inputs; the real backend422 maps Keep Last to associated `aria-invalid` feedback without changing the unrelated #187 defaults/description contracts.
- **SQLite cancellation:** a real unfinished SELECT retained a read lock after cancellation, despite a closed session and zero checked-out pool connections. Controlled asyncio/repeated/AnyIO cancellation at execute/fetchall reproduces it for execute/get/refresh. A native SQLite/aiosqlite-only `Engine.handle_error` hook closes the existing cursor under cancellation shielding before normal connection invalidation; it does not replay statements, alter the exception/disconnect, commit, tune timeouts/pools/WAL or upgrade dependencies.32 new cases check actual locks/commits/reads, native driver stop completion without GC, normal errors, missing context/cursor and static-only cleanup-failure logging. Removing only the hook in a process causes six lock failures with six normal controls; the candidate passes32/32. Owner independently reproduced the mechanism and candidate fix, and tested that SQLAlchemy2.1.8 alone remains affected.
- **Honest native logging:** six forced AnyIO cases still emit SQLAlchemy's `Exception terminating connection / CancelledError` while invalidation proceeds. Tests independently await native worker-stop completion and prove subsequent EXCLUSIVE/commit/read immediately succeeds while task/driver references remain retained. These native log events are not suppressed or called clean logs. A failed cursor-close logs only a static warning and preserves cancellation/invalidation, never success.
- **Gate integrity:** original102 runner's Unconfirmed heading is a pre-POST checkpoint, not the completed lost-response outcome. Both policy/target checks now await the existing final save failure before the identical durable-effect assertions; no assertion/15s timeout was loosened. The reconciliation browser holds a real POST before commit to prove this ordering deterministically. Earlier old-backend Auth500/Target500 failures, failed synchronization attempts and native fixture timeouts remain in the evidence; five new complete102 runs and the214 run pass on the corrected image.

Static self-review traced source→model→serializer→API→readback, private hook ownership/cancellation, all policy/target retry branches, draft timer/session guards, legacy scopes and claimed archive roots. RED→GREEN findings corrected confirmed-create source locking, private-hook discard, legacy-subset migration, ambiguous/overlapping roots and strict enum types. Central SFTP correction accepts all seven existing backend types and rejects malformed arrays at all five enum boundaries. Schedule-job validation also accepts the real maintenance jobs, not only backup jobs.

Gate review preserves every existing behavioral assertion: the retired modal-only editor cases are replaced by nonmodal route/caller tests; the old two editors now share one implementation. Existing214-browser selectors follow the route editor and await URL commitment without longer timeouts. Recovery holds every pre-acceptance empty snapshot (`requests.length === 0`) as required by the owner, rather than only the first GET. Static validation copy replaces raw server error echo; privacy remains enforced. No checks are skipped, disabled or weakened.

Ruling: preserve incumbent #182 design/primitives and existing API/query/private-editor infrastructure. No new product library, alias/discovery engine, codec, policy-compensation/exactly-once API or Komodo lifecycle implementation. Local/Chromium synthetic evidence is not production/provider/native-AT evidence; physical devices/native AT remain #191. Inherited npm audit reports28 advisories; Browserslist and large-bundle warnings remain disclosed. Independent same-card review and owner integration remain required.
