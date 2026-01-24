"""
Test configuration and shared fixtures.
"""

import asyncio
import os
import tempfile
from pathlib import Path
from typing import AsyncGenerator, Generator
import pytest
import pytest_asyncio
from httpx import AsyncClient, ASGITransport
from sqlalchemy import create_engine
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker
from unittest.mock import AsyncMock, MagicMock, patch

from app.main import app
from app.database import get_db, Base
from app.config import settings

# Override settings for testing
settings.DATABASE_URL = "sqlite+aiosqlite:///:memory:"
settings.BACKUP_BASE_PATH = "/tmp/test_backups"


@pytest.fixture(scope="session")
def event_loop():
    """Create event loop for the test session."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


@pytest_asyncio.fixture
async def test_db():
    """Create test database."""
    engine = create_async_engine(settings.DATABASE_URL, echo=False)
    
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)
    
    async_session_maker = async_sessionmaker(
        engine, class_=AsyncSession, expire_on_commit=False
    )
    
    yield async_session_maker
    
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all)
    await engine.dispose()


@pytest_asyncio.fixture
async def db_session(test_db):
    """Create database session for tests."""
    async with test_db() as session:
        yield session
        await session.rollback()


@pytest_asyncio.fixture
async def async_client(test_db):
    """Create async test client."""
    async def override_get_db():
        async with test_db() as session:
            yield session
    
    app.dependency_overrides[get_db] = override_get_db
    
    async with AsyncClient(
        transport=ASGITransport(app=app),
        base_url="http://test",
    ) as client:
        yield client
    
    app.dependency_overrides.clear()


@pytest.fixture
def mock_docker_client():
    """Mock Docker client for testing."""
    with patch('app.docker_client.docker_client') as mock:
        # Configure common mock responses
        mock.volumes.list.return_value = []
        mock.containers.list.return_value = []
        mock.api.ping.return_value = True
        yield mock


@pytest.fixture
def temp_backup_dir():
    """Create temporary directory for backup tests."""
    with tempfile.TemporaryDirectory() as temp_dir:
        yield Path(temp_dir)


@pytest.fixture
def mock_file_operations():
    """Mock file operations for testing."""
    with patch('builtins.open'), \
         patch('os.path.exists') as mock_exists, \
         patch('os.makedirs') as mock_makedirs, \
         patch('shutil.copy2') as mock_copy, \
         patch('tarfile.open') as mock_tarfile:
        
        mock_exists.return_value = True
        yield {
            'exists': mock_exists,
            'makedirs': mock_makedirs,
            'copy': mock_copy,
            'tarfile': mock_tarfile,
        }


@pytest.fixture
def mock_remote_storage():
    """Mock remote storage operations."""
    with patch('app.remote_storage.RemoteStorage') as mock:
        instance = mock.return_value
        instance.upload.return_value = AsyncMock(return_value=True)
        instance.download.return_value = AsyncMock(return_value=True)
        instance.delete.return_value = AsyncMock(return_value=True)
        instance.list.return_value = AsyncMock(return_value=[])
        yield instance
