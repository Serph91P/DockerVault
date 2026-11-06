"""Single-file response owns preparation, stream, slot and cleanup together."""

import asyncio
import io
import tarfile
import threading

import pytest

from app.api import backups
from app.database import Backup, BackupStatus, BackupTarget


async def catalog(test_db, tmp_path):
    archive = tmp_path / "archive.tar"
    with tarfile.open(archive, "w") as tar:
        for index in range(100):
            member = tarfile.TarInfo(f"entry-{index:03d}")
            member.size = 4
            tar.addfile(member, io.BytesIO(b"data"))
    async with test_db() as session:
        target = BackupTarget(
            name="owned-download",
            target_type="path",
            host_path=str(tmp_path / "source"),
        )
        session.add(target)
        await session.flush()
        backup = Backup(
            target_id=target.id, status=BackupStatus.COMPLETED, file_path=str(archive)
        )
        session.add(backup)
        await session.commit()
        return backup.id, archive


@pytest.mark.asyncio
async def test_single_file_tar_preparation_runs_off_eventloop_with_bounded_tarinfo(
    async_client, test_db, tmp_path, monkeypatch
):
    identifier, archive = await catalog(test_db, tmp_path)
    original = tarfile.open
    observations = []

    def opened(*args, **kwargs):
        observations.append((threading.current_thread().name, kwargs.get("stream")))
        return original(*args, **kwargs)

    monkeypatch.setattr(tarfile, "open", opened)
    response = await async_client.get(f"/api/v1/backups/{identifier}/files/entry-099")
    assert response.status_code == 200 and response.content == b"data"
    assert observations and all(
        name != "MainThread" and stream is True for name, stream in observations
    )


@pytest.mark.asyncio
async def test_prepare_cancel_joins_reader_before_releasing_shared_slot(
    async_client, test_db, tmp_path, monkeypatch
):
    identifier, archive = await catalog(test_db, tmp_path)
    original = tarfile.open
    entered, release = threading.Event(), threading.Event()
    monkeypatch.setattr(backups, "_archive_scan_semaphore", asyncio.Semaphore(1))

    def opened(*args, **kwargs):
        entered.set()
        assert release.wait(5)
        return original(*args, **kwargs)

    monkeypatch.setattr(tarfile, "open", opened)
    response = await backups.download_backup_file(identifier, "entry-099")

    async def receive():
        await asyncio.sleep(60)

    async def send(message):
        pass

    operation = asyncio.create_task(
        response({"type": "http", "asgi": {"spec_version": "2.4"}}, receive, send)
    )
    try:
        async with asyncio.timeout(5):
            while not entered.is_set():
                await asyncio.sleep(0.001)
        operation.cancel()
        done, _ = await asyncio.wait([operation], timeout=0.05)
        assert not done, (
            "Preparation thread still owns archive after response cancellation"
        )
        assert backups._archive_scan_semaphore.locked()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await operation
        assert not backups._archive_scan_semaphore.locked()
        from pathlib import Path

        for descriptor in Path("/proc/self/fd").iterdir():
            try:
                assert descriptor.resolve(strict=True) != archive
            except FileNotFoundError:
                pass
    finally:
        release.set()
        await asyncio.gather(operation, return_exceptions=True)


@pytest.mark.asyncio
async def test_cancel_before_first_body_chunk_still_closes_archive(
    async_client, test_db, tmp_path, monkeypatch
):
    identifier, archive = await catalog(test_db, tmp_path)
    monkeypatch.setattr(backups, "_archive_scan_semaphore", asyncio.Semaphore(1))
    response = await backups.download_backup_file(identifier, "entry-099")

    async def receive():
        await asyncio.sleep(60)

    async def send(message):
        if message["type"] == "http.response.start":
            raise asyncio.CancelledError

    with pytest.raises(asyncio.CancelledError):
        await response({"type": "http", "asgi": {"spec_version": "2.4"}}, receive, send)
    assert not backups._archive_scan_semaphore.locked()
    from pathlib import Path

    for descriptor in Path("/proc/self/fd").iterdir():
        try:
            assert descriptor.resolve(strict=True) != archive
        except FileNotFoundError:
            pass
