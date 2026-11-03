"""SQLite lifecycle intents; recovery never replays archive or upload effects."""

from datetime import datetime, timezone

from sqlalchemy import select, update

from app.database import Backup, BackupLog, BackupStatus, LogLevel, WorkloadOperation


class OperationJournal:
    def __init__(self, sessions, docker):
        self.sessions = sessions
        self.docker = docker

    async def begin(self, backup_id, kind, original_states, start_order=None):
        if any(state is None for state in original_states.values()):
            raise RuntimeError("Unknown workload state; refusing external changes")
        order = list(
            dict.fromkeys(
                start_order if start_order is not None else reversed(original_states)
            )
        )
        order.extend(identity for identity in original_states if identity not in order)
        ranks = {identity: index for index, identity in enumerate(order)}
        async with self.sessions() as session:
            operation = WorkloadOperation(
                backup_id=backup_id,
                kind=kind,
                resources={
                    identity: {
                        "original": state,
                        "start_index": ranks[identity],
                        "requested": False,
                        "stopped": False,
                        "restored": state != "running",
                    }
                    for identity, state in original_states.items()
                },
            )
            session.add(operation)
            await session.commit()
            return operation.id

    async def phase(self, operation_id, phase):
        async with self.sessions() as session:
            await session.execute(
                update(WorkloadOperation)
                .where(WorkloadOperation.id == operation_id)
                .values(phase=phase)
            )
            await session.commit()

    async def stop(self, operation_id, identity, confirmed=False):
        async with self.sessions() as session:
            operation = await session.get(WorkloadOperation, operation_id)
            resources = dict(operation.resources)
            resources[identity] = {
                **resources[identity],
                "requested": True,
                "stopped": confirmed,
            }
            operation.resources = resources
            operation.phase = "stopped" if confirmed else "stopping"
            await session.commit()

    async def recovered(self, operation_id, failures):
        async with self.sessions() as session:
            operation = await session.get(WorkloadOperation, operation_id)
            operation.resources = {
                identity: {**value, "restored": identity not in failures}
                for identity, value in operation.resources.items()
            }
            await session.commit()

    async def finish(self, operation_id):
        if operation_id is None:
            return
        async with self.sessions() as session:
            operation = await session.get(WorkloadOperation, operation_id)
            outstanding = any(
                value["requested"] and not value["restored"]
                for value in operation.resources.values()
            )
            operation.state = "recovery_required" if outstanding else "finished"
            if outstanding:
                operation.error = (
                    "Workload recovery incomplete; retry reconciliation before new jobs"
                )
            await session.commit()

    async def reconcile(self, restart):
        async with self.sessions() as session:
            operations = (
                await session.scalars(
                    select(WorkloadOperation)
                    .where(WorkloadOperation.state != "finished")
                    .order_by(WorkloadOperation.id)
                )
            ).all()
        errors = []
        for operation in operations:
            failures, ready = [], []
            for identity, value in operation.resources.items():
                if (
                    value["original"] != "running"
                    or not value["requested"]
                    or value["restored"]
                ):
                    continue
                state = await self.docker.get_container_state(identity)
                if state not in ("running", "exited", "created"):
                    failures.append(identity)
                    continue
                if not value["stopped"]:
                    # The process may have died while Docker was still stopping.
                    # Settle that recorded intent before any restart can race it.
                    if state == "running" and not await self.docker.stop_container(
                        identity
                    ):
                        failures.append(identity)
                        continue
                    await self.stop(operation.id, identity, confirmed=True)
                ready.append(identity)
            # Historical journals only persisted stop-order insertion, not start ranks.
            legacy_order = {
                identity: index
                for index, identity in enumerate(reversed(operation.resources))
            }
            ready.sort(
                key=lambda identity: operation.resources[identity].get(
                    "start_index", legacy_order[identity]
                )
            )
            failures.extend(
                await restart(ready, {identity: "running" for identity in ready})
            )
            await self.recovered(operation.id, failures)
            message = (
                "Manual recovery required for workload IDs: " + ", ".join(failures)
                if failures
                else "Interrupted operation reconciled; retry the job explicitly"
            )
            async with self.sessions() as session:
                saved = await session.get(WorkloadOperation, operation.id)
                saved.state = "recovery_required" if failures else "finished"
                saved.error = message
                if operation.backup_id is not None:
                    if operation.kind == "backup":
                        await session.execute(
                            update(Backup)
                            .where(Backup.id == operation.backup_id)
                            .values(
                                status=BackupStatus.FAILED,
                                error_message=message,
                                completed_at=datetime.now(timezone.utc),
                            )
                        )
                    session.add(
                        BackupLog(
                            backup_id=operation.backup_id,
                            level=LogLevel.ERROR,
                            step="recovery",
                            message=message,
                        )
                    )
                await session.commit()
            if failures:
                errors.append(message)
        if errors:
            raise RuntimeError("; ".join(errors))
