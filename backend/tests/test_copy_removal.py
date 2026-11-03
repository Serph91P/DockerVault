"""Exact confirmed copy removal against real SQLite and local/remote files."""

from pathlib import Path
from unittest.mock import patch

import pytest
from sqlalchemy import select

from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
)
from app.remote_storage import storage_manager


async def seed(test_db, tmp_path, *, count=1, encrypted=False):
    async with test_db() as db:
        target = BackupTarget(
            name="confirmed plan", target_type="path", host_path=str(tmp_path)
        )
        remote = tmp_path / "remote"
        remote.mkdir(exist_ok=True)
        storage = RemoteStorage(
            name="fixture", storage_type="local", base_path=str(remote)
        )
        db.add_all([target, storage])
        await db.flush()
        ids = []
        for n in range(count):
            path = tmp_path / f"archive-{n}.tar.gz"
            path.write_bytes(b"retained bytes")
            (remote / path.name).write_bytes(path.read_bytes())
            key = str(path) + ".key" if encrypted else None
            if key:
                Path(key).write_bytes(b"sidecar")
                (remote / (path.name + ".key")).write_bytes(b"sidecar")
            backup = Backup(
                target_id=target.id,
                status=BackupStatus.COMPLETED,
                file_path=str(path),
                encrypted=encrypted,
                encryption_key_path=key,
            )
            db.add(backup)
            await db.flush()
            db.add(
                BackupStorageSync(
                    backup_id=backup.id,
                    storage_id=storage.id,
                    remote_path=path.name,
                    sync_status="completed",
                )
            )
            ids.append(backup.id)
        await db.commit()
        return target.id, ids, storage.id


async def preview(client, ids=None, **kwargs):
    response = await client.post(
        "/api/v1/backups/removal/preview", json={"backup_ids": ids, **kwargs}
    )
    assert response.status_code == 200, response.text
    return response.json()


@pytest.mark.asyncio
@pytest.mark.parametrize("scope", ["local", "remote", "all"])
async def test_explicit_copy_scope_preserves_other_copy(
    async_client, test_db, tmp_path, scope
):
    _, ids, _ = await seed(test_db, tmp_path, encrypted=True)
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
    ):
        review = await preview(async_client, ids, scope=scope)
        assert review["archive_count"] == 1
        assert review["copy_count"] == (2 if scope == "all" else 1)
        response = await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": review["token"]}
        )
        assert response.status_code == 200, response.text
        assert response.json()["status"] == "completed"
        assert (tmp_path / "archive-0.tar.gz").exists() == (scope == "remote")
        assert (tmp_path / "remote/archive-0.tar.gz").exists() == (scope == "local")
        # The local key is required to restore retained remote copies.
        assert (tmp_path / "archive-0.tar.gz.key").exists() == (scope != "all")
        async with test_db() as db:
            backup = await db.get(Backup, ids[0])
            syncs = (await db.scalars(select(BackupStorageSync))).all()
            assert len(syncs) == (1 if scope == "local" else 0)
            if scope != "all":
                assert backup is not None
                assert bool(backup.file_path) == (scope == "remote")
            else:
                assert backup is None
        repeated = await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": review["token"]}
        )
        assert repeated.json() == response.json()  # bounded duplicate, no repeated I/O


@pytest.mark.asyncio
async def test_preview_over_500_freezes_filter_and_rejects_changed_scope(
    async_client, test_db, tmp_path
):
    target_id, ids, _ = await seed(test_db, tmp_path, count=505)
    review = await preview(async_client, target_id=target_id, scope="local")
    assert review["archive_count"] == 505 and review["copy_count"] == 505
    async with test_db() as db:
        new_path = tmp_path / "later.tar"
        new_path.write_bytes(b"not confirmed")
        later = Backup(
            target_id=target_id, status=BackupStatus.COMPLETED, file_path=str(new_path)
        )
        db.add(later)
        await db.commit()
    result = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": review["token"]}
    )
    assert result.status_code == 200, result.text
    assert result.json()["succeeded"] == 505
    assert new_path.read_bytes() == b"not confirmed"
    changed = await preview(async_client, [ids[0]], scope="remote")
    async with test_db() as db:
        row = await db.scalar(
            select(BackupStorageSync).where(BackupStorageSync.backup_id == ids[0])
        )
        row.remote_path = "changed.tar"
        await db.commit()
    result = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": changed["token"]}
    )
    assert result.json()["status"] == "incomplete"
    assert result.json()["skipped"] == 1
    assert (tmp_path / "remote/archive-0.tar.gz").exists()


