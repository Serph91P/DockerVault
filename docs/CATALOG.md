# Backup catalog lifecycle

A backup row identifies an archive, not merely its local pathname. Removing a
local copy while a remote path remains keeps the row, logs, remote sync records
and local encryption-key sidecar. The local pathname becomes null after local
removal; existing download/restore discovery can still use completed remote
copies. Retention prunes local copies under the same rule.

Deleting a target with catalogued remote paths or a storage configuration with
sync references returns HTTP 409 before deleting files or metadata. Remove the
remote copies explicitly first. A failed, rejected or unavailable remote delete
retains its sync record and archive identity and reports a warning. Successful
archive deletion cascades its logs and sync records at the SQLite boundary,
including bulk SQL deletion and retention.

Every SQLite connection enables foreign-key enforcement. On legacy upgrade,
logs and sync rows whose backup/storage parent no longer exists are copied to
`catalog_quarantine` as full JSON rows before removal from the active catalog.
No local/remote archive or key is deleted by this repair. Quarantined remote paths
remain available for administrator inspection and manual recovery; they must not
be automatically attached to a newly reused numeric ID. Existing valid
remote-only rows are unchanged. Already-misassociated historical rows cannot be
unambiguously reconstructed from an integer ID alone. Other foreign-key
violations stop startup rather than guessing ownership or silently dropping data.

## Active artifact ownership

Before any archive worker writes, the backup commits its potential archive,
encrypted output, temporary encrypted output and key paths in existing backup
metadata (`active_artifacts`). Orphan collection protects those reservations only
while the owner is RUNNING. Completed archive/key paths remain protected by their
normal catalog fields, including during upload. Failed/recovered abandoned jobs do
not retain staging reservations forever.

The initial inventory is only a hint. Before unlinking an unknown file, cleanup
acquires SQLite's writer transaction and re-reads current ownership. Producers
must commit reservations before writing, so completion/new reservations cannot
race the final ownership check. Truly orphaned files in the same active target
directory can still be deleted; no age heuristic or directory-wide exclusion is
used.

Back up the SQLite database before upgrades. Keep the matching previous image
and database backup together for rollback; do not run an older image against a
partially upgraded database. The migration ledger and supported legacy upgrades
are documented separately by the versioned-migrations change (#174).

Restore resolves a retained remote-only archive and its required key before
calling the engine, passes those working paths explicitly, and removes temporary
downloads after restore. It never rewrites the permanent catalog to temporary
paths. A missing archive/key cannot report a successful restore.

A failed/pending sync may already have uploaded archive bytes (for example when
the key sidecar failed). Every non-empty recorded remote path therefore needs a
confirmed provider deletion before its metadata/key can be discarded, regardless
of sync status. Missing backend or refused deletion retains recovery identity and
reports a warning for both single and bulk deletion.
