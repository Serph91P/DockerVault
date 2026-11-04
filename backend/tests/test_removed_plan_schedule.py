"""Schedule deletion must unlink historical plans without deleting recovery data."""

import io
import tarfile

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from app.config import settings
from app.database import Base, Backup, BackupLog, BackupStatus, BackupTarget, Schedule
from app.encryption import encrypt_backup, generate_key_pair


@pytest.fixture
async def test_engine(tmp_path):
    engine = create_async_engine(f"sqlite+aiosqlite:///{tmp_path}/schedule.sqlite")
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all)
        assert (await connection.execute(text("PRAGMA foreign_keys"))).scalar() == 1
    yield engine
    await engine.dispose()


@pytest.mark.asyncio
@pytest.mark.parametrize("active_count", [0, 1], ids=["historical-only", "mixed"])
async def test_schedule_delete_unlinks_all_plans_and_keeps_encrypted_history(
    async_client, test_db, tmp_path, monkeypatch, active_count
):
    source = tmp_path / "restore"
    source.mkdir()
    archive = tmp_path / "archive.tar.gz"
    payload = b"retained after schedule deletion"
    with tarfile.open(archive, "w:gz") as tar:
        member = tarfile.TarInfo("payload.txt")
        member.size = len(payload)
        tar.addfile(member, io.BytesIO(payload))
    pair = await generate_key_pair()
    encrypted = await encrypt_backup(archive, pair.public_key)
    archive_bytes = encrypted.encrypted_path.read_bytes()
    key_bytes = encrypted.key_path.read_bytes()
    monkeypatch.setattr(settings, "BACKUP_BASE_PATH", str(tmp_path))

    async with test_db() as db:
        schedule = Schedule(name="shared schedule", cron_expression="0 2 * * *")
        db.add(schedule)
        await db.flush()
        historical = BackupTarget(
            name="historical plan",
            target_type="path",
            host_path=str(source),
            stop_container=False,
            schedule_id=schedule.id,
        )
        active = [
            BackupTarget(
                name=f"active {index}", target_type="path", schedule_id=schedule.id
            )
            for index in range(active_count)
        ]
        db.add_all([historical, *active])
        await db.flush()
        backup = Backup(
            target_id=historical.id,
            status=BackupStatus.COMPLETED,
            encrypted=True,
            file_path=str(encrypted.encrypted_path),
            encryption_key_path=str(encrypted.key_path),
        )
        db.add(backup)
        await db.flush()
        db.add(BackupLog(backup_id=backup.id, step="complete", message="retained log"))
        await db.commit()
        target_id, schedule_id, backup_id = historical.id, schedule.id, backup.id
        active_ids = [target.id for target in active]

    assert (
        await async_client.delete(f"/api/v1/targets/{target_id}")
    ).status_code == 200
    detail = (await async_client.get(f"/api/v1/schedules/{schedule_id}")).json()
    assert detail["target_count"] == active_count
    assert {target["id"] for target in detail["targets"]} == set(active_ids)
    response = await async_client.delete(f"/api/v1/schedules/{schedule_id}")
    assert response.status_code == 200, response.text
    assert response.json()["unlinked_targets"] == active_count + 1
    assert (
        await async_client.delete(f"/api/v1/schedules/{schedule_id}")
    ).status_code == 404

    async with test_db() as db:
        assert await db.get(Schedule, schedule_id) is None
        for identifier in [target_id, *active_ids]:
            target = await db.get(BackupTarget, identifier)
            assert target is not None and target.schedule_id is None
            assert bool(target.removed_at) == (identifier == target_id)
        assert await db.get(Backup, backup_id) is not None
        assert (await db.execute(text("PRAGMA foreign_key_check"))).all() == []
    assert encrypted.encrypted_path.read_bytes() == archive_bytes
    assert encrypted.key_path.read_bytes() == key_bytes
    assert (await async_client.get(f"/api/v1/backups/{backup_id}")).json()[
        "target_name"
    ] == "historical plan"
    assert (
        "retained log"
        in (await async_client.get(f"/api/v1/backups/{backup_id}/logs")).text
    )
    files = await async_client.post(
        f"/api/v1/backups/{backup_id}/files", json={"private_key": pair.private_key}
    )
    assert files.status_code == 200 and "payload.txt" in files.text
    restore = await async_client.post(
        f"/api/v1/backups/{backup_id}/restore", json={"private_key": pair.private_key}
    )
    assert restore.status_code == 200, restore.text
    assert (source / "payload.txt").read_bytes() == payload
