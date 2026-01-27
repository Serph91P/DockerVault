"""
Backups API endpoints.
"""

import asyncio
import os
from typing import List, Optional

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel
from sqlalchemy import select

from app.backup_engine import backup_engine
from app.config import settings
from app.database import Backup, BackupStatus, BackupTarget, BackupType, async_session

router = APIRouter()


class BackupResponse(BaseModel):
    """Backup response model."""

    id: int
    target_id: int
    target_name: Optional[str] = None
    backup_type: str
    status: str
    file_path: Optional[str] = None
    file_size: Optional[int] = None
    file_size_human: Optional[str] = None
    checksum: Optional[str] = None
    started_at: Optional[str] = None
    completed_at: Optional[str] = None
    duration_seconds: Optional[int] = None
    error_message: Optional[str] = None
    encrypted: bool = False
    created_at: str

    class Config:
        from_attributes = True


class CreateBackupRequest(BaseModel):
    """Create backup request."""

    target_id: int
    backup_type: str = "full"


class RestoreBackupRequest(BaseModel):
    """Restore backup request."""

    target_path: Optional[str] = None
    private_key: Optional[str] = None  # Required for encrypted backups


def format_size(size_bytes: Optional[int]) -> Optional[str]:
    """Format size in human readable format."""
    if size_bytes is None:
        return None
    for unit in ["B", "KB", "MB", "GB", "TB"]:
        if size_bytes < 1024:
            return f"{size_bytes:.2f} {unit}"
        size_bytes /= 1024
    return f"{size_bytes:.2f} PB"


@router.get("", response_model=List[BackupResponse])
async def list_backups(
    target_id: Optional[int] = None,
    status: Optional[str] = None,
    limit: int = 50,
    offset: int = 0,
):
    """List backups with optional filters and pagination.

    Args:
        target_id: Filter by target ID
        status: Filter by backup status
        limit: Maximum number of results (default 50)
        offset: Number of results to skip for pagination (default 0)
    """
    async with async_session() as session:
        query = (
            select(Backup)
            .order_by(Backup.created_at.desc())
            .limit(limit)
            .offset(offset)
        )

        if target_id:
            query = query.where(Backup.target_id == target_id)
        if status:
            query = query.where(Backup.status == BackupStatus(status))

        result = await session.execute(query)
        backups = result.scalars().all()

        # Get target names
        target_ids = set(b.target_id for b in backups)
        targets_result = await session.execute(
            select(BackupTarget).where(BackupTarget.id.in_(target_ids))
        )
        targets = {t.id: t.name for t in targets_result.scalars().all()}

        return [
            BackupResponse(
                id=b.id,
                target_id=b.target_id,
                target_name=targets.get(b.target_id),
                backup_type=b.backup_type.value,
                status=b.status.value,
                file_path=b.file_path,
                file_size=b.file_size,
                file_size_human=format_size(b.file_size),
                checksum=b.checksum,
                started_at=b.started_at.isoformat() if b.started_at else None,
                completed_at=b.completed_at.isoformat() if b.completed_at else None,
                duration_seconds=b.duration_seconds,
                error_message=b.error_message,
                encrypted=b.encrypted or False,
                created_at=b.created_at.isoformat(),
            )
            for b in backups
        ]


@router.get("/{backup_id}", response_model=BackupResponse)
async def get_backup(backup_id: int):
    """Get a specific backup."""
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        # Get target name
        target_result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == backup.target_id)
        )
        target = target_result.scalar_one_or_none()

        return BackupResponse(
            id=backup.id,
            target_id=backup.target_id,
            target_name=target.name if target else None,
            backup_type=backup.backup_type.value,
            status=backup.status.value,
            file_path=backup.file_path,
            file_size=backup.file_size,
            file_size_human=format_size(backup.file_size),
            checksum=backup.checksum,
            started_at=backup.started_at.isoformat() if backup.started_at else None,
            completed_at=(
                backup.completed_at.isoformat() if backup.completed_at else None
            ),
            duration_seconds=backup.duration_seconds,
            error_message=backup.error_message,
            encrypted=backup.encrypted or False,
            created_at=backup.created_at.isoformat(),
        )


