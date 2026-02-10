"""
Backup targets API endpoints.
"""

import logging
import os
from typing import List, Optional

from croniter import croniter
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, field_validator
from sqlalchemy import select
from sqlalchemy.orm import selectinload

from app.database import BackupTarget, Schedule, async_session

logger = logging.getLogger(__name__)

router = APIRouter()


def validate_cron_expression(cron_expr: Optional[str]) -> Optional[str]:
    """Validate a cron expression."""
    if cron_expr is None:
        return None

    # Trim whitespace
    cron_expr = cron_expr.strip()
    if not cron_expr:
        return None

    # Validate using croniter
    if not croniter.is_valid(cron_expr):
        raise ValueError(f"Invalid cron expression: {cron_expr}")

    return cron_expr


class TargetCreate(BaseModel):
    """Create backup target request."""

    name: str
    target_type: str  # container, volume, path, stack
    container_name: Optional[str] = None
    volume_name: Optional[str] = None
    host_path: Optional[str] = None
    stack_name: Optional[str] = None
    schedule_id: Optional[int] = None  # NEW: Reference to Schedule entity
    schedule_cron: Optional[str] = None  # DEPRECATED: Keep for backwards compatibility
    enabled: bool = True
    retention_policy_id: Optional[int] = None
    dependencies: List[str] = []
    # Volume selection for container/stack backups
    selected_volumes: List[str] = []  # Empty = all volumes
    # Path filtering
    include_paths: List[str] = []  # Include only these paths (empty = all)
    exclude_paths: List[str] = []  # Exclude these paths/patterns
    pre_backup_command: Optional[str] = None
    post_backup_command: Optional[str] = None
    stop_container: bool = True
    compression_enabled: bool = True

    @field_validator("schedule_cron")
    @classmethod
    def validate_schedule_cron(cls, v: Optional[str]) -> Optional[str]:
        return validate_cron_expression(v)


class TargetUpdate(BaseModel):
    """Update backup target request."""

    name: Optional[str] = None
    schedule_id: Optional[int] = None  # NEW: Reference to Schedule entity
    schedule_cron: Optional[str] = None  # DEPRECATED: Keep for backwards compatibility
    enabled: Optional[bool] = None
    retention_policy_id: Optional[int] = None
    dependencies: Optional[List[str]] = None
    # Volume selection for container/stack backups
    selected_volumes: Optional[List[str]] = None
    # Path filtering
    include_paths: Optional[List[str]] = None
    exclude_paths: Optional[List[str]] = None
    pre_backup_command: Optional[str] = None
    post_backup_command: Optional[str] = None
    stop_container: Optional[bool] = None
    compression_enabled: Optional[bool] = None

    @field_validator("schedule_cron")
    @classmethod
    def validate_schedule_cron(cls, v: Optional[str]) -> Optional[str]:
        return validate_cron_expression(v)


class ScheduleInfo(BaseModel):
    """Embedded schedule information."""

    id: int
    name: str
    cron_expression: str


class RetentionPolicyInfo(BaseModel):
    """Embedded retention policy information."""

    id: int
    name: str
    keep_last: int
    keep_daily: int
    keep_weekly: int
    keep_monthly: int


class TargetResponse(BaseModel):
    """Backup target response."""

    id: int
    name: str
    target_type: str
    container_id: Optional[str] = None
    container_name: Optional[str] = None
    volume_name: Optional[str] = None
    host_path: Optional[str] = None
    stack_name: Optional[str] = None
    schedule_id: Optional[int] = None  # NEW: Reference to Schedule entity
    schedule: Optional[ScheduleInfo] = None  # Embedded schedule info
    schedule_cron: Optional[str] = None  # DEPRECATED: backwards compat
    enabled: bool
    retention_policy_id: Optional[int] = None
    retention_policy: Optional[RetentionPolicyInfo] = None  # Embedded info
    dependencies: List[str]
    # Volume selection for container/stack backups
    selected_volumes: List[str]
    # Path filtering
    include_paths: List[str]
    exclude_paths: List[str]
    pre_backup_command: Optional[str] = None
    post_backup_command: Optional[str] = None
    stop_container: bool
    compression_enabled: bool
    created_at: str
    updated_at: str

    class Config:
        from_attributes = True


def _build_target_response(t: BackupTarget) -> TargetResponse:
    """Helper function to build TargetResponse from BackupTarget."""
    schedule_info = None
    if t.schedule:
        schedule_info = ScheduleInfo(
            id=t.schedule.id,
            name=t.schedule.name,
            cron_expression=t.schedule.cron_expression,
        )

    retention_policy_info = None
    if t.retention_policy:
        retention_policy_info = RetentionPolicyInfo(
            id=t.retention_policy.id,
            name=t.retention_policy.name,
            keep_last=t.retention_policy.keep_last,
            keep_daily=t.retention_policy.keep_daily,
            keep_weekly=t.retention_policy.keep_weekly,
            keep_monthly=t.retention_policy.keep_monthly,
        )

    return TargetResponse(
        id=t.id,
        name=t.name,
        target_type=t.target_type,
        container_id=t.container_id,
        container_name=t.container_name,
        volume_name=t.volume_name,
        host_path=t.host_path,
        stack_name=t.stack_name,
        schedule_id=t.schedule_id,
        schedule=schedule_info,
        schedule_cron=t.schedule_cron,
        enabled=t.enabled,
        retention_policy_id=t.retention_policy_id,
        retention_policy=retention_policy_info,
        dependencies=t.dependencies or [],
        selected_volumes=t.selected_volumes or [],
        include_paths=t.include_paths or [],
        exclude_paths=t.exclude_paths or [],
        pre_backup_command=t.pre_backup_command,
        post_backup_command=t.post_backup_command,
        stop_container=t.stop_container,
        compression_enabled=t.compression_enabled,
        created_at=t.created_at.isoformat(),
        updated_at=t.updated_at.isoformat(),
    )


