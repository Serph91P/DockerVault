"""Ownership is shared by resource identity, not configuration target IDs."""

import asyncio

import tarfile
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import pytest

from app.backup_engine import BackupEngine
from app.database import Backup, BackupStatus, BackupTarget
from app.docker_client import DockerClientWrapper


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "case",
    [
        "shared",
        "partial",
        "inverse",
        "aliases",
        "independent",
        "restore",
        "path",
        "cancel-waiter",
        "identity-failure",
        "identity-change",
    ],
)
async def test_resource_ownership_spans_stop_archive_restore_and_restart(
    test_db, tmp_path, case
):
    blocked = asyncio.Event()

    class ObservedLock(asyncio.Lock):
        async def acquire(self):
            if self.locked():
                blocked.set()
            return await super().acquire()

    class ObservedResources(dict):
        def setdefault(self, key, default=None):
            return super().setdefault(key, ObservedLock())

    class SDKContainer:
        def __init__(self, name):
            self.id = f"id-{name}"
            self.status = "running"

        def stop(self, timeout):
            self.status = "exited"

        def start(self):
            self.status = "running"

    containers = {name: SDKContainer(name) for name in ("first", "shared", "last")}
    registry = {**containers, **{c.id: c for c in containers.values()}}
    shared_lookups = 0

    def lookup(name):
        nonlocal shared_lookups
        if case == "identity-change" and name == "shared":
            shared_lookups += 1
            return containers["shared"] if shared_lookups == 1 else containers["last"]
        if (
            case == "identity-failure"
            and name == "id-shared"
            and a_entered.is_set()
            and not a_release.is_set()
        ):
            raise RuntimeError("controlled identity lookup failure")
        return registry[name]

    wrapper = DockerClientWrapper()
    wrapper._client = SimpleNamespace(containers=SimpleNamespace(get=lookup))
    first = ["first", "shared"] if case in ("partial", "inverse") else ["shared"]
    second = {
        "partial": ["shared", "last"],
        "inverse": ["shared", "first"],
        "aliases": ["id-shared"],
        "identity-failure": ["id-shared"],
        "identity-change": ["id-shared"],
        "independent": ["last"],
        "path": ["last"],
        "cancel-waiter": ["first", "shared"],
    }.get(case, ["shared"])
    engine = BackupEngine()
    # Instrument real mutex contention; the competing archive event makes the
    # missing-lock baseline fail deterministically, without negative time sleeps.
    engine._workload_locks = ObservedResources()
    backups = []
    async with test_db() as session:
        for index, names in enumerate((first, second)):
            source = tmp_path / f"source-{index}"
            if case == "path" and index == 1:
                source.symlink_to(tmp_path / "source-0", target_is_directory=True)
            else:
                source.mkdir()
                (source / "payload").write_bytes(b"original synthetic bytes")
            target = BackupTarget(
                name=f"target-{index}",
                target_type="path",
                host_path=str(source),
                container_name=names[0],
                dependencies=names[1:],
                stop_container=True,
            )
            session.add(target)
            await session.flush()
            archive = tmp_path / f"restore-{index}.tar"
            with tarfile.open(archive, "w") as tar:
                tar.add(source, arcname=source.name)
            backup = Backup(
                target_id=target.id,
                status=BackupStatus.COMPLETED
                if case == "restore" and index == 1
                else BackupStatus.PENDING,
                file_path=str(archive) if case == "restore" and index == 1 else None,
            )
            session.add(backup)
            backups.append(backup)
        await session.commit()

    a_entered, b_entered = asyncio.Event(), asyncio.Event()
    a_release, b_release = asyncio.Event(), asyncio.Event()
    original_archive = engine._create_backup_archive
    original_executor = engine._run_executor_cancellation_safe

    async def read_boundary(index):
        (a_entered if index == 0 else b_entered).set()
        await (a_release if index == 0 else b_release).wait()
        names = first if index == 0 else second
        assert all(registry[name].status == "exited" for name in names), (
            "read raced workload restart"
        )

    async def archive(target, backup_id, **kwargs):
        await read_boundary(0 if backup_id == backups[0].id else 1)
        return await original_archive(target, backup_id, **kwargs)

    async def extract(func):
        if case == "restore" and asyncio.current_task() is b:
            await read_boundary(1)
        return await original_executor(func)

    with (
        patch("app.backup_engine.async_session", test_db),
        patch("app.backup_engine.settings.BACKUP_BASE_PATH", str(tmp_path / "backups")),
        patch("app.backup_engine.docker_client", wrapper),
        patch.object(
            wrapper, "get_dependency_order", AsyncMock(side_effect=lambda names: names)
        ),
        patch.object(engine, "_sudo_available", return_value=False),
        patch.object(engine, "_create_backup_archive", side_effect=archive),
        patch.object(engine, "_run_executor_cancellation_safe", side_effect=extract),
    ):
        a = asyncio.create_task(engine.run_backup(backups[0].id, skip_validation=True))
        b = None
        observers = []
        try:
            await asyncio.wait_for(a_entered.wait(), 10)
            b = asyncio.create_task(
                engine.restore_backup(backups[1].id)
                if case == "restore"
                else engine.run_backup(backups[1].id, skip_validation=True)
            )
            observers = [
                asyncio.create_task(b_entered.wait()),
                asyncio.create_task(blocked.wait()),
            ]
            done, _ = await asyncio.wait(
                [*observers, b], timeout=10, return_when=asyncio.FIRST_COMPLETED
            )
            assert done
            if case == "identity-failure":
                assert b.done() and b.result() is False
            elif case == "independent":
                assert b_entered.is_set() and not blocked.is_set()
            else:
                assert blocked.is_set() and not b_entered.is_set(), (
                    "shared workload critical sections overlap"
                )
            if case == "cancel-waiter":
                b.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await b
                assert not engine._workload_locks["container:id-first"].locked()
            a_release.set()
            assert await asyncio.wait_for(a, 10) is True
            if case not in ("cancel-waiter", "identity-failure"):
                await asyncio.wait_for(b_entered.wait(), 10)
                b_release.set()
                assert await asyncio.wait_for(b, 10) is True
        finally:
            a_release.set()
            b_release.set()
            await asyncio.gather(
                *(task for task in (a, b) if task), return_exceptions=True
            )
            for observer in observers:
                observer.cancel()
            await asyncio.gather(*observers, return_exceptions=True)
    assert all(container.status == "running" for container in containers.values())
    assert all(
        (tmp_path / f"source-{i}" / "payload").read_bytes()
        == b"original synthetic bytes"
        for i in range(2)
    )
