"""
Backups API endpoints.
"""

import logging
import os
import re
import shutil
import tarfile
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import List, Literal, Optional

from fastapi import APIRouter, HTTPException, Query, Request
from fastapi.responses import StreamingResponse
from pydantic import BaseModel
from sqlalchemy import select

from app.backup_engine import backup_engine
from app.config import settings
from app.credential_encryption import decrypt_value
from app.database import (
    Backup,
    BackupLog,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    BackupType,
    RemoteStorage,
    async_session,
)
from app.encryption import DecryptionError, decrypt_backup
from app.rate_limit import limiter
from app.remote_storage import StorageConfig, StorageType, storage_manager

logger = logging.getLogger(__name__)

router = APIRouter()

# Characters that _sanitize_path_name replaces with underscores
_UNSAFE_PATH_CHARS = re.compile(r'[<>:"|?*]')


def _resolve_backup_path(file_path: str | None) -> str | None:
    """Resolve a backup file path, handling legacy paths with unsafe characters.

    Old backups may have been stored with names containing characters like ':'
    that were later sanitized by _sanitize_path_name.  This helper checks both
    the stored path AND a sanitised variant so that old DB records still resolve
    after the path-sanitisation fix has been applied.

    Returns the first existing path, or None if neither variant exists.
    """
    if not file_path:
        return None

    # Try the exact path first (most common case)
    if os.path.exists(file_path):
        return file_path

    # Try sanitised variant: replace unsafe chars with '_' and collapse
    sanitized = _UNSAFE_PATH_CHARS.sub("_", file_path)
    sanitized = re.sub(r"_+", "_", sanitized)
    if sanitized != file_path and os.path.exists(sanitized):
        logger.info(
            "Resolved backup via sanitised path fallback: %s -> %s",
            file_path,
            sanitized,
        )
        return sanitized

    return None


def _db_to_storage_config(storage: RemoteStorage) -> StorageConfig:
    """Convert a RemoteStorage DB model to a StorageConfig for backend init."""
    return StorageConfig(
        id=storage.id,
        name=storage.name,
        storage_type=StorageType(storage.storage_type),
        enabled=storage.enabled,
        host=storage.host,
        port=storage.port,
        username=storage.username,
        password=decrypt_value(storage.password)
        if storage.password
        else storage.password,
        base_path=storage.base_path,
        ssh_key_path=storage.ssh_key_path,
        s3_bucket=storage.s3_bucket,
        s3_region=storage.s3_region,
        s3_access_key=decrypt_value(storage.s3_access_key)
        if storage.s3_access_key
        else storage.s3_access_key,
        s3_secret_key=decrypt_value(storage.s3_secret_key)
        if storage.s3_secret_key
        else storage.s3_secret_key,
        s3_endpoint_url=storage.s3_endpoint_url,
        webdav_url=storage.webdav_url,
        rclone_remote=storage.rclone_remote,
    )


async def _ensure_backup_locally(
    backup: Backup,
) -> tuple[str, Path | None]:
    """Ensure backup file is available locally, downloading from remote if needed.

    Returns (local_path, temp_dir) where temp_dir is not None when a remote
    download was performed and must be cleaned up by the caller.
    """
    resolved = _resolve_backup_path(backup.file_path)
    if resolved:
        return resolved, None

    # Local file missing – try downloading from a remote storage that has it
    async with async_session() as session:
        result = await session.execute(
            select(BackupStorageSync)
            .where(
                BackupStorageSync.backup_id == backup.id,
                BackupStorageSync.sync_status == "completed",
            )
            .order_by(BackupStorageSync.synced_at.desc())
        )
        syncs = result.scalars().all()

        if not syncs:
            return None, None

        for sync in syncs:
            backend = storage_manager.get_backend(sync.storage_id)
            if not backend:
                storage = await session.get(RemoteStorage, sync.storage_id)
                if not storage or not storage.enabled:
                    continue
                config = _db_to_storage_config(storage)
                backend = storage_manager.add_storage(config)

            temp_dir = Path(tempfile.mkdtemp(prefix="dockervault_remote_"))
            filename = os.path.basename(sync.remote_path)
            local_path = temp_dir / filename

            try:
                success = await backend.download(sync.remote_path, local_path)
                if success and local_path.exists():
                    logger.info(
                        "Downloaded backup %s from remote storage %s",
                        backup.id,
                        sync.storage_id,
                    )

                    # Also download .key sidecar file for encrypted backups
                    if backup.encrypted:
                        key_remote = sync.remote_path.replace(".enc", ".key")
                        key_local = temp_dir / filename.replace(".enc", ".key")
                        try:
                            await backend.download(key_remote, key_local)
                            # Validate that the downloaded key file is non-empty
                            if key_local.exists() and key_local.stat().st_size == 0:
                                logger.warning(
                                    "Downloaded .key sidecar is empty for backup %s, removing",
                                    backup.id,
                                )
                                key_local.unlink(missing_ok=True)
                        except Exception:
                            # Clean up potentially empty/partial file from failed download
                            if isinstance(key_local, Path) and key_local.exists():
                                key_local.unlink(missing_ok=True)
                            logger.debug(
                                "No .key sidecar found on remote for backup %s",
                                backup.id,
                            )

                    return str(local_path), temp_dir
            except Exception as e:
                logger.warning(
                    "Failed to download backup %s from storage %s: %s",
                    backup.id,
                    sync.storage_id,
                    e,
                )
                shutil.rmtree(temp_dir, ignore_errors=True)
                continue

    return None, None


