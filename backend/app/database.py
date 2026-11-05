"""
Database configuration and models using SQLAlchemy with async support.
"""

import asyncio
import enum
import logging
from datetime import datetime, timezone

from anyio import CancelScope
from sqlalchemy import JSON, Boolean, Column, DateTime
from sqlalchemy import Enum as SQLEnum
from sqlalchemy import ForeignKey, Integer, String, Text, UniqueConstraint, event, text
from sqlalchemy.engine import Engine, ExceptionContext
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from sqlalchemy.orm import DeclarativeBase, backref, relationship

from app.config import settings


class Base(DeclarativeBase):
    pass


class BackupStatus(enum.Enum):
    PENDING = "pending"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"


class BackupType(enum.Enum):
    FULL = "full"
    INCREMENTAL = "incremental"


class LogLevel(enum.Enum):
    DEBUG = "debug"
    INFO = "info"
    WARNING = "warning"
    ERROR = "error"


def _utcnow():
    return datetime.now(timezone.utc)


class RetentionPolicy(Base):
    """Retention policy configuration using GFS (Grandfather-Father-Son) strategy."""

    __tablename__ = "retention_policies"

    id = Column(Integer, primary_key=True, index=True)
    name = Column(String(255), unique=True, nullable=False)
    description = Column(Text, nullable=True)

    # GFS retention settings (similar to restic)
    keep_last = Column(Integer, default=3)  # Keep last N backups regardless of age
    keep_daily = Column(Integer, default=7)  # Keep last N daily backups
    keep_weekly = Column(Integer, default=4)  # Keep last N weekly backups
    keep_monthly = Column(Integer, default=6)  # Keep last N monthly backups
    keep_yearly = Column(Integer, default=2)  # Keep last N yearly backups
    max_age_days = Column(Integer, default=365)  # Maximum age of any backup
    created_at = Column(DateTime, default=_utcnow)
    updated_at = Column(DateTime, default=_utcnow, onupdate=_utcnow)


class BackupTarget(Base):
    """Defines what to backup (container, volume, or path)."""

    __tablename__ = "backup_targets"

    id = Column(Integer, primary_key=True, index=True)
    name = Column(String(255), nullable=False)
    target_type = Column(String(50), nullable=False)  # container, volume, path, stack
    container_id = Column(String(255), nullable=True)
    container_name = Column(String(255), nullable=True)
    volume_name = Column(String(255), nullable=True)
    host_path = Column(String(1024), nullable=True)
    stack_name = Column(String(255), nullable=True)

    # Scheduling - NEW: Reference to Schedule entity
    schedule_id = Column(Integer, ForeignKey("schedules.id"), nullable=True)
    schedule = relationship("Schedule", back_populates="targets")
    # DEPRECATED: Keep for migration, will be removed later
    schedule_cron = Column(String(100), nullable=True)
    enabled = Column(Boolean, default=True)

    # Retention
    removed_at = Column(DateTime, nullable=True)

    retention_policy_id = Column(
        Integer, ForeignKey("retention_policies.id"), nullable=True
    )
    retention_policy = relationship("RetentionPolicy")

    # Dependencies (other targets that must be stopped before backup)
    dependencies = Column(JSON, default=list)  # List of container names

    # Volume selection (for container/stack backups - which volumes to include)
    selected_volumes = Column(JSON, default=list)  # Empty = all volumes

    # Path filtering within volumes
    include_paths = Column(JSON, default=list)  # Include only these paths (empty = all)
    exclude_paths = Column(JSON, default=list)  # Exclude these paths/patterns

    # Per-volume path rules (overrides global include/exclude for specific volumes)
    per_volume_rules = Column(
        JSON, default=dict
    )  # {volume_name: {include_paths: [], exclude_paths: []}}

    # Pre/Post hooks
    pre_backup_command = Column(Text, nullable=True)
    post_backup_command = Column(Text, nullable=True)

    # Settings
    stop_container = Column(Boolean, default=True)  # Stop container during backup
    compression_enabled = Column(Boolean, default=True)

    # Remote storage sync
    sync_to_remote = Column(Boolean, default=False)
    remote_storage_ids = Column(JSON, default=list)  # List of storage IDs to sync to
    delete_local_after_sync = Column(
        Boolean, default=False
    )  # Delete local files after successful remote sync

    created_at = Column(DateTime, default=_utcnow)
    updated_at = Column(DateTime, default=_utcnow, onupdate=_utcnow)

    backups = relationship(
        "Backup", back_populates="target", cascade="all, delete-orphan"
    )


