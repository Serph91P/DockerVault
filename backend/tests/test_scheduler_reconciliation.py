"""Committed API/SQLite state must agree with native, paused APScheduler jobs."""

import asyncio
from datetime import datetime, timezone

from apscheduler.schedulers.base import STATE_PAUSED
import pytest
from sqlalchemy import select, text

from app.database import BackupTarget, Schedule
from tests.test_remote_object_ownership import test_engine  # noqa: F401


@pytest.fixture
def live_scheduler(async_client, monkeypatch):
    from app.main import app

    monkeypatch.setattr(async_client._transport, "raise_app_exceptions", False)
    scheduler = app.state.scheduler
    assert scheduler.scheduler.state == STATE_PAUSED
    return scheduler


async def schedules(test_db):
    async with test_db() as session:
        a = Schedule(name="owned-a", cron_expression="0 2 * * *", enabled=True)
        b = Schedule(name="owned-b", cron_expression="0 9 * * *", enabled=True)
        session.add_all([a, b])
        await session.commit()
        return a.id, b.id


async def target_record(test_db, schedule_id, *, legacy=None, enabled=True):
    async with test_db() as session:
        target = BackupTarget(
            name="owned-target",
            target_type="volume",
            volume_name="owned-unexecuted-volume",
            schedule_id=schedule_id,
            schedule_cron=legacy,
            enabled=enabled,
        )
        session.add(target)
        await session.commit()
        return target.id


def job(scheduler, target_id):
    return scheduler.scheduler.get_job(f"backup_target_{target_id}")


def hour(scheduled_job):
    return next(
        str(field) for field in scheduled_job.trigger.fields if field.name == "hour"
    )


@pytest.mark.asyncio
async def test_new_scheduled_target_immediately_registers_one_real_job(
    async_client, test_db, live_scheduler
):
    a, _ = await schedules(test_db)
    response = await async_client.post(
        "/api/v1/targets",
        json={
            "name": "owned-new",
            "target_type": "volume",
            "volume_name": "owned-unexecuted-volume",
            "schedule_id": a,
        },
    )
    assert response.status_code == 200
    tid = response.json()["id"]
    assert job(live_scheduler, tid) is not None
    assert hour(job(live_scheduler, tid)) == "2"
    assert [j.id for j in live_scheduler.scheduler.get_jobs()] == [
        f"backup_target_{tid}"
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "change",
    ["assign", "reassign", "disable", "enable", "clear", "delete", "legacy", "rename"],
)
async def test_target_mutations_reconcile_committed_effective_job(
    async_client, test_db, live_scheduler, change
):
    a, b = await schedules(test_db)
    tid = await target_record(
        test_db,
        None if change in ("assign", "legacy") else a,
        legacy="0 4 * * *" if change in ("assign", "clear", "legacy") else None,
        enabled=change != "enable",
    )
    await live_scheduler._load_schedules()
    previous_due = None
    if change == "rename":
        previous_due = datetime(2000, 1, 1, tzinfo=timezone.utc)
        job(live_scheduler, tid).modify(next_run_time=previous_due)
    updates = {
        "assign": {"schedule_id": a},
        "reassign": {"schedule_id": b},
        "disable": {"enabled": False},
        "enable": {"enabled": True},
        "clear": {"schedule_id": None, "schedule_cron": None},
        "legacy": {"schedule_cron": "0 9 * * *"},
        "rename": {"name": "owned-renamed"},
    }
    response = (
        await async_client.delete(f"/api/v1/targets/{tid}")
        if change == "delete"
        else await async_client.put(f"/api/v1/targets/{tid}", json=updates[change])
    )
    assert response.status_code == 200
    current = job(live_scheduler, tid)
    if change in ("disable", "clear", "delete"):
        assert current is None
    else:
        assert current is not None
        assert hour(current) == ("9" if change in ("reassign", "legacy") else "2")
        assert len(live_scheduler.scheduler.get_jobs()) == 1
        if change == "rename":
            assert current.name == "Backup: owned-renamed"
            assert current.next_run_time == previous_due, (
                "Metadata changes must not drop an already due backup"
            )


@pytest.mark.asyncio
async def test_put_and_get_return_the_same_new_embedded_schedule(
    async_client, test_db, live_scheduler
):
    a, b = await schedules(test_db)
    tid = await target_record(test_db, a)
    updated = await async_client.put(f"/api/v1/targets/{tid}", json={"schedule_id": b})
    fetched = await async_client.get(f"/api/v1/targets/{tid}")
    assert updated.status_code == fetched.status_code == 200
    assert updated.json()["schedule_id"] == b
    assert updated.json()["schedule"] == fetched.json()["schedule"]
    assert updated.json()["schedule"]["id"] == b


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["time", "disable", "enable", "delete", "rename"])
async def test_schedule_mutations_share_effective_reconciliation(
    async_client, test_db, live_scheduler, change
):
    a, _ = await schedules(test_db)
    tid = await target_record(test_db, a, legacy="0 4 * * *")
    if change == "enable":
        async with test_db() as session:
            (await session.get(Schedule, a)).enabled = False
            await session.commit()
    await live_scheduler._load_schedules()
    previous_due = None
    if change == "rename":
        previous_due = datetime(2000, 1, 1, tzinfo=timezone.utc)
        job(live_scheduler, tid).modify(next_run_time=previous_due)
    updates = {
        "time": {"cron_expression": "0 9 * * *"},
        "disable": {"enabled": False},
        "enable": {"enabled": True},
        "rename": {"name": "owned-renamed-schedule"},
    }
    response = (
        await async_client.delete(f"/api/v1/schedules/{a}")
        if change == "delete"
        else await async_client.put(f"/api/v1/schedules/{a}", json=updates[change])
    )
    assert response.status_code == 200
    current = job(live_scheduler, tid)
    if change == "disable":
        assert current is None
    else:
        assert current is not None
        assert hour(current) == (
            "9" if change == "time" else "4" if change == "delete" else "2"
        )
        if change == "rename":
            assert current.next_run_time == previous_due


