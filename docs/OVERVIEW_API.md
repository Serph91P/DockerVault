# Overview summary

Authenticated read-only `GET /api/v1/overview/summary?limit=5` (integer 1–20).
The limit bounds each example list, never a total. No provider scan, file-stat loop,
job submission, or user-selection parameter is performed. Storage operations use
exactly the authenticated owner boundary of `/operations`; backup/plan/catalog
information retains the existing installation-wide authenticated scope.

`observed_at` is UTC; `window.after = observed_at - 24h`, `window.before =
observed_at`, `time_field = created_at`. Pass both values unchanged to
`/backups/history?view=runs&status=failed&after=…&before=…`. History's inclusive
boundaries and descending created_at/id order apply. This counts **runs created
in the window**, not the time at which they failed.

`dataset = retained_catalog_and_current_process`. Database counts and bounded
examples share an explicit short SQLite read transaction. Errors propagate;
a failed query never returns a successful zero-filled snapshot. Live worker
observation is separate point-in-time evidence from the current engine process,
not a cross-process/exactly-once assertion. The frontend preserves independent
queries and last valid data with explicit stale/error feedback.

| Field | Exact scope / denominator |
|---|---|
| plans.total | All target rows with removed_at IS NULL |
| plans.enabled / disabled | Partition of those nonremoved plans by enabled flag |
| plans.manual | Nonremoved plans with neither shared schedule reference nor nonempty legacy cron |
| plans.automatic_enabled | Nonremoved enabled plans with enabled shared schedule, or legacy cron when no shared reference; shared schedule takes precedence as in scheduler |
| plans.automatic_paused | Remaining automatic configurations, including disabled/missing schedule or disabled plan; no claim of actual job registration |
| plans.without_recorded_run | Nonremoved plans with no retained Backup row; `enabled` is its enabled subset |
| plans.without_recorded_completed_run | Nonremoved plans with no retained COMPLETED Backup row; `enabled` is its enabled subset |
| backup_activity.recorded_pending / recorded_running | All retained rows with that catalog status, including removed-plan history |
| backup_activity.live | Distinct IDs observed inside current run_backup calls, across queue/batch/direct callers, including upload after early COMPLETED |
| backup_activity.total | Distinct union of pending/running records and observed live IDs (active records cannot be removed under existing claims) |
| storage_activity.total | All queued/running StorageOperation rows owned by authenticated user, regardless of location availability |
| recorded_copy_failures.total | All retained BackupStorageSync rows whose sync_status is failed, including disabled/missing storage references |
| recorded_copy_failures.backups | Distinct backup IDs represented by those failed sync rows |

Plan-gap examples sort by plan ID ascending. Backup activity sorts created_at,
then backup ID descending. Storage activity sorts created_at, then operation UUID
descending. Copy failures sort sync-row ID descending. Missing plan/name/location
references remain nullable, never replaced by another record. A failed sync row
is a historical result, not a current incident or evidence that a remote object
is absent. Completed status, archive identity and physical availability differ.
Retention can erase earlier evidence: neither gap counter means lifetime
"never run" or guarantees restorability.

Live observation registers immediately before existing per-target/global permit
acquisition; stage `waiting`, then `starting`, then the current structured-log
step (e.g. `upload`). It is removed in finally for success, failure and
cancellation, without changing queue tasks, locks, admission, cleanup, shutdown
or upload status. It is not persisted or replayed. A restarted process may see
pending/running catalog records with no live worker; show these as unconfirmed
work, not proven active. Stages describe work, never success percentages.

## Example (empty retained catalog)

```json
{
  "observed_at": "2026-11-08T12:00:00Z",
  "dataset": "retained_catalog_and_current_process",
  "window": {"after":"2026-11-07T12:00:00Z","before":"2026-11-08T12:00:00Z","time_field":"created_at"},
  "plans": {
    "total":0,"enabled":0,"disabled":0,"manual":0,"automatic_enabled":0,"automatic_paused":0,
    "without_recorded_run":{"total":0,"enabled":0,"items":[]},
    "without_recorded_completed_run":{"total":0,"enabled":0,"items":[]}
  },
  "backup_activity":{"recorded_pending":0,"recorded_running":0,"live":0,"total":0,"items":[]},
  "storage_activity":{"total":0,"items":[]},
  "recorded_copy_failures":{"backups":0,"total":0,"items":[]}
}
```

Example items: backup `{backup_id:7,plan_id:1,name:"Synthetic",catalog_status:
"completed",stage:"upload",live_worker:true}` remains active, not a success
message. Storage `{operation_id:"UUID",storage_id:"local",state:"running",
stage:"deleting"}` opens `/storage?operation=UUID&storage=local`. Copy failure
`{backup_id:7,storage_id:2,storage_name:null,status:"failed"}` opens run/archive
context #7 and location #2, which may legitimately be unavailable.

## Other Overview sources

Global retained runs/completed runs, bounded recent archives, and window failures
come from `/backups/history` totals, never from a 500-row frontend sample.
Resource inventory and configured source selection use `/docker/*` and `/targets`;
`selected_volumes=[]` means all matching volumes, not zero coverage. `/schedules`
definitions are separate from `/schedules/jobs` actual registered times. Docker
health, encryption/custody/deferred readiness and configured storage use their
existing endpoints. WebSocket is invalidation transport, not backend health.

`/settings/system-info` adds `disk_status: available | unavailable`. The existing
numeric legacy fields remain compatible (zeros on OSError), but Overview must
not evaluate them unless status is `available`. Values measure bytes on the
filesystem containing BACKUP_BASE_PATH, not remote quota. Low capacity means
free **< 1 GiB (1024**3 bytes)**, exactly the existing engine threshold. Equality
is not low. No extra paths/OSError details are exposed.
