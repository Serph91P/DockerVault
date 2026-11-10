"""Recovery prepares scheduler projection before commit; failures restore real RAM jobs."""

import asyncio

import pytest
from sqlalchemy import func, select

from app import database
from app import scheduler as schedulers
from app.recovery_activation import RecoveryActivationError, activate_state
from app.recovery_service import export_state
from tests.test_recovery_activation import file_database as file_database


@pytest.fixture
async def running_scheduler(file_database, monkeypatch):
    monkeypatch.setattr(schedulers, "async_session", file_database)
    scheduler = schedulers.BackupScheduler()
    await scheduler.start()
    yield scheduler
    await scheduler.stop()
    await asyncio.sleep(0)


async def source(db_session):
    db_session.add(
        database.Schedule(
            id=3, name="synthetic calendar", cron_expression="0 0 1 1 *", enabled=True
        )
    )
    await db_session.flush()
    db_session.add_all(
        [
            database.BackupTarget(
                id=identifier,
                name=f"imported {identifier}",
                target_type="volume",
                volume_name="synthetic-volume",
                schedule_id=3,
                enabled=True,
            )
            for identifier in (7, 8)
        ]
    )
    await db_session.commit()
    return await export_state(db_session)


@pytest.mark.asyncio
async def test_recovery_installs_effective_jobs_before_returning_success(
    db_session, file_database, running_scheduler
):
    payload = await source(db_session)
    assert not running_scheduler.get_scheduled_jobs()
    await activate_state(
        payload,
        "synthetic-admin",
        "synthetic-password",
        file_database,
        projection=running_scheduler,
    )
    jobs = running_scheduler.get_scheduled_jobs()
    assert {job["id"] for job in jobs} == {"backup_target_7", "backup_target_8"}
    peer = schedulers.BackupScheduler()
    await peer.start()
    try:
        peer.scheduler.remove_job("backup_target_7")
        peer.scheduler.remove_job("backup_target_8")
        await peer._reconcile_recovery()
        assert {job["id"] for job in peer.get_scheduled_jobs()} == {
            "backup_target_7",
            "backup_target_8",
        }
        assert peer._recovery_generation == running_scheduler._recovery_generation
    finally:
        await peer.stop()
        await asyncio.sleep(0)
    for job in running_scheduler.scheduler.get_jobs():
        if job.id.startswith("backup_target_"):
            assert str(job.trigger) == str(running_scheduler._trigger("0 0 1 1 *"))


@pytest.mark.asyncio
async def test_scheduler_install_failure_rolls_back_db_and_partially_installed_jobs(
    db_session, file_database, running_scheduler, monkeypatch
):
    payload = await source(db_session)
    async with file_database() as session:
        session.add(
            database.BackupTarget(
                id=1,
                name="original",
                target_type="volume",
                volume_name="original",
                schedule_cron="0 0 2 1 *",
                enabled=True,
            )
        )
        await session.commit()
    await running_scheduler.reconcile()
    original = running_scheduler.get_scheduled_jobs()
    native_add = running_scheduler.scheduler.add_job

    def fail_second(*args, **kwargs):
        if kwargs.get("id") == "backup_target_8":
            raise OSError("synthetic scheduler installation failure")
        return native_add(*args, **kwargs)

    monkeypatch.setattr(running_scheduler.scheduler, "add_job", fail_second)
    with pytest.raises(RecoveryActivationError):
        await activate_state(
            payload,
            "synthetic-admin",
            "synthetic-password",
            file_database,
            projection=running_scheduler,
        )
    assert running_scheduler.get_scheduled_jobs() == original
    async with file_database() as session:
        assert await session.get(database.BackupTarget, 1) is not None
        assert await session.get(database.BackupTarget, 7) is None
        assert await session.scalar(select(func.count(database.User.id))) == 0
        assert await session.scalar(select(func.count(database.Session.id))) == 0
        assert (
            await session.scalar(
                select(database.AppSettings.value).where(
                    database.AppSettings.key == "recovery_activation"
                )
            )
            is None
        )


@pytest.mark.asyncio
async def test_disconnect_during_durable_commit_keeps_cache_and_db_in_same_generation(
    db_session, file_database, running_scheduler, monkeypatch
):
    import json
    from sqlalchemy.ext.asyncio import AsyncSession

    payload = await source(db_session)
    committed, release = asyncio.Event(), asyncio.Event()
    native_commit = AsyncSession.commit

    async def delayed_commit(session):
        value = await session.scalar(
            select(database.AppSettings.value).where(
                database.AppSettings.key == "recovery_activation"
            )
        )
        active = value and json.loads(value)["state"] == "active"
        result = await native_commit(session)
        if active:
            committed.set()
            await release.wait()
        return result

    monkeypatch.setattr(AsyncSession, "commit", delayed_commit)
    task = asyncio.create_task(
        activate_state(
            payload,
            "synthetic-admin",
            "synthetic-password",
            file_database,
            projection=running_scheduler,
        )
    )
    try:
        await asyncio.wait_for(committed.wait(), 10)
        task.cancel()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        async with file_database() as session:
            assert await session.scalar(select(func.count(database.User.id))) == 1
            marker = json.loads(
                await session.scalar(
                    select(database.AppSettings.value).where(
                        database.AppSettings.key == "recovery_activation"
                    )
                )
            )
            assert marker["state"] == "active"
            assert running_scheduler._recovery_generation == marker["generation"]
        assert {job["id"] for job in running_scheduler.get_scheduled_jobs()} == {
            "backup_target_7",
            "backup_target_8",
        }
    finally:
        release.set()
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)
