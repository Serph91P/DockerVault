"""Removing an automation plan must preserve discoverable, restorable archives."""

import io
import tarfile
from unittest.mock import patch

import pytest

from app.config import settings
from app.database import (
    Backup,
    BackupLog,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
)
from app.remote_storage import storage_manager


@pytest.mark.asyncio
@pytest.mark.parametrize("copies", ["local", "mixed", "remote"])
async def test_plan_removal_preserves_archive_and_actual_restore(
    async_client, test_db, tmp_path, monkeypatch, copies
):
    """Catch target deletion cascading into archive loss or blocking safe removal."""
    source = tmp_path / "source"
    source.mkdir()
    archive = tmp_path / "archive.tar.gz"
    payload = b"synthetic retained backup content"
    with tarfile.open(archive, "w:gz") as handle:
        member = tarfile.TarInfo("payload.txt")
        member.size = len(payload)
        handle.addfile(member, io.BytesIO(payload))
    remote = tmp_path / "remote"
    remote.mkdir()
    if copies != "local":
        (remote / archive.name).write_bytes(archive.read_bytes())
    if copies == "remote":
        archive.unlink()

    async with test_db() as db:
        target = BackupTarget(
            name="retained history",
            target_type="path",
            host_path=str(source),
            stop_container=False,
        )
        db.add(target)
        await db.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=None if copies == "remote" else str(archive),
        )
        db.add(backup)
        await db.flush()
        db.add(BackupLog(backup_id=backup.id, step="complete", message="retained log"))
        if copies != "local":
            storage = RemoteStorage(
                name="retained copy",
                storage_type="local",
                base_path=str(remote),
            )
            db.add(storage)
            await db.flush()
            db.add(
                BackupStorageSync(
                    backup_id=backup.id,
                    storage_id=storage.id,
                    remote_path=archive.name,
                    sync_status="completed",
                )
            )
        await db.commit()
        target_id, backup_id = target.id, backup.id

    monkeypatch.setattr(settings, "BACKUP_BASE_PATH", str(tmp_path))
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
    ):
        response = await async_client.delete(f"/api/v1/targets/{target_id}")
        assert response.status_code == 200, response.text
        if copies != "remote":
            assert archive.exists(), "Plan removal deleted a retained local archive"
        if copies != "local":
            assert (remote / archive.name).exists()

        plans = await async_client.get("/api/v1/targets")
        assert plans.status_code == 200
        assert target_id not in {plan["id"] for plan in plans.json()}
        detail = await async_client.get(f"/api/v1/backups/{backup_id}")
        assert detail.status_code == 200
        assert detail.json()["target_name"] == "retained history"
        logs = await async_client.get(f"/api/v1/backups/{backup_id}/logs")
        assert logs.status_code == 200 and "retained log" in logs.text
        restored = await async_client.post(
            f"/api/v1/backups/{backup_id}/restore", json={}
        )
        assert restored.status_code == 200, restored.text
        assert (source / "payload.txt").read_bytes() == payload
