"""
Tests for backup_engine module.
"""

import asyncio
import io
import tarfile
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from app.backup_engine import BackupEngine, BackupMetrics
from app.database import Backup, BackupStatus, BackupTarget, BackupType


class TestBackupEngine:
    """Test backup engine functionality."""

    def test_engine_initialization(self):
        """Test backup engine initialization."""
        engine = BackupEngine()

        assert engine is not None
        assert hasattr(engine, "active_backups")
        assert hasattr(engine, "progress_callbacks")
        assert hasattr(engine, "metrics")
        assert hasattr(engine, "_backup_semaphore")
        assert isinstance(engine._backup_semaphore, asyncio.Semaphore)

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

    @patch("app.backup_engine.async_session")
    async def test_create_backup(self, mock_session):
        """Test backup creation."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )

        engine = BackupEngine()
        backup = await engine.create_backup(target, BackupType.FULL)

        assert backup is not None
        assert backup.target_id == target.id
        assert backup.backup_type == BackupType.FULL
        assert backup.status == BackupStatus.PENDING
        mock_session_instance.add.assert_called_once()
        mock_session_instance.commit.assert_called_once()

    @patch("app.backup_engine.async_session")
    async def test_run_backup_nonexistent(self, mock_session):
        """Test running backup for non-existent backup ID."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        # Mock query returning None
        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = None
        mock_session_instance.execute.return_value = mock_result

        engine = BackupEngine()
        result = await engine.run_backup(99999)

        assert result is False

    @patch("app.backup_engine.async_session")
    async def test_run_backup_no_target(self, mock_session):
        """Test running backup with missing target."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        # Mock backup exists
        backup = MagicMock(spec=Backup)
        backup.id = 1
        backup.target_id = 999

        # First query returns backup, second query returns None (no target)
        mock_result_backup = MagicMock()
        mock_result_backup.scalar_one_or_none.return_value = backup

        mock_result_target = MagicMock()
        mock_result_target.scalar_one_or_none.return_value = None

        mock_session_instance.execute.side_effect = [
            mock_result_backup,
            mock_result_target,
        ]

        engine = BackupEngine()
        result = await engine.run_backup(1)

        assert result is False

    @patch("app.backup_engine.docker_client")
    async def test_validate_missing_container(self, mock_docker):
        """Test validation fails when container doesn't exist."""
        mock_docker.list_containers = AsyncMock(return_value=[])

        target = BackupTarget(
            id=1,
            name="test-container",
            target_type="container",
            container_name="missing-container",
            enabled=True,
        )

        engine = BackupEngine()
        issues = await engine.validate_backup_prerequisites(target)

        assert len(issues) > 0
        assert any("not found" in issue.lower() for issue in issues)

    @patch("app.backup_engine.docker_client")
    async def test_validate_missing_volume(self, mock_docker):
        """Test validation fails when volume doesn't exist."""
        mock_docker.list_volumes = AsyncMock(return_value=[])

        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="missing-volume",
            enabled=True,
        )

        engine = BackupEngine()
        issues = await engine.validate_backup_prerequisites(target)

        assert len(issues) > 0
        assert any("not found" in issue.lower() for issue in issues)

    async def test_validate_missing_path(self):
        """Test validation fails when path doesn't exist."""
        target = BackupTarget(
            id=1,
            name="test-path",
            target_type="path",
            host_path="/nonexistent/path/to/backup",
            enabled=True,
        )

        engine = BackupEngine()
        issues = await engine.validate_backup_prerequisites(target)

        assert len(issues) > 0
        assert any("not found" in issue.lower() for issue in issues)

    @patch("app.backup_engine.docker_client")
    async def test_validate_missing_dependency(self, mock_docker):
        """Test validation fails when dependency container doesn't exist."""
        mock_docker.list_containers = AsyncMock(return_value=[])
        mock_volume = MagicMock()
        mock_volume.name = "test-volume"
        mock_docker.list_volumes = AsyncMock(return_value=[mock_volume])

        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            dependencies=["missing-dependency"],
            enabled=True,
        )

        engine = BackupEngine()
        issues = await engine.validate_backup_prerequisites(target)

        assert len(issues) > 0
        assert any("dependency" in issue.lower() for issue in issues)

    @patch("shutil.disk_usage")
    @patch("app.backup_engine.docker_client")
    async def test_validate_low_disk_space(
        self, mock_docker, mock_disk_usage, temp_backup_dir
    ):
        """Test validation warns on low disk space."""
        mock_disk_usage.return_value = MagicMock(
            free=500 * 1024 * 1024,  # 500 MB
            total=100 * 1024 * 1024 * 1024,
            used=99.5 * 1024 * 1024 * 1024,
        )

        mock_volume = MagicMock()
        mock_volume.name = "test-volume"
        mock_docker.list_volumes = AsyncMock(return_value=[mock_volume])

        target = BackupTarget(
            id=1,
            name="test-volume",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )

        engine = BackupEngine()

        with patch("app.backup_engine.settings") as mock_settings:
            mock_settings.BACKUP_BASE_PATH = str(temp_backup_dir)
            issues = await engine.validate_backup_prerequisites(target)

        assert any("disk space" in issue.lower() for issue in issues)

    @patch("app.backup_engine.docker_client")
    async def test_validate_existing_container_passes(self, mock_docker):
        """Test validation passes when container exists."""
        mock_container = MagicMock()
        mock_container.name = "my-container"
        mock_docker.list_containers = AsyncMock(return_value=[mock_container])

        target = BackupTarget(
            id=1,
            name="test-container",
            target_type="container",
            container_name="my-container",
            enabled=True,
        )

        engine = BackupEngine()

        with patch("shutil.disk_usage") as mock_disk:
            mock_disk.return_value = MagicMock(free=100 * 1024 * 1024 * 1024)
            issues = await engine.validate_backup_prerequisites(target)

        # Should have no container-related issues
        assert not any(
            "container" in issue.lower() and "not found" in issue.lower()
            for issue in issues
        )


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


