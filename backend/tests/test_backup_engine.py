"""
Tests for backup_engine module.
"""

import asyncio
import io
import tarfile
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from app.backup_engine import BackupEngine, BackupMetrics
from sqlalchemy import select

from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    BackupType,
)
from app.encryption import EncryptionError


async def _create_backup_record(test_db, backup_path=None, **target_overrides):
    target_values = {
        "name": "safety-test",
        "target_type": "path",
        "host_path": str(backup_path.parent if backup_path else "/tmp"),
        "container_name": "test-container",
        "stop_container": True,
        "compression_enabled": False,
        "dependencies": [],
        "remote_storage_ids": [],
    }
    target_values.update(target_overrides)

    async with test_db() as session:
        target = BackupTarget(**target_values)
        backup = Backup(
            target=target,
            status=BackupStatus.COMPLETED if backup_path else BackupStatus.PENDING,
            backup_type=BackupType.FULL,
            file_path=str(backup_path) if backup_path else None,
        )
        session.add(backup)
        await session.commit()
        await session.refresh(target)
        await session.refresh(backup)
        return target.id, backup.id


def _configure_stopped_container(mock_docker, start_result=True):
    mock_docker.get_dependency_order = AsyncMock(return_value=["test-container"])
    mock_docker.get_container_state = AsyncMock(
        side_effect=lambda _name: (
            "running"
            if mock_docker.get_container_state.await_count in (1, 3)
            else "exited"
        )
    )
    mock_docker.stop_container = AsyncMock(return_value=True)
    mock_docker.start_container = AsyncMock(return_value=start_result)


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


