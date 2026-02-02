"""
Backup targets API endpoints.
"""

from typing import List, Optional

from croniter import croniter
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, field_validator
from sqlalchemy import select

from app.database import BackupTarget, async_session

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
    schedule_cron: Optional[str] = None
    enabled: bool = True
    retention_policy_id: Optional[int] = None
    dependencies: List[str] = []
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
    schedule_cron: Optional[str] = None
    enabled: Optional[bool] = None
    retention_policy_id: Optional[int] = None
    dependencies: Optional[List[str]] = None
    pre_backup_command: Optional[str] = None
    post_backup_command: Optional[str] = None
    stop_container: Optional[bool] = None
    compression_enabled: Optional[bool] = None

    @field_validator("schedule_cron")
    @classmethod
    def validate_schedule_cron(cls, v: Optional[str]) -> Optional[str]:
        return validate_cron_expression(v)


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
    schedule_cron: Optional[str] = None
    enabled: bool
    retention_policy_id: Optional[int] = None
    dependencies: List[str]
    pre_backup_command: Optional[str] = None
    post_backup_command: Optional[str] = None
    stop_container: bool
    compression_enabled: bool
    created_at: str
    updated_at: str

    class Config:
        from_attributes = True


@router.get("", response_model=List[TargetResponse])
async def list_targets():
    """List all backup targets."""
    async with async_session() as session:
        result = await session.execute(select(BackupTarget))
        targets = result.scalars().all()

        return [
            TargetResponse(
                id=t.id,
                name=t.name,
                target_type=t.target_type,
                container_id=t.container_id,
                container_name=t.container_name,
                volume_name=t.volume_name,
                host_path=t.host_path,
                stack_name=t.stack_name,
                schedule_cron=t.schedule_cron,
                enabled=t.enabled,
                retention_policy_id=t.retention_policy_id,
                dependencies=t.dependencies or [],
                pre_backup_command=t.pre_backup_command,
                post_backup_command=t.post_backup_command,
                stop_container=t.stop_container,
                compression_enabled=t.compression_enabled,
                created_at=t.created_at.isoformat(),
                updated_at=t.updated_at.isoformat(),
            )
            for t in targets
        ]


@router.post("", response_model=TargetResponse)
async def create_target(target: TargetCreate):
    """Create a new backup target."""
    async with async_session() as session:
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
            schedule_cron=target.schedule_cron,
            enabled=target.enabled,
            retention_policy_id=target.retention_policy_id,
            dependencies=target.dependencies,
            pre_backup_command=target.pre_backup_command,
            post_backup_command=target.post_backup_command,
            stop_container=target.stop_container,
            compression_enabled=target.compression_enabled,
        )

        session.add(db_target)
        await session.commit()
        await session.refresh(db_target)

        return TargetResponse(
            id=db_target.id,
            name=db_target.name,
            target_type=db_target.target_type,
            container_id=db_target.container_id,
            container_name=db_target.container_name,
            volume_name=db_target.volume_name,
            host_path=db_target.host_path,
            stack_name=db_target.stack_name,
            schedule_cron=db_target.schedule_cron,
            enabled=db_target.enabled,
            retention_policy_id=db_target.retention_policy_id,
            dependencies=db_target.dependencies or [],
            pre_backup_command=db_target.pre_backup_command,
            post_backup_command=db_target.post_backup_command,
            stop_container=db_target.stop_container,
            compression_enabled=db_target.compression_enabled,
            created_at=db_target.created_at.isoformat(),
            updated_at=db_target.updated_at.isoformat(),
        )


@router.get("/{target_id}", response_model=TargetResponse)
async def get_target(target_id: int):
    """Get a specific backup target."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == target_id)
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

        return TargetResponse(
            id=target.id,
            name=target.name,
            target_type=target.target_type,
            container_id=target.container_id,
            container_name=target.container_name,
            volume_name=target.volume_name,
            host_path=target.host_path,
            stack_name=target.stack_name,
            schedule_cron=target.schedule_cron,
            enabled=target.enabled,
            retention_policy_id=target.retention_policy_id,
            dependencies=target.dependencies or [],
            pre_backup_command=target.pre_backup_command,
            post_backup_command=target.post_backup_command,
            stop_container=target.stop_container,
            compression_enabled=target.compression_enabled,
            created_at=target.created_at.isoformat(),
            updated_at=target.updated_at.isoformat(),
        )


@router.put("/{target_id}", response_model=TargetResponse)
async def update_target(target_id: int, update: TargetUpdate):
    """Update a backup target."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == target_id)
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

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
        if update.pre_backup_command is not None:
            target.pre_backup_command = update.pre_backup_command
        if update.post_backup_command is not None:
            target.post_backup_command = update.post_backup_command
        if update.stop_container is not None:
            target.stop_container = update.stop_container
        if update.compression_enabled is not None:
            target.compression_enabled = update.compression_enabled

        await session.commit()
        await session.refresh(target)

        return TargetResponse(
            id=target.id,
            name=target.name,
            target_type=target.target_type,
            container_id=target.container_id,
            container_name=target.container_name,
            volume_name=target.volume_name,
            host_path=target.host_path,
            stack_name=target.stack_name,
            schedule_cron=target.schedule_cron,
            enabled=target.enabled,
            retention_policy_id=target.retention_policy_id,
            dependencies=target.dependencies or [],
            pre_backup_command=target.pre_backup_command,
            post_backup_command=target.post_backup_command,
            stop_container=target.stop_container,
            compression_enabled=target.compression_enabled,
            created_at=target.created_at.isoformat(),
            updated_at=target.updated_at.isoformat(),
        )


@router.delete("/{target_id}")
async def delete_target(target_id: int):
    """Delete a backup target."""
    async with async_session() as session:
        result = await session.execute(
            select(BackupTarget).where(BackupTarget.id == target_id)
        )
        target = result.scalar_one_or_none()

        if not target:
            raise HTTPException(status_code=404, detail="Target not found")

        await session.delete(target)
        await session.commit()

        return {"status": "deleted"}