def _to_utc_isoformat(dt: Optional[datetime]) -> Optional[str]:
    """Serialize a datetime to ISO format with UTC timezone indicator.

    SQLite stores datetimes without timezone info. Since all datetimes
    in the database are UTC, this appends 'Z' to naive datetimes so
    the frontend correctly interprets them as UTC.
    """
    if dt is None:
        return None
    if dt.tzinfo is None:
        return dt.isoformat() + "Z"
    return dt.astimezone(timezone.utc).isoformat()


class RemoteSyncInfo(BaseModel):
    """Remote sync status for a single storage backend."""

    storage_id: int
    storage_name: str
    status: str  # completed, failed, pending
    synced_at: Optional[str] = None
    remote_path: Optional[str] = None


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
    local_available: bool = True
    remote_synced: bool = False
    remote_sync_details: List[RemoteSyncInfo] = []
    created_at: str

    class Config:
        from_attributes = True


class CreateBackupRequest(BaseModel):
    """Create backup request."""

    target_id: int
    backup_type: Literal["full", "incremental"] = "full"


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


async def _get_sync_details(
    session, backup_ids: list[int]
) -> dict[int, list[RemoteSyncInfo]]:
    """Load remote sync details for a list of backup IDs.

    Returns a dict mapping backup_id -> list[RemoteSyncInfo].
    """
    if not backup_ids:
        return {}

    result = await session.execute(
        select(BackupStorageSync, RemoteStorage.name)
        .join(RemoteStorage, BackupStorageSync.storage_id == RemoteStorage.id)
        .where(BackupStorageSync.backup_id.in_(backup_ids))
    )
    rows = result.all()

    details: dict[int, list[RemoteSyncInfo]] = {}
    for sync, storage_name in rows:
        info = RemoteSyncInfo(
            storage_id=sync.storage_id,
            storage_name=storage_name,
            status=sync.sync_status or "unknown",
            synced_at=_to_utc_isoformat(sync.synced_at),
            remote_path=sync.remote_path,
        )
        details.setdefault(sync.backup_id, []).append(info)
    return details


def _build_backup_response(
    b: Backup,
    target_name: Optional[str],
    sync_details: list[RemoteSyncInfo],
) -> BackupResponse:
    """Build a BackupResponse from a Backup record with sync info."""
    local_available = _resolve_backup_path(b.file_path) is not None
    any_completed = any(s.status == "completed" for s in sync_details)
    return BackupResponse(
        id=b.id,
        target_id=b.target_id,
        target_name=target_name,
        backup_type=b.backup_type.value,
        status=b.status.value,
        file_path=b.file_path,
        file_size=b.file_size,
        file_size_human=format_size(b.file_size),
        checksum=b.checksum,
        started_at=_to_utc_isoformat(b.started_at),
        completed_at=_to_utc_isoformat(b.completed_at),
        duration_seconds=b.duration_seconds,
        error_message=b.error_message,
        encrypted=b.encrypted or False,
        local_available=local_available,
        remote_synced=any_completed,
        remote_sync_details=sync_details,
        created_at=_to_utc_isoformat(b.created_at) or "",
    )


