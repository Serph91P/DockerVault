"""
Komodo integration API endpoints.
"""

from typing import Literal, Optional

import logging

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from app.config import settings
from app.komodo import komodo_client

router = APIRouter()
logger = logging.getLogger(__name__)


class KomodoConfigUpdate(BaseModel):
    """Komodo configuration update request."""

    api_url: Optional[str] = None
    api_key: Optional[str] = None
    enabled: Optional[bool] = None


class ContainerActionRequest(BaseModel):
    """Container action request."""

    container_name: str
    action: Literal["start", "stop"]
    reason: Optional[str] = None


@router.get("/status")
async def get_komodo_status():
    """Get Komodo integration status."""
    is_available = await komodo_client.is_available()

    return {
        "enabled": settings.KOMODO_ENABLED,
        "api_url": settings.KOMODO_API_URL or None,
        "connected": is_available,
    }


@router.post("/test")
async def test_komodo_connection():
    """Test connection to Komodo."""
    if not settings.KOMODO_ENABLED:
        raise HTTPException(status_code=400, detail="Komodo integration is not enabled")

    is_available = await komodo_client.is_available()

    if not is_available:
        raise HTTPException(status_code=503, detail="Cannot connect to Komodo")

    return {"status": "connected"}


@router.post("/container-action")
async def request_container_action(request: ContainerActionRequest):
    """Request Komodo to perform a container action."""
    if not settings.KOMODO_ENABLED:
        raise HTTPException(status_code=400, detail="Komodo integration is not enabled")

    logger.info(
        "Komodo container action: %s on %s (reason: %s)",
        request.action,
        request.container_name,
        request.reason,
    )

    if request.action == "stop":
        success = await komodo_client.request_container_stop(
            request.container_name,
            request.reason or "backup",
        )
    else:
        success = await komodo_client.request_container_start(
            request.container_name,
            request.reason or "backup_complete",
        )

    if not success:
        raise HTTPException(
            status_code=500, detail=f"Failed to {request.action} container"
        )

    return {"status": "success", "action": request.action}


@router.get("/container/{container_name}")
async def get_container_status(container_name: str):
    """Get container status from Komodo."""
    if not settings.KOMODO_ENABLED:
        raise HTTPException(status_code=400, detail="Komodo integration is not enabled")

    status = await komodo_client.get_container_status(container_name)

    if status is None:
        raise HTTPException(
            status_code=404, detail="Container not found or Komodo unavailable"
        )

    return status
