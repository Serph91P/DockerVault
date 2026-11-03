"""Owned synthetic fixture for rendered #180 acceptance, never a production entrypoint."""

import asyncio
import io
import json
import os
from pathlib import Path
import shutil
import tarfile

from fastapi import HTTPException
from sqlalchemy import select
from starlette.staticfiles import StaticFiles
from starlette.exceptions import HTTPException as StarletteHTTPException
import uvicorn

from app.database import (
    Backup,
    BackupLog,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
    async_session,
    engine,
    init_db,
)
from app.encryption import encrypt_backup, generate_key_pair
from app.main import app

root = Path(os.environ["DV_REMOVAL_ROOT"])


async def seed():
    root.mkdir(parents=True, exist_ok=True)
    await init_db()
    if (root / "manifest.json").exists():
        return
    pair = await generate_key_pair()
    (root / "private-key").write_text(pair.private_key)
    (root / "private-key").chmod(0o600)
    manifest = []
    async with async_session() as db:
        for entry in ("Backups", "Targets", "Backups copies", "Targets copies"):
            for scope in ("local", "mixed", "remote"):
                stored_scope = "mixed" if "copies" in entry else scope
                folder = root / f"{entry}-{scope}"
                folder.mkdir()
                destination, remote = folder / "destination", folder / "remote"
                destination.mkdir()
                remote.mkdir()
                payload = f"Synthetic retained {entry} {scope}".encode()
                archive = folder / "archive.tar.gz"
                with tarfile.open(archive, "w:gz") as tar:
                    member = tarfile.TarInfo("payload.txt")
                    member.size = len(payload)
                    tar.addfile(member, io.BytesIO(payload))
                encrypted = await encrypt_backup(archive, pair.public_key)
                if stored_scope != "local":
                    shutil.copyfile(
                        encrypted.encrypted_path, remote / encrypted.encrypted_path.name
                    )
                    shutil.copyfile(
                        encrypted.key_path, remote / encrypted.key_path.name
                    )
                if stored_scope == "remote":
                    encrypted.encrypted_path.unlink()
                    encrypted.key_path.unlink()
                target = BackupTarget(
                    name=f"{entry} {scope}",
                    target_type="path",
                    host_path=str(destination),
                    stop_container=False,
                )
                storage = RemoteStorage(
                    name=f"{entry} {scope} remote",
                    storage_type="local",
                    base_path=str(remote),
                )
                db.add_all([target, storage])
                await db.flush()
                backup = Backup(
                    target_id=target.id,
                    status=BackupStatus.COMPLETED,
                    encrypted=True,
                    file_path=str(encrypted.encrypted_path)
                    if stored_scope != "remote"
                    else None,
                    encryption_key_path=str(encrypted.key_path)
                    if stored_scope != "remote"
                    else None,
                )
                db.add(backup)
                await db.flush()
                db.add(
                    BackupLog(
                        backup_id=backup.id,
                        step="complete",
                        message="Retained synthetic log",
                    )
                )
                if stored_scope != "local":
                    db.add(
                        BackupStorageSync(
                            backup_id=backup.id,
                            storage_id=storage.id,
                            sync_status="completed",
                            remote_path=encrypted.encrypted_path.name,
                        )
                    )
                manifest.append(
                    {
                        "entry": entry,
                        "scope": scope,
                        "name": target.name,
                        "target_id": target.id,
                        "backup_id": backup.id,
                        "storage_id": storage.id,
                        "folder": str(folder),
                        "payload": payload.decode(),
                    }
                )
        await db.commit()
    (root / "manifest.json").write_text(json.dumps(manifest))


@app.get("/api/v1/removal-fixture/manifest")
async def manifest():
    return json.loads((root / "manifest.json").read_text())


@app.get("/api/v1/removal-fixture/key")
async def synthetic_key():
    return {"private_key": (root / "private-key").read_text()}


@app.get("/api/v1/removal-fixture/state/{backup_id}")
async def state(backup_id: int):
    item = next(i for i in await manifest() if i["backup_id"] == backup_id)
    folder = Path(item["folder"])
    async with async_session() as db:
        backup = await db.get(Backup, backup_id)
        target = await db.get(BackupTarget, item["target_id"])
        syncs = (
            await db.scalars(
                select(BackupStorageSync).where(
                    BackupStorageSync.backup_id == backup_id
                )
            )
        ).all()
        return {
            "archive": (folder / "archive.tar.gz.enc").is_file(),
            "key": (folder / "archive.tar.gz.key").is_file(),
            "remote": (folder / "remote/archive.tar.gz.enc").is_file(),
            "remote_key": (folder / "remote/archive.tar.gz.key").is_file(),
            "catalog": backup is not None,
            "syncs": len(syncs),
            "removed": bool(target.removed_at),
            "restored_bytes_match": (folder / "destination/payload.txt").is_file()
            and (folder / "destination/payload.txt").read_bytes()
            == item["payload"].encode(),
        }


@app.post("/api/v1/removal-fixture/block/{backup_id}")
async def block_remote(backup_id: int, blocked: bool = True):
    item = next(i for i in await manifest() if i["backup_id"] == backup_id)
    if item["scope"] == "local":
        raise HTTPException(400, "No remote fixture")
    path = Path(item["folder"]) / "remote/archive.tar.gz.enc"
    saved = path.with_suffix(".held")
    if blocked:
        path.rename(saved)
        path.mkdir()
    else:
        path.rmdir()
        saved.rename(path)
    return {"blocked": blocked}


class SPAFiles(StaticFiles):
    async def get_response(self, path, scope):
        try:
            return await super().get_response(path, scope)
        except StarletteHTTPException as exc:
            if exc.status_code != 404:
                raise
            return await super().get_response("index.html", scope)


if __name__ == "__main__":
    asyncio.run(seed())
    asyncio.run(engine.dispose())
    app.mount("/", SPAFiles(directory="/usr/share/nginx/html", html=True))
    uvicorn.run(app, host="127.0.0.1", port=8443, access_log=False, log_level="warning")