class Backup(Base):
    """Individual backup record."""

    __tablename__ = "backups"

    id = Column(Integer, primary_key=True, index=True)
    target_id = Column(Integer, ForeignKey("backup_targets.id"), nullable=False)
    target = relationship("BackupTarget", back_populates="backups")

    backup_type = Column(SQLEnum(BackupType), default=BackupType.FULL)
    status = Column(SQLEnum(BackupStatus), default=BackupStatus.PENDING)

    # Backup details
    file_path = Column(String(1024), nullable=True)
    file_size = Column(Integer, nullable=True)  # Size in bytes
    checksum = Column(String(64), nullable=True)  # SHA256

    # Encryption
    encrypted = Column(Boolean, default=False)
    encryption_key_path = Column(String(1024), nullable=True)  # Path to .key file

    # Timing
    started_at = Column(DateTime, nullable=True)
    completed_at = Column(DateTime, nullable=True)
    duration_seconds = Column(Integer, nullable=True)

    # Error handling
    error_message = Column(Text, nullable=True)
    retry_count = Column(Integer, default=0)

    # Extra data
    backup_metadata = Column(JSON, default=dict)

    created_at = Column(DateTime, default=_utcnow)


class WorkloadOperation(Base):
    """Durable lifecycle intent survives both process loss and backup deletion."""

    __tablename__ = "workload_operations"
    id = Column(Integer, primary_key=True)
    backup_id = Column(
        Integer, ForeignKey("backups.id", ondelete="SET NULL"), nullable=True
    )
    kind = Column(String(16), nullable=False)
    phase = Column(String(32), nullable=False, default="claimed")
    state = Column(String(32), nullable=False, default="active")
    resources = Column(JSON, nullable=False, default=dict)
    error = Column(Text, nullable=True)
    created_at = Column(DateTime(timezone=True), default=_utcnow)


class StorageOperation(Base):
    """Durable storage effects; intentionally separate from workload recovery."""

    __tablename__ = "storage_operations"
    __table_args__ = (UniqueConstraint("user_id", "request_id"),)
    id = Column(String(36), primary_key=True)
    user_id = Column(Integer, ForeignKey("users.id"), nullable=False, index=True)
    request_id = Column(String(36), nullable=False)
    payload = Column(JSON, nullable=False)
    storage_identity = Column(JSON, nullable=False)
    parent_id = Column(String(36), ForeignKey("storage_operations.id"), nullable=True)
    action = Column(String(32), nullable=False)
    storage_id = Column(String(32), nullable=False, index=True)
    path = Column(String(1024), nullable=False)
    state = Column(String(16), nullable=False, default="queued")
    stage = Column(String(32), nullable=False, default="queued")
    total = Column(Integer, nullable=False)
    completed = Column(Integer, nullable=False, default=0)
    succeeded = Column(Integer, nullable=False, default=0)
    failed = Column(Integer, nullable=False, default=0)
    skipped = Column(Integer, nullable=False, default=0)
    unknown = Column(Integer, nullable=False, default=0)
    cancel_requested = Column(Boolean, nullable=False, default=False)
    created_at = Column(DateTime, nullable=False, default=_utcnow)
    updated_at = Column(DateTime, nullable=False, default=_utcnow)
    finished_at = Column(DateTime, nullable=True)


class StorageOperationItem(Base):
    __tablename__ = "storage_operation_items"
    id = Column(Integer, primary_key=True)
    operation_id = Column(
        String(36), ForeignKey("storage_operations.id"), nullable=False, index=True
    )
    path = Column(String(1024), nullable=False)
    state = Column(String(16), nullable=False, default="pending")
    error = Column(String(255), nullable=True)


class BackupLog(Base):
    """Structured log entry for a backup job step."""

    __tablename__ = "backup_logs"

    id = Column(Integer, primary_key=True, index=True)
    backup_id = Column(
        Integer,
        ForeignKey("backups.id", ondelete="CASCADE"),
        nullable=False,
        index=True,
    )
    level = Column(SQLEnum(LogLevel), default=LogLevel.INFO)
    step = Column(
        String(50), nullable=False
    )  # e.g. start, stop_containers, archive, encrypt, upload, restart, cleanup, complete, error
    message = Column(Text, nullable=False)
    details = Column(
        JSON, nullable=True
    )  # Extra structured data (container names, file sizes, etc.)
    created_at = Column(DateTime, default=_utcnow)

    backup = relationship(
        "Backup",
        backref=backref("logs", cascade="all, delete-orphan", passive_deletes=True),
    )


