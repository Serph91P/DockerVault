from pathlib import Path
import pytest
from app.database import (
    Backup,
    BackupStatus,
    BackupTarget,
    BackupStorageSync,
    RemoteStorage,
)


@pytest.mark.asyncio
async def test_history_distinguishes_confirmed_local_deletion_from_missing_file(
    async_client, db_session, tmp_path
):
    local = tmp_path / "retained.tar"
    local.write_bytes(b"owned archive")
    target = BackupTarget(name="Retained", target_type="path", host_path="/fixture")
    remote = RemoteStorage(
        name="Offsite",
        storage_type="local",
        base_path=str(tmp_path / "remote"),
        enabled=True,
    )
    db_session.add_all([target, remote])
    await db_session.flush()
    backup = Backup(
        target_id=target.id,
        status=BackupStatus.COMPLETED,
        file_path=str(local),
        file_size=13,
    )
    db_session.add(backup)
    await db_session.flush()
    db_session.add(
        BackupStorageSync(
            backup_id=backup.id,
            storage_id=remote.id,
            sync_status="completed",
            remote_path="retained.tar",
        )
    )
    await db_session.commit()
    preview = await async_client.post(
        "/api/v1/backups/removal/preview",
        json={"scope": "local", "backup_ids": [backup.id]},
    )
    assert preview.status_code == 200
    removed = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": preview.json()["token"]}
    )
    assert removed.status_code == 200
    assert removed.json()["succeeded"] == 1
    assert not Path(local).exists()
    detail = (await async_client.get(f"/api/v1/backups/history/{backup.id}")).json()
    assert detail["local_state"] == "deleted"
    assert detail["can_restore"] is True
    assert detail["remote_sync_details"][0]["status"] == "completed"
