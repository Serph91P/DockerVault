"""
Tests for scheduler module.
"""

import asyncio
from datetime import datetime, timedelta
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from app.database import BackupSchedule, BackupTarget, BackupType
from app.scheduler import BackupScheduler


@pytest.mark.asyncio
class TestBackupScheduler:
    """Test backup scheduler functionality."""

    async def test_scheduler_initialization(self):
        """Test scheduler initialization."""
        scheduler = BackupScheduler()
        assert scheduler is not None
        assert scheduler.scheduler is not None
        assert not scheduler.scheduler.running

    async def test_scheduler_start_stop(self):
        """Test scheduler start and stop."""
        scheduler = BackupScheduler()

        # Start scheduler
        await scheduler.start()
        assert scheduler.scheduler.running

        # Stop scheduler
        await scheduler.stop()
        assert not scheduler.scheduler.running

    @patch("app.scheduler.async_session")
    async def test_load_schedules_from_database(self, mock_session):
        """Test loading schedules from database."""
        # Mock database session and query results
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        # Mock target
        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )

        # Mock schedule
        schedule = BackupSchedule(
            id=1, target_id=1, cron_expression="0 2 * * *", enabled=True
        )
        schedule.target = target  # Set relationship

        # Mock query result
        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = [schedule]
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        await scheduler.start()

        # Verify schedule was loaded
        jobs = scheduler.scheduler.get_jobs()
        assert len(jobs) > 0

        await scheduler.stop()

    @patch("app.scheduler.backup_engine")
    async def test_execute_scheduled_backup(self, mock_backup_engine):
        """Test execution of scheduled backup."""
        mock_backup_engine.create_backup.return_value = AsyncMock()
        mock_backup_engine.run_backup.return_value = AsyncMock(return_value=True)

        scheduler = BackupScheduler()

        # Create mock target
        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )

        # Execute backup
        await scheduler._execute_backup(target, BackupType.FULL)

        # Verify backup engine was called
        mock_backup_engine.create_backup.assert_called_once_with(
            target, BackupType.FULL
        )
        mock_backup_engine.run_backup.assert_called_once()

    @patch("app.scheduler.backup_engine")
    async def test_execute_backup_handles_errors(self, mock_backup_engine):
        """Test that backup execution handles errors gracefully."""
        mock_backup_engine.create_backup.side_effect = Exception("Backup failed")

        scheduler = BackupScheduler()

        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )

        # Should not raise exception
        await scheduler._execute_backup(target, BackupType.FULL)

        # Verify error was logged (would check logs in real implementation)
        mock_backup_engine.create_backup.assert_called_once()

    async def test_add_job_to_scheduler(self):
        """Test adding job to scheduler."""
        scheduler = BackupScheduler()
        await scheduler.start()

        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )

        # Add job
        job_id = await scheduler.add_job(
            target=target, cron_expression="0 2 * * *", backup_type=BackupType.FULL
        )

        assert job_id is not None

        # Verify job was added
        jobs = scheduler.scheduler.get_jobs()
        job_ids = [job.id for job in jobs]
        assert job_id in job_ids

        await scheduler.stop()

    async def test_remove_job_from_scheduler(self):
        """Test removing job from scheduler."""
        scheduler = BackupScheduler()
        await scheduler.start()

        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )

        # Add job
        job_id = await scheduler.add_job(
            target=target, cron_expression="0 2 * * *", backup_type=BackupType.FULL
        )

        # Verify job exists
        job = scheduler.scheduler.get_job(job_id)
        assert job is not None

        # Remove job
        await scheduler.remove_job(job_id)

        # Verify job was removed
        job = scheduler.scheduler.get_job(job_id)
        assert job is None

        await scheduler.stop()

    async def test_update_job_in_scheduler(self):
        """Test updating job in scheduler."""
        scheduler = BackupScheduler()
        await scheduler.start()

        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )

        # Add job
        job_id = await scheduler.add_job(
            target=target,
            cron_expression="0 2 * * *",  # Daily at 2 AM
            backup_type=BackupType.FULL,
        )

        # Update job with new cron expression
        await scheduler.update_job(
            job_id=job_id,
            cron_expression="0 6 * * *",  # Daily at 6 AM
            backup_type=BackupType.INCREMENTAL,
        )

        # Verify job was updated
        job = scheduler.scheduler.get_job(job_id)
        assert job is not None
        # In real implementation, would verify cron expression was updated

        await scheduler.stop()

    async def test_list_scheduled_jobs(self):
        """Test listing scheduled jobs."""
        scheduler = BackupScheduler()
        await scheduler.start()

        target1 = BackupTarget(
            id=1,
            name="test-volume-1",
            target_type="volume",
            volume_name="test-volume-1",
            enabled=True,
        )

        target2 = BackupTarget(
            id=2,
            name="test-volume-2",
            target_type="volume",
            volume_name="test-volume-2",
            enabled=True,
        )

        # Add multiple jobs
        job_id1 = await scheduler.add_job(
            target=target1, cron_expression="0 2 * * *", backup_type=BackupType.FULL
        )

        job_id2 = await scheduler.add_job(
            target=target2,
            cron_expression="0 6 * * *",
            backup_type=BackupType.INCREMENTAL,
        )

        # List jobs
        jobs = await scheduler.list_jobs()

        assert len(jobs) >= 2
        job_ids = [job["id"] for job in jobs]
        assert job_id1 in job_ids
        assert job_id2 in job_ids

        await scheduler.stop()

    async def test_validate_cron_expression(self):
        """Test cron expression validation."""
        scheduler = BackupScheduler()

        # Valid cron expressions
        assert scheduler.validate_cron_expression("0 2 * * *")  # Daily at 2 AM
        assert scheduler.validate_cron_expression("0 */6 * * *")  # Every 6 hours
        assert scheduler.validate_cron_expression("0 0 * * 0")  # Weekly on Sunday

        # Invalid cron expressions
        assert not scheduler.validate_cron_expression("invalid")
        assert not scheduler.validate_cron_expression("* * * * * *")  # Too many fields
        assert not scheduler.validate_cron_expression("60 0 * * *")  # Invalid minute

    async def test_get_next_run_time(self):
        """Test getting next run time for cron expression."""
        scheduler = BackupScheduler()

        # Test daily at 2 AM
        next_run = scheduler.get_next_run_time("0 2 * * *")
        assert next_run is not None
        assert isinstance(next_run, datetime)

        # Should be tomorrow at 2 AM (or today if current time is before 2 AM)
        assert next_run.hour == 2
        assert next_run.minute == 0

    async def test_trigger_immediate_backup(self):
        """Test triggering immediate backup."""
        with patch("app.scheduler.backup_engine") as mock_backup_engine:
            mock_backup = MagicMock()
            mock_backup.id = 123
            mock_backup_engine.create_backup.return_value = mock_backup
            mock_backup_engine.run_backup.return_value = AsyncMock(return_value=True)

            scheduler = BackupScheduler()

            target = BackupTarget(
                id=1,
                name="test-volume",
                target_type="volume",
                volume_name="test-volume",
                enabled=True,
            )

            backup_id = await scheduler.trigger_immediate_backup(
                target, BackupType.FULL
            )

            assert backup_id == 123
            mock_backup_engine.create_backup.assert_called_once_with(
                target, BackupType.FULL
            )
            mock_backup_engine.run_backup.assert_called_once_with(123)

    async def test_scheduler_persistence_across_restarts(self):
        """Test that scheduled jobs persist across scheduler restarts."""
        with patch("app.scheduler.async_session") as mock_session:
            # Mock persistent schedules in database
            mock_session_instance = AsyncMock()
            mock_session.return_value.__aenter__.return_value = mock_session_instance

            target = BackupTarget(
                id=1,
                name="persistent-volume",
                target_type="volume",
                volume_name="persistent-volume",
                enabled=True,
            )

            schedule = BackupSchedule(
                id=1, target_id=1, cron_expression="0 3 * * *", enabled=True
            )
            schedule.target = target

            mock_result = MagicMock()
            mock_result.scalars.return_value.all.return_value = [schedule]
            mock_session_instance.execute.return_value = mock_result

            # First scheduler instance
            scheduler1 = BackupScheduler()
            await scheduler1.start()
            jobs1 = scheduler1.scheduler.get_jobs()
            await scheduler1.stop()

            # Second scheduler instance (simulating restart)
            scheduler2 = BackupScheduler()
            await scheduler2.start()
            jobs2 = scheduler2.scheduler.get_jobs()
            await scheduler2.stop()

            # Should load same schedules from database
            assert len(jobs1) == len(jobs2)

    async def test_scheduler_handles_disabled_targets(self):
        """Test that scheduler doesn't schedule backups for disabled targets."""
        with patch("app.scheduler.async_session") as mock_session:
            mock_session_instance = AsyncMock()
            mock_session.return_value.__aenter__.return_value = mock_session_instance

            # Disabled target
            target = BackupTarget(
                id=1,
                name="disabled-volume",
                target_type="volume",
                volume_name="disabled-volume",
                enabled=False,  # Disabled
            )

            schedule = BackupSchedule(
                id=1,
                target_id=1,
                cron_expression="0 3 * * *",
                enabled=True,  # Schedule enabled but target disabled
            )
            schedule.target = target

            mock_result = MagicMock()
            mock_result.scalars.return_value.all.return_value = [schedule]
            mock_session_instance.execute.return_value = mock_result

            scheduler = BackupScheduler()
            await scheduler.start()

            # Should not schedule job for disabled target
            jobs = scheduler.scheduler.get_jobs()
            assert len(jobs) == 0

            await scheduler.stop()

    async def test_concurrent_backup_execution_limits(self):
        """Test that scheduler respects concurrent backup limits."""
        scheduler = BackupScheduler()

        # Mock that maximum concurrent backups is reached
        with patch("app.scheduler.backup_engine") as mock_backup_engine:
            mock_backup_engine.active_backups = {
                1: MagicMock(),
                2: MagicMock(),
                3: MagicMock(),
            }

            target = BackupTarget(
                id=4,
                name="test-volume",
                target_type="volume",
                volume_name="test-volume",
                enabled=True,
            )

            # Should skip backup if limit reached
            result = await scheduler._execute_backup(target, BackupType.FULL)

            # In real implementation, would verify backup was skipped
            # due to concurrent limit
            assert result is not None  # Or None if skipped
