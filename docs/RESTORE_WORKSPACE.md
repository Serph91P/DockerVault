# Restore workspace contract (#186)

`/history/archives/:id/restore` preserves the archive's complete search/return context. The workspace uses the existing synchronous restore engine, not a restore-job system. Only the final **Confirm restore** action writes; Enter does not activate it (keyboard users use Space). Review/back/close do not restore. Leaving a pending view is **not cancellation**. No final response means an unknown outcome, including after reload; inspect destination files and workload/run logs before another explicit attempt. A confirmed result stays visible with the executed destination and effects until the view is closed. There is no durable result lookup, resume or cancel API.

## Request and capability compatibility

- `destination_type: original | path | volume`, with only `target_path` for path or only `target_volume` for volume. Contradictory/missing combinations fail validation before effects.
- Legacy `target_path` without `destination_type` still means custom path; legacy empty body means original. These authenticated API callers retain compatibility without a mandatory review field.
- Typed execution requires `review_fingerprint` from read-only `POST /backups/:id/restore-review`. This endpoint does not download, decrypt, stop workloads or write files.
- Original path/volume, custom allowed path, and explicit Docker volume destinations use the canonical engine layout. Original container/stack mapping is unsupported and disabled with explanation. Custom container/stack archives preserve their archive roots.
- The server resolves a volume name via native Docker discovery and requires its physical mount directory to be accessible; a missing mountpoint must never become a newly created local substitute directory. The browser never constructs Docker mount paths. Discovery failure is unavailable, not an empty inventory. Custom realpaths retain the existing allowlist (backup root or Docker volumes). Original paths are the configured source paths.

## Narrow impact binding, not an authorization token or dry run

A SHA-256 fingerprint of canonical JSON binds archive catalog identity/checksum/layout/key requirement, relevant plan destination/lifecycle configuration, resolved destination and nearest existing ancestor device/inode, volume name/driver/creation identity, and configured container immutable IDs/states. The public review exposes only the fingerprint, destination and user-facing effects. It contains no private key. Authentication remains the existing cookie/session boundary; the digest is an optimistic impact comparison, not a secret capability or an idempotency token.

Execution takes the existing target lock, recomputes the review before materialization, then validates again under the existing workload claims before any stop. The guard compares the actual engine target, acquired container identities and captured states too. Extraction uses the canonical reviewed path, not the lexical client alias; its path identity is checked again immediately before extraction. Changed impact fails closed and requires review again. This does not freeze external privileged filesystem/Docker changes transactionally, authenticate remote bytes during review or guarantee provider availability. The canonical archive/decryption/tar validation still runs at execution. A missing or disabled remote copy cannot be resurrected by a cached provider, including sidecar access.

Only configured and originally-running workloads are stopped/restarted; unrelated volume users are not automatically stopped. Restart order is the existing reverse order. Decryption authenticates before workload stop. Merge extraction overwrites collisions, retains unrelated files, and provides neither transactional rollback nor application-health verification. A backend/transport error after admission can follow partial effects and must not be shown as success.

Private keys exist only in the active component and necessary authenticated request. They never enter query/mutation caches, routes, history state, notifications, drafts, storage or logs; errors, close, session revocation and final settlement clear them. Old callbacks cannot update another session or a closed view.

## Reproduction

Full native product acceptance (existing images must be provisioned):

```
python backend/tests/run_restore_browser.py PRODUCT_IMAGE BROWSER_IMAGE AXE_JS EVIDENCE_DIR
```

The runner creates a uniquely owned offline DinD daemon and shares **only its owned** Docker data/socket volumes with the synthetic product fixture. No production/host socket, host bind, network, credentials or data are attached. Only that disposable daemon is privileged. Workloads handle TERM normally so native stop/restart is observable without changing the product's stop timeout. Native bytes, real Docker lifecycle, age keys/sidecars, remote-only Local-provider materialization, pending/unknown response loss, five CSS widths, keyboard/focus/session boundaries and axe are exercised. Transport faults are explicitly named and never counted as a provider or cancellation proof. CSS/axe are not physical-device/native-AT evidence; those remain #191. Original provider/Komodo/system-recovery expansion is not part of this change.
