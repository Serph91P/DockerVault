"""Owned real FTP service, including repeated operations and failed handshakes."""

import asyncio
from pathlib import Path

import aioftp
import pytest
import pytest_asyncio

from app.remote_storage import FTPStorage, StorageConfig, StorageType


@pytest_asyncio.fixture
async def ftp_service(tmp_path):
    root = tmp_path / "server"
    root.mkdir()
    server = aioftp.Server([aioftp.User("owned", "synthetic", base_path=root)])
    await server.start("127.0.0.1", 0)
    port = server.server.sockets[0].getsockname()[1]
    storage = FTPStorage(
        StorageConfig(
            id=1,
            name="owned",
            storage_type=StorageType.FTP,
            host="127.0.0.1",
            port=port,
            username="owned",
            password="synthetic",
            base_path="/backups",
        )
    )
    try:
        yield storage, root
    finally:
        await server.close()


@pytest.mark.asyncio
async def test_ftp_real_roundtrip_and_repeated_operations(ftp_service, tmp_path):
    storage, root = ftp_service
    assert (await storage.test_connection())["success"]
    source = tmp_path / "archive.tar"
    source.write_bytes(b"owned archive bytes")
    for name in ["nested/archive.tar", "nested/archive.tar.key"]:
        assert await storage.upload(source, name) == (True, "")
        assert (root / "backups" / name).read_bytes() == source.read_bytes()
        files = await storage.list_files("nested")
        assert any(
            f["name"] == Path(name).name and f["size"] == source.stat().st_size
            for f in files
        )
        restored = tmp_path / "downloads" / Path(name).name
        assert await storage.download(name, restored)
        assert restored.read_bytes() == source.read_bytes()
        assert await storage.delete(name)
        assert not (root / "backups" / name).exists()
    assert (await storage.test_connection())["success"]


@pytest.mark.asyncio
async def test_ftp_login_failure_and_upload_error_close_real_connections(
    ftp_service, tmp_path, monkeypatch
):
    storage, root = ftp_service
    clients = []
    original = aioftp.Client

    class ObservedClient(original):
        def __init__(self, **kwargs):
            super().__init__(**kwargs)
            clients.append(self)
            self.was_closed = False

        def close(self):
            self.was_closed = True
            return super().close()

    monkeypatch.setattr(aioftp, "Client", ObservedClient)
    storage.config.password = "wrong"
    result = await asyncio.wait_for(storage.test_connection(), 3)
    assert not result["success"] and "530" in result["message"]
    assert clients[-1].was_closed
    source = tmp_path / "source"
    source.write_bytes(b"owned")
    success, error = await storage.upload(source, "archive.tar")
    assert not success and "530" in error
    assert clients[-1].was_closed
    storage.config.password = "synthetic"
    assert await storage.upload(tmp_path / "missing", "missing") != (True, "")
    assert clients[-1].was_closed


@pytest.mark.asyncio
@pytest.mark.parametrize("phase", ["connect", "login", "cancel"])
async def test_ftp_stalled_handshake_is_bounded_and_closed(
    tmp_path, monkeypatch, phase
):
    clients, arguments = [], []
    entered = asyncio.Event()
    release = asyncio.Event()

    async def stalled(reader, writer):
        try:
            if phase != "connect":
                writer.write(b"220 Owned service\r\n")
                await writer.drain()
            entered.set()
            await release.wait()
        finally:
            writer.close()
            await writer.wait_closed()

    server = await asyncio.start_server(stalled, "127.0.0.1", 0)
    original = aioftp.Client

    class ObservedClient(original):
        def __init__(self, **kwargs):
            arguments.append(kwargs.copy())
            # Retain real aioftp networking with short fixture-only deadlines.
            super().__init__(
                **(kwargs | {"socket_timeout": 0.05, "connection_timeout": 0.05})
            )
            clients.append(self)
            self.was_closed = False

        def close(self):
            self.was_closed = True
            return super().close()

    monkeypatch.setattr(aioftp, "Client", ObservedClient)
    storage = FTPStorage(
        StorageConfig(
            id=1,
            name="owned",
            storage_type=StorageType.FTP,
            host="127.0.0.1",
            port=server.sockets[0].getsockname()[1],
            username="owned",
            password="synthetic",
        )
    )
    task = asyncio.create_task(storage.test_connection())
    try:
        if phase == "cancel":
            await asyncio.wait_for(entered.wait(), 2)
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
        else:
            result = await asyncio.wait_for(task, 2)
            assert not result["success"] and "Timeout" in result["message"]
        assert clients[-1].was_closed
        assert (
            arguments[0]["socket_timeout"] > 0
            and arguments[0]["connection_timeout"] > 0
        )
        assert not arguments[0].get("ssl"), (
            "Existing FTP must not silently switch transport"
        )
    finally:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
        release.set()
        server.close()
        await server.wait_closed()
