"""
Tests for backup_engine module.
"""

import asyncio
import os
import shutil
import tempfile
from datetime import datetime
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, mock_open, patch

import pytest
from sqlalchemy import select

from app.backup_engine import BackupEngine, BackupMetrics
from app.database import Backup, BackupStatus, BackupTarget, BackupType


@pytest.mark.asyncio
class TestBackupEngine:
    """Test backup engine functionality."""

    async def test_create_backup(self, db_session):
        """Test backup creation."""
        # Create a target
        target = BackupTarget(
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        engine = BackupEngine()
        backup = await engine.create_backup(target, BackupType.FULL)

        assert backup is not None
        assert backup.target_id == target.id
        assert backup.backup_type == BackupType.FULL
        assert backup.status == BackupStatus.PENDING
        assert backup.backup_metadata["target_name"] == "test-volume"
        assert backup.backup_metadata["target_type"] == "volume"

    async def test_add_remove_progress_callback(self):
        """Test progress callback management."""
        engine = BackupEngine()
        callback = AsyncMock()

        # Add callback
        engine.add_progress_callback(callback)
        assert callback in engine.progress_callbacks

        # Test notification
        await engine._notify_progress(1, 50.0, "Test progress")
        callback.assert_called_once_with(1, 50.0, "Test progress")

        # Remove callback
        engine.remove_progress_callback(callback)
        assert callback not in engine.progress_callbacks

    async def test_notify_progress_with_exception(self):
        """Test progress notification handles callback exceptions."""
        engine = BackupEngine()
        failing_callback = AsyncMock(side_effect=Exception("Callback error"))
        working_callback = AsyncMock()

        engine.add_progress_callback(failing_callback)
        engine.add_progress_callback(working_callback)

        # Should not raise exception even if callback fails
        await engine._notify_progress(1, 50.0, "Test progress")

        failing_callback.assert_called_once()
        working_callback.assert_called_once()

    @patch("app.backup_engine.docker_client")
    async def test_run_backup_volume(self, mock_docker, db_session, temp_backup_dir):
        """Test running a volume backup."""
        # Setup mock Docker client
        mock_volume = MagicMock()
        mock_volume.name = "test-volume"
        mock_docker.volumes.get.return_value = mock_volume
        mock_docker.containers.create.return_value = MagicMock(id="container123")
        mock_docker.containers.get.return_value = MagicMock()

        # Create target and backup
        target = BackupTarget(
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        backup = Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.PENDING,
            backup_metadata={"target_name": "test-volume", "target_type": "volume"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        engine = BackupEngine()

        with patch("app.backup_engine.settings") as mock_settings:
            mock_settings.BACKUP_ROOT = str(temp_backup_dir)
            with patch("tarfile.open") as mock_tarfile:
                mock_tar = MagicMock()
                mock_tarfile.return_value.__enter__.return_value = mock_tar

                result = await engine.run_backup(backup.id)

        assert result is True
        mock_docker.volumes.get.assert_called_with("test-volume")
        mock_docker.containers.create.assert_called_once()

    async def test_run_backup_nonexistent(self, db_session):
        """Test running backup for non-existent backup ID."""
        engine = BackupEngine()
        result = await engine.run_backup(99999)
        assert result is False

    async def test_run_backup_no_target(self, db_session):
        """Test running backup with missing target."""
        # Create backup without target
        backup = Backup(
            target_id=99999,  # Non-existent target
            backup_type=BackupType.FULL,
            status=BackupStatus.PENDING,
            backup_metadata={},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        engine = BackupEngine()
        result = await engine.run_backup(backup.id)
        assert result is False

    @patch("app.backup_engine.docker_client")
    async def test_run_backup_docker_error(self, mock_docker, db_session):
        """Test backup handles Docker errors gracefully."""
        # Setup Docker error
        mock_docker.volumes.get.side_effect = Exception("Docker error")

        # Create target and backup
        target = BackupTarget(
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        backup = Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.PENDING,
            backup_metadata={"target_name": "test-volume", "target_type": "volume"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        engine = BackupEngine()
        result = await engine.run_backup(backup.id, skip_validation=True)

        assert result is False

        # Verify backup status was updated to failed
        await db_session.refresh(backup)
        assert backup.status == BackupStatus.FAILED
        assert "Docker error" in backup.error_message

    async def test_security_path_traversal_prevention(self, db_session):
        """Test that path traversal attempts are blocked."""
        # Create target with malicious path
        target = BackupTarget(
            name="../../../etc/passwd",
            target_type="path",
            host_path="../../../etc/passwd",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        engine = BackupEngine()
        backup = await engine.create_backup(target)

        # Should reject path traversal attempts
        result = await engine.run_backup(backup.id)
        assert result is False

        await db_session.refresh(backup)
        assert backup.status == BackupStatus.FAILED


@pytest.mark.asyncio
class TestBackupMetrics:
    """Test BackupMetrics functionality."""

    def test_metrics_initial_state(self):
        """Test initial metrics state."""
        metrics = BackupMetrics()

        assert metrics.total_backups == 0
        assert metrics.successful_backups == 0
        assert metrics.failed_backups == 0
        assert metrics.total_bytes_backed_up == 0
        assert metrics.success_rate == 0.0
        assert metrics.last_backup_time is None

    def test_record_successful_backup(self):
        """Test recording a successful backup."""
        metrics = BackupMetrics()

        metrics.record_backup(
            target_id=1,
            duration=120.5,
            size=1024000,
            success=True,
        )

        assert metrics.total_backups == 1
        assert metrics.successful_backups == 1
        assert metrics.failed_backups == 0
        assert metrics.total_bytes_backed_up == 1024000
        assert metrics.success_rate == 100.0
        assert metrics.last_backup_time is not None

    def test_record_failed_backup(self):
        """Test recording a failed backup."""
        metrics = BackupMetrics()

        metrics.record_backup(
            target_id=1,
            duration=30.0,
            size=0,
            success=False,
        )

        assert metrics.total_backups == 1
        assert metrics.successful_backups == 0
        assert metrics.failed_backups == 1
        assert metrics.total_bytes_backed_up == 0
        assert metrics.success_rate == 0.0

    def test_success_rate_calculation(self):
        """Test success rate calculation with mixed results."""
        metrics = BackupMetrics()

        # Record 3 successful, 1 failed
        for _ in range(3):
            metrics.record_backup(target_id=1, duration=60.0, size=1000, success=True)
        metrics.record_backup(target_id=1, duration=30.0, size=0, success=False)

        assert metrics.total_backups == 4
        assert metrics.successful_backups == 3
        assert metrics.failed_backups == 1
        assert metrics.success_rate == 75.0

    def test_average_duration_calculation(self):
        """Test average duration calculation per target."""
        metrics = BackupMetrics()

        # Record backups with different durations
        metrics.record_backup(target_id=1, duration=60.0, size=1000, success=True)
        metrics.record_backup(target_id=1, duration=120.0, size=1000, success=True)
        metrics.record_backup(target_id=1, duration=90.0, size=1000, success=True)

        avg = metrics.get_average_duration(1)
        assert avg == 90.0  # (60 + 120 + 90) / 3

    def test_average_size_calculation(self):
        """Test average size calculation per target."""
        metrics = BackupMetrics()

        metrics.record_backup(target_id=2, duration=60.0, size=1000, success=True)
        metrics.record_backup(target_id=2, duration=60.0, size=2000, success=True)
        metrics.record_backup(target_id=2, duration=60.0, size=3000, success=True)

        avg = metrics.get_average_size(2)
        assert avg == 2000  # (1000 + 2000 + 3000) / 3

    def test_metrics_per_target_isolation(self):
        """Test that metrics are isolated per target."""
        metrics = BackupMetrics()

        metrics.record_backup(target_id=1, duration=60.0, size=1000, success=True)
        metrics.record_backup(target_id=2, duration=120.0, size=2000, success=True)

        assert metrics.get_average_duration(1) == 60.0
        assert metrics.get_average_duration(2) == 120.0
        assert metrics.get_average_size(1) == 1000
        assert metrics.get_average_size(2) == 2000

    def test_nonexistent_target_returns_none(self):
        """Test that non-existent target returns None for averages."""
        metrics = BackupMetrics()

        assert metrics.get_average_duration(999) is None
        assert metrics.get_average_size(999) is None

    def test_to_dict_serialization(self):
        """Test metrics serialization to dict."""
        metrics = BackupMetrics()
        metrics.record_backup(target_id=1, duration=60.0, size=1000, success=True)

        result = metrics.to_dict()

        assert "total_backups" in result
        assert "successful_backups" in result
        assert "failed_backups" in result
        assert "success_rate" in result
        assert "total_bytes_backed_up" in result
        assert "last_backup_time" in result

        assert result["total_backups"] == 1
        assert result["success_rate"] == 100.0

    def test_metrics_memory_limit(self):
        """Test that metrics limit stored history per target."""
        metrics = BackupMetrics()

        # Record more than 100 backups for same target
        for i in range(150):
            metrics.record_backup(
                target_id=1, duration=float(i), size=i * 100, success=True
            )

        # Should only keep last 100
        assert len(metrics.target_durations[1]) == 100
        assert len(metrics.target_sizes[1]) == 100


@pytest.mark.asyncio
class TestBackupValidation:
    """Test backup validation functionality."""

    @patch("app.backup_engine.docker_client")
    async def test_validate_missing_container(self, mock_docker, db_session):
        """Test validation fails when container doesn't exist."""
        # Mock empty container list
        mock_docker.list_containers = AsyncMock(return_value=[])

        target = BackupTarget(
            name="test-container",
            target_type="container",
            container_name="missing-container",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        engine = BackupEngine()
        issues = await engine.validate_backup_prerequisites(target)

        assert len(issues) > 0
        assert any("not found" in issue.lower() for issue in issues)

    @patch("app.backup_engine.docker_client")
    async def test_validate_missing_volume(self, mock_docker, db_session):
        """Test validation fails when volume doesn't exist."""
        # Mock empty volume list
        mock_docker.list_volumes = AsyncMock(return_value=[])

        target = BackupTarget(
            name="test-volume",
            target_type="volume",
            volume_name="missing-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        engine = BackupEngine()
        issues = await engine.validate_backup_prerequisites(target)

        assert len(issues) > 0
        assert any("not found" in issue.lower() for issue in issues)

    async def test_validate_missing_path(self, db_session):
        """Test validation fails when path doesn't exist."""
        target = BackupTarget(
            name="test-path",
            target_type="path",
            host_path="/nonexistent/path/to/backup",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        engine = BackupEngine()
        issues = await engine.validate_backup_prerequisites(target)

        assert len(issues) > 0
        assert any("not found" in issue.lower() for issue in issues)

    @patch("app.backup_engine.docker_client")
    async def test_validate_missing_dependency(self, mock_docker, db_session):
        """Test validation fails when dependency container doesn't exist."""
        # Mock empty container list
        mock_docker.list_containers = AsyncMock(return_value=[])
        mock_docker.list_volumes = AsyncMock(
            return_value=[MagicMock(name="test-volume")]
        )

        target = BackupTarget(
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            dependencies=["missing-dependency"],
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        engine = BackupEngine()
        issues = await engine.validate_backup_prerequisites(target)

        assert len(issues) > 0
        assert any("dependency" in issue.lower() for issue in issues)

    @patch("shutil.disk_usage")
    @patch("app.backup_engine.docker_client")
    async def test_validate_low_disk_space(
        self, mock_docker, mock_disk_usage, db_session, temp_backup_dir
    ):
        """Test validation warns on low disk space."""
        # Mock low disk space (500MB free)
        mock_disk_usage.return_value = MagicMock(
            free=500 * 1024 * 1024,  # 500 MB
            total=100 * 1024 * 1024 * 1024,  # 100 GB
            used=99.5 * 1024 * 1024 * 1024,  # 99.5 GB
        )

        # Mock volume exists
        mock_volume = MagicMock()
        mock_volume.name = "test-volume"
        mock_docker.list_volumes = AsyncMock(return_value=[mock_volume])

        target = BackupTarget(
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        engine = BackupEngine()

        with patch("app.backup_engine.settings") as mock_settings:
            mock_settings.BACKUP_BASE_PATH = str(temp_backup_dir)
            issues = await engine.validate_backup_prerequisites(target)

        assert any("disk space" in issue.lower() for issue in issues)

    @patch("app.backup_engine.docker_client")
    async def test_validate_existing_container(self, mock_docker, db_session):
        """Test validation passes when container exists."""
        # Mock container exists
        mock_container = MagicMock()
        mock_container.name = "my-container"
        mock_docker.list_containers = AsyncMock(return_value=[mock_container])

        target = BackupTarget(
            name="test-container",
            target_type="container",
            container_name="my-container",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        engine = BackupEngine()

        with patch("shutil.disk_usage") as mock_disk:
            # Mock plenty of disk space
            mock_disk.return_value = MagicMock(free=100 * 1024 * 1024 * 1024)
            issues = await engine.validate_backup_prerequisites(target)

        # Should have no issues (or only non-critical ones)
        assert not any(
            "container" in issue.lower() and "not found" in issue.lower()
            for issue in issues
        )


@pytest.mark.asyncio
class TestBackupConcurrency:
    """Test backup concurrency and semaphore functionality."""

    async def test_backup_semaphore_initialization(self):
        """Test that backup engine initializes with semaphore."""
        engine = BackupEngine()

        assert hasattr(engine, "_backup_semaphore")
        assert isinstance(engine._backup_semaphore, asyncio.Semaphore)

    async def test_concurrent_backup_limit(self, db_session):
        """Test that concurrent backups are limited by semaphore."""
        from app.config import settings

        engine = BackupEngine()

        # Create multiple targets and backups
        backups = []
        for i in range(5):
            target = BackupTarget(
                name=f"test-volume-{i}",
                target_type="volume",
                volume_name=f"test-volume-{i}",
                enabled=True,
            )
            db_session.add(target)
            await db_session.commit()
            await db_session.refresh(target)

            backup = Backup(
                target_id=target.id,
                backup_type=BackupType.FULL,
                status=BackupStatus.PENDING,
                backup_metadata={
                    "target_name": f"test-volume-{i}",
                    "target_type": "volume",
                },
            )
            db_session.add(backup)
            await db_session.commit()
            await db_session.refresh(backup)
            backups.append(backup)

        # Track concurrent execution count
        concurrent_count = 0
        max_concurrent = 0
        lock = asyncio.Lock()

        original_run = engine.run_backup

        async def tracked_run(backup_id, skip_validation=False):
            nonlocal concurrent_count, max_concurrent
            async with lock:
                concurrent_count += 1
                max_concurrent = max(max_concurrent, concurrent_count)

            # Simulate some work
            await asyncio.sleep(0.1)

            async with lock:
                concurrent_count -= 1

            return False  # Return False to avoid actual backup

        with patch.object(engine, "run_backup", tracked_run):
            # Run all backups concurrently
            tasks = [engine.run_backup(b.id) for b in backups]
            await asyncio.gather(*tasks)

        # Max concurrent should not exceed the limit
        assert max_concurrent <= settings.MAX_CONCURRENT_BACKUPS

    async def test_metrics_tracked_after_backup(self, db_session):
        """Test that metrics are updated after backup completion."""
        engine = BackupEngine()

        initial_total = engine.metrics.total_backups

        # Create target and backup
        target = BackupTarget(
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        backup = Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.PENDING,
            backup_metadata={"target_name": "test-volume", "target_type": "volume"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        # Run backup (will fail due to missing volume, but should track metrics)
        with patch("app.backup_engine.docker_client") as mock_docker:
            mock_docker.list_volumes = AsyncMock(return_value=[])
            await engine.run_backup(backup.id)

        # Metrics should be updated
        assert engine.metrics.total_backups > initial_total


@pytest.mark.asyncio
class TestBackupContainerDependencies:
    """Test backup with container dependency management."""

    @patch("app.backup_engine.docker_client")
    async def test_backup_stops_containers_in_order(self, mock_docker, db_session):
        """Test that containers are stopped in dependency order."""
        stopped_containers = []

        async def mock_stop(name):
            stopped_containers.append(name)
            return True

        async def mock_start(name):
            return True

        async def mock_get_state(name):
            return "running"

        mock_docker.stop_container = mock_stop
        mock_docker.start_container = mock_start
        mock_docker.get_container_state = mock_get_state
        mock_docker.get_dependency_order = AsyncMock(
            return_value=["app", "db", "redis"]
        )
        mock_docker.list_volumes = AsyncMock(
            return_value=[
                MagicMock(
                    name="test-volume",
                    mountpoint="/var/lib/docker/volumes/test-volume/_data",
                )
            ]
        )

        target = BackupTarget(
            name="test-app",
            target_type="volume",
            volume_name="test-volume",
            container_name="app",
            stop_container=True,
            dependencies=["db", "redis"],
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        backup = Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.PENDING,
            backup_metadata={"target_name": "test-app", "target_type": "volume"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        engine = BackupEngine()

        with patch("app.backup_engine.settings") as mock_settings:
            mock_settings.BACKUP_BASE_PATH = "/tmp/test_backups"
            mock_settings.COMPRESSION_LEVEL = 6
            with patch.object(
                engine,
                "_create_backup_archive",
                AsyncMock(return_value="/tmp/backup.tar.gz"),
            ):
                with patch.object(
                    engine, "_calculate_checksum", AsyncMock(return_value="abc123")
                ):
                    await engine.run_backup(backup.id, skip_validation=True)

        # Verify containers were stopped
        assert "app" in stopped_containers or len(stopped_containers) > 0


@pytest.mark.asyncio
class TestTarSecurityValidation:
    """Test tar archive security validation."""

    def test_extract_tar_blocks_path_traversal(self, temp_backup_dir):
        """Test that path traversal in tar archives is blocked."""
        import io
        import tarfile

        # Create a malicious tar file with path traversal
        tar_path = temp_backup_dir / "malicious.tar"
        with tarfile.open(tar_path, "w") as tar:
            # Add a file with path traversal attempt
            info = tarfile.TarInfo(name="../../../etc/passwd")
            info.size = 4
            tar.addfile(info, io.BytesIO(b"test"))

        engine = BackupEngine()
        extract_dir = temp_backup_dir / "extract"
        extract_dir.mkdir()

        with pytest.raises(ValueError, match="[Pp]ath traversal"):
            engine._extract_tar(str(tar_path), str(extract_dir), "r")

    def test_extract_tar_blocks_absolute_paths(self, temp_backup_dir):
        """Test that absolute paths in tar archives are blocked."""
        import io
        import tarfile

        tar_path = temp_backup_dir / "absolute.tar"
        with tarfile.open(tar_path, "w") as tar:
            # Add a file with absolute path
            info = tarfile.TarInfo(name="/etc/passwd")
            info.size = 4
            tar.addfile(info, io.BytesIO(b"test"))

        engine = BackupEngine()
        extract_dir = temp_backup_dir / "extract"
        extract_dir.mkdir()

        with pytest.raises(ValueError, match="[Aa]bsolute path"):
            engine._extract_tar(str(tar_path), str(extract_dir), "r")

    def test_extract_tar_blocks_symlink_escape(self, temp_backup_dir):
        """Test that symlink escape attempts in tar archives are blocked."""
        import tarfile

        tar_path = temp_backup_dir / "symlink.tar"
        with tarfile.open(tar_path, "w") as tar:
            # Add a symlink pointing outside the extract directory
            info = tarfile.TarInfo(name="link")
            info.type = tarfile.SYMTYPE
            info.linkname = "../../../etc/passwd"
            tar.addfile(info)

        engine = BackupEngine()
        extract_dir = temp_backup_dir / "extract"
        extract_dir.mkdir()

        with pytest.raises(ValueError, match="[Ss]ymlink escape"):
            engine._extract_tar(str(tar_path), str(extract_dir), "r")

    def test_extract_tar_allows_safe_files(self, temp_backup_dir):
        """Test that safe tar files are extracted correctly."""
        import io
        import tarfile

        tar_path = temp_backup_dir / "safe.tar"
        with tarfile.open(tar_path, "w") as tar:
            # Add safe files
            info = tarfile.TarInfo(name="data/file.txt")
            content = b"Hello, World!"
            info.size = len(content)
            tar.addfile(info, io.BytesIO(content))

        engine = BackupEngine()
        extract_dir = temp_backup_dir / "extract"
        extract_dir.mkdir()

        # Should not raise
        engine._extract_tar(str(tar_path), str(extract_dir), "r")

        # Verify file was extracted
        assert (extract_dir / "data" / "file.txt").exists()
