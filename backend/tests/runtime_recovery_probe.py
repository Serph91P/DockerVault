"""Child process for crash/restart contracts; owns every path under argv root."""

import asyncio
import json
import os
from pathlib import Path
import sys
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

root = Path(sys.argv[1]).resolve()
mode, phase = sys.argv[2:4]
os.environ["DATABASE_URL"] = f"sqlite+aiosqlite:///{root / 'backup.db'}"
os.environ["BACKUP_BASE_PATH"] = str(root / "backups")

from sqlalchemy import select
from app.backup_engine import backup_engine
from app.database import (
    Backup,
    BackupTarget,
    RemoteStorage,
    async_session,
    engine,
    init_db,
)
from app.docker_client import DockerClientWrapper


state_file = root / "workload.json"
real_ids = json.loads(os.environ.get("DV_RECOVERY_DOCKER_IDS", "{}"))
real_docker = None
if real_ids:
    import docker

    real_docker = docker.from_env()
    assert set(real_ids) == {"running", "pre-stopped"}
    for identity in real_ids.values():
        assert (
            real_docker.containers.get(identity).labels.get("dockervault.task")
            == "t_539de28b"
        )


class DurableContainer:
    def __init__(self, name):
        self.name = name
        self.id = real_ids.get(name, "owned-id-" + name)

    @property
    def status(self):
        if real_docker:
            return real_docker.containers.get(self.id).status
        return json.loads(state_file.read_text())[self.name]["state"]

    def change(self, action, state):
        if real_docker:
            container = real_docker.containers.get(self.id)
            if action == "stops":
                container.stop(timeout=30)
            else:
                container.start()
        data = json.loads(state_file.read_text())
        data[self.name][action] += 1
        data[self.name]["state"] = state
        temporary = state_file.with_suffix(".tmp")
        temporary.write_text(json.dumps(data))
        temporary.replace(state_file)

    def stop(self, timeout):
        if mode == "crash" and phase == "intent":
            os._exit(81)
        self.change("stops", "exited")
        if mode == "crash" and phase == "stop":
            os._exit(81)

    def start(self):
        self.change("starts", "running")
        if mode == "crash" and phase == "restart":
            os._exit(81)


containers = {name: DurableContainer(name) for name in ("running", "pre-stopped")}
registry = {**containers, **{c.id: c for c in containers.values()}}
wrapper = DockerClientWrapper()
wrapper._client = SimpleNamespace(containers=SimpleNamespace(get=registry.__getitem__))


async def main():
    with (
        patch("app.backup_engine.docker_client", wrapper),
        patch.object(
            wrapper, "get_dependency_order", AsyncMock(side_effect=lambda names: names)
        ),
        patch.object(backup_engine, "_sudo_available", return_value=False),
    ):
        if mode == "crash":
            await init_db()
            state_file.write_text(
                json.dumps(
                    {
                        "running": {"state": "running", "stops": 0, "starts": 0},
                        "pre-stopped": {"state": "exited", "stops": 0, "starts": 0},
                    }
                )
            )
            source = root / "source"
            source.mkdir()
            (source / "payload").write_bytes(b"recoverable synthetic bytes")
            async with async_session() as session:
                session.add(
                    RemoteStorage(
                        id=1,
                        name="owned-local",
                        storage_type="local",
                        base_path=str(root / "remote"),
                        enabled=True,
                    )
                )
                target = BackupTarget(
                    name="crash-target",
                    target_type="path",
                    host_path=str(source),
                    container_name="running",
                    stop_container=True,
                    dependencies=["pre-stopped"],
                    remote_storage_ids=[1],
                )
                session.add(target)
                await session.flush()
                backups = [Backup(target_id=target.id), Backup(target_id=target.id)]
                session.add_all(backups)
                await session.commit()
            original_archive = backup_engine._create_backup_archive
            original_upload = backup_engine._sync_to_remote
            original_progress = backup_engine._notify_progress

            async def archive(*args, **kwargs):
                result = await original_archive(*args, **kwargs)
                if phase == "archive":
                    os._exit(81)
                return result

            async def upload(*args):
                await original_upload(*args)
                if phase == "upload":
                    os._exit(81)

            async def progress(backup_id, percent, message):
                if phase == "terminal" and percent == 90:
                    os._exit(81)
                await original_progress(backup_id, percent, message)

            with (
                patch.object(
                    backup_engine, "_create_backup_archive", side_effect=archive
                ),
                patch.object(backup_engine, "_sync_to_remote", side_effect=upload),
                patch.object(backup_engine, "_notify_progress", side_effect=progress),
            ):
                await backup_engine.enqueue(backups[1].id)
                await backup_engine.run_backup(backups[0].id, skip_validation=True)
            raise AssertionError("crash boundary not reached")
        else:
            from app.main import app, lifespan

            # Exercise actual startup; periodic scheduler and auth-key chmod are
            # unrelated boundaries and cannot touch host configuration in this probe.
            with (
                patch("app.main._enforce_key_file_permissions"),
                patch("app.main.BackupScheduler") as scheduler,
            ):
                scheduler.return_value.start = AsyncMock()
                scheduler.return_value.stop = AsyncMock()
                async with lifespan(app):
                    pass
            async with async_session() as session:
                backups = (
                    await session.scalars(select(Backup).order_by(Backup.id))
                ).all()
                workloads = json.loads(state_file.read_text())
                for name in workloads:
                    workloads[name]["state"] = containers[name].status
                result = {
                    "workloads": workloads,
                    "statuses": [b.status.value for b in backups],
                    "errors": [b.error_message for b in backups],
                    "queue": backup_engine._queue.qsize(),
                    "remote_files": sorted(
                        str(p.relative_to(root))
                        for p in (root / "remote").rglob("*")
                        if p.is_file()
                    ),
                }
            print(json.dumps(result))
    await engine.dispose()
    if real_docker:
        real_docker.close()


asyncio.run(main())
