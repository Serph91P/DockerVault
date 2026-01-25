"""
Schedules API endpoints.
"""

from typing import List, Optional
from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel
from sqlalchemy import select

from app.database import BackupTarget, async_session
from app.scheduler import BackupScheduler
from app.backup_engine import backup_engine

router = APIRouter()


class ScheduleResponse(BaseModel):
    """Schedule response model."""
    id: int
    target_id: int
    target_name: str
    cron_expression: str
    next_run: Optional[str] = None
    last_run: Optional[str] = None
    enabled: bool


class UpdateScheduleRequest(BaseModel):
    """Update schedule request."""
    cron_expression: Optional[str] = None
    enabled: Optional[bool] = None


class EstimateRequest(BaseModel):
    """Backup window estimation request."""
    target_id: int
    cron_expression: str


@router.get("", response_model=List[ScheduleResponse])
async def list_schedules():
    """List all backup schedules."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget).where(BackupTarget.schedule_cron.isnot(None))
        )
        targets = result.scalars().all()

        schedules = []
        for target in targets:
            # Calculate next run
            scheduler = BackupScheduler()
            next_run = None
            if target.schedule_cron:
                try:
                    next_run = scheduler.get_next_run(target.schedule_cron)
                except Exception:
                    pass

            schedules.append(ScheduleResponse(
                id=target.id,
                target_id=target.id,
                target_name=target.name,
                cron_expression=target.schedule_cron,
                next_run=next_run.isoformat() if next_run else None,
                last_run=None,  # Would need to track this
                enabled=target.enabled,
            ))

        return schedules


@router.get("/jobs")
async def list_scheduled_jobs(request: Request):
    """List currently scheduled jobs in the scheduler."""
    scheduler: BackupScheduler = request.app.state.scheduler
    return scheduler.get_scheduled_jobs()


@router.post("/{target_id}/trigger")
async def trigger_backup(target_id: int, request: Request):
    """Trigger a backup immediately for a target."""
    scheduler: BackupScheduler = request.app.state.scheduler
    success = await scheduler.trigger_backup_now(target_id)

    if not success:
        raise HTTPException(status_code=404, detail="Target not found")

    return {"status": "triggered"}


@router.put("/{target_id}")
async def update_schedule(target_id: int, update: UpdateScheduleRequest, request: Request):
    """Update a backup schedule."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == target_id)
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

        if update.cron_expression is not None:
            target.schedule_cron = update.cron_expression
        if update.enabled is not None:
            target.enabled = update.enabled

        await session.commit()
        await session.refresh(target)

    # Update scheduler
    scheduler: BackupScheduler = request.app.state.scheduler

    if target.enabled and target.schedule_cron:
        await scheduler.add_schedule(target)
    else:
        await scheduler.remove_schedule(target_id)

    return {"status": "updated"}


@router.post("/estimate")
async def estimate_backup_window(request: EstimateRequest, app_request: Request):
    """Estimate backup window and check for conflicts."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == request.target_id)
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

    # Get estimated duration
    estimated_duration = await backup_engine.estimate_backup_duration(target)

    # Get backup window
    scheduler: BackupScheduler = app_request.app.state.scheduler
    window = scheduler.estimate_backup_window(
        request.cron_expression,
        estimated_duration,
    )

    return window


@router.get("/cron-help")
async def cron_help():
    """Get help for cron expressions."""
    return {
        "format": "minute hour day month weekday",
        "examples": [
            {"expression": "0 2 * * *", "description": "Every day at 2:00 AM"},
            {"expression": "0 3 * * 0", "description": "Every Sunday at 3:00 AM"},
            {"expression": "0 4 1 * *", "description": "First day of every month at 4:00 AM"},
            {"expression": "0 */6 * * *", "description": "Every 6 hours"},
            {"expression": "30 1 * * 1-5", "description": "Weekdays at 1:30 AM"},
        ],
        "special": {
            "*": "Any value",
            "*/n": "Every n units",
            "n-m": "Range from n to m",
            "n,m": "n and m",
        },
    }
