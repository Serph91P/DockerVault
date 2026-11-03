# Provider configuration and reference validation

Storage create/update validate the effective merged record before any SQLite commit or adapter registration, per provider: `local`, `ssh`/`sftp`, `webdav`, `s3`, `ftp`, `rclone` (`backend/app/storage_validation.py`). Ports are bounded to 1-65535 at schema and validation layers. Empty-string credential edits keep the stored secret; `null` is an explicit clear, and enabling-invalid configurations is rejected. List/get responses expose `validation_errors` repair hints for legacy records without blocking startup or reads.

Target create/update reject new dangling `schedule_id`, `retention_policy_id` and `remote_storage_ids` with a structured 400 before persistence (`backend/app/target_validation.py`). Existing invalid legacy records remain readable and are surfaced with the same repair hints on list/get. `GET /backups?status=` accepts only real `BackupStatus` values; unknown statuses return a deliberate 422 instead of a 500.

No provider is contacted during validation, no storage architecture is replaced and no destructive normalization is applied to stored records.
