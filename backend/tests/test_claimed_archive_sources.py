"""Owner R4/R5: lock actual data aliases and retain immutable archive selection."""

import asyncio
from pathlib import Path
import tarfile
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import pytest

from app.backup_engine import BackupEngine
from app.database import Backup, BackupStatus, BackupTarget
from app.docker_client import DockerClientWrapper


def topology(source):
    container = SimpleNamespace(
        id="owned-id",
        name="service",
        status="running",
        mounts=[{"type": "volume", "name": "owned-volume", "destination": "data"}],
    )
    wrapper = DockerClientWrapper()
    wrapper._client = SimpleNamespace(
        containers=SimpleNamespace(get=lambda name: container)
    )
    wrapper.list_containers = AsyncMock(return_value=[container])
    wrapper.list_volumes = AsyncMock(
        return_value=[SimpleNamespace(name="owned-volume", mountpoint=str(source))]
    )
    wrapper.get_stacks = AsyncMock(
        return_value=[
            SimpleNamespace(
                name="owned-stack",
                containers=[container],
                volumes=["owned-volume"],
                stop_order=[],
                start_order=[],
            )
        ]
    )
    return wrapper


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["volume", "container", "stack"])
async def test_mountpoint_alias_cannot_restore_during_archive_read(
    test_db, tmp_path, kind
):
    source = tmp_path / "volume"
    source.mkdir()
    (source / "payload").write_bytes(b"original volume")
    incoming = tmp_path / "incoming.tar"
    replacement = tmp_path / "incoming"
    replacement.mkdir()
    (replacement / "payload").write_bytes(b"restored later")
    with tarfile.open(incoming, "w") as tar:
        tar.add(replacement, arcname="incoming")
    wrapper = topology(source)
    async with test_db() as db:
        target = BackupTarget(
            name="archive",
            target_type=kind,
            volume_name="owned-volume" if kind == "volume" else None,
            container_name="service" if kind == "container" else None,
            stack_name="owned-stack" if kind == "stack" else None,
            stop_container=False,
            compression_enabled=False,
        )
        other = BackupTarget(
            name="restore",
            target_type="path",
            host_path=str(source),
            stop_container=False,
        )
        db.add_all([target, other])
        await db.flush()
        backup = Backup(target_id=target.id)
        restore = Backup(
            target_id=other.id, status=BackupStatus.COMPLETED, file_path=str(incoming)
        )
        db.add_all([backup, restore])
        await db.commit()
        backup_id, restore_id = backup.id, restore.id
    entered, release, contention = asyncio.Event(), asyncio.Event(), asyncio.Event()

    class ObservedLock(asyncio.Lock):
        async def acquire(self):
            if self.locked():
                contention.set()
            return await super().acquire()

    class ObservedResources(dict):
        def setdefault(self, key, default=None):
            return super().setdefault(key, ObservedLock())

    engine = BackupEngine()
    engine._workload_locks = ObservedResources()
    original = engine._run_executor_cancellation_safe

    async def gate(func):
        if asyncio.current_task() is backup_task:
            entered.set()
            await release.wait()
        return await original(func)

    with (
        patch("app.backup_engine.async_session", test_db),
        patch("app.backup_engine.docker_client", wrapper),
        patch("app.backup_engine.settings.BACKUP_BASE_PATH", str(tmp_path / "backups")),
        patch.object(engine, "_sudo_available", return_value=False),
        patch.object(engine, "_run_executor_cancellation_safe", gate),
    ):
        backup_task = asyncio.create_task(
            engine.run_backup(backup_id, skip_validation=True)
        )
        restore_task = observer = None
        try:
            await asyncio.wait_for(entered.wait(), 5)
            restore_task = asyncio.create_task(engine.restore_backup(restore_id))
            observer = asyncio.create_task(contention.wait())
            await asyncio.wait(
                [restore_task, observer], return_when=asyncio.FIRST_COMPLETED, timeout=5
            )
            assert contention.is_set() and not restore_task.done(), (
                "Volume/container/stack and path aliases must share data ownership"
            )
            assert (source / "payload").read_bytes() == b"original volume"
            release.set()
            assert await asyncio.wait_for(backup_task, 10)
            assert await asyncio.wait_for(restore_task, 10)
        finally:
            release.set()
            await asyncio.gather(
                *(task for task in (backup_task, restore_task) if task),
                return_exceptions=True,
            )
            if observer:
                observer.cancel()
                await asyncio.gather(observer, return_exceptions=True)
    async with test_db() as db:
        backup = await db.get(Backup, backup_id)
    with tarfile.open(backup.file_path) as tar:
        contents = [
            tar.extractfile(member).read()
            for member in tar.getmembers()
            if member.isfile()
        ]
    assert contents == [b"original volume"]
    assert (source / "payload").read_bytes() == b"restored later"


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["container", "stack"])
@pytest.mark.parametrize("change", ["after_stop", "before_claim"])
async def test_archive_reads_only_claimed_immutable_container(
    test_db, tmp_path, kind, change
):
    old_source, new_source = tmp_path / "old", tmp_path / "new"
    old_source.mkdir()
    new_source.mkdir()
    (old_source / "payload").write_bytes(b"owned original container")
    (new_source / "payload").write_bytes(b"unclaimed running replacement")
    registry, current = {}, []

    class SDKContainer:
        def __init__(self, identity, volume):
            self.id, self.status, self.name = identity, "running", "service"
            self.mounts = [{"type": "volume", "name": volume, "destination": "data"}]

        def stop(self, timeout):
            self.status = "exited"
            registry["service"] = new
            current[:] = [new]

        def start(self):
            self.status = "running"

    old, new = (
        SDKContainer("immutable-old", "old-volume"),
        SDKContainer("immutable-new", "new-volume"),
    )
    registry.update({"service": old, old.id: old, new.id: new})
    current[:] = [new if change == "before_claim" else old]
    wrapper = DockerClientWrapper()
    wrapper._client = SimpleNamespace(
        containers=SimpleNamespace(get=registry.__getitem__)
    )
    wrapper.list_containers = AsyncMock(side_effect=lambda: list(current))
    wrapper.list_volumes = AsyncMock(
        return_value=[
            SimpleNamespace(name="old-volume", mountpoint=str(old_source)),
            SimpleNamespace(name="new-volume", mountpoint=str(new_source)),
        ]
    )
    wrapper.get_dependency_order = AsyncMock(side_effect=lambda names: names)
    wrapper.get_stacks = AsyncMock(
        side_effect=lambda: [
            SimpleNamespace(
                name="owned",
                containers=list(current),
                volumes=[c.mounts[0]["name"] for c in current],
                stop_order=["service"],
                start_order=["service"],
            )
        ]
    )
    async with test_db() as db:
        target = BackupTarget(
            name="identity",
            target_type=kind,
            container_name="service" if kind == "container" else None,
            stack_name="owned" if kind == "stack" else None,
            stop_container=True,
            compression_enabled=False,
        )
        db.add(target)
        await db.flush()
        backup = Backup(target_id=target.id)
        db.add(backup)
        await db.commit()
        backup_id = backup.id
    engine = BackupEngine()
    with (
        patch("app.backup_engine.async_session", test_db),
        patch("app.backup_engine.docker_client", wrapper),
        patch("app.backup_engine.settings.BACKUP_BASE_PATH", str(tmp_path / "backups")),
        patch.object(engine, "_sudo_available", return_value=False),
    ):
        result = await engine.run_backup(backup_id, skip_validation=True)
    if change == "before_claim":
        assert result is False, "Disagreeing identity/topology must fail closed"
    else:
        assert result is True
        async with test_db() as db:
            backup = await db.get(Backup, backup_id)
        with tarfile.open(backup.file_path) as tar:
            contents = [
                tar.extractfile(member).read()
                for member in tar.getmembers()
                if member.isfile()
            ]
        assert contents == [b"owned original container"], (
            "Archive must retain the immutable claimed source snapshot"
        )
    assert old.status == new.status == "running"
