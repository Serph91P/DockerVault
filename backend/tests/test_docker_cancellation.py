"""Cancellation must settle a real SDK executor stop before recovery starts."""

import asyncio
from pathlib import Path
import tarfile
import threading
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import pytest

from app.backup_engine import BackupEngine
from app.database import Backup, BackupStatus, BackupTarget
from app.docker_client import DockerClientWrapper


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", ["backup", "restore"])
@pytest.mark.parametrize(
    "scenario",
    ["cancel", "repeat-cancel", "failed-cancel", "stop-failed", "already-stopped"],
)
async def test_sdk_stop_settles_before_cancellation_recovery(
    test_db, tmp_path, operation, scenario
):
    entered, released, settled = threading.Event(), threading.Event(), threading.Event()
    cleanup_entered = asyncio.Event()

    class SDKContainer:
        id = "owned-workload-id"
        status = "exited" if scenario == "already-stopped" else "running"
        stops = 0
        starts = 0

        def stop(self, timeout):
            assert timeout == 30
            self.stops += 1
            entered.set()
            try:
                if not released.wait(10):
                    raise RuntimeError("test did not release SDK stop")
                if scenario in ("failed-cancel", "stop-failed"):
                    raise RuntimeError("controlled SDK stop failure")
                self.status = "exited"
            finally:
                settled.set()

        def start(self):
            assert settled.is_set(), "restart raced ahead of pending external stop"
            self.starts += 1
            self.status = "running"

    container = SDKContainer()
    wrapper = DockerClientWrapper()
    wrapper._client = SimpleNamespace(
        containers=SimpleNamespace(get=lambda name: container)
    )
    source = tmp_path / "source"
    source.mkdir()
    (source / "payload").write_bytes(b"original bytes")
    archive = tmp_path / "restore.tar"
    with tarfile.open(archive, "w") as tar:
        tar.add(source, arcname="source")
    async with test_db() as session:
        target = BackupTarget(
            name="owned",
            target_type="path",
            host_path=str(source),
            container_name="owned-workload",
            stop_container=True,
        )
        session.add(target)
        await session.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED
            if operation == "restore"
            else BackupStatus.PENDING,
            file_path=str(archive) if operation == "restore" else None,
        )
        session.add(backup)
        await session.commit()
        backup_id = backup.id
    engine = BackupEngine()
    original_cleanup = engine._restart_containers_cancellation_safe

    async def cleanup(*args, **kwargs):
        cleanup_entered.set()
        return await original_cleanup(*args, **kwargs)

    with (
        patch("app.backup_engine.async_session", test_db),
        patch("app.backup_engine.settings.BACKUP_BASE_PATH", str(tmp_path / "backups")),
        patch("app.backup_engine.docker_client", wrapper),
        patch.object(
            wrapper, "get_dependency_order", AsyncMock(return_value=["owned-workload"])
        ),
        patch.object(engine, "_sudo_available", return_value=False),
        patch.object(
            engine, "_restart_containers_cancellation_safe", side_effect=cleanup
        ),
    ):
        if scenario in ("stop-failed", "already-stopped"):
            released.set()
        task = asyncio.create_task(
            engine.run_backup(backup_id, skip_validation=True)
            if operation == "backup"
            else engine.restore_backup(backup_id)
        )
        try:
            if "cancel" in scenario:
                assert await asyncio.to_thread(entered.wait, 10)
                task.cancel("operator-cancel")
                await asyncio.sleep(
                    0
                )  # Deliver cancellation, not a wall-clock grace period.
                assert not cleanup_entered.is_set(), (
                    "cleanup started before SDK stop settled"
                )
                assert not task.done()
                if scenario == "repeat-cancel":
                    for _ in range(3):
                        task.cancel("repeat")
                        await asyncio.sleep(0)
                        assert not cleanup_entered.is_set()
                        assert not task.done()
                released.set()
                with pytest.raises(asyncio.CancelledError) as error:
                    await asyncio.wait_for(task, 10)
                assert str(error.value) == "operator-cancel"
            else:
                assert await asyncio.wait_for(task, 10) is (
                    scenario == "already-stopped"
                )
        finally:
            released.set()
            await asyncio.gather(task, return_exceptions=True)
            if entered.is_set():
                assert await asyncio.to_thread(settled.wait, 10)

    assert cleanup_entered.is_set()
    assert container.status == (
        "exited" if scenario == "already-stopped" else "running"
    )
    assert container.stops == (0 if scenario == "already-stopped" else 1)
    assert container.starts == (1 if scenario in ("cancel", "repeat-cancel") else 0)
    async with test_db() as session:
        backup = await session.get(Backup, backup_id)
        if operation == "backup":
            assert backup.status == (
                BackupStatus.COMPLETED
                if scenario == "already-stopped"
                else BackupStatus.FAILED
            )
        else:
            assert backup.status == BackupStatus.COMPLETED
    assert (source / "payload").read_bytes() == b"original bytes"
