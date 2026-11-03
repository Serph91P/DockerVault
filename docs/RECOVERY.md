# Interrupted operations and startup recovery

SQLite migration 3 adds `workload_operations`. Backup and restore operations
commit original workload states and immutable Docker IDs before stop intent,
then commit that intent before sending a stop. The journal is independent of a
backup's catalog lifetime: deleting a backup cannot discard its recovery intent.
Resource locks remain held until restart recovery and its journal update finish.

On startup, before the queue worker or scheduler starts, unfinished journals are
reconciled against actual Docker state. Only an originally running resource with
persisted stop intent is eligible for recovery. An originally stopped resource
is never started. A stop whose completion was not recorded may still be executing
inside Docker; recovery settles that recorded bounded stop before restarting,
instead of trusting a transient `running` state. Subsequent reconciliation sees
confirmed stop/recovery state and does not repeat completed transitions. Recovery
starts dependencies before consumers using start ranks persisted with the
immutable IDs, not stop-order iteration or mutable names. Historical journals
without ranks use the reverse of their recorded stop order. Both backup and
restore follow this rule; originally stopped members stay stopped.

Interrupted accepted pending/running jobs are explicitly marked failed, not
silently abandoned or replayed. An unfinished journal also fails a backup that
had already recorded its local archive as completed before upload. Recovery does
not recreate archives or replay uploads, so it cannot duplicate remote objects.
Restore recovery leaves the source backup's completed status intact and records
its interrupted operation in the existing backup logs. Retry the requested job
explicitly after inspecting its recovery/error logs and any partial output.

If a recorded immutable workload ID is missing or its state cannot be established,
recovery records `recovery_required` plus the affected IDs and prevents readiness.
Verify the workload/Docker connection and restart to retry reconciliation. Never
substitute a newly created container sharing an old name. Retain the SQLite DB
and logs for diagnosis; do not delete journal entries to bypass this condition.

Legacy pending/running records have no trustworthy lifecycle snapshot. They are
terminally failed with an actionable unknown-state warning; startup does not
infer permission to start arbitrary containers from their current target config.
Verify their workloads manually. See `MIGRATIONS.md` for isolated upgrade testing
and backup/rollback guidance.