async def _delete_remote_copies(session, backup_ids: list[int]) -> list[str]:
    """Delete remote copies for the given backup IDs.

    Returns a list of error messages for failed deletions.
    """
    from app.remote_storage import storage_manager

    result = await session.execute(
        select(BackupStorageSync).where(BackupStorageSync.backup_id.in_(backup_ids))
    )
    sync_records = result.scalars().all()

    remote_errors: list[str] = []
    for sync in sync_records:
        if sync.remote_path and sync.sync_status == "completed":
            backend = storage_manager.get_backend(sync.storage_id)
            if backend:
                try:
                    await backend.delete(sync.remote_path)
                except Exception as e:
                    logger.warning(
                        "Could not delete remote file %s from storage %s: %s",
                        sync.remote_path,
                        sync.storage_id,
                        e,
                    )
                    remote_errors.append(
                        f"storage:{sync.storage_id}/{sync.remote_path}"
                    )
        await session.delete(sync)

    return remote_errors


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

        # Get remote sync details
        backup_ids = [b.id for b in backups]
        sync_map = await _get_sync_details(session, backup_ids)

        return [
            _build_backup_response(
                b,
                targets.get(b.target_id),
                sync_map.get(b.id, []),
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

        # Get remote sync details
        sync_map = await _get_sync_details(session, [backup.id])

        return _build_backup_response(
            backup,
            target.name if target else None,
            sync_map.get(backup.id, []),
        )


@router.post("/{backup_id}/retry-sync")
async def retry_remote_sync(backup_id: int):
    """Retry remote sync for a backup that has failed storage uploads.

    Re-runs the upload for every BackupStorageSync record with status 'failed'.
    Returns updated sync details.
    """
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()
        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")
        if backup.status != BackupStatus.COMPLETED:
            raise HTTPException(
                status_code=400,
                detail="Only completed backups can be re-synced",
            )

        resolved = _resolve_backup_path(backup.file_path)
        if not resolved:
            raise HTTPException(
                status_code=400,
                detail="Local backup file not available for re-sync",
            )

        # Get target for path sanitisation
        target_result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == backup.target_id)
        )
        target = target_result.scalar_one_or_none()
        if not target:
            raise HTTPException(status_code=404, detail="Backup target not found")

        # Find failed sync records
        failed_result = await session.execute(
            select(BackupStorageSync).where(
                BackupStorageSync.backup_id == backup_id,
                BackupStorageSync.sync_status == "failed",
            )
        )
        failed_syncs = list(failed_result.scalars().all())

        # Also find storage IDs that have no sync record at all
        # (remote was configured after backup, or initial sync never ran)
        missing_storage_ids: list[int] = []
        if target.remote_storage_ids:
            existing_result = await session.execute(
                select(BackupStorageSync.storage_id).where(
                    BackupStorageSync.backup_id == backup_id
                )
            )
            existing_ids = {row[0] for row in existing_result}
            missing_storage_ids = [
                sid for sid in target.remote_storage_ids if sid not in existing_ids
            ]

        if not failed_syncs and not missing_storage_ids:
            raise HTTPException(
                status_code=400,
                detail="No failed or missing sync records to retry",
            )

    # Re-upload to each failed storage
    local_path = Path(resolved)
    safe_name = backup_engine._sanitize_path_name(target.name)
    retried: dict[int, bool] = {}

    for sync in failed_syncs:
        backend = storage_manager.get_backend(sync.storage_id)
        if not backend:
            async with async_session() as session:
                storage = await session.get(RemoteStorage, sync.storage_id)
                if not storage or not storage.enabled:
                    retried[sync.storage_id] = False
                    continue
                config = _db_to_storage_config(storage)
                backend = storage_manager.add_storage(config)

        remote_path = sync.remote_path or f"{safe_name}/{local_path.name}"
        try:
            success = await backend.upload(local_path, remote_path)
        except Exception as e:
            logger.warning(
                "Retry sync failed for backup %s -> storage %s: %s",
                backup_id,
                sync.storage_id,
                e,
            )
            success = False

        retried[sync.storage_id] = success

        # Update sync record
        async with async_session() as session:
            result = await session.execute(
                select(BackupStorageSync).where(
                    BackupStorageSync.backup_id == backup_id,
                    BackupStorageSync.storage_id == sync.storage_id,
                )
            )
            record = result.scalar_one_or_none()
            if record:
                if success:
                    record.sync_status = "completed"
                    record.synced_at = datetime.now(timezone.utc)
                    record.error_message = None
                else:
                    record.error_message = "Retry upload failed"
                await session.commit()

        # Also upload .key sidecar for encrypted backups on success
        if success and backup.encrypted and backup.encryption_key_path:
            key_path = Path(backup.encryption_key_path)
            if key_path.exists():
                key_remote = remote_path.replace(".enc", ".key")
                try:
                    await backend.upload(key_path, key_remote)
                except Exception:
                    pass

    # Handle storages with no sync record yet (remote configured after backup)
    for storage_id in missing_storage_ids:
        backend = storage_manager.get_backend(storage_id)
        if not backend:
            async with async_session() as session:
                storage = await session.get(RemoteStorage, storage_id)
                if not storage or not storage.enabled:
                    retried[storage_id] = False
                    continue
                config = _db_to_storage_config(storage)
                backend = storage_manager.add_storage(config)

        remote_path = f"{safe_name}/{local_path.name}"
        try:
            success = await backend.upload(local_path, remote_path)
        except Exception as e:
            logger.warning(
                "Initial sync failed for backup %s -> storage %s: %s",
                backup_id,
                storage_id,
                e,
            )
            success = False

        retried[storage_id] = success

        # Create new sync record
        async with async_session() as session:
            sync_record = BackupStorageSync(
                backup_id=backup_id,
                storage_id=storage_id,
                remote_path=remote_path,
                synced_at=datetime.now(timezone.utc) if success else None,
                sync_status="completed" if success else "failed",
                error_message=None if success else "Upload failed",
            )
            session.add(sync_record)
            await session.commit()

        # Also upload .key sidecar for encrypted backups on success
        if success and backup.encrypted and backup.encryption_key_path:
            key_path = Path(backup.encryption_key_path)
            if key_path.exists():
                key_remote = remote_path.replace(".enc", ".key")
                try:
                    await backend.upload(key_path, key_remote)
                except Exception:
                    pass

    succeeded = sum(1 for v in retried.values() if v)
    failed = sum(1 for v in retried.values() if not v)

    # Return updated sync details
    async with async_session() as session:
        sync_map = await _get_sync_details(session, [backup_id])

    return {
        "message": f"Retry complete: {succeeded} succeeded, {failed} failed",
        "succeeded": succeeded,
        "failed": failed,
        "sync_details": sync_map.get(backup_id, []),
    }


