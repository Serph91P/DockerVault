"""Explicit clear is different from omission, False and an empty collection."""

import pytest
from app.database import BackupTarget, RetentionPolicy, Schedule
from tests.test_remote_object_ownership import test_engine  # noqa: F401


async def configured(test_db):
    async with test_db() as session:
        policy = RetentionPolicy(
            name="owned-zero", keep_last=0, keep_daily=0, keep_weekly=0, keep_monthly=0
        )
        schedule = Schedule(name="owned", cron_expression="0 2 * * *")
        session.add_all([policy, schedule])
        await session.flush()
        target = BackupTarget(
            name="owned",
            target_type="volume",
            volume_name="owned",
            schedule_id=schedule.id,
            schedule_cron="0 3 * * *",
            retention_policy_id=policy.id,
            pre_backup_command="pg_dump before",
            post_backup_command="redis-cli after",
            include_paths=["old"],
            enabled=True,
        )
        session.add(target)
        await session.commit()
        return target.id


@pytest.mark.asyncio
@pytest.mark.parametrize("clear", [None, ""])
@pytest.mark.parametrize("field", ["pre_backup_command", "post_backup_command"])
async def test_hook_clear_persists_and_other_hook_is_omitted(
    async_client, test_db, field, clear
):
    tid = await configured(test_db)
    updated = await async_client.put(f"/api/v1/targets/{tid}", json={field: clear})
    assert updated.status_code == 200
    assert updated.json()[field] is None
    fetched = (await async_client.get(f"/api/v1/targets/{tid}")).json()
    assert fetched[field] is None
    other = (
        "post_backup_command" if field == "pre_backup_command" else "pre_backup_command"
    )
    assert fetched[other]
    async with test_db() as fresh:
        assert getattr(await fresh.get(BackupTarget, tid), field) is None


@pytest.mark.asyncio
async def test_clears_links_and_legacy_job_and_retains_false_collections(
    async_client, test_db
):
    from app.main import app

    tid = await configured(test_db)
    await app.state.scheduler._load_schedules()
    assert app.state.scheduler.scheduler.get_job(f"backup_target_{tid}")
    updated = await async_client.put(
        f"/api/v1/targets/{tid}",
        json={
            "schedule_id": None,
            "schedule_cron": None,
            "retention_policy_id": None,
            "enabled": False,
            "stop_container": False,
            "compression_enabled": False,
            "include_paths": [],
            "dependencies": [],
        },
    )
    assert updated.status_code == 200
    body = updated.json()
    assert (
        body["schedule_id"]
        is body["schedule"]
        is body["retention_policy_id"]
        is body["retention_policy"]
        is None
    )
    assert (
        body["enabled"]
        is body["stop_container"]
        is body["compression_enabled"]
        is False
    )
    assert body["include_paths"] == body["dependencies"] == []
    assert app.state.scheduler.scheduler.get_job(f"backup_target_{tid}") is None
    app.state.scheduler.scheduler.remove_all_jobs()
    await app.state.scheduler._load_schedules()
    assert app.state.scheduler.scheduler.get_job(f"backup_target_{tid}") is None
    assert (await async_client.get(f"/api/v1/targets/{tid}")).json() == body


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "patch",
    [
        {"name": None},
        {"name": ""},
        {"enabled": None},
        {"dependencies": None},
        {"remote_storage_ids": None},
        {"compression_enabled": None},
    ],
)
async def test_required_field_clears_rejected_atomically(async_client, test_db, patch):
    tid = await configured(test_db)
    before = (await async_client.get(f"/api/v1/targets/{tid}")).json()
    response = await async_client.put(
        f"/api/v1/targets/{tid}", json={**patch, "retention_policy_id": None}
    )
    assert 400 <= response.status_code < 500
    assert (await async_client.get(f"/api/v1/targets/{tid}")).json() == before


@pytest.mark.asyncio
async def test_omission_keeps_links_hooks_and_zero_policy(async_client, test_db):
    tid = await configured(test_db)
    before = (await async_client.get(f"/api/v1/targets/{tid}")).json()
    assert before["retention_policy"]["keep_last"] == 0
    updated = await async_client.put(f"/api/v1/targets/{tid}", json={})
    assert updated.status_code == 200
    for field in [
        "schedule_id",
        "schedule_cron",
        "retention_policy_id",
        "pre_backup_command",
        "post_backup_command",
    ]:
        assert updated.json()[field] == before[field]
