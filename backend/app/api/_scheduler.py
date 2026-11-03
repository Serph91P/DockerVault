"""Shared post-commit scheduler projection for configuration endpoints."""

import logging

from fastapi import HTTPException, Request

logger = logging.getLogger(__name__)


async def reconcile_after_commit(request: Request) -> None:
    try:
        await request.app.state.scheduler.reconcile()
    except Exception as exc:
        logger.exception(
            "Persisted configuration could not be reconciled with live jobs"
        )
        raise HTTPException(
            status_code=503,
            detail="Configuration saved; scheduler reconciliation failed. Retry POST /api/v1/schedules/reload.",
        ) from exc
