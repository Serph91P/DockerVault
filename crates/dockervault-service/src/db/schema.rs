// Published SQLite DDL frozen independently from Python control 18d8c3d.
// Runtime migration logic must not regenerate the historical contract.
pub const TABLES: &[(&str, &str, &[&str])] = &[
    (
        "app_settings",
        "CREATE TABLE app_settings (\n\tid INTEGER NOT NULL, \n\t\"key\" VARCHAR(100) NOT NULL, \n\tvalue TEXT, \n\tupdated_at DATETIME, \n\tPRIMARY KEY (id)\n)",
        &[
            "CREATE INDEX ix_app_settings_id ON app_settings (id)",
            "CREATE UNIQUE INDEX ix_app_settings_key ON app_settings (\"key\")",
        ],
    ),
    (
        "backup_logs",
        "CREATE TABLE backup_logs (\n\tid INTEGER NOT NULL, \n\tbackup_id INTEGER NOT NULL, \n\tlevel VARCHAR(7), \n\tstep VARCHAR(50) NOT NULL, \n\tmessage TEXT NOT NULL, \n\tdetails JSON, \n\tcreated_at DATETIME, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(backup_id) REFERENCES backups (id) ON DELETE CASCADE\n)",
        &[
            "CREATE INDEX ix_backup_logs_backup_id ON backup_logs (backup_id)",
            "CREATE INDEX ix_backup_logs_id ON backup_logs (id)",
        ],
    ),
    (
        "backup_schedules",
        "CREATE TABLE backup_schedules (\n\tid INTEGER NOT NULL, \n\ttarget_id INTEGER NOT NULL, \n\tcron_expression VARCHAR(100) NOT NULL, \n\tnext_run DATETIME, \n\tlast_run DATETIME, \n\tenabled BOOLEAN, \n\tcreated_at DATETIME, \n\tupdated_at DATETIME, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(target_id) REFERENCES backup_targets (id)\n)",
        &["CREATE INDEX ix_backup_schedules_id ON backup_schedules (id)"],
    ),
    (
        "backup_storage_syncs",
        "CREATE TABLE backup_storage_syncs (\n\tid INTEGER NOT NULL, \n\tbackup_id INTEGER NOT NULL, \n\tstorage_id INTEGER NOT NULL, \n\tremote_path VARCHAR(1024), \n\tsynced_at DATETIME, \n\tsync_status VARCHAR(50), \n\terror_message TEXT, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(backup_id) REFERENCES backups (id) ON DELETE CASCADE, \n\tFOREIGN KEY(storage_id) REFERENCES remote_storages (id)\n)",
        &["CREATE INDEX ix_backup_storage_syncs_id ON backup_storage_syncs (id)"],
    ),
    (
        "backup_targets",
        "CREATE TABLE backup_targets (\n\tid INTEGER NOT NULL, \n\tname VARCHAR(255) NOT NULL, \n\ttarget_type VARCHAR(50) NOT NULL, \n\tcontainer_id VARCHAR(255), \n\tcontainer_name VARCHAR(255), \n\tvolume_name VARCHAR(255), \n\thost_path VARCHAR(1024), \n\tstack_name VARCHAR(255), \n\tschedule_id INTEGER, \n\tschedule_cron VARCHAR(100), \n\tenabled BOOLEAN, \n\tremoved_at DATETIME, \n\tretention_policy_id INTEGER, \n\tdependencies JSON, \n\tselected_volumes JSON, \n\tinclude_paths JSON, \n\texclude_paths JSON, \n\tper_volume_rules JSON, \n\tpre_backup_command TEXT, \n\tpost_backup_command TEXT, \n\tstop_container BOOLEAN, \n\tcompression_enabled BOOLEAN, \n\tsync_to_remote BOOLEAN, \n\tremote_storage_ids JSON, \n\tdelete_local_after_sync BOOLEAN, \n\tcreated_at DATETIME, \n\tupdated_at DATETIME, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(schedule_id) REFERENCES schedules (id), \n\tFOREIGN KEY(retention_policy_id) REFERENCES retention_policies (id)\n)",
        &["CREATE INDEX ix_backup_targets_id ON backup_targets (id)"],
    ),
    (
        "backups",
        "CREATE TABLE backups (\n\tid INTEGER NOT NULL, \n\ttarget_id INTEGER NOT NULL, \n\tbackup_type VARCHAR(11), \n\tstatus VARCHAR(9), \n\tfile_path VARCHAR(1024), \n\tfile_size INTEGER, \n\tchecksum VARCHAR(64), \n\tencrypted BOOLEAN, \n\tencryption_key_path VARCHAR(1024), \n\tstarted_at DATETIME, \n\tcompleted_at DATETIME, \n\tduration_seconds INTEGER, \n\terror_message TEXT, \n\tretry_count INTEGER, \n\tbackup_metadata JSON, \n\tcreated_at DATETIME, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(target_id) REFERENCES backup_targets (id)\n)",
        &["CREATE INDEX ix_backups_id ON backups (id)"],
    ),
    (
        "catalog_quarantine",
        "CREATE TABLE catalog_quarantine (table_name TEXT NOT NULL, row_id INTEGER NOT NULL, row_json TEXT NOT NULL, reason TEXT NOT NULL, PRIMARY KEY (table_name, row_id))",
        &[],
    ),
    (
        "encryption_config",
        "CREATE TABLE encryption_config (\n\tid INTEGER NOT NULL, \n\tpublic_key TEXT NOT NULL, \n\tkey_created_at DATETIME, \n\tencryption_enabled BOOLEAN, \n\tsetup_completed BOOLEAN, \n\tcreated_at DATETIME, \n\tupdated_at DATETIME, \n\tPRIMARY KEY (id)\n)",
        &["CREATE INDEX ix_encryption_config_id ON encryption_config (id)"],
    ),
    (
        "remote_storages",
        "CREATE TABLE remote_storages (\n\tid INTEGER NOT NULL, \n\tname VARCHAR(255) NOT NULL, \n\tstorage_type VARCHAR(50) NOT NULL, \n\tenabled BOOLEAN, \n\thost VARCHAR(255), \n\tport INTEGER, \n\tusername VARCHAR(255), \n\tpassword VARCHAR(255), \n\tbase_path VARCHAR(1024), \n\tssh_key_path VARCHAR(1024), \n\ts3_bucket VARCHAR(255), \n\ts3_region VARCHAR(50), \n\ts3_access_key VARCHAR(255), \n\ts3_secret_key VARCHAR(255), \n\ts3_endpoint_url VARCHAR(1024), \n\twebdav_url VARCHAR(1024), \n\trclone_remote VARCHAR(255), \n\tcreated_at DATETIME, \n\tupdated_at DATETIME, \n\tPRIMARY KEY (id), \n\tUNIQUE (name)\n)",
        &["CREATE INDEX ix_remote_storages_id ON remote_storages (id)"],
    ),
    (
        "retention_policies",
        "CREATE TABLE retention_policies (\n\tid INTEGER NOT NULL, \n\tname VARCHAR(255) NOT NULL, \n\tdescription TEXT, \n\tkeep_last INTEGER, \n\tkeep_daily INTEGER, \n\tkeep_weekly INTEGER, \n\tkeep_monthly INTEGER, \n\tkeep_yearly INTEGER, \n\tmax_age_days INTEGER, \n\tcreated_at DATETIME, \n\tupdated_at DATETIME, \n\tPRIMARY KEY (id), \n\tUNIQUE (name)\n)",
        &["CREATE INDEX ix_retention_policies_id ON retention_policies (id)"],
    ),
    (
        "schedules",
        "CREATE TABLE schedules (\n\tid INTEGER NOT NULL, \n\tname VARCHAR(255) NOT NULL, \n\tcron_expression VARCHAR(100) NOT NULL, \n\tdescription TEXT, \n\tenabled BOOLEAN, \n\tcreated_at DATETIME, \n\tupdated_at DATETIME, \n\tPRIMARY KEY (id), \n\tUNIQUE (name)\n)",
        &["CREATE INDEX ix_schedules_id ON schedules (id)"],
    ),
    (
        "schema_migrations",
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)",
        &[],
    ),
    (
        "sessions",
        "CREATE TABLE sessions (\n\tid INTEGER NOT NULL, \n\tuser_id INTEGER NOT NULL, \n\ttoken VARCHAR(255) NOT NULL, \n\texpires_at DATETIME NOT NULL, \n\tcreated_at DATETIME, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(user_id) REFERENCES users (id)\n)",
        &[
            "CREATE INDEX ix_sessions_id ON sessions (id)",
            "CREATE UNIQUE INDEX ix_sessions_token ON sessions (token)",
        ],
    ),
    (
        "storage_operation_items",
        "CREATE TABLE storage_operation_items (\n\tid INTEGER NOT NULL, \n\toperation_id VARCHAR(36) NOT NULL, \n\tpath VARCHAR(1024) NOT NULL, \n\tstate VARCHAR(16) NOT NULL, \n\terror VARCHAR(255), \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(operation_id) REFERENCES storage_operations (id)\n)",
        &[
            "CREATE INDEX ix_storage_operation_items_operation_id ON storage_operation_items (operation_id)",
        ],
    ),
    (
        "storage_operations",
        "CREATE TABLE storage_operations (\n\tid VARCHAR(36) NOT NULL, \n\tuser_id INTEGER NOT NULL, \n\trequest_id VARCHAR(36) NOT NULL, \n\tpayload JSON NOT NULL, \n\tstorage_identity JSON NOT NULL, \n\tparent_id VARCHAR(36), \n\taction VARCHAR(32) NOT NULL, \n\tstorage_id VARCHAR(32) NOT NULL, \n\tpath VARCHAR(1024) NOT NULL, \n\tstate VARCHAR(16) NOT NULL, \n\tstage VARCHAR(32) NOT NULL, \n\ttotal INTEGER NOT NULL, \n\tcompleted INTEGER NOT NULL, \n\tsucceeded INTEGER NOT NULL, \n\tfailed INTEGER NOT NULL, \n\tskipped INTEGER NOT NULL, \n\tunknown INTEGER NOT NULL, \n\tcancel_requested BOOLEAN NOT NULL, \n\tcreated_at DATETIME NOT NULL, \n\tupdated_at DATETIME NOT NULL, \n\tfinished_at DATETIME, \n\tPRIMARY KEY (id), \n\tUNIQUE (user_id, request_id), \n\tFOREIGN KEY(user_id) REFERENCES users (id), \n\tFOREIGN KEY(parent_id) REFERENCES storage_operations (id)\n)",
        &[
            "CREATE INDEX ix_storage_operations_storage_id ON storage_operations (storage_id)",
            "CREATE INDEX ix_storage_operations_user_id ON storage_operations (user_id)",
        ],
    ),
    (
        "users",
        "CREATE TABLE users (\n\tid INTEGER NOT NULL, \n\tusername VARCHAR(255) NOT NULL, \n\tpassword_hash VARCHAR(255) NOT NULL, \n\tis_admin BOOLEAN, \n\tcreated_at DATETIME, \n\tupdated_at DATETIME, \n\tlast_login DATETIME, \n\tPRIMARY KEY (id)\n)",
        &[
            "CREATE INDEX ix_users_id ON users (id)",
            "CREATE UNIQUE INDEX ix_users_username ON users (username)",
        ],
    ),
    (
        "workload_operations",
        "CREATE TABLE workload_operations (\n\tid INTEGER NOT NULL, \n\tbackup_id INTEGER, \n\tkind VARCHAR(16) NOT NULL, \n\tphase VARCHAR(32) NOT NULL, \n\tstate VARCHAR(32) NOT NULL, \n\tresources JSON NOT NULL, \n\terror TEXT, \n\tcreated_at DATETIME, \n\tPRIMARY KEY (id), \n\tFOREIGN KEY(backup_id) REFERENCES backups (id) ON DELETE SET NULL\n)",
        &[],
    ),
];
