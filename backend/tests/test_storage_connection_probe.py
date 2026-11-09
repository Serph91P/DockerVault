"""Connection probes must never own pre-existing names or swapped identities."""

import asyncio
import os
import threading

import pytest

from app.remote_storage import LocalStorage, StorageConfig, StorageType


def backend(root):
    return LocalStorage(
        StorageConfig(
            id=1, name="Synthetic", storage_type=StorageType.LOCAL, base_path=str(root)
        )
    )


@pytest.mark.parametrize("kind", ["file", "directory", "symlink"])
async def test_preserves_legacy_probe_name(tmp_path, kind):
    root = tmp_path / "root"
    root.mkdir()
    target = tmp_path / "target"
    target.write_bytes(b"must survive")
    old = root / ".backup_test"
    if kind == "file":
        old.write_bytes(b"original")
    elif kind == "directory":
        old.mkdir()
    else:
        old.symlink_to(target)
    identity = old.lstat()
    result = await backend(root).test_connection()
    assert result["success"] is True
    assert old.lstat().st_ino == identity.st_ino
    if kind == "file":
        assert old.read_bytes() == b"original"
    assert target.read_bytes() == b"must survive"
    assert list(root.iterdir()) == [old]


async def test_listing_distinguishes_links(tmp_path):
    (tmp_path / "file").write_bytes(b"bytes")
    (tmp_path / "link").symlink_to(tmp_path / "file")
    files = {item["name"]: item for item in await backend(tmp_path).list_files()}
    assert files["link"]["is_symlink"] is True
    assert files["file"]["is_symlink"] is False
    assert files["link"]["size"] is None


async def test_new_directory_and_probe_cleanup(tmp_path):
    root = tmp_path / "new" / "nested"
    result = await backend(root).test_connection()
    assert result["success"] is True
    assert list(root.iterdir()) == []
    assert str(root) not in result["message"]


async def test_exclusive_name_collision_preserved(tmp_path, monkeypatch):
    real_open = os.open
    collision = []

    def open_collision(path, flags, mode=0o777, *, dir_fd=None):
        if flags & os.O_EXCL:
            fd = real_open(path, flags, mode, dir_fd=dir_fd)
            os.write(fd, b"foreign")
            os.close(fd)
            collision.append(path)
        return real_open(path, flags, mode, dir_fd=dir_fd)

    monkeypatch.setattr(os, "open", open_collision)
    result = await backend(tmp_path).test_connection()
    assert result["success"] is False
    assert len(collision) == 1
    assert (tmp_path / collision[0]).read_bytes() == b"foreign"


@pytest.mark.parametrize("swap", ["root", "name", "symlink"])
async def test_swapped_identity_fails_closed(tmp_path, monkeypatch, swap):
    root = tmp_path / "root"
    root.mkdir()
    target = tmp_path / "target"
    target.write_bytes(b"keep")
    real_write = os.write
    preserved = []

    def write_swap(fd, data):
        value = real_write(fd, data)
        probe = next(root.iterdir())
        if swap == "root":
            root.rename(tmp_path / "moved")
            root.mkdir()
            probe = root / probe.name
        else:
            probe.rename(root / "owned-moved")
        if swap == "symlink":
            probe.symlink_to(target)
        else:
            probe.write_bytes(b"foreign")
        preserved.append(probe)
        return value

    monkeypatch.setattr(os, "write", write_swap)
    result = await backend(root).test_connection()
    assert result["success"] is False
    assert (
        preserved[0].is_symlink()
        if swap == "symlink"
        else preserved[0].read_bytes() == b"foreign"
    )
    assert target.read_bytes() == b"keep"


@pytest.mark.parametrize("failure", ["write", "unlink", "open"])
async def test_probe_errors_are_not_success(tmp_path, monkeypatch, failure):
    def fail(*args, **kwargs):
        raise OSError("synthetic private path")

    monkeypatch.setattr(os, failure, fail)
    result = await backend(tmp_path).test_connection()
    assert result["success"] is False
    assert "synthetic private path" not in result["message"]
    if failure == "write":
        assert list(tmp_path.iterdir()) == []


async def test_cancel_waits_for_owned_cleanup(tmp_path, monkeypatch):
    entered, release = threading.Event(), threading.Event()
    real_write = os.write

    def held_write(fd, data):
        entered.set()
        assert release.wait(5)
        return real_write(fd, data)

    monkeypatch.setattr(os, "write", held_write)
    task = asyncio.create_task(backend(tmp_path).test_connection())
    try:
        assert await asyncio.to_thread(entered.wait, 2)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done()
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert list(tmp_path.iterdir()) == []