@router.post("", response_model=BackupResponse)
@limiter.limit("10/minute")
async def create_backup(request: Request, request_body: CreateBackupRequest):
    """Create and run a new backup."""
    logger.info(
        f"Creating backup for target {request_body.target_id}, type: {request_body.backup_type}"
    )

    async with async_session() as session:
        # Get target
        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == request_body.target_id)
        )
        target = result.scalar_one_or_none()

        if not target:
            logger.error("Target %s not found", request_body.target_id)
            raise HTTPException(status_code=404, detail="Target not found")

        logger.info(f"Found target: {target.name} (type: {target.target_type})")

    # Create backup
    backup_type = (
        BackupType.FULL
        if request_body.backup_type == "full"
        else BackupType.INCREMENTAL
    )
    backup = await backup_engine.create_backup(target, backup_type)

    logger.info(f"Created backup {backup.id}, starting backup task...")

    # Enqueue backup for processing (per-target lock prevents overlaps)
    await backup_engine.enqueue(backup.id)

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
        started_at=_to_utc_isoformat(backup.started_at),
        completed_at=_to_utc_isoformat(backup.completed_at),
        duration_seconds=backup.duration_seconds,
        error_message=backup.error_message,
        encrypted=backup.encrypted or False,
        created_at=_to_utc_isoformat(backup.created_at) or "",
    )


@router.get("/{backup_id}/restore-info")
async def get_restore_info(backup_id: int):
    """Return metadata needed by the Restore Wizard.

    Includes backup details, original target info, whether the backup is
    encrypted, and for volume backups a list of available Docker volumes that
    can serve as an alternative restore destination.
    """
    from app.docker_client import docker_client

    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()
        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")
        if backup.status != BackupStatus.COMPLETED:
            raise HTTPException(
                status_code=400, detail="Only completed backups can be restored"
            )

        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == backup.target_id)
        )
        target = result.scalar_one_or_none()

    local_available = _resolve_backup_path(backup.file_path) is not None

    # For volume backups list available volumes as alternative destinations
    available_volumes: list[str] = []
    if target and target.target_type == "volume":
        try:
            volumes = await docker_client.list_volumes()
            available_volumes = sorted(v.name for v in volumes)
        except Exception:
            pass

    containers_to_stop: list[str] = []
    if target:
        if target.stop_container and target.container_name:
            containers_to_stop.append(target.container_name)
        if target.dependencies:
            containers_to_stop.extend(target.dependencies)

    return {
        "backup": {
            "id": backup.id,
            "file_size_human": format_size(backup.file_size),
            "encrypted": backup.encrypted or False,
            "created_at": _to_utc_isoformat(backup.created_at),
            "completed_at": _to_utc_isoformat(backup.completed_at),
            "local_available": local_available,
        },
        "target": {
            "id": target.id if target else None,
            "name": target.name if target else "Unknown",
            "target_type": target.target_type if target else None,
            "volume_name": target.volume_name if target else None,
            "host_path": target.host_path if target else None,
            "container_name": target.container_name if target else None,
        },
        "containers_to_stop": containers_to_stop,
        "available_volumes": available_volumes,
    }


