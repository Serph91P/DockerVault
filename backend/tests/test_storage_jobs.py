"""Durable storage jobs against file-backed SQLite and physical filesystem effects."""

import asyncio
import uuid


import pytest
import pytest_asyncio
from httpx import ASGITransport, AsyncClient
from sqlalchemy import func, select, text
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app import database
from app.config import settings
from app.database import User


@pytest_asyncio.fixture
async def jobs_env(tmp_path, monkeypatch):
    from app import auth, main, storage_jobs

    root = tmp_path / "root"
    root.mkdir()
    engine = create_async_engine(f"sqlite+aiosqlite:///{tmp_path}/jobs.sqlite")
    sessions = async_sessionmaker(
        engine, class_=database.CancellationSafeAsyncSession, expire_on_commit=False
    )
    monkeypatch.setattr(database, "engine", engine)
    monkeypatch.setattr(database, "async_session", sessions)
    monkeypatch.setattr(auth, "async_session", sessions)
    monkeypatch.setattr(main, "async_session", sessions)
    monkeypatch.setattr(settings, "BACKUP_BASE_PATH", str(root))
    await database.init_db()
    async with sessions() as db:
        db.add_all(
            [
                User(id=1, username="first", password_hash="unused"),
                User(id=2, username="second", password_hash="unused"),
            ]
        )
        await db.commit()
        first = await auth.create_session(1, db)
        second = await auth.create_session(2, db)
    jobs = storage_jobs.StorageJobs(sessions)
    monkeypatch.setattr(storage_jobs, "storage_jobs", jobs)
    async with AsyncClient(
        transport=ASGITransport(app=main.app),
        base_url="http://test",
        cookies={"session_token": first},
    ) as client:
        yield jobs, sessions, root, client, second
    await jobs.shutdown()
    await engine.dispose()


def payload(paths, **kwargs):
    return dict(
        action="storage_delete",
        storage_id="local",
        path="",
        paths=paths,
        request_id=str(uuid.uuid4()),
        **kwargs,
    )


async def terminal(client, id):
    async with asyncio.timeout(5):
        while True:
            response = await client.get(f"/api/v1/operations/{id}")
            assert response.status_code == 200, response.text
            detail = response.json()
            if detail["state"] not in ("queued", "running"):
                return detail
            await asyncio.sleep(0.01)


@pytest.mark.asyncio
async def test_acceptance_is_committed_before_effect_and_idempotent(jobs_env):
    jobs, sessions, root, client, _ = jobs_env
    (root / "nested").mkdir()
    (root / "nested" / "file").write_bytes(b"content")
    request = payload(["nested", "nested/"])
    response = await client.post("/api/v1/operations", json=request)
    assert response.status_code == 202, response.text
    id = response.json()["operation_id"]
    assert (root / "nested" / "file").read_bytes() == b"content"
    async with sessions() as db:
        row = await db.get(database.StorageOperation, id)
        assert row.state == "queued"
    assert (await client.post("/api/v1/operations", json=request)).json() == {
        "operation_id": id
    }
    changed = {**request, "paths": ["different"]}
    assert (await client.post("/api/v1/operations", json=changed)).status_code == 409
    await jobs.start()
    detail = await terminal(client, id)
    assert [
        detail[k]
        for k in ("state", "total", "completed", "succeeded", "failed", "unknown")
    ] == ["completed", 1, 1, 1, 0, 0]
    assert not (root / "nested").exists()
    (root / "nested").mkdir()
    assert (await client.post("/api/v1/operations", json=request)).json() == {
        "operation_id": id
    }
    await asyncio.sleep(0.03)
    assert (root / "nested").exists()  # no second physical deletion


