"""Retry the existing archive only when the full recovery copy can be verified."""

import asyncio
import hashlib
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
from app.backup_engine import backup_engine


@pytest.fixture
async def retry_catalog(test_db, tmp_path):
    archive, key = tmp_path / "archive.tar.enc", tmp_path / "archive.tar.key"
    archive.write_bytes(b"owned ciphertext")
    key.write_bytes(b"owned sidecar")
    async with test_db() as db:
        storage = RemoteStorage(
            name="owned", storage_type="local", base_path=str(tmp_path / "remote")
        )
        target = BackupTarget(name="owned", target_type="path", host_path=str(tmp_path))
        db.add_all([target, storage])
        await db.flush()
        target.remote_storage_ids = [storage.id]
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(archive),
            file_size=archive.stat().st_size,
            checksum=hashlib.sha256(archive.read_bytes()).hexdigest(),
            encrypted=True,
            encryption_key_path=str(key),
        )
        db.add(backup)
        await db.flush()
        db.add(
            BackupStorageSync(
                backup_id=backup.id,
                storage_id=storage.id,
                remote_path="owned/archive.tar.enc",
                sync_status="failed",
                error_message="original provider denial",
            )
        )
        await db.commit()
        ids = backup.id, storage.id
    with (
        patch.dict(backup_engine._target_locks, {}, clear=True),
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
        patch("app.config.settings.BACKUP_BASE_PATH", str(tmp_path)),
    ):
        yield *ids, archive, key, tmp_path / "remote"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "failure", ["missing_key", "key_upload", "disabled_warm", "disabled_cold"]
)
async def test_retry_never_completes_unrecoverable_copy(
    async_client, test_db, retry_catalog, monkeypatch, failure
):
    backup_id, storage_id, archive, key, remote = retry_catalog
    if failure == "missing_key":
        key.unlink()
    elif failure.startswith("disabled"):
        from app.api.storage import _db_to_config

        async with test_db() as db:
            storage = await db.get(RemoteStorage, storage_id)
            if failure == "disabled_warm":
                storage_manager.add_storage(_db_to_config(storage))
            storage.enabled = False
            await db.commit()
    else:
        original = LocalStorage.upload

        async def upload(self, source, destination):
            return (
                (False, "owned sidecar failure")
                if source == key
                else await original(self, source, destination)
            )

        monkeypatch.setattr(LocalStorage, "upload", upload)
    response = await async_client.post(f"/api/v1/backups/{backup_id}/retry-sync")
    assert response.status_code >= 400 or response.json()["succeeded"] == 0, (
        response.text
    )
    async with test_db() as db:
        row = (await db.execute(select(BackupStorageSync))).scalar_one()
        assert row.sync_status == "failed" and row.error_message
    assert archive.exists()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "case", ["verified_remote_only", "archive_failure", "tampered", "no_checksum"]
)
async def test_failed_backup_retry_requires_proven_remote_only_failure(
    async_client, test_db, retry_catalog, case
):
    backup_id, storage_id, archive, key, remote = retry_catalog
    async with test_db() as db:
        backup = await db.get(Backup, backup_id)
        backup.status = BackupStatus.FAILED
        backup.error_message = (
            "Remote sync failed: original provider denial"
            if case != "archive_failure"
            else "Archive creation failed"
        )
        if case == "no_checksum":
            backup.checksum = None
        await db.commit()
    if case == "tampered":
        archive.write_bytes(b"corrupt archive")
    response = await async_client.post(f"/api/v1/backups/{backup_id}/retry-sync")
    if case == "verified_remote_only":
        assert response.status_code == 200 and response.json()["succeeded"] == 1, (
            response.text
        )
        assert (remote / "owned/archive.tar.enc").read_bytes() == archive.read_bytes()
        assert (remote / "owned/archive.tar.key").read_bytes() == key.read_bytes()
        async with test_db() as db:
            backup = await db.get(Backup, backup_id)
            assert backup.status == BackupStatus.COMPLETED
            assert backup.backup_metadata["recovered_remote_sync_error"].startswith(
                "Remote sync failed:"
            )
    else:
        assert response.status_code == 409, response.text
        assert not remote.exists()


