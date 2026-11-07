"""Restore destination contract: real SQLite/TAR/files, discovery doubles only."""

import io
import tarfile
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from app.database import (
    Backup,
    BackupStatus,
    BackupTarget,
    RemoteStorage,
    BackupStorageSync,
)
from app.api import backups
from app.docker_client import docker_client


@pytest.mark.asyncio
async def test_disabled_cached_remote_is_not_a_restore_source(
    async_client, archive, test_db, tmp_path, monkeypatch
):
    import shutil
    from app.remote_storage import storage_manager

    id, _ = archive
    remote = tmp_path / "offsite"
    remote.mkdir()
    shutil.copyfile(tmp_path / "fixture.tar", remote / "fixture.tar")
    (tmp_path / "fixture.tar").unlink()
    (remote / "fixture.tar.key").write_bytes(b"owned synthetic sidecar")
    monkeypatch.setattr(storage_manager, "backends", {})
    monkeypatch.setattr(storage_manager, "configs", {})
    async with test_db() as db:
        storage = RemoteStorage(
            name="fixture", storage_type="local", base_path=str(remote), enabled=True
        )
        db.add(storage)
        await db.flush()
        db.add(
            BackupStorageSync(
                backup_id=id,
                storage_id=storage.id,
                remote_path="fixture.tar",
                sync_status="completed",
            )
        )
        await db.commit()
        row = await db.get(Backup, id)
        source, temporary = await backups._ensure_backup_locally(row)
        assert (
            source
            and __import__("pathlib").Path(source).read_bytes()
            == (remote / "fixture.tar").read_bytes()
        )
        shutil.rmtree(temporary)
        storage.enabled = False
        await db.commit()
    source, temporary = await backups._ensure_backup_locally(row)
    try:
        assert source is None and temporary is None
        assert (
            await backups._download_key_from_remote(row, str(tmp_path / "fixture.tar"))
            is None
        )
    finally:
        if temporary:
            shutil.rmtree(temporary)


@pytest.mark.asyncio
async def test_review_deduplicates_aliases_of_one_actual_workload(
    async_client, archive, test_db, monkeypatch
):
    id, target_id = archive
    async with test_db() as db:
        target = await db.get(BackupTarget, target_id)
        target.dependencies = ["worker", "owned-id"]
        await db.commit()
    monkeypatch.setattr(
        docker_client, "get_container_identity", AsyncMock(return_value="owned-id")
    )
    monkeypatch.setattr(
        docker_client, "get_container_state", AsyncMock(return_value="running")
    )
    review = (
        await async_client.post(
            f"/api/v1/backups/{id}/restore-review",
            json={"destination_type": "original"},
        )
    ).json()
    assert review["containers_to_stop"] == ["worker"]
    assert review["containers_to_restart"] == ["worker"]


@pytest.mark.asyncio
@pytest.mark.parametrize("original", [False, True])
async def test_missing_volume_mountpoint_never_creates_local_substitute(
    async_client, archive, tmp_path, test_db, monkeypatch, original
):
    id, target_id = archive
    missing = tmp_path / "not-mounted"
    monkeypatch.setattr(
        docker_client,
        "list_volumes",
        AsyncMock(
            return_value=[
                SimpleNamespace(
                    name="owned",
                    mountpoint=str(missing),
                    driver="local",
                    created_at="fixture",
                )
            ]
        ),
    )
    if original:
        async with test_db() as db:
            target = await db.get(BackupTarget, target_id)
            target.target_type = "volume"
            target.volume_name = "owned"
            await db.commit()
    selection = (
        {"destination_type": "original"}
        if original
        else {"destination_type": "volume", "target_volume": "owned"}
    )
    review = await async_client.post(
        f"/api/v1/backups/{id}/restore-review", json=selection
    )
    assert review.status_code == 400, review.text
    legacy = await async_client.post(
        f"/api/v1/backups/{id}/restore",
        json={} if original else {**selection, "review_fingerprint": "unavailable"},
    )
    assert legacy.status_code == 400
    assert not missing.exists()