@pytest.mark.asyncio
async def test_mixed_results_and_failed_only_retry_preserve_original(jobs_env):
    jobs, _, root, client, _ = jobs_env
    (root / "good").write_text("original")
    request = payload(["good", "missing", "../outside"])
    response = await client.post("/api/v1/operations", json=request)
    assert response.status_code == 202, response.text
    id = response.json()["operation_id"]
    await jobs.start()
    original = await terminal(client, id)
    assert [original[k] for k in ("state", "completed", "succeeded", "failed")] == [
        "failed",
        3,
        1,
        2,
    ]
    assert [i["state"] for i in original["items"]] == ["succeeded", "failed", "failed"]
    (root / "good").write_text("new - must survive retry")
    (root / "missing").write_text("now available")
    response = await client.post(
        f"/api/v1/operations/{id}/retry", json={"request_id": str(uuid.uuid4())}
    )
    assert response.status_code == 202, response.text
    retried = await terminal(client, response.json()["operation_id"])
    assert retried["parent_id"] == id
    assert [i["path"] for i in retried["items"]] == ["missing", "../outside"]
    assert retried["succeeded"] == 1
    assert (root / "good").read_text() == "new - must survive retry"
    assert (await client.get(f"/api/v1/operations/{id}")).json() == original


@pytest.mark.asyncio
async def test_authorization_and_list_do_not_expose_other_users(jobs_env):
    _, _, _, client, second = jobs_env
    response = await client.post("/api/v1/operations", json=payload(["one"]))
    assert response.status_code == 202, response.text
    id = response.json()["operation_id"]
    assert len((await client.get("/api/v1/operations?storage_id=local")).json()) == 1
    client.cookies.set("session_token", second)
    assert (await client.get("/api/v1/operations")).json() == []
    for suffix in ("", "/cancel", "/retry"):
        response = await (
            client.get(f"/api/v1/operations/{id}")
            if not suffix
            else client.post(
                f"/api/v1/operations/{id}{suffix}",
                json={"request_id": str(uuid.uuid4())},
            )
        )
        assert response.status_code == 404
    client.cookies.clear()
    assert (await client.get("/api/v1/operations")).status_code == 401


@pytest.mark.asyncio
async def test_cancel_settles_physical_item_before_skipping_future_work(
    jobs_env, monkeypatch
):
    from app import storage_operations
    import threading

    jobs, _, root, client, _ = jobs_env
    entered, release = threading.Event(), threading.Event()
    real_delete = storage_operations.delete_local_target

    def held(*args, **kwargs):
        entered.set()
        assert release.wait(5)
        return real_delete(*args, **kwargs)

    monkeypatch.setattr(storage_operations, "delete_local_target", held)
    for name in ("one", "two"):
        (root / name).write_text(name)
    response = await client.post("/api/v1/operations", json=payload(["one", "two"]))
    assert response.status_code == 202, response.text
    id = response.json()["operation_id"]
    await jobs.start()
    assert await asyncio.to_thread(entered.wait, 3)
    try:
        response = await client.post(f"/api/v1/operations/{id}/cancel")
        assert response.status_code == 200
        detail = (await client.get(f"/api/v1/operations/{id}")).json()
        assert detail["cancel_requested"] and detail["completed"] == 0
        assert detail["state"] == "running"
    finally:
        release.set()
    detail = await terminal(client, id)
    assert [detail[k] for k in ("state", "completed", "succeeded", "skipped")] == [
        "cancelled",
        2,
        1,
        1,
    ]
    assert not (root / "one").exists() and (root / "two").exists()
    await client.post(f"/api/v1/operations/{id}/cancel")
    assert (await client.get(f"/api/v1/operations/{id}")).json() == detail
    assert (
        await client.post(
            f"/api/v1/operations/{id}/retry", json={"request_id": str(uuid.uuid4())}
        )
    ).status_code == 409


