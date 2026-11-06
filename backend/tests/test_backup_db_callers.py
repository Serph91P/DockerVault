"""Real backup callers must queue before SQL, not inside SQLite's busy handler."""

import asyncio
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app import backup_engine as backup_module, operations, retention, scheduler
from app.api import schedules
from app.backup_engine import ArchiveSources, BackupEngine
from app.database import (
    Base,
    Backup,
    BackupTarget,
    Schedule,
    WorkloadOperation,
    backup_db_session,
)


@pytest.mark.parametrize(
    "caller",
    [
        "create",
        "peek",
        "inner",
        "log",
        "reserve",
        "encryption",
        "sync",
        "journal-begin",
        "journal-phase",
        "journal-recovered",
        "scheduled",
        "trigger",
        "retention",
        "trigger-all",
    ],
)
async def test_backup_callers_wait_before_entering_sql(caller, tmp_path, monkeypatch):
    attempts = []

    class ObservedSession(AsyncSession):
        async def execute(self, *args, **kwargs):
            attempts.append("execute")
            return await super().execute(*args, **kwargs)

        async def get(self, *args, **kwargs):
            attempts.append("get")
            return await super().get(*args, **kwargs)

        async def commit(self):
            attempts.append("commit")
            return await super().commit()

    database = create_async_engine(f"sqlite+aiosqlite:///{tmp_path / 'callers.db'}")
    sessions = async_sessionmaker(
        database, class_=ObservedSession, expire_on_commit=False
    )
    async with database.begin() as connection:
        await connection.run_sync(Base.metadata.create_all)
    source = tmp_path / "source"
    source.mkdir()
    (source / "payload").write_bytes(b"owned payload")
    async with sessions() as session:
        plan_schedule = Schedule(name="owned", cron_expression="0 2 * * *")
        session.add(plan_schedule)
        await session.flush()
        target = BackupTarget(
            name="owned",
            target_type="path",
            host_path=str(source),
            stop_container=False,
            schedule_id=plan_schedule.id,
        )
        session.add(target)
        await session.flush()
        backup = Backup(target_id=target.id)
        session.add(backup)
        await session.flush()
        operation = WorkloadOperation(backup_id=backup.id, kind="backup", resources={})
        session.add(operation)
        await session.commit()
    for module in (backup_module, retention, scheduler, schedules):
        monkeypatch.setattr(module, "async_session", sessions)
    monkeypatch.setattr(
        backup_module.settings, "BACKUP_BASE_PATH", str(tmp_path / "backups")
    )
    from app import komodo_config

    monkeypatch.setattr(
        komodo_config,
        "load_komodo_config",
        AsyncMock(return_value=SimpleNamespace(enabled=False)),
    )
    engine = BackupEngine()
    monkeypatch.setattr(schedules, "backup_engine", engine)
    journal = operations.OperationJournal(sessions, None)
    jobs = scheduler.BackupScheduler()
    actions = {
        "create": lambda: engine.create_backup(target),
        "peek": lambda: engine.run_backup(backup.id),
        "inner": lambda: engine._run_backup_inner(backup.id),
        "log": lambda: engine._log(backup.id, "test", "owned"),
        "reserve": lambda: engine._create_backup_archive(
            target,
            backup.id,
            sources=ArchiveSources(single_path=str(source), root="source"),
        ),
        "encryption": engine._get_encryption_config,
        "sync": lambda: engine._sync_to_remote(
            backup.id, target, str(tmp_path / "owned.tar.gz")
        ),
        "journal-begin": lambda: journal.begin(backup.id, "backup", {}),
        "journal-phase": lambda: journal.phase(operation.id, "archive"),
        "journal-recovered": lambda: journal.recovered(operation.id, []),
        "scheduled": lambda: jobs._run_scheduled_backup(target.id),
        "trigger": lambda: jobs.trigger_backup_now(target.id),
        "retention": lambda: retention.RetentionManager().apply_retention(target.id),
        "trigger-all": lambda: schedules.trigger_all_backups(plan_schedule.id),
    }
    started = asyncio.Event()

    async def run():
        started.set()
        await actions[caller]()

    try:
        async with backup_db_session(sessions) as owner:
            await owner.execute(text("UPDATE backup_targets SET name=name WHERE id=1"))
            attempts.clear()
            task = asyncio.create_task(run())
            await started.wait()
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            assert attempts == [], (
                f"{caller} dispatched SQL while the owner transaction was still live"
            )
            await owner.rollback()
        async with sessions() as session:
            assert (
                await session.execute(text("SELECT count(*) FROM backups"))
            ).scalar_one() == 1
    finally:
        await database.dispose()


async def test_validation_wait_does_not_own_admission_or_deadlock_its_log(
    tmp_path, monkeypatch
):
    database = create_async_engine(f"sqlite+aiosqlite:///{tmp_path / 'validation.db'}")
    sessions = async_sessionmaker(database, expire_on_commit=False)
    async with database.begin() as connection:
        await connection.run_sync(Base.metadata.create_all)
    async with sessions() as session:
        target = BackupTarget(
            name="owned",
            target_type="path",
            host_path=str(tmp_path),
            stop_container=False,
        )
        session.add(target)
        await session.flush()
        backup = Backup(target_id=target.id)
        session.add(backup)
        await session.commit()
    monkeypatch.setattr(backup_module, "async_session", sessions)
    engine = BackupEngine()
    validating, release = asyncio.Event(), asyncio.Event()

    async def validate(_target):
        validating.set()
        await release.wait()
        return ["owned validation rejection"]

    monkeypatch.setattr(engine, "validate_backup_prerequisites", validate)
    task = asyncio.create_task(engine._run_backup_inner(backup.id))
    try:
        await asyncio.wait_for(validating.wait(), 3)
        # This real write must complete while external validation is still waiting.
        await asyncio.wait_for(engine._log(backup.id, "test", "independent write"), 3)
        release.set()
        assert await asyncio.wait_for(task, 3) is False
        async with sessions() as session:
            row = await session.get(Backup, backup.id)
            assert row.status.value == "failed" and row.file_path is None
            assert "owned validation rejection" in row.error_message
    finally:
        release.set()
        if not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
        await database.dispose()
