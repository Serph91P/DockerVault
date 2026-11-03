"""Archive browsing must be bounded, paginated and off the event loop."""

import asyncio
import io
import tarfile
import threading

import pytest

from app.database import Backup, BackupStatus, BackupTarget
from tests.test_remote_object_ownership import test_engine  # noqa: F401


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

    async def slow_scan(archive_path, mode, limit, offset):
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
