"""Isolated #187 fixture controls, never part of the production app/image.

Reuses the #184 synthetic discovery/lifecycle boundary. Cookie auth, policy APIs,
scheduler, pruning, SQLite, archive engine and successful file effects are native.
Only unlink failures and one scheduler-reconciliation fault are injected explicitly.
"""

from datetime import datetime, timedelta, timezone
from pathlib import Path
import runpy

from fastapi import HTTPException
from sqlalchemy import select, text
from app.config import settings
from app.database import async_session, backup_db_session, Backup, BackupStatus
from app.main import app
from app.scheduler import BackupScheduler

ROOT = Path("/app/data/policy187-control")
ROOT.mkdir(parents=True, exist_ok=True)
CLEAN = Path(settings.BACKUP_BASE_PATH) / "policy187-orphans"
CLEAN.mkdir(parents=True, exist_ok=True)
original_unlink = Path.unlink
original_reconcile = BackupScheduler.reconcile


def injected_unlink(path, *args, **kwargs):
    if path.parent == CLEAN and (ROOT / ("fail-" + path.name)).exists():
        raise PermissionError("explicit synthetic policy187 unlink failure")
    return original_unlink(path, *args, **kwargs)


async def injected_reconcile(self):
    marker = ROOT / "reconcile-once"
    if marker.exists():
        original_unlink(marker)
        raise RuntimeError("explicit synthetic postcommit reconciliation fault")
    return await original_reconcile(self)


Path.unlink = injected_unlink
BackupScheduler.reconcile = injected_reconcile


@app.post("/api/v1/test-policy187/{action}")
async def control(action: str, data: dict):
    # The real app's cookie middleware guards these fixture-only controls too.
    if action == "reconcile-once":
        (ROOT / "reconcile-once").touch()
        return {"armed": True}
    if action == "cleanup":
        mode = data["mode"]
        assert mode in ("empty", "success", "partial", "failure")
        for path in ROOT.glob("fail-*"):
            original_unlink(path)
        for name in ("orphan0.tar.gz", "orphan1.tar.gz"):
            path = CLEAN / name
            if path.exists():
                original_unlink(path)
        async with backup_db_session(async_session) as session:
            if not (
                await session.execute(
                    select(Backup).where(
                        Backup.file_path == str(CLEAN / "catalog.tar.gz")
                    )
                )
            ).scalar_one_or_none():
                (CLEAN / "catalog.tar.gz").write_bytes(b"cataloged sentinel")
                (CLEAN / "active.tar.gz.tmp").write_bytes(b"active sentinel")
                session.add(
                    Backup(
                        target_id=data["target_id"],
                        status=BackupStatus.COMPLETED,
                        file_path=str(CLEAN / "catalog.tar.gz"),
                    )
                )
                session.add(
                    Backup(
                        target_id=data["target_id"],
                        status=BackupStatus.RUNNING,
                        backup_metadata={
                            "active_artifacts": [str(CLEAN / "active.tar.gz.tmp")]
                        },
                    )
                )
                await session.commit()
        if mode != "empty":
            for index in range(2):
                name = f"orphan{index}.tar.gz"
                (CLEAN / name).write_bytes(b"orphan")
                if mode == "failure" or (mode == "partial" and index == 0):
                    (ROOT / ("fail-" + name)).touch()
        return {"mode": mode}
    if action == "seed-pruning":
        folder = Path(settings.BACKUP_BASE_PATH) / (
            "policy187-age-" + str(data["target_id"])
        )
        folder.mkdir(exist_ok=True)
        now = datetime.now(timezone.utc)
        ages = [1, 25, 49] if data["kind"] == "gfs" else [48, 72, 96]
        async with backup_db_session(async_session) as session:
            ids = []
            for index, hours in enumerate(ages):
                path = folder / f"backup{index}.tar.gz"
                path.write_bytes(b"pruning fixture bytes")
                backup = Backup(
                    target_id=data["target_id"],
                    status=BackupStatus.COMPLETED,
                    file_path=str(path),
                    created_at=now - timedelta(hours=hours),
                    completed_at=now - timedelta(hours=hours),
                    file_size=path.stat().st_size,
                )
                session.add(backup)
                await session.flush()
                ids.append(backup.id)
            await session.commit()
        return {"ids": ids}
    if action == "snapshot":
        async with backup_db_session(async_session) as session:
            rows = (await session.execute(select(Backup))).scalars().all()
            violations = (await session.execute(text("PRAGMA foreign_key_check"))).all()
            return {
                "files": {
                    p.name: p.exists()
                    for p in [
                        CLEAN / name
                        for name in (
                            "catalog.tar.gz",
                            "active.tar.gz.tmp",
                            "orphan0.tar.gz",
                            "orphan1.tar.gz",
                        )
                    ]
                },
                "backups": [
                    {
                        "id": row.id,
                        "target_id": row.target_id,
                        "status": row.status.value,
                        "exists": bool(row.file_path and Path(row.file_path).exists()),
                    }
                    for row in rows
                ],
                "foreign_key_violations": len(violations),
            }
    raise HTTPException(404)


runpy.run_path("/app/data/runtime_protection_app.py", run_name="__main__")
