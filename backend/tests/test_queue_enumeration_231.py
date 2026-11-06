"""Real pinned Docker SDK enumeration and accepted-job terminal failure contracts."""

import asyncio
from unittest.mock import AsyncMock, Mock

import docker
import pytest
from docker.errors import APIError, NotFound

from app.docker_client import DockerClientWrapper
from app.backup_engine import BackupEngine
from app.database import Backup, BackupStatus, BackupTarget


@pytest.mark.asyncio
@pytest.mark.parametrize("error", [NotFound("removed"), APIError("permission denied")])
async def test_sdk_enumeration_ignores_only_objects_removed_during_inspect(
    monkeypatch, error
):
    client = docker.DockerClient(
        base_url="unix:///unused-owned-test.sock", version="1.45"
    )
    monkeypatch.setattr(
        client.api, "containers", Mock(return_value=[{"Id": "gone"}, {"Id": "kept"}])
    )
    attrs = {
        "Id": "kept",
        "Name": "/kept",
        "Config": {"Image": "owned", "Labels": {}},
        "State": {"Status": "running"},
    }
    monkeypatch.setattr(
        client.api,
        "inspect_container",
        Mock(
            side_effect=lambda identifier: (
                (_ for _ in ()).throw(error) if identifier == "gone" else attrs
            )
        ),
    )
    wrapper = DockerClientWrapper()
    wrapper._client = client
    try:
        if isinstance(error, NotFound):
            rows = await wrapper.list_containers()
            assert [row.id for row in rows] == ["kept"]
            # Explicitly requested identities still use strict native get/inspect.
            with pytest.raises(NotFound):
                await wrapper.get_container_identity("gone")
        else:
            with pytest.raises(APIError, match="permission denied"):
                await wrapper.list_containers()
    finally:
        client.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("cancel", [True, False])
async def test_queued_preflight_failure_is_terminal_without_swallowing_cancellation(
    test_db, monkeypatch, cancel
):
    async with test_db() as session:
        target = BackupTarget(
            name="owned queue", target_type="path", host_path="/owned"
        )
        session.add(target)
        await session.flush()
        row = Backup(target_id=target.id, status=BackupStatus.PENDING)
        session.add(row)
        await session.commit()
        identifier = row.id
    from app import backup_engine as module

    monkeypatch.setattr(module, "async_session", test_db)
    engine = BackupEngine()
    error = asyncio.CancelledError() if cancel else APIError("owned enumeration denied")
    monkeypatch.setattr(engine, "run_backup", AsyncMock(side_effect=error))
    engine._queue.put_nowait(identifier)
    assert await engine._queue.get() == identifier
    if cancel:
        with pytest.raises(asyncio.CancelledError):
            await engine._run_queued_backup(identifier)
    else:
        await engine._run_queued_backup(identifier)
    await asyncio.wait_for(engine._queue.join(), timeout=1)
    async with test_db() as session:
        row = await session.get(Backup, identifier)
        assert row.status == (BackupStatus.PENDING if cancel else BackupStatus.FAILED)
        if not cancel:
            assert "owned enumeration denied" in row.error_message
            assert row.completed_at is not None
