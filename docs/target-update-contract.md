# Target update representation

`PUT /api/v1/targets/{id}` distinguishes omitted fields (keep current state) from explicit values:

- `schedule_id`, `schedule_cron`, `retention_policy_id`, `pre_backup_command`, `post_backup_command`: `null` clears the field. Empty/whitespace hooks normalize to `null` on create/update and GET.
- Clearing only `schedule_id` retains the legacy `schedule_cron` fallback. To select **manual only**, clients explicitly clear both fields. This immediately removes the native scheduler job and survives registry reload.
- Required fields cannot be explicitly `null`. Empty names are rejected. `false`, empty arrays and empty rule objects are real values, not omission. Existing zero-count retention policies remain valid references.
- The common route-backed create/edit model serializes intentional nullable removals. Group remounts never override explicit choices to remove scheduling, retention or destinations. `retention_policy_id: null` means the backend's default-policy fallback, not retention disabled. An empty destination list also writes `delete_local_after_sync: false`.

No existing records are rewritten; references and hook strings are changed only when supplied by the caller.

## Volume rules and claimed archive sources

New editor rules use `volume:<Docker-volume-name>` keys in the existing `per_volume_rules` JSON. This reserved namespace identifies a Docker named volume, never a persisted mount destination. An explicit all-volume choice writes `selected_volumes: []`; an empty or disappeared subset is invalid, not all.

Before lifecycle changes, the backup engine resolves the reserved keys against the already claimed source/topology snapshot and freezes the resolved rules in `ArchiveSources`. Every mount root for a volume receives the same override. Container mount-destination roots and stack `volumes/name` layouts remain unchanged. Worker and no-sudo fallback consume that same resolved snapshot; no second discovery or schema migration occurs.

Existing raw/root keys are retained without volume-name reinterpretation. A `{root: {}}` rule inherits global paths, while explicit empty include/exclude arrays override them. The editor shows legacy mappings and requires explicit rebind/removal. Known, deliberately unselected volumes keep dormant rules; disappeared identities, missing subset entries, namespace/root collisions, overlapping/ambiguous roots or conflicting rules fail closed before stop/archive work. Multiple mounts and stack-shared volumes are supported.