class Schedule(Base):
    """Reusable schedule that can be assigned to multiple targets."""

    __tablename__ = "schedules"

    id = Column(Integer, primary_key=True, index=True)
    name = Column(String(255), unique=True, nullable=False)
    cron_expression = Column(String(100), nullable=False)
    description = Column(Text, nullable=True)
    enabled = Column(Boolean, default=True)

    created_at = Column(DateTime, default=_utcnow)
    updated_at = Column(DateTime, default=_utcnow, onupdate=_utcnow)

    # Relationship to targets using this schedule
    targets = relationship("BackupTarget", back_populates="schedule")


class BackupSchedule(Base):
    """Scheduled backup jobs (legacy - kept for compatibility)."""

    __tablename__ = "backup_schedules"

    id = Column(Integer, primary_key=True, index=True)
    target_id = Column(Integer, ForeignKey("backup_targets.id"), nullable=False)
    target = relationship("BackupTarget")

    cron_expression = Column(String(100), nullable=False)
    next_run = Column(DateTime, nullable=True)
    last_run = Column(DateTime, nullable=True)
    enabled = Column(Boolean, default=True)

    created_at = Column(DateTime, default=_utcnow)
    updated_at = Column(DateTime, default=_utcnow, onupdate=_utcnow)


class RemoteStorage(Base):
    """Remote storage configuration for off-site backups."""

    __tablename__ = "remote_storages"

    id = Column(Integer, primary_key=True, index=True)
    name = Column(String(255), unique=True, nullable=False)
    storage_type = Column(
        String(50), nullable=False
    )  # local, ssh, webdav, s3, ftp, rclone
    enabled = Column(Boolean, default=True)

    # Connection settings
    host = Column(String(255), nullable=True)
    port = Column(Integer, nullable=True)
    username = Column(String(255), nullable=True)
    password = Column(String(255), nullable=True)  # TODO: Encrypt in production

    # Path settings
    base_path = Column(String(1024), default="/backups")

    # SSH/SFTP specific
    ssh_key_path = Column(String(1024), nullable=True)

    # S3 specific
    s3_bucket = Column(String(255), nullable=True)
    s3_region = Column(String(50), nullable=True)
    s3_access_key = Column(String(255), nullable=True)
    s3_secret_key = Column(String(255), nullable=True)
    s3_endpoint_url = Column(String(1024), nullable=True)

    # WebDAV specific
    webdav_url = Column(String(1024), nullable=True)

    # Rclone specific
    rclone_remote = Column(String(255), nullable=True)

    created_at = Column(DateTime, default=_utcnow)
    updated_at = Column(DateTime, default=_utcnow, onupdate=_utcnow)


class BackupStorageSync(Base):
    """Track which backups are synced to which remote storages."""

    __tablename__ = "backup_storage_syncs"

    id = Column(Integer, primary_key=True, index=True)
    backup_id = Column(
        Integer, ForeignKey("backups.id", ondelete="CASCADE"), nullable=False
    )
    storage_id = Column(Integer, ForeignKey("remote_storages.id"), nullable=False)
    remote_path = Column(String(1024), nullable=True)
    synced_at = Column(DateTime, nullable=True)
    sync_status = Column(
        String(50), default="pending"
    )  # pending, syncing, completed, failed
    error_message = Column(Text, nullable=True)

    backup = relationship(
        "Backup",
        backref=backref(
            "storage_syncs", cascade="all, delete-orphan", passive_deletes=True
        ),
    )
    storage = relationship("RemoteStorage")


class EncryptionConfig(Base):
    """Encryption configuration - stores public key, private key is exported to user."""

    __tablename__ = "encryption_config"

    id = Column(Integer, primary_key=True, index=True)
    public_key = Column(Text, nullable=False)  # age public key (age1...)
    # Private key is NOT stored - user must export and save it
    key_created_at = Column(DateTime, default=_utcnow)
    encryption_enabled = Column(Boolean, default=True)
    setup_completed = Column(Boolean, default=False)  # User confirmed key export

    created_at = Column(DateTime, default=_utcnow)
    updated_at = Column(DateTime, default=_utcnow, onupdate=_utcnow)


