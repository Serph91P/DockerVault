"""Real filesystem regressions for the storage browser's destructive boundary."""

import os

import pytest

from app.config import settings
from app.database import RemoteStorage
from app.remote_storage import LocalStorage, StorageConfig, StorageType, storage_manager


@pytest.fixture(params=["local", "configured"])
async def storage_root(request, tmp_path, monkeypatch, db_session):
    root = tmp_path / "root"
    root.mkdir()
    if request.param == "local":
        monkeypatch.setattr(settings, "BACKUP_BASE_PATH", str(root))
        prefix = "/api/v1/storage/local/files"
    else:
        row = RemoteStorage(name="Synthetic", storage_type="local", base_path=str(root))
        db_session.add(row)
        await db_session.commit()
        storage_manager.remove_storage(row.id)
        prefix = f"/api/v1/storage/{row.id}/files"
    yield root, prefix
    if request.param != "local":
        storage_manager.remove_storage(row.id)


async def test_local_delete_nested_directory(async_client, storage_root, tmp_path):
    root, prefix = storage_root
    nested = root / "backup" / "nested"
    nested.mkdir(parents=True)
    (nested / "data").write_bytes(b"synthetic backup")
    sentinel = tmp_path / "sentinel"
    sentinel.write_bytes(b"keep")
    (nested / "external").symlink_to(sentinel)
    response = await async_client.delete(prefix, params={"path": "backup"})
    assert response.status_code == 200, response.text
    assert not (root / "backup").exists()
    assert sentinel.read_bytes() == b"keep"


@pytest.mark.parametrize(
    "path",
    [
        "",
        ".",
        "/",
        "/backup",
        "../backup",
        "x/../backup",
        "x/../../backup",
        "C:/backup",
        "C:backup",
        "\\\\host\\backup",
        "backup\x00",
    ],
)
async def test_local_delete_rejects_lexical_paths(async_client, storage_root, path):
    root, prefix = storage_root
    (root / "backup").write_bytes(b"keep")
    response = await async_client.delete(prefix, params={"path": path})
    assert response.status_code == 400
    assert (root / "backup").read_bytes() == b"keep"
    assert str(root) not in response.text


@pytest.mark.parametrize("target", ["internal", "external", "dangling"])
async def test_local_delete_unlinks_leaf_not_target(
    async_client, storage_root, tmp_path, target
):
    root, prefix = storage_root
    sentinel = (root if target == "internal" else tmp_path) / target
    if target != "dangling":
        sentinel.write_bytes(b"keep")
    (root / "link").symlink_to(sentinel)
    response = await async_client.delete(prefix, params={"path": "link"})
    assert response.status_code == 200
    assert not os.path.lexists(root / "link")
    if target != "dangling":
        assert sentinel.read_bytes() == b"keep"


async def test_local_delete_parent_symlink_and_missing(
    async_client, storage_root, tmp_path
):
    root, prefix = storage_root
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "sentinel").write_bytes(b"keep")
    (root / "parent").symlink_to(outside, target_is_directory=True)
    response = await async_client.delete(prefix, params={"path": "parent/sentinel"})
    assert response.status_code == 400
    assert (outside / "sentinel").read_bytes() == b"keep"
    response = await async_client.delete(prefix, params={"path": "missing"})
    assert response.status_code == 404


async def test_local_bulk_preserves_all_results(async_client, storage_root):
    root, prefix = storage_root
    (root / "first").write_bytes(b"delete")
    (root / "last").write_bytes(b"delete")
    response = await async_client.post(
        prefix + "/bulk-delete",
        json={"paths": ["first", "../escape", "missing", "last"]},
    )
    assert response.status_code == 200
    data = response.json()
    assert [r["success"] for r in data["results"]] == [True, False, False, True]
    assert (data["succeeded"], data["failed"]) == (2, 2)
    assert list(root.iterdir()) == []
    assert str(root) not in response.text


