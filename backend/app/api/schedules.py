"""
Schedules API endpoints.
Redesigned to support Schedule as a standalone entity that can be reused across targets.
"""

from datetime import datetime, timezone
from typing import List, Optional

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.orm import selectinload

from app.backup_engine import backup_engine
from app.database import BackupTarget, Schedule, async_session
from app.scheduler import BackupScheduler

router = APIRouter()


# ============================================
# Pydantic Models
# ============================================


class ScheduleCreate(BaseModel):
    """Create a new schedule."""

    name: str
    cron_expression: str
    description: Optional[str] = None
    enabled: bool = True


class ScheduleUpdate(BaseModel):
    """Update an existing schedule."""

    name: Optional[str] = None
    cron_expression: Optional[str] = None
    description: Optional[str] = None
    enabled: Optional[bool] = None


class ScheduleResponse(BaseModel):
    """Schedule response model."""

    id: int
    name: str
    cron_expression: str
    description: Optional[str] = None
    enabled: bool
    target_count: int = 0
    next_run: Optional[str] = None
    created_at: str
    updated_at: str


class ScheduleWithTargets(ScheduleResponse):
    """Schedule with assigned targets."""

    targets: List[dict] = []


class LegacyScheduleResponse(BaseModel):
    """Legacy schedule response for backwards compatibility."""

    id: int
    target_id: int
    target_name: str
    cron_expression: str
    next_run: Optional[str] = None
    last_run: Optional[str] = None
    enabled: bool


class EstimateRequest(BaseModel):
    """Backup window estimation request."""

    target_id: int
    cron_expression: str


# ============================================
# Schedule CRUD Endpoints
# ============================================


@router.get("", response_model=List[ScheduleResponse])
async def list_schedules():
    """List all schedules with target counts."""
    async with async_session() as session:
        result = await session.execute(
            select(Schedule).options(selectinload(Schedule.targets))
        )
        schedules = result.scalars().all()

        scheduler = BackupScheduler()
        response = []

        for schedule in schedules:
            next_run = None
            if schedule.cron_expression and schedule.enabled:
                try:
                    next_run = scheduler.get_next_run(schedule.cron_expression)
                except Exception:
                    pass

            response.append(
                ScheduleResponse(
                    id=schedule.id,
                    name=schedule.name,
                    cron_expression=schedule.cron_expression,
                    description=schedule.description,
                    enabled=schedule.enabled,
                    target_count=len(schedule.targets),
                    next_run=next_run.isoformat() if next_run else None,
                    created_at=schedule.created_at.isoformat(),
                    updated_at=schedule.updated_at.isoformat(),
                )
            )

        return response


@router.post("", response_model=ScheduleResponse)
async def create_schedule(data: ScheduleCreate, request: Request):
    """Create a new schedule."""
    async with async_session() as session:
        # Check for duplicate name
        existing = await session.execute(
            select(Schedule).where(Schedule.name == data.name)
        )
        if existing.scalar_one_or_none():
            raise HTTPException(
                status_code=400,
                detail=f"Schedule with name '{data.name}' already exists",
            )

        # Validate cron expression
        scheduler: BackupScheduler = request.app.state.scheduler
        try:
            scheduler.get_next_run(data.cron_expression)
        except Exception as e:
            raise HTTPException(status_code=400, detail=f"Invalid cron expression: {e}")

        schedule = Schedule(
            name=data.name,
            cron_expression=data.cron_expression,
            description=data.description,
            enabled=data.enabled,
        )
        session.add(schedule)
        await session.commit()
        await session.refresh(schedule)

        return ScheduleResponse(
            id=schedule.id,
            name=schedule.name,
            cron_expression=schedule.cron_expression,
            description=schedule.description,
            enabled=schedule.enabled,
            target_count=0,
            next_run=None,
            created_at=schedule.created_at.isoformat(),
            updated_at=schedule.updated_at.isoformat(),
        )


