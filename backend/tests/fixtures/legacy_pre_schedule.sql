-- Frozen from audited eea44ed schema with the supported pre-column-upgrade fields.
-- Test inputs are static; tests must not regenerate them from the current models.
CREATE TABLE app_settings (
	id INTEGER NOT NULL,
	"key" VARCHAR(100) NOT NULL,
	value TEXT,
	updated_at DATETIME,
	PRIMARY KEY (id)
);

CREATE TABLE encryption_config (
	id INTEGER NOT NULL,
	public_key TEXT NOT NULL,
	key_created_at DATETIME,
	encryption_enabled BOOLEAN,
	setup_completed BOOLEAN,
	created_at DATETIME,
	updated_at DATETIME,
	PRIMARY KEY (id)
);

CREATE TABLE remote_storages (
	id INTEGER NOT NULL,
	name VARCHAR(255) NOT NULL,
	storage_type VARCHAR(50) NOT NULL,
	enabled BOOLEAN,
	host VARCHAR(255),
	port INTEGER,
	username VARCHAR(255),
	password VARCHAR(255),
	base_path VARCHAR(1024),
	ssh_key_path VARCHAR(1024),
	s3_bucket VARCHAR(255),
	s3_region VARCHAR(50),
	s3_access_key VARCHAR(255),
	s3_secret_key VARCHAR(255),
	s3_endpoint_url VARCHAR(1024),
	webdav_url VARCHAR(1024),
	rclone_remote VARCHAR(255),
	created_at DATETIME,
	updated_at DATETIME,
	PRIMARY KEY (id),
	UNIQUE (name)
);

CREATE TABLE retention_policies (
	id INTEGER NOT NULL,
	name VARCHAR(255) NOT NULL,
	description TEXT,
	keep_daily INTEGER,
	keep_weekly INTEGER,
	keep_monthly INTEGER,
	keep_yearly INTEGER,
	max_age_days INTEGER,
	created_at DATETIME,
	updated_at DATETIME,
	PRIMARY KEY (id),
	UNIQUE (name)
);

CREATE TABLE schedules (
	id INTEGER NOT NULL,
	name VARCHAR(255) NOT NULL,
	cron_expression VARCHAR(100) NOT NULL,
	description TEXT,
	enabled BOOLEAN,
	created_at DATETIME,
	updated_at DATETIME,
	PRIMARY KEY (id),
	UNIQUE (name)
);

CREATE TABLE users (
	id INTEGER NOT NULL,
	username VARCHAR(255) NOT NULL,
	password_hash VARCHAR(255) NOT NULL,
	is_admin BOOLEAN,
	created_at DATETIME,
	updated_at DATETIME,
	last_login DATETIME,
	PRIMARY KEY (id)
);

CREATE TABLE backup_targets (
	id INTEGER NOT NULL,
	name VARCHAR(255) NOT NULL,
	target_type VARCHAR(50) NOT NULL,
	container_id VARCHAR(255),
	container_name VARCHAR(255),
	volume_name VARCHAR(255),
	host_path VARCHAR(1024),
	stack_name VARCHAR(255),
	schedule_cron VARCHAR(100),
	enabled BOOLEAN,
	dependencies JSON,
	pre_backup_command TEXT,
	post_backup_command TEXT,
	stop_container BOOLEAN,
	compression_enabled BOOLEAN,
	sync_to_remote BOOLEAN,
	remote_storage_ids JSON,
	created_at DATETIME,
	updated_at DATETIME,
	PRIMARY KEY (id)
);

CREATE TABLE sessions (
	id INTEGER NOT NULL,
	user_id INTEGER NOT NULL,
	token VARCHAR(255) NOT NULL,
	expires_at DATETIME NOT NULL,
	created_at DATETIME,
	PRIMARY KEY (id),
	FOREIGN KEY(user_id) REFERENCES users (id)
);

CREATE TABLE backup_schedules (
	id INTEGER NOT NULL,
	target_id INTEGER NOT NULL,
	cron_expression VARCHAR(100) NOT NULL,
	next_run DATETIME,
	last_run DATETIME,
	enabled BOOLEAN,
	created_at DATETIME,
	updated_at DATETIME,
	PRIMARY KEY (id),
	FOREIGN KEY(target_id) REFERENCES backup_targets (id)
);

CREATE TABLE backups (
	id INTEGER NOT NULL,
	target_id INTEGER NOT NULL,
	backup_type VARCHAR(11),
	status VARCHAR(9),
	file_path VARCHAR(1024),
	file_size INTEGER,
	checksum VARCHAR(64),
	started_at DATETIME,
	completed_at DATETIME,
	duration_seconds INTEGER,
	error_message TEXT,
	retry_count INTEGER,
	backup_metadata JSON,
	created_at DATETIME,
	PRIMARY KEY (id),
	FOREIGN KEY(target_id) REFERENCES backup_targets (id)
);

CREATE TABLE backup_logs (
	id INTEGER NOT NULL,
	backup_id INTEGER NOT NULL,
	level VARCHAR(7),
	step VARCHAR(50) NOT NULL,
	message TEXT NOT NULL,
	details JSON,
	created_at DATETIME,
	PRIMARY KEY (id),
	FOREIGN KEY(backup_id) REFERENCES backups (id) ON DELETE CASCADE
);

CREATE TABLE backup_storage_syncs (
	id INTEGER NOT NULL,
	backup_id INTEGER NOT NULL,
	storage_id INTEGER NOT NULL,
	remote_path VARCHAR(1024),
	synced_at DATETIME,
	sync_status VARCHAR(50),
	error_message TEXT,
	PRIMARY KEY (id),
	FOREIGN KEY(backup_id) REFERENCES backups (id) ON DELETE CASCADE,
	FOREIGN KEY(storage_id) REFERENCES remote_storages (id)
);
