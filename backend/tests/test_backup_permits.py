"""Real archive flows share capacity through cleanup/upload across entrypoints."""

import asyncio
from pathlib import Path
from unittest.mock import patch

import pytest
import pytest_asyncio
from sqlalchemy import select
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.backup_engine import BackupEngine
from app.database import (
    Base,
    Backup,
    BackupStatus,
    BackupTarget,
    RemoteStorage,
    Schedule,
)
from app.remote_storage import storage_manager
from app.scheduler import BackupScheduler


@pytest_asyncio.fixture
async def test_db(tmp_path):
    # Separate real connections; an in-memory StaticPool masks transaction races.
    db = create_async_engine(f"sqlite+aiosqlite:///{tmp_path / 'permits.db'}")
    async with db.begin() as connection:
        await connection.run_sync(Base.metadata.create_all)
    yield async_sessionmaker(db, expire_on_commit=False)
    await db.dispose()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "entrypoint", ["direct", "scheduled", "trigger-all", "direct-same-target-waiter"]
)
@pytest.mark.parametrize("phase", ["archive", "cleanup", "upload"])
async def test_all_callers_hold_capacity_through_work_and_cleanup(
    async_client, test_db, tmp_path, entrypoint, phase
):
    engine = BackupEngine()
    third_attempt = asyncio.Event()

    class ObservedSemaphore(asyncio.Semaphore):
        attempts = 0

        async def acquire(self):
            self.attempts += 1
            if self.attempts == 3:
                third_attempt.set()
            return await super().acquire()

    engine._backup_semaphore = ObservedSemaphore(2)
    async with test_db() as session:
        session.add(
            RemoteStorage(
                id=1,
                name="owned",
                storage_type="local",
                base_path=str(tmp_path / "remote"),
                enabled=True,
            )
        )
        targets = []
        for index in range(3):
            source = tmp_path / f"source-{index}"
            source.mkdir()
            (source / "payload").write_bytes(b"real recoverable bytes")
            schedule = Schedule(name=f"schedule-{index}", cron_expression="0 2 * * *")
            session.add(schedule)
            await session.flush()
            target = BackupTarget(
                name=f"target-{index}",
                target_type="path",
                host_path=str(source),
                schedule_id=schedule.id,
                stop_container=False,
                remote_storage_ids=[1],
            )
            session.add(target)
            targets.append(target)
        await session.commit()

    two_entered = asyncio.Event()
    release = asyncio.Event()
    entries = []
    active = 0
    maximum = 0
    selected = {
        "archive": "_create_backup_archive",
        "cleanup": "_restart_containers_cancellation_safe",
        "upload": "_sync_to_remote",
    }[phase]
    original = getattr(engine, selected)

    async def gated(*args, **kwargs):
        nonlocal active, maximum
        entries.append(args)
        active += 1
        maximum = max(maximum, active)
        if len(entries) >= 2:
            two_entered.set()
        try:
            await release.wait()
            return await original(*args, **kwargs)
        finally:
            active -= 1

    tasks = []
    held_lock = None
    if entrypoint == "direct-same-target-waiter":
        held_lock = engine._get_target_lock(targets[0].id)
        await held_lock.acquire()
    original_batch = engine.run_batch_sequential

    async def tracked_batch(ids):
        tasks.append(asyncio.current_task())
        return await original_batch(ids)

    with (
        patch("app.backup_engine.settings.BACKUP_BASE_PATH", str(tmp_path / "backups")),
        patch.object(engine, "_sudo_available", return_value=False),
        patch("app.api.schedules.backup_engine", engine),
        patch("app.scheduler.backup_engine", engine),
        patch.object(engine, selected, side_effect=gated),
        patch.object(engine, "run_batch_sequential", side_effect=tracked_batch),
        patch.object(storage_manager, "backends", {}),
        patch.object(storage_manager, "configs", {}),
    ):
        try:
            if entrypoint == "trigger-all":
                responses = await asyncio.gather(
                    *(
                        async_client.post(
                            f"/api/v1/schedules/{t.schedule_id}/trigger-all"
                        )
                        for t in targets
                    )
                )
                assert [r.status_code for r in responses] == [200, 200, 200]
            elif entrypoint == "scheduled":
                scheduler = BackupScheduler()
                tasks.extend(
                    asyncio.create_task(scheduler._run_scheduled_backup(t.id))
                    for t in targets
                )
            else:
                for target in targets:
                    backup = await engine.create_backup(target)
                    tasks.append(asyncio.create_task(engine.run_backup(backup.id)))
            await asyncio.wait_for(two_entered.wait(), 10)
            if held_lock:
                assert engine._backup_semaphore.attempts == 2
            else:
                await asyncio.wait_for(third_attempt.wait(), 10)
            assert engine._backup_semaphore.locked(), (
                "capacity released before work/cleanup"
            )
            assert len(entries) == 2, (
                "third target entered before a permit was released"
            )
        finally:
            if held_lock:
                held_lock.release()
            release.set()
            await asyncio.wait_for(asyncio.gather(*tasks), 20)

    assert maximum == 2
    assert len(entries) == 3
    async with test_db() as session:
        backups = (await session.scalars(select(Backup))).all()
        assert len(backups) == 3
        assert all(b.status == BackupStatus.COMPLETED for b in backups)
        assert all(Path(b.file_path).is_file() for b in backups)
        for backup in backups:
            local = Path(backup.file_path)
            target_name = next(t.name for t in targets if t.id == backup.target_id)
            remote = tmp_path / "remote" / target_name / local.name
            assert remote.read_bytes() == local.read_bytes()