@router.get("/{schedule_id}", response_model=ScheduleWithTargets)
async def get_schedule(schedule_id: int):
    """Get a specific schedule with its assigned targets."""
    async with async_session() as session:
        result = await session.execute(
            select(Schedule)
            .where(Schedule.id == schedule_id)
            .options(selectinload(Schedule.targets))
        )
        schedule = result.scalar_one_or_none()

        if not schedule:
            raise HTTPException(status_code=404, detail="Schedule not found")

        scheduler = BackupScheduler()
        next_run = None
        if schedule.cron_expression and schedule.enabled:
            try:
                next_run = scheduler.get_next_run(schedule.cron_expression)
            except Exception:
                pass

        targets = [
            {
                "id": t.id,
                "name": t.name,
                "target_type": t.target_type,
                "enabled": t.enabled,
            }
            for t in schedule.targets
        ]

        return ScheduleWithTargets(
            id=schedule.id,
            name=schedule.name,
            cron_expression=schedule.cron_expression,
            description=schedule.description,
            enabled=schedule.enabled,
            target_count=len(targets),
            next_run=next_run.isoformat() if next_run else None,
            created_at=schedule.created_at.isoformat(),
            updated_at=schedule.updated_at.isoformat(),
            targets=targets,
        )


@router.put("/{schedule_id}", response_model=ScheduleResponse)
async def update_schedule(schedule_id: int, data: ScheduleUpdate, request: Request):
    """Update a schedule."""
    async with async_session() as session:
        result = await session.execute(
            select(Schedule)
            .where(Schedule.id == schedule_id)
            .options(selectinload(Schedule.targets))
        )
        schedule = result.scalar_one_or_none()

        if not schedule:
            raise HTTPException(status_code=404, detail="Schedule not found")

        # Validate cron expression if provided
        if data.cron_expression is not None:
            scheduler: BackupScheduler = request.app.state.scheduler
            try:
                scheduler.get_next_run(data.cron_expression)
            except Exception as e:
                raise HTTPException(
                    status_code=400, detail=f"Invalid cron expression: {e}"
                )
            schedule.cron_expression = data.cron_expression

        if data.name is not None:
            # Check for duplicate name
            existing = await session.execute(
                select(Schedule).where(
                    Schedule.name == data.name, Schedule.id != schedule_id
                )
            )
            if existing.scalar_one_or_none():
                raise HTTPException(
                    status_code=400,
                    detail=f"Schedule with name '{data.name}' already exists",
                )
            schedule.name = data.name

        if data.description is not None:
            schedule.description = data.description

        if data.enabled is not None:
            schedule.enabled = data.enabled

        schedule.updated_at = datetime.now(timezone.utc)
        await session.commit()
        await session.refresh(schedule)

        # Update scheduler for all targets using this schedule
        scheduler: BackupScheduler = request.app.state.scheduler
        for target in schedule.targets:
            if schedule.enabled and target.enabled:
                await scheduler.add_schedule(target)
            else:
                await scheduler.remove_schedule(target.id)

        scheduler_obj = BackupScheduler()
        next_run = None
        if schedule.cron_expression and schedule.enabled:
            try:
                next_run = scheduler_obj.get_next_run(schedule.cron_expression)
            except Exception:
                pass

        return ScheduleResponse(
            id=schedule.id,
            name=schedule.name,
            cron_expression=schedule.cron_expression,
            description=schedule.description,
            enabled=schedule.enabled,
            target_count=len(schedule.targets),
            next_run=next_run.isoformat() if next_run else None,
            created_at=schedule.created_at.isoformat(),
            updated_at=schedule.updated_at.isoformat(),
        )


@router.delete("/{schedule_id}")
async def delete_schedule(schedule_id: int, request: Request):
    """Delete a schedule. Targets using this schedule will be unlinked."""
    async with async_session() as session:
        result = await session.execute(
            select(Schedule)
            .where(Schedule.id == schedule_id)
            .options(selectinload(Schedule.targets))
        )
        schedule = result.scalar_one_or_none()

        if not schedule:
            raise HTTPException(status_code=404, detail="Schedule not found")

        # Unlink targets and remove from scheduler
        scheduler: BackupScheduler = request.app.state.scheduler
        for target in schedule.targets:
            target.schedule_id = None
            await scheduler.remove_schedule(target.id)

        await session.delete(schedule)
        await session.commit()

        return {"status": "deleted", "unlinked_targets": len(schedule.targets)}