@pytest.mark.asyncio
async def test_local_failure_and_remote_offline_are_truthful_retry_remainder(
    async_client, test_db, tmp_path, monkeypatch
):
    _, ids, _ = await seed(test_db, tmp_path)
    review = await preview(async_client, ids, scope="all")
    original = Path.unlink

    def fail_archive(path, *args, **kwargs):
        if path == tmp_path / "archive-0.tar.gz":
            raise OSError("fixture denied")
        return original(path, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", fail_archive)
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
    ):
        response = await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": review["token"]}
        )
        body = response.json()
        assert response.status_code == 200 and body["status"] == "incomplete"
        assert body["failed"] == 1 and body["succeeded"] == 1 and body["warning"]
        assert body["retry_token"]
        assert not (tmp_path / "remote/archive-0.tar.gz").exists()
        async with test_db() as db:
            assert (await db.get(Backup, ids[0])).file_path
        monkeypatch.setattr(Path, "unlink", original)
        retry = await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": body["retry_token"]}
        )
        assert retry.json()["succeeded"] == 1 and retry.json()["failed"] == 0
        assert not (tmp_path / "archive-0.tar.gz").exists()


@pytest.mark.asyncio
async def test_offline_and_sidecar_failure_keep_exact_retry_metadata(
    async_client, test_db, tmp_path, monkeypatch
):
    _, ids, storage_id = await seed(test_db, tmp_path, encrypted=True)
    review = await preview(async_client, ids, scope="remote")
    async with test_db() as db:
        storage = await db.get(RemoteStorage, storage_id)
        storage.enabled = False
        await db.commit()
    changed = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": review["token"]}
    )
    assert changed.json()["skipped"] == 1
    review = await preview(async_client, ids, scope="remote")
    offline = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": review["token"]}
    )
    assert offline.json()["failed"] == 1
    assert (tmp_path / "remote/archive-0.tar.gz").exists()
    async with test_db() as db:
        storage = await db.get(RemoteStorage, storage_id)
        storage.enabled = True
        await db.commit()
    review = await preview(async_client, ids, scope="all")
    original = Path.unlink

    def fail_key(path, *args, **kwargs):
        if path == tmp_path / "archive-0.tar.gz.key":
            raise OSError("sidecar denied")
        return original(path, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", fail_key)
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
    ):
        partial = await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": review["token"]}
        )
        assert partial.json()["failed"] == 1 and partial.json()["succeeded"] == 1
        assert not (tmp_path / "archive-0.tar.gz").exists()
        assert (tmp_path / "archive-0.tar.gz.key").exists()
        async with test_db() as db:
            assert (await db.get(Backup, ids[0])).encryption_key_path
        monkeypatch.setattr(Path, "unlink", original)
        retry = await async_client.post(
            "/api/v1/backups/removal/execute",
            json={"token": partial.json()["retry_token"]},
        )
        assert retry.json()["status"] == "completed" and retry.json()["succeeded"] == 1
        assert not (tmp_path / "archive-0.tar.gz.key").exists()


@pytest.mark.asyncio
async def test_active_claim_and_replaced_file_never_deleted(
    async_client, test_db, tmp_path
):
    from app.backup_engine import backup_engine

    _, ids, _ = await seed(test_db, tmp_path)
    review = await preview(async_client, ids, scope="local")
    async with backup_engine._get_target_lock(1):
        result = await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": review["token"]}
        )
    assert result.json()["failed"] == 1
    path = tmp_path / "archive-0.tar.gz"
    path.write_bytes(b"changed after confirmation")
    result = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": result.json()["retry_token"]}
    )
    assert result.json()["skipped"] == 1 and result.json()["retry_token"] is None
    assert path.read_bytes() == b"changed after confirmation"


@pytest.mark.asyncio
async def test_shared_remote_identity_is_not_deleted(async_client, test_db, tmp_path):
    target_id, ids, storage_id = await seed(test_db, tmp_path)
    async with test_db() as db:
        other = Backup(
            target_id=target_id, status=BackupStatus.COMPLETED, encrypted=False
        )
        db.add(other)
        await db.flush()
        db.add(
            BackupStorageSync(
                backup_id=other.id,
                storage_id=storage_id,
                remote_path="archive-0.tar.gz",
                sync_status="completed",
            )
        )
        await db.commit()
    review = await preview(async_client, ids, scope="remote")
    result = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": review["token"]}
    )
    assert result.json()["skipped"] == 1
    assert (tmp_path / "remote/archive-0.tar.gz").exists()


@pytest.mark.asyncio
async def test_expired_unknown_and_unconfirmed_bulk_do_not_mutate(
    async_client, test_db, tmp_path
):
    from app.copy_removal import _confirmations

    _, ids, _ = await seed(test_db, tmp_path)
    review = await preview(async_client, ids, scope="all")
    _confirmations[review["token"]].expires = 0
    for token in [review["token"], "unknown"]:
        result = await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": token}
        )
        assert result.status_code == 409
    assert (await async_client.delete("/api/v1/backups")).status_code == 409
    assert (tmp_path / "archive-0.tar.gz").exists()


@pytest.mark.asyncio
async def test_combined_retry_cleans_only_confirmed_deferred_key(
    async_client, test_db, tmp_path
):
    _, ids, _ = await seed(test_db, tmp_path, encrypted=True)
    path = tmp_path / "remote/archive-0.tar.gz"
    saved = path.read_bytes()
    path.unlink()
    path.mkdir()
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
    ):
        review = await preview(async_client, ids, scope="all")
        first = await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": review["token"]}
        )
        assert first.json()["status"] == "incomplete"
        assert not (tmp_path / "archive-0.tar.gz").exists()
        assert (tmp_path / "archive-0.tar.gz.key").exists()
        path.rmdir()
        path.write_bytes(saved)
        retried = await async_client.post(
            "/api/v1/backups/removal/execute",
            json={"token": first.json()["retry_token"]},
        )
        assert retried.json()["status"] == "completed"
        assert not (tmp_path / "archive-0.tar.gz.key").exists()
        async with test_db() as db:
            assert await db.get(Backup, ids[0]) is None