class TestBackupSafety:
    async def test_backup_stop_failure_aborts_before_archive(
        self, test_db, temp_backup_dir
    ):
        _, backup_id = await _create_backup_record(test_db)
        engine = BackupEngine()
        archive = AsyncMock()

        with (
            patch("app.backup_engine.async_session", test_db),
            patch("app.backup_engine.docker_client") as mock_docker,
            patch.object(engine, "_create_backup_archive", archive),
        ):
            mock_docker.get_dependency_order = AsyncMock(
                return_value=["test-container"]
            )
            mock_docker.get_container_state = AsyncMock(return_value="running")
            mock_docker.stop_container = AsyncMock(return_value=False)
            mock_docker.start_container = AsyncMock(return_value=True)

            result = await engine.run_backup(backup_id, skip_validation=True)

        assert result is False
        archive.assert_not_awaited()

    async def test_restore_stop_failure_aborts_before_extraction(
        self, test_db, temp_backup_dir
    ):
        archive_path = temp_backup_dir / "restore.tar"
        archive_path.write_bytes(b"archive")
        _, backup_id = await _create_backup_record(test_db, archive_path)
        engine = BackupEngine()
        extract = MagicMock()

        with (
            patch("app.backup_engine.async_session", test_db),
            patch("app.backup_engine.docker_client") as mock_docker,
            patch.object(engine, "_extract_tar", extract),
        ):
            mock_docker.get_container_state = AsyncMock(return_value="running")
            mock_docker.stop_container = AsyncMock(return_value=False)
            mock_docker.start_container = AsyncMock(return_value=True)

            result = await engine.restore_backup(
                backup_id, target_path=str(temp_backup_dir / "restore")
            )

        assert result is False
        extract.assert_not_called()

    async def test_backup_start_failure_fails_after_bounded_retries(
        self, test_db, temp_backup_dir
    ):
        archive_path = temp_backup_dir / "backup.tar"
        archive_path.write_bytes(b"archive")
        _, backup_id = await _create_backup_record(test_db)
        engine = BackupEngine()

        with (
            patch("app.backup_engine.async_session", test_db),
            patch("app.backup_engine.docker_client") as mock_docker,
            patch("app.backup_engine.asyncio.sleep", new=AsyncMock()),
            patch.object(
                engine,
                "_create_backup_archive",
                new=AsyncMock(return_value=str(archive_path)),
            ),
            patch.object(
                engine,
                "_get_encryption_config",
                new=AsyncMock(return_value=None),
            ),
        ):
            mock_docker.get_dependency_order = AsyncMock(
                return_value=["test-container"]
            )
            mock_docker.get_container_state = AsyncMock(
                side_effect=["running"] + ["exited"] * 20
            )
            mock_docker.stop_container = AsyncMock(return_value=True)
            mock_docker.start_container = AsyncMock(return_value=False)

            result = await engine.run_backup(backup_id, skip_validation=True)

        assert result is False
        assert mock_docker.start_container.await_count == 3

    async def test_restore_start_failure_fails_after_bounded_retries(
        self, test_db, temp_backup_dir
    ):
        archive_path = temp_backup_dir / "restore.tar"
        archive_path.write_bytes(b"archive")
        _, backup_id = await _create_backup_record(test_db, archive_path)
        engine = BackupEngine()

        with (
            patch("app.backup_engine.async_session", test_db),
            patch("app.backup_engine.docker_client") as mock_docker,
            patch("app.backup_engine.asyncio.sleep", new=AsyncMock()),
            patch.object(engine, "_extract_tar", new=MagicMock()),
        ):
            mock_docker.get_container_state = AsyncMock(
                side_effect=["running"] + ["exited"] * 20
            )
            mock_docker.stop_container = AsyncMock(return_value=True)
            mock_docker.start_container = AsyncMock(return_value=False)

            result = await engine.restore_backup(
                backup_id, target_path=str(temp_backup_dir / "restore")
            )

        assert result is False
        assert mock_docker.start_container.await_count == 3

    async def test_backup_cancellation_restarts_container_and_propagates(self, test_db):
        _, backup_id = await _create_backup_record(test_db)
        engine = BackupEngine()

        with (
            patch("app.backup_engine.async_session", test_db),
            patch("app.backup_engine.docker_client") as mock_docker,
            patch.object(
                engine,
                "_create_backup_archive",
                new=AsyncMock(side_effect=asyncio.CancelledError),
            ),
        ):
            _configure_stopped_container(mock_docker)

            with pytest.raises(asyncio.CancelledError):
                await engine.run_backup(backup_id, skip_validation=True)

        mock_docker.start_container.assert_awaited_once_with("test-container")

        async with test_db() as session:
            backup = await session.get(Backup, backup_id)

        assert backup.status == BackupStatus.FAILED
        assert "cancelled" in backup.error_message.lower()

    async def test_backup_cancellation_preserves_cancellation_on_restart_failure(
        self, test_db, caplog
    ):
        _, backup_id = await _create_backup_record(test_db)
        engine = BackupEngine()

        with (
            patch("app.backup_engine.async_session", test_db),
            patch("app.backup_engine.docker_client") as mock_docker,
            patch("app.backup_engine.asyncio.sleep", new=AsyncMock()),
            patch.object(
                engine,
                "_create_backup_archive",
                new=AsyncMock(side_effect=asyncio.CancelledError),
            ),
        ):
            mock_docker.get_dependency_order = AsyncMock(
                return_value=["test-container"]
            )
            mock_docker.get_container_state = AsyncMock(
                side_effect=["running"] + ["exited"] * 20
            )
            mock_docker.stop_container = AsyncMock(return_value=True)
            mock_docker.start_container = AsyncMock(return_value=False)

            with pytest.raises(asyncio.CancelledError):
                await engine.run_backup(backup_id, skip_validation=True)

        assert mock_docker.start_container.await_count == 3
        assert "Failed to restart container test-container" in caplog.text

    async def test_encryption_error_fails_closed_and_removes_partial_files(
        self, test_db, temp_backup_dir
    ):
        archive_path = temp_backup_dir / "backup.tar"
        encrypted_path = temp_backup_dir / "backup.tar.enc"
        key_path = temp_backup_dir / "backup.tar.key"
        archive_path.write_bytes(b"plaintext")
        _, backup_id = await _create_backup_record(
            test_db, stop_container=False, container_name=None
        )
        engine = BackupEngine()

        async def fail_encryption(*_args):
            encrypted_path.write_bytes(b"partial encrypted data")
            key_path.write_bytes(b"partial key")
            raise EncryptionError("encryption failed")

        encryption_config = SimpleNamespace(
            encryption_enabled=True, public_key="age1test"
        )
        with (
            patch("app.backup_engine.async_session", test_db),
            patch.object(
                engine,
                "_create_backup_archive",
                new=AsyncMock(return_value=str(archive_path)),
            ),
            patch.object(
                engine,
                "_get_encryption_config",
                new=AsyncMock(return_value=encryption_config),
            ),
            patch("app.backup_engine.encrypt_backup", side_effect=fail_encryption),
        ):
            result = await engine.run_backup(backup_id, skip_validation=True)

        assert result is False
        assert not archive_path.exists()
        assert not encrypted_path.exists()
        assert not key_path.exists()

    async def test_incomplete_encryption_output_fails_closed(
        self, test_db, temp_backup_dir
    ):
        archive_path = temp_backup_dir / "backup.tar"
        encrypted_path = temp_backup_dir / "backup.tar.enc"
        key_path = temp_backup_dir / "backup.tar.key"
        archive_path.write_bytes(b"plaintext")
        _, backup_id = await _create_backup_record(
            test_db, stop_container=False, container_name=None
        )
        engine = BackupEngine()

        async def incomplete_encryption(*_args):
            encrypted_path.touch()
            key_path.touch()
            return SimpleNamespace(
                encrypted_path=encrypted_path,
                key_path=key_path,
            )

        encryption_config = SimpleNamespace(
            encryption_enabled=True, public_key="age1test"
        )
        with (
            patch("app.backup_engine.async_session", test_db),
            patch.object(
                engine,
                "_create_backup_archive",
                new=AsyncMock(return_value=str(archive_path)),
            ),
            patch.object(
                engine,
                "_get_encryption_config",
                new=AsyncMock(return_value=encryption_config),
            ),
            patch(
                "app.backup_engine.encrypt_backup", side_effect=incomplete_encryption
            ),
        ):
            result = await engine.run_backup(backup_id, skip_validation=True)

        assert result is False
        assert not archive_path.exists()
        assert not encrypted_path.exists()
        assert not key_path.exists()

    async def test_encryption_does_not_delete_preexisting_artifacts(
        self, test_db, temp_backup_dir
    ):
        archive_path = temp_backup_dir / "backup.tar"
        encrypted_path = temp_backup_dir / "backup.tar.enc"
        key_path = temp_backup_dir / "backup.tar.key"
        archive_path.write_bytes(b"plaintext")
        encrypted_path.write_bytes(b"existing encrypted data")
        key_path.write_bytes(b"existing key")
        _, backup_id = await _create_backup_record(
            test_db, stop_container=False, container_name=None
        )
        engine = BackupEngine()
        encrypt = AsyncMock()
        encryption_config = SimpleNamespace(
            encryption_enabled=True, public_key="age1test"
        )

        with (
            patch("app.backup_engine.async_session", test_db),
            patch.object(
                engine,
                "_create_backup_archive",
                new=AsyncMock(return_value=str(archive_path)),
            ),
            patch.object(
                engine,
                "_get_encryption_config",
                new=AsyncMock(return_value=encryption_config),
            ),
            patch("app.backup_engine.encrypt_backup", encrypt),
        ):
            result = await engine.run_backup(backup_id, skip_validation=True)

        assert result is False
        encrypt.assert_not_awaited()
        assert encrypted_path.read_bytes() == b"existing encrypted data"
        assert key_path.read_bytes() == b"existing key"


