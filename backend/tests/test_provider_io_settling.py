"""Real non-abortable copy settling and native FTP ABOR at provider boundaries."""

import asyncio
from pathlib import Path
import shutil
import threading

import aioftp
import pytest

from app.remote_storage import LocalStorage, StorageConfig, StorageType
from tests import test_ftp_protocol
from tests.test_remote_object_ownership import test_engine  # noqa: F401

ftp_service = test_ftp_protocol.ftp_service


@pytest.mark.asyncio
@pytest.mark.parametrize("direction", ["upload", "download"])
async def test_local_cancellation_joins_actual_copy_before_return(
    tmp_path, monkeypatch, direction
):
    remote = tmp_path / "remote"
    remote.mkdir()
    backend = LocalStorage(
        StorageConfig(
            id=1, name="owned", storage_type=StorageType.LOCAL, base_path=str(remote)
        )
    )
    source = tmp_path / "source" if direction == "upload" else remote / "archive.tar"
    destination = (
        remote / "archive.tar" if direction == "upload" else tmp_path / "download"
    )
    source.write_bytes(b"owned non-abortable file copy")
    entered, settled = asyncio.Event(), asyncio.Event()
    release = threading.Event()
    loop = asyncio.get_running_loop()
    original = shutil.copy2

    def copy(*args, **kwargs):
        loop.call_soon_threadsafe(entered.set)
        if not release.wait(10):
            raise TimeoutError("Owned copy fixture unreleased")
        try:
            return original(*args, **kwargs)
        finally:
            loop.call_soon_threadsafe(settled.set)

    monkeypatch.setattr("app.remote_storage.shutil.copy2", copy)
    operation = (
        backend.upload(source, "archive.tar")
        if direction == "upload"
        else backend.download("archive.tar", destination)
    )
    task = asyncio.create_task(operation)
    try:
        await asyncio.wait_for(entered.wait(), 5)
        task.cancel()
        done, _ = await asyncio.wait([task], timeout=0.05)
        assert not done, (
            "Cancellation returned while the actual copy executor still owns the file"
        )
        task.cancel()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert settled.is_set()
        assert destination.read_bytes() == source.read_bytes()
    finally:
        release.set()
        await asyncio.wait_for(settled.wait(), 5)
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
async def test_ftp_cancellation_aborts_and_settles_real_server_writer(
    ftp_service, tmp_path, monkeypatch
):
    backend, root = ftp_service
    entered, release, settled, aborted = (asyncio.Event() for _ in range(4))
    original_write = aioftp.PathIO.write

    async def write(self, file, data):
        if Path(file.name).is_relative_to(root):
            entered.set()
            try:
                await release.wait()
                return await original_write(self, file, data)
            finally:
                settled.set()
        return await original_write(self, file, data)

    monkeypatch.setattr(aioftp.PathIO, "write", write)
    original_client = aioftp.Client

    class ObservedClient(original_client):
        async def command(self, command, *args, **kwargs):
            if command == "ABOR":
                aborted.set()
            return await super().command(command, *args, **kwargs)

    monkeypatch.setattr(aioftp, "Client", ObservedClient)
    source = tmp_path / "source"
    source.write_bytes(b"owned FTP must not write after cancellation settles")
    task = asyncio.create_task(backend.upload(source, "archive.tar"))
    try:
        await asyncio.wait_for(entered.wait(), 5)
        task.cancel()
        done, _ = await asyncio.wait([task], timeout=2)
        assert done and aborted.is_set(), (
            "Cancellation closed locally but never sent native FTP ABOR"
        )
        with pytest.raises(asyncio.CancelledError):
            await task
        assert settled.is_set(), (
            "Server writer must be stopped before object ownership is released"
        )
        destination = root / "backups/archive.tar"
        assert (
            not destination.exists() or destination.read_bytes() != source.read_bytes()
        )
    finally:
        release.set()
        await asyncio.wait_for(settled.wait(), 5)
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
async def test_non_abortable_webdav_write_is_settled_before_cancellation(
    tmp_path, test_db, monkeypatch
):
    from aiohttp import web
    from app.api import settings as api_settings
    from app.remote_storage import RemoteStorageManager

    monkeypatch.setattr(api_settings, "async_session", test_db)
    entered, release, settled = (asyncio.Event() for _ in range(3))
    destination = tmp_path / "owned-dav-object"

    async def service(request):
        if request.method == "MKCOL":
            return web.Response(status=201)
        if request.method == "PUT":
            body = await request.read()
            if not entered.is_set():
                entered.set()
                await release.wait()
            destination.write_bytes(body)
            settled.set()
            return web.Response(status=201)
        if request.method == "HEAD":
            return web.Response(
                headers={"Content-Length": str(destination.stat().st_size)}
            )
        if request.method == "GET":
            return web.Response(body=destination.read_bytes())
        return web.Response(status=405)

    app = web.Application()
    app.router.add_route("*", "/{path:.*}", service)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    manager = RemoteStorageManager()
    manager.add_storage(
        StorageConfig(
            id=1,
            name="owned-dav",
            storage_type=StorageType.WEBDAV,
            base_path="",
            webdav_url=f"http://127.0.0.1:{site._server.sockets[0].getsockname()[1]}/dav",
        )
    )
    source = tmp_path / "source"
    source.write_bytes(b"first cancelled identity")
    task = asyncio.create_task(manager.upload_to_all(source, "archive.tar", [1]))
    try:
        await asyncio.wait_for(entered.wait(), 5)
        task.cancel()
        done, _ = await asyncio.wait([task], timeout=0.05)
        assert not done, "A cancelled HTTP request alone cannot stop a server-side PUT"
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert settled.is_set() and destination.read_bytes() == source.read_bytes()
        source.write_bytes(b"second acknowledged identity")
        assert await manager.upload_to_all(source, "archive.tar", [1]) == {
            1: (True, "")
        }
        assert destination.read_bytes() == source.read_bytes()
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
        await runner.cleanup()
