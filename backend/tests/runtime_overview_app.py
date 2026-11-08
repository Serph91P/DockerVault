"""Owned #188 synthetic runtime: real auth/SQLite/path-engine/archive/disk APIs.

Docker discovery/ping are named doubles, never a production socket claim. Seeded
failed/sync rows and low/unavailable capacity are explicit fault fixtures. No
external provider, host bind or real credentials are used.
"""

import asyncio
from datetime import datetime, timezone
from pathlib import Path
import shutil
import subprocess

import uvicorn
from app.main import app
from app import database
from app.backup_engine import backup_engine
from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
)
from app.docker_client import docker_client
from sqlalchemy import select

root = Path("/app/data/overview188")
source = root / "source"
(source / "nested").mkdir(parents=True, exist_ok=True)
(source / "nested/payload.txt").write_bytes(b"owned overview nested bytes\x00\xff")
remote = root / "remote"
remote.mkdir(exist_ok=True)
mode = "normal"
real_usage = shutil.disk_usage


async def ping():
    return True


async def inventory(*args, **kwargs):
    return []


docker_client.ping = ping
docker_client.list_containers = inventory
docker_client.list_volumes = inventory
docker_client.get_stacks = inventory


def disk_usage(path):
    if mode == "low":
        return shutil._ntuple_diskusage(4 * 1024**3, 4 * 1024**3 - 1024, 1024)
    if mode == "unavailable":
        raise OSError("Explicit synthetic disk measurement fault")
    return real_usage(path)


shutil.disk_usage = disk_usage
release = asyncio.Event()
real_sync = backup_engine._sync_to_remote


async def held_sync(*args, **kwargs):
    if mode == "active":
        await release.wait()
    return await real_sync(*args, **kwargs)


backup_engine._sync_to_remote = held_sync


@app.post("/api/v1/test-overview188/mode/{value}")
async def fixture(value: str):
    global mode
    mode = value
    if value != "active":
        release.set()
    else:
        release.clear()
    async with database.async_session() as db:
        if value == "never-run":
            plan = BackupTarget(
                name="No retained run fixture",
                target_type="path",
                host_path=str(source),
            )
            db.add(plan)
            await db.commit()
            return {"plan": plan.id}
        if value == "failed":
            plan = (
                (await db.execute(select(BackupTarget).order_by(BackupTarget.id)))
                .scalars()
                .first()
            )
            row = Backup(
                target_id=plan.id,
                status=BackupStatus.FAILED,
                created_at=datetime.now(timezone.utc),
                error_message="Explicit failed-run fixture; archive never created",
            )
            db.add(row)
            await db.commit()
            return {"run": row.id}
        if value == "partial":
            location = RemoteStorage(
                name="Disabled copy-fault location",
                storage_type="local",
                base_path=str(remote),
                enabled=False,
            )
            db.add(location)
            await db.flush()
            row = (
                (
                    await db.execute(
                        select(Backup)
                        .where(Backup.status == BackupStatus.COMPLETED)
                        .order_by(Backup.id)
                    )
                )
                .scalars()
                .first()
            )
            db.add(
                BackupStorageSync(
                    backup_id=row.id,
                    storage_id=location.id,
                    sync_status="failed",
                    error_message="Explicit historical sync fault fixture",
                )
            )
            await db.commit()
            return {"run": row.id, "storage": location.id}
        if value == "active":
            location = RemoteStorage(
                name="Held local copy",
                storage_type="local",
                base_path=str(remote),
                enabled=True,
            )
            db.add(location)
            await db.flush()
            plan = BackupTarget(
                name="Active upload fixture",
                target_type="path",
                host_path=str(source),
                remote_storage_ids=[location.id],
            )
            db.add(plan)
            await db.commit()
            backup = await backup_engine.create_backup(plan.id)
            # Native queue path; the controlled delay is only at the actual upload boundary.
            await backup_engine.enqueue(backup.id)
            return {"run": backup.id, "plan": plan.id}
    return {"mode": mode}


@app.get("/api/v1/test-overview188/effects")
async def effects():
    import tarfile
    from sqlalchemy import select

    async with database.async_session() as db:
        rows = (
            (
                await db.execute(
                    select(Backup).where(Backup.status == BackupStatus.COMPLETED)
                )
            )
            .scalars()
            .all()
        )
        checked = []
        for row in rows:
            if row.file_path and Path(row.file_path).is_file():
                with tarfile.open(row.file_path) as archive:
                    payloads = [
                        archive.extractfile(item).read()
                        for item in archive.getmembers()
                        if item.isfile() and item.name.endswith("nested/payload.txt")
                    ]
                assert payloads == [(source / "nested/payload.txt").read_bytes()]
                checked.append(row.id)
        return {"verified_archives": checked, "nested_bytes_equal": bool(checked)}


subprocess.Popen(["nginx", "-g", "daemon off;"])
uvicorn.run(app, host="127.0.0.1", port=8000, access_log=False, log_level="warning")