async def test_local_sizes_are_separate_and_do_not_follow_symlinks(
    async_client, storage_root, tmp_path
):
    root, prefix = storage_root
    nested = root / "backup" / "nested"
    nested.mkdir(parents=True)
    (nested / "a").write_bytes(b"12345")
    (root / "backup" / "b").write_bytes(b"123")
    (tmp_path / "outside").write_bytes(b"x" * 100)
    (nested / "external").symlink_to(tmp_path / "outside")
    (root / "dangling").symlink_to(tmp_path / "absent")
    listing = await async_client.get(prefix)
    assert listing.status_code == 200
    entries = {item["name"]: item for item in listing.json()["files"]}
    assert entries["backup"]["size"] is None
    assert entries["dangling"]["is_dir"] is False
    response = await async_client.get(prefix + "/size", params={"path": "backup"})
    assert response.status_code == 200
    assert response.json() == {"path": "backup", "state": "completed", "bytes": 8}
    (nested / "a").write_bytes(b"1234567890")
    response = await async_client.get(prefix + "/size", params={"path": "backup"})
    assert response.json()["bytes"] == 13
    for path in ("missing", "dangling", "../outside", ""):
        response = await async_client.get(prefix + "/size", params={"path": path})
        assert response.json()["state"] == "unavailable"
        assert response.json()["bytes"] is None


async def test_local_listing_rejects_parent_symlink(
    async_client, storage_root, tmp_path
):
    root, prefix = storage_root
    (tmp_path / "outside").mkdir()
    (root / "link").symlink_to(tmp_path / "outside", target_is_directory=True)
    response = await async_client.get(prefix, params={"path": "link"})
    assert response.status_code == 400


async def test_internal_local_cleanup_stays_idempotent(tmp_path):
    backend = LocalStorage(
        StorageConfig(
            id=1, name="test", storage_type=StorageType.LOCAL, base_path=str(tmp_path)
        )
    )
    assert await backend.delete("missing") is True
    (tmp_path / "archive").mkdir()
    (tmp_path / "archive" / "sentinel").write_bytes(b"keep")
    assert await backend.delete("archive") is False
    assert (tmp_path / "archive" / "sentinel").read_bytes() == b"keep"


async def test_local_permission_bulk_keeps_successes(async_client, storage_root):
    assert os.geteuid() != 0, "Permission checks must execute unprivileged"
    root, prefix = storage_root
    locked = root / "locked"
    locked.mkdir()
    (locked / "data").write_bytes(b"keep")
    (root / "first").write_bytes(b"delete")
    (root / "last").write_bytes(b"delete")
    locked.chmod(0)
    try:
        single = await async_client.delete(prefix, params={"path": "locked/data"})
        assert single.status_code == 403
        response = await async_client.post(
            prefix + "/bulk-delete", json={"paths": ["first", "locked/data", "last"]}
        )
        assert response.status_code == 200
        assert [r["success"] for r in response.json()["results"]] == [True, False, True]
        assert response.json()["results"][1]["error"] == "Permission denied"
        size = await async_client.get(prefix + "/size", params={"path": "locked"})
        assert size.json()["bytes"] is None
        assert size.json()["state"] == "unavailable"
        assert str(root) not in response.text
    finally:
        locked.chmod(0o700)
    assert (locked / "data").read_bytes() == b"keep"


async def test_listing_and_health_do_not_wait_for_size_scan(
    async_client, storage_root, monkeypatch
):
    import asyncio
    import threading
    from app import storage_operations as operations

    root, prefix = storage_root
    (root / "folder").mkdir()
    entered, release = threading.Event(), threading.Event()
    real_scan = operations.scan_local_size

    def slow(*args):
        entered.set()
        assert release.wait(3)
        return real_scan(*args)

    monkeypatch.setattr(operations, "scan_local_size", slow)
    pending = asyncio.create_task(
        async_client.get(prefix + "/size", params={"path": "folder"})
    )
    try:
        while not entered.is_set():
            await asyncio.sleep(0.005)
        listing = await asyncio.wait_for(async_client.get(prefix), timeout=1)
        health = await asyncio.wait_for(async_client.get("/health"), timeout=1)
        assert listing.status_code == health.status_code == 200
        assert not pending.done()
    finally:
        release.set()
        await pending


async def test_unsupported_provider_size_is_honestly_unavailable(
    async_client, db_session
):
    row = RemoteStorage(
        name="Synthetic remote",
        storage_type="s3",
        base_path="backups",
        s3_bucket="synthetic",
    )
    db_session.add(row)
    await db_session.commit()
    try:
        response = await async_client.get(
            f"/api/v1/storage/{row.id}/files/size", params={"path": "folder"}
        )
        assert response.json() == {
            "path": "folder",
            "state": "unavailable",
            "bytes": None,
        }
    finally:
        storage_manager.remove_storage(row.id)
