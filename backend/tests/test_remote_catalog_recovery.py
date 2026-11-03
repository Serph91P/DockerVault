"""Owner R3/R6: retained remote identity must really restore and delete conservatively."""

import io
from pathlib import Path
import tarfile
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
from app.remote_storage import storage_manager


@pytest.fixture
async def remote_catalog(test_db, tmp_path):
    target_path = tmp_path / "destination"
    target_path.mkdir()
    (target_path / "payload").write_bytes(b"damaged")
    archive = tmp_path / "archive.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        member = tarfile.TarInfo("payload")
        member.size = len(b"owned original")
        tar.addfile(member, io.BytesIO(b"owned original"))
    remote = tmp_path / "remote"
    remote.mkdir()
    (remote / archive.name).write_bytes(archive.read_bytes())
    key = tmp_path / "archive.key"
    key.write_bytes(b"owned wrapped key")
    async with test_db() as db:
        target = BackupTarget(
            name="remote lifecycle",
            target_type="path",
            host_path=str(target_path),
            stop_container=False,
        )
        storage = RemoteStorage(
            name="owned local", storage_type="local", base_path=str(remote)
        )
        db.add_all([target, storage])
        await db.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(archive),
            encryption_key_path=str(key),
        )
        db.add(backup)
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
        ids = backup.id, storage.id
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
        patch("app.config.settings.BACKUP_BASE_PATH", str(tmp_path)),
    ):
        yield *ids, archive, key, remote, target_path


@pytest.mark.asyncio
async def test_local_delete_then_actual_remote_restore(
    async_client, test_db, remote_catalog, monkeypatch
):
    backup_id, storage_id, archive, key, remote, target = remote_catalog
    downloads = archive.parent / "temporary-downloads"
    downloads.mkdir()
    monkeypatch.setattr("app.api.backups.tempfile.tempdir", str(downloads))
    response = await async_client.delete(
        f"/api/v1/backups/{backup_id}?delete_remote=false"
    )
    assert response.status_code == 200 and not archive.exists()
    restored = await async_client.post(f"/api/v1/backups/{backup_id}/restore", json={})
    assert restored.status_code == 200, restored.text
    assert (target / "payload").read_bytes() == b"owned original"
    assert not list(downloads.iterdir()), (
        "Restore must clean its downloaded working paths"
    )
    async with test_db() as db:
        backup = await db.get(Backup, backup_id)
        assert backup is not None and backup.file_path is None
        assert (
            await db.execute(select(BackupStorageSync))
        ).scalar_one().sync_status == "completed"
    assert (remote / archive.name).exists()


@pytest.mark.asyncio
@pytest.mark.parametrize("bulk", [False, True])
async def test_failed_sync_requires_actual_provider_delete(
    async_client, test_db, remote_catalog, monkeypatch, bulk
):
    from app.api.storage import _db_to_config

    backup_id, storage_id, archive, key, remote, target = remote_catalog
    async with test_db() as db:
        row = (await db.execute(select(BackupStorageSync))).scalar_one()
        row.sync_status = "failed"
        row.error_message = "Sidecar upload failed after archive upload"
        await db.commit()
        storage = await db.get(RemoteStorage, storage_id)
        backend = storage_manager.add_storage(_db_to_config(storage))
    calls = []

    async def refuse(path):
        calls.append(path)
        return False

    monkeypatch.setattr(backend, "delete", refuse)
    endpoint = "/api/v1/backups" if bulk else f"/api/v1/backups/{backup_id}"
    response = await async_client.delete(endpoint)
    assert response.status_code == 200
    assert calls == [archive.name], "Failed sync can still own uploaded remote bytes"
    assert key.exists() and (remote / archive.name).exists()
    async with test_db() as db:
        assert await db.get(Backup, backup_id) is not None
        assert (
            await db.execute(select(BackupStorageSync))
        ).scalar_one().remote_path == archive.name
    assert "warning" in response.text.lower() or response.json().get("errors")


def test_actual_authenticated_remote_only_restore_process(tmp_path):
    import os
    import subprocess
    import sys

    backend = Path(__file__).resolve().parents[1]
    result = subprocess.run(
        [
            sys.executable,
            str(backend / "tests/runtime_catalog_recovery_probe.py"),
            str(tmp_path),
        ],
        env=os.environ
        | {
            "DATABASE_URL": f"sqlite+aiosqlite:///{tmp_path}/catalog.db",
            "BACKUP_BASE_PATH": str(tmp_path),
            "PYTHONPATH": str(backend),
        },
        cwd=tmp_path,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert "remote TAR restore passed" in result.stdout
