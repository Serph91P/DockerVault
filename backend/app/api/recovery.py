"""Confidential system-backup export and first-run-only recovery endpoints."""

import asyncio
import json
import logging
import secrets

from fastapi import APIRouter, Depends, HTTPException, Request, Response
from fastapi.responses import StreamingResponse
from sqlalchemy import func, select, text

from app import database
from app.auth import SESSION_EXPIRE_HOURS, get_current_user
from app.config import settings
from app.database import AppSettings, User
from app.rate_limit import limiter
from app.recovery_activation import (
    RecoveryActivationError,
    SetupClaimError,
    activate_state,
    crypto_claim,
)
from app.recovery_bundle import (
    MAX_BUNDLE_BYTES,
    RecoveryBundleError,
    _json,
    decode_bundle,
    encode_bundle,
)
from app.recovery_service import RecoveryStateError, export_state, validate_state
from app.remote_storage import _await_io_settled

router = APIRouter()
logger = logging.getLogger(__name__)
MAX_REQUEST_BYTES = MAX_BUNDLE_BYTES + 65536


async def _body(request, fields):
    data = bytearray()
    async for chunk in request.stream():
        if len(data) + len(chunk) > MAX_REQUEST_BYTES:
            raise HTTPException(
                status_code=413, detail="Recovery request exceeds size limit"
            )
        data.extend(chunk)
    try:
        value = _json(bytes(data))
        if set(value) != set(fields) or any(
            type(item) is not str for item in value.values()
        ):
            raise ValueError()
        for key, item in value.items():
            limit = MAX_BUNDLE_BYTES if key == "bundle" else 1024
            if len(item.encode()) > limit:
                raise ValueError()
        return value
    except (ValueError, UnicodeError):
        raise HTTPException(
            status_code=400, detail="Invalid recovery request"
        ) from None


async def _require_setup():
    async with database.async_session() as session:
        if await session.scalar(select(func.count(User.id))):
            raise HTTPException(
                status_code=403,
                detail="Recovery is only available before initial setup",
            )


def _failure(error):
    if isinstance(error, (RecoveryBundleError, RecoveryStateError)):
        return HTTPException(
            status_code=400,
            detail="Invalid or incompatible recovery bundle or password",
        )
    if isinstance(error, SetupClaimError):
        return HTTPException(
            status_code=409, detail="Setup or recovery is already in progress"
        )
    if isinstance(error, RecoveryActivationError):
        return HTTPException(status_code=500, detail=str(error))
    logger.error("System recovery failed internally (%s)", type(error).__name__)
    return HTTPException(
        status_code=500,
        detail="System recovery failed; inspect setup status before retrying",
    )


@router.post("/export")
@limiter.limit("5/minute")
async def export_recovery(request: Request, user: User = Depends(get_current_user)):
    if not user.is_admin:
        raise HTTPException(status_code=403, detail="Administrator access required")
    data = await _body(request, {"password", "confirm_password"})
    if len(data["password"]) < 8 or not secrets.compare_digest(
        data["password"].encode(), data["confirm_password"].encode()
    ):
        raise HTTPException(
            status_code=400,
            detail="Enter and confirm the dedicated system-backup password",
        )
    try:
        async with crypto_claim(database.async_session):
            async with database.async_session() as session:
                await session.execute(text("BEGIN"))
                payload = await export_state(session)
            bundle = await _await_io_settled(
                asyncio.to_thread(encode_bundle, payload, data["password"])
            )
        return StreamingResponse(
            content=(
                bundle[offset : offset + 65536]
                for offset in range(0, len(bundle), 65536)
            ),
            media_type="application/octet-stream",
            headers={
                "Cache-Control": "no-store",
                "Pragma": "no-cache",
                "Content-Disposition": 'attachment; filename="dockervault-system-backup.dockervault"',
                "X-Content-Type-Options": "nosniff",
            },
        )
    except Exception as error:
        raise _failure(error) from None


@router.post("/preview")
@limiter.limit("10/minute")
async def preview_recovery(request: Request):
    await _require_setup()
    data = await _body(request, {"bundle", "password"})
    try:
        async with crypto_claim(database.async_session):
            payload = await _await_io_settled(
                asyncio.to_thread(
                    decode_bundle, data["bundle"].encode(), data["password"]
                )
            )
            preview = await validate_state(payload)
        await _require_setup()
        return preview
    except HTTPException:
        raise
    except Exception as error:
        raise _failure(error) from None


@router.post("/restore")
@limiter.limit("5/minute")
async def restore_recovery(request: Request, response: Response):
    await _require_setup()
    data = await _body(
        request,
        {"bundle", "password", "username", "admin_password", "confirm_admin_password"},
    )
    if not secrets.compare_digest(
        data["admin_password"].encode(), data["confirm_admin_password"].encode()
    ):
        raise HTTPException(
            status_code=400, detail="Administrator passwords do not match"
        )
    try:
        async with crypto_claim(database.async_session):
            payload = await _await_io_settled(
                asyncio.to_thread(
                    decode_bundle, data["bundle"].encode(), data["password"]
                )
            )
        result = await activate_state(
            payload,
            data["username"],
            data["admin_password"],
            database.async_session,
            projection=getattr(request.app.state, "scheduler", None),
        )
        response.set_cookie(
            key="session_token",
            value=result["token"],
            httponly=True,
            secure=settings.COOKIE_SECURE,
            samesite="lax",
            max_age=SESSION_EXPIRE_HOURS * 3600,
        )
        return {"user": result["user"], "result": result["result"]}
    except Exception as error:
        raise _failure(error) from None


@router.get("/status")
async def recovery_status(user: User = Depends(get_current_user)):
    async with database.async_session() as session:
        value = await session.scalar(
            select(AppSettings.value).where(AppSettings.key == "recovery_result")
        )
        references = await session.scalar(
            select(AppSettings.value).where(
                AppSettings.key == "recovery_ssh_public_keys"
            )
        )
    return (
        {**json.loads(value), "ssh_public_keys": json.loads(references or "[]")}
        if value
        else None
    )
