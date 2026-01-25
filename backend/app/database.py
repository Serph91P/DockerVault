"""
Database configuration and models using SQLAlchemy with async support.
"""

from datetime import datetime
from sqlalchemy import Column, Integer, String, DateTime, Boolean, ForeignKey, JSON, Enum as SQLEnum, Text
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker
from sqlalchemy.orm import DeclarativeBase, relationship
import enum

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
    created_at = Column(DateTime, default=datetime.utcnow)
    updated_at = Column(DateTime, default=datetime.utcnow, onupdate=datetime.utcnow)


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

    # Scheduling
    schedule_cron = Column(String(100), nullable=True)  # Cron expression
    enabled = Column(Boolean, default=True)

    # Retention
    retention_policy_id = Column(Integer, ForeignKey("retention_policies.id"), nullable=True)
    retention_policy = relationship("RetentionPolicy")

    # Dependencies (other targets that must be stopped before backup)
    dependencies = Column(JSON, default=list)  # List of container names

    # Pre/Post hooks
    pre_backup_command = Column(Text, nullable=True)
    post_backup_command = Column(Text, nullable=True)

    # Settings
    stop_container = Column(Boolean, default=True)  # Stop container during backup
    compression_enabled = Column(Boolean, default=True)

    # Remote storage sync
    sync_to_remote = Column(Boolean, default=False)
    remote_storage_ids = Column(JSON, default=list)  # List of storage IDs to sync to

    created_at = Column(DateTime, default=datetime.utcnow)
    updated_at = Column(DateTime, default=datetime.utcnow, onupdate=datetime.utcnow)

    backups = relationship("Backup", back_populates="target")


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

    # Timing
    started_at = Column(DateTime, nullable=True)
    completed_at = Column(DateTime, nullable=True)
    duration_seconds = Column(Integer, nullable=True)

    # Error handling
    error_message = Column(Text, nullable=True)
    retry_count = Column(Integer, default=0)

    # Extra data
    backup_metadata = Column(JSON, default=dict)

    created_at = Column(DateTime, default=datetime.utcnow)


class BackupSchedule(Base):
    """Scheduled backup jobs."""
    __tablename__ = "backup_schedules"

    id = Column(Integer, primary_key=True, index=True)
    target_id = Column(Integer, ForeignKey("backup_targets.id"), nullable=False)
    target = relationship("BackupTarget")

    cron_expression = Column(String(100), nullable=False)
    next_run = Column(DateTime, nullable=True)
    last_run = Column(DateTime, nullable=True)
    enabled = Column(Boolean, default=True)

    created_at = Column(DateTime, default=datetime.utcnow)
    updated_at = Column(DateTime, default=datetime.utcnow, onupdate=datetime.utcnow)


class RemoteStorage(Base):
    """Remote storage configuration for off-site backups."""
    __tablename__ = "remote_storages"

    id = Column(Integer, primary_key=True, index=True)
    name = Column(String(255), unique=True, nullable=False)
    storage_type = Column(String(50), nullable=False)  # local, ssh, webdav, s3, ftp, rclone
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

    created_at = Column(DateTime, default=datetime.utcnow)
    updated_at = Column(DateTime, default=datetime.utcnow, onupdate=datetime.utcnow)


class BackupStorageSync(Base):
    """Track which backups are synced to which remote storages."""
    __tablename__ = "backup_storage_syncs"

    id = Column(Integer, primary_key=True, index=True)
    backup_id = Column(Integer, ForeignKey("backups.id"), nullable=False)
    storage_id = Column(Integer, ForeignKey("remote_storages.id"), nullable=False)
    remote_path = Column(String(1024), nullable=True)
    synced_at = Column(DateTime, nullable=True)
    sync_status = Column(String(50), default="pending")  # pending, syncing, completed, failed
    error_message = Column(Text, nullable=True)

    backup = relationship("Backup")
    storage = relationship("RemoteStorage")


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


async def get_session() -> AsyncSession:
    """Get database session."""
    async with async_session() as session:
        yield session


# Alias for backwards compatibility
get_db = get_session