class User(Base):
    """User account for authentication."""

    __tablename__ = "users"

    id = Column(Integer, primary_key=True, index=True)
    username = Column(String(255), unique=True, nullable=False, index=True)
    password_hash = Column(String(255), nullable=False)
    is_admin = Column(Boolean, default=True)  # First user is always admin

    created_at = Column(DateTime, default=_utcnow)
    updated_at = Column(DateTime, default=_utcnow, onupdate=_utcnow)
    last_login = Column(DateTime, nullable=True)


class Session(Base):
    """User session for authentication."""

    __tablename__ = "sessions"

    id = Column(Integer, primary_key=True, index=True)
    user_id = Column(Integer, ForeignKey("users.id"), nullable=False)
    token = Column(String(255), unique=True, nullable=False, index=True)
    expires_at = Column(DateTime, nullable=False)
    created_at = Column(DateTime, default=_utcnow)

    user = relationship("User")


class AppSettings(Base):
    """Application settings stored in database."""

    __tablename__ = "app_settings"

    id = Column(Integer, primary_key=True, index=True)
    key = Column(String(100), unique=True, nullable=False, index=True)
    value = Column(Text, nullable=True)
    updated_at = Column(DateTime, default=_utcnow, onupdate=_utcnow)


# Every pooled SQLite connection must enforce the catalog's declared cascades.
@event.listens_for(Engine, "connect")
def _enable_sqlite_foreign_keys(connection, _record):
    if "sqlite" not in type(connection).__module__:
        return
    cursor = connection.cursor()
    try:
        cursor.execute("PRAGMA foreign_keys=ON")
    finally:
        cursor.close()


@event.listens_for(Engine, "handle_error")
def _close_cancelled_sqlite_cursor(context: ExceptionContext) -> None:
    if (
        context.dialect.name != "sqlite"
        or context.dialect.driver != "aiosqlite"
        or not isinstance(context.original_exception, asyncio.CancelledError)
        or context.execution_context is None
    ):
        return
    cursor = getattr(context.execution_context, "cursor", None)
    if cursor is None:
        return
    # SQLAlchemy skips cursor close on disconnect; an unfinished SQLite SELECT still owns its read lock.
    while True:
        try:
            with CancelScope(shield=True):
                cursor.close()
            return
        except asyncio.CancelledError:
            continue  # Finish only cursor cleanup; never replay the cancelled statement.
        except Exception:
            logging.getLogger(__name__).warning(
                "Cancelled SQLite cursor cleanup failed; connection invalidation will continue"
            )
            return


# Database engine and session
engine = create_async_engine(
    settings.DATABASE_URL,
    echo=False,
)


class CancellationSafeAsyncSession(AsyncSession):
    """Finish rollback/close without level cancellation spinning in SQLAlchemy."""

    async def __aexit__(self, type_, value, traceback) -> None:
        with CancelScope(shield=True):
            await super().__aexit__(type_, value, traceback)


async_session = async_sessionmaker(
    engine,
    class_=CancellationSafeAsyncSession,
    expire_on_commit=False,
)


async def init_db():
    """Initialize the database only after all required migrations succeed."""
    await run_migrations()

    # Encrypt any plaintext credentials in remote_storage
    await _migrate_plaintext_credentials()

    # Mark backups that were RUNNING when the container died as FAILED.
    # Without this, the UI shows perpetual "running" rows and the per-target
    # lock logic can race against the orphaned record on restart.
    await _recover_orphaned_backups()

    # Create default retention policy
    async with async_session() as session:
        from sqlalchemy import select

        result = await session.execute(
            select(RetentionPolicy).where(RetentionPolicy.name == "default")
        )
        if not result.scalar_one_or_none():
            default_policy = RetentionPolicy(
                name="default",
                keep_daily=7,
                keep_weekly=4,
                keep_monthly=6,
                keep_yearly=2,
                max_age_days=365,
            )
            session.add(default_policy)
            await session.commit()


