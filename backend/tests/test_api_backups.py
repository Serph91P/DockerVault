"""
Tests for backups API endpoints.
"""

from datetime import datetime, timezone
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from httpx import AsyncClient

from app.database import Backup, BackupStatus, BackupTarget, BackupType


@pytest.mark.asyncio
class TestBackupsAPI:
    """Test backups API endpoints."""

    async def test_list_backups_empty(self, async_client: AsyncClient):
        """Test listing backups when none exist."""
        response = await async_client.get("/api/v1/backups")
        assert response.status_code == 200
        assert response.json() == []

    async def test_list_backups_with_data(self, async_client: AsyncClient, db_session):
        """Test listing backups with data."""
        # Create target and backup
        target = BackupTarget(
            name="test-target",
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
            status=BackupStatus.COMPLETED,
            file_path="/backups/test.tar.gz",
            file_size=1024,
            checksum="abc123",
            backup_metadata={"target_name": "test-target"},
        )
        db_session.add(backup)
        await db_session.commit()

        response = await async_client.get("/api/v1/backups")
        assert response.status_code == 200

        data = response.json()
        assert len(data) == 1
        assert data[0]["id"] == backup.id
        assert data[0]["target_id"] == target.id
        assert data[0]["target_name"] == "test-target"
        assert data[0]["status"] == "completed"
        assert data[0]["file_size"] == 1024
        assert data[0]["checksum"] == "abc123"

    @patch("app.api.backups.backup_engine")
    async def test_create_backup_success(
        self, mock_engine, async_client: AsyncClient, db_session
    ):
        """Test successful backup creation."""
        # Create target
        target = BackupTarget(
            name="test-target",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        # Mock backup engine - create_backup must be AsyncMock returning a backup
        mock_backup = MagicMock()
        mock_backup.id = 1
        mock_backup.target_id = target.id
        mock_backup.backup_type = BackupType.FULL
        mock_backup.status = BackupStatus.PENDING
        mock_backup.file_path = None
        mock_backup.file_size = None
        mock_backup.checksum = None
        mock_backup.started_at = None
        mock_backup.completed_at = None
        mock_backup.duration_seconds = None
        mock_backup.error_message = None
        mock_backup.encrypted = False
        mock_backup.created_at = datetime(2024, 1, 1, tzinfo=timezone.utc)

        mock_engine.create_backup = AsyncMock(return_value=mock_backup)
        mock_engine.run_backup = AsyncMock(return_value=True)
        mock_engine.enqueue = AsyncMock()

        response = await async_client.post(
            "/api/v1/backups", json={"target_id": target.id, "backup_type": "full"}
        )

        assert response.status_code == 200  # API returns 200, not 201
        data = response.json()
        assert data["target_id"] == target.id
        assert data["backup_type"] == "full"
        assert data["status"] == "pending"

    async def test_create_backup_invalid_target(self, async_client: AsyncClient):
        """Test backup creation with invalid target."""
        response = await async_client.post(
            "/api/v1/backups", json={"target_id": 99999, "backup_type": "full"}
        )

        assert response.status_code == 404
        assert "Target not found" in response.json()["detail"]

    @patch("app.api.backups.backup_engine")
    async def test_create_backup_invalid_type(
        self, mock_engine, async_client: AsyncClient, db_session
    ):
        """Test backup creation with invalid backup type is rejected."""
        # Create target
        target = BackupTarget(
            name="test-target",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        response = await async_client.post(
            "/api/v1/backups",
            json={"target_id": target.id, "backup_type": "invalid_type"},
        )

        # Invalid backup_type is rejected with 422
        assert response.status_code == 422

    @patch("app.api.backups.backup_engine")
    async def test_create_backup_with_disabled_target(
        self, mock_engine, async_client: AsyncClient, db_session
    ):
        """Test backup creation with disabled target - API still allows it."""
        # Create disabled target
        target = BackupTarget(
            name="disabled-target",
            target_type="volume",
            volume_name="test-volume",
            enabled=False,  # Disabled
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        # Mock backup engine
        mock_backup = MagicMock()
        mock_backup.id = 1
        mock_backup.target_id = target.id
        mock_backup.backup_type = BackupType.FULL
        mock_backup.status = BackupStatus.PENDING
        mock_backup.file_path = None
        mock_backup.file_size = None
        mock_backup.checksum = None
        mock_backup.started_at = None
        mock_backup.completed_at = None
        mock_backup.duration_seconds = None
        mock_backup.error_message = None
        mock_backup.encrypted = False
        mock_backup.created_at = datetime(2024, 1, 1, tzinfo=timezone.utc)

        mock_engine.create_backup = AsyncMock(return_value=mock_backup)
        mock_engine.run_backup = AsyncMock(return_value=True)
        mock_engine.enqueue = AsyncMock()

        # API currently allows backup of disabled targets
        response = await async_client.post(
            "/api/v1/backups", json={"target_id": target.id, "backup_type": "full"}
        )

        assert response.status_code == 200

    async def test_get_backup_by_id(self, async_client: AsyncClient, db_session):
        """Test getting backup by ID."""
        # Create target and backup
        target = BackupTarget(
            name="test-target",
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
            status=BackupStatus.COMPLETED,
            backup_metadata={"target_name": "test-target"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        response = await async_client.get(f"/api/v1/backups/{backup.id}")
        assert response.status_code == 200

        data = response.json()
        assert data["id"] == backup.id
        assert data["target_id"] == target.id
        assert data["status"] == "completed"

    async def test_get_backup_not_found(self, async_client: AsyncClient):
        """Test getting non-existent backup."""
        response = await async_client.get("/api/v1/backups/99999")
        assert response.status_code == 404
        assert "Backup not found" in response.json()["detail"]

    @patch("os.path.exists")
    @patch("os.remove")
    async def test_delete_backup(
        self, mock_remove, mock_exists, async_client: AsyncClient, db_session
    ):
        """Test backup deletion."""
        mock_exists.return_value = True

        # Create target and backup
        target = BackupTarget(
            name="test-target",
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
            status=BackupStatus.COMPLETED,
            file_path="/backups/test.tar.gz",
            backup_metadata={"target_name": "test-target"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        response = await async_client.delete(f"/api/v1/backups/{backup.id}")
        assert response.status_code == 200
        assert response.json()["status"] == "deleted"

        # Verify file deletion was attempted
        mock_remove.assert_called_once_with("/backups/test.tar.gz")

        # Verify backup was removed from database
        response = await async_client.get(f"/api/v1/backups/{backup.id}")
        assert response.status_code == 404

    async def test_security_sql_injection_prevention(self, async_client: AsyncClient):
        """Test that SQL injection attempts are blocked."""
        # Try SQL injection in backup ID parameter
        malicious_id = "1; DROP TABLE backups; --"
        response = await async_client.get(f"/api/v1/backups/{malicious_id}")

        # Should return 422 (validation error) not 500 (server error)
        assert response.status_code == 422

    @patch("app.api.backups.backup_engine")
    async def test_input_validation_large_backup_type(
        self, mock_engine, async_client: AsyncClient, db_session
    ):
        """Test that invalid backup_type strings are rejected."""
        target = BackupTarget(
            name="test-target",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        response = await async_client.post(
            "/api/v1/backups",
            json={
                "target_id": target.id,
                "backup_type": "x" * 1000,  # Very long invalid string
            },
        )

        # Invalid backup_type is rejected with 422
        assert response.status_code == 422

    @patch("app.api.backups.backup_engine")
    async def test_restore_backup_success(
        self, mock_engine, async_client: AsyncClient, db_session
    ):
        """Test successful backup restoration."""
        # Create target and backup
        target = BackupTarget(
            name="test-target",
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
            status=BackupStatus.COMPLETED,
            file_path="/backups/test.tar.gz",
            backup_metadata={"target_name": "test-target"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        # Mock restore operation - must be AsyncMock returning True directly
        mock_engine.restore_backup = AsyncMock(return_value=True)

        # Send empty JSON body since endpoint expects RestoreBackupRequest
        response = await async_client.post(
            f"/api/v1/backups/{backup.id}/restore", json={}
        )
        assert response.status_code == 200

        data = response.json()
        assert data["status"] == "restored"

        # Verify restore was called with target_path=None and private_key=None
        mock_engine.restore_backup.assert_called_once_with(backup.id, None, None)

    async def test_restore_backup_path_traversal_prevention(
        self, async_client: AsyncClient, db_session
    ):
        """Test that restore prevents path traversal attacks."""
        # Create target and backup
        target = BackupTarget(
            name="test-target",
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
            status=BackupStatus.COMPLETED,
            file_path="/backups/test.tar.gz",
            backup_metadata={"target_name": "test-target"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        # Attempt path traversal in restore request
        response = await async_client.post(
            f"/api/v1/backups/{backup.id}/restore",
            json={"target_path": "../../../etc/passwd"},
        )

        assert response.status_code == 400
        assert "allowed directories" in response.json()["detail"].lower()


@pytest.mark.asyncio
class TestBackupsPaginationAPI:
    """Test backups API pagination functionality."""

    async def test_list_backups_with_pagination(
        self, async_client: AsyncClient, db_session
    ):
        """Test listing backups with pagination."""
        # Create target
        target = BackupTarget(
            name="test-target",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        # Create multiple backups
        for i in range(15):
            backup = Backup(
                target_id=target.id,
                backup_type=BackupType.FULL,
                status=BackupStatus.COMPLETED,
                backup_metadata={"target_name": "test-target", "index": i},
            )
            db_session.add(backup)
        await db_session.commit()

        # Test first page
        response = await async_client.get("/api/v1/backups?limit=5&offset=0")
        assert response.status_code == 200
        data = response.json()
        assert len(data) == 5

        # Test second page
        response = await async_client.get("/api/v1/backups?limit=5&offset=5")
        assert response.status_code == 200
        data = response.json()
        assert len(data) == 5

        # Test third page
        response = await async_client.get("/api/v1/backups?limit=5&offset=10")
        assert response.status_code == 200
        data = response.json()
        assert len(data) == 5

        # Test beyond available data
        response = await async_client.get("/api/v1/backups?limit=5&offset=20")
        assert response.status_code == 200
        data = response.json()
        assert len(data) == 0

    async def test_list_backups_default_pagination(
        self, async_client: AsyncClient, db_session
    ):
        """Test default pagination values."""
        # Create target
        target = BackupTarget(
            name="test-target",
            target_type="volume",
            volume_name="test-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        # Create 60 backups (more than default limit)
        for i in range(60):
            backup = Backup(
                target_id=target.id,
                backup_type=BackupType.FULL,
                status=BackupStatus.COMPLETED,
                backup_metadata={"target_name": "test-target"},
            )
            db_session.add(backup)
        await db_session.commit()

        # Without pagination params, should use defaults
        response = await async_client.get("/api/v1/backups")
        assert response.status_code == 200
        data = response.json()
        # Default limit is 50
        assert len(data) == 50


@pytest.mark.asyncio
class TestBackupsMetricsAPI:
    """Test backups metrics API endpoints."""

    @patch("app.api.backups.backup_engine")
    async def test_get_backup_metrics_summary(
        self, mock_engine, async_client: AsyncClient
    ):
        """Test getting backup metrics summary."""
        # Mock metrics
        mock_engine.metrics.to_dict.return_value = {
            "total_backups": 100,
            "successful_backups": 95,
            "failed_backups": 5,
            "success_rate": 95.0,
            "total_bytes_backed_up": 1073741824,  # 1 GB
            "last_backup_time": "2026-01-24T12:00:00",
        }

        response = await async_client.get("/api/v1/backups/metrics/summary")
        assert response.status_code == 200

        data = response.json()
        assert data["total_backups"] == 100
        assert data["successful_backups"] == 95
        assert data["failed_backups"] == 5
        assert data["success_rate"] == 95.0

    @patch("app.api.backups.backup_engine")
    async def test_get_target_metrics(self, mock_engine, async_client: AsyncClient):
        """Test getting metrics for a specific target."""
        # Mock metrics methods
        mock_engine.metrics.get_average_duration.return_value = 120.5
        mock_engine.metrics.get_average_size.return_value = 1048576  # 1 MB
        mock_engine.metrics.target_durations = {1: [100, 120, 141.5]}

        response = await async_client.get("/api/v1/backups/metrics/target/1")
        assert response.status_code == 200

        data = response.json()
        assert data["target_id"] == 1
        assert data["average_duration_seconds"] == 120.5
        assert data["average_size_bytes"] == 1048576
        assert "average_size_human" in data
        assert data["backup_count"] == 3

    @patch("app.api.backups.backup_engine")
    async def test_get_target_metrics_no_data(
        self, mock_engine, async_client: AsyncClient
    ):
        """Test getting metrics for target with no data."""
        mock_engine.metrics.get_average_duration.return_value = None
        mock_engine.metrics.get_average_size.return_value = None
        mock_engine.metrics.target_durations = {}

        response = await async_client.get("/api/v1/backups/metrics/target/999")
        assert response.status_code == 200

        data = response.json()
        assert data["target_id"] == 999
        assert data["average_duration_seconds"] is None
        assert data["average_size_bytes"] is None
        assert data["backup_count"] == 0


@pytest.mark.asyncio
class TestBackupsValidationAPI:
    """Test backup validation API endpoints."""

    @patch("app.api.backups.backup_engine")
    async def test_validate_backup_success(
        self, mock_engine, async_client: AsyncClient, db_session
    ):
        """Test validating backup prerequisites successfully."""
        # Create target and backup
        target = BackupTarget(
            name="test-target",
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
            backup_metadata={"target_name": "test-target"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        # Mock validation returns no issues
        mock_engine.validate_backup_prerequisites = AsyncMock(return_value=[])

        response = await async_client.post(f"/api/v1/backups/{backup.id}/validate")
        assert response.status_code == 200

        data = response.json()
        assert data["valid"] is True
        assert data["issues"] == []

    @patch("app.api.backups.backup_engine")
    async def test_validate_backup_with_issues(
        self, mock_engine, async_client: AsyncClient, db_session
    ):
        """Test validating backup that has issues."""
        # Create target and backup
        target = BackupTarget(
            name="test-target",
            target_type="volume",
            volume_name="missing-volume",
            enabled=True,
        )
        db_session.add(target)
        await db_session.commit()
        await db_session.refresh(target)

        backup = Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.PENDING,
            backup_metadata={"target_name": "test-target"},
        )
        db_session.add(backup)
        await db_session.commit()
        await db_session.refresh(backup)

        # Mock validation returns issues
        mock_engine.validate_backup_prerequisites = AsyncMock(
            return_value=["Volume 'missing-volume' not found", "Low disk space"]
        )

        response = await async_client.post(f"/api/v1/backups/{backup.id}/validate")
        assert response.status_code == 200

        data = response.json()
        assert data["valid"] is False
        assert len(data["issues"]) == 2
        assert "Volume 'missing-volume' not found" in data["issues"]
        assert "Low disk space" in data["issues"]

    async def test_validate_backup_not_found(self, async_client: AsyncClient):
        """Test validating non-existent backup."""
        response = await async_client.post("/api/v1/backups/99999/validate")
        assert response.status_code == 404
        assert "Backup not found" in response.json()["detail"]
