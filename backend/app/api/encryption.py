"""Encryption API endpoints"""

import logging
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from ..database import EncryptionConfig, get_db
from ..encryption import (
    EncryptionError,
    generate_key_pair,
    get_recovery_instructions,
)
from ..rate_limit import limiter

router = APIRouter()
logger = logging.getLogger(__name__)


class EncryptionSetupRequest(BaseModel):
    """Request to complete encryption setup"""

    confirmed_export: bool = False


class EncryptionSetupResponse(BaseModel):
    """Response with key pair for initial setup"""

    public_key: str
    private_key: str  # Only returned once during setup!
    recovery_instructions: str


class EncryptionStatusResponse(BaseModel):
    """Current encryption status"""

    setup_completed: bool
    encryption_enabled: bool
    public_key: Optional[str] = None
    key_created_at: Optional[str] = None


class ConfirmSetupRequest(BaseModel):
    """Confirm that user has exported their private key"""

    confirmed: bool


@router.get("/status", response_model=EncryptionStatusResponse)
async def get_encryption_status(db: AsyncSession = Depends(get_db)):
    """Get current encryption configuration status"""
    result = await db.execute(select(EncryptionConfig).limit(1))
    config = result.scalar_one_or_none()

    if not config:
        return EncryptionStatusResponse(
            setup_completed=False,
            encryption_enabled=False,
        )

    return EncryptionStatusResponse(
        setup_completed=config.setup_completed,
        encryption_enabled=config.encryption_enabled,
        public_key=config.public_key,
        key_created_at=(
            config.key_created_at.isoformat() if config.key_created_at else None
        ),
    )


@router.post("/setup", response_model=EncryptionSetupResponse)
@limiter.limit("3/minute")
async def setup_encryption(request: Request, db: AsyncSession = Depends(get_db)):
    """
    Generate a new encryption key pair.

    IMPORTANT: The private key is only returned ONCE during this call.
    The user MUST save it securely - it cannot be recovered!
    """
    # Check if already set up
    result = await db.execute(select(EncryptionConfig).limit(1))
    existing = result.scalar_one_or_none()

    if existing and existing.setup_completed:
        raise HTTPException(
            status_code=400,
            detail=(
                "Encryption already configured. Use /regenerate to create "
                "new keys (this will make existing backups unrecoverable!)."
            ),
        )

    try:
        # Generate new key pair
        key_pair = await generate_key_pair()

        # Store only public key
        if existing:
            existing.public_key = key_pair.public_key
            existing.setup_completed = False
        else:
            config = EncryptionConfig(
                public_key=key_pair.public_key,
                encryption_enabled=True,
                setup_completed=False,
            )
            db.add(config)

        await db.commit()

        # Generate recovery instructions
        instructions = get_recovery_instructions(key_pair.public_key)

        return EncryptionSetupResponse(
            public_key=key_pair.public_key,
            private_key=key_pair.private_key,
            recovery_instructions=instructions,
        )

    except EncryptionError as e:
        logger.error("Encryption setup failed: %s", e)
        raise HTTPException(status_code=500, detail="Encryption setup failed")


@router.post("/confirm-setup")
async def confirm_setup(
    request: ConfirmSetupRequest, db: AsyncSession = Depends(get_db)
):
    """
    Confirm that user has exported and saved their private key.

    After this, the private key cannot be retrieved again.
    """
    if not request.confirmed:
        raise HTTPException(
            status_code=400,
            detail="You must confirm that you have saved your private key.",
        )

    result = await db.execute(select(EncryptionConfig).limit(1))
    config = result.scalar_one_or_none()

    if not config:
        raise HTTPException(
            status_code=400, detail="Encryption not set up. Call /setup first."
        )

    config.setup_completed = True
    await db.commit()

    return {
        "message": "Encryption setup completed. Your backups will now be encrypted."
    }


@router.post("/toggle")
async def toggle_encryption(enabled: bool, db: AsyncSession = Depends(get_db)):
    """Enable or disable encryption for new backups"""
    result = await db.execute(select(EncryptionConfig).limit(1))
    config = result.scalar_one_or_none()

    if not config or not config.setup_completed:
        raise HTTPException(
            status_code=400,
            detail="Encryption must be set up before it can be toggled.",
        )

    config.encryption_enabled = enabled
    await db.commit()

    status = "enabled" if enabled else "disabled"
    return {"message": f"Encryption {status} for new backups."}


@router.get("/recovery-instructions")
async def get_instructions(db: AsyncSession = Depends(get_db)):
    """Get recovery instructions for the current key"""
    result = await db.execute(select(EncryptionConfig).limit(1))
    config = result.scalar_one_or_none()

    if not config:
        raise HTTPException(status_code=400, detail="Encryption not configured.")

    instructions = get_recovery_instructions(config.public_key)

    return {
        "public_key": config.public_key,
        "instructions": instructions,
    }


class RegenerateKeysRequest(BaseModel):
    """Request to regenerate encryption keys."""

    confirm_data_loss: bool


@router.post("/regenerate", response_model=EncryptionSetupResponse)
@limiter.limit("3/minute")
async def regenerate_keys(
    request: Request, data: RegenerateKeysRequest, db: AsyncSession = Depends(get_db)
):
    """
    Generate new encryption keys.

    WARNING: This will make ALL existing encrypted backups UNRECOVERABLE
    unless you still have the old private key!
    """
    if not data.confirm_data_loss:
        raise HTTPException(
            status_code=400,
            detail=(
                "You must confirm that you understand existing backups "
                "will become unrecoverable."
            ),
        )

    result = await db.execute(select(EncryptionConfig).limit(1))
    existing = result.scalar_one_or_none()

    try:
        key_pair = await generate_key_pair()

        if existing:
            existing.public_key = key_pair.public_key
            existing.setup_completed = False
        else:
            config = EncryptionConfig(
                public_key=key_pair.public_key,
                encryption_enabled=True,
                setup_completed=False,
            )
            db.add(config)

        await db.commit()

        instructions = get_recovery_instructions(key_pair.public_key)

        logger.warning(
            "Encryption keys regenerated - old backups may be unrecoverable!"
        )

        return EncryptionSetupResponse(
            public_key=key_pair.public_key,
            private_key=key_pair.private_key,
            recovery_instructions=instructions,
        )

    except EncryptionError as e:
        logger.error("Key regeneration failed: %s", e)
        raise HTTPException(status_code=500, detail="Key regeneration failed")
