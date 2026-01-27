"""
Authentication module for DockerVault.

Provides password hashing, session management, and auth dependencies.
"""

import logging
import secrets
from datetime import datetime, timedelta
from typing import Optional

from fastapi import Depends, HTTPException, Request, status
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer
from passlib.context import CryptContext
from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import Session, User, async_session

logger = logging.getLogger(__name__)

# Password hashing
pwd_context = CryptContext(schemes=["bcrypt"], deprecated="auto")

# Session configuration
SESSION_EXPIRE_HOURS = 24 * 7  # 7 days
SESSION_TOKEN_LENGTH = 64

# Security scheme
security = HTTPBearer(auto_error=False)


def hash_password(password: str) -> str:
    """Hash a password using bcrypt.

    Bcrypt has a 72-byte password limit. Passwords are truncated
    to 72 bytes to prevent errors during hashing.
    """
    # Bcrypt has a 72-byte limit, truncate and pass as bytes
    password_bytes = password.encode("utf-8")[:72]
    return pwd_context.hash(password_bytes)


def verify_password(plain_password: str, hashed_password: str) -> bool:
    """Verify a password against a hash.

    Passwords are truncated to 72 bytes to match bcrypt's limit.
    """
    # Truncate to 72 bytes to match hash_password behavior
    password_bytes = plain_password.encode("utf-8")[:72]
    return pwd_context.verify(password_bytes, hashed_password)


def generate_session_token() -> str:
    """Generate a secure session token."""
    return secrets.token_urlsafe(SESSION_TOKEN_LENGTH)


async def create_session(user_id: int, db: AsyncSession) -> str:
    """Create a new session for a user."""
    token = generate_session_token()
    expires_at = datetime.utcnow() + timedelta(hours=SESSION_EXPIRE_HOURS)

    session = Session(
        user_id=user_id,
        token=token,
        expires_at=expires_at,
    )
    db.add(session)
    await db.commit()

    logger.info(f"Created session for user {user_id}")
    return token


async def invalidate_session(token: str, db: AsyncSession) -> bool:
    """Invalidate a session token."""
    result = await db.execute(delete(Session).where(Session.token == token))
    await db.commit()
    return result.rowcount > 0


async def invalidate_all_user_sessions(user_id: int, db: AsyncSession) -> int:
    """Invalidate all sessions for a user."""
    result = await db.execute(delete(Session).where(Session.user_id == user_id))
    await db.commit()
    return result.rowcount


async def cleanup_expired_sessions(db: AsyncSession) -> int:
    """Remove expired sessions from database."""
    result = await db.execute(
        delete(Session).where(Session.expires_at < datetime.utcnow())
    )
    await db.commit()
    return result.rowcount


async def get_session_user(token: str, db: AsyncSession) -> Optional[User]:
    """Get user from session token."""
    result = await db.execute(
        select(Session).where(
            Session.token == token, Session.expires_at > datetime.utcnow()
        )
    )
    session = result.scalar_one_or_none()

    if not session:
        return None

    # Get user
    user_result = await db.execute(select(User).where(User.id == session.user_id))
    return user_result.scalar_one_or_none()


async def get_user_by_username(username: str, db: AsyncSession) -> Optional[User]:
    """Get user by username."""
    result = await db.execute(select(User).where(User.username == username))
    return result.scalar_one_or_none()


async def is_setup_complete() -> bool:
    """Check if initial setup is complete (at least one user exists)."""
    async with async_session() as db:
        result = await db.execute(select(User).limit(1))
        return result.scalar_one_or_none() is not None


async def get_current_user(
    request: Request,
    credentials: Optional[HTTPAuthorizationCredentials] = Depends(security),
) -> User:
    """
    Dependency to get current authenticated user.

    Checks for token in:
    1. Authorization header (Bearer token)
    2. Cookie (session_token)
    """
    token = None

    # Check Authorization header
    if credentials:
        token = credentials.credentials

    # Check cookie
    if not token:
        token = request.cookies.get("session_token")

    if not token:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Not authenticated",
            headers={"WWW-Authenticate": "Bearer"},
        )

    async with async_session() as db:
        user = await get_session_user(token, db)

        if not user:
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Invalid or expired session",
                headers={"WWW-Authenticate": "Bearer"},
            )

        return user


async def get_optional_user(
    request: Request,
    credentials: Optional[HTTPAuthorizationCredentials] = Depends(security),
) -> Optional[User]:
    """
    Dependency to get current user if authenticated, None otherwise.
    """
    token = None

    if credentials:
        token = credentials.credentials

    if not token:
        token = request.cookies.get("session_token")

    if not token:
        return None

    async with async_session() as db:
        return await get_session_user(token, db)
