"""Level cancellation must not spin or release ownership before real cleanup."""

import asyncio
from contextlib import asynccontextmanager
from types import SimpleNamespace
import threading

import anyio
import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app import database
from app.backup_engine import BackupEngine
from app.docker_client import DockerClientWrapper
from app.remote_storage import _await_io_settled


async def cancel_while_held(operation, entered, release, mode):
    """Observe cancellation deliveries while a real operation remains gated."""
    scopes = []
    propagated = asyncio.Event()

    async def run():
        with anyio.CancelScope() as scope:
            scopes.append(scope)
            try:
                await operation()
            except asyncio.CancelledError:
                propagated.set()
                raise
            pytest.fail("Cancellation was converted to success")

    task = asyncio.create_task(run())
    try:
        await asyncio.wait_for(entered.wait(), 2)
        if mode == "level":
            scopes[0].cancel()
        else:
            task.cancel()
        for _ in range(20):
            await asyncio.sleep(0)
        assert not task.done(), "Cleanup returned before the gated I/O settled"
        task.cancel()  # A second asyncio edge must not interrupt cleanup either.
        for _ in range(20):
            await asyncio.sleep(0)
        deliveries = task.cancelling()
        assert not task.done(), "Repeated cancellation released live I/O"
    finally:
        release.set()
        await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 3)
    assert propagated.is_set(), "Cancellation must reach the original caller"
    assert deliveries <= 4, f"Hot cancellation loop: {deliveries} deliveries"


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["edge", "level"])
@pytest.mark.parametrize("boundary", ["copy", "executor", "restart", "docker"])
async def test_worker_cleanup_does_not_spin(tmp_path, mode, boundary):
    entered, release = asyncio.Event(), threading.Event()
    loop = asyncio.get_running_loop()
    output = tmp_path / "settled"

    def worker(*args, **kwargs):
        loop.call_soon_threadsafe(entered.set)
        if not release.wait(5):
            raise TimeoutError("Test worker was not released")
        output.write_bytes(b"physical I/O finished")

    engine = BackupEngine()
    if boundary == "copy":

        async def operation():
            await _await_io_settled(asyncio.to_thread(worker))
    elif boundary == "executor":

        async def operation():
            await engine._run_executor_cancellation_safe(worker)
    elif boundary == "restart":

        async def restart(*args):
            await asyncio.to_thread(worker)
            return []

        engine._restart_containers = restart

        async def operation():
            await engine._restart_containers_cancellation_safe(["owned"], {})
    else:
        docker = DockerClientWrapper()
        docker._client = SimpleNamespace(
            containers=SimpleNamespace(get=lambda name: SimpleNamespace(stop=worker))
        )

        async def operation():
            await docker.stop_container("owned")

    await cancel_while_held(operation, entered, release, mode)
    assert output.read_bytes() == b"physical I/O finished"


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["edge", "level"])
async def test_sync_claims_survive_single_abort_and_settlement(monkeypatch, mode):
    engine = BackupEngine()
    entered, aborting, release = (asyncio.Event() for _ in range(3))
    aborts = []

    @asynccontextmanager
    async def empty_catalog():
        yield SimpleNamespace(execute=execute)

    async def execute(statement):
        return SimpleNamespace(all=lambda: [])

    async def transfer(*args):
        entered.set()
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            aborts.append("provider abort")
            aborting.set()
            await release.wait()
            assert all(lock.locked() for lock in engine._remote_sync_locks.values())
            raise

    monkeypatch.setattr("app.backup_engine.async_session", empty_catalog)
    monkeypatch.setattr(engine, "_sync_to_remote_claimed", transfer)

    async def operation():
        await engine._sync_to_remote(
            1, SimpleNamespace(name="owned"), "/owned/archive", storage_ids=[1]
        )

    await cancel_while_held(operation, entered, release, mode)
    assert aborting.is_set() and aborts == ["provider abort"]
    assert not any(lock.locked() for lock in engine._remote_sync_locks.values())


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["edge", "level"])
async def test_dependency_session_rollback_settles_without_spin(
    tmp_path, monkeypatch, mode
):
    engine = create_async_engine(f"sqlite+aiosqlite:///{tmp_path / 'owned.db'}")
    maker = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    monkeypatch.setattr(database, "async_session", maker)
    entered, release, rolled_back = (asyncio.Event() for _ in range(3))
    import aiosqlite

    original = aiosqlite.Connection.rollback

    async def rollback(connection):
        entered.set()
        await release.wait()
        await original(connection)
        rolled_back.set()

    async with engine.begin() as connection:
        await connection.execute(text("CREATE TABLE owned (id INTEGER)"))
    monkeypatch.setattr(aiosqlite.Connection, "rollback", rollback)

    async def operation():
        # Cleanup happens after yield, in the same task, as for a FastAPI dependency.
        async with asynccontextmanager(database.get_session)() as session:
            await session.execute(text("INSERT INTO owned VALUES (1)"))

    try:
        await cancel_while_held(operation, entered, release, mode)
        assert rolled_back.is_set()
        async with maker() as session:
            assert await session.scalar(text("SELECT count(*) FROM owned")) == 0
    finally:
        release.set()
        await engine.dispose()
