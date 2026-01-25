"""
Komodo integration API endpoints.
"""

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel
from typing import Optional

from app.komodo import komodo_client
from app.config import settings

router = APIRouter()


class KomodoConfigUpdate(BaseModel):
    """Komodo configuration update request."""
    api_url: Optional[str] = None
    api_key: Optional[str] = None
    enabled: Optional[bool] = None


class ContainerActionRequest(BaseModel):
    """Container action request."""
    container_name: str
    action: str  # start, stop
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
        raise HTTPException(
            status_code=400, detail="Komodo integration is not enabled"
        )

    is_available = await komodo_client.is_available()

    if not is_available:
        raise HTTPException(
            status_code=503, detail="Cannot connect to Komodo"
        )

    return {"status": "connected"}


@router.post("/container-action")
async def request_container_action(request: ContainerActionRequest):
    """Request Komodo to perform a container action."""
    if not settings.KOMODO_ENABLED:
        raise HTTPException(
            status_code=400, detail="Komodo integration is not enabled"
        )

    if request.action == "stop":
        success = await komodo_client.request_container_stop(
            request.container_name,
            request.reason or "backup",
        )
    elif request.action == "start":
        success = await komodo_client.request_container_start(
            request.container_name,
            request.reason or "backup_complete",
        )
    else:
        raise HTTPException(
            status_code=400, detail=f"Unknown action: {request.action}"
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
        raise HTTPException(
            status_code=400, detail="Komodo integration is not enabled"
        )

    status = await komodo_client.get_container_status(container_name)

    if status is None:
        raise HTTPException(
            status_code=404, detail="Container not found or Komodo unavailable"
        )

    return status
