"""Archive browsing must be bounded, paginated and off the event loop."""

import asyncio
import io
import tarfile
import threading
import tracemalloc

import pytest

from app.database import Backup, BackupStatus, BackupTarget
from tests.test_remote_object_ownership import test_engine  # noqa: F401


@pytest.fixture(autouse=True)
def isolated_browse_registry(monkeypatch):
    from app.remote_storage import storage_manager

    monkeypatch.setattr(storage_manager, "backends", {})
    monkeypatch.setattr(storage_manager, "configs", {})


@pytest.fixture
def populated_archive(tmp_path):
    """A real .tar.gz with many distinct members plus a completed unencrypted backup."""
    archive = tmp_path / "owned.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        for index in range(9):
            name = f"owned/{index:03d}.txt"
            info = tarfile.TarInfo(name=name)
            payload = f"owned payload {index}".encode()
            info.size = len(payload)
            tar.addfile(info, io.BytesIO(payload))
    return archive


@pytest.fixture
async def backup_record(test_db, populated_archive):
    async with test_db() as session:
        target = BackupTarget(
            name="owned-browse", target_type="path", host_path="/owned"
        )
        session.add(target)
        await session.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(populated_archive),
            encrypted=False,
        )
        session.add(backup)
        await session.commit()
        return backup.id


@pytest.mark.asyncio
async def test_listing_is_paginated_without_duplicates_or_omissions(
    async_client, test_db, backup_record, populated_archive
):
    seen = []
    offset = 0
    while True:
        response = await async_client.get(
            f"/api/v1/backups/{backup_record}/files",
            params={"limit": 4, "offset": offset},
        )
        assert response.status_code == 200
        seen.extend(item["path"] for item in response.json())
        assert int(response.headers["X-Total-Count"]) == 9
        if response.headers["X-Has-More"] == "false":
            break
        offset += 4
    assert len(seen) == 9
    assert len(set(seen)) == 9
    with tarfile.open(populated_archive, "r:gz") as tar:
        assert sorted(seen) == [member.name for member in tar.getmembers()]


@pytest.mark.asyncio
async def test_default_listing_is_bounded_and_backward_compatible(
    async_client, test_db, backup_record
):
    response = await async_client.get(f"/api/v1/backups/{backup_record}/files")
    assert response.status_code == 200
    assert len(response.json()) == 9
    assert response.headers["X-Has-More"] == "false"
    from app.api.backups import MAX_ARCHIVE_LISTING

    assert MAX_ARCHIVE_LISTING >= 9
    clamped = await async_client.get(
        f"/api/v1/backups/{backup_record}/files",
        params={"limit": MAX_ARCHIVE_LISTING + 5000},
    )
    assert clamped.status_code == 422


def test_scan_memory_is_bounded_with_real_large_archive(tmp_path):
    from app.api.backups import _scan_archive_entries

    peaks = []
    for count in (1000, 30000):
        archive = tmp_path / f"owned-{count}.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            for index in range(count):
                tar.addfile(tarfile.TarInfo(f"owned/{index:06d}"))
        tracemalloc.start()
        try:
            files, total = _scan_archive_entries(str(archive), "r:gz", 1, 0)
            peaks.append(tracemalloc.get_traced_memory()[1])
        finally:
            tracemalloc.stop()
        assert total == count and len(files) == 1
    assert peaks[1] < 2_000_000, peaks
    assert peaks[1] < peaks[0] + 1_000_000, peaks