@pytest.mark.asyncio
async def test_concurrent_missing_sync_retries_are_idempotent(
    async_client, test_db, retry_catalog
):
    backup_id, storage_id, archive, key, remote = retry_catalog
    async with test_db() as db:
        row = (await db.execute(select(BackupStorageSync))).scalar_one()
        await db.delete(row)
        await db.commit()
    responses = await asyncio.gather(
        *(
            async_client.post(f"/api/v1/backups/{backup_id}/retry-sync")
            for _ in range(3)
        )
    )
    assert any(
        response.status_code == 200 and response.json()["succeeded"] == 1
        for response in responses
    )
    async with test_db() as db:
        row = (await db.execute(select(BackupStorageSync))).scalar_one()
        assert row.sync_status == "completed" and row.error_message is None
    assert (remote / "owned/archive.tar.enc").read_bytes() == archive.read_bytes()
    assert (remote / "owned/archive.tar.key").read_bytes() == key.read_bytes()


@pytest.mark.asyncio
async def test_retry_does_not_overwrite_other_backup_identity(
    async_client, test_db, retry_catalog
):
    backup_id, storage_id, archive, key, remote = retry_catalog
    destination = remote / "owned/archive.tar.enc"
    destination.parent.mkdir(parents=True)
    destination.write_bytes(b"other identity")
    async with test_db() as db:
        original = await db.get(Backup, backup_id)
        other = Backup(target_id=original.target_id, status=BackupStatus.COMPLETED)
        db.add(other)
        await db.flush()
        db.add(
            BackupStorageSync(
                backup_id=other.id,
                storage_id=storage_id,
                remote_path="owned/archive.tar.enc",
                sync_status="completed",
            )
        )
        await db.commit()
    response = await async_client.post(f"/api/v1/backups/{backup_id}/retry-sync")
    assert response.status_code >= 400 or response.json()["succeeded"] == 0
    assert destination.read_bytes() == b"other identity"


@pytest.mark.asyncio
async def test_retry_preserves_enc_directories_and_ignores_noncolliding_key_names(
    async_client, test_db, retry_catalog
):
    from app.api.backups import _ensure_backup_locally, _download_key_from_remote
    import shutil

    backup_id, storage_id, archive, key, remote = retry_catalog
    archive = archive.rename(archive.with_name("archive.enc.tar.enc"))
    key = key.rename(key.with_name("different-local.key"))
    foreign_path = "owned.key/archive.key.tar.key"
    foreign = remote / foreign_path
    foreign.parent.mkdir(parents=True)
    foreign.write_bytes(b"foreign identity remains")
    async with test_db() as session:
        backup = await session.get(Backup, backup_id)
        target = await session.get(BackupTarget, backup.target_id)
        target.name = "owned.enc"
        backup.file_path, backup.encryption_key_path = str(archive), str(key)
        other = Backup(
            target_id=target.id, status=BackupStatus.COMPLETED, encrypted=False
        )
        session.add(other)
        await session.flush()
        session.add(
            BackupStorageSync(
                backup_id=other.id,
                storage_id=storage_id,
                remote_path=foreign_path,
                sync_status="completed",
            )
        )
        await session.commit()
    response = await async_client.post(f"/api/v1/backups/{backup_id}/retry-sync")
    assert response.status_code == 200 and response.json()["succeeded"] == 1, (
        response.text
    )
    assert foreign.read_bytes() == b"foreign identity remains"
    assert (
        remote / "owned.enc/archive.enc.tar.enc"
    ).read_bytes() == b"owned ciphertext"
    assert (remote / "owned.enc/archive.enc.tar.key").read_bytes() == b"owned sidecar"
    archive.unlink()
    key.unlink()
    storage_manager.backends.clear()
    storage_manager.configs.clear()
    restored, temporary = await _ensure_backup_locally(backup)
    try:
        assert Path(restored).read_bytes() == b"owned ciphertext"
        restored_key = await _download_key_from_remote(backup, restored)
        assert restored_key and Path(restored_key).read_bytes() == b"owned sidecar"
    finally:
        if temporary:
            shutil.rmtree(temporary)
