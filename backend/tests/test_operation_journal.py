"""Persistent recovery failures and operation ownership survive catalog changes."""

import pytest
from sqlalchemy import delete, select, text
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from app.backup_engine import BackupEngine
from app.database import (
    Backup,
    BackupLog,
    BackupStatus,
    BackupTarget,
    WorkloadOperation,
)
from app.docker_client import DockerClientWrapper
from app.operations import OperationJournal
from app import database


@pytest.mark.asyncio
@pytest.mark.parametrize("delete_backup", [False, True])
@pytest.mark.parametrize("kind", ["backup", "restore"])
async def test_unknown_recovery_is_visible_and_replay_preserves_ownership(
    test_db, delete_backup, kind
):
    class Container:
        id = "owned-immutable-id"
        status = "exited"
        starts = 0

        def start(self):
            self.starts += 1
            self.status = "running"

    container = Container()
    available = False

    def lookup(identity):
        assert identity == container.id
        if not available:
            raise LookupError("owned fixture unavailable")
        return container

    wrapper = DockerClientWrapper()
    wrapper._client = SimpleNamespace(containers=SimpleNamespace(get=lookup))
    journal = OperationJournal(test_db, wrapper)
    async with test_db() as session:
        target = BackupTarget(name="owned", target_type="path")
        session.add(target)
        await session.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED
            if kind == "restore"
            else BackupStatus.RUNNING,
        )
        session.add(backup)
        await session.commit()
        backup_id = backup.id
    operation_id = await journal.begin(backup_id, kind, {container.id: "running"})
    await journal.stop(operation_id, container.id, confirmed=True)
    if delete_backup:
        async with test_db() as session:
            await session.execute(delete(Backup).where(Backup.id == backup_id))
            await session.commit()
    engine = BackupEngine()
    with (
        patch("app.backup_engine.async_session", test_db),
        patch("app.backup_engine.docker_client", wrapper),
    ):
        with pytest.raises(
            RuntimeError, match="Manual recovery required.*owned-immutable-id"
        ):
            await engine.recover_operations()
        async with test_db() as session:
            saved = await session.get(WorkloadOperation, operation_id)
            assert saved.state == "recovery_required"
            assert not saved.resources[container.id]["restored"]
            if delete_backup:
                assert saved.backup_id is None
            else:
                backup = await session.get(Backup, backup_id)
                assert backup.status == (
                    BackupStatus.COMPLETED if kind == "restore" else BackupStatus.FAILED
                )
                if kind == "backup":
                    assert "owned-immutable-id" in backup.error_message
                assert (
                    await session.scalars(select(BackupLog))
                ).one().step == "recovery"
        assert container.starts == 0
        available = True
        await engine.recover_operations()
        await engine.recover_operations()
    assert container.starts == 1 and container.status == "running"
    async with test_db() as session:
        saved = await session.get(WorkloadOperation, operation_id)
        assert saved.state == "finished" and saved.resources[container.id]["restored"]


@pytest.mark.asyncio
async def test_legacy_jobs_fail_explicitly_without_guessing_workload_state(
    test_engine, test_db, tmp_path
):
    async with test_db() as session:
        target = BackupTarget(
            name="legacy", target_type="path", container_name="unknown-legacy-workload"
        )
        session.add(target)
        await session.flush()
        session.add_all(
            [
                Backup(target_id=target.id, status=status)
                for status in (BackupStatus.RUNNING, BackupStatus.PENDING)
            ]
        )
        await session.commit()
    docker = MagicMock(spec=DockerClientWrapper)
    with (
        patch.object(database, "engine", test_engine),
        patch.object(database, "async_session", test_db),
        patch("app.database.settings.BACKUP_BASE_PATH", str(tmp_path)),
        patch("app.backup_engine.async_session", test_db),
        patch("app.backup_engine.docker_client", docker),
    ):
        await database.init_db()
        await BackupEngine().recover_operations()
    docker.get_container_state.assert_not_called()
    docker.start_container.assert_not_called()
    async with test_db() as session:
        backups = (await session.scalars(select(Backup))).all()
        assert len(backups) == 2
        assert all(
            b.status == BackupStatus.FAILED and "unknown" in b.error_message
            for b in backups
        )
        assert (await session.scalars(select(WorkloadOperation))).all() == []


@pytest.mark.asyncio
async def test_version_three_upgrade_rolls_back_then_retries(test_engine):
    async with test_engine.begin() as connection:
        await connection.run_sync(lambda sync: WorkloadOperation.__table__.drop(sync))
        await connection.execute(
            text(
                "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT)"
            )
        )
        await connection.execute(
            text("INSERT INTO schema_migrations VALUES (1,'prior'),(2,'prior')")
        )
    original = database._create_operation_journal

    async def fail(connection):
        await original(connection)
        raise RuntimeError("controlled v3 interruption")

    with (
        patch.object(database, "engine", test_engine),
        patch.object(database, "_create_operation_journal", fail),
    ):
        with pytest.raises(RuntimeError, match="controlled v3 interruption"):
            await database.run_migrations()
    async with test_engine.connect() as connection:
        assert (
            await connection.execute(
                text("SELECT version FROM schema_migrations ORDER BY version")
            )
        ).scalars().all() == [1, 2]
        assert (
            await connection.execute(
                text("SELECT name FROM sqlite_master WHERE name='workload_operations'")
            )
        ).first() is None
    with patch.object(database, "engine", test_engine):
        await database.run_migrations()
        await database.run_migrations()
    async with test_engine.connect() as connection:
        assert (
            await connection.execute(
                text("SELECT version FROM schema_migrations ORDER BY version")
            )
        ).scalars().all() == [1, 2, 3]
        assert (
            await connection.execute(text("SELECT count(*) FROM workload_operations"))
        ).scalar_one() == 0