async def run_migrations():
    """Upgrade atomically; restore FK enforcement before returning a pooled connection."""
    migrations = (
        _upgrade_legacy_schema,
        _repair_catalog_orphans,
        _create_operation_journal,
        _ensure_target_foreign_keys,
        _create_operation_journal,
        _add_plan_tombstone,
        _create_storage_operations,
    )
    async with engine.connect() as conn:
        # SQLite's documented rebuild procedure requires this outside a transaction.
        await conn.execute(text("PRAGMA foreign_keys=OFF"))
        await conn.commit()
        try:
            async with conn.begin():
                await conn.execute(text("BEGIN IMMEDIATE"))
                await conn.execute(
                    text(
                        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)"
                    )
                )
                completed = (
                    (
                        await conn.execute(
                            text(
                                "SELECT version FROM schema_migrations ORDER BY version"
                            )
                        )
                    )
                    .scalars()
                    .all()
                )
                if completed != list(range(1, len(completed) + 1)) or len(
                    completed
                ) > len(migrations):
                    raise RuntimeError(
                        "Unsupported or incomplete migration ledger; refusing startup"
                    )
                for version, migrate in enumerate(migrations, start=1):
                    if version in completed:
                        continue
                    if version == 1:
                        await conn.run_sync(Base.metadata.create_all)
                    await migrate(conn)
                    await conn.execute(
                        text(
                            "INSERT INTO schema_migrations (version) VALUES (:version)"
                        ),
                        {"version": version},
                    )
                if (
                    await conn.execute(text("PRAGMA foreign_key_check"))
                ).first() is not None:
                    raise RuntimeError(
                        "Catalog foreign-key violations; refusing startup"
                    )
        finally:
            await conn.rollback()
            await conn.execute(text("PRAGMA foreign_keys=ON"))
            await conn.commit()


async def _create_storage_operations(conn):
    for model in (StorageOperation, StorageOperationItem):
        await conn.run_sync(lambda sync: model.__table__.create(sync, checkfirst=True))


async def _add_plan_tombstone(conn):
    """Keep historical source identity when active automation is removed."""
    columns = {
        row[1]
        for row in (await conn.execute(text("PRAGMA table_info(backup_targets)"))).all()
    }
    if "removed_at" not in columns:
        await conn.execute(
            text("ALTER TABLE backup_targets ADD COLUMN removed_at DATETIME")
        )


async def _ensure_target_foreign_keys(conn):
    """Converge supported legacy target tables without losing extension columns/objects."""
    import re

    references = {
        "schedule_id": "schedules",
        "retention_policy_id": "retention_policies",
    }
    existing = {
        (row[3], row[2], row[4])
        for row in (
            await conn.execute(text("PRAGMA foreign_key_list(backup_targets)"))
        ).all()
    }
    missing = {
        column: parent
        for column, parent in references.items()
        if (column, parent, "id") not in existing
    }
    if not missing:
        return
    for column, parent in missing.items():
        invalid = (
            await conn.execute(
                text(
                    f'SELECT id FROM backup_targets WHERE "{column}" IS NOT NULL AND "{column}" NOT IN (SELECT id FROM "{parent}") LIMIT 1'
                )
            )
        ).first()
        if invalid:
            raise RuntimeError(
                f"Invalid legacy target {column} reference; refusing startup without changing data"
            )
    original = (
        await conn.execute(
            text(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='backup_targets'"
            )
        )
    ).scalar_one()
    replacement, count = re.subn(
        r'(?i)^CREATE TABLE\s+(?:"backup_targets"|`backup_targets`|\[backup_targets\]|backup_targets)',
        'CREATE TABLE "backup_targets__fk_rebuild"',
        original,
        count=1,
    )
    if count != 1:
        raise RuntimeError("Unsupported legacy target table declaration")
    closing = replacement.rfind(")")
    constraints = ", " + ", ".join(
        f'FOREIGN KEY ("{column}") REFERENCES "{parent}" (id)'
        for column, parent in missing.items()
    )
    replacement = replacement[:closing] + constraints + replacement[closing:]
    objects = (
        (
            await conn.execute(
                text(
                    "SELECT sql FROM sqlite_master WHERE tbl_name='backup_targets' AND type IN ('index','trigger') AND sql IS NOT NULL"
                )
            )
        )
        .scalars()
        .all()
    )
    columns = [
        row[1]
        for row in (await conn.execute(text("PRAGMA table_info(backup_targets)"))).all()
    ]
    quoted = ", ".join('"' + name.replace('"', '""') + '"' for name in columns)
    await conn.execute(text(replacement))
    await conn.execute(
        text(
            f"INSERT INTO backup_targets__fk_rebuild ({quoted}) SELECT {quoted} FROM backup_targets"
        )
    )
    await conn.execute(text("DROP TABLE backup_targets"))
    await conn.execute(
        text("ALTER TABLE backup_targets__fk_rebuild RENAME TO backup_targets")
    )
    for sql in objects:
        await conn.execute(text(sql))


