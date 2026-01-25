"""
Docker Volume Backup Manager - Main Application
FastAPI backend with Docker integration, scheduling, and WebSocket support.
"""

from contextlib import asynccontextmanager

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from app.api import router as api_router
from app.config import settings
from app.database import init_db
from app.scheduler import BackupScheduler
from app.websocket import router as ws_router


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan manager."""
    # Startup
    await init_db()
    scheduler = BackupScheduler()
    await scheduler.start()
    app.state.scheduler = scheduler

    yield

    # Shutdown
    await scheduler.stop()


app = FastAPI(
    title="Docker Volume Backup Manager",
    description=(
        "Automated backup solution for Docker volumes "
        "and host paths with dependency management"
    ),
    version="1.0.0",
    lifespan=lifespan,
)

# CORS Configuration
app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.CORS_ORIGINS,
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
