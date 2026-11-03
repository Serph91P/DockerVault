"""Actual auth/API/SQLite/TAR remote-only restoration, without mocks."""

import asyncio
import io
from pathlib import Path
import sys
import tarfile

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


async def run(root):
    await init_db()
    target_path, remote = root / "destination", root / "remote"
    target_path.mkdir()
    remote.mkdir()
    (target_path / "payload").write_bytes(b"damaged")
    archive = root / "archive.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        member = tarfile.TarInfo("payload")
        member.size = len(b"owned original")
        tar.addfile(member, io.BytesIO(b"owned original"))
    (remote / archive.name).write_bytes(archive.read_bytes())
    async with async_session() as db:
        target = BackupTarget(
            name="owned",
            target_type="path",
            host_path=str(target_path),
            stop_container=False,
        )
        storage = RemoteStorage(
            name="owned", storage_type="local", base_path=str(remote)
        )
        db.add_all([target, storage])
        await db.flush()
        backup = Backup(
            target_id=target.id, status=BackupStatus.COMPLETED, file_path=str(archive)
        )
        db.add(backup)
        await db.flush()
        backup_id = backup.id
        db.add(
            BackupStorageSync(
                backup_id=backup.id,
                storage_id=storage.id,
                remote_path=archive.name,
                sync_status="completed",
            )
        )
        await db.commit()
    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="https://owned"
    ) as client:
        response = await client.post(
            "/api/v1/auth/setup",
            json={
                "username": "owned",
                "password": "Synthetic-password-R3",
                "confirm_password": "Synthetic-password-R3",
            },
        )
        assert response.status_code == 200
        response = await client.delete(
            f"/api/v1/backups/{backup_id}?delete_remote=false"
        )
        assert response.status_code == 200 and not archive.exists()
        response = await client.post(f"/api/v1/backups/{backup_id}/restore", json={})
        assert response.status_code == 200, response.text
        assert (target_path / "payload").read_bytes() == b"owned original"
    async with async_session() as db:
        backup = await db.get(Backup, backup_id)
        assert backup.file_path is None
        assert (
            await db.execute(select(BackupStorageSync))
        ).scalar_one().sync_status == "completed"
    await engine.dispose()
    print(
        "actual auth + local-only deletion + remote TAR restore passed; permanent remote-only catalog preserved"
    )


if __name__ == "__main__":
    asyncio.run(run(Path(sys.argv[1])))