async def _create_operation_journal(conn):
    await conn.run_sync(
        lambda sync: WorkloadOperation.__table__.create(sync, checkfirst=True)
    )


async def _upgrade_legacy_schema(conn):
    """Baseline the supported unversioned schema and its once-only data rewrite."""
    # Check and add missing columns to backups table
    result = await conn.execute(text("PRAGMA table_info(backups)"))
    backups_columns = {row[1] for row in result.fetchall()}

    if "encrypted" not in backups_columns:
        await conn.execute(
            text("ALTER TABLE backups ADD COLUMN encrypted BOOLEAN DEFAULT 0")
        )

    if "encryption_key_path" not in backups_columns:
        await conn.execute(
            text("ALTER TABLE backups ADD COLUMN encryption_key_path VARCHAR(1024)")
        )

    # Check and add missing columns to backup_targets table
    result = await conn.execute(text("PRAGMA table_info(backup_targets)"))
    targets_columns = {row[1] for row in result.fetchall()}

    if "schedule_id" not in targets_columns:
        await conn.execute(
            text("ALTER TABLE backup_targets ADD COLUMN schedule_id INTEGER")
        )

    if "retention_policy_id" not in targets_columns:
        await conn.execute(
            text("ALTER TABLE backup_targets ADD COLUMN retention_policy_id INTEGER")
        )

    # Add volume selection and path filtering columns
    if "selected_volumes" not in targets_columns:
        await conn.execute(
            text(
                "ALTER TABLE backup_targets ADD COLUMN selected_volumes JSON "
                "DEFAULT '[]'"
            )
        )

    if "include_paths" not in targets_columns:
        await conn.execute(
            text(
                "ALTER TABLE backup_targets ADD COLUMN include_paths JSON DEFAULT '[]'"
            )
        )

    if "exclude_paths" not in targets_columns:
        await conn.execute(
            text(
                "ALTER TABLE backup_targets ADD COLUMN exclude_paths JSON DEFAULT '[]'"
            )
        )

    if "per_volume_rules" not in targets_columns:
        await conn.execute(
            text(
                "ALTER TABLE backup_targets ADD COLUMN per_volume_rules JSON "
                "DEFAULT '{}'"
            )
        )

    if "sync_to_remote" not in targets_columns:
        await conn.execute(
            text(
                "ALTER TABLE backup_targets ADD COLUMN sync_to_remote BOOLEAN DEFAULT 0"
            )
        )

    if "remote_storage_ids" not in targets_columns:
        await conn.execute(
            text(
                "ALTER TABLE backup_targets ADD COLUMN remote_storage_ids "
                "JSON DEFAULT '[]'"
            )
        )

    if "delete_local_after_sync" not in targets_columns:
        await conn.execute(
            text(
                "ALTER TABLE backup_targets ADD COLUMN "
                "delete_local_after_sync BOOLEAN DEFAULT 0"
            )
        )

    # Data migration: fix rows where remote_storage_ids is set but
    # sync_to_remote is still False (can happen on older databases).
    await conn.execute(
        text(
            "UPDATE backup_targets SET sync_to_remote = 1 "
            "WHERE remote_storage_ids IS NOT NULL "
            "AND remote_storage_ids != '[]' "
            "AND remote_storage_ids != '' "
            "AND sync_to_remote = 0"
        )
    )

    # Check and add missing columns to retention_policies table
    result = await conn.execute(text("PRAGMA table_info(retention_policies)"))
    retention_columns = {row[1] for row in result.fetchall()}

    if "keep_last" not in retention_columns:
        await conn.execute(
            text(
                "ALTER TABLE retention_policies ADD COLUMN keep_last INTEGER DEFAULT 0"
            )
        )