@pytest.fixture
async def archive(test_db, tmp_path, monkeypatch, async_client, request):
    # Independent synthetic callers, rather than accumulating a whole suite as one IP.
    monkeypatch.setattr(async_client._transport, "client", (request.node.nodeid, 123))
    monkeypatch.setattr(backups.settings, "BACKUP_BASE_PATH", str(tmp_path))
    path = tmp_path / "fixture.tar"
    with tarfile.open(path, "w") as tar:
        item = tarfile.TarInfo("source/nested/file.txt")
        item.size = 5
        tar.addfile(item, io.BytesIO(b"bytes"))
    async with test_db() as db:
        target = BackupTarget(
            name="fixture",
            target_type="path",
            host_path=str(tmp_path / "original"),
            stop_container=False,
            dependencies=[],
        )
        db.add(target)
        await db.flush()
        row = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(path),
            backup_metadata={
                "archive_layout": {"version": 1, "kind": "single", "root": "source"}
            },
        )
        db.add(row)
        await db.commit()
        return row.id, target.id


@pytest.mark.asyncio
@pytest.mark.parametrize("choice", ["original", "path", "volume"])
async def test_review_then_real_restore(
    async_client, archive, tmp_path, monkeypatch, choice
):
    id, _ = archive
    mount = tmp_path / "actual-mount"
    mount.mkdir()
    monkeypatch.setattr(
        docker_client,
        "list_volumes",
        AsyncMock(
            return_value=[
                SimpleNamespace(
                    name="alternative",
                    mountpoint=str(mount),
                    driver="local",
                    created_at="fixture",
                    labels={},
                )
            ]
        ),
    )
    selection = {"destination_type": choice}
    if choice == "path":
        selection["target_path"] = str(tmp_path / "custom")
    if choice == "volume":
        selection["target_volume"] = "alternative"
    review = await async_client.post(
        f"/api/v1/backups/{id}/restore-review", json=selection
    )
    assert review.status_code == 200, review.text
    info = review.json()
    assert not (tmp_path / "custom").exists()
    result = await async_client.post(
        f"/api/v1/backups/{id}/restore",
        json={**selection, "review_fingerprint": info["fingerprint"]},
    )
    assert result.status_code == 200, result.text
    assert result.json()["status"] == "restored"
    assert result.json()["destination"] == info["destination"]
    assert (
        __import__("pathlib").Path(info["destination"]["path"]) / "nested/file.txt"
    ).read_bytes() == b"bytes"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "selection",
    [
        {"destination_type": "original", "target_path": "/ignored"},
        {"destination_type": "path"},
        {"destination_type": "volume", "target_volume": "missing"},
        {"destination_type": "path", "target_path": "/etc"},
        {"destination_type": "volume", "target_volume": "v", "target_path": "/ignored"},
    ],
)
async def test_invalid_destinations_never_extract(
    async_client, archive, tmp_path, selection
):
    id, _ = archive
    result = await async_client.post(f"/api/v1/backups/{id}/restore", json=selection)
    assert result.status_code in (400, 409, 422)
    assert not (tmp_path / "original").exists()


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["container", "stack"])
async def test_original_unsupported(async_client, archive, test_db, kind):
    id, target_id = archive
    async with test_db() as db:
        target = await db.get(BackupTarget, target_id)
        target.target_type = kind
        await db.commit()
    info = (await async_client.get(f"/api/v1/backups/{id}/restore-info")).json()
    assert info["original_supported"] is False
    result = await async_client.post(
        f"/api/v1/backups/{id}/restore-review", json={"destination_type": "original"}
    )
    assert result.status_code == 400


@pytest.mark.asyncio
async def test_changed_destination_rejected_without_write(
    async_client, archive, tmp_path, test_db
):
    id, target_id = archive
    review = (
        await async_client.post(
            f"/api/v1/backups/{id}/restore-review",
            json={"destination_type": "original"},
        )
    ).json()
    async with test_db() as db:
        target = await db.get(BackupTarget, target_id)
        target.host_path = str(tmp_path / "changed")
        await db.commit()
    result = await async_client.post(
        f"/api/v1/backups/{id}/restore",
        json={
            "destination_type": "original",
            "review_fingerprint": review["fingerprint"],
        },
    )
    assert result.status_code == 409
    assert not (tmp_path / "changed").exists()