@pytest.mark.asyncio
async def test_restart_unknown_and_pending_are_terminal_without_replay(jobs_env):
    jobs, sessions, root, client, _ = jobs_env
    (root / "one").write_text("preserve")
    response = await client.post("/api/v1/operations", json=payload(["one", "two"]))
    assert response.status_code == 202, response.text
    id = response.json()["operation_id"]
    async with sessions() as db:
        row = await db.get(database.StorageOperation, id)
        row.state = "running"
        item = (
            (
                await db.execute(
                    select(database.StorageOperationItem)
                    .where(database.StorageOperationItem.operation_id == id)
                    .order_by(database.StorageOperationItem.id)
                )
            )
            .scalars()
            .first()
        )
        item.state = "running"
        await db.commit()
    await jobs.recover()
    await jobs.start()
    detail = await terminal(client, id)
    assert [detail[k] for k in ("state", "completed", "unknown", "skipped")] == [
        "failed",
        2,
        1,
        1,
    ]
    assert [i["state"] for i in detail["items"]] == ["unknown", "skipped"]
    assert (root / "one").read_text() == "preserve"
    assert (
        await client.post(
            f"/api/v1/operations/{id}/retry", json={"request_id": str(uuid.uuid4())}
        )
    ).status_code == 409


@pytest.mark.asyncio
async def test_changed_root_cannot_redirect_accepted_job(jobs_env):
    jobs, _, root, client, _ = jobs_env
    (root / "one").write_text("old")
    response = await client.post("/api/v1/operations", json=payload(["one"]))
    assert response.status_code == 202, response.text
    root.rename(root.with_name("old"))
    root.mkdir()
    (root / "one").write_text("replacement")
    await jobs.start()
    detail = await terminal(client, response.json()["operation_id"])
    assert detail["state"] == "failed" and detail["failed"] == 1
    assert (root / "one").read_text() == "replacement"
    assert (root.with_name("old") / "one").read_text() == "old"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "paths", [[], [""], ["x"] * 10001, [f"x{i}" for i in range(1001)]]
)
async def test_bounded_nonempty_admission(jobs_env, paths):
    _, _, _, client, _ = jobs_env
    assert (
        await client.post("/api/v1/operations", json=payload(paths))
    ).status_code == 422


@pytest.mark.asyncio
async def test_additive_tables_exist_in_migration_ledger_and_repeat_init(jobs_env):
    _, sessions, _, _, _ = jobs_env
    await database.init_db()
    async with sessions() as db:
        assert (
            await db.execute(
                text("SELECT version FROM schema_migrations ORDER BY version")
            )
        ).scalars().all() == [1, 2, 3, 4, 5, 6, 7]
        assert (
            await db.execute(text("SELECT count(*) FROM storage_operations"))
        ).scalar_one() == 0
        assert (await db.execute(text("PRAGMA foreign_key_check"))).all() == []


@pytest.mark.asyncio
async def test_configured_storage_list_and_mutated_reused_identity(jobs_env):
    jobs, sessions, root, client, _ = jobs_env
    replacement = root.parent / "replacement"
    replacement.mkdir()
    for folder in (root, replacement):
        (folder / "one").write_text("preserve")
    async with sessions() as db:
        db.add(
            database.RemoteStorage(
                id=7,
                name="configured",
                storage_type="local",
                base_path=str(root),
                enabled=True,
            )
        )
        await db.commit()
    response = await client.post(
        "/api/v1/operations", json={**payload(["one"]), "storage_id": 7}
    )
    assert response.status_code == 202, response.text
    listed = await client.get("/api/v1/operations?storage_id=7")
    assert listed.status_code == 200, listed.text
    assert len(listed.json()) == 1
    async with sessions() as db:
        row = await db.get(database.RemoteStorage, 7)
        await db.delete(row)
        await db.commit()
        db.add(
            database.RemoteStorage(
                id=7,
                name="replacement",
                storage_type="local",
                base_path=str(replacement),
                enabled=True,
            )
        )
        await db.commit()
    await jobs.start()
    detail = await terminal(client, response.json()["operation_id"])
    assert detail["state"] == "failed" and detail["failed"] == 1
    assert (root / "one").exists() and (replacement / "one").exists()
    assert (
        await client.post(
            f"/api/v1/operations/{detail['id']}/retry",
            json={"request_id": str(uuid.uuid4())},
        )
    ).status_code == 409


