"""Overview counts the retained catalog; examples never define a total."""

import asyncio
import hashlib
from datetime import datetime, timedelta, timezone
from unittest.mock import patch
from uuid import uuid4

import pytest
from sqlalchemy import update

from app.backup_engine import BackupEngine
from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
    Schedule,
    StorageOperation,
    User,
)


@pytest.mark.asyncio
async def test_overview_full_catalog_and_owner_scoped_examples(
    async_client, db_session
):
    paused = Schedule(name="Paused", cron_expression="0 1 * * *", enabled=False)
    db_session.add(paused)
    await db_session.flush()
    plans = [
        BackupTarget(name=f"Plan {i}", target_type="path", host_path="/fixture")
        for i in range(507)
    ]
    plans[0].schedule_id = paused.id
    plans[1].schedule_cron = "0 1 * * *"
    plans[2].enabled = False
    plans[3].removed_at = datetime.now(timezone.utc)
    db_session.add_all(plans)
    from app.database import Session

    db_session.add_all(
        [
            User(id=1, username="one", password_hash="synthetic"),
            User(id=2, username="two", password_hash="synthetic"),
            Session(
                user_id=1,
                token=hashlib.sha256(b"test-token").hexdigest(),
                expires_at=datetime.now(timezone.utc) + timedelta(hours=1),
            ),
        ]
    )
    await db_session.flush()
    rows = [
        Backup(
            target_id=plans[0].id,
            status=BackupStatus.RUNNING,
            created_at=datetime(2026, 1, 1),
        )
        for _ in range(507)
    ]
    rows.append(Backup(target_id=plans[1].id, status=BackupStatus.COMPLETED))
    db_session.add_all(rows)
    remote = RemoteStorage(
        name="Disabled location",
        storage_type="local",
        base_path="/fixture",
        enabled=False,
    )
    db_session.add(remote)
    await db_session.flush()
    db_session.add_all(
        [
            BackupStorageSync(
                backup_id=b.id, storage_id=remote.id, sync_status="failed"
            )
            for b in rows
        ]
    )
    for owner in [1, 2]:
        for _ in range(7):
            db_session.add(
                StorageOperation(
                    id=str(uuid4()),
                    user_id=owner,
                    request_id=str(uuid4()),
                    payload={},
                    storage_identity={},
                    action="storage_delete",
                    storage_id="local",
                    path="",
                    total=1,
                    state="running",
                )
            )
    await db_session.commit()
    response = await async_client.get("/api/v1/overview/summary", params={"limit": 3})
    assert response.status_code == 200
    body = response.json()
    assert body["dataset"] == "retained_catalog_and_current_process"
    assert body["plans"] == {
        "total": 506,
        "enabled": 505,
        "disabled": 1,
        "manual": 504,
        "automatic_enabled": 1,
        "automatic_paused": 1,
        "without_recorded_run": {
            "total": 504,
            "enabled": 503,
            "items": [
                {"plan_id": p.id, "name": p.name}
                for p in [plans[2], plans[4], plans[5]]
            ],
        },
        "without_recorded_completed_run": {
            "total": 505,
            "enabled": 504,
            "items": [
                {"plan_id": plans[0].id, "name": plans[0].name},
                {"plan_id": plans[2].id, "name": plans[2].name},
                {"plan_id": plans[4].id, "name": plans[4].name},
            ],
        },
    }
    activity = body["backup_activity"]
    assert (
        activity["recorded_pending"],
        activity["recorded_running"],
        activity["live"],
        activity["total"],
    ) == (0, 507, 0, 507)
    assert [i["backup_id"] for i in activity["items"]] == [
        b.id for b in reversed(rows[:507])
    ][:3]
    assert all(
        i["live_worker"] is False and i["stage"] is None for i in activity["items"]
    )
    assert body["storage_activity"]["total"] == 7
    assert len(body["storage_activity"]["items"]) == 3
    assert body["recorded_copy_failures"]["total"] == 508
    assert body["recorded_copy_failures"]["backups"] == 508
    assert len(body["recorded_copy_failures"]["items"]) == 3
    assert (
        body["recorded_copy_failures"]["items"][0]["storage_name"]
        == "Disabled location"
    )
    before = datetime.fromisoformat(body["window"]["before"])
    after = datetime.fromisoformat(body["window"]["after"])
    assert before == datetime.fromisoformat(body["observed_at"])
    assert before - after == timedelta(hours=24)
    assert before.utcoffset() == timedelta(0)
    assert body["window"]["time_field"] == "created_at"
    for value in [0, 21, "bad"]:
        assert (
            await async_client.get("/api/v1/overview/summary", params={"limit": value})
        ).status_code == 422


