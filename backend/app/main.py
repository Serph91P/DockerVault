"""
Docker Volume Backup Manager - Main Application
FastAPI backend with Docker integration, scheduling, and WebSocket support.
"""

import logging
import sys
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request, status
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from slowapi import _rate_limit_exceeded_handler
from slowapi.errors import RateLimitExceeded

from app.api import router as api_router
from app.auth import get_session_user, is_setup_complete
from app.config import settings
from app.credential_encryption import _enforce_key_file_permissions
from app.database import async_session, init_db
from app.rate_limit import limiter
from app.scheduler import BackupScheduler
from app.websocket import router as ws_router

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    handlers=[logging.StreamHandler(sys.stdout)],
)
logger = logging.getLogger(__name__)

# Paths that don't require authentication
PUBLIC_PATHS = {
    "/health",
    "/api/v1/auth/status",
    "/api/v1/auth/setup",
    "/api/v1/auth/login",
}


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan manager."""
    # Startup
    logger.info("Starting DockerVault backend...")
    _enforce_key_file_permissions()
    await init_db()
    logger.info("Database initialized")
    scheduler = BackupScheduler()
    await scheduler.start()
    logger.info("Scheduler started")
    app.state.scheduler = scheduler

    yield

    # Shutdown
    logger.info("Shutting down DockerVault backend...")
    await scheduler.stop()


app = FastAPI(
    title="Docker Volume Backup Manager",
    description=(
        "Automated backup solution for Docker volumes "
        "and host paths with dependency management"
    ),
    version="1.0.0",
    lifespan=lifespan,
    docs_url="/docs" if settings.ENABLE_DOCS else None,
    redoc_url="/redoc" if settings.ENABLE_DOCS else None,
    openapi_url="/openapi.json" if settings.ENABLE_DOCS else None,
)

app.state.limiter = limiter
app.add_exception_handler(RateLimitExceeded, _rate_limit_exceeded_handler)


@app.middleware("http")
async def auth_middleware(request: Request, call_next):
    """
    Authentication middleware.

    Protects all routes except public paths.
    Redirects to setup if no users exist.
    """
    path = request.url.path

    # Allow public paths
    if any(path.startswith(p) for p in PUBLIC_PATHS):
        return await call_next(request)

    # Check if setup is complete
    setup_complete = await is_setup_complete()
    if not setup_complete:
        return JSONResponse(
            status_code=status.HTTP_403_FORBIDDEN,
            content={"detail": "Setup required", "setup_required": True},
        )

    # Get session token from cookie or header
    token = request.cookies.get("session_token")

    if not token:
        auth_header = request.headers.get("Authorization")
        if auth_header and auth_header.startswith("Bearer "):
            token = auth_header[7:]

    if not token:
        return JSONResponse(
            status_code=status.HTTP_401_UNAUTHORIZED,
            content={"detail": "Not authenticated"},
        )

    # Validate session
    async with async_session() as db:
        user = await get_session_user(token, db)
        if not user:
            return JSONResponse(
                status_code=status.HTTP_401_UNAUTHORIZED,
                content={"detail": "Invalid or expired session"},
            )

    # Continue with request
    return await call_next(request)


# CORS Configuration
app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.CORS_ORIGINS.split(","),
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Include routers
app.include_router(api_router, prefix="/api/v1")
app.include_router(ws_router, prefix="/ws")


@app.get("/health")
async def health_check():
    """Health check endpoint."""
    return {"status": "healthy", "version": "1.0.0"}