async def _repair_catalog_orphans(conn):
    """Retain unassignable metadata, without ever touching an archive or provider."""
    import json

    await conn.execute(
        text(
            "CREATE TABLE IF NOT EXISTS catalog_quarantine ("
            "table_name TEXT NOT NULL, row_id INTEGER NOT NULL, row_json TEXT NOT NULL, "
            "reason TEXT NOT NULL, PRIMARY KEY (table_name, row_id))"
        )
    )
    queries = {
        "backup_logs": "SELECT * FROM backup_logs WHERE backup_id NOT IN (SELECT id FROM backups)",
        "backup_storage_syncs": (
            "SELECT * FROM backup_storage_syncs WHERE backup_id NOT IN (SELECT id FROM backups) "
            "OR storage_id NOT IN (SELECT id FROM remote_storages)"
        ),
    }
    for table_name, query in queries.items():
        rows = (await conn.execute(text(query))).mappings().all()
        for row in rows:
            await conn.execute(
                text(
                    "INSERT OR IGNORE INTO catalog_quarantine (table_name, row_id, row_json, reason) "
                    "VALUES (:table, :id, :data, 'missing parent reference')"
                ),
                {"table": table_name, "id": row["id"], "data": json.dumps(dict(row))},
            )
            # Table names come only from the fixed catalog mapping above.
            await conn.execute(
                text(f"DELETE FROM {table_name} WHERE id = :id"), {"id": row["id"]}
            )
    violations = (await conn.execute(text("PRAGMA foreign_key_check"))).all()
    if violations:
        raise RuntimeError(
            "Unresolved catalog foreign-key violations; refusing startup"
        )


async def _migrate_plaintext_credentials():
    """Encrypt any plaintext credentials in remote_storage rows."""
    from app.credential_encryption import FERNET_PREFIX, encrypt_value

    from sqlalchemy import select as sa_select

    async with async_session() as session:
        result = await session.execute(sa_select(RemoteStorage))
        storages = result.scalars().all()
        for storage in storages:
            changed = False
            for col in ("password", "s3_secret_key", "s3_access_key"):
                val = getattr(storage, col, None)
                if val and not val.startswith(FERNET_PREFIX):
                    setattr(storage, col, encrypt_value(val))
                    changed = True
            if changed:
                session.add(storage)
        await session.commit()


async def _recover_orphaned_backups():
    """Terminally fail interrupted/queued jobs and remove half-written files.

    A container crash, OOM kill, or hard restart leaves ``BackupStatus.RUNNING``
    rows behind that will never complete. The frontend would show them as
    perpetually in-progress and the per-target lock logic could race against
    the dead task on restart. Also clean up any temporary artifacts
    (``*.tmp`` from atomic encryption, ``*.partial`` from interrupted writes)
    that are now orphaned.
    """
    import logging
    import os
    from pathlib import Path

    from sqlalchemy import update as sa_update

    from app.config import settings as app_settings

    log = logging.getLogger(__name__)

    async with async_session() as session:
        result = await session.execute(
            sa_update(Backup)
            .where(Backup.status.in_([BackupStatus.RUNNING, BackupStatus.PENDING]))
            .values(
                status=BackupStatus.FAILED,
                error_message=(
                    "Interrupted by restart; accepted job terminally failed. "
                    "Legacy workload state may be unknown: verify workloads manually and retry."
                ),
                completed_at=_utcnow(),
            )
        )
        if result.rowcount:
            log.warning(
                "Resolved %d interrupted or pending backup(s) as FAILED",
                result.rowcount,
            )
        await session.commit()

    backup_root = Path(app_settings.BACKUP_BASE_PATH)
    if not backup_root.is_dir():
        return

    removed = 0
    try:
        for dirpath, _dirnames, filenames in os.walk(backup_root):
            for name in filenames:
                if name.endswith(".tmp") or name.endswith(".partial"):
                    p = Path(dirpath) / name
                    try:
                        p.unlink()
                        removed += 1
                    except OSError as exc:
                        log.warning("Could not remove orphan %s: %s", p, exc)
    except OSError as exc:
        log.warning("Walking %s for orphan cleanup failed: %s", backup_root, exc)

    if removed:
        log.info("Removed %d orphaned temporary backup file(s)", removed)


async def has_remote_copies(session, backup_id: int) -> bool:
    """Keep recovery identity while a catalogued remote object may still exist."""
    from sqlalchemy import select

    return (
        await session.execute(
            select(BackupStorageSync.id)
            .where(
                BackupStorageSync.backup_id == backup_id,
                BackupStorageSync.remote_path.isnot(None),
                BackupStorageSync.remote_path != "",
            )
            .limit(1)
        )
    ).first() is not None


async def get_session() -> AsyncSession:
    """Get database session."""
    session = async_session()
    try:
        yield session
    finally:
        # Keep scopes out of the dependency yield; SQLAlchemy joins repeated edges.
        with CancelScope(shield=True):
            await session.__aexit__(None, None, None)


# Alias for backwards compatibility
get_db = get_session
