"""Read-only retained-catalog aggregates for the operational overview."""

from datetime import datetime, timedelta, timezone
from typing import Literal

from fastapi import APIRouter, Depends, Query
from pydantic import BaseModel
from sqlalchemy import and_, exists, func, or_, select, text
from sqlalchemy.sql.elements import ColumnElement

from app import database
from app.auth import get_current_user
from app.backup_engine import backup_engine
from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
    Schedule,
    StorageOperation,
    User,
)

router = APIRouter()


class PlanItem(BaseModel):
    plan_id: int
    name: str


class PlanGap(BaseModel):
    total: int
    enabled: int
    items: list[PlanItem]


class Plans(BaseModel):
    total: int
    enabled: int
    disabled: int
    manual: int
    automatic_enabled: int
    automatic_paused: int
    without_recorded_run: PlanGap
    without_recorded_completed_run: PlanGap


class BackupActivityItem(BaseModel):
    backup_id: int
    plan_id: int | None
    name: str | None
    catalog_status: BackupStatus | None
    stage: str | None
    live_worker: bool


class BackupActivity(BaseModel):
    recorded_pending: int
    recorded_running: int
    live: int
    total: int
    items: list[BackupActivityItem]


class StorageActivityItem(BaseModel):
    operation_id: str
    storage_id: str
    state: Literal["queued", "running"]
    stage: str


class StorageActivity(BaseModel):
    total: int
    items: list[StorageActivityItem]


class CopyFailureItem(BaseModel):
    backup_id: int
    storage_id: int
    storage_name: str | None
    status: Literal["failed"]


class CopyFailures(BaseModel):
    backups: int
    total: int
    items: list[CopyFailureItem]


class Window(BaseModel):
    after: datetime
    before: datetime
    time_field: Literal["created_at"] = "created_at"


class OverviewSummary(BaseModel):
    observed_at: datetime
    dataset: Literal["retained_catalog_and_current_process"]
    window: Window
    plans: Plans
    backup_activity: BackupActivity
    storage_activity: StorageActivity
    recorded_copy_failures: CopyFailures


@router.get("/summary", response_model=OverviewSummary)
async def summary(
    limit: int = Query(5, ge=1, le=20), user: User = Depends(get_current_user)
):
    observed_at = datetime.now(timezone.utc)
    live = backup_engine.observed_backups()
    async with database.async_session() as session:
        # SQLite deferred SELECTs alone do not start a repeatable read transaction.
        await session.execute(text("BEGIN"))

        async def count(model, *conditions):
            return (
                await session.scalar(
                    select(func.count()).select_from(model).where(*conditions)
                )
            ) or 0

        current: ColumnElement[bool] = BackupTarget.removed_at.is_(None)
        automatic = or_(
            BackupTarget.schedule_id.is_not(None),
            and_(
                BackupTarget.schedule_cron.is_not(None),
                BackupTarget.schedule_cron != "",
            ),
        )
        # Match scheduler precedence: a shared disabled schedule never falls back to legacy cron.
        effective = and_(
            BackupTarget.enabled.is_(True),
            or_(
                exists(
                    select(Schedule.id).where(
                        Schedule.id == BackupTarget.schedule_id,
                        Schedule.enabled.is_(True),
                    )
                ),
                and_(
                    BackupTarget.schedule_id.is_(None),
                    BackupTarget.schedule_cron.is_not(None),
                    BackupTarget.schedule_cron != "",
                ),
            ),
        )
        total = await count(BackupTarget, current)
        enabled = await count(BackupTarget, current, BackupTarget.enabled.is_(True))
        manual = await count(BackupTarget, current, ~automatic)
        automatic_enabled = await count(BackupTarget, current, automatic, effective)

        async def gap(completed=False):
            run = select(Backup.id).where(Backup.target_id == BackupTarget.id)
            if completed:
                run = run.where(Backup.status == BackupStatus.COMPLETED)
            conditions = (current, ~exists(run))
            rows = (
                await session.execute(
                    select(BackupTarget.id, BackupTarget.name)
                    .where(*conditions)
                    .order_by(BackupTarget.id)
                    .limit(limit)
                )
            ).all()
            return PlanGap(
                total=await count(BackupTarget, *conditions),
                enabled=await count(
                    BackupTarget, *conditions, BackupTarget.enabled.is_(True)
                ),
                items=[PlanItem(plan_id=id, name=name) for id, name in rows],
            )

        plans = Plans(
            total=total,
            enabled=enabled,
            disabled=total - enabled,
            manual=manual,
            automatic_enabled=automatic_enabled,
            automatic_paused=total - manual - automatic_enabled,
            without_recorded_run=await gap(),
            without_recorded_completed_run=await gap(True),
        )
        active = or_(
            Backup.status.in_([BackupStatus.PENDING, BackupStatus.RUNNING]),
            Backup.id.in_(live),
        )
        active_rows = (
            await session.execute(
                select(Backup.id, Backup.target_id, BackupTarget.name, Backup.status)
                .outerjoin(BackupTarget, BackupTarget.id == Backup.target_id)
                .where(active)
                .order_by(Backup.created_at.desc(), Backup.id.desc())
                .limit(limit)
            )
        ).all()
        activity = BackupActivity(
            recorded_pending=await count(Backup, Backup.status == BackupStatus.PENDING),
            recorded_running=await count(Backup, Backup.status == BackupStatus.RUNNING),
            live=len(live),
            total=await count(Backup, active),
            items=[
                BackupActivityItem(
                    backup_id=id,
                    plan_id=plan,
                    name=name,
                    catalog_status=status,
                    stage=live.get(id),
                    live_worker=id in live,
                )
                for id, plan, name, status in active_rows
            ],
        )
        owned_active = (
            StorageOperation.user_id == user.id,
            StorageOperation.state.in_(["queued", "running"]),
        )
        jobs = (
            await session.execute(
                select(
                    StorageOperation.id,
                    StorageOperation.storage_id,
                    StorageOperation.state,
                    StorageOperation.stage,
                )
                .where(*owned_active)
                .order_by(
                    StorageOperation.created_at.desc(), StorageOperation.id.desc()
                )
                .limit(limit)
            )
        ).all()
        storage = StorageActivity(
            total=await count(StorageOperation, *owned_active),
            items=[
                StorageActivityItem(
                    operation_id=id, storage_id=storage_id, state=state, stage=stage
                )
                for id, storage_id, state, stage in jobs
            ],
        )
        failed = BackupStorageSync.sync_status == "failed"
        copies = (
            await session.execute(
                select(
                    BackupStorageSync.backup_id,
                    BackupStorageSync.storage_id,
                    RemoteStorage.name,
                )
                .outerjoin(
                    RemoteStorage, RemoteStorage.id == BackupStorageSync.storage_id
                )
                .where(failed)
                .order_by(BackupStorageSync.id.desc())
                .limit(limit)
            )
        ).all()
        failures = CopyFailures(
            backups=(
                await session.scalar(
                    select(
                        func.count(func.distinct(BackupStorageSync.backup_id))
                    ).where(failed)
                )
            )
            or 0,
            total=await count(BackupStorageSync, failed),
            items=[
                CopyFailureItem(
                    backup_id=id,
                    storage_id=storage_id,
                    storage_name=name,
                    status="failed",
                )
                for id, storage_id, name in copies
            ],
        )
    return OverviewSummary(
        observed_at=observed_at,
        dataset="retained_catalog_and_current_process",
        window=Window(after=observed_at - timedelta(hours=24), before=observed_at),
        plans=plans,
        backup_activity=activity,
        storage_activity=storage,
        recorded_copy_failures=failures,
    )
