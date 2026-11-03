"""Actual SDK/default checksum middleware against an owned loopback S3 subset."""

import asyncio
import hashlib
import io
from pathlib import Path
from types import SimpleNamespace
import tarfile
from xml.etree import ElementTree as ET

from aiohttp import web
import pytest
import pytest_asyncio

from app.remote_storage import S3Storage, StorageConfig, StorageType


@pytest_asyncio.fixture
async def s3_service():
    state = SimpleNamespace(
        objects={},
        uploads={},
        checksums=[],
        events=[],
        mode="normal",
        entered=asyncio.Event(),
        release=asyncio.Event(),
    )

    async def service(request):
        key, query = request.path, request.query
        state.events.append((request.method, key, dict(query)))
        if request.method == "POST" and "uploads" in query:
            state.uploads["owned-upload"] = {}
            return web.Response(
                text="<InitiateMultipartUploadResult><UploadId>owned-upload</UploadId></InitiateMultipartUploadResult>",
                content_type="application/xml",
            )
        if request.method == "DELETE" and "uploadId" in query:
            state.uploads.pop(query["uploadId"], None)
            return web.Response(status=204)
        if request.method == "POST" and "uploadId" in query:
            xml = ET.fromstring(await request.read())
            numbers = [
                int(node.text) for node in xml.iter() if node.tag.endswith("PartNumber")
            ]
            parts = state.uploads.pop(query["uploadId"])
            assert numbers == sorted(parts)
            state.objects[key] = b"".join(parts[number] for number in numbers)
            return web.Response(
                text='<CompleteMultipartUploadResult><ETag>"owned-complete"</ETag></CompleteMultipartUploadResult>',
                content_type="application/xml",
            )
        if request.method == "PUT":
            if state.mode == "forbidden":
                return web.Response(
                    status=403,
                    text="<Error><Code>AccessDenied</Code><Message>Owned fixture denial</Message></Error>",
                    content_type="application/xml",
                )
            data = await request.read()
            state.checksums.append(
                {k: v for k, v in request.headers.items() if "checksum" in k.lower()}
            )
            state.entered.set()
            if state.mode == "hold":
                await state.release.wait()
            if "partNumber" in query:
                state.uploads[query["uploadId"]][int(query["partNumber"])] = data
            else:
                state.objects[key] = data
            return web.Response(
                headers={
                    "ETag": '"'
                    + hashlib.md5(data, usedforsecurity=False).hexdigest()
                    + '"'
                }
            )
        if request.method == "GET":
            return web.Response(body=state.objects[key])
        raise AssertionError(request.method)

    app = web.Application(client_max_size=16 * 1024 * 1024)
    app.router.add_route("*", "/{path:.*}", service)
    runner = web.AppRunner(app, access_log=None)
    await runner.setup()
    assert runner.server is not None
    server = await asyncio.get_running_loop().create_server(
        runner.server, "127.0.0.1", 0
    )
    port = server.sockets[0].getsockname()[1]
    storage = S3Storage(
        StorageConfig(
            id=1,
            name="owned-s3",
            storage_type=StorageType.S3,
            s3_bucket="owned-bucket",
            s3_access_key="owned",
            s3_secret_key="not-real",
            s3_endpoint_url=f"http://127.0.0.1:{port}",
        )
    )
    try:
        yield storage, state
    finally:
        state.release.set()
        if storage._client is not None:
            await storage._client.close()
        server.close()
        await server.wait_closed()
        await runner.cleanup()


def assert_file_closed(source):
    """Linux CI resource check without coupling to open()/Path.open() call shape."""
    for descriptor in Path("/proc/self/fd").iterdir():
        try:
            assert descriptor.resolve(strict=True) != source.resolve()
        except FileNotFoundError:
            pass


@pytest.mark.asyncio
@pytest.mark.parametrize("name", ["archive.tar", "archive.tar.age.key", "empty.key"])
async def test_small_s3_body_reaches_real_http_with_default_checksum(
    s3_service, tmp_path, name
):
    storage, state = s3_service
    source = tmp_path / name
    if name.endswith(".tar"):
        content = b"owned archive payload"
        member = tarfile.TarInfo("payload.txt")
        member.size = len(content)
        with tarfile.open(source, "w") as tar:
            tar.addfile(member, io.BytesIO(content))
    else:
        source.write_bytes(b"synthetic sidecar" if name != "empty.key" else b"")
    assert await storage.upload(source, name) == (True, "")
    assert state.objects == {"/owned-bucket/backups/" + name: source.read_bytes()}
    assert state.checksums and all(state.checksums)
    restored = tmp_path / "restored"
    assert await storage.download(name, restored)
    assert restored.read_bytes() == source.read_bytes()
    assert_file_closed(source)


@pytest.mark.asyncio
async def test_small_s3_error_closes_file_and_preserves_result_contract(
    s3_service, tmp_path
):
    storage, state = s3_service
    source = tmp_path / "denied.key"
    source.write_bytes(b"synthetic sidecar")
    state.mode = "forbidden"
    success, error = await storage.upload(source, source.name)
    assert not success and "AccessDenied" in error
    assert not state.objects
    assert_file_closed(source)


@pytest.mark.asyncio
async def test_small_s3_cancellation_closes_file(s3_service, tmp_path):
    storage, state = s3_service
    source = tmp_path / "cancel.key"
    source.write_bytes(b"synthetic sidecar")
    state.mode = "hold"
    task = asyncio.create_task(storage.upload(source, source.name))
    entered = asyncio.create_task(state.entered.wait())
    try:
        await asyncio.wait_for(
            asyncio.wait([task, entered], return_when=asyncio.FIRST_COMPLETED), 5
        )
        assert not task.done(), (
            "Upload must reach the actual HTTP service before cancellation"
        )
        task.cancel()
        done, _ = await asyncio.wait([task], timeout=0.05)
        assert not done, (
            "A single PUT cannot be aborted after the server accepted its bytes"
        )
        state.release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert_file_closed(source)
    finally:
        state.release.set()
        for pending in (task, entered):
            pending.cancel()
        await asyncio.gather(task, entered, return_exceptions=True)


@pytest.mark.asyncio
async def test_large_s3_multipart_actual_http_control(
    s3_service, tmp_path, monkeypatch
):
    from app.config import settings

    storage, state = s3_service
    monkeypatch.setattr(settings, "S3_MULTIPART_THRESHOLD_BYTES", 5 * 1024 * 1024)
    monkeypatch.setattr(settings, "S3_MULTIPART_CHUNK_SIZE", 5 * 1024 * 1024)
    source = tmp_path / "large.tar"
    source.write_bytes(b"x" * (5 * 1024 * 1024) + b"final part")
    assert await storage.upload(source, source.name) == (True, "")
    assert state.objects["/owned-bucket/backups/large.tar"] == source.read_bytes()
    assert not state.uploads
    assert len(state.checksums) == 2 and all(state.checksums)
    assert_file_closed(source)