# ============================================
# Legacy Endpoints (for backwards compatibility)
# ============================================


@router.get("/legacy/by-target", response_model=List[LegacyScheduleResponse])
async def list_schedules_legacy():
    """Legacy endpoint: List schedules grouped by target."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget)
            .where(
                (BackupTarget.schedule_id.isnot(None))
                | (BackupTarget.schedule_cron.isnot(None))
            )
            .options(selectinload(BackupTarget.schedule))
        )
        targets = result.scalars().all()

        scheduler = BackupScheduler()
        schedules = []

        for target in targets:
            cron = None
            if target.schedule:
                cron = target.schedule.cron_expression
            elif target.schedule_cron:
                cron = target.schedule_cron

            if not cron:
                continue

            next_run = None
            try:
                next_run = scheduler.get_next_run(cron)
            except Exception:
                pass

            schedules.append(
                LegacyScheduleResponse(
                    id=target.id,
                    target_id=target.id,
                    target_name=target.name,
                    cron_expression=cron,
                    next_run=next_run.isoformat() if next_run else None,
                    last_run=None,
                    enabled=target.enabled,
                )
            )

        return schedules


# ============================================
# Scheduler Management Endpoints
# ============================================


@router.get("/jobs")
async def list_scheduled_jobs(request: Request):
    """List currently scheduled jobs in the scheduler."""
    scheduler: BackupScheduler = request.app.state.scheduler
    return scheduler.get_scheduled_jobs()


@router.post("/target/{target_id}/trigger")
async def trigger_backup(target_id: int, request: Request):
    """Trigger a backup immediately for a target."""
    scheduler: BackupScheduler = request.app.state.scheduler
    success = await scheduler.trigger_backup_now(target_id)

    if not success:
        raise HTTPException(status_code=404, detail="Target not found")

    return {"status": "triggered"}


@router.post("/{schedule_id}/trigger-all")
async def trigger_all_backups(schedule_id: int):
    """Trigger a backup for every enabled target of a schedule.

    Backups are executed **sequentially** so each target completes its
    full stop → backup → start cycle before the next one begins.
    If Komodo integration is enabled the Komodo stack target (if any)
    is moved to the end of the list so that Komodo remains available
    to manage other containers for as long as possible.
    """
    import asyncio

    from app.backup_engine import BackupType
    from app.config import settings as app_settings

    async with async_session() as session:
        result = await session.execute(
            select(Schedule)
            .where(Schedule.id == schedule_id)
            .options(selectinload(Schedule.targets))
        )
        schedule = result.scalar_one_or_none()

        if not schedule:
            raise HTTPException(status_code=404, detail="Schedule not found")

        enabled_targets = [t for t in schedule.targets if t.enabled]
        skipped = [t.id for t in schedule.targets if not t.enabled]

        # When Komodo is active, push targets whose stack/container name
        # contains "komodo" to the end so that Komodo keeps running
        # while every other backup goes through its stop/start cycle.
        if app_settings.KOMODO_ENABLED:

            def _is_komodo_target(t: BackupTarget) -> bool:
                name = (t.stack_name or t.container_name or "").lower()
                return "komodo" in name

            enabled_targets.sort(key=_is_komodo_target)

        # Create backup records
        backup_ids: list[int] = []
        triggered: list[int] = []
        for target in enabled_targets:
            backup = await backup_engine.create_backup(target, BackupType.FULL)
            backup_ids.append(backup.id)
            triggered.append(target.id)

    # Fire off sequential execution as a background task so the
    # HTTP response returns immediately.
    asyncio.create_task(backup_engine.run_batch_sequential(backup_ids))

    return {
        "status": "triggered",
        "schedule_id": schedule_id,
        "triggered_targets": triggered,
        "skipped_targets": skipped,
    }


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
            {
                "expression": "0 4 1 * *",
                "description": "First day of every month at 4:00 AM",
            },
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
