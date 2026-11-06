"""Real archives and bounded thread/cache ownership for issue231."""

import asyncio
import io
import os
import tarfile
import threading
from unittest.mock import AsyncMock

import pytest

from app.api import backups
from app.backup_engine import BackupEngine
from app.database import Backup, BackupStatus, BackupTarget


def archive(path, prefix="owned", count=20):
    with tarfile.open(path, "w") as tar:
        for index in range(count):
            member = tarfile.TarInfo(f"{prefix}/entry-{index:05d}")
            member.size = 8
            tar.addfile(member, io.BytesIO(b"payload!"))


@pytest.mark.asyncio
async def test_listing_repeated_page_reuses_bounded_metadata_not_tar_scan(
    tmp_path, monkeypatch
):
    path = tmp_path / "archive.tar"
    archive(path)
    original = tarfile.open
    opened = []

    def tracked(*args, **kwargs):
        opened.append(args)
        return original(*args, **kwargs)

    monkeypatch.setattr(tarfile, "open", tracked)
    first, total = await backups._list_archive_entries(str(path), "r:", 5, 5)
    second, repeated_total = await backups._list_archive_entries(str(path), "r:", 5, 5)
    third, _ = await backups._list_archive_entries(str(path), "r:", 5, 10)
    assert third[0].path == "owned/entry-00010"
    assert first == second and total == repeated_total == 20
    assert len(opened) == 1, "Second identical page still scans the entire tar"
    assert all(info.path.startswith("owned/") for info in second)


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["replace", "mutate", "delete"])
async def test_listing_cache_identity_does_not_hide_file_changes(tmp_path, change):
    path = tmp_path / "archive.tar"
    archive(path)
    first, _ = await backups._list_archive_entries(str(path), "r:", 5, 0)
    before = path.stat()
    if change == "replace":
        replacement = tmp_path / "replacement"
        archive(replacement, "newer")
        os.replace(replacement, path)
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    elif change == "mutate":
        archive(path, "newer")
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    else:
        path.unlink()
        with pytest.raises(FileNotFoundError):
            await backups._list_archive_entries(str(path), "r:", 5, 0)
        return
    second, total = await backups._list_archive_entries(str(path), "r:", 5, 0)
    assert total == 20 and second != first
    assert all(info.path.startswith("newer/") for info in second)


@pytest.mark.asyncio
@pytest.mark.parametrize("page_limit,byte_limit", [(3, 8 * 1024 * 1024), (16, 512)])
async def test_listing_cache_has_explicit_entry_and_byte_limits(
    tmp_path, monkeypatch, page_limit, byte_limit
):
    monkeypatch.setattr(backups, "_ARCHIVE_CACHE_PAGES", page_limit)
    monkeypatch.setattr(backups, "_ARCHIVE_CACHE_BYTES", byte_limit)
    with backups._archive_cache_lock:
        backups._archive_listing_cache.clear()
    paths = []
    for index in range(25):
        path = tmp_path / f"archive-{index}.tar"
        archive(path, count=10)
        paths.append(path)
        entries, total = await backups._list_archive_entries(str(path), "r:", 2, 0)
        assert len(entries) == 2 and total == 10
    assert len(backups._archive_listing_cache) <= page_limit
    assert (
        sum(len(value[1]) for value in backups._archive_listing_cache.values())
        <= byte_limit
    )
    # A real LRU miss, not merely assertions on a single shared archive entry.
    original = tarfile.open
    reopened = []

    def observed(*args, **kwargs):
        reopened.append(args[0])
        return original(*args, **kwargs)

    monkeypatch.setattr(tarfile, "open", observed)
    await backups._list_archive_entries(str(paths[0]), "r:", 2, 0)
    assert reopened == [str(paths[0])]


@pytest.mark.asyncio
@pytest.mark.parametrize("posthook", [False, True])
async def test_restart_at_snapshot_boundary_only_without_posthook(
    async_client, test_db, tmp_path, monkeypatch, posthook
):
    from app import backup_engine as module

    engine = BackupEngine()
    source = tmp_path / "source"
    source.mkdir()
    (source / "payload").write_bytes(b"source bytes must survive")
    monkeypatch.setattr(module.settings, "BACKUP_BASE_PATH", str(tmp_path / "backups"))
    monkeypatch.setattr(engine, "_sudo_available", lambda: False)
    state = {"owned-id": "running"}
    events = []
    monkeypatch.setattr(
        module.docker_client,
        "get_dependency_order",
        AsyncMock(side_effect=lambda names: names),
    )
    monkeypatch.setattr(
        module.docker_client,
        "get_container_identity",
        AsyncMock(return_value="owned-id"),
    )
    monkeypatch.setattr(
        module.docker_client,
        "get_container_state",
        AsyncMock(side_effect=lambda name: state[name]),
    )

    async def stop(name):
        state[name] = "exited"
        events.append("stop")
        return True

    async def start(name):
        state[name] = "running"
        events.append("start")
        return True

    original_hash = engine._calculate_checksum

    async def checksum(path):
        events.append("hash:" + state["owned-id"])
        return await original_hash(path)

    async def hook(command):
        events.append("hook:" + state["owned-id"])

    monkeypatch.setattr(module.docker_client, "stop_container", stop)
    monkeypatch.setattr(module.docker_client, "start_container", start)
    monkeypatch.setattr(engine, "_calculate_checksum", checksum)
    monkeypatch.setattr(engine, "_run_hook", hook)
    async with test_db() as session:
        target = BackupTarget(
            name="owned",
            target_type="path",
            host_path=str(source),
            container_name="owned",
            stop_container=True,
            post_backup_command="test-hook" if posthook else None,
        )
        session.add(target)
        await session.commit()
    backup = await engine.create_backup(target)
    await engine.run_backup(backup.id)
    async with test_db() as session:
        saved = await session.get(Backup, backup.id)
        assert saved.status == BackupStatus.COMPLETED, saved.error_message
    assert "hash:" + ("exited" if posthook else "running") in events
    if posthook:
        assert events.index("hook:exited") < events.index("start")
    assert events.count("start") == 1 and state["owned-id"] == "running"


@pytest.mark.asyncio
async def test_hash_cancellation_waits_for_actual_reader(tmp_path, monkeypatch):
    import builtins

    source = tmp_path / "source"
    source.write_bytes(b"owned bytes")
    entered, release = threading.Event(), threading.Event()
    original = builtins.open

    def opened(path, *args, **kwargs):
        if str(path) == str(source):
            entered.set()
            assert release.wait(5), "Owned hash reader fixture unreleased"
        return original(path, *args, **kwargs)

    monkeypatch.setattr(builtins, "open", opened)
    task = asyncio.create_task(BackupEngine()._calculate_checksum(str(source)))
    try:
        async with asyncio.timeout(5):
            while not entered.is_set():
                await asyncio.sleep(0.001)
        task.cancel()
        done, _ = await asyncio.wait([task], timeout=0.05)
        assert not done, "Checksum thread outlives operation/claim cleanup"
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
