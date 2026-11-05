"""Authenticated, user-owned durable storage operations."""

from typing import Annotated, Literal
from uuid import UUID

from fastapi import APIRouter, Depends, Query
from pydantic import BaseModel, Field

from app.auth import get_current_user
from app.database import User
from app.remote_storage import _await_io_settled
from app import storage_jobs as jobs_module

router = APIRouter()
StorageId = Literal["local"] | Annotated[int, Field(strict=True, gt=0)]


class CreateOperation(BaseModel):
    action: Literal["storage_delete"]
    storage_id: StorageId
    path: str = Field(max_length=1024)
    paths: list[Annotated[str, Field(min_length=1, max_length=1024)]] = Field(
        min_length=1, max_length=10000
    )
    request_id: UUID


class RetryOperation(BaseModel):
    request_id: UUID


@router.post("", status_code=202)
async def create_operation(
    request: CreateOperation, user: User = Depends(get_current_user)
):
    id = await _await_io_settled(jobs_module.storage_jobs.submit(user.id, request))
    return {"operation_id": id}


@router.get("")
async def list_operations(
    storage_id: Literal["local"] | Annotated[int, Field(gt=0)] | None = None,
    limit: int = Query(default=20, ge=1, le=100),
    user: User = Depends(get_current_user),
):
    return await jobs_module.storage_jobs.list(user.id, storage_id, limit)


@router.get("/{id}")
async def get_operation(id: str, user: User = Depends(get_current_user)):
    return await jobs_module.storage_jobs.detail(user.id, id)


@router.post("/{id}/cancel")
async def cancel_operation(id: str, user: User = Depends(get_current_user)):
    return await _await_io_settled(jobs_module.storage_jobs.cancel(user.id, id))


@router.post("/{id}/retry", status_code=202)
async def retry_operation(
    id: str, request: RetryOperation, user: User = Depends(get_current_user)
):
    id = await _await_io_settled(jobs_module.storage_jobs.retry(user.id, id, request))
    return {"operation_id": id}
