"""Fair admission to real file-backed backup transactions, not SQLite busy retries."""

import asyncio
from unittest.mock import patch

import pytest
import pytest_asyncio
from sqlalchemy import text
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.database import backup_db_session


@pytest_asyncio.fixture
async def catalog(tmp_path):
    engine = create_async_engine(f"sqlite+aiosqlite:///{tmp_path / 'fair.db'}")
    async with engine.begin() as connection:
        await connection.execute(text("CREATE TABLE probes (id INTEGER PRIMARY KEY)"))
    sessions = async_sessionmaker(engine, expire_on_commit=False)
    yield sessions
    await engine.dispose()


@pytest.mark.parametrize("separate_engine", [False, True])
async def test_waiting_transaction_precedes_eager_writer_reentry(
    catalog, separate_engine
):
    other = create_async_engine(catalog.kw["bind"].url) if separate_engine else None
    second_factory = async_sessionmaker(other) if other else catalog
    entered = []
    queued = asyncio.Event()
    try:
        async with backup_db_session(catalog) as first:
            await first.execute(text("INSERT INTO probes VALUES (1)"))
            entered.append("first")

            async def waiting():
                queued.set()
                async with backup_db_session(second_factory) as second:
                    entered.append("waiting")
                    await second.execute(text("INSERT INTO probes VALUES (2)"))
                    await second.commit()

            waiter = asyncio.create_task(waiting())
            await queued.wait()
            await first.commit()
            entered.append("first committed")
        # The previous writer must not immediately jump the existing waiter.
        async with backup_db_session(catalog) as third:
            entered.append("reentry")
            await third.execute(text("INSERT INTO probes VALUES (3)"))
            await third.commit()
        await waiter
        assert entered == ["first", "first committed", "waiting", "reentry"]
        async with catalog() as session:
            assert (
                await session.execute(text("SELECT count(*) FROM probes"))
            ).scalar_one() == 3
    finally:
        if other:
            await other.dispose()


@pytest.mark.parametrize("read_only", [False, True])
async def test_cancelled_waiter_never_enters_a_live_transaction(catalog, read_only):
    queued = asyncio.Event()
    entered = []
    async with backup_db_session(catalog) as owner:
        await owner.execute(text("INSERT INTO probes VALUES (1)"))

        async def waiting():
            queued.set()
            async with backup_db_session(catalog) as session:
                entered.append("waiter")
                await session.execute(
                    text(
                        "SELECT * FROM probes"
                        if read_only
                        else "INSERT INTO probes VALUES (2)"
                    )
                )
                await session.commit()

        task = asyncio.create_task(waiting())
        await queued.wait()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert entered == []
        await owner.commit()
    async with backup_db_session(catalog) as session:
        assert (
            await session.execute(text("SELECT id FROM probes"))
        ).scalars().all() == [1]


async def test_exception_rolls_back_before_releasing_the_waiter(catalog):
    queued = asyncio.Event()
    entered = []
    waiter = None

    async def waiting():
        queued.set()
        async with backup_db_session(catalog) as session:
            entered.append("waiter")
            await session.execute(text("INSERT INTO probes VALUES (2)"))
            await session.commit()

    with pytest.raises(ValueError, match="synthetic body failure"):
        async with backup_db_session(catalog) as session:
            await session.execute(text("INSERT INTO probes VALUES (1)"))
            waiter = asyncio.create_task(waiting())
            await queued.wait()
            entered.append("owner failed")
            raise ValueError("synthetic body failure")
    await waiter
    assert entered == ["owner failed", "waiter"]
    async with catalog() as session:
        assert (
            await session.execute(text("SELECT id FROM probes"))
        ).scalars().all() == [2]


async def test_cancellation_keeps_admission_until_real_close_finishes(catalog):
    entered = []
    owner_started, waiter_started = asyncio.Event(), asyncio.Event()
    closing, finish_close = asyncio.Event(), asyncio.Event()
    original_session = catalog()
    original_close = original_session.close

    async def held_close():
        await original_close()
        closing.set()
        await finish_close.wait()
        entered.append("close finished")

    async def owner():
        async with backup_db_session(lambda: original_session) as session:
            await session.execute(text("INSERT INTO probes VALUES (1)"))
            owner_started.set()
            await asyncio.Event().wait()

    async def waiting():
        waiter_started.set()
        async with backup_db_session(catalog) as session:
            entered.append("waiter")
            await session.execute(text("INSERT INTO probes VALUES (2)"))
            await session.commit()

    with patch.object(original_session, "close", held_close):
        first = asyncio.create_task(owner())
        await owner_started.wait()
        second = asyncio.create_task(waiting())
        await waiter_started.wait()
        first.cancel()
        await closing.wait()
        first.cancel()  # A repeated edge must not release the gate or cancel close.
        finish_close.set()
        with pytest.raises(asyncio.CancelledError):
            await first
        await second
    assert entered == ["close finished", "waiter"]
    async with catalog() as session:
        assert (
            await session.execute(text("SELECT id FROM probes"))
        ).scalars().all() == [2]


async def test_an_unrelated_catalog_does_not_wait_for_this_writer(catalog, tmp_path):
    engine = create_async_engine(f"sqlite+aiosqlite:///{tmp_path / 'other.db'}")
    other = async_sessionmaker(engine)
    try:
        async with backup_db_session(catalog) as owner:
            await owner.execute(text("INSERT INTO probes VALUES (1)"))
            async with backup_db_session(other) as independent:
                assert (await independent.execute(text("SELECT 42"))).scalar_one() == 42
            await owner.commit()
    finally:
        await engine.dispose()


async def test_level_cancellation_rolls_back_without_holding_admission(catalog):
    from anyio import CancelScope

    with CancelScope() as scope:
        async with backup_db_session(catalog) as owner:
            await owner.execute(text("INSERT INTO probes VALUES (1)"))
            scope.cancel()
            await asyncio.Event().wait()
    async with backup_db_session(catalog) as next_session:
        assert (
            await next_session.execute(text("SELECT count(*) FROM probes"))
        ).scalar_one() == 0
        await next_session.execute(text("INSERT INTO probes VALUES (2)"))
        await next_session.commit()


async def test_close_failure_is_not_hidden_or_reported_as_success(catalog):
    session = catalog()
    original_close = session.close

    async def failing_close():
        await original_close()
        raise RuntimeError("synthetic cleanup failure")

    with patch.object(session, "close", failing_close):
        with pytest.raises(RuntimeError, match="synthetic cleanup failure"):
            async with backup_db_session(lambda: session) as owner:
                await owner.execute(text("INSERT INTO probes VALUES (1)"))
    async with backup_db_session(catalog) as next_session:
        assert (
            await next_session.execute(text("SELECT count(*) FROM probes"))
        ).scalar_one() == 0


async def test_in_memory_session_keeps_its_original_context_contract():
    engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    sessions = async_sessionmaker(engine)
    try:
        async with backup_db_session(sessions) as first:
            async with backup_db_session(sessions) as second:
                assert (await second.execute(text("SELECT 42"))).scalar_one() == 42
            assert (await first.execute(text("SELECT 43"))).scalar_one() == 43
    finally:
        await engine.dispose()
