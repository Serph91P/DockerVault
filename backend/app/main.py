"""
Docker Volume Backup Manager - Main Application
FastAPI backend with Docker integration, scheduling, and WebSocket support.
"""

import json
import logging
import sys
import traceback
from contextlib import asynccontextmanager

import docker
from fastapi import FastAPI, Request, status
from fastapi.exceptions import RequestValidationError
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from slowapi import _rate_limit_exceeded_handler
from slowapi.errors import RateLimitExceeded
from sqlalchemy import text
from starlette.exceptions import HTTPException as StarletteHTTPException

from app.api import router as api_router
from app.auth import get_session_user, is_setup_complete
from app.config import settings
from app.credential_encryption import _enforce_key_file_permissions
from app.database import async_session, init_db
from app.rate_limit import limiter
from app.scheduler import BackupScheduler
from app.websocket import router as ws_router


class JsonFormatter(logging.Formatter):
    """Structured JSON log formatter for production use."""

    def format(self, record: logging.LogRecord) -> str:
        log_entry = {
            "timestamp": self.formatTime(record, self.datefmt),
            "level": record.levelname,
            "logger": record.name,
            "message": record.getMessage(),
        }
        if record.exc_info and record.exc_info[0] is not None:
            log_entry["exception"] = self.formatException(record.exc_info)
        return json.dumps(log_entry)


def configure_logging() -> None:
    """Configure application logging based on settings."""
    log_level = getattr(logging, settings.LOG_LEVEL.upper(), logging.INFO)
    handler = logging.StreamHandler(sys.stdout)

    if settings.LOG_FORMAT.lower() == "json":
        handler.setFormatter(JsonFormatter())
    else:
        handler.setFormatter(
            logging.Formatter("%(asctime)s - %(name)s - %(levelname)s - %(message)s")
        )

    logging.basicConfig(level=log_level, handlers=[handler], force=True)


configure_logging()
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

    if settings.COOKIE_SECURE:
        logger.info(
            "COOKIE_SECURE=True — ensure TLS termination is in front of this service"
        )
    else:
        logger.warning(
            "COOKIE_SECURE=False — session cookies will be sent over HTTP. "
            "Set COOKIE_SECURE=True behind a TLS-terminating reverse proxy."
        )

    await init_db()
    logger.info("Database initialized")
    scheduler = BackupScheduler()
    await scheduler.start()
    logger.info("Scheduler started")
    app.state.scheduler = scheduler

    yield

    # Shutdown — drain in-progress backups before stopping scheduler
    logger.info("Shutting down DockerVault backend...")
    from app.backup_engine import BackupEngine

    backup_engine = BackupEngine()
    await backup_engine.shutdown(timeout=settings.SHUTDOWN_TIMEOUT)
    await scheduler.stop()
    logger.info("Shutdown complete")


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


@app.exception_handler(StarletteHTTPException)
async def http_exception_handler(request: Request, exc: StarletteHTTPException):
    """Handle HTTP exceptions with consistent JSON responses."""
    return JSONResponse(
        status_code=exc.status_code,
        content={"detail": exc.detail},
    )


@app.exception_handler(RequestValidationError)
async def validation_exception_handler(request: Request, exc: RequestValidationError):
    """Handle request validation errors without leaking internal details."""
    errors = []
    for error in exc.errors():
        errors.append(
            {
                "field": ".".join(str(loc) for loc in error.get("loc", [])),
                "message": error.get("msg", "Invalid value"),
            }
        )
    return JSONResponse(
        status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
        content={"detail": "Validation error", "errors": errors},
    )


@app.exception_handler(Exception)
async def unhandled_exception_handler(request: Request, exc: Exception):
    """Catch-all for unhandled exceptions — log full trace, return safe message."""
    logger.error(
        "Unhandled exception on %s %s: %s",
        request.method,
        request.url.path,
        traceback.format_exc(),
    )
    return JSONResponse(
        status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
        content={"detail": "Internal server error"},
    )


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
    """Liveness probe — always 200 if the process is up.

    Dependency checks (database, Docker) are reported as informational
    fields but do NOT cause a non-200 status, so healthcheck-aware
    reverse proxies keep routing traffic to this container.
    """
    checks: dict = {}

    # Database check
    try:
        async with async_session() as session:
            await session.execute(text("SELECT 1"))
        checks["database"] = "ok"
    except Exception:
        checks["database"] = "unavailable"

    # Docker daemon check
    client = None
    try:
        client = docker.from_env()
        client.ping()
        checks["docker"] = "ok"
    except Exception:
        checks["docker"] = "unavailable"
    finally:
        if client:
            client.close()

    return {
        "status": "healthy",
        "version": "1.0.0",
        "checks": checks,
    }
