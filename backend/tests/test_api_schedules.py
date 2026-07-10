"""Tests for schedules API endpoints."""

import pytest
from httpx import AsyncClient

from app.database import BackupTarget
from app.scheduler import BackupScheduler


@pytest.mark.asyncio
async def test_trigger_backup_rejects_disabled_target(
    async_client: AsyncClient, db_session, monkeypatch
):
    """Test manual backup trigger rejects a disabled target."""
    target = BackupTarget(
        name="disabled-target",
        target_type="volume",
        volume_name="test-volume",
        enabled=False,
    )
    db_session.add(target)
    await db_session.commit()
    await db_session.refresh(target)

    from app.main import app

    monkeypatch.setattr(app.state, "scheduler", BackupScheduler(), raising=False)

    response = await async_client.post(f"/api/v1/schedules/target/{target.id}/trigger")

    assert response.status_code == 409
    assert response.json() == {"detail": "Target is disabled"}
