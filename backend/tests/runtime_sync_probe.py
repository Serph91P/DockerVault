"""Real auth/API/catalog/provider roundtrip across separate process lifetimes."""

import asyncio
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
    archive, key = root / "archive.tar.enc", root / "archive.tar.key"
    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="https://owned-test"
    ) as client:
        if mode == "sync":
            archive.write_bytes(b"owned persisted archive")
            key.write_bytes(b"owned persisted key")
            async with async_session() as db:
                target = BackupTarget(
                    name="owned", target_type="path", host_path=str(root)
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
