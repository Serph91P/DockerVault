"""Owned real-Docker R4/R5 acceptance; runner mounts only its two labelled volumes."""

import asyncio
import os
from pathlib import Path
import tarfile
import tempfile
from unittest.mock import patch
import uuid

import docker
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.backup_engine import BackupEngine
from app.database import Base, Backup, BackupStatus, BackupTarget
from app.docker_client import DockerClientWrapper


async def run():
    client = docker.from_env()
    volumes = [os.environ[f"OWNED_VOLUME_{index}"] for index in (0, 1)]
    paths = [Path(client.volumes.get(name).attrs["Mountpoint"]) for name in volumes]
    for path, content in zip(paths, (b"owned original", b"unclaimed replacement")):
        (path / "payload").write_bytes(content)
    workloads = []
    wrapper = DockerClientWrapper()
    wrapper._client = client
    with tempfile.TemporaryDirectory(prefix="owned-workload-acceptance-") as directory:
        root = Path(directory)
        database = create_async_engine(f"sqlite+aiosqlite:///{root}/catalog.db")
        sessions = async_sessionmaker(database, expire_on_commit=False)
        async with database.begin() as connection:
            await connection.run_sync(Base.metadata.create_all)
        try:
            # An actual Docker volume source and actual path restore must serialize.
            engine = BackupEngine()
            contention, entered, release = (
                asyncio.Event(),
                asyncio.Event(),
                asyncio.Event(),
            )

            class ObservedLock(asyncio.Lock):
                async def acquire(self):
                    if self.locked():
                        contention.set()
                    return await super().acquire()

            class Registry(dict):
                def setdefault(self, key, default=None):
                    return super().setdefault(key, ObservedLock())

            engine._workload_locks = Registry()
            incoming = root / "incoming.tar"
            replacement = root / "incoming"
            replacement.mkdir()
            (replacement / "payload").write_bytes(b"restored after read")
            with tarfile.open(incoming, "w") as tar:
                tar.add(replacement, arcname="incoming")
            async with sessions() as session:
                target = BackupTarget(
                    name="alias-archive",
                    target_type="volume",
                    volume_name=volumes[0],
                    stop_container=False,
                    compression_enabled=False,
                )
                destination = BackupTarget(
                    name="alias-restore",
                    target_type="path",
                    host_path=str(paths[0]),
                    stop_container=False,
                )
                session.add_all([target, destination])
                await session.flush()
                backup = Backup(target_id=target.id)
                restore = Backup(
                    target_id=destination.id,
                    status=BackupStatus.COMPLETED,
                    file_path=str(incoming),
                )
                session.add_all([backup, restore])
                await session.commit()
                backup_id, restore_id = backup.id, restore.id
            original_executor = engine._run_executor_cancellation_safe

            async def gated(func):
                if asyncio.current_task() is backup_task:
                    entered.set()
                    await release.wait()
                return await original_executor(func)

            with (
                patch("app.backup_engine.async_session", sessions),
                patch("app.backup_engine.docker_client", wrapper),
                patch(
                    "app.backup_engine.settings.BACKUP_BASE_PATH", str(root / "backups")
                ),
                patch.object(engine, "_run_executor_cancellation_safe", gated),
                patch.object(engine, "_sudo_available", return_value=False),
            ):
                backup_task = asyncio.create_task(
                    engine.run_backup(backup_id, skip_validation=True)
                )
                restore_task = observer = None
                try:
                    await asyncio.wait_for(entered.wait(), 20)
                    restore_task = asyncio.create_task(
                        engine.restore_backup(restore_id)
                    )
                    observer = asyncio.create_task(contention.wait())
                    await asyncio.wait(
                        [restore_task, observer],
                        timeout=10,
                        return_when=asyncio.FIRST_COMPLETED,
                    )
                    assert contention.is_set() and not restore_task.done()
                    release.set()
                    assert await asyncio.wait_for(backup_task, 20)
                    assert await asyncio.wait_for(restore_task, 20)
                finally:
                    release.set()
                    await asyncio.gather(
                        *(task for task in (backup_task, restore_task) if task),
                        return_exceptions=True,
                    )
                    if observer:
                        observer.cancel()
                        await asyncio.gather(observer, return_exceptions=True)
            async with sessions() as session:
                backup = await session.get(Backup, backup_id)
            with tarfile.open(backup.file_path) as tar:
                assert [
                    tar.extractfile(member).read()
                    for member in tar.getmembers()
                    if member.isfile()
                ] == [b"owned original"]
            assert (paths[0] / "payload").read_bytes() == b"restored after read"
            print(
                "PASS R4: real Docker mountpoint/path ownership; original archive bytes before restore"
            )

            # Name reuse after a real stop must not switch the archive's mounts.
            for kind in ("container", "stack"):
                (paths[0] / "payload").write_bytes(b"owned original")
                name = "dv-t539-r5-" + uuid.uuid4().hex[:10]
                project = name + "-stack"
                labels = {
                    "dockervault.task": "t_539de28b",
                    "com.docker.compose.project": project,
                    "com.docker.compose.service": "service",
                }
                command = [
                    "sh",
                    "-c",
                    "trap 'exit 0' TERM; while :; do sleep 0.1; done",
                ]
                old = client.containers.create(
                    "node:24-alpine",
                    name=name,
                    command=command,
                    labels=labels,
                    volumes={volumes[0]: {"bind": "/data", "mode": "rw"}},
                    network_disabled=True,
                )
                workloads.append(old)
                old.start()
                replacement_container = None
                original_stop = wrapper.stop_container

                async def reuse(identity):
                    nonlocal replacement_container
                    stopped = await original_stop(identity)
                    if identity == old.id and stopped:
                        old.rename(name + "-retired")
                        replacement_container = client.containers.create(
                            "node:24-alpine",
                            name=name,
                            command=command,
                            labels=labels,
                            volumes={volumes[1]: {"bind": "/data", "mode": "rw"}},
                            network_disabled=True,
                        )
                        workloads.append(replacement_container)
                        replacement_container.start()
                    return stopped

                async with sessions() as session:
                    target = BackupTarget(
                        name=f"identity-{kind}",
                        target_type=kind,
                        container_name=name if kind == "container" else None,
                        stack_name=project if kind == "stack" else None,
                        stop_container=True,
                        compression_enabled=False,
                    )
                    session.add(target)
                    await session.flush()
                    backup = Backup(target_id=target.id)
                    session.add(backup)
                    await session.commit()
                    backup_id = backup.id
                engine = BackupEngine()
                with (
                    patch("app.backup_engine.async_session", sessions),
                    patch("app.backup_engine.docker_client", wrapper),
                    patch(
                        "app.backup_engine.settings.BACKUP_BASE_PATH",
                        str(root / "backups"),
                    ),
                    patch.object(wrapper, "stop_container", reuse),
                    patch.object(engine, "_sudo_available", return_value=False),
                ):
                    assert await engine.run_backup(backup_id, skip_validation=True)
                async with sessions() as session:
                    backup = await session.get(Backup, backup_id)
                with tarfile.open(backup.file_path) as tar:
                    assert [
                        tar.extractfile(member).read()
                        for member in tar.getmembers()
                        if member.isfile()
                    ] == [b"owned original"]
                old.reload()
                replacement_container.reload()
                assert old.status == replacement_container.status == "running"
                print(
                    f"PASS R5 {kind}: real stopped/renamed immutable ID retained; replacement running and not archived"
                )
        finally:
            await database.dispose()
            for workload in reversed(workloads):
                workload.reload()
                assert workload.labels.get("dockervault.task") == "t_539de28b"
                workload.remove(force=True)
                try:
                    client.containers.get(workload.id)
                    raise AssertionError("Owned test workload still exists")
                except docker.errors.NotFound:
                    pass
            client.close()
            print(
                "PASS owned workload cleanup verified; no production workload changed"
            )


if __name__ == "__main__":
    asyncio.run(run())
