"""
Application settings API endpoints.
"""

import logging
import asyncio
import os
import shutil
import time
from pathlib import Path
from typing import Optional

import httpx
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel
from sqlalchemy import func, select

from app.config import Settings
from app.database import AppSettings, Backup, BackupTarget, async_session
from app.komodo_config import (
    load_komodo_config,
    save_komodo_config,
    validate_komodo_url,
)

logger = logging.getLogger(__name__)

router = APIRouter()

_start_time = time.time()


def _validate_komodo_url(url: str) -> str:
    try:
        return validate_komodo_url(url)
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


class KomodoSettingsRequest(BaseModel):
    """Komodo settings update request."""

    enabled: bool
    api_url: Optional[str] = None
    api_key: Optional[str] = None
    api_secret: Optional[str] = None


class KomodoSettingsResponse(BaseModel):
    """Komodo settings response."""

    enabled: bool
    api_url: Optional[str] = None
    has_api_key: bool = False
    has_api_secret: bool = False
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
    config = await load_komodo_config()
    enabled, api_url, api_key, api_secret = (
        config.enabled,
        config.api_url,
        config.api_key,
        config.api_secret,
    )

    connected = False
    if enabled and api_url and api_key and api_secret:
        try:
            await asyncio.to_thread(_validate_komodo_url, api_url)
            async with httpx.AsyncClient(timeout=5.0) as client:
                response = await client.post(
                    f"{api_url.rstrip('/')}/read",
                    json={"type": "GetVersion"},
                    headers={
                        "x-api-key": api_key,
                        "x-api-secret": api_secret,
                    },
                )
                connected = response.status_code == 200
        except Exception:
            connected = False

    return KomodoSettingsResponse(
        enabled=enabled,
        api_url=api_url,
        has_api_key=bool(api_key),
        has_api_secret=bool(api_secret),
        connected=connected,
    )


@router.put("/komodo", response_model=KomodoSettingsResponse)
async def update_komodo_settings(request: KomodoSettingsRequest):
    """Update Komodo integration settings."""
    try:
        await save_komodo_config(request.model_dump(exclude_unset=True))
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc

    from app.komodo import komodo_client

    await komodo_client.refresh()

    logger.info(f"Komodo settings updated: enabled={request.enabled}")

    return await get_komodo_settings()


@router.post("/komodo/test", response_model=KomodoTestResponse)
async def test_komodo_connection():
    """Test Komodo connection with current settings."""
    config = await load_komodo_config()
    enabled, api_url, api_key, api_secret = (
        config.enabled,
        config.api_url,
        config.api_key,
        config.api_secret,
    )

    if not enabled:
        return KomodoTestResponse(
            success=False, message="Komodo integration is disabled"
        )

    if not api_url:
        return KomodoTestResponse(success=False, message="API URL is not configured")

    if not api_key:
        return KomodoTestResponse(success=False, message="API key is not configured")

    if not api_secret:
        return KomodoTestResponse(success=False, message="API secret is not configured")

    try:
        await asyncio.to_thread(_validate_komodo_url, api_url)
        async with httpx.AsyncClient(timeout=10.0) as client:
            response = await client.post(
                f"{api_url.rstrip('/')}/read",
                json={"type": "GetVersion"},
                headers={
                    "x-api-key": api_key,
                    "x-api-secret": api_secret,
                },
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
                    success=False,
                    message="Authentication failed - check API key and secret",
                )
            elif response.status_code == 403:
                return KomodoTestResponse(
                    success=False,
                    message="Access forbidden - check API key/secret permissions",
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
        backup_dir="[configured]",
        database_path="[configured]",
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
