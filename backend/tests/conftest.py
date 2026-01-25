"""
Test configuration and shared fixtures.
"""

import asyncio
import tempfile
from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest
import pytest_asyncio
from httpx import ASGITransport, AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base

# Test database URL - in-memory SQLite
TEST_DATABASE_URL = "sqlite+aiosqlite:///:memory:"


@pytest.fixture(scope="session")
def event_loop():
    """Create event loop for the test session."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


@pytest_asyncio.fixture
async def test_engine():
    """Create test database engine."""
    engine = create_async_engine(TEST_DATABASE_URL, echo=False)

    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)

    yield engine

    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all)
    await engine.dispose()


@pytest_asyncio.fixture
async def test_db(test_engine):
    """Create test database session maker."""
    async_session_maker = async_sessionmaker(
        test_engine, class_=AsyncSession, expire_on_commit=False
    )
    yield async_session_maker


@pytest_asyncio.fixture
async def db_session(test_db):
    """Create database session for tests."""
    async with test_db() as session:
        yield session
        await session.rollback()


@pytest_asyncio.fixture
async def async_client(test_db):
    """Create async test client with mocked database."""
    # Patch the async_session in all modules that import it
    with patch("app.database.async_session", test_db), patch(
        "app.api.backups.async_session", test_db
    ), patch("app.api.targets.async_session", test_db), patch(
        "app.api.schedules.async_session", test_db
    ), patch(
        "app.api.retention.async_session", test_db
    ), patch(
        "app.backup_engine.async_session", test_db
    ), patch(
        "app.scheduler.async_session", test_db
    ), patch(
        "app.retention.async_session", test_db
    ):

        # Import app after patching
        from app.main import app

        async with AsyncClient(
            transport=ASGITransport(app=app),
            base_url="http://test",
        ) as client:
            yield client


@pytest.fixture
def mock_docker_client():
    """Mock Docker client for testing."""
    with patch("app.docker_client.docker_client") as mock:
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
    with patch("builtins.open"), patch("os.path.exists") as mock_exists, patch(
        "os.makedirs"
    ) as mock_makedirs, patch("shutil.copy2") as mock_copy, patch(
        "tarfile.open"
    ) as mock_tarfile:

        mock_exists.return_value = True
        yield {
            "exists": mock_exists,
            "makedirs": mock_makedirs,
            "copy": mock_copy,
            "tarfile": mock_tarfile,
        }


@pytest.fixture
def mock_remote_storage():
    """Mock remote storage operations."""
    with patch("app.remote_storage.RemoteStorage") as mock:
        instance = mock.return_value
        instance.upload.return_value = AsyncMock(return_value=True)
        instance.download.return_value = AsyncMock(return_value=True)
        instance.delete.return_value = AsyncMock(return_value=True)
        instance.list.return_value = AsyncMock(return_value=[])
        yield instance
