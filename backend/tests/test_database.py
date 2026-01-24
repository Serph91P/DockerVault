"""
Tests for database module.
"""

import pytest
from sqlalchemy import select
from sqlalchemy.exc import IntegrityError
from datetime import datetime

from app.database import (
    Backup, BackupTarget, BackupSchedule, RemoteStorage,
    BackupStatus, BackupType
)


@pytest.mark.asyncio
class TestDatabaseModels:
    """Test database models and relationships."""

    async def test_backup_target_creation(self, db_session):
        """Test backup target creation."""
        target = BackupTarget(
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True
        )
        
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)
        
        assert target.id is not None
        assert target.name == "test-volume"
        assert target.target_type == "volume"
        assert target.enabled is True
        assert target.created_at is not None

    async def test_backup_target_with_path(self, db_session):
        """Test backup target with host path."""
        target = BackupTarget(
            name="path-target",
            target_type="path",
            host_path="/data/backup",
            enabled=True
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)
        
        assert target.host_path == "/data/backup"
        assert target.target_type == "path"

    async def test_backup_creation(self, db_session):
        """Test backup creation with target relationship."""
        # Create target first
        target = BackupTarget(
            name="test-target",
            target_type="volume",
            volume_name="test-volume",
            enabled=True
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)
        
        # Create backup
        backup = Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.PENDING,
            backup_metadata={"test": "data"},
            file_path="/backups/test.tar.gz",
            file_size=1024,
            checksum="abc123"
        )
        
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)
        
        assert backup.id is not None
        assert backup.target_id == target.id
        assert backup.backup_type == BackupType.FULL
        assert backup.status == BackupStatus.PENDING
        assert backup.backup_metadata == {"test": "data"}
        assert backup.created_at is not None

    async def test_backup_target_relationship(self, db_session):
        """Test backup-target relationship loading."""
        # Create target
        target = BackupTarget(
            name="relationship-test",
            target_type="volume",
            volume_name="test-volume",
            enabled=True
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)
        
        # Create multiple backups for target
        backup1 = Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.COMPLETED
        )
        backup2 = Backup(
            target_id=target.id,
            backup_type=BackupType.INCREMENTAL,
            status=BackupStatus.PENDING
        )
        
        db_session.add_all([backup1, backup2])
        await db_session.commit()
        
        # Query target with backups
        result = await db_session.execute(
            select(BackupTarget).where(BackupTarget.id == target.id)
        )
        loaded_target = result.scalar_one()
        
        # Note: In real app, you'd use relationship loading
        # This tests the foreign key relationship exists
        backup_result = await db_session.execute(
            select(Backup).where(Backup.target_id == target.id)
        )
        target_backups = backup_result.scalars().all()
        
        assert len(target_backups) == 2
        assert all(b.target_id == target.id for b in target_backups)

    async def test_backup_schedule_creation(self, db_session):
        """Test backup schedule creation."""
        # Create target first
        target = BackupTarget(
            name="scheduled-target",
            target_type="volume",
            volume_name="test-volume",
            enabled=True
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)
        
        # Create schedule
        schedule = BackupSchedule(
            target_id=target.id,
            cron_expression="0 2 * * *",  # Daily at 2 AM
            enabled=True
        )
        
        db_session.add(schedule)
        await db_session.commit()
        await db_session.refresh(schedule)
        
        assert schedule.id is not None
        assert schedule.target_id == target.id
        assert schedule.cron_expression == "0 2 * * *"
        assert schedule.enabled is True

    async def test_remote_storage_creation(self, db_session):
        """Test remote storage configuration."""
        storage = RemoteStorage(
            name="S3 Storage",
            storage_type="s3",
            s3_bucket="my-backup-bucket",
            s3_region="us-east-1",
            s3_access_key="AKIAIOSFODNN7EXAMPLE",
            s3_secret_key="wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            enabled=True
        )
        
        db_session.add(storage)
        await db_session.commit()
        await db_session.refresh(storage)
        
        assert storage.id is not None
        assert storage.name == "S3 Storage"
        assert storage.storage_type == "s3"
        assert storage.s3_bucket == "my-backup-bucket"
        assert storage.enabled is True

    async def test_backup_status_transitions(self, db_session):
        """Test backup status transitions."""
        # Create target
        target = BackupTarget(
            name="status-test",
            target_type="volume",
            volume_name="test-volume",
            enabled=True
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)
        
        # Create backup
        backup = Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.PENDING
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)
        
        # Update status to running
        backup.status = BackupStatus.RUNNING
        backup.started_at = datetime.utcnow()
        await db_session.commit()
        
        # Update status to completed
        backup.status = BackupStatus.COMPLETED
        backup.completed_at = datetime.utcnow()
        backup.file_size = 2048
        backup.checksum = "def456"
        await db_session.commit()
        
        await db_session.refresh(backup)
        
        assert backup.status == BackupStatus.COMPLETED
        assert backup.started_at is not None
        assert backup.completed_at is not None
        assert backup.file_size == 2048
        assert backup.checksum == "def456"

    async def test_database_connection_error_handling(self, db_session):
        """Test database connection error handling."""
        # This test checks that database operations handle errors gracefully
        # In real scenarios, this might involve network issues, disk full, etc.
        
        # Create a backup with invalid target_id (foreign key constraint)
        backup = Backup(
            target_id=99999,  # Non-existent target
            backup_type=BackupType.FULL,
            status=BackupStatus.PENDING
        )
        
        db_session.add(backup)
        
        with pytest.raises(IntegrityError):
            await db_session.commit()

    async def test_metadata_json_field(self, db_session):
        """Test JSON metadata field functionality."""
        target = BackupTarget(
            name="json-test",
            target_type="volume",
            volume_name="test-volume",
            enabled=True
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)
        
        # Test complex metadata structure
        complex_metadata = {
            "containers_stopped": ["container1", "container2"],
            "backup_settings": {
                "compression": "gzip",
                "encryption": False,
                "exclude_patterns": ["*.log", "tmp/*"]
            },
            "performance": {
                "start_time": "2024-01-01T10:00:00Z",
                "duration_seconds": 120,
                "bytes_processed": 1048576
            }
        }
        
        backup = Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.COMPLETED,
            backup_metadata=complex_metadata
        )
        
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)
        
        assert backup.backup_metadata["containers_stopped"] == ["container1", "container2"]
        assert backup.backup_metadata["backup_settings"]["compression"] == "gzip"
        assert backup.backup_metadata["performance"]["bytes_processed"] == 1048576

    async def test_query_performance_indexes(self, db_session):
        """Test that database indexes work correctly for common queries."""
        # Create multiple targets and backups
        targets = []
        for i in range(10):
            target = BackupTarget(
                name=f"target-{i}",
                target_type="volume",
                volume_name=f"volume-{i}",
                enabled=True
            )
            targets.append(target)
            db_session.add(target)
        
        await db_session.commit()
        
        # Create many backups
        for target in targets:
            await db_session.refresh(target)
            for j in range(5):
                backup = Backup(
                    target_id=target.id,
                    backup_type=BackupType.FULL,
                    status=BackupStatus.COMPLETED if j < 3 else BackupStatus.FAILED
                )
                db_session.add(backup)
        
        await db_session.commit()
        
        # Query by status (should use index)
        completed_result = await db_session.execute(
            select(Backup).where(Backup.status == BackupStatus.COMPLETED)
        )
        completed_backups = completed_result.scalars().all()
        assert len(completed_backups) == 30  # 10 targets * 3 completed each
        
        # Query by target_id (should use foreign key index)
        target_result = await db_session.execute(
            select(Backup).where(Backup.target_id == targets[0].id)
        )
        target_backups = target_result.scalars().all()
        assert len(target_backups) == 5
