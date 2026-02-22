"""
Application settings API endpoints.
"""

import logging
import os
import shutil
import time
from pathlib import Path
from typing import Optional

import httpx
from fastapi import APIRouter
from pydantic import BaseModel
from sqlalchemy import func, select

from app.config import Settings
from app.database import AppSettings, Backup, BackupTarget, async_session

logger = logging.getLogger(__name__)

router = APIRouter()

_start_time = time.time()


class KomodoSettingsRequest(BaseModel):
    """Komodo settings update request."""

    enabled: bool
    api_url: Optional[str] = None
    api_key: Optional[str] = None


class KomodoSettingsResponse(BaseModel):
    """Komodo settings response."""

    enabled: bool
    api_url: Optional[str] = None
    has_api_key: bool = False
    connected: bool = False


class KomodoTestResponse(BaseModel):
    """Komodo connection test response."""

    success: bool
    message: str
    version: Optional[str] = None


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


@router.get("/komodo", response_model=KomodoSettingsResponse)
async def get_komodo_settings():
    """Get Komodo integration settings."""
    enabled = await get_setting("komodo_enabled") == "true"
    api_url = await get_setting("komodo_api_url")
    api_key = await get_setting("komodo_api_key")

    connected = False
    if enabled and api_url and api_key:
        try:
            async with httpx.AsyncClient(timeout=5.0) as client:
                response = await client.get(
                    f"{api_url.rstrip('/')}/api/version",
                    headers={"Authorization": f"Bearer {api_key}"},
                )
                connected = response.status_code == 200
        except Exception:
            connected = False

    return KomodoSettingsResponse(
        enabled=enabled,
        api_url=api_url,
        has_api_key=bool(api_key),
        connected=connected,
    )


@router.put("/komodo", response_model=KomodoSettingsResponse)
async def update_komodo_settings(request: KomodoSettingsRequest):
    """Update Komodo integration settings."""
    await set_setting("komodo_enabled", "true" if request.enabled else "false")

    if request.api_url is not None:
        await set_setting("komodo_api_url", request.api_url)

    if request.api_key is not None:
        await set_setting("komodo_api_key", request.api_key)

    logger.info(f"Komodo settings updated: enabled={request.enabled}")

    return await get_komodo_settings()


@router.post("/komodo/test", response_model=KomodoTestResponse)
async def test_komodo_connection():
    """Test Komodo connection with current settings."""
    enabled = await get_setting("komodo_enabled") == "true"
    api_url = await get_setting("komodo_api_url")
    api_key = await get_setting("komodo_api_key")

    if not enabled:
        return KomodoTestResponse(
            success=False, message="Komodo integration is disabled"
        )

    if not api_url:
        return KomodoTestResponse(success=False, message="API URL is not configured")

    if not api_key:
        return KomodoTestResponse(success=False, message="API key is not configured")

    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            response = await client.get(
                f"{api_url.rstrip('/')}/api/version",
                headers={"Authorization": f"Bearer {api_key}"},
            )

            if response.status_code == 200:
                data = response.json()
                version = data.get("version", "unknown")
                return KomodoTestResponse(
                    success=True,
                    message="Connection successful",
                    version=version,
                )
            elif response.status_code == 401:
                return KomodoTestResponse(
                    success=False, message="Authentication failed - check API key"
                )
            elif response.status_code == 403:
                return KomodoTestResponse(
                    success=False,
                    message="Access forbidden - check API key permissions",
                )
            else:
                return KomodoTestResponse(
                    success=False,
                    message=f"Unexpected response: {response.status_code}",
                )
    except httpx.ConnectError:
        return KomodoTestResponse(
            success=False, message=f"Cannot connect to {api_url} - check URL"
        )
    except httpx.TimeoutException:
        return KomodoTestResponse(
            success=False, message="Connection timed out - check network"
        )
    except Exception as e:
        logger.error(f"Komodo connection test failed: {e}")
        return KomodoTestResponse(success=False, message=str(e))


class SystemInfoResponse(BaseModel):
    """System information response."""

    backup_dir: str
    database_path: str
    timezone: str
    disk_total: int
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
    except OSError:
        disk_total = disk_used = disk_free = 0

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
        backup_dir=backup_dir,
        database_path=db_path,
        timezone=tz,
        disk_total=disk_total,
        disk_used=disk_used,
        disk_free=disk_free,
        db_size=db_size,
        backup_count=backup_count,
        target_count=target_count,
        uptime_seconds=uptime,
        app_version="1.0.0",
    )
