"""
API routes for the backup manager.
"""

from fastapi import APIRouter

from app.api.docker import router as docker_router
from app.api.backups import router as backups_router
from app.api.targets import router as targets_router
from app.api.schedules import router as schedules_router
from app.api.retention import router as retention_router
from app.api.komodo import router as komodo_router
from app.api.storage import router as storage_router

router = APIRouter()

router.include_router(docker_router, prefix="/docker", tags=["Docker"])
router.include_router(targets_router, prefix="/targets", tags=["Backup Targets"])
router.include_router(backups_router, prefix="/backups", tags=["Backups"])
router.include_router(schedules_router, prefix="/schedules", tags=["Schedules"])
router.include_router(retention_router, prefix="/retention", tags=["Retention"])
router.include_router(komodo_router, prefix="/komodo", tags=["Komodo"])
router.include_router(storage_router, prefix="/storage", tags=["Remote Storage"])
