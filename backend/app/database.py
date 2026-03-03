"""
Database configuration and models using SQLAlchemy with async support.
"""

import enum
from datetime import datetime, timezone

from sqlalchemy import JSON, Boolean, Column, DateTime
from sqlalchemy import Enum as SQLEnum
from sqlalchemy import ForeignKey, Integer, String, Text, text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from sqlalchemy.orm import DeclarativeBase, relationship

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
    backup_id = Column(Integer, ForeignKey("backups.id"), nullable=False)
    storage_id = Column(Integer, ForeignKey("remote_storages.id"), nullable=False)
    remote_path = Column(String(1024), nullable=True)
    synced_at = Column(DateTime, nullable=True)
    sync_status = Column(
        String(50), default="pending"
    )  # pending, syncing, completed, failed
    error_message = Column(Text, nullable=True)

    backup = relationship("Backup")
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


# Database engine and session
engine = create_async_engine(
    settings.DATABASE_URL,
    echo=False,
)

async_session = async_sessionmaker(
    engine,
    class_=AsyncSession,
    expire_on_commit=False,
)


async def init_db():
    """Initialize database tables."""
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)

    # Run migrations for existing databases
    await run_migrations()

    # Encrypt any plaintext credentials in remote_storage
    await _migrate_plaintext_credentials()

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
    """Run database migrations for existing databases.

    This handles adding new columns to existing tables that
    create_all doesn't update.
    """
    async with engine.begin() as conn:
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
                text(
                    "ALTER TABLE backup_targets ADD COLUMN retention_policy_id INTEGER"
                )
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
                    "ALTER TABLE backup_targets ADD COLUMN include_paths JSON "
                    "DEFAULT '[]'"
                )
            )

        if "exclude_paths" not in targets_columns:
            await conn.execute(
                text(
                    "ALTER TABLE backup_targets ADD COLUMN exclude_paths JSON "
                    "DEFAULT '[]'"
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
                    "ALTER TABLE backup_targets ADD COLUMN sync_to_remote "
                    "BOOLEAN DEFAULT 0"
                )
            )

        if "remote_storage_ids" not in targets_columns:
            await conn.execute(
                text(
                    "ALTER TABLE backup_targets ADD COLUMN remote_storage_ids "
                    "JSON DEFAULT '[]'"
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
                    "ALTER TABLE retention_policies ADD COLUMN keep_last INTEGER "
                    "DEFAULT 0"
                )
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


async def get_session() -> AsyncSession:
    """Get database session."""
    async with async_session() as session:
        yield session


# Alias for backwards compatibility
get_db = get_session