@pytest.mark.asyncio
async def test_recreated_remote_archive_identity_needs_new_confirmation(
    async_client, test_db, tmp_path
):
    target_id, ids, storage_id = await seed(test_db, tmp_path)
    old = await preview(async_client, ids, scope="remote")
    async with test_db() as db:
        await db.delete(await db.get(Backup, ids[0]))
        await db.commit()
        recreated = Backup(
            id=ids[0], target_id=target_id, status=BackupStatus.COMPLETED
        )
        db.add(recreated)
        await db.flush()
        db.add(
            BackupStorageSync(
                backup_id=recreated.id,
                storage_id=storage_id,
                remote_path="archive-0.tar.gz",
                sync_status="completed",
            )
        )
        await db.commit()
    result = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": old["token"]}
    )
    assert result.json()["skipped"] == 1
    assert (tmp_path / "remote/archive-0.tar.gz").exists()


@pytest.mark.asyncio
async def test_full_confirmation_cache_cannot_lose_partial_result(
    async_client, test_db, tmp_path, monkeypatch
):
    from app import copy_removal

    monkeypatch.setattr(copy_removal, "_confirmations", {})
    _, ids, _ = await seed(test_db, tmp_path)
    token = (await preview(async_client, ids, scope="remote"))["token"]
    for _ in range(99):
        await preview(async_client, ids, scope="local")
    path = tmp_path / "remote/archive-0.tar.gz"
    path.unlink()
    path.mkdir()
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
    ):
        response = await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": token}
        )
        assert response.status_code == 200, response.text
        assert (
            response.json()["status"] == "incomplete" and response.json()["retry_token"]
        )


@pytest.mark.asyncio
async def test_second_confirmation_reports_not_found_without_redeleting(
    async_client, test_db, tmp_path
):
    _, ids, _ = await seed(test_db, tmp_path)
    first = await preview(async_client, ids, scope="all")
    second = await preview(async_client, ids, scope="all")
    assert (
        await async_client.post(
            "/api/v1/backups/removal/execute", json={"token": first["token"]}
        )
    ).json()["status"] == "completed"
    missing = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": second["token"]}
    )
    assert missing.json()["status"] == "incomplete" and missing.json()["skipped"] == 2
    assert missing.json()["succeeded"] == 0 and missing.json()["retry_token"] is None


@pytest.mark.asyncio
async def test_recovery_required_journal_keeps_real_copies(
    async_client, test_db, tmp_path
):
    from app.database import WorkloadOperation

    _, ids, _ = await seed(test_db, tmp_path)
    async with test_db() as db:
        db.add(
            WorkloadOperation(
                backup_id=ids[0],
                kind="restore",
                state="recovery_required",
                resources={},
            )
        )
        await db.commit()
    review = await preview(async_client, ids, scope="all")
    result = await async_client.post(
        "/api/v1/backups/removal/execute", json={"token": review["token"]}
    )
    assert result.json()["failed"] == 2
    assert (tmp_path / "archive-0.tar.gz").exists() and (
        tmp_path / "remote/archive-0.tar.gz"
    ).exists()


@pytest.mark.asyncio
async def test_cancelled_caller_keeps_claim_until_real_provider_and_catalog_settle(
    async_client, test_db, tmp_path, monkeypatch
):
    import asyncio
    from app.copy_removal import execute_removal
    from app.api.backups import _db_to_storage_config
    from app.backup_engine import backup_engine

    target_id, ids, storage_id = await seed(test_db, tmp_path)
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
    ):
        async with test_db() as db:
            backend = storage_manager.add_storage(
                _db_to_storage_config(await db.get(RemoteStorage, storage_id))
            )
        entered, release = asyncio.Event(), asyncio.Event()
        original = backend.delete

        async def delayed_delete(path):
            entered.set()
            await release.wait()
            return await original(path)

        monkeypatch.setattr(backend, "delete", delayed_delete)
        review = await preview(async_client, ids, scope="remote")
        work = asyncio.create_task(execute_removal(review["token"]))
        try:
            await asyncio.wait_for(entered.wait(), 5)
            work.cancel()
            await asyncio.sleep(0)
            assert backup_engine._get_target_lock(target_id).locked()
            assert (tmp_path / "remote/archive-0.tar.gz").exists()
        finally:
            release.set()
            with pytest.raises(asyncio.CancelledError):
                await work
        result = await execute_removal(review["token"])
        assert result["status"] == "completed" and result["succeeded"] == 1
        assert not (tmp_path / "remote/archive-0.tar.gz").exists()
        assert (tmp_path / "archive-0.tar.gz").exists()
        async with test_db() as db:
            assert (await db.scalars(select(BackupStorageSync))).all() == []
