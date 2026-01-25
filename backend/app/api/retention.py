"""
Retention policy API endpoints.
"""

from typing import List, Optional
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel
from sqlalchemy import select

from app.database import RetentionPolicy, async_session
from app.retention import retention_manager

router = APIRouter()


class RetentionPolicyResponse(BaseModel):
    """Retention policy response model."""
    id: int
    name: str
    keep_daily: int
    keep_weekly: int
    keep_monthly: int
    keep_yearly: int
    max_age_days: int
    created_at: str
    updated_at: str

    class Config:
        from_attributes = True


class CreateRetentionPolicyRequest(BaseModel):
    """Create retention policy request."""
    name: str
    keep_daily: int = 7
    keep_weekly: int = 4
    keep_monthly: int = 6
    keep_yearly: int = 2
    max_age_days: int = 365


class UpdateRetentionPolicyRequest(BaseModel):
    """Update retention policy request."""
    name: Optional[str] = None
    keep_daily: Optional[int] = None
    keep_weekly: Optional[int] = None
    keep_monthly: Optional[int] = None
    keep_yearly: Optional[int] = None
    max_age_days: Optional[int] = None


@router.get("", response_model=List[RetentionPolicyResponse])
async def list_retention_policies():
    """List all retention policies."""
    async with async_session() as session:
        result = await session.execute(select(RetentionPolicy))
        policies = result.scalars().all()

        return [
            RetentionPolicyResponse(
                id=p.id,
                name=p.name,
                keep_daily=p.keep_daily,
                keep_weekly=p.keep_weekly,
                keep_monthly=p.keep_monthly,
                keep_yearly=p.keep_yearly,
                max_age_days=p.max_age_days,
                created_at=p.created_at.isoformat(),
                updated_at=p.updated_at.isoformat(),
            )
            for p in policies
        ]


@router.post("", response_model=RetentionPolicyResponse)
async def create_retention_policy(request: CreateRetentionPolicyRequest):
    """Create a new retention policy."""
    async with async_session() as session:
        # Check for duplicate name
        result = await session.execute(
            select(RetentionPolicy).where(RetentionPolicy.name == request.name)
        )
        if result.scalar_one_or_none():
            raise HTTPException(
                status_code=400, detail="Policy with this name already exists"
            )

        policy = RetentionPolicy(
            name=request.name,
            keep_daily=request.keep_daily,
            keep_weekly=request.keep_weekly,
            keep_monthly=request.keep_monthly,
            keep_yearly=request.keep_yearly,
            max_age_days=request.max_age_days,
        )

        session.add(policy)
        await session.commit()
        await session.refresh(policy)

        return RetentionPolicyResponse(
            id=policy.id,
            name=policy.name,
            keep_daily=policy.keep_daily,
            keep_weekly=policy.keep_weekly,
            keep_monthly=policy.keep_monthly,
            keep_yearly=policy.keep_yearly,
            max_age_days=policy.max_age_days,
            created_at=policy.created_at.isoformat(),
            updated_at=policy.updated_at.isoformat(),
        )


@router.get("/{policy_id}", response_model=RetentionPolicyResponse)
async def get_retention_policy(policy_id: int):
    """Get a specific retention policy."""
    async with async_session() as session:
        result = await session.execute(
            select(RetentionPolicy).where(RetentionPolicy.id == policy_id)
        )
        policy = result.scalar_one_or_none()

        if not policy:
            raise HTTPException(status_code=404, detail="Policy not found")

        return RetentionPolicyResponse(
            id=policy.id,
            name=policy.name,
            keep_daily=policy.keep_daily,
            keep_weekly=policy.keep_weekly,
            keep_monthly=policy.keep_monthly,
            keep_yearly=policy.keep_yearly,
            max_age_days=policy.max_age_days,
            created_at=policy.created_at.isoformat(),
            updated_at=policy.updated_at.isoformat(),
        )


@router.put("/{policy_id}", response_model=RetentionPolicyResponse)
async def update_retention_policy(policy_id: int, request: UpdateRetentionPolicyRequest):
    """Update a retention policy."""
    async with async_session() as session:
        result = await session.execute(
            select(RetentionPolicy).where(RetentionPolicy.id == policy_id)
        )
        policy = result.scalar_one_or_none()

        if not policy:
            raise HTTPException(status_code=404, detail="Policy not found")

        if request.name is not None:
            policy.name = request.name
        if request.keep_daily is not None:
            policy.keep_daily = request.keep_daily
        if request.keep_weekly is not None:
            policy.keep_weekly = request.keep_weekly
        if request.keep_monthly is not None:
            policy.keep_monthly = request.keep_monthly
        if request.keep_yearly is not None:
            policy.keep_yearly = request.keep_yearly
        if request.max_age_days is not None:
            policy.max_age_days = request.max_age_days

        await session.commit()
        await session.refresh(policy)

        return RetentionPolicyResponse(
            id=policy.id,
            name=policy.name,
            keep_daily=policy.keep_daily,
            keep_weekly=policy.keep_weekly,
            keep_monthly=policy.keep_monthly,
            keep_yearly=policy.keep_yearly,
            max_age_days=policy.max_age_days,
            created_at=policy.created_at.isoformat(),
            updated_at=policy.updated_at.isoformat(),
        )


@router.delete("/{policy_id}")
async def delete_retention_policy(policy_id: int):
    """Delete a retention policy."""
    async with async_session() as session:
        result = await session.execute(
            select(RetentionPolicy).where(RetentionPolicy.id == policy_id)
        )
        policy = result.scalar_one_or_none()

        if not policy:
            raise HTTPException(status_code=404, detail="Policy not found")

        if policy.name == "default":
            raise HTTPException(
                status_code=400, detail="Cannot delete default policy"
            )

        await session.delete(policy)
        await session.commit()

        return {"status": "deleted"}


@router.post("/{target_id}/apply")
async def apply_retention(target_id: int):
    """Apply retention policy to a target's backups."""
    stats = await retention_manager.apply_retention(target_id)

    if "error" in stats:
        raise HTTPException(status_code=400, detail=stats["error"])

    return stats


@router.get("/{target_id}/stats")
async def get_retention_stats(target_id: int):
    """Get retention statistics for a target."""
    return await retention_manager.get_retention_stats(target_id)


@router.post("/cleanup-orphaned")
async def cleanup_orphaned_files():
    """Clean up orphaned backup files."""
    return await retention_manager.cleanup_orphaned_files()
