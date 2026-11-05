"""A cancelled buffered SQLite read must not strand a live cursor/read lock."""

import asyncio
import sqlite3
from contextlib import closing
from types import SimpleNamespace

import aiosqlite
import pytest
from anyio import CancelScope
from sqlalchemy import Integer, text
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column

from app import database
from app.database import CancellationSafeAsyncSession


@pytest.mark.parametrize(
    "case",
    [
        "cancel",
        "repeat_cancel",
        "cleanup_error",
        "ordinary_error",
        "other_dialect",
        "other_driver",
        "no_context",
        "no_cursor",
    ],
)
@pytest.mark.asyncio
async def test_native_cleanup_hook_preserves_error_and_invalidation(case, caplog):
    marker = "PRIVATE_DATABASE_ERROR"
    calls = 0

    def close():
        nonlocal calls
        calls += 1
        if case == "repeat_cancel" and calls == 1:
            raise asyncio.CancelledError()
        if case == "cleanup_error":
            raise RuntimeError(marker)

    original = (
        ValueError(marker) if case == "ordinary_error" else asyncio.CancelledError()
    )
    context = SimpleNamespace(
        dialect=SimpleNamespace(
            name="postgresql" if case == "other_dialect" else "sqlite",
            driver="pysqlite" if case == "other_driver" else "aiosqlite",
        ),
        original_exception=original,
        execution_context=None
        if case == "no_context"
        else SimpleNamespace(
            cursor=None if case == "no_cursor" else SimpleNamespace(close=close)
        ),
        is_disconnect=True,
    )
    for handler in database.engine.sync_engine.dialect.dispatch.handle_error:
        assert handler(context) is None
    assert calls == (
        2
        if case == "repeat_cancel"
        else 1
        if case in {"cancel", "cleanup_error"}
        else 0
    )
    assert context.original_exception is original
    assert context.is_disconnect is True
    assert marker not in caplog.text
    if case == "cleanup_error":
        assert "Cancelled SQLite cursor cleanup failed" in caplog.text


class FixtureBase(DeclarativeBase):
    pass


class CursorFixture(FixtureBase):
    __tablename__ = "fixture"
    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    fixture_value: Mapped[int] = mapped_column(Integer)


@pytest.mark.asyncio
@pytest.mark.parametrize("cancel_count", [0, 1, 2, "anyio"])
@pytest.mark.parametrize("method", ["execute", "get", "refresh"])
@pytest.mark.parametrize("phase", ["execute", "fetchall"])
async def test_buffered_read_settles_before_cancellation_releases_session(
    tmp_path, monkeypatch, cancel_count, method, phase
):
    filename = tmp_path / "cursor.sqlite"
    engine = create_async_engine(f"sqlite+aiosqlite:///{filename}")
    sessions = async_sessionmaker(engine, class_=CancellationSafeAsyncSession)
    entered, release = asyncio.Event(), asyncio.Event()
    driver_closed = asyncio.Event()
    original = aiosqlite.Cursor.fetchall
    original_execute = aiosqlite.Cursor.execute
    original_stop = aiosqlite.Connection.stop
    stops = []
    read_connection = None
    reads = 0
    scope = None
    cancelled = False

    async def pause(cursor):
        nonlocal reads, read_connection
        if cursor.description and any(
            column[0] == "fixture_value" for column in cursor.description
        ):
            reads += 1
            # Refresh must exercise refresh itself, not the preceding get.
            if method != "refresh" or reads > 1:
                read_connection = cursor._conn
                entered.set()
                await release.wait()

    async def hold_execute(cursor, *args, **kwargs):
        result = await original_execute(cursor, *args, **kwargs)
        if phase == "execute":
            await pause(cursor)
        return result

    async def hold_buffering(cursor):
        if phase == "fetchall":
            await pause(cursor)
        return await original(cursor)

    def observe_stop(connection):
        stopped = original_stop(connection)
        if connection is read_connection and stopped is not None:
            stops.append(stopped)
            stopped.add_done_callback(lambda _: driver_closed.set())
        return stopped

    async def read():
        nonlocal scope, cancelled
        with CancelScope() as scope:
            try:
                async with sessions() as session:
                    if method == "execute":
                        result = await session.execute(
                            text("select fixture_value from fixture")
                        )
                        return result.scalar_one()
                    row = await session.get(CursorFixture, 1)
                    assert row is not None
                    if method == "refresh":
                        await session.refresh(row)
                    return row.fixture_value
            except asyncio.CancelledError:
                cancelled = True
                raise

    task = None
    try:
        async with engine.begin() as connection:
            await connection.run_sync(FixtureBase.metadata.create_all)
            await connection.execute(text("insert into fixture values (1,7)"))
        monkeypatch.setattr(aiosqlite.Cursor, "fetchall", hold_buffering)
        monkeypatch.setattr(aiosqlite.Cursor, "execute", hold_execute)
        monkeypatch.setattr(aiosqlite.Connection, "stop", observe_stop)
        task = asyncio.create_task(read())
        await entered.wait()
        # Independent SQLite control proves the real, unfinished cursor owns a read lock.
        with closing(sqlite3.connect(filename, timeout=0)) as probe:
            with pytest.raises(sqlite3.OperationalError, match="database is locked"):
                probe.execute("BEGIN EXCLUSIVE")
        if cancel_count == "anyio":
            assert scope is not None
            scope.cancel()
            await asyncio.sleep(0)
        else:
            for _ in range(cancel_count):
                task.cancel()
                await asyncio.sleep(0)
        release.set()
        if cancel_count == "anyio":
            assert await task is None
            assert cancelled
        elif cancel_count:
            with pytest.raises(asyncio.CancelledError):
                await task
        else:
            assert await task == 7
        if cancel_count:
            # SQLAlchemy may log CancelledError while invalidating under AnyIO;
            # its native stop must nevertheless close SQLite and finish the worker, without GC.
            await driver_closed.wait()
            for stopped in stops:
                if stopped.done():
                    stopped.result()
        assert engine.pool.checkedout() == 0
        # Zero-wait lock probe, not a changed product busy timeout or a timing assertion.
        with closing(sqlite3.connect(filename, timeout=0)) as probe:
            probe.execute("BEGIN EXCLUSIVE")
            probe.execute("update fixture set fixture_value=8")
            probe.commit()
            assert probe.execute("select fixture_value from fixture").fetchone() == (8,)
        async with sessions() as session:
            assert (
                await session.execute(text("select fixture_value from fixture"))
            ).scalar_one() == 8
    finally:
        release.set()
        if task is not None:
            await asyncio.gather(task, return_exceptions=True)
        await engine.dispose()