@pytest.mark.asyncio
async def test_concurrent_idempotent_admission_and_capacity(jobs_env):
    jobs, sessions, _, client, _ = jobs_env
    jobs.capacity = 1
    request = payload(["one"])
    responses = await asyncio.gather(
        *(client.post("/api/v1/operations", json=request) for _ in range(5))
    )
    assert all(response.status_code == 202 for response in responses)
    assert len({response.json()["operation_id"] for response in responses}) == 1
    assert (
        await client.post("/api/v1/operations", json=payload(["two"]))
    ).status_code == 429
    async with sessions() as db:
        assert (
            await db.scalar(select(func.count()).select_from(database.StorageOperation))
            == 1
        )


@pytest.mark.asyncio
async def test_serial_conflicting_jobs_and_shutdown_join_physical_effect(
    jobs_env, monkeypatch
):
    import threading
    from app import storage_operations

    jobs, _, root, client, _ = jobs_env
    entered, release = threading.Event(), threading.Event()
    real_delete = storage_operations.delete_local_target

    def held(*args, **kwargs):
        entered.set()
        assert release.wait(5)
        return real_delete(*args, **kwargs)

    monkeypatch.setattr(storage_operations, "delete_local_target", held)
    (root / "one").write_text("one")
    ids = []
    for _ in range(2):
        response = await client.post("/api/v1/operations", json=payload(["one"]))
        assert response.status_code == 202
        ids.append(response.json()["operation_id"])
    await jobs.start()
    assert await asyncio.to_thread(entered.wait, 3)
    shutdown = asyncio.create_task(jobs.shutdown())
    await asyncio.sleep(0.05)
    try:
        assert not shutdown.done()
        details = [(await client.get(f"/api/v1/operations/{id}")).json() for id in ids]
        assert sorted(d["state"] for d in details) == ["queued", "running"]
        assert sum(d["completed"] for d in details) == 0
    finally:
        release.set()
    await shutdown
    details = [(await client.get(f"/api/v1/operations/{id}")).json() for id in ids]
    assert sum(d["succeeded"] for d in details) == 1
    assert not (root / "one").exists()


@pytest.mark.asyncio
async def test_root_replaced_after_worker_check_is_not_followed(jobs_env, monkeypatch):
    from app import storage_operations

    jobs, _, root, client, _ = jobs_env
    real_delete = storage_operations.delete_local_target

    def replace_then_delete(*args, **kwargs):
        root.rename(root.with_name("pinned-old"))
        root.mkdir()
        (root / "one").write_text("replacement")
        return real_delete(*args, **kwargs)

    monkeypatch.setattr(storage_operations, "delete_local_target", replace_then_delete)
    (root / "one").write_text("original")
    response = await client.post("/api/v1/operations", json=payload(["one"]))
    await jobs.start()
    detail = await terminal(client, response.json()["operation_id"])
    assert detail["state"] == "failed"
    assert (root / "one").read_text() == "replacement"
    assert (root.with_name("pinned-old") / "one").read_text() == "original"


@pytest.mark.asyncio
async def test_accepted_root_inode_is_pinned_until_terminal_to_prevent_reuse(jobs_env):
    import os
    from pathlib import Path

    jobs, _, root, client, _ = jobs_env
    identity = root.stat().st_dev, root.stat().st_ino

    def open_references():
        count = 0
        for descriptor in Path("/proc/self/fd").iterdir():
            try:
                info = os.fstat(int(descriptor.name))
            except OSError:
                continue
            count += (info.st_dev, info.st_ino) == identity
        return count

    before = open_references()
    response = await client.post("/api/v1/operations", json=payload(["one"]))
    assert response.status_code == 202
    assert open_references() > before
    root.rmdir()
    root.mkdir()
    (root / "one").write_text("replacement must survive")
    await jobs.start()
    detail = await terminal(client, response.json()["operation_id"])
    assert detail["state"] == "failed"
    assert (root / "one").read_text() == "replacement must survive"
    assert open_references() == before