@pytest.mark.asyncio
async def test_overview_empty_and_disk_unavailable(
    async_client, test_db, db_session, monkeypatch
):
    from app.database import Session

    db_session.add(User(id=1, username="one", password_hash="synthetic"))
    await db_session.flush()
    db_session.add(
        Session(
            user_id=1,
            token=hashlib.sha256(b"test-token").hexdigest(),
            expires_at=datetime.now(timezone.utc) + timedelta(hours=1),
        )
    )
    await db_session.commit()
    import app.api.settings as settings_api

    monkeypatch.setattr(settings_api, "async_session", test_db)
    response = await async_client.get("/api/v1/overview/summary")
    assert response.status_code == 200
    assert response.json()["plans"]["total"] == 0
    for failure in [False, True]:
        with patch(
            "app.api.settings.shutil.disk_usage",
            side_effect=OSError() if failure else None,
        ) as disk:
            if not failure:
                (
                    disk.return_value.total,
                    disk.return_value.used,
                    disk.return_value.free,
                ) = 10, 8, 2
            info = (await async_client.get("/api/v1/settings/system-info")).json()
        assert info["disk_status"] == ("unavailable" if failure else "available")
        assert info["disk_free"] == (0 if failure else 2)


@pytest.mark.asyncio
@pytest.mark.parametrize("ending", ["success", "error", "cancel"])
async def test_shared_run_observation_survives_early_completed_and_cleans_up(
    test_db, db_session, monkeypatch, ending
):
    import app.backup_engine as engine_module

    monkeypatch.setattr(engine_module, "async_session", test_db)
    engine = BackupEngine()
    plan = BackupTarget(name="Observed", target_type="path", host_path="/fixture")
    db_session.add(plan)
    await db_session.flush()
    backup = Backup(target_id=plan.id)
    db_session.add(backup)
    await db_session.commit()
    reached, release = asyncio.Event(), asyncio.Event()

    async def inner(id, skip):
        async with test_db() as session:
            await session.execute(
                update(Backup)
                .where(Backup.id == id)
                .values(status=BackupStatus.COMPLETED)
            )
            await session.commit()
        await engine._log(id, "upload", "synthetic held upload")
        reached.set()
        await release.wait()
        if ending == "error":
            raise RuntimeError("synthetic error")
        return True

    monkeypatch.setattr(engine, "_run_backup_inner", inner)
    task = asyncio.create_task(engine.run_backup(backup.id))
    await asyncio.wait_for(reached.wait(), 3)
    try:
        assert engine.observed_backups() == {backup.id: "upload"}
        assert (
            BackupEngine().observed_backups() == {}
        )  # Another engine instance has no live evidence.
        if ending == "cancel":
            task.cancel()
        else:
            release.set()
        if ending == "success":
            assert await task is True
        else:
            with pytest.raises(
                asyncio.CancelledError if ending == "cancel" else RuntimeError
            ):
                await task
        assert engine.observed_backups() == {}
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
async def test_overview_missing_storage_live_union_auth_and_exact_history_window(
    async_client, test_db, db_session, monkeypatch
):
    from app import auth
    from app.api import overview
    from sqlalchemy import text

    db_session.add_all(
        [
            User(id=1, username="first", password_hash="unused"),
            User(id=2, username="second", password_hash="unused"),
        ]
    )
    await db_session.commit()
    token = await auth.create_session(1, db_session)
    second = await auth.create_session(2, db_session)
    async_client.cookies.set("session_token", token)
    plan = BackupTarget(
        name="Retained catalog only", target_type="path", host_path="/fixture"
    )
    db_session.add(plan)
    await db_session.flush()
    stamp = datetime(2026, 11, 8, 12, tzinfo=timezone.utc)

    class Clock:
        @staticmethod
        def now(tz):
            return stamp

    monkeypatch.setattr(overview, "datetime", Clock)
    rows = [
        Backup(target_id=plan.id, status=BackupStatus.COMPLETED, created_at=stamp)
        for _ in range(7)
    ]
    failed = [
        Backup(target_id=plan.id, status=BackupStatus.FAILED, created_at=value)
        for value in [
            stamp - timedelta(days=1, microseconds=1),
            stamp - timedelta(days=1),
            stamp,
            stamp + timedelta(microseconds=1),
        ]
    ]
    db_session.add_all(rows + failed)
    await db_session.flush()
    await db_session.commit()
    await db_session.execute(text("PRAGMA foreign_keys=OFF"))
    # Legacy missing association; no inner join may hide this failed sync.
    db_session.add(
        BackupStorageSync(backup_id=rows[0].id, storage_id=777, sync_status="failed")
    )
    await db_session.commit()
    await db_session.execute(text("PRAGMA foreign_keys=ON"))
    await db_session.commit()
    monkeypatch.setattr(
        overview.backup_engine,
        "observed_backups",
        lambda: {b.id: "upload" for b in rows},
    )
    result = (
        await async_client.get("/api/v1/overview/summary?limit=2&user_id=2")
    ).json()
    assert result["backup_activity"]["live"] == result["backup_activity"]["total"] == 7
    assert result["backup_activity"]["recorded_running"] == 0
    assert [item["backup_id"] for item in result["backup_activity"]["items"]] == [
        rows[6].id,
        rows[5].id,
    ]
    assert all(
        item["live_worker"]
        and item["catalog_status"] == "completed"
        and item["stage"] == "upload"
        for item in result["backup_activity"]["items"]
    )
    assert result["recorded_copy_failures"]["items"] == [
        {
            "backup_id": rows[0].id,
            "storage_id": 777,
            "storage_name": None,
            "status": "failed",
        }
    ]
    history = (
        await async_client.get(
            "/api/v1/backups/history",
            params={
                "view": "runs",
                "status": "failed",
                "after": result["window"]["after"],
                "before": result["window"]["before"],
            },
        )
    ).json()
    assert history["total"] == 2
    assert [item["id"] for item in history["items"]] == [failed[2].id, failed[1].id]
    # A different authenticated owner cannot select the first owner's jobs via query parameters.
    async_client.cookies.set("session_token", second)
    assert (await async_client.get("/api/v1/overview/summary?user_id=1")).json()[
        "storage_activity"
    ]["total"] == 0
    async_client.cookies.clear()
    assert (await async_client.get("/api/v1/overview/summary")).status_code == 401
    async_client.cookies.set("session_token", token)
    # Losing retained evidence returns to 'without recorded', not lifetime never-run.
    await db_session.execute(text("DELETE FROM backup_storage_syncs"))
    await db_session.execute(text("DELETE FROM backups"))
    await db_session.commit()
    monkeypatch.setattr(overview.backup_engine, "observed_backups", lambda: {})
    after = (await async_client.get("/api/v1/overview/summary")).json()
    assert after["plans"]["without_recorded_run"]["total"] == 1
    assert after["plans"]["without_recorded_completed_run"]["total"] == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("caller", ["direct", "batch", "queue"])
