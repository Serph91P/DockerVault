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
        history = await async_client.get("/api/v1/targets?include_removed=true")
        assert any(
            plan["id"] == target_id and plan["removed_at"] for plan in history.json()
        )
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


@pytest.mark.asyncio
async def test_removal_between_api_validation_and_creation_returns_conflict(
    async_client, test_db, monkeypatch
):
    from app.backup_engine import backup_engine

    async with test_db() as db:
        target = BackupTarget(name="racing", target_type="path", host_path="/unused")
        db.add(target)
        await db.commit()
        target_id = target.id
    original = backup_engine.create_backup

    async def remove_before_create(target, backup_type):
        assert (
            await async_client.delete(f"/api/v1/targets/{target_id}")
        ).status_code == 200
        return await original(target, backup_type)

    monkeypatch.setattr(backup_engine, "create_backup", remove_before_create)
    response = await async_client.post("/api/v1/backups", json={"target_id": target_id})
    assert response.status_code == 409
    assert (await async_client.get("/api/v1/backups")).json() == []


@pytest.mark.asyncio
async def test_removed_plan_is_not_an_assigned_schedule_target(async_client, test_db):
    from app.database import Schedule

    async with test_db() as db:
        schedule = Schedule(name="active schedule", cron_expression="0 2 * * *")
        db.add(schedule)
        await db.flush()
        target = BackupTarget(
            name="old scheduled plan", target_type="path", schedule_id=schedule.id
        )
        db.add(target)
        await db.commit()
        target_id, schedule_id = target.id, schedule.id
    assert (
        await async_client.delete(f"/api/v1/targets/{target_id}")
    ).status_code == 200
    response = await async_client.get(f"/api/v1/schedules/{schedule_id}")
    assert response.json()["target_count"] == 0 and response.json()["targets"] == []


@pytest.mark.asyncio
async def test_removed_plan_cannot_be_edited_started_or_retained(async_client, test_db):
    from app.backup_engine import backup_engine
    from app.retention import retention_manager
    from app.scheduler import BackupScheduler

    async with test_db() as db:
        target = BackupTarget(name="removed", target_type="path", host_path="/unused")
        db.add(target)
        await db.commit()
        target_id = target.id
    assert (
        await async_client.delete(f"/api/v1/targets/{target_id}")
    ).status_code == 200
    assert (await async_client.get(f"/api/v1/targets/{target_id}")).status_code == 404
    assert (
        await async_client.put(f"/api/v1/targets/{target_id}", json={"enabled": True})
    ).status_code == 404
    assert (
        await async_client.post("/api/v1/backups", json={"target_id": target_id})
    ).status_code == 404
    assert not await BackupScheduler().trigger_backup_now(target_id)
    async with test_db() as db:
        historical = await db.get(BackupTarget, target_id)
        assert historical is not None and historical.removed_at is not None
        with pytest.raises(ValueError, match="removed"):
            await backup_engine.create_backup(historical)
    stats = await retention_manager.apply_retention(target_id)
    assert stats["deleted"] == 0 and stats["error"] == "Plan removed"
    assert (await async_client.delete(f"/api/v1/targets/{target_id}")).json()[
        "status"
    ] == "already_removed"
