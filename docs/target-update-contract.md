# Target update representation

`PUT /api/v1/targets/{id}` distinguishes omitted fields (keep current state) from explicit values:

- `schedule_id`, `schedule_cron`, `retention_policy_id`, `pre_backup_command`, `post_backup_command`: `null` clears the field. Empty/whitespace hooks normalize to `null` on create/update and GET.
- Clearing only `schedule_id` retains the legacy `schedule_cron` fallback. To select **manual only**, clients explicitly clear both fields. This immediately removes the native scheduler job and survives registry reload.
- Required fields cannot be explicitly `null`. Empty names are rejected. `false`, empty arrays and empty rule objects are real values, not omission. Existing zero-count retention policies remain valid references.
- Both existing edit dialogs serialize intentional nullable removals. Wizard smart defaults never override an explicit user choice to remove scheduling/retention.

No existing records are rewritten; references and hook strings are changed only when supplied by the caller.