@router.post("/{backup_id}/restore")
@limiter.limit("10/minute")
async def restore_backup(
    request: Request, backup_id: int, restore_request: RestoreBackupRequest
):
    """Restore a backup.

    If target_path is provided, it must be within allowed restore directories
    to prevent path traversal attacks.
    """
    if restore_request.target_path:
        # Resolve symlinks and normalize to prevent TOCTOU attacks
        abs_path = os.path.realpath(restore_request.target_path)

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
        backup_id, restore_request.target_path, restore_request.private_key
    )

    if not success:
        raise HTTPException(status_code=500, detail="Restore failed")

    return {"status": "restored"}


@router.delete("/{backup_id}")
@limiter.limit("20/minute")
async def delete_backup(
    request: Request,
    backup_id: int,
    delete_local: bool = Query(True, description="Delete local backup file"),
    delete_remote: bool = Query(True, description="Delete remote copies"),
):
    """Delete a backup. Use query params to control what gets deleted.

    - delete_local=true (default): Removes local file and DB record.
    - delete_remote=true (default): Removes remote copies.
    - Both false is rejected (nothing to do).
    """
    if not delete_local and not delete_remote:
        raise HTTPException(
            status_code=400,
            detail="At least one of delete_local or delete_remote must be true.",
        )

    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        file_errors: list[str] = []

        # Delete local file if requested
        if delete_local and backup.file_path:
            resolved = _resolve_backup_path(backup.file_path)
            if resolved:
                try:
                    os.remove(resolved)
                except OSError as e:
                    logger.warning("Could not delete backup file %s: %s", resolved, e)
                    file_errors.append(str(resolved))
            # Also try to remove the .key sidecar file
            if backup.encryption_key_path:
                resolved_key = _resolve_backup_path(backup.encryption_key_path)
                if resolved_key:
                    try:
                        os.remove(resolved_key)
                    except OSError as e:
                        logger.warning(
                            "Could not delete key file %s: %s", resolved_key, e
                        )
                        file_errors.append(str(resolved_key))

            # Try to remove the parent directory if it is now empty
            if resolved and not file_errors:
                parent = os.path.dirname(resolved)
                try:
                    if parent and os.path.isdir(parent) and not os.listdir(parent):
                        os.rmdir(parent)
                except OSError:
                    pass  # non-critical

        # Delete remote copies if requested
        if delete_remote:
            remote_errors = await _delete_remote_copies(session, [backup.id])
            file_errors.extend(remote_errors)

        # Only delete DB record when local is being removed (or already gone)
        if delete_local:
            await session.delete(backup)
        elif delete_remote:
            # Keep DB record but clear sync records (already done in _delete_remote_copies)
            pass

        await session.commit()

        if file_errors:
            return {
                "status": "deleted",
                "warning": (
                    "Database record removed but some files could not be "
                    f"deleted (permission denied): {file_errors}"
                ),
            }
        return {"status": "deleted"}


@router.delete("")
@limiter.limit("10/minute")
async def delete_all_backups(
    request: Request,
    target_id: int | None = Query(
        None, description="Delete only backups for this target"
    ),
    delete_local: bool = Query(True, description="Delete local backup files"),
    delete_remote: bool = Query(True, description="Delete remote copies"),
):
    """Delete all backups, optionally filtered by target_id.

    Use delete_local/delete_remote to control scope.
    """
    if not delete_local and not delete_remote:
        raise HTTPException(
            status_code=400,
            detail="At least one of delete_local or delete_remote must be true.",
        )

    async with async_session() as session:
        query = select(Backup)
        if target_id is not None:
            query = query.where(Backup.target_id == target_id)

        result = await session.execute(query)
        backups = result.scalars().all()

        if not backups:
            return {"status": "deleted", "deleted_count": 0}

        deleted_count = 0
        file_errors: list[str] = []
        backup_ids = [b.id for b in backups]

        # Delete remote copies if requested
        if delete_remote:
            remote_errors = await _delete_remote_copies(session, backup_ids)
            file_errors.extend(remote_errors)

        for backup in backups:
            # Delete local file if requested
            if delete_local and backup.file_path:
                resolved = _resolve_backup_path(backup.file_path)
                if resolved:
                    try:
                        os.remove(resolved)
                    except OSError as e:
                        logger.warning(
                            "Could not delete backup file %s: %s", resolved, e
                        )
                        file_errors.append(str(resolved))

                # Delete .key sidecar
                if backup.encryption_key_path:
                    resolved_key = _resolve_backup_path(backup.encryption_key_path)
                    if resolved_key:
                        try:
                            os.remove(resolved_key)
                        except OSError as e:
                            logger.warning(
                                "Could not delete key file %s: %s",
                                resolved_key,
                                e,
                            )
                            file_errors.append(str(resolved_key))

            if delete_local:
                await session.delete(backup)
                deleted_count += 1

        await session.commit()

        # Clean up empty target directories
        if delete_local:
            for backup in backups:
                if backup.file_path:
                    resolved = _resolve_backup_path(backup.file_path)
                    if resolved:
                        parent = os.path.dirname(resolved)
                        try:
                            if (
                                parent
                                and os.path.isdir(parent)
                                and not os.listdir(parent)
                            ):
                                os.rmdir(parent)
                        except OSError:
                            pass

        if not delete_local:
            deleted_count = len(backups)

        response: dict = {"status": "deleted", "deleted_count": deleted_count}
        if file_errors:
            response["warning"] = (
                f"{deleted_count} record(s) processed but {len(file_errors)} "
                f"file(s) could not be deleted: {file_errors}"
            )
        return response


