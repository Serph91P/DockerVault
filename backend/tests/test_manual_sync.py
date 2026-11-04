"""Manual sync must create a verified, durable recovery copy, not just a 200."""

import asyncio
import shutil
from pathlib import Path
from unittest.mock import patch

import pytest
from sqlalchemy import select

from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
)
from app.remote_storage import LocalStorage, storage_manager


@pytest.fixture
async def sync_catalog(test_db, tmp_path):
    archive = tmp_path / "archive.tar.enc"
    archive.write_bytes(b"owned synthetic encrypted archive")
    key = tmp_path / "archive.tar.key"
    key.write_bytes(b"owned synthetic key")
    async with test_db() as db:
        target = BackupTarget(
            name="../unsafe / target", target_type="path", host_path=str(tmp_path)
        )
        storage = RemoteStorage(
            name="owned", storage_type="local", base_path=str(tmp_path / "remote")
        )
        db.add_all([target, storage])
        await db.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(archive),
            encrypted=True,
            encryption_key_path=str(key),
        )
        db.add(backup)
        await db.commit()
        ids = backup.id, target.id, storage.id
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
        patch("app.config.settings.BACKUP_BASE_PATH", str(tmp_path)),
    ):
        yield *ids, archive, key, tmp_path / "remote"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "failure",
    [
        "missing",
        "disabled",
        "incomplete",
        "missing_key",
        "false",
        "exception",
        "key_failure",
        "corrupt",
    ],
)
async def test_manual_sync_rejects_unrecoverable_copy(
    async_client, test_db, sync_catalog, monkeypatch, failure
):
    backup_id, target_id, storage_id, archive, key, remote = sync_catalog
    if failure == "missing":
        archive.unlink()
    elif failure == "missing_key":
        key.unlink()
    elif failure in ("disabled", "incomplete"):
        async with test_db() as db:
            if failure == "disabled":
                storage = await db.get(RemoteStorage, storage_id)
                storage.enabled = False
                storage_manager.add_storage(
                    __import__(
                        "app.api.storage", fromlist=["_db_to_config"]
                    )._db_to_config(storage)
                )
            else:
                backup = await db.get(Backup, backup_id)
                backup.status = BackupStatus.RUNNING
            await db.commit()
    else:
        original = LocalStorage.upload

        async def transfer(self, source, destination):
            if failure == "exception":
                raise OSError("owned failure")
            if failure == "false" or (failure == "key_failure" and source == key):
                return False, "owned failure"
            result = await original(self, source, destination)
            if failure == "corrupt":
                self._safe_resolve(destination).write_bytes(
                    b"x" * source.stat().st_size
                )
            return result

        monkeypatch.setattr(LocalStorage, "upload", transfer)
    response = await async_client.post(f"/api/v1/storage/{storage_id}/sync/{backup_id}")
    assert response.status_code >= 400, response.text
    async with test_db() as db:
        rows = (await db.execute(select(BackupStorageSync))).scalars().all()
        assert all(row.sync_status != "completed" for row in rows)
        if failure in ("false", "exception", "key_failure", "corrupt"):
            assert len(rows) == 1 and rows[0].error_message


@pytest.mark.asyncio
async def test_manual_sync_durable_idempotent_remote_fallback(
    async_client, test_db, sync_catalog
):
    from app.api.backups import _ensure_backup_locally, _download_key_from_remote

    backup_id, target_id, storage_id, archive, key, remote = sync_catalog
    expected_archive, expected_key = archive.read_bytes(), key.read_bytes()
    endpoint = f"/api/v1/storage/{storage_id}/sync/{backup_id}"
    responses = await asyncio.gather(
        async_client.post(endpoint), async_client.post(endpoint)
    )
    assert all(response.status_code == 200 for response in responses), [
        r.text for r in responses
    ]
    assert responses[0].json()["remote_path"] == responses[1].json()["remote_path"]
    async with test_db() as db:
        rows = (await db.execute(select(BackupStorageSync))).scalars().all()
        assert len(rows) == 1 and rows[0].sync_status == "completed"
        assert rows[0].synced_at is not None
        path = rows[0].remote_path
        assert ".." not in Path(path).parts and not Path(path).is_absolute()
        backup = await db.get(Backup, backup_id)
    assert (remote / path).read_bytes() == expected_archive
    assert (remote / path.replace(".enc", ".key")).read_bytes() == expected_key
    archive.unlink()
    key.unlink()
    storage_manager.backends.clear()
    storage_manager.configs.clear()
    restored, temporary = await _ensure_backup_locally(backup)
    try:
        assert Path(restored).read_bytes() == expected_archive
        restored_key = await _download_key_from_remote(backup, restored)
        assert Path(restored_key).read_bytes() == expected_key
    finally:
        if temporary:
            shutil.rmtree(temporary)


@pytest.mark.asyncio
async def test_manual_sync_cannot_overwrite_another_catalog_identity(
    async_client, test_db, sync_catalog
):
    backup_id, target_id, storage_id, archive, key, remote = sync_catalog
    assert (
        await async_client.post(f"/api/v1/storage/{storage_id}/sync/{backup_id}")
    ).status_code == 200
    async with test_db() as db:
        other = Backup(
            target_id=target_id, status=BackupStatus.COMPLETED, file_path=str(archive)
        )
        db.add(other)
        await db.commit()
        other_id = other.id
    archive.write_bytes(b"different backup identity")
    response = await async_client.post(f"/api/v1/storage/{storage_id}/sync/{other_id}")
    assert response.status_code == 409
    async with test_db() as db:
        row = (await db.execute(select(BackupStorageSync))).scalar_one()
        assert (
            remote / row.remote_path
        ).read_bytes() == b"owned synthetic encrypted archive"