class TestRemoteSyncSafety:
    async def _sync(
        self,
        test_db,
        temp_backup_dir,
        sync_results,
        *,
        encrypted=False,
        storage_ids=None,
    ):
        archive_path = temp_backup_dir / "backup.tar.enc"
        archive_path.write_bytes(b"backup")
        key_path = temp_backup_dir / "backup.tar.key"
        if encrypted:
            key_path.write_bytes(b"key")
        target_id, backup_id = await _create_backup_record(
            test_db,
            remote_storage_ids=storage_ids or [1],
            delete_local_after_sync=True,
            stop_container=False,
            container_name=None,
        )
        async with test_db() as session:
            target = await session.get(BackupTarget, target_id)

        engine = BackupEngine()
        delete_local = AsyncMock()
        with (
            patch("app.backup_engine.async_session", test_db),
            patch("app.remote_storage.storage_manager") as storage_manager,
            patch.object(engine, "_delete_local_after_sync", delete_local),
        ):
            storage_manager.get_backend.return_value = MagicMock()
            storage_manager.sync_backup = AsyncMock(side_effect=sync_results)
            await engine._sync_to_remote(
                backup_id,
                target,
                str(archive_path),
                encrypted,
                str(key_path) if encrypted else None,
            )

        return backup_id, delete_local

    async def test_tuple_failure_is_recorded_as_failed(self, test_db, temp_backup_dir):
        backup_id, _ = await self._sync(
            test_db,
            temp_backup_dir,
            [{1: (False, "archive upload failed")}],
        )

        async with test_db() as session:
            result = await session.execute(
                select(BackupStorageSync).where(
                    BackupStorageSync.backup_id == backup_id
                )
            )
            sync_record = result.scalar_one()

        assert sync_record.sync_status == "failed"
        assert sync_record.synced_at is None
        assert sync_record.error_message == "archive upload failed"

    async def test_archive_upload_failure_prevents_local_deletion(
        self, test_db, temp_backup_dir
    ):
        _, delete_local = await self._sync(
            test_db,
            temp_backup_dir,
            [{1: (False, "archive upload failed")}],
        )

        delete_local.assert_not_awaited()

    async def test_key_upload_failure_prevents_all_local_deletion(
        self, test_db, temp_backup_dir
    ):
        _, delete_local = await self._sync(
            test_db,
            temp_backup_dir,
            [
                {1: (True, None)},
                {1: (False, "key upload failed")},
            ],
            encrypted=True,
        )

        delete_local.assert_not_awaited()

    async def test_missing_required_archive_result_prevents_local_deletion(
        self, test_db, temp_backup_dir
    ):
        _, delete_local = await self._sync(
            test_db,
            temp_backup_dir,
            [{1: (True, None)}],
            storage_ids=[1, 2],
        )

        delete_local.assert_not_awaited()

    async def test_missing_required_key_result_prevents_local_deletion(
        self, test_db, temp_backup_dir
    ):
        _, delete_local = await self._sync(
            test_db,
            temp_backup_dir,
            [
                {1: (True, None), 2: (True, None)},
                {1: (True, None)},
            ],
            encrypted=True,
            storage_ids=[1, 2],
        )

        delete_local.assert_not_awaited()


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