class BackupLogEntry(BaseModel):
    id: int
    level: str
    step: str
    message: str
    details: Optional[dict] = None
    created_at: datetime


@router.get("/{backup_id}/logs", response_model=List[BackupLogEntry])
async def get_backup_logs(backup_id: int):
    """Get structured log entries for a backup job."""
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        if not result.scalar_one_or_none():
            raise HTTPException(status_code=404, detail="Backup not found")

        result = await session.execute(
            select(BackupLog)
            .where(BackupLog.backup_id == backup_id)
            .order_by(BackupLog.created_at.asc())
        )
        logs = result.scalars().all()

        return [
            BackupLogEntry(
                id=log.id,
                level=log.level.value,
                step=log.step,
                message=log.message,
                details=log.details,
                created_at=log.created_at,
            )
            for log in logs
        ]


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


async def _download_key_from_remote(
    backup: Backup, resolved_archive: str
) -> str | None:
    """Try to download the .key sidecar from remote storage.

    Returns the local path to the downloaded .key file, or None on failure.
    The file is placed next to *resolved_archive* so that callers (and any
    future local-path lookups) find it without extra work.
    """
    async with async_session() as session:
        result = await session.execute(
            select(BackupStorageSync).where(
                BackupStorageSync.backup_id == backup.id,
                BackupStorageSync.sync_status == "completed",
            )
        )
        syncs = result.scalars().all()
        if not syncs:
            return None

        archive_dir = os.path.dirname(resolved_archive)
        archive_basename = os.path.basename(resolved_archive)
        key_basename = archive_basename.replace(".enc", ".key")
        key_local = os.path.join(archive_dir, key_basename)

        for sync in syncs:
            key_remote = sync.remote_path.replace(".enc", ".key")
            backend = storage_manager.get_backend(sync.storage_id)
            if not backend:
                storage = await session.get(RemoteStorage, sync.storage_id)
                if not storage or not storage.enabled:
                    continue
                config = _db_to_storage_config(storage)
                backend = storage_manager.add_storage(config)

            try:
                success = await backend.download(key_remote, Path(key_local))
                if success and os.path.exists(key_local):
                    # Validate the downloaded key is non-empty
                    if os.path.getsize(key_local) == 0:
                        logger.warning(
                            "Downloaded .key sidecar is empty from storage %s for backup %s, removing",
                            sync.storage_id,
                            backup.id,
                        )
                        os.unlink(key_local)
                        continue
                    logger.info(
                        "Downloaded .key sidecar from remote for backup %s",
                        backup.id,
                    )
                    return key_local
            except Exception as exc:
                # Clean up potentially empty/partial file from failed download
                if os.path.exists(key_local):
                    try:
                        os.unlink(key_local)
                    except OSError:
                        pass
                logger.debug(
                    "Failed to download .key from storage %s for backup %s: %s",
                    sync.storage_id,
                    backup.id,
                    exc,
                )
    return None


def _is_valid_key_file(path: str | None) -> bool:
    """Return True if *path* points to a non-empty file (a plausible .key)."""
    if not path:
        return False
    try:
        return os.path.getsize(path) > 0
    except OSError:
        return False


