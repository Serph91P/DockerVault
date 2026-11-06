"""#185 fixture-only seed route; native app auth/SQLite/TAR/age/local-provider I/O.

507 catalog seeds are NOT 507 engine runs. No production socket/mount/provider.
"""

import hashlib
import io
from pathlib import Path
import subprocess
import tarfile
from datetime import datetime

import uvicorn
from app.main import app
from app.database import (
    async_session,
    BackupTarget,
    Backup,
    BackupStatus,
    BackupLog,
    LogLevel,
    RemoteStorage,
    BackupStorageSync,
)
from app.encryption import generate_key_pair, encrypt_backup

root = Path("/app/data/history185")
root.mkdir(exist_ok=True)
private_key = ""


@app.post("/api/v1/test-history185/seed")
async def seed():
    global private_key
    plain = root / "plain.tar"
    with tarfile.open(plain, "w") as archive:
        directory = tarfile.TarInfo("root")
        directory.type = tarfile.DIRTYPE
        archive.addfile(directory)
        for index in range(2005):
            data = f"owned history payload {index}".encode()
            member = tarfile.TarInfo(f"root/file-{index:04}.txt")
            member.size = len(data)
            archive.addfile(member, io.BytesIO(data))
    keys = await generate_key_pair()
    private_key = keys.private_key
    encrypted_input = root / "encrypted-input.tar"
    encrypted_input.write_bytes(plain.read_bytes())
    encrypted = await encrypt_backup(encrypted_input, keys.public_key)
    remote_root = root / "remote"
    (remote_root / "retained").mkdir(parents=True, exist_ok=True)
    (remote_root / "retained/remote.tar").write_bytes(plain.read_bytes())
    async with async_session() as session:
        remote = RemoteStorage(
            name="History offsite",
            storage_type="local",
            base_path=str(remote_root),
            enabled=True,
        )
        new_remote = RemoteStorage(
            name="New destination",
            storage_type="local",
            base_path=str(root / "new-remote"),
            enabled=True,
        )
        (root / "new-remote").mkdir(exist_ok=True)
        session.add_all([remote, new_remote])
        await session.flush()
        target = BackupTarget(
            name="Retained history source",
            target_type="path",
            host_path=str(root),
            remote_storage_ids=[remote.id, new_remote.id],
        )
        session.add(target)
        await session.flush()
        for index in range(507):
            session.add(
                Backup(
                    target_id=target.id,
                    status=BackupStatus.COMPLETED,
                    file_path=f"/backups/seed-{index:04}.tar",
                    file_size=index,
                    created_at=datetime(2025, 1, 1),
                )
            )
        rows = {}
        for name in ("local", "remote", "encrypted", "missing", "mixed", "failed"):
            if name in ("local", "mixed"):
                (root / f"{name}.tar").write_bytes(plain.read_bytes())
            row = Backup(
                target_id=target.id,
                status=BackupStatus.FAILED
                if name == "failed"
                else BackupStatus.COMPLETED,
                file_path=str(encrypted.encrypted_path)
                if name == "encrypted"
                else None
                if name in ("remote", "failed")
                else str(root / "absent.tar")
                if name == "missing"
                else str(root / f"{name}.tar"),
                file_size=None if name == "failed" else plain.stat().st_size,
                checksum=None
                if name == "failed"
                else hashlib.sha256(plain.read_bytes()).hexdigest(),
                encrypted=name == "encrypted",
                encryption_key_path=str(encrypted.key_path)
                if name == "encrypted"
                else None,
                started_at=datetime(2026, 1, 1),
                completed_at=datetime(2026, 1, 1, 0, 0, 2),
                duration_seconds=2,
                error_message="Explicit failed fixture, no archive"
                if name == "failed"
                else None,
            )
            session.add(row)
            await session.flush()
            rows[name] = row.id
            session.add(
                BackupLog(
                    backup_id=row.id,
                    level=LogLevel.INFO,
                    step="complete" if name != "failed" else "error",
                    message=f"Native retained {name} log",
                )
            )
            if name in ("remote", "mixed"):
                session.add(
                    BackupStorageSync(
                        backup_id=row.id,
                        storage_id=remote.id,
                        sync_status="completed",
                        remote_path="retained/remote.tar",
                    )
                )
            if name == "mixed":
                session.add(
                    BackupStorageSync(
                        backup_id=row.id,
                        storage_id=new_remote.id,
                        sync_status="failed",
                        error_message="Fixture prior sync failure",
                    )
                )
        await session.commit()
        return {
            "ids": rows,
            "target": target.id,
            "remote": remote.id,
            "new_remote": new_remote.id,
            "total": 513,
        }


@app.post("/api/v1/test-history185/key")
async def fixture_key():
    return {"private_key": private_key}


subprocess.Popen(["nginx", "-g", "daemon off;"])
uvicorn.run(app, host="127.0.0.1", port=8000, access_log=False, log_level="warning")