@pytest.mark.asyncio
async def test_detail_snapshot_counters_cannot_race_its_item_outcomes(
    jobs_env, monkeypatch
):
    jobs, _, root, client, _ = jobs_env
    (root / "one").write_text("one")
    response = await client.post("/api/v1/operations", json=payload(["one"]))
    id = response.json()["operation_id"]
    entered, release, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()
    real_items, real_notify = jobs._items, jobs._notify

    async def held_items(db, job_id):
        if asyncio.current_task().get_name() == "held-snapshot":
            entered.set()
            await release.wait()
        return await real_items(db, job_id)

    async def notified(job_id, owner):
        await real_notify(job_id, owner)
        async with jobs.sessions() as db:
            if (await db.get(database.StorageOperation, job_id)).state == "completed":
                finished.set()

    monkeypatch.setattr(jobs, "_items", held_items)
    monkeypatch.setattr(jobs, "_notify", notified)
    snapshot = asyncio.create_task(jobs.detail(1, id), name="held-snapshot")
    await entered.wait()
    await jobs.start()
    try:
        # Deadline only releases the controlled interleaving if reads block writes.
        await asyncio.wait_for(finished.wait(), 0.3)
    except TimeoutError:
        pass
    finally:
        release.set()
    detail = await snapshot
    assert detail["completed"] == sum(
        i["state"] in ("succeeded", "failed", "skipped", "unknown")
        for i in detail["items"]
    )
    assert (await terminal(client, id))["succeeded"] == 1


@pytest.mark.asyncio
async def test_version_seven_creation_is_atomic_and_preserves_previous_schema(
    jobs_env, monkeypatch
):
    jobs, sessions, _, _, _ = jobs_env
    async with database.engine.begin() as conn:
        await conn.execute(text("DROP TABLE storage_operation_items"))
        await conn.execute(text("DROP TABLE storage_operations"))
        await conn.execute(text("DELETE FROM schema_migrations WHERE version=7"))
    real_migrate = database._create_storage_operations

    async def interrupted(conn):
        await real_migrate(conn)
        raise RuntimeError("controlled version seven interruption")

    monkeypatch.setattr(database, "_create_storage_operations", interrupted)
    with pytest.raises(RuntimeError, match="controlled version seven"):
        await database.run_migrations()
    async with sessions() as db:
        assert await db.scalar(text("SELECT MAX(version) FROM schema_migrations")) == 6
        assert (
            await db.scalar(
                text(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'storage_operation%'"
                )
            )
            == 0
        )
        assert await db.scalar(select(func.count()).select_from(User)) == 2
    monkeypatch.setattr(database, "_create_storage_operations", real_migrate)
    await database.run_migrations()
    await database.run_migrations()
    assert await jobs.list(1) == []


@pytest.mark.asyncio
async def test_worker_read_failure_closes_admission_and_resolves_pending_without_replay(
    jobs_env,
):
    from sqlalchemy import event

    jobs, _, root, client, _ = jobs_env
    (root / "one").write_text("must survive")
    response = await client.post("/api/v1/operations", json=payload(["one"]))
    failed = False

    def unavailable(conn, cursor, statement, parameters, context, executemany):
        nonlocal failed
        if not failed and statement.startswith("SELECT storage_operations.id"):
            failed = True
            raise RuntimeError("controlled worker SQLite read failure")

    event.listen(database.engine.sync_engine, "before_cursor_execute", unavailable)
    try:
        await jobs.start()
        await jobs.worker
        assert jobs.stopping
        detail = await terminal(client, response.json()["operation_id"])
        assert detail["state"] == "failed" and detail["skipped"] == 1
        assert (root / "one").read_text() == "must survive"
        assert (
            await client.post("/api/v1/operations", json=payload(["one"]))
        ).status_code == 503
    finally:
        event.remove(database.engine.sync_engine, "before_cursor_execute", unavailable)