async def _get_decrypted_archive_path(
    backup: Backup, private_key: str
) -> tuple[str, str, Path | None]:
    """Decrypt an encrypted backup to a temp file and return (archive_path, mode, temp_path).

    The caller is responsible for cleaning up temp_path if not None.
    """
    # Resolve archive path on disk first (handles legacy/sanitized path differences)
    resolved_archive = _resolve_backup_path(backup.file_path)
    if not resolved_archive:
        raise HTTPException(
            status_code=404,
            detail="Backup archive file not found on disk.",
        )

    # Find the encryption key file – try multiple strategies.
    # Each step validates the candidate is a non-empty file; empty files left
    # behind by failed downloads are removed so the remote fallback can run.
    resolved_key: str | None = None

    def _try_key_candidate(candidate: str | None) -> str | None:
        """Return *candidate* if it is a valid key file, else clean up & return None."""
        if not candidate or not os.path.exists(candidate):
            return None
        if _is_valid_key_file(candidate):
            return candidate
        # Empty / corrupt – remove so it does not block future lookups
        logger.warning(
            "Removing empty/corrupt .key file for backup %s: %s",
            backup.id,
            candidate,
        )
        try:
            os.unlink(candidate)
        except OSError:
            pass
        return None

    # 1. Try the DB-stored key path
    if backup.encryption_key_path:
        resolved_key = _try_key_candidate(
            _resolve_backup_path(backup.encryption_key_path)
        )

    # 2. Derive .key path from the resolved archive path (.enc -> .key)
    if not resolved_key:
        resolved_key = _try_key_candidate(resolved_archive.replace(".enc", ".key"))

    # 3. Derive from the original DB path (handles legacy naming)
    if not resolved_key:
        potential_key = backup.file_path.replace(".enc", ".key")
        resolved_key = _try_key_candidate(_resolve_backup_path(potential_key))

    # 4. Look for .key in the same directory as the resolved archive
    #    (covers temp dirs from remote downloads)
    if not resolved_key:
        archive_dir = os.path.dirname(resolved_archive)
        archive_basename = os.path.basename(resolved_archive)
        key_basename = archive_basename.replace(".enc", ".key")
        resolved_key = _try_key_candidate(os.path.join(archive_dir, key_basename))

    # 5. Download .key from remote storage as last resort
    if not resolved_key:
        resolved_key = await _download_key_from_remote(backup, resolved_archive)

    if not resolved_key:
        raise HTTPException(
            status_code=404,
            detail="Encryption key file (.key) not found on disk or remote storage.",
        )

    # Decrypt to temp file
    temp_fd, temp_name = tempfile.mkstemp(suffix=".tar.gz")
    os.close(temp_fd)
    temp_path = Path(temp_name)

    try:
        await decrypt_backup(
            encrypted_path=Path(resolved_archive),
            key_path=Path(resolved_key),
            private_key=private_key,
            output_path=temp_path,
        )
    except DecryptionError as e:
        logger.error("Backup decryption failed for backup %s: %s", backup.id, e)
        temp_path.unlink(missing_ok=True)
        error_msg = str(e)
        if "empty" in error_msg.lower() or "corrupted" in error_msg.lower():
            detail = f"Decryption failed: {error_msg}"
        else:
            detail = "Decryption failed. Is the private key correct?"
        raise HTTPException(status_code=400, detail=detail)

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

        if not backup.encrypted:
            raise HTTPException(
                status_code=400,
                detail="Backup is not encrypted. Use GET endpoint instead.",
            )

    remote_temp_dir: Path | None = None
    temp_path: Path | None = None
    try:
        resolved_path, remote_temp_dir = await _ensure_backup_locally(backup)
        if not resolved_path:
            raise HTTPException(
                status_code=404,
                detail="Backup file not found locally or on any remote storage.",
            )
        backup.file_path = resolved_path

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
        if remote_temp_dir:
            shutil.rmtree(remote_temp_dir, ignore_errors=True)


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

        if not backup.encrypted:
            raise HTTPException(
                status_code=400,
                detail="Backup is not encrypted. Use GET endpoint instead.",
            )

    remote_temp_dir: Path | None = None
    temp_path: Path | None = None
    tar = None
    try:
        resolved_path, remote_temp_dir = await _ensure_backup_locally(backup)
        if not resolved_path:
            raise HTTPException(
                status_code=404,
                detail="Backup file not found locally or on any remote storage.",
            )
        backup.file_path = resolved_path

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
            if remote_temp_dir:
                shutil.rmtree(remote_temp_dir, ignore_errors=True)
            raise HTTPException(status_code=400, detail="Cannot download directories")

        file_obj = tar.extractfile(member)
        if file_obj is None:
            tar.close()
            if temp_path and temp_path.exists():
                temp_path.unlink(missing_ok=True)
            if remote_temp_dir:
                shutil.rmtree(remote_temp_dir, ignore_errors=True)
            raise HTTPException(status_code=500, detail="Failed to extract file")

        filename = _sanitize_filename(member.name)
        cleanup_temp = temp_path
        cleanup_remote_dir = remote_temp_dir

        def iter_file():
            try:
                while chunk := file_obj.read(65536):
                    yield chunk
            finally:
                file_obj.close()
                tar.close()
                if cleanup_temp and cleanup_temp.exists():
                    cleanup_temp.unlink(missing_ok=True)
                if cleanup_remote_dir:
                    shutil.rmtree(cleanup_remote_dir, ignore_errors=True)

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
        if remote_temp_dir:
            shutil.rmtree(remote_temp_dir, ignore_errors=True)
        raise HTTPException(status_code=404, detail="File not found in archive")
    except tarfile.TarError as e:
        if tar:
            tar.close()
        if temp_path and temp_path.exists():
            temp_path.unlink(missing_ok=True)
        if remote_temp_dir:
            shutil.rmtree(remote_temp_dir, ignore_errors=True)
        logger.error("Failed to extract file from decrypted archive: %s", e)
        raise HTTPException(status_code=500, detail="Failed to read backup archive")


