"""
Application settings API endpoints.
"""

import logging
import os
import shutil
import time
from pathlib import Path
from typing import Literal, Optional

from fastapi import APIRouter
from pydantic import BaseModel
from sqlalchemy import func, select

from app.config import Settings
from app.database import AppSettings, Backup, BackupTarget, async_session

logger = logging.getLogger(__name__)

router = APIRouter()

_start_time = time.time()


async def get_setting(key: str) -> Optional[str]:
    """Get a setting value from the database."""
    async with async_session() as session:
        result = await session.execute(
            select(AppSettings).where(AppSettings.key == key)
        )
        setting = result.scalar_one_or_none()
        return setting.value if setting else None


async def set_setting(key: str, value: Optional[str]) -> None:
    """Set a setting value in the database."""
    async with async_session() as session:
        result = await session.execute(
            select(AppSettings).where(AppSettings.key == key)
        )
        setting = result.scalar_one_or_none()

        if setting:
            setting.value = value
        else:
            setting = AppSettings(key=key, value=value)
            session.add(setting)

        await session.commit()


class StorageSettingsResponse(BaseModel):
    """Storage settings response."""

    check_quota_before_upload: bool


@router.get("/storage", response_model=StorageSettingsResponse)
async def get_storage_settings():
    """Get storage and upload settings."""
    check_quota = await get_setting("storage_check_quota_before_upload")
    return StorageSettingsResponse(
        check_quota_before_upload=check_quota == "true"  # Default: false (opt-in)
    )


@router.put("/storage", response_model=StorageSettingsResponse)
async def update_storage_settings(request: StorageSettingsResponse):
    """Update storage and upload settings."""
    await set_setting(
        "storage_check_quota_before_upload",
        "true" if request.check_quota_before_upload else "false",
    )
    logger.info(
        "Storage settings updated: check_quota=%s",
        request.check_quota_before_upload,
    )
    return request


class SystemInfoResponse(BaseModel):
    """System information response."""

    backup_dir: str
    database_path: str
    timezone: str
    disk_total: int
    disk_status: Literal["available", "unavailable"]
    disk_used: int
    disk_free: int
    db_size: int
    backup_count: int
    target_count: int
    uptime_seconds: int
    app_version: str


@router.get("/system-info", response_model=SystemInfoResponse)
async def get_system_info():
    """Get system information and statistics."""
    settings = Settings()

    # Disk usage for backup directory
    backup_dir = settings.BACKUP_BASE_PATH
    try:
        disk = shutil.disk_usage(backup_dir)
        disk_total = disk.total
        disk_used = disk.used
        disk_free = disk.free
        disk_status = "available"
    except OSError:
        disk_total = disk_used = disk_free = 0
        disk_status = "unavailable"

    # Database file size
    db_path = settings.DATABASE_URL.replace("sqlite+aiosqlite:///", "")
    try:
        db_size = Path(db_path).stat().st_size
    except OSError:
        db_size = 0

    # Counts from database
    async with async_session() as session:
        backup_count_result = await session.execute(select(func.count(Backup.id)))
        backup_count = backup_count_result.scalar() or 0

        target_count_result = await session.execute(select(func.count(BackupTarget.id)))
        target_count = target_count_result.scalar() or 0

    # Uptime
    uptime = int(time.time() - _start_time)

    # Timezone
    tz = os.environ.get("TZ", settings.TZ)

    return SystemInfoResponse(
        backup_dir="[configured]",
        database_path="[configured]",
        timezone=tz,
        disk_total=disk_total,
        disk_status=disk_status,
        disk_used=disk_used,
        disk_free=disk_free,
        db_size=db_size,
        backup_count=backup_count,
        target_count=target_count,
        uptime_seconds=uptime,
        app_version="1.0.0",
    )
