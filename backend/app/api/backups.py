"""
Backups API endpoints.
"""

import asyncio
import logging
import os
import re
import tarfile
import tempfile
from datetime import datetime
from pathlib import Path
from typing import List, Optional

from fastapi import APIRouter, HTTPException, Query
from fastapi.responses import StreamingResponse
from pydantic import BaseModel
from sqlalchemy import select

from app.backup_engine import backup_engine
from app.config import settings
from app.database import Backup, BackupStatus, BackupTarget, BackupType, async_session
from app.encryption import DecryptionError, decrypt_backup

logger = logging.getLogger(__name__)

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
    limit: int = Query(default=50, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
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
    logger.info(
        f"Creating backup for target {request.target_id}, type: {request.backup_type}"
    )

    async with async_session() as session:
        # Get target
        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == request.target_id)
        )
        target = result.scalar_one_or_none()

        if not target:
            logger.error("Target %s not found", request.target_id)
            raise HTTPException(status_code=404, detail="Target not found")

        logger.info(f"Found target: {target.name} (type: {target.target_type})")

    # Create backup
    backup_type = (
        BackupType.FULL if request.backup_type == "full" else BackupType.INCREMENTAL
    )
    backup = await backup_engine.create_backup(target, backup_type)

    logger.info(f"Created backup {backup.id}, starting backup task...")

    # Run backup in background with error logging
    async def _run_backup_logged(bid: int):
        try:
            await backup_engine.run_backup(bid)
        except Exception as exc:
            logger.error("Background backup task %d failed: %s", bid, exc)

    asyncio.create_task(_run_backup_logged(backup.id))

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
        # Resolve symlinks and normalize to prevent TOCTOU attacks
        abs_path = os.path.realpath(request.target_path)

        # Ensure path is within allowed restore directories
        allowed_prefixes = [
            os.path.realpath(settings.BACKUP_BASE_PATH),
            "/var/lib/docker/volumes",
        ]

        is_allowed = any(
            abs_path == prefix or abs_path.startswith(prefix + os.sep)
            for prefix in allowed_prefixes
        )

        if not is_allowed:
            raise HTTPException(
                status_code=400,
                detail="Invalid restore path: must be within allowed directories",
            )

    success = await backup_engine.restore_backup(
        backup_id, request.target_path, request.private_key
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


class BackupFileInfo(BaseModel):
    """Information about a file in a backup archive."""

    name: str
    path: str
    size: int
    size_human: str
    is_dir: bool
    mode: str
    mtime: str


def _format_file_size(size_bytes: int) -> str:
    """Format size in human readable format."""
    for unit in ["B", "KB", "MB", "GB", "TB"]:
        if size_bytes < 1024:
            return f"{size_bytes:.1f} {unit}"
        size_bytes /= 1024
    return f"{size_bytes:.1f} PB"


def _sanitize_filename(name: str) -> str:
    """Sanitize a filename for use in Content-Disposition headers.

    Prevents header injection by stripping unsafe characters.
    """
    basename = os.path.basename(name)
    sanitized = re.sub(r"[^a-zA-Z0-9._-]", "_", basename)
    sanitized = sanitized[:255]
    return sanitized or "download"


def _validate_archive_path(file_path: str) -> str:
    """Normalize and validate a path used for tar member lookup.

    Prevents path traversal by rejecting '..' components and leading slashes.
    Returns the normalized path for use with tar.getmember().
    """
    normalized = os.path.normpath(file_path).lstrip("/\\")
    parts = normalized.split("/")
    if ".." in parts or normalized.startswith("/"):
        raise HTTPException(status_code=400, detail="Invalid file path")
    return normalized


async def _get_decrypted_archive_path(
    backup: Backup, private_key: str
) -> tuple[str, str, Path | None]:
    """Decrypt an encrypted backup to a temp file and return (archive_path, mode, temp_path).

    The caller is responsible for cleaning up temp_path if not None.
    """
    archive_path = backup.file_path
    key_path = backup.encryption_key_path

    if not key_path:
        # Try to find key file next to the backup
        potential_key = archive_path.replace(".enc", ".key")
        if os.path.exists(potential_key):
            key_path = potential_key
        else:
            raise HTTPException(
                status_code=400,
                detail="Encryption key file (.key) not found for this backup.",
            )

    if not os.path.exists(key_path):
        raise HTTPException(
            status_code=404,
            detail="Encryption key file not found on disk.",
        )

    # Decrypt to temp file
    temp_fd, temp_name = tempfile.mkstemp(suffix=".tar.gz")
    os.close(temp_fd)
    temp_path = Path(temp_name)

    try:
        await decrypt_backup(
            encrypted_path=Path(archive_path),
            key_path=Path(key_path),
            private_key=private_key,
            output_path=temp_path,
        )
    except DecryptionError:
        temp_path.unlink(missing_ok=True)
        raise HTTPException(
            status_code=400,
            detail="Decryption failed. Is the private key correct?",
        )

    decrypted_path = str(temp_path)
    mode = _get_tar_mode(decrypted_path)
    return decrypted_path, mode, temp_path


def _get_tar_mode(archive_path: str) -> str:
    """Determine tarfile open mode from file extension."""
    if archive_path.endswith(".tar.gz") or archive_path.endswith(".tgz"):
        return "r:gz"
    elif archive_path.endswith(".tar.bz2"):
        return "r:bz2"
    elif archive_path.endswith(".tar.xz"):
        return "r:xz"
    elif archive_path.endswith(".tar"):
        return "r:"
    else:
        raise HTTPException(status_code=400, detail="Unsupported archive format")


class BrowseEncryptedRequest(BaseModel):
    """Request body for browsing encrypted backups."""

    private_key: str


@router.post("/{backup_id}/files", response_model=List[BackupFileInfo])
async def list_encrypted_backup_files(backup_id: int, body: BrowseEncryptedRequest):
    """List files in an encrypted backup archive using the provided private key."""
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        if backup.status != BackupStatus.COMPLETED:
            raise HTTPException(status_code=400, detail="Backup is not completed")

        if not backup.file_path or not os.path.exists(backup.file_path):
            raise HTTPException(status_code=404, detail="Backup file not found")

        if not backup.encrypted:
            raise HTTPException(
                status_code=400,
                detail="Backup is not encrypted. Use GET endpoint instead.",
            )

    temp_path: Path | None = None
    try:
        archive_path, mode, temp_path = await _get_decrypted_archive_path(
            backup, body.private_key
        )

        files: List[BackupFileInfo] = []
        with tarfile.open(archive_path, mode) as tar:
            for member in tar.getmembers():
                files.append(
                    BackupFileInfo(
                        name=os.path.basename(member.name) or member.name,
                        path=member.name,
                        size=member.size,
                        size_human=_format_file_size(member.size),
                        is_dir=member.isdir(),
                        mode=oct(member.mode)[2:] if member.mode else "0",
                        mtime=(
                            datetime.fromtimestamp(member.mtime).isoformat()
                            if member.mtime
                            else ""
                        ),
                    )
                )

        return files

    except tarfile.TarError as e:
        logger.error("Failed to read decrypted archive: %s", e)
        raise HTTPException(status_code=500, detail="Failed to read backup archive")
    finally:
        if temp_path and temp_path.exists():
            temp_path.unlink(missing_ok=True)


@router.post("/{backup_id}/files/{file_path:path}")
async def download_encrypted_backup_file(
    backup_id: int, file_path: str, body: BrowseEncryptedRequest
):
    """Download a specific file from an encrypted backup archive."""
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        if backup.status != BackupStatus.COMPLETED:
            raise HTTPException(status_code=400, detail="Backup is not completed")

        if not backup.file_path or not os.path.exists(backup.file_path):
            raise HTTPException(status_code=404, detail="Backup file not found")

        if not backup.encrypted:
            raise HTTPException(
                status_code=400,
                detail="Backup is not encrypted. Use GET endpoint instead.",
            )

    temp_path: Path | None = None
    tar = None
    try:
        archive_path, mode, temp_path = await _get_decrypted_archive_path(
            backup, body.private_key
        )

        tar = tarfile.open(archive_path, mode)
        normalized_path = _validate_archive_path(file_path)

        member = tar.getmember(normalized_path)

        if member.isdir():
            tar.close()
            if temp_path and temp_path.exists():
                temp_path.unlink(missing_ok=True)
            raise HTTPException(status_code=400, detail="Cannot download directories")

        file_obj = tar.extractfile(member)
        if file_obj is None:
            tar.close()
            if temp_path and temp_path.exists():
                temp_path.unlink(missing_ok=True)
            raise HTTPException(status_code=500, detail="Failed to extract file")

        filename = _sanitize_filename(member.name)
        cleanup_temp = temp_path

        def iter_file():
            try:
                while chunk := file_obj.read(65536):
                    yield chunk
            finally:
                file_obj.close()
                tar.close()
                if cleanup_temp and cleanup_temp.exists():
                    cleanup_temp.unlink(missing_ok=True)

        return StreamingResponse(
            iter_file(),
            media_type="application/octet-stream",
            headers={
                "Content-Disposition": f'attachment; filename="{filename}"',
                "Cache-Control": "no-store",
            },
        )

    except KeyError:
        if tar:
            tar.close()
        if temp_path and temp_path.exists():
            temp_path.unlink(missing_ok=True)
        raise HTTPException(status_code=404, detail="File not found in archive")
    except tarfile.TarError as e:
        if tar:
            tar.close()
        if temp_path and temp_path.exists():
            temp_path.unlink(missing_ok=True)
        logger.error("Failed to extract file from decrypted archive: %s", e)
        raise HTTPException(status_code=500, detail="Failed to read backup archive")


@router.get("/{backup_id}/files", response_model=List[BackupFileInfo])
async def list_backup_files(backup_id: int):
    """List all files in a backup archive.

    Returns a flat list of all files and directories in the backup
    with their sizes and metadata.
    """
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        if backup.status != BackupStatus.COMPLETED:
            raise HTTPException(status_code=400, detail="Backup is not completed")

        if not backup.file_path or not os.path.exists(backup.file_path):
            raise HTTPException(status_code=404, detail="Backup file not found")

    files: List[BackupFileInfo] = []

    try:
        # Handle encrypted backups
        archive_path = backup.file_path
        if backup.encrypted and archive_path.endswith(".enc"):
            raise HTTPException(
                status_code=400,
                detail="Cannot browse encrypted backups without private key. Use POST endpoint with your private key.",
            )

        mode = _get_tar_mode(archive_path)

        with tarfile.open(archive_path, mode) as tar:
            for member in tar.getmembers():
                files.append(
                    BackupFileInfo(
                        name=os.path.basename(member.name) or member.name,
                        path=member.name,
                        size=member.size,
                        size_human=_format_file_size(member.size),
                        is_dir=member.isdir(),
                        mode=oct(member.mode)[2:] if member.mode else "0",
                        mtime=(
                            datetime.fromtimestamp(member.mtime).isoformat()
                            if member.mtime
                            else ""
                        ),
                    )
                )

    except tarfile.TarError as e:
        logger.error("Failed to read archive: %s", e)
        raise HTTPException(status_code=500, detail="Failed to read backup archive")

    return files


@router.get("/{backup_id}/files/{file_path:path}")
async def download_backup_file(backup_id: int, file_path: str):
    """Download a specific file from a backup archive.

    The file is extracted and streamed to the client.
    """
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        if backup.status != BackupStatus.COMPLETED:
            raise HTTPException(status_code=400, detail="Backup is not completed")

        if not backup.file_path or not os.path.exists(backup.file_path):
            raise HTTPException(status_code=404, detail="Backup file not found")

    # Handle encrypted backups
    archive_path = backup.file_path
    if backup.encrypted and archive_path.endswith(".enc"):
        raise HTTPException(
            status_code=400,
            detail="Cannot download from encrypted backups without private key. Use POST endpoint with your private key.",
        )

    mode = _get_tar_mode(archive_path)

    tar = None
    try:
        tar = tarfile.open(archive_path, mode)
        normalized_path = _validate_archive_path(file_path)

        member = tar.getmember(normalized_path)

        if member.isdir():
            tar.close()
            raise HTTPException(status_code=400, detail="Cannot download directories")

        file_obj = tar.extractfile(member)
        if file_obj is None:
            tar.close()
            raise HTTPException(status_code=500, detail="Failed to extract file")

        filename = _sanitize_filename(member.name)

        def iter_file():
            try:
                while chunk := file_obj.read(65536):
                    yield chunk
            finally:
                file_obj.close()
                tar.close()

        return StreamingResponse(
            iter_file(),
            media_type="application/octet-stream",
            headers={
                "Content-Disposition": f'attachment; filename="{filename}"',
                "Cache-Control": "no-store",
            },
        )

    except KeyError:
        if tar:
            tar.close()
        raise HTTPException(status_code=404, detail="File not found in archive")
    except tarfile.TarError as e:
        if tar:
            tar.close()
        logger.error("Failed to extract file from archive: %s", e)
        raise HTTPException(status_code=500, detail="Failed to read backup archive")