@router.get("/{backup_id}/files", response_model=List[BackupFileInfo])
async def list_backup_files(backup_id: int):
    """List all files in a backup archive.

    Returns a flat list of all files and directories in the backup
    with their sizes and metadata.  Falls back to remote storage when
    the archive is not available locally.
    """
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        if backup.status != BackupStatus.COMPLETED:
            raise HTTPException(status_code=400, detail="Backup is not completed")

    remote_temp_dir: Path | None = None
    try:
        resolved_path, remote_temp_dir = await _ensure_backup_locally(backup)
        if not resolved_path:
            raise HTTPException(
                status_code=404,
                detail="Backup file not found locally or on any remote storage.",
            )
        backup.file_path = resolved_path

        # Handle encrypted backups
        archive_path = backup.file_path
        if backup.encrypted and archive_path.endswith(".enc"):
            raise HTTPException(
                status_code=400,
                detail="Cannot browse encrypted backups without private key. Use POST endpoint with your private key.",
            )

        mode = _get_tar_mode(archive_path)
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
        logger.error("Failed to read archive: %s", e)
        raise HTTPException(status_code=500, detail="Failed to read backup archive")
    finally:
        if remote_temp_dir:
            shutil.rmtree(remote_temp_dir, ignore_errors=True)


@router.get("/{backup_id}/files/{file_path:path}")
async def download_backup_file(backup_id: int, file_path: str):
    """Download a specific file from a backup archive.

    The file is extracted and streamed to the client.
    Falls back to remote storage when the archive is not available locally.
    """
    async with async_session() as session:
        result = await session.execute(select(Backup).where(Backup.id == backup_id))
        backup = result.scalar_one_or_none()

        if not backup:
            raise HTTPException(status_code=404, detail="Backup not found")

        if backup.status != BackupStatus.COMPLETED:
            raise HTTPException(status_code=400, detail="Backup is not completed")

    remote_temp_dir: Path | None = None
    tar = None
    try:
        resolved_path, remote_temp_dir = await _ensure_backup_locally(backup)
        if not resolved_path:
            raise HTTPException(
                status_code=404,
                detail="Backup file not found locally or on any remote storage.",
            )
        backup.file_path = resolved_path

        # Handle encrypted backups
        archive_path = backup.file_path
        if backup.encrypted and archive_path.endswith(".enc"):
            raise HTTPException(
                status_code=400,
                detail="Cannot download from encrypted backups without private key. Use POST endpoint with your private key.",
            )

        mode = _get_tar_mode(archive_path)

        tar = tarfile.open(archive_path, mode)
        normalized_path = _validate_archive_path(file_path)

        member = tar.getmember(normalized_path)

        if member.isdir():
            tar.close()
            if remote_temp_dir:
                shutil.rmtree(remote_temp_dir, ignore_errors=True)
            raise HTTPException(status_code=400, detail="Cannot download directories")

        file_obj = tar.extractfile(member)
        if file_obj is None:
            tar.close()
            if remote_temp_dir:
                shutil.rmtree(remote_temp_dir, ignore_errors=True)
            raise HTTPException(status_code=500, detail="Failed to extract file")

        filename = _sanitize_filename(member.name)
        cleanup_remote_dir = remote_temp_dir

        def iter_file():
            try:
                while chunk := file_obj.read(65536):
                    yield chunk
            finally:
                file_obj.close()
                tar.close()
                if cleanup_remote_dir:
                    shutil.rmtree(cleanup_remote_dir, ignore_errors=True)

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
        if remote_temp_dir:
            shutil.rmtree(remote_temp_dir, ignore_errors=True)
        raise HTTPException(status_code=404, detail="File not found in archive")
    except tarfile.TarError as e:
        if tar:
            tar.close()
        if remote_temp_dir:
            shutil.rmtree(remote_temp_dir, ignore_errors=True)
        logger.error("Failed to extract file from archive: %s", e)
        raise HTTPException(status_code=500, detail="Failed to read backup archive")
