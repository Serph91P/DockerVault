"""Backup target reference and provider shape validation, shared create/update."""

from app.database import BackupTarget, RetentionPolicy, RemoteStorage, Schedule


async def check_target_references(
    session, *, schedule_id, retention_policy_id, remote_storage_ids
):
    errors = []
    with session.no_autoflush:
        if schedule_id is not None:
            if await session.get(Schedule, schedule_id) is None:
                errors.append(f"schedule_id {schedule_id} does not exist")
        if retention_policy_id is not None:
            if await session.get(RetentionPolicy, retention_policy_id) is None:
                errors.append(
                    f"retention_policy_id {retention_policy_id} does not exist"
                )
        for storage_id in remote_storage_ids or []:
            if await session.get(RemoteStorage, storage_id) is None:
                errors.append(f"remote_storage_id {storage_id} does not exist")
    return errors


async def target_validation_errors(session, target: BackupTarget) -> list[str]:
    """Repair guidance for legacy stored records; never blocks reads or startup."""
    return await check_target_references(
        session,
        schedule_id=target.schedule_id,
        retention_policy_id=target.retention_policy_id,
        remote_storage_ids=target.remote_storage_ids,
    )