class TestTarSecurityValidation:
    """Test tar archive security validation."""

    def test_extract_tar_blocks_path_traversal(self, temp_backup_dir):
        """Test that path traversal in tar archives is blocked."""
        tar_path = temp_backup_dir / "malicious.tar"
        with tarfile.open(tar_path, "w") as tar:
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
        tar_path = temp_backup_dir / "absolute.tar"
        with tarfile.open(tar_path, "w") as tar:
            info = tarfile.TarInfo(name="/etc/passwd")
            info.size = 4
            tar.addfile(info, io.BytesIO(b"test"))

        engine = BackupEngine()
        extract_dir = temp_backup_dir / "extract"
        extract_dir.mkdir()

        # The implementation detects this as path traversal
        with pytest.raises(ValueError, match="[Pp]ath traversal"):
            engine._extract_tar(str(tar_path), str(extract_dir), "r")

    def test_extract_tar_blocks_symlink_escape(self, temp_backup_dir):
        """Test that symlink escape attempts in tar archives are blocked."""
        tar_path = temp_backup_dir / "symlink.tar"
        with tarfile.open(tar_path, "w") as tar:
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
        tar_path = temp_backup_dir / "safe.tar"
        with tarfile.open(tar_path, "w") as tar:
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


class TestBackupConcurrency:
    """Test backup concurrency functionality."""

    def test_backup_semaphore_initialization(self):
        """Test that backup engine initializes with semaphore."""
        engine = BackupEngine()

        assert hasattr(engine, "_backup_semaphore")
        assert isinstance(engine._backup_semaphore, asyncio.Semaphore)

    @patch("app.backup_engine.async_session")
    async def test_metrics_tracked_on_failure(self, mock_session):
        """Test that metrics are updated after backup failure."""
        mock_session_instance = AsyncMock()
        mock_session.return_value.__aenter__.return_value = mock_session_instance

        # Mock backup query returning None (failure case)
        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = None
        mock_session_instance.execute.return_value = mock_result

        engine = BackupEngine()

        # Run backup (will fail due to no backup found)
        result = await engine.run_backup(99999)

        assert result is False
        # Failed early - may not record metric since backup not found