async def test_real_engine_activity_covers_all_entrypoints_and_upload(
    async_client, test_db, db_session, monkeypatch, tmp_path, caller
):
    from app import backup_engine as engine_module
    from app.remote_storage import storage_manager
    from sqlalchemy import select
    import subprocess
    import sys

    source = tmp_path / "source"
    source.mkdir()
    (source / "nested").mkdir()
    payload = b"actual observed archive bytes"
    (source / "nested/payload").write_bytes(payload)
    location = RemoteStorage(
        name="Physical test copy",
        storage_type="local",
        base_path=str(tmp_path / "remote"),
        enabled=True,
    )
    db_session.add(location)
    await db_session.flush()
    plan = BackupTarget(
        name="Observed real engine",
        target_type="path",
        host_path=str(source),
        remote_storage_ids=[location.id],
    )
    db_session.add(plan)
    await db_session.commit()
    engine = BackupEngine()
    monkeypatch.setattr(engine_module, "async_session", test_db)
    monkeypatch.setattr(
        engine_module.settings, "BACKUP_BASE_PATH", str(tmp_path / "backups")
    )
    monkeypatch.setattr(engine, "_sudo_available", lambda: False)
    monkeypatch.setattr(storage_manager, "backends", {})
    monkeypatch.setattr(storage_manager, "configs", {})
    reached, release = asyncio.Event(), asyncio.Event()
    original = engine._sync_to_remote

    async def held(*args, **kwargs):
        reached.set()
        await release.wait()
        return await original(*args, **kwargs)

    monkeypatch.setattr(engine, "_sync_to_remote", held)
    row = await engine.create_backup(plan)
    task = None
    try:
        if caller == "queue":
            await engine.start_queue_worker()
            await engine.enqueue(row.id)
        else:
            task = asyncio.create_task(
                engine.run_batch_sequential([row.id])
                if caller == "batch"
                else engine.run_backup(row.id)
            )
        await asyncio.wait_for(reached.wait(), 10)
        async with test_db() as db:
            current = await db.get(Backup, row.id)
            assert current.status == BackupStatus.COMPLETED
        assert engine.observed_backups() == {row.id: "upload"}
        if caller == "direct":
            child = subprocess.run(
                [
                    sys.executable,
                    "-c",
                    'from app.backup_engine import backup_engine; assert backup_engine.observed_backups() == {}; print("new process has no live observation")',
                ],
                capture_output=True,
                text=True,
                check=True,
            )
            assert child.stdout.strip().endswith("new process has no live observation")
    finally:
        release.set()
        if task:
            await asyncio.wait_for(task, 20)
        await engine.shutdown()
    assert engine.observed_backups() == {}
    async with test_db() as db:
        stored = await db.get(Backup, row.id)
        sync = (
            await db.execute(
                select(BackupStorageSync).where(BackupStorageSync.backup_id == row.id)
            )
        ).scalar_one()
        from pathlib import Path
        import tarfile

        local = Path(stored.file_path)
        remote = tmp_path / "remote" / plan.name / local.name
        assert (
            stored.status == BackupStatus.COMPLETED and sync.sync_status == "completed"
        )
        assert remote.read_bytes() == local.read_bytes()
        with tarfile.open(local) as archive:
            member = next(
                m for m in archive.getmembers() if m.name.endswith("nested/payload")
            )
            assert archive.extractfile(member).read() == payload


@pytest.mark.asyncio
async def test_database_failure_is_not_a_successful_zero_summary(
    async_client, monkeypatch
):
    from app.api import overview
    from contextlib import asynccontextmanager
    from fastapi import HTTPException
    from app.auth import get_current_user
    from app.main import app

    @asynccontextmanager
    async def unavailable():
        raise HTTPException(503, "Synthetic database unavailable")
        yield

    async def user():
        return User(id=1, username="fixture", password_hash="unused")

    app.dependency_overrides[get_current_user] = user
    monkeypatch.setattr(overview.database, "async_session", unavailable)
    try:
        response = await async_client.get("/api/v1/overview/summary")
        assert response.status_code == 503
        assert "plans" not in response.json()
    finally:
        app.dependency_overrides.pop(get_current_user, None)