@router.get("", response_model=List[TargetResponse])
async def list_targets():
    """List all backup targets."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget).options(
                selectinload(BackupTarget.schedule),
                selectinload(BackupTarget.retention_policy),
            )
        )
        targets = result.scalars().all()

        return [_build_target_response(t) for t in targets]


@router.post("", response_model=TargetResponse)
async def create_target(target: TargetCreate):
    """Create a new backup target."""
    async with async_session() as session:
        # Validate schedule_id if provided
        if target.schedule_id is not None:
            schedule_result = await session.execute(
                select(Schedule).where(Schedule.id == target.schedule_id)
            )
            if not schedule_result.scalar_one_or_none():
                raise HTTPException(
                    status_code=400,
                    detail=f"Schedule with id {target.schedule_id} not found",
                )

        # Validate target type and required fields
        if target.target_type == "container" and not target.container_name:
            raise HTTPException(
                status_code=400, detail="container_name required for container type"
            )
        if target.target_type == "volume" and not target.volume_name:
            raise HTTPException(
                status_code=400, detail="volume_name required for volume type"
            )
        if target.target_type == "path" and not target.host_path:
            raise HTTPException(
                status_code=400, detail="host_path required for path type"
            )
        if target.target_type == "stack" and not target.stack_name:
            raise HTTPException(
                status_code=400, detail="stack_name required for stack type"
            )

        db_target = BackupTarget(
            name=target.name,
            target_type=target.target_type,
            container_name=target.container_name,
            volume_name=target.volume_name,
            host_path=target.host_path,
            stack_name=target.stack_name,
            schedule_id=target.schedule_id,
            schedule_cron=target.schedule_cron,
            enabled=target.enabled,
            retention_policy_id=target.retention_policy_id,
            dependencies=target.dependencies,
            selected_volumes=target.selected_volumes,
            include_paths=target.include_paths,
            exclude_paths=target.exclude_paths,
            pre_backup_command=target.pre_backup_command,
            post_backup_command=target.post_backup_command,
            stop_container=target.stop_container,
            compression_enabled=target.compression_enabled,
        )

        session.add(db_target)
        await session.commit()

        # Reload with schedule and retention_policy relationships
        result = await session.execute(
            select(BackupTarget)
            .where(BackupTarget.id == db_target.id)
            .options(
                selectinload(BackupTarget.schedule),
                selectinload(BackupTarget.retention_policy),
            )
        )
        db_target = result.scalar_one()

        return _build_target_response(db_target)


@router.get("/{target_id}", response_model=TargetResponse)
async def get_target(target_id: int):
    """Get a specific backup target."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget)
            .where(BackupTarget.id == target_id)
            .options(
                selectinload(BackupTarget.schedule),
                selectinload(BackupTarget.retention_policy),
            )
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

        return _build_target_response(target)


@router.put("/{target_id}", response_model=TargetResponse)
async def update_target(target_id: int, update: TargetUpdate):
    """Update a backup target."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget)
            .where(BackupTarget.id == target_id)
            .options(
                selectinload(BackupTarget.schedule),
                selectinload(BackupTarget.retention_policy),
            )
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

        # Validate schedule_id if provided
        if update.schedule_id is not None:
            schedule_result = await session.execute(
                select(Schedule).where(Schedule.id == update.schedule_id)
            )
            if not schedule_result.scalar_one_or_none():
                raise HTTPException(
                    status_code=400,
                    detail=f"Schedule with id {update.schedule_id} not found",
                )
            target.schedule_id = update.schedule_id

        # Update fields
        if update.name is not None:
            target.name = update.name
        if update.schedule_cron is not None:
            target.schedule_cron = update.schedule_cron
        if update.enabled is not None:
            target.enabled = update.enabled
        if update.retention_policy_id is not None:
            target.retention_policy_id = update.retention_policy_id
        if update.dependencies is not None:
            target.dependencies = update.dependencies
        if update.selected_volumes is not None:
            target.selected_volumes = update.selected_volumes
        if update.include_paths is not None:
            target.include_paths = update.include_paths
        if update.exclude_paths is not None:
            target.exclude_paths = update.exclude_paths
        if update.pre_backup_command is not None:
            target.pre_backup_command = update.pre_backup_command
        if update.post_backup_command is not None:
            target.post_backup_command = update.post_backup_command
        if update.stop_container is not None:
            target.stop_container = update.stop_container
        if update.compression_enabled is not None:
            target.compression_enabled = update.compression_enabled

        await session.commit()

        # Reload with schedule and retention_policy relationships
        result = await session.execute(
            select(BackupTarget)
            .where(BackupTarget.id == target_id)
            .options(
                selectinload(BackupTarget.schedule),
                selectinload(BackupTarget.retention_policy),
            )
        )
        target = result.scalar_one()

        return _build_target_response(target)


@router.delete("/{target_id}")
async def delete_target(target_id: int):
    """Delete a backup target and all associated backups."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget)
            .where(BackupTarget.id == target_id)
            .options(selectinload(BackupTarget.backups))
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

        # Delete backup files from disk before removing DB records
        for backup in target.backups:
            if backup.file_path and os.path.exists(backup.file_path):
                try:
                    os.remove(backup.file_path)
                except OSError as e:
                    logger.warning(
                        f"Failed to delete backup file {backup.file_path}: {e}"
                    )

        await session.delete(target)
        await session.commit()

        return {"status": "deleted"}
