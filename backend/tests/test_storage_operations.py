import asyncio
import os
import threading

import pytest

from app import storage_operations as operations


def test_delete_target_swap_does_not_follow_link(tmp_path, monkeypatch):
    target = tmp_path / "target"
    target.mkdir()
    (target / "data").write_bytes(b"delete")
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "sentinel").write_bytes(b"keep")
    real_rmtree = operations.shutil.rmtree

    def swap(name, *, dir_fd):
        target.rename(tmp_path / "original")
        target.symlink_to(outside, target_is_directory=True)
        real_rmtree(name, dir_fd=dir_fd)

    swap.avoids_symlink_attacks = True
    monkeypatch.setattr(operations.shutil, "rmtree", swap)
    result = operations.delete_local_target(tmp_path, "target")
    assert not result.success
    assert (outside / "sentinel").read_bytes() == b"keep"


def test_delete_parent_swap_never_uses_replacement(tmp_path, monkeypatch):
    parent = tmp_path / "parent"
    parent.mkdir()
    (parent / "data").write_bytes(b"delete")
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "data").write_bytes(b"keep")
    real_unlink = os.unlink

    def swap(name, *, dir_fd):
        parent.rename(tmp_path / "original")
        parent.symlink_to(outside, target_is_directory=True)
        real_unlink(name, dir_fd=dir_fd)

    monkeypatch.setattr(operations.os, "unlink", swap)
    assert operations.delete_local_target(tmp_path, "parent/data").success
    assert (outside / "data").read_bytes() == b"keep"
    assert not (tmp_path / "original" / "data").exists()


async def test_size_cache_nested_changes_ttl_identity_and_bound(tmp_path, monkeypatch):
    sizes = operations.DirectorySizes(max_entries=2, ttl=0.05)
    folder = tmp_path / "folder"
    folder.mkdir()
    nested = folder / "nested"
    nested.mkdir()
    (nested / "data").write_bytes(b"12345")
    original = operations.scan_local_size
    calls = []

    def scan(*args):
        calls.append(1)
        return original(*args)

    monkeypatch.setattr(operations, "scan_local_size", scan)
    assert (await sizes.get(1, tmp_path, "folder"))["bytes"] == 5
    assert (await sizes.get(1, tmp_path, "folder"))["bytes"] == 5
    assert len(calls) == 1
    (nested / "data").write_bytes(b"12345678")
    assert (await sizes.get(1, tmp_path, "folder"))["bytes"] == 8
    assert len(calls) == 2
    (nested / "new").write_bytes(b"xx")
    assert (await sizes.get(1, tmp_path, "folder"))["bytes"] == 10
    await asyncio.sleep(0.06)
    await sizes.get(1, tmp_path, "folder")
    assert len(calls) == 4
    await sizes.get(2, tmp_path, "folder")
    await sizes.get(3, tmp_path, "folder")
    assert len(sizes.cache) == 2
    sizes.invalidate(tmp_path)
    assert len(sizes.cache) == 0


async def test_size_cancel_joins_worker_and_bounds_parallelism(tmp_path, monkeypatch):
    sizes = operations.DirectorySizes(parallelism=2)
    (tmp_path / "folder").mkdir()
    entered = threading.Event()
    release = threading.Event()
    active = 0
    peak = 0
    lock = threading.Lock()
    real_scan = operations.scan_local_size

    def slow(root, path, cancel):
        nonlocal active, peak
        with lock:
            active += 1
            peak = max(peak, active)
        entered.set()
        try:
            while not release.wait(0.005):
                if cancel.is_set():
                    raise InterruptedError
            return real_scan(root, path, cancel)
        finally:
            with lock:
                active -= 1

    monkeypatch.setattr(operations, "scan_local_size", slow)
    tasks = [asyncio.create_task(sizes.get(i, tmp_path, "folder")) for i in range(6)]
    while not entered.is_set():
        await asyncio.sleep(0.005)
    # The loop is responsive even with recursive workers blocked in filesystem I/O.
    await asyncio.wait_for(asyncio.sleep(0.01), timeout=0.1)
    tasks[0].cancel()
    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(tasks[0], timeout=1)
    assert active <= 2
    release.set()
    results = await asyncio.gather(*tasks[1:])
    assert all(r["bytes"] == 0 for r in results)
    assert peak == 2
    assert active == 0
    assert all(key[0] != 0 for key in sizes.cache)


async def test_real_permission_errors_are_not_zero_or_success(tmp_path):
    assert os.geteuid() != 0, "This permission regression must run unprivileged"
    parent = tmp_path / "locked"
    parent.mkdir()
    (parent / "data").write_bytes(b"keep")
    parent.chmod(0)
    try:
        result = operations.delete_local_target(tmp_path, "locked/data")
        assert (result.success, result.status_code) == (False, 403)
        result = await operations.DirectorySizes().get(1, tmp_path, "locked")
        assert result == {"path": "locked", "state": "unavailable", "bytes": None}
    finally:
        parent.chmod(0o700)
    assert (parent / "data").read_bytes() == b"keep"


@pytest.mark.parametrize("level_cancel", [False, True])
async def test_disconnect_or_level_cancel_settles_size_worker(
    tmp_path, monkeypatch, level_cancel
):
    from anyio import CancelScope
    from starlette.requests import Request
    from app.api.storage import _browser_directory_size
    from app.remote_storage import LocalStorage, StorageConfig, StorageType

    (tmp_path / "folder").mkdir()
    entered, stopped = threading.Event(), threading.Event()

    def slow(root, path, cancel):
        entered.set()
        try:
            while not cancel.wait(0.005):
                pass
            raise InterruptedError
        finally:
            stopped.set()

    monkeypatch.setattr(operations, "scan_local_size", slow)
    monkeypatch.setattr(operations, "directory_sizes", operations.DirectorySizes())
    backend = LocalStorage(
        StorageConfig(
            id=1, name="test", storage_type=StorageType.LOCAL, base_path=str(tmp_path)
        )
    )

    async def receive():
        if not level_cancel:
            return {"type": "http.disconnect"}
        await asyncio.sleep(10)

    request = Request({"type": "http"}, receive=receive)
    if level_cancel:
        with CancelScope() as scope:

            async def cancel_when_started():
                while not entered.is_set():
                    await asyncio.sleep(0.005)
                scope.cancel()

            trigger = asyncio.create_task(cancel_when_started())
            try:
                await _browser_directory_size(backend, "folder", request)
            finally:
                await trigger
    else:
        with pytest.raises(asyncio.CancelledError):
            await _browser_directory_size(backend, "folder", request)
    assert entered.is_set() and stopped.is_set()
    assert not operations.directory_sizes.cache
