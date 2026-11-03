"""Fresh-process R8 probe. IDs must name only caller-owned task-labelled workloads."""

import asyncio
import json
import os
from pathlib import Path
import sys

root, mode, kind, legacy = Path(sys.argv[1]), *sys.argv[2:5]
os.environ["DATABASE_URL"] = f"sqlite+aiosqlite:///{root / 'catalog.sqlite'}"
os.environ["BACKUP_BASE_PATH"] = str(root / "backups")

from sqlalchemy import select
from app.backup_engine import backup_engine
from app.database import (
    Backup,
    BackupStatus,
    BackupTarget,
    WorkloadOperation,
    async_session,
    engine,
    init_db,
)
from app.docker_client import docker_client
from app.operations import OperationJournal


async def main():
    ids = json.loads(os.environ["OWNED_DEPENDENCY_IDS"])
    assert set(ids) == {"app", "db", "pre-stopped"}
    for identity in ids.values():
        assert (
            docker_client.client.containers.get(identity).labels.get("dockervault.task")
            == "t_539de28b"
        )
    await init_db()
    journal = OperationJournal(async_session, docker_client)
    if mode == "crash":
        async with async_session() as session:
            target = BackupTarget(name="owned-dependencies", target_type="path")
            session.add(target)
            await session.flush()
            backup = Backup(
                target_id=target.id,
                status=BackupStatus.RUNNING
                if kind == "backup"
                else BackupStatus.COMPLETED,
            )
            session.add(backup)
            await session.commit()
            backup_id = backup.id
        states = {
            ids[name]: await docker_client.get_container_state(ids[name])
            for name in ("app", "db", "pre-stopped")
        }
        assert states[ids["app"]] == states[ids["db"]] == "running"
        operation_id = await journal.begin(
            backup_id,
            kind,
            states,
            start_order=[ids[name] for name in ("db", "app", "pre-stopped")],
        )
        if legacy == "legacy":
            async with async_session() as session:
                operation = await session.get(WorkloadOperation, operation_id)
                operation.resources = {
                    identity: {
                        key: value
                        for key, value in data.items()
                        if key != "start_index"
                    }
                    for identity, data in operation.resources.items()
                }
                await session.commit()
        for name in ("app", "db"):
            await journal.stop(operation_id, ids[name])
            assert await docker_client.stop_container(ids[name])
            await journal.stop(operation_id, ids[name], confirmed=True)
        os._exit(81)
    await backup_engine.recover_operations()
    await init_db()
    async with async_session() as session:
        operation = (await session.scalars(select(WorkloadOperation))).one()
        assert operation.state == "finished"
        backup = (await session.scalars(select(Backup))).one()
        assert backup.status == (
            BackupStatus.FAILED if kind == "backup" else BackupStatus.COMPLETED
        )
    # Native app exits without the dependency marker. Check after Node actually boots.
    await asyncio.sleep(1)
    result = {}
    for name, identity in ids.items():
        container = docker_client.client.containers.get(identity)
        assert container.status == ("exited" if name == "pre-stopped" else "running"), (
            f"{name}: {container.status}"
        )
        result[name] = {
            "status": container.status,
            "started": container.attrs["State"]["StartedAt"],
        }
    app = docker_client.client.containers.get(ids["app"])
    assert app.logs().count(b"dependency-ready") == 2
    print(json.dumps(result, sort_keys=True))
    await engine.dispose()
    docker_client.client.close()


if __name__ == "__main__":
    asyncio.run(main())
