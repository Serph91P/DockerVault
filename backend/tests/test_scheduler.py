"""
Tests for scheduler module.
"""

from datetime import datetime
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from app.database import BackupTarget
from app.scheduler import BackupScheduler


@pytest.mark.asyncio
class TestBackupScheduler:
    """Test backup scheduler functionality."""

    def test_scheduler_initialization(self):
        """Test scheduler initialization."""
        scheduler = BackupScheduler()
        assert scheduler is not None
        assert scheduler.scheduler is not None
        assert scheduler._running is False

    @patch("app.scheduler.async_session")
    async def test_scheduler_start_stop(self, mock_session):
        """Test scheduler start and stop."""
        # Mock empty database query
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()

        # Start scheduler
        await scheduler.start()
        assert scheduler._running is True

        # Stop scheduler
        await scheduler.stop()
        assert scheduler._running is False

    @patch("app.scheduler.async_session")
    async def test_start_twice_only_starts_once(self, mock_session):
        """Test that starting twice doesn't cause issues."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()

        await scheduler.start()
        await scheduler.start()  # Should not throw

        assert scheduler._running is True
        await scheduler.stop()

    @patch("app.scheduler.async_session")
    async def test_load_schedules_from_database(self, mock_session):
        """Test loading schedules from database."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        # Mock target with schedule
        target = MagicMock(spec=BackupTarget)
        target.id = 1
        target.name = "test-volume"
        target.target_type = "volume"
        target.volume_name = "test-volume"
        target.enabled = True
        target.schedule_cron = "0 2 * * *"
        target.schedule = None  # No Schedule entity, use legacy schedule_cron

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = [target]
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        await scheduler.start()

        # Verify schedule was loaded
        jobs = scheduler.scheduler.get_jobs()
        # Should have retention_cleanup + the backup job
        job_ids = [job.id for job in jobs]
        assert "backup_target_1" in job_ids

        await scheduler.stop()

    @patch("app.scheduler.async_session")
    async def test_add_schedule(self, mock_session):
        """Test adding a schedule."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        await scheduler.start()

        target = MagicMock(spec=BackupTarget)
        target.id = 1
        target.name = "test-volume"
        target.schedule_cron = "0 2 * * *"
        target.schedule = None  # No Schedule entity, use legacy schedule_cron

        success = await scheduler.add_schedule(target)
        assert success is True

        # Verify job was added
        job = scheduler.scheduler.get_job("backup_target_1")
        assert job is not None

        await scheduler.stop()

    @patch("app.scheduler.async_session")
    async def test_add_schedule_no_cron(self, mock_session):
        """Test adding schedule without cron expression returns False."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        await scheduler.start()

        target = MagicMock(spec=BackupTarget)
        target.id = 1
        target.name = "test-volume"
        target.schedule_cron = None
        target.schedule = None  # No Schedule entity

        success = await scheduler.add_schedule(target)
        assert success is False

        await scheduler.stop()

    @patch("app.scheduler.async_session")
    async def test_remove_schedule(self, mock_session):
        """Test removing a schedule."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        await scheduler.start()

        target = MagicMock(spec=BackupTarget)
        target.id = 1
        target.name = "test-volume"
        target.schedule_cron = "0 2 * * *"
        target.schedule = None  # No Schedule entity, use legacy schedule_cron

        # Add schedule
        await scheduler.add_schedule(target)
        assert scheduler.scheduler.get_job("backup_target_1") is not None

        # Remove schedule
        success = await scheduler.remove_schedule(1)
        assert success is True
        assert scheduler.scheduler.get_job("backup_target_1") is None

        await scheduler.stop()

    @patch("app.scheduler.async_session")
    async def test_remove_nonexistent_schedule(self, mock_session):
        """Test removing a schedule that doesn't exist."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        await scheduler.start()

        # Try to remove non-existent schedule
        success = await scheduler.remove_schedule(999)
        # Should return False (warns but doesn't raise)
        assert success is False

        await scheduler.stop()

    def test_get_next_run(self):
        """Test getting next run time for cron expression."""
        scheduler = BackupScheduler()

        # Test daily at 2 AM
        next_run = scheduler.get_next_run("0 2 * * *")
        assert next_run is not None
        assert isinstance(next_run, datetime)
        assert next_run.hour == 2
        assert next_run.minute == 0

    def test_get_next_run_with_base_time(self):
        """Test getting next run with custom base time."""
        scheduler = BackupScheduler()

        base_time = datetime(2024, 1, 15, 10, 0, 0)
        next_run = scheduler.get_next_run("0 12 * * *", base_time)

        assert next_run.hour == 12
        assert next_run.minute == 0
        # Should be same day since 12:00 is after 10:00
        assert next_run.day == 15

    @patch("app.scheduler.async_session")
    async def test_get_scheduled_jobs(self, mock_session):
        """Test getting all scheduled jobs."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        await scheduler.start()

        # Add some schedules
        target1 = MagicMock(spec=BackupTarget)
        target1.id = 1
        target1.name = "volume-1"
        target1.schedule_cron = "0 2 * * *"
        target1.schedule = None  # No Schedule entity

        target2 = MagicMock(spec=BackupTarget)
        target2.id = 2
        target2.name = "volume-2"
        target2.schedule_cron = "0 6 * * *"
        target2.schedule = None  # No Schedule entity

        await scheduler.add_schedule(target1)
        await scheduler.add_schedule(target2)

        jobs = scheduler.get_scheduled_jobs()
        assert len(jobs) == 2

        job_ids = [job["id"] for job in jobs]
        assert "backup_target_1" in job_ids
        assert "backup_target_2" in job_ids

        await scheduler.stop()

    @patch("app.scheduler.async_session")
    @patch("app.scheduler.backup_engine")
    async def test_trigger_backup_now(self, mock_backup_engine, mock_session):
        """Test triggering immediate backup."""
        # Setup mock session
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        # Mock target from database
        target = MagicMock(spec=BackupTarget)
        target.id = 1
        target.name = "test-volume"
        target.enabled = True

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = target
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        # Mock backup engine
        mock_backup = MagicMock()
        mock_backup.id = 123
        mock_backup_engine.create_backup = AsyncMock(return_value=mock_backup)
        mock_backup_engine.run_backup = AsyncMock(return_value=True)

        scheduler = BackupScheduler()

        success = await scheduler.trigger_backup_now(1)
        assert success is True

        mock_backup_engine.create_backup.assert_called_once()

    @patch("app.scheduler.async_session")
    @patch("app.scheduler.backup_engine")
    async def test_trigger_backup_now_nonexistent_target(
        self, mock_backup_engine, mock_session
    ):
        """Test triggering backup for non-existent target."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = None
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()

        success = await scheduler.trigger_backup_now(999)
        assert success is False

        mock_backup_engine.create_backup.assert_not_called()

    @patch("app.scheduler.async_session")
    async def test_estimate_backup_window(self, mock_session):
        """Test estimating backup window."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        await scheduler.start()

        # Estimate 1 hour backup
        estimate = scheduler.estimate_backup_window(
            cron_expr="0 2 * * *",
            estimated_duration_seconds=3600,
        )

        assert "start_time" in estimate
        assert "estimated_end_time" in estimate
        assert "duration_seconds" in estimate
        assert estimate["duration_seconds"] == 3600
        assert "conflicts" in estimate
        assert isinstance(estimate["conflicts"], list)

        await scheduler.stop()

    @patch("app.scheduler.async_session")
    @patch("app.scheduler.backup_engine")
    @patch("app.scheduler.retention_manager")
    async def test_run_scheduled_backup_success(
        self, mock_retention, mock_backup_engine, mock_session
    ):
        """Test running a scheduled backup."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        # Mock target from database
        target = MagicMock(spec=BackupTarget)
        target.id = 1
        target.name = "test-volume"
        target.enabled = True
        target.schedule_cron = "0 2 * * *"

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = target
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        # Mock backup engine
        mock_backup = MagicMock()
        mock_backup.id = 123
        mock_backup_engine.create_backup = AsyncMock(return_value=mock_backup)
        mock_backup_engine.run_backup = AsyncMock(return_value=True)

        # Mock retention manager
        mock_retention.apply_retention = AsyncMock()

        scheduler = BackupScheduler()
        await scheduler._run_scheduled_backup(1)

        mock_backup_engine.create_backup.assert_called_once()
        mock_backup_engine.run_backup.assert_called_once_with(123)
        mock_retention.apply_retention.assert_called_once_with(1)

    @patch("app.scheduler.async_session")
    async def test_run_scheduled_backup_target_not_found(self, mock_session):
        """Test scheduled backup with non-existent target."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = None
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        # Should not raise exception
        await scheduler._run_scheduled_backup(999)

    @patch("app.scheduler.async_session")
    async def test_run_scheduled_backup_disabled_target(self, mock_session):
        """Test scheduled backup skips disabled targets."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        # Mock disabled target
        target = MagicMock(spec=BackupTarget)
        target.id = 1
        target.name = "test-volume"
        target.enabled = False  # Disabled

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = target
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        # Should return early without running backup
        await scheduler._run_scheduled_backup(1)


@pytest.mark.asyncio
class TestSchedulerEdgeCases:
    """Test scheduler edge cases."""

    def test_invalid_cron_expression_in_get_next_run(self):
        """Test handling invalid cron expression."""
        scheduler = BackupScheduler()

        # Invalid cron should raise exception
        with pytest.raises(Exception):
            scheduler.get_next_run("invalid cron")

    @patch("app.scheduler.async_session")
    async def test_add_schedule_with_invalid_cron(self, mock_session):
        """Test adding schedule with invalid cron expression."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_session_instance.execute.return_value = mock_result

        scheduler = BackupScheduler()
        await scheduler.start()

        target = MagicMock(spec=BackupTarget)
        target.id = 1
        target.name = "test-volume"
        target.schedule_cron = "invalid cron expression"
        target.schedule = None  # No Schedule entity

        # Should return False due to invalid cron
        success = await scheduler.add_schedule(target)
        assert success is False

        await scheduler.stop()

    @patch("app.scheduler.async_session")
    async def test_stop_without_start(self, mock_session):
        """Test stopping scheduler that was never started."""
        scheduler = BackupScheduler()
        # Should not raise exception
        await scheduler.stop()
        assert scheduler._running is False
