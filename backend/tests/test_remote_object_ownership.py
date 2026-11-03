"""R1/R2: concrete remote object ownership and suffix-exact sidecar recovery."""

import asyncio
from pathlib import Path
import shutil
from unittest.mock import patch

import pytest
from sqlalchemy import select
from sqlalchemy.ext.asyncio import create_async_engine

from app.backup_engine import backup_engine
from app.database import (
    Base,
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
)
from app.remote_storage import LocalStorage, storage_manager
from tests.test_manual_sync import sync_catalog  # noqa: F401


@pytest.fixture
async def test_engine(tmp_path):
    engine = create_async_engine(f"sqlite+aiosqlite:///{tmp_path}/objects.sqlite")
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all)
    yield engine
    await engine.dispose()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "target_name,stem,custom_key",
    [
        ("vault", "archive", False),
        ("vault.enc", "archive", False),
        ("vault", "archive.enc", False),
        ("vault.enc", "archive.enc", True),
    ],
)
async def test_encrypted_sync_recovers_exact_archive_and_sidecar(
    async_client, test_db, sync_catalog, target_name, stem, custom_key
):
    from app.api.backups import _ensure_backup_locally, _download_key_from_remote

    backup_id, target_id, storage_id, archive, key, remote = sync_catalog
    archive = archive.rename(archive.with_name(stem + ".tar.enc"))
    key = key.rename(
        key.with_name("different-local.key" if custom_key else stem + ".tar.key")
    )
    expected_archive, expected_key = archive.read_bytes(), key.read_bytes()
    async with test_db() as session:
        target = await session.get(BackupTarget, target_id)
        target.name = target_name
        backup = await session.get(Backup, backup_id)
        backup.file_path, backup.encryption_key_path = str(archive), str(key)
        await session.commit()
    response = await async_client.post(f"/api/v1/storage/{storage_id}/sync/{backup_id}")
    assert response.status_code == 200, response.text
    remote_path = response.json()["remote_path"]
    assert (remote / remote_path).read_bytes() == expected_archive
    assert (
        remote / (remote_path.removesuffix(".enc") + ".key")
    ).read_bytes() == expected_key
    archive.unlink()
    key.unlink()
    storage_manager.backends.clear()
    storage_manager.configs.clear()
    restored, temporary = await _ensure_backup_locally(backup)
    try:
        assert Path(restored).read_bytes() == expected_archive
        recovered_key = await _download_key_from_remote(backup, restored)
        assert recovered_key is not None
        assert Path(recovered_key).read_bytes() == expected_key
    finally:
        if temporary:
            shutil.rmtree(temporary)
            assert not Path(temporary).exists()


@pytest.mark.asyncio
@pytest.mark.parametrize("collision", ["archive", "sidecar"])
async def test_concurrent_normalized_targets_cannot_overwrite_remote_identity(
    async_client, test_db, tmp_path, monkeypatch, collision
):
    archives, keys = [], []
    for index in range(2):
        directory = tmp_path / str(index)
        directory.mkdir()
        suffix = ".key" if index == 1 and collision == "sidecar" else ".enc"
        archive = directory / ("snapshot.tar" + suffix)
        archive.write_bytes(f"archive identity {index}".encode())
        archives.append(archive)
        key = directory / "snapshot.tar.key"
        if suffix == ".enc":
            key.write_bytes(f"key identity {index}".encode())
        keys.append(key)
    remote = tmp_path / "remote"
    async with test_db() as session:
        targets = [
            BackupTarget(name=name, target_type="path", host_path=str(path.parent))
            for name, path in zip(("reports?", "reports*"), archives)
        ]
        storage = RemoteStorage(
            name="owned", storage_type="local", base_path=str(remote)
        )
        session.add_all([*targets, storage])
        await session.flush()
        backups = [
            Backup(
                target_id=target.id,
                status=BackupStatus.COMPLETED,
                file_path=str(path),
                encrypted=index == 0 or collision == "archive",
                encryption_key_path=str(keys[index])
                if index == 0 or collision == "archive"
                else None,
            )
            for index, (target, path) in enumerate(zip(targets, archives))
        ]
        session.add_all(backups)
        await session.commit()
        ids, storage_id = [b.id for b in backups], storage.id
    assert backup_engine._sanitize_path_name(
        targets[0].name
    ) == backup_engine._sanitize_path_name(targets[1].name)
    entered, release, contention = asyncio.Event(), asyncio.Event(), asyncio.Event()

    class ObservedLock(asyncio.Lock):
        async def acquire(self):
            if self.locked():
                contention.set()
            return await super().acquire()

    class Registry(dict):
        def setdefault(self, key, default=None):
            return super().setdefault(key, ObservedLock())

    monkeypatch.setattr(backup_engine, "_remote_sync_locks", Registry(), raising=False)
    original = LocalStorage.upload

    async def upload(self, source, destination):
        if source == archives[0]:
            entered.set()
            await release.wait()
        return await original(self, source, destination)

    monkeypatch.setattr(LocalStorage, "upload", upload)
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
        patch("app.config.settings.BACKUP_BASE_PATH", str(tmp_path)),
    ):
        first = asyncio.create_task(
            async_client.post(f"/api/v1/storage/{storage_id}/sync/{ids[0]}")
        )
        second = observer = None
        try:
            await asyncio.wait_for(entered.wait(), 10)
            second = asyncio.create_task(
                async_client.post(f"/api/v1/storage/{storage_id}/sync/{ids[1]}")
            )
            observer = asyncio.create_task(contention.wait())
            done, _ = await asyncio.wait(
                (second, observer), timeout=10, return_when=asyncio.FIRST_COMPLETED
            )
            assert done, (
                "Neither contender completed nor awaited the shared object claim"
            )
            release.set()
            response_a, response_b = await asyncio.wait_for(
                asyncio.gather(first, second), 10
            )
            assert response_a.status_code == 200, response_a.text
            assert response_b.status_code == 409, response_b.text
            async with test_db() as session:
                rows = (await session.scalars(select(BackupStorageSync))).all()
            assert (
                len(rows) == 1
                and rows[0].backup_id == ids[0]
                and rows[0].sync_status == "completed"
            )
            assert (remote / rows[0].remote_path).read_bytes() == b"archive identity 0"
            assert (
                remote / (rows[0].remote_path.removesuffix(".enc") + ".key")
            ).read_bytes() == b"key identity 0"
        finally:
            release.set()
            await asyncio.gather(
                *(task for task in (first, second) if task), return_exceptions=True
            )
            if observer:
                observer.cancel()
                await asyncio.gather(observer, return_exceptions=True)
