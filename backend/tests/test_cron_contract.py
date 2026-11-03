"""Preview must describe actual native job fire instants, not another parser.

These parity cases do not choose a new numeric weekday/DOM-DOW policy: the
configured execution trigger is the source of truth. Separate contract tests
will pin the owner-selected syntax and existing-data treatment.
"""

from datetime import datetime, timezone

import pytest

from app.config import settings
from app.scheduler import BackupScheduler


@pytest.mark.parametrize("zone", ["UTC", "Europe/Berlin"])
@pytest.mark.parametrize(
    "expression,base_text",
    [
        ("0 3 * * 1-5", "2026-11-01T00:00:00+00:00"),
        ("0 3 13 * fri", "2026-11-01T00:00:00+00:00"),
        ("0 3 * * sun", "2026-11-01T00:00:00+00:00"),
        ("0 3 * * *", "2026-11-01T00:00:00+00:00"),
        ("30 2 * * *", "2026-03-28T00:00:00+00:00"),
        ("30 2 * * *", "2026-10-24T00:00:00+00:00"),
    ],
)
def test_preview_tracks_configured_native_fire_instants(
    monkeypatch, zone, expression, base_text
):
    monkeypatch.setattr(settings, "TZ", zone)
    scheduler = BackupScheduler()
    trigger = scheduler._trigger(expression)
    assert str(trigger.timezone) == zone
    base = datetime.fromisoformat(base_text)
    for _ in range(6):
        # A preview is the next run strictly after its supplied base instant.
        actual = trigger.get_next_fire_time(base, base)
        assert actual is not None
        preview = scheduler.get_next_run(expression, base)
        assert preview.astimezone(timezone.utc) == actual.astimezone(timezone.utc)
        assert preview.timestamp() > base.timestamp()
        base = actual


@pytest.mark.parametrize(
    "expression", ["@daily", "0 0 * * * *", "0 0 * *", "0 0 * * 7", "0 0 31 2 *"]
)
def test_no_nonregisterable_expression_is_accepted(expression):
    from app.api.targets import validate_cron_expression

    with pytest.raises(ValueError):
        validate_cron_expression(expression)
    with pytest.raises(ValueError):
        BackupScheduler().get_next_run(expression)


def test_preserves_monday_zero_and_dom_dow_and():
    scheduler = BackupScheduler()
    base = datetime(2026, 11, 1, tzinfo=timezone.utc)
    assert scheduler.get_next_run("0 3 * * 0", base).weekday() == 0
    assert scheduler.get_next_run("0 3 * * sun", base).weekday() == 6
    both = scheduler.get_next_run("0 3 13 * fri", base)
    assert both.day == 13 and both.weekday() == 4


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "expression", ["@daily", "0 0 * * * *", "0 0 * *", "0 0 * * 7"]
)
async def test_api_rejects_unsupported_shared_and_legacy_before_persistence(
    async_client, test_db, expression
):
    from sqlalchemy import select, func
    from app.database import Schedule, BackupTarget

    shared = await async_client.post(
        "/api/v1/schedules", json={"name": "invalid", "cron_expression": expression}
    )
    legacy = await async_client.post(
        "/api/v1/targets",
        json={
            "name": "invalid",
            "target_type": "volume",
            "volume_name": "owned",
            "schedule_cron": expression,
        },
    )
    assert 400 <= shared.status_code < 500
    assert 400 <= legacy.status_code < 500
    async with test_db() as session:
        assert await session.scalar(select(func.count()).select_from(Schedule)) == 0
        assert await session.scalar(select(func.count()).select_from(BackupTarget)) == 0


@pytest.mark.asyncio
async def test_preview_endpoint_uses_native_trigger_and_rejects_naive_base(
    async_client,
):
    response = await async_client.post(
        "/api/v1/schedules/preview",
        json={
            "cron_expression": "0 3 13 * fri",
            "base_time": "2026-11-01T00:00:00Z",
            "count": 3,
        },
    )
    assert response.status_code == 200
    body = response.json()
    assert body["timezone"] == settings.TZ
    base = datetime(2026, 11, 1, tzinfo=timezone.utc)
    trigger = BackupScheduler()._trigger("0 3 13 * fri")
    for text in body["next_runs"]:
        native = trigger.get_next_fire_time(base, base)
        assert datetime.fromisoformat(text).timestamp() == native.timestamp()
        base = native
    assert len(body["next_runs"]) == 3
    bad = await async_client.post(
        "/api/v1/schedules/preview",
        json={"cron_expression": "0 3 * * *", "base_time": "2026-11-01T00:00:00"},
    )
    assert bad.status_code == 422


@pytest.mark.asyncio
async def test_existing_inventory_is_visible_and_valid_jobs_unchanged_on_reload(
    async_client, test_db
):
    from app.database import Schedule, BackupTarget
    from app.main import app
    from sqlalchemy import select

    async with test_db() as session:
        shared = Schedule(name="numeric-weekdays", cron_expression="0 3 * * 0-4")
        invalid = Schedule(name="unsupported-old-macro", cron_expression="@daily")
        session.add_all([shared, invalid])
        await session.flush()
        a = BackupTarget(
            name="shared",
            target_type="volume",
            volume_name="owned",
            schedule_id=shared.id,
        )
        b = BackupTarget(
            name="legacy",
            target_type="volume",
            volume_name="owned",
            schedule_cron="0 3 13 * fri",
        )
        session.add_all([a, b])
        await session.commit()
        aid, bid, sid = a.id, b.id, shared.id
    scheduler = app.state.scheduler
    await scheduler._load_schedules()
    original = {
        i: str(scheduler.scheduler.get_job(f"backup_target_{i}").trigger)
        for i in (aid, bid)
    }
    inventory = await async_client.get("/api/v1/schedules/compatibility/inventory")
    assert inventory.status_code == 200
    items = inventory.json()["items"]
    assert len(items) == 3
    assert all(item["warnings"] for item in items)
    assert any("Invalid" in warning for item in items for warning in item["warnings"])
    listing = (await async_client.get("/api/v1/schedules")).json()
    assert all(item["cron_warnings"] for item in listing)
    targets = (await async_client.get("/api/v1/targets")).json()
    assert all(item["cron_warnings"] for item in targets)
    update = await async_client.put(
        f"/api/v1/schedules/{sid}", json={"name": "renamed-only"}
    )
    assert update.status_code == 200
    assert update.json()["cron_warnings"]
    # Rebuild the registry, as startup does, without mutating stored cron strings.
    scheduler.scheduler.remove_all_jobs()
    await scheduler._load_schedules()
    assert original == {
        i: str(scheduler.scheduler.get_job(f"backup_target_{i}").trigger)
        for i in (aid, bid)
    }
    async with test_db() as session:
        assert (await session.get(Schedule, sid)).cron_expression == "0 3 * * 0-4"
        assert (await session.get(BackupTarget, bid)).schedule_cron == "0 3 13 * fri"
