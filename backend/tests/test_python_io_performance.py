"""Issue231 regressions: bounded downloads, complete verification and settled hooks."""

import asyncio
import hashlib
import os
from pathlib import Path
import shlex
import sys
from types import SimpleNamespace

from aiohttp import web
import pytest
import pytest_asyncio

from app.backup_engine import BackupEngine
from app.config import settings
from app.remote_storage import (
    LocalStorage,
    RemoteStorageManager,
    S3Storage,
    StorageConfig,
    StorageType,
)


@pytest_asyncio.fixture
async def download_service():
    state = SimpleNamespace(
        mode="normal", entered=asyncio.Event(), release=asyncio.Event()
    )
    payload = b"owned independent remote bytes" * 100000

    async def serve(request):
        response = web.StreamResponse(headers={"Content-Length": str(len(payload))})
        await response.prepare(request)
        await response.write(payload[:1024])
        state.entered.set()
        if state.mode == "hold":
            await state.release.wait()
        if state.mode == "truncate":
            request.transport.close()
            return response
        try:
            for offset in range(1024, len(payload), 16384):
                await response.write(payload[offset : offset + 16384])
            await response.write_eof()
        except ConnectionResetError:
            pass
        return response

    app = web.Application()
    app.router.add_get("/{path:.*}", serve)
    runner = web.AppRunner(app, access_log=None)
    await runner.setup()
    server = await asyncio.get_running_loop().create_server(
        runner.server, "127.0.0.1", 0
    )
    port = server.sockets[0].getsockname()[1]
    storage = S3Storage(
        StorageConfig(
            id=1,
            name="owned",
            storage_type=StorageType.S3,
            s3_endpoint_url=f"http://127.0.0.1:{port}",
            s3_bucket="owned",
            s3_access_key="owned",
            s3_secret_key="owned",
        )
    )
    try:
        yield storage, state, payload
    finally:
        state.release.set()
        await storage.close()
        server.close()
        await server.wait_closed()
        await runner.cleanup()


@pytest.mark.asyncio
async def test_s3_real_http_uses_only_bounded_body_reads(
    download_service, tmp_path, monkeypatch
):
    from aiobotocore.response import StreamingBody

    storage, state, payload = download_service
    original = StreamingBody.read
    reads = []

    async def read(self, amt=None):
        reads.append(amt)
        return await original(self, amt)

    monkeypatch.setattr(StreamingBody, "read", read)
    destination = tmp_path / "archive"
    assert await storage.download("archive", destination)
    assert destination.read_bytes() == payload
    assert reads and all(
        isinstance(size, int) and 0 < size <= 1024 * 1024 for size in reads
    )
    assert set(tmp_path.iterdir()) == {destination}


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["truncate", "hold"])
async def test_s3_error_or_cancel_preserves_destination_and_removes_partials(
    download_service, tmp_path, mode
):
    storage, state, _ = download_service
    state.mode = mode
    destination = tmp_path / "archive"
    destination.write_bytes(b"preexisting published destination")
    operation = asyncio.create_task(storage.download("archive", destination))
    try:
        await asyncio.wait_for(state.entered.wait(), 5)
        if mode == "hold":
            operation.cancel()
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(operation, 5)
        else:
            assert not await asyncio.wait_for(operation, 5)
        assert destination.read_bytes() == b"preexisting published destination"
        assert set(tmp_path.iterdir()) == {destination}
    finally:
        state.release.set()
        await asyncio.gather(operation, return_exceptions=True)


@pytest.mark.asyncio
async def test_s3_same_size_corruption_is_not_published(download_service, tmp_path):
    storage, _, payload = download_service
    destination = tmp_path / "archive"
    destination.write_bytes(b"prior verified bytes")
    wrong = hashlib.sha256(b"z" * len(payload)).digest()
    assert not await storage._download("archive", destination, wrong)
    assert destination.read_bytes() == b"prior verified bytes"
    assert set(tmp_path.iterdir()) == {destination}


@pytest.mark.asyncio
async def test_multi_destination_upload_hashes_unchanged_source_only_once(
    tmp_path, monkeypatch
):
    source = tmp_path / "source"
    source.write_bytes(os.urandom(8192))
    manager = RemoteStorageManager()
    for identifier in (1, 2, 3):
        config = StorageConfig(
            id=identifier,
            name="owned",
            storage_type=StorageType.LOCAL,
            base_path=str(tmp_path / str(identifier)),
        )
        manager.configs[identifier] = config
        manager.backends[identifier] = LocalStorage(config)
    original = Path.open
    source_reads = []

    def opened(self, mode="r", *args, **kwargs):
        if self == source and mode == "rb":
            source_reads.append(self)
        return original(self, mode, *args, **kwargs)

    monkeypatch.setattr(Path, "open", opened)
    result = await manager.upload_to_all(source, "archive")
    assert all(success for success, error in result.values()), result
    assert len(source_reads) == 1
    assert all(
        (tmp_path / str(identifier) / "archive").read_bytes() == source.read_bytes()
        for identifier in (1, 2, 3)
    )


@pytest.mark.asyncio
async def test_mutating_upload_source_never_uses_a_stale_digest(tmp_path, monkeypatch):
    source = tmp_path / "source"
    source.write_bytes(b"original")
    manager = RemoteStorageManager()
    config = StorageConfig(
        id=1,
        name="owned",
        storage_type=StorageType.LOCAL,
        base_path=str(tmp_path / "remote"),
    )
    backend = LocalStorage(config)
    manager.configs[1], manager.backends[1] = config, backend
    original = backend.upload

    async def mutate(*args):
        source.write_bytes(b"mutation")
        return await original(*args)

    monkeypatch.setattr(backend, "upload", mutate)
    success, error = (await manager.upload_to_all(source, "archive"))[1]
    assert not success and "changed" in error.lower()


@pytest.mark.asyncio
async def test_cancelled_real_hook_child_is_reaped_before_return(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "ALLOWED_HOOK_COMMANDS", sys.executable)
    started = tmp_path / "started"
    completed = tmp_path / "completed"
    script = (
        "import os,time;from pathlib import Path;Path(%r).write_text(str(os.getpid()));time.sleep(.4);Path(%r).write_text('escaped')"
        % (str(started), str(completed))
    )
    operation = asyncio.create_task(
        BackupEngine()._run_hook(shlex.join([sys.executable, "-c", script]))
    )
    try:
        async with asyncio.timeout(5):
            while not started.exists():
                await asyncio.sleep(0.005)
        pid = int(started.read_text())
        operation.cancel()
        with pytest.raises(asyncio.CancelledError):
            await operation
        assert not Path(f"/proc/{pid}").exists(), (
            "Hook still owns source after caller cancellation"
        )
        await asyncio.sleep(0.5)
        assert not completed.exists()
    finally:
        await asyncio.gather(operation, return_exceptions=True)
        # Give an unfixed baseline child time to finish before pytest removes its fixture.
        await asyncio.sleep(0.5)