@pytest.mark.asyncio
@pytest.mark.parametrize("assignment", ["legacy", "shared", "disabled-shared"])
async def test_startup_uses_committed_shared_precedence_without_resave(
    async_client, test_db, live_scheduler, assignment
):
    a, _ = await schedules(test_db)
    if assignment == "disabled-shared":
        async with test_db() as session:
            (await session.get(Schedule, a)).enabled = False
            await session.commit()
    tid = await target_record(
        test_db, None if assignment == "legacy" else a, legacy="0 4 * * *"
    )
    await live_scheduler._load_schedules()
    current = job(live_scheduler, tid)
    if assignment == "disabled-shared":
        assert current is None
    else:
        assert current is not None and hour(current) == (
            "4" if assignment == "legacy" else "2"
        )
    legacy_response = await async_client.get("/api/v1/schedules/legacy/by-target")
    assert legacy_response.status_code == 200
    row = next(row for row in legacy_response.json() if row["target_id"] == tid)
    assert row["enabled"] is (assignment != "disabled-shared")
    assert (row["next_run"] is None) is (assignment == "disabled-shared")


@pytest.mark.asyncio
async def test_failed_schedule_delete_does_not_mutate_live_jobs(
    async_client, test_db, live_scheduler, monkeypatch
):
    a, _ = await schedules(test_db)
    tid = await target_record(test_db, a)
    await live_scheduler._load_schedules()
    async with test_db() as session:
        await session.execute(
            text(
                "CREATE TRIGGER owned_deny_schedule_delete BEFORE DELETE ON schedules BEGIN SELECT RAISE(ABORT,'owned persistence failure'); END"
            )
        )
        await session.commit()
    monkeypatch.setattr(async_client._transport, "raise_app_exceptions", False)
    response = await async_client.delete(f"/api/v1/schedules/{a}")
    assert response.status_code >= 400
    async with test_db() as session:
        assert await session.get(Schedule, a) is not None
        assert (await session.get(BackupTarget, tid)).schedule_id == a
    assert (
        job(live_scheduler, tid) is not None and hour(job(live_scheduler, tid)) == "2"
    )


@pytest.mark.asyncio
async def test_transient_job_store_failure_is_visible_and_retryable_without_resaving(
    async_client, test_db, live_scheduler, monkeypatch
):
    a, _ = await schedules(test_db)
    store = live_scheduler.scheduler._jobstores["default"]
    original = store.add_job
    failed = False

    def fail_once(scheduled_job):
        nonlocal failed
        if not failed:
            failed = True
            raise RuntimeError("Owned transient native job store failure")
        return original(scheduled_job)

    monkeypatch.setattr(store, "add_job", fail_once)
    response = await async_client.post(
        "/api/v1/targets",
        json={
            "name": "owned-retry",
            "target_type": "volume",
            "volume_name": "owned-unexecuted-volume",
            "schedule_id": a,
        },
    )
    assert response.status_code == 503
    assert failed and "saved" in response.json()["detail"].lower()
    async with test_db() as session:
        target = await session.scalar(
            select(BackupTarget).where(BackupTarget.name == "owned-retry")
        )
        assert target is not None and target.schedule_id == a
        tid = target.id
    retried = await async_client.post("/api/v1/schedules/reload")
    assert retried.status_code == 200
    assert (
        job(live_scheduler, tid) is not None and hour(job(live_scheduler, tid)) == "2"
    )
    assert len(live_scheduler.scheduler.get_jobs()) == 1


@pytest.mark.asyncio
async def test_parallel_creates_reconcile_without_holding_request_db_connections(
    async_client, test_db, live_scheduler
):
    a, _ = await schedules(test_db)
    responses = await asyncio.wait_for(
        asyncio.gather(
            *(
                async_client.post(
                    "/api/v1/targets",
                    json={
                        "name": f"owned-parallel-{i}",
                        "target_type": "volume",
                        "volume_name": "owned-unexecuted-volume",
                        "schedule_id": a,
                    },
                )
                for i in range(8)
            )
        ),
        10,
    )
    assert all(response.status_code == 200 for response in responses)
    ids = {response.json()["id"] for response in responses}
    assert len(ids) == len(responses)
    assert {j.id for j in live_scheduler.scheduler.get_jobs()} == {
        f"backup_target_{tid}" for tid in ids
    }