@pytest.mark.asyncio
async def test_operation_notifications_are_user_scoped_invalidations(
    jobs_env, monkeypatch
):
    from app import auth, websocket
    from tests.test_websocket_backpressure import Socket

    jobs, sessions, root, client, second = jobs_env
    monkeypatch.setattr(websocket, "async_session", sessions)
    manager = websocket.ConnectionManager()
    monkeypatch.setattr(websocket, "manager", manager)
    first_socket, other_socket = Socket(), Socket()
    await manager.connect(
        first_socket, auth._hash_token(client.cookies.get("session_token")), 1
    )
    await manager.connect(other_socket, auth._hash_token(second), 2)
    try:
        (root / "private-name").write_text("data")
        response = await client.post(
            "/api/v1/operations", json=payload(["private-name"])
        )
        await jobs.start()
        id = response.json()["operation_id"]
        assert (await terminal(client, id))["state"] == "completed"
        await asyncio.wait_for(first_socket.started.wait(), 3)
        assert first_socket.messages
        assert all(
            message == {"type": "operation_update", "operation_id": id}
            for message in first_socket.messages
        )
        assert other_socket.messages == []
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_real_http_webdav_deletion_and_failed_only_retry(jobs_env):
    from aiohttp import web

    jobs, sessions, _, client, _ = jobs_env
    objects = {"/base/good": b"good", "/base/locked": b"locked"}
    allow_locked = False

    async def delete(request):
        if request.path == "/base/locked" and not allow_locked:
            return web.Response(status=403)
        assert request.path in objects
        objects.pop(request.path)
        return web.Response(status=204)

    app = web.Application()
    app.router.add_delete("/{tail:.*}", delete)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    port = site._server.sockets[0].getsockname()[1]
    try:
        async with sessions() as db:
            db.add(
                database.RemoteStorage(
                    id=7,
                    name="synthetic-webdav",
                    storage_type="webdav",
                    enabled=True,
                    base_path="/base",
                    webdav_url=f"http://127.0.0.1:{port}",
                )
            )
            await db.commit()
        response = await client.post(
            "/api/v1/operations", json={**payload(["good", "locked"]), "storage_id": 7}
        )
        assert response.status_code == 202
        id = response.json()["operation_id"]
        await jobs.start()
        detail = await terminal(client, id)
        assert (
            detail["succeeded"] == 1
            and detail["failed"] == 1
            and detail["state"] == "failed"
        )
        assert objects == {"/base/locked": b"locked"}
        objects["/base/good"] = b"new object must survive retry"
        allow_locked = True
        retried = await client.post(
            f"/api/v1/operations/{id}/retry", json={"request_id": str(uuid.uuid4())}
        )
        assert retried.status_code == 202
        assert (await terminal(client, retried.json()["operation_id"]))[
            "state"
        ] == "completed"
        assert objects == {"/base/good": b"new object must survive retry"}
    finally:
        await runner.cleanup()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "change",
    [
        {"action": "backup"},
        {"storage_id": 0},
        {"storage_id": True},
        {"request_id": "not-a-uuid"},
        {"path": "../escape"},
        {"paths": ["   "]},
    ],
)
async def test_invalid_operation_input_has_no_durable_effect(jobs_env, change):
    _, sessions, _, client, _ = jobs_env
    response = await client.post(
        "/api/v1/operations", json={**payload(["one"]), **change}
    )
    assert response.status_code == 422
    async with sessions() as db:
        assert (
            await db.scalar(select(func.count()).select_from(database.StorageOperation))
            == 0
        )
