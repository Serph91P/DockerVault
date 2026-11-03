"""Opaque synthetic-byte auth/provider/catalog probe, NOT app backup or restore."""

import asyncio
import hashlib
import os
from pathlib import Path
import shutil
import sys

from httpx import ASGITransport, AsyncClient
from sqlalchemy import select

from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
    async_session,
    engine,
    init_db,
)
from app.main import app
from app.api.backups import _ensure_backup_locally, _download_key_from_remote


async def run(mode, root):
    await init_db()
    special = os.environ.get("SYNC_SPECIAL_NAMES") == "1"
    stem = "archive.enc" if special else "archive"
    archive = root / (stem + ".tar.enc")
    key = root / ("different-local.key" if special else stem + ".tar.key")
    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="https://owned-test"
    ) as client:
        if mode in ("sync", "retry"):
            archive.write_bytes(b"owned persisted archive")
            key.write_bytes(b"owned persisted key")
            async with async_session() as db:
                target = BackupTarget(
                    name="owned.enc" if special else "owned",
                    target_type="path",
                    host_path=str(root),
                )
                storage = RemoteStorage(
                    name="owned", storage_type="local", base_path=str(root / "remote")
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
                if mode == "retry":
                    await db.flush()
                    target.remote_storage_ids = [storage.id]
                    backup.status = BackupStatus.FAILED
                    backup.file_size = archive.stat().st_size
                    backup.checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
                    backup.error_message = "Remote sync failed: owned initial denial"
                    db.add(
                        BackupStorageSync(
                            backup_id=backup.id,
                            storage_id=storage.id,
                            remote_path="owned/archive.tar.enc",
                            sync_status="failed",
                            error_message="owned initial denial",
                        )
                    )
                await db.commit()
                backup_id, storage_id = backup.id, storage.id
            setup = await client.post(
                "/api/v1/auth/setup",
                json={
                    "username": "owned",
                    "password": "Synthetic-password-161",
                    "confirm_password": "Synthetic-password-161",
                },
            )
            assert setup.status_code == 200, setup.text
            if mode == "retry":
                response = await client.post(f"/api/v1/backups/{backup_id}/retry-sync")
                assert (
                    response.status_code == 200 and response.json()["succeeded"] == 1
                ), response.text
            else:
                for _ in range(2):
                    response = await client.post(
                        f"/api/v1/storage/{storage_id}/sync/{backup_id}"
                    )
                    assert response.status_code == 200, response.text
            archive.unlink()
            key.unlink()
        else:
            login = await client.post(
                "/api/v1/auth/login",
                json={"username": "owned", "password": "Synthetic-password-161"},
            )
            assert login.status_code == 200
            async with async_session() as db:
                backup = (await db.execute(select(Backup))).scalar_one()
                row = (await db.execute(select(BackupStorageSync))).scalar_one()
                assert row.sync_status == "completed"
            detail = await client.get(f"/api/v1/backups/{backup.id}")
            assert detail.status_code == 200 and detail.json()["remote_synced"]
            restored, temporary = await _ensure_backup_locally(backup)
            try:
                assert Path(restored).read_bytes() == b"owned persisted archive"
                restored_key = await _download_key_from_remote(backup, restored)
                assert (
                    restored_key
                    and Path(restored_key).read_bytes() == b"owned persisted key"
                )
            finally:
                if temporary:
                    shutil.rmtree(temporary)
    await engine.dispose()
    print(f"{mode}: real auth/API + persistent catalog + verified archive/key passed")


if __name__ == "__main__":
    asyncio.run(run(sys.argv[1], Path(sys.argv[2])))