@pytest.mark.asyncio
async def test_cancelled_listing_holds_capacity_and_files_until_threads_settle(
    async_client, test_db, backup_record, populated_archive, monkeypatch
):
    from app.api import backups

    monkeypatch.setattr(backups, "_archive_scan_semaphore", asyncio.Semaphore(4))
    release = threading.Event()
    lock = threading.Lock()
    running = 0
    peak = 0
    preparations = 0
    original = backups._scan_archive_entries
    ensure = backups._ensure_backup_locally

    async def prepare(backup):
        nonlocal preparations
        preparations += 1
        return await ensure(backup)

    def blocked_scan(*args):
        nonlocal running, peak
        with lock:
            running += 1
            peak = max(peak, running)
        try:
            assert release.wait(10)
            assert populated_archive.exists()
            return original(*args)
        finally:
            with lock:
                running -= 1

    monkeypatch.setattr(backups, "_scan_archive_entries", blocked_scan)
    monkeypatch.setattr(backups, "_ensure_backup_locally", prepare)
    tasks = [
        asyncio.create_task(async_client.get(f"/api/v1/backups/{backup_record}/files"))
        for _ in range(4)
    ]
    try:
        async with asyncio.timeout(10):
            while running != 4:
                await asyncio.sleep(0.01)
        for task in tasks:
            task.cancel()
        tasks.append(
            asyncio.create_task(
                async_client.get(f"/api/v1/backups/{backup_record}/files")
            )
        )
        await asyncio.sleep(0.1)
        assert peak == 4
        assert preparations == 4, "remote/decryption preparation must share capacity"
        assert all(not task.done() for task in tasks[:4]), "cancel must join real scans"
    finally:
        release.set()
        await asyncio.gather(*tasks, return_exceptions=True)
    assert running == 0


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "encrypted,remote", [(False, False), (False, True), (True, False), (True, True)]
)
async def test_large_real_archive_pages_health_and_cancel_cleanup(
    async_client, test_db, tmp_path, monkeypatch, encrypted, remote
):
    from app.api import backups
    from app.encryption import encrypt_backup, generate_key_pair

    archive = tmp_path / "large-owned.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        for index in range(2005):
            tar.addfile(tarfile.TarInfo(f"owned/{index:06d}"))
    key = None
    if encrypted:
        key = await generate_key_pair()
        result = await encrypt_backup(archive, key.public_key)
        archive, key_path = result.encrypted_path, result.key_path
    async with test_db() as session:
        target = BackupTarget(
            name="owned-large", target_type="path", host_path=str(tmp_path)
        )
        session.add(target)
        await session.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(archive),
            encrypted=encrypted,
            encryption_key_path=str(key_path) if encrypted else None,
        )
        session.add(backup)
        await session.commit()
        bid = backup.id
    if remote:
        storage = await async_client.post(
            "/api/v1/storage",
            json={
                "name": "owned-large",
                "storage_type": "local",
                "base_path": str(tmp_path / "remote"),
            },
        )
        assert storage.status_code == 200
        synced = await async_client.post(
            f"/api/v1/storage/{storage.json()['id']}/sync/{bid}"
        )
        assert synced.status_code == 200, synced.text
        archive.unlink()
        if encrypted:
            key_path.unlink()

    async def listing(**params):
        if encrypted:
            return await async_client.post(
                f"/api/v1/backups/{bid}/files",
                json={"private_key": key.private_key},
                params=params,
            )
        return await async_client.get(f"/api/v1/backups/{bid}/files", params=params)

    first = await listing()
    second = await listing(limit=2000, offset=2000)
    assert first.status_code == second.status_code == 200
    assert len(first.json()) == 2000 and len(second.json()) == 5
    assert (
        first.headers["X-Has-More"] == "true"
        and second.headers["X-Has-More"] == "false"
    )
    assert first.headers["X-Total-Count"] == "2005"
    assert len({item["path"] for item in first.json() + second.json()}) == 2005

    entered = threading.Event()
    release = threading.Event()
    # Force real reader work rather than a valid cross-page metadata cache hit.
    with backups._archive_cache_lock:
        backups._archive_listing_cache.clear()
    original = backups._scan_archive_entries
    scanned_paths = []

    def blocked_scan(path, *args):
        from pathlib import Path

        scanned_paths.append(Path(path))
        entered.set()
        assert release.wait(10)
        assert Path(path).is_file(), "cancel must not remove a live worker's input"
        return original(path, *args)

    monkeypatch.setattr(backups, "_scan_archive_entries", blocked_scan)
    task = asyncio.create_task(listing(limit=1))
    try:
        async with asyncio.timeout(10):
            while not entered.is_set():
                await asyncio.sleep(0.01)
        # Real health handler must remain responsive during the blocking native scan.
        health = await asyncio.wait_for(async_client.get("/health"), 1)
        assert health.status_code == 200
        task.cancel()
        await asyncio.sleep(0.05)
        assert not task.done()
        assert scanned_paths[0].exists()
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
    if remote or encrypted:
        assert not scanned_paths[0].exists()
    else:
        assert archive.exists()


@pytest.mark.asyncio
async def test_scan_runs_off_the_event_loop(
    async_client, test_db, backup_record, monkeypatch
):
    from app.api import backups

    thread_names = []
    original = backups._scan_archive_entries

    def recording_scan(*args, **kwargs):
        thread_names.append(threading.current_thread().name)
        return original(*args, **kwargs)

    monkeypatch.setattr(backups, "_scan_archive_entries", recording_scan)
    response = await async_client.get(f"/api/v1/backups/{backup_record}/files")
    assert response.status_code == 200
    assert thread_names and all(name != "MainThread" for name in thread_names)


@pytest.mark.asyncio
async def test_cancelled_encrypted_listing_removes_decrypted_temp_file(
    async_client, test_db, monkeypatch, tmp_path
):
    from app.api import backups

    async with test_db() as session:
        target = BackupTarget(
            name="owned-browse-enc", target_type="path", host_path="/owned"
        )
        session.add(target)
        await session.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(tmp_path / "owned.tar.enc"),
            encrypted=True,
            encryption_key_path=str(tmp_path / "owned.tar.key"),
        )
        session.add(backup)
        await session.commit()
        backup_id = backup.id

    decrypted = tmp_path / "owned-decrypted.tar"
    decrypted.write_bytes(b"owned synthetic plaintext")
    encrypted_file = tmp_path / "owned.tar.enc"
    encrypted_file.write_bytes(b"owned synthetic ciphertext")
    key_file = tmp_path / "owned.tar.key"
    key_file.write_bytes(b"owned synthetic key")

    async def fake_decrypt(backup, private_key):
        return str(decrypted), "r:", decrypted

    entered = asyncio.Event()

    async def slow_scan(archive_path, mode, limit, offset, *, cache_identity=None):
        entered.set()
        await asyncio.sleep(60)
        raise AssertionError("unreachable")

    monkeypatch.setattr(backups, "_get_decrypted_archive_path", fake_decrypt)
    monkeypatch.setattr(backups, "_list_archive_entries", slow_scan)
    task = asyncio.create_task(
        async_client.post(
            f"/api/v1/backups/{backup_id}/files",
            json={"private_key": "owned synthetic private key"},
        )
    )
    await asyncio.wait_for(entered.wait(), 10)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert not decrypted.exists()
