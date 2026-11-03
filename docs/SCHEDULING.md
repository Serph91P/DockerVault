# Live scheduling configuration

Target and shared-schedule create/update/delete operations reconcile live backup
jobs **after** committing and releasing their request database session. Startup
uses the same reconciliation path for existing records; no user re-save is needed.
Reconciliation serializes its fresh database reads and updates so late callbacks
do not reinstall captured stale configuration. Maintenance jobs are not removed.

Each eligible target has one `backup_target_<id>` job. Reassigning a schedule or
editing its cron replaces the trigger; disabling, clearing or deleting removes the
job. Metadata-only name changes preserve the existing next-run time, including an
already due backup, rather than rescheduling it away. Target PUT returns refreshed
schedule/retention relations, not the old relation alongside a new foreign key.

## Shared and legacy precedence

An assigned shared schedule takes precedence over legacy `schedule_cron`.
A disabled assigned shared schedule suppresses the job; it does not reactivate a
legacy cron. Unlinking/deleting the shared schedule restores a retained legacy
cron. To remove scheduling entirely, clear both `schedule_id` and `schedule_cron`.
For these scheduling fields, omission preserves the current value and explicit
JSON null clears it. Other nullable target-field rules are unchanged here.
The legacy listing retains its response shape but reports effective enablement
and no next run when an assigned shared schedule is disabled.

## Failures and retry

A failed configuration database transaction does not change live jobs. If saving
succeeds but projecting that state into the scheduler fails, the API returns503
with an explicit **configuration saved** message. After state is read, obsolete
jobs and jobs whose replacement fails are removed rather than left on a stale
trigger; healthy targets continue. A database-read failure is also visible and
requires reconciliation retry once the database is available.
Do not repeat a create request assuming rollback. Refresh the saved configuration
and retry `POST /api/v1/schedules/reload`, which reconciles persisted state without
another save. Normal authentication still applies. A successful reload returns
`{"status":"reloaded"}`. Repeated reloads do not duplicate jobs or reset unchanged
next-run times. Startup logs any incomplete reconciliation so invalid historical
entries do not hide the error or prevent healthy entries from loading.

This is the existing single-process APScheduler/SQLite application, not a new
broker. Cron syntax/timezone behavior is not expanded by this reconciliation fix.