@pytest.mark.asyncio
async def test_typed_execute_requires_review_but_legacy_path_still_works(
    async_client, archive, tmp_path
):
    id, _ = archive
    result = await async_client.post(
        f"/api/v1/backups/{id}/restore",
        json={"destination_type": "path", "target_path": str(tmp_path / "custom")},
    )
    assert result.status_code == 409
    result = await async_client.post(
        f"/api/v1/backups/{id}/restore", json={"target_path": str(tmp_path / "legacy")}
    )
    assert result.status_code == 200
    assert (tmp_path / "legacy/nested/file.txt").read_bytes() == b"bytes"


@pytest.mark.asyncio
async def test_volume_replaced_after_review_is_rejected(
    async_client, archive, tmp_path, monkeypatch
):
    id, _ = archive
    mount = tmp_path / "volume"
    mount.mkdir()
    volumes = [
        SimpleNamespace(
            name="v", mountpoint=str(mount), driver="local", created_at="first"
        )
    ]
    monkeypatch.setattr(
        docker_client, "list_volumes", AsyncMock(side_effect=lambda: volumes)
    )
    selection = {"destination_type": "volume", "target_volume": "v"}
    review = (
        await async_client.post(f"/api/v1/backups/{id}/restore-review", json=selection)
    ).json()
    volumes[0].created_at = "replacement"
    response = await async_client.post(
        f"/api/v1/backups/{id}/restore",
        json={**selection, "review_fingerprint": review["fingerprint"]},
    )
    assert response.status_code == 409
    assert list(mount.iterdir()) == []


@pytest.mark.asyncio
async def test_symlink_retarget_after_review_is_rejected(
    async_client, archive, tmp_path
):
    id, _ = archive
    first, second = tmp_path / "first", tmp_path / "second"
    first.mkdir()
    second.mkdir()
    link = tmp_path / "link"
    link.symlink_to(first)
    selection = {"destination_type": "path", "target_path": str(link)}
    review = (
        await async_client.post(f"/api/v1/backups/{id}/restore-review", json=selection)
    ).json()
    link.unlink()
    link.symlink_to(second)
    response = await async_client.post(
        f"/api/v1/backups/{id}/restore",
        json={**selection, "review_fingerprint": review["fingerprint"]},
    )
    assert response.status_code == 409
    assert not list(first.iterdir()) and not list(second.iterdir())


@pytest.mark.asyncio
async def test_workload_changed_while_waiting_for_claim_never_stops(
    async_client, archive, tmp_path, test_db, monkeypatch
):
    id, target_id = archive
    async with test_db() as db:
        target = await db.get(BackupTarget, target_id)
        target.dependencies = ["worker"]
        await db.commit()
    monkeypatch.setattr(
        docker_client, "get_container_identity", AsyncMock(return_value="owned-id")
    )
    monkeypatch.setattr(
        docker_client, "get_container_state", AsyncMock(return_value="running")
    )
    stop = AsyncMock(return_value=True)
    start = AsyncMock(return_value=True)
    monkeypatch.setattr(docker_client, "stop_container", stop)
    monkeypatch.setattr(docker_client, "start_container", start)
    selection = {"destination_type": "original"}
    review = (
        await async_client.post(f"/api/v1/backups/{id}/restore-review", json=selection)
    ).json()
    assert review["containers_to_stop"] == ["worker"]
    original = backups.backup_engine._acquire_workload_locks

    async def changed(*args, **kwargs):
        result = await original(*args, **kwargs)
        monkeypatch.setattr(
            docker_client, "get_container_state", AsyncMock(return_value="exited")
        )
        return result

    monkeypatch.setattr(backups.backup_engine, "_acquire_workload_locks", changed)
    response = await async_client.post(
        f"/api/v1/backups/{id}/restore",
        json={**selection, "review_fingerprint": review["fingerprint"]},
    )
    assert response.status_code == 409
    stop.assert_not_called()
    start.assert_not_called()
    assert not (tmp_path / "original").exists()
