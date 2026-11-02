# SQLite upgrades and rollback

Supported inputs are an empty database, the unversioned audited schema, and the
historical pre-column-upgrade schema captured in
`backend/tests/fixtures/legacy_pre_schedule.sql`. Existing IDs, opaque credential
values, remote references, archive metadata and unknown extra columns are kept.
This is not an import/export or recovery-bundle feature.

`schema_migrations` records ordered versions: **1** baselines the legacy columns
and applies the historic remote-sync flag rewrite once; **2** quarantines orphan
log/sync metadata under the catalog lifecycle in `CATALOG.md`. Completed data
transformations are never replayed, including when a user subsequently disables
remote sync. A future/gapped ledger or unresolved foreign-key violation prevents
startup instead of serving a partially understood database.

All pending DDL, data changes and ledger entries execute under one explicit
SQLite `BEGIN IMMEDIATE` transaction. This is necessary because the sqlite3
legacy driver does not automatically begin a transaction for DDL. An exception,
cancellation or process exit rolls back the pending upgrade; retry starts from
the same committed ledger. The FastAPI lifespan cannot become ready before the
required migration finishes. Other startup responsibilities (credential wrapping
and recovery of interrupted backup jobs) are separate from this schema ledger.

## Before upgrading an existing installation

1. Stop the application cleanly and retain the exact previous image/version.
2. Copy the stopped SQLite database **and any existing `-wal`/`-shm` companions**
   to a separate backup location, or use SQLite's online backup API if a copy
   must be taken while the application is still running. A plain copy of an
   actively written database is not a consistent backup.
3. Test the upgrade against that backup in isolation first, with synthetic or
   explicitly authorized data and no access to production workload controls.
4. Start the new image with the original database and confirm healthy readiness.
   Do not delete a failed database: retain it and the logs for diagnosis.

For rollback, stop the new application, restore the matching pre-upgrade database
backup (including its saved companions as applicable) and use the matching old
image. Do not run an older image against a database with a newer ledger. Archives
and provider objects are never deleted by schema/catalog migration itself.
