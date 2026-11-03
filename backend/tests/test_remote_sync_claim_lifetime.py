"""Concrete remote ownership preserves independent concurrency and cancellation cleanup."""

import asyncio
from pathlib import Path

import pytest

from app.backup_engine import backup_engine
from app.database import Backup, BackupStatus, BackupTarget
from app.remote_storage import LocalStorage
from tests.test_manual_sync import sync_catalog  # noqa: F401
from tests.test_remote_object_ownership import test_engine  # noqa: F401


@pytest.mark.asyncio
async def test_independent_remote_paths_do_not_wait_for_unrelated_upload(
    async_client, test_db, sync_catalog, monkeypatch
):
    backup_id, target_id, storage_id, archive, key, remote = sync_catalog
    async with test_db() as session:
        target = await session.get(BackupTarget, target_id)
        first_path = f"{backup_engine._sanitize_path_name(target.name)}/{archive.name}"
        other_target = BackupTarget(name="independent", target_type="path")
        session.add(other_target)
        await session.flush()
        other = Backup(
            target_id=other_target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(archive),
            encrypted=True,
            encryption_key_path=str(key),
        )
        session.add(other)
        await session.commit()
        other_id = other.id
    entered, release = asyncio.Event(), asyncio.Event()
    original = LocalStorage.upload

    async def upload(self, source, destination):
        if destination == first_path:
            entered.set()
            await release.wait()
        return await original(self, source, destination)

    monkeypatch.setattr(LocalStorage, "upload", upload)
    first = asyncio.create_task(
        async_client.post(f"/api/v1/storage/{storage_id}/sync/{backup_id}")
    )
    try:
        await asyncio.wait_for(entered.wait(), 10)
        second = await asyncio.wait_for(
            async_client.post(f"/api/v1/storage/{storage_id}/sync/{other_id}"), 10
        )
        assert second.status_code == 200, second.text
        assert not first.done()
        assert (
            remote / second.json()["remote_path"]
        ).read_bytes() == archive.read_bytes()
    finally:
        release.set()
        first_response = await asyncio.wait_for(first, 10)
    assert first_response.status_code == 200
    assert (remote / first_path).read_bytes() == archive.read_bytes()


@pytest.mark.asyncio
async def test_cancelled_partial_object_claim_releases_archive_before_retry(
    async_client, test_db, sync_catalog, monkeypatch
):
    backup_id, target_id, storage_id, archive, key, remote = sync_catalog
    async with test_db() as session:
        target = await session.get(BackupTarget, target_id)
        remote_path = f"{backup_engine._sanitize_path_name(target.name)}/{archive.name}"
    key_path = remote_path.removesuffix(".enc") + ".key"
    waiting = asyncio.Event()

    class SidecarLock(asyncio.Lock):
        async def acquire(self):
            if self.locked():
                waiting.set()
            return await super().acquire()

    archive_lock, sidecar_lock = asyncio.Lock(), SidecarLock()
    await sidecar_lock.acquire()
    monkeypatch.setattr(
        backup_engine,
        "_remote_sync_locks",
        {(storage_id, remote_path): archive_lock, (storage_id, key_path): sidecar_lock},
    )
    task = asyncio.create_task(
        async_client.post(f"/api/v1/storage/{storage_id}/sync/{backup_id}")
    )
    try:
        await asyncio.wait_for(waiting.wait(), 10)
        assert archive_lock.locked()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert not archive_lock.locked()
        assert not [path for path in remote.rglob("*") if path.is_file()]
    finally:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
        sidecar_lock.release()
    retried = await asyncio.wait_for(
        async_client.post(f"/api/v1/storage/{storage_id}/sync/{backup_id}"), 10
    )
    assert retried.status_code == 200, retried.text
    assert (remote / remote_path).read_bytes() == archive.read_bytes()
    assert (remote / key_path).read_bytes() == Path(key).read_bytes()
