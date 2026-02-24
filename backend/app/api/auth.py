"""Authentication API endpoints."""

import logging
from datetime import datetime, timezone
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, Request, Response, status
from pydantic import BaseModel, Field
from sqlalchemy import func, select

from app.auth import (
    SESSION_EXPIRE_HOURS,
    create_session,
    get_current_user,
    get_session_user,
    get_user_by_username,
    hash_password,
    invalidate_all_user_sessions,
    invalidate_session,
    verify_password,
)
from app.config import settings
from app.database import User, async_session
from app.rate_limit import limiter

router = APIRouter()
logger = logging.getLogger(__name__)


class LoginRequest(BaseModel):
    """Login request."""

    username: str = Field(..., min_length=3, max_length=255)
    password: str = Field(..., min_length=8)


class SetupRequest(BaseModel):
    """Initial setup request to create admin user."""

    username: str = Field(..., min_length=3, max_length=255)
    password: str = Field(..., min_length=8)
    confirm_password: str = Field(..., min_length=8)


class ChangePasswordRequest(BaseModel):
    """Change password request."""

    current_password: str = Field(..., min_length=1)
    new_password: str = Field(..., min_length=8)
    confirm_password: str = Field(..., min_length=8)


class UserResponse(BaseModel):
    """User response model."""

    id: int
    username: str
    is_admin: bool
    created_at: str
    last_login: Optional[str] = None


class AuthStatusResponse(BaseModel):
    """Auth status response."""

    authenticated: bool
    setup_complete: bool
    user: Optional[UserResponse] = None


@router.get("/status", response_model=AuthStatusResponse)
async def get_auth_status(request: Request):
    """
    Check authentication status.

    Returns whether user is authenticated and if initial setup is complete.
    """
    async with async_session() as db:
        # Check if any user exists (setup complete)
        result = await db.execute(select(func.count(User.id)))
        user_count = result.scalar()
        setup_complete = user_count > 0

        # Check if current user is authenticated
        token = request.cookies.get("session_token")
        user = None
        authenticated = False

        if token:
            user = await get_session_user(token, db)
            if user:
                authenticated = True

        return AuthStatusResponse(
            authenticated=authenticated,
            setup_complete=setup_complete,
            user=(
                UserResponse(
                    id=user.id,
                    username=user.username,
                    is_admin=user.is_admin,
                    created_at=user.created_at.isoformat(),
                    last_login=user.last_login.isoformat() if user.last_login else None,
                )
                if user
                else None
            ),
        )


@router.post("/setup")
@limiter.limit("5/minute")
async def initial_setup(request: Request, data: SetupRequest, response: Response):
    """
    Initial setup - create the first admin user.

    This endpoint only works when no users exist yet.
    """
    async with async_session() as db:
        # Check if setup already completed
        result = await db.execute(select(func.count(User.id)))
        user_count = result.scalar()

        if user_count > 0:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Setup already completed. Use login instead.",
            )

        # Validate passwords match
        if data.password != data.confirm_password:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Passwords do not match",
            )

        # Create admin user
        user = User(
            username=data.username,
            password_hash=hash_password(data.password),
            is_admin=True,
        )
        db.add(user)
        await db.commit()
        await db.refresh(user)

        # Create session
        token = await create_session(user.id, db)

        # Set cookie
        response.set_cookie(
            key="session_token",
            value=token,
            httponly=True,
            secure=settings.COOKIE_SECURE,
            samesite="lax",
            max_age=SESSION_EXPIRE_HOURS * 3600,
        )

        logger.info(f"Initial setup completed, created admin user: {user.username}")

        return {
            "message": "Setup completed successfully",
            "user": UserResponse(
                id=user.id,
                username=user.username,
                is_admin=user.is_admin,
                created_at=user.created_at.isoformat(),
                last_login=None,
            ),
        }


@router.post("/login")
@limiter.limit("5/minute")
async def login(request: Request, data: LoginRequest, response: Response):
    """
    Login with username and password.
    """
    async with async_session() as db:
        # Check if setup complete
        result = await db.execute(select(func.count(User.id)))
        user_count = result.scalar()

        if user_count == 0:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Setup not complete. Please create an admin account first.",
            )

        # Get user
        user = await get_user_by_username(data.username, db)

        # Constant-time comparison: always run bcrypt to prevent
        # timing side-channel that reveals whether a username exists
        DUMMY_HASH = "$2b$12$LJ3m4ys3Lg2UsSEgDKr9ceULEiZoSUsVT2D0E.kFYRPQr0K3YBmiy"
        password_valid = verify_password(
            data.password, user.password_hash if user else DUMMY_HASH
        )

        if not user or not password_valid:
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Invalid username or password",
            )

        # Update last login
        user.last_login = datetime.now(timezone.utc)
        await db.commit()

        # Create session
        token = await create_session(user.id, db)

        # Set cookie
        response.set_cookie(
            key="session_token",
            value=token,
            httponly=True,
            secure=settings.COOKIE_SECURE,
            samesite="lax",
            max_age=SESSION_EXPIRE_HOURS * 3600,
        )

        logger.info(f"User logged in: {user.username}")

        return {
            "message": "Login successful",
            "user": UserResponse(
                id=user.id,
                username=user.username,
                is_admin=user.is_admin,
                created_at=user.created_at.isoformat(),
                last_login=user.last_login.isoformat() if user.last_login else None,
            ),
        }


@router.post("/logout")
async def logout(request: Request, response: Response):
    """
    Logout current user and invalidate session.
    """
    token = request.cookies.get("session_token")

    if token:
        async with async_session() as db:
            await invalidate_session(token, db)

    # Clear cookie
    response.delete_cookie(key="session_token")

    return {"message": "Logged out successfully"}


@router.post("/change-password")
@limiter.limit("5/minute")
async def change_password(
    request: Request,
    data: ChangePasswordRequest,
    response: Response,
    current_user: User = Depends(get_current_user),
):
    """
    Change password for current user.

    Invalidates all existing sessions and creates a new one for the
    current request, forcing re-authentication on all other devices.
    """
    async with async_session() as db:
        # Get fresh user from database
        result = await db.execute(select(User).where(User.id == current_user.id))
        user = result.scalar_one_or_none()

        if not user:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="User not found",
            )

        # Verify current password
        if not verify_password(data.current_password, user.password_hash):
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Current password is incorrect",
            )

        # Validate new passwords match
        if data.new_password != data.confirm_password:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="New passwords do not match",
            )

        # Update password
        user.password_hash = hash_password(data.new_password)
        await db.commit()

        # Invalidate all sessions, then create a fresh one for this device
        count = await invalidate_all_user_sessions(user.id, db)
        new_token = await create_session(user.id, db)

        response.set_cookie(
            key="session_token",
            value=new_token,
            httponly=True,
            secure=settings.COOKIE_SECURE,
            samesite="lax",
            max_age=SESSION_EXPIRE_HOURS * 3600,
        )

        logger.info(
            f"Password changed for user {user.username}, invalidated {count} sessions"
        )

        return {"message": "Password changed successfully"}


@router.get("/me", response_model=UserResponse)
async def get_current_user_info(current_user: User = Depends(get_current_user)):
    """
    Get current user information.
    """
    return UserResponse(
        id=current_user.id,
        username=current_user.username,
        is_admin=current_user.is_admin,
        created_at=current_user.created_at.isoformat(),
        last_login=(
            current_user.last_login.isoformat() if current_user.last_login else None
        ),
    )