@router.post("", response_model=BackupResponse)
async def create_backup(request: CreateBackupRequest):
    """Create and run a new backup."""
    async with async_session() as session:
        # Get target
        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == request.target_id)
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

    # Create backup
    backup_type = (
        BackupType.FULL if request.backup_type == "full" else BackupType.INCREMENTAL
    )
    backup = await backup_engine.create_backup(target, backup_type)

    # Run backup in background
    asyncio.create_task(backup_engine.run_backup(backup.id))

    return BackupResponse(
        id=backup.id,
        target_id=backup.target_id,
        target_name=target.name,
        backup_type=backup.backup_type.value,
        status=backup.status.value,
        file_path=backup.file_path,
        file_size=backup.file_size,
        file_size_human=format_size(backup.file_size),
        checksum=backup.checksum,
        started_at=backup.started_at.isoformat() if backup.started_at else None,
        completed_at=backup.completed_at.isoformat() if backup.completed_at else None,
        duration_seconds=backup.duration_seconds,
        error_message=backup.error_message,
        encrypted=backup.encrypted or False,
        created_at=backup.created_at.isoformat(),
    )


@router.post("/{backup_id}/restore")
async def restore_backup(backup_id: int, request: RestoreBackupRequest):
    """Restore a backup.

    If target_path is provided, it must be within allowed restore directories
    to prevent path traversal attacks.
    """
    if request.target_path:
        # Validate the target path to prevent path traversal
        abs_path = os.path.abspath(request.target_path)

        # Check for path traversal attempts
        if ".." in request.target_path:
            raise HTTPException(
                status_code=400,
                detail="Invalid restore path: path traversal not allowed",
            )

        # Ensure path is within allowed restore directories
        # Allow restoring to backup base path or Docker volume paths
        allowed_prefixes = [
            os.path.abspath(settings.BACKUP_BASE_PATH),
            "/var/lib/docker/volumes",
        ]

        is_allowed = any(abs_path.startswith(prefix) for prefix in allowed_prefixes)

        if not is_allowed:
            raise HTTPException(
                status_code=400,
                detail="Invalid restore path: must be within allowed directories",
            )

    success = await backup_engine.restore_backup(
        backup_id, 
        request.target_path,
        request.private_key
    )

    if not success:
        raise HTTPException(status_code=500, detail="Restore failed")

    return {"status": "restored"}


@router.delete("/{backup_id}")
async def delete_backup(backup_id: int):
    """Delete a backup."""
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        # Delete file if exists
        if backup.file_path:
            import os

            if os.path.exists(backup.file_path):
                os.remove(backup.file_path)

        await session.delete(backup)
        await session.commit()

        return {"status": "deleted"}


@router.get("/{backup_id}/stats")
async def get_backup_stats(backup_id: int):
    """Get statistics for a backup."""
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        return {
            "backup_id": backup.id,
            "file_size": backup.file_size,
            "file_size_human": format_size(backup.file_size),
            "duration_seconds": backup.duration_seconds,
            "status": backup.status.value,
        }


@router.get("/metrics/summary")
async def get_backup_metrics():
    """Get overall backup metrics and statistics.

    Returns aggregate statistics about backup operations including:
    - Total number of backups
    - Success/failure counts and rate
    - Total data backed up
    - Last backup timestamp
    """
    return backup_engine.metrics.to_dict()


@router.get("/metrics/target/{target_id}")
async def get_target_metrics(target_id: int):
    """Get metrics for a specific backup target.

    Returns:
    - Average backup duration
    - Average backup size
    - Recent backup history
    """
    avg_duration = backup_engine.metrics.get_average_duration(target_id)
    avg_size = backup_engine.metrics.get_average_size(target_id)

    return {
        "target_id": target_id,
        "average_duration_seconds": avg_duration,
        "average_size_bytes": avg_size,
        "average_size_human": format_size(avg_size) if avg_size else None,
        "backup_count": len(backup_engine.metrics.target_durations.get(target_id, [])),
    }


@router.post("/{backup_id}/validate")
async def validate_backup_target(backup_id: int):
    """Validate backup prerequisites before running.

    Checks:
    - Target container/volume/path exists
    - Sufficient disk space
    - Dependencies are available

    Returns list of validation issues, or empty list if valid.
    """
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == backup.target_id)
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

    issues = await backup_engine.validate_backup_prerequisites(target)

    return {
        "valid": len(issues) == 0,
        "issues": issues,
    }
