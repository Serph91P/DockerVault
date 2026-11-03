"""R8: recovery must start dependencies before their consumers, including old journals."""

from types import SimpleNamespace
from unittest.mock import patch

import pytest

from app.backup_engine import BackupEngine
from app.database import Backup, BackupTarget, WorkloadOperation
from app.docker_client import DockerClientWrapper
from app.operations import OperationJournal


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["backup", "restore"])
@pytest.mark.parametrize("legacy", [False, True])
async def test_recovery_starts_dependency_first_and_preserves_pre_stopped(
    test_db, kind, legacy
):
    starts = []

    class Container:
        def __init__(self, identity):
            self.id, self.status = identity, "exited"

        def start(self):
            starts.append(self.id)
            if self.id == "app" and containers["db"].status != "running":
                raise RuntimeError("dependency database unavailable")
            self.status = "running"

    containers = {name: Container(name) for name in ("app", "db", "pre-stopped")}
    wrapper = DockerClientWrapper()
    wrapper._client = SimpleNamespace(
        containers=SimpleNamespace(get=containers.__getitem__)
    )
    async with test_db() as session:
        target = BackupTarget(name="owned", target_type="path")
        session.add(target)
        await session.flush()
        backup = Backup(target_id=target.id)
        session.add(backup)
        await session.commit()
        backup_id = backup.id
    journal = OperationJournal(test_db, wrapper)
    operation_id = await journal.begin(
        backup_id, kind, {"app": "running", "db": "running", "pre-stopped": "exited"}
    )
    for name in ("app", "db"):
        await journal.stop(operation_id, name, confirmed=True)
    async with test_db() as session:
        operation = await session.get(WorkloadOperation, operation_id)
        if legacy:
            operation.resources = {
                identity: {k: v for k, v in value.items() if k != "start_index"}
                for identity, value in operation.resources.items()
            }
        else:
            # Recovery uses persisted ranks even if JSON key order changes on disk.
            operation.resources = dict(reversed(list(operation.resources.items())))
        await session.commit()
    with (
        patch("app.backup_engine.async_session", test_db),
        patch("app.backup_engine.docker_client", wrapper),
    ):
        await BackupEngine().recover_operations()
        await BackupEngine().recover_operations()
    assert starts == ["db", "app"]
    assert containers["app"].status == containers["db"].status == "running"
    assert containers["pre-stopped"].status == "exited"
    async with test_db() as session:
        operation = await session.get(WorkloadOperation, operation_id)
        assert operation.state == "finished"
        assert all(value["restored"] for value in operation.resources.values())
