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
        listing_pages={},
        listing_requests=[],
        mode="normal",
        abort_mode="normal",
        created=asyncio.Event(),
        abort_entered=asyncio.Event(),
        abort_release=asyncio.Event(),
        entered=asyncio.Event(),
        release=asyncio.Event(),
    )

    async def service(request):
        key, query = request.path, request.query
        state.events.append((request.method, key, dict(query)))
        if request.method == "GET" and query.get("list-type") == "2":
            token = query.get("continuation-token")
            state.listing_requests.append((query.get("prefix"), token))
            page = state.listing_pages[token]
            if page.get("error"):
                return web.Response(
                    status=403,
                    text="<Error><Code>AccessDenied</Code><Message>Owned listing denied</Message></Error>",
                    content_type="application/xml",
                )
            xml = ET.Element(
                "ListBucketResult", xmlns="http://s3.amazonaws.com/doc/2006-03-01/"
            )
            ET.SubElement(xml, "Name").text = "owned-bucket"
            ET.SubElement(xml, "Prefix").text = query.get("prefix", "")
            ET.SubElement(xml, "IsTruncated").text = (
                "true" if page.get("truncated", bool(page.get("next"))) else "false"
            )
            if page.get("next"):
                ET.SubElement(xml, "NextContinuationToken").text = page["next"]
            for object_key in page.get("keys", []):
                item = ET.SubElement(xml, "Contents")
                ET.SubElement(item, "Key").text = object_key
                ET.SubElement(item, "Size").text = "17"
                ET.SubElement(item, "LastModified").text = "2026-01-01T00:00:00.000Z"
            return web.Response(body=ET.tostring(xml), content_type="application/xml")
        if request.method == "POST" and "uploads" in query:
            state.uploads["owned-upload"] = {}
            state.created.set()
            if state.mode == "create_hold":
                await state.release.wait()
            return web.Response(
                text="<InitiateMultipartUploadResult><UploadId>owned-upload</UploadId></InitiateMultipartUploadResult>",
                content_type="application/xml",
            )
        if request.method == "DELETE" and "uploadId" in query:
            state.abort_entered.set()
            if state.abort_mode == "hold":
                await state.abort_release.wait()
            if state.abort_mode == "failed":
                return web.Response(
                    status=403,
                    text="<Error><Code>AccessDenied</Code></Error>",
                    content_type="application/xml",
                )
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
            if state.mode == "part_failure" and "partNumber" in query:
                return web.Response(
                    status=400,
                    text="<Error><Code>SourceRejected</Code></Error>",
                    content_type="application/xml",
                )
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
                if query["uploadId"] not in state.uploads:
                    return web.Response(status=404)
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
        state.abort_release.set()
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


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "phase,abort",
    [
        ("create_hold", "normal"),
        ("hold", "normal"),
        ("hold", "failed"),
        ("hold", "hold"),
        ("hold", "repeated_cancel"),
    ],
)
async def test_multipart_cancellation_aborts_at_real_http_boundary(
    s3_service, tmp_path, monkeypatch, phase, abort
):
    from app.config import settings

    storage, state = s3_service
    monkeypatch.setattr(settings, "S3_MULTIPART_THRESHOLD_BYTES", 5 * 1024 * 1024)
    monkeypatch.setattr(S3Storage, "MULTIPART_CLEANUP_TIMEOUT", 0.2, raising=False)
    source = tmp_path / "cancel-large.tar"
    source.write_bytes(b"x" * (6 * 1024 * 1024))
    state.mode = phase
    state.abort_mode = "hold" if abort == "repeated_cancel" else abort
    task = asyncio.create_task(storage.upload(source, source.name))
    try:
        await asyncio.wait_for(
            (state.created if phase == "create_hold" else state.entered).wait(), 5
        )
        task.cancel()
        if phase == "create_hold":
            state.release.set()
        if abort == "repeated_cancel":
            await asyncio.wait_for(state.abort_entered.wait(), 2)
            task.cancel()
            state.abort_release.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 2)
        assert state.abort_entered.is_set(), (
            "Cancellation must send a real AbortMultipartUpload DELETE"
        )
        if abort in ("normal", "repeated_cancel"):
            assert not state.uploads
        else:
            assert state.uploads, (
                "Failed/hung provider abort is not a successful cleanup"
            )
        assert not state.objects
        assert_file_closed(source)
    finally:
        state.release.set()
        state.abort_release.set()
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("cache", ["active", "removed", "replaced"])
async def test_storage_shutdown_closes_real_sdk_client(s3_service, tmp_path, cache):
    from app.remote_storage import RemoteStorageManager

    storage, state = s3_service
    manager = RemoteStorageManager()
    manager.configs[storage.config.id] = storage.config
    manager.backends[storage.config.id] = storage
    source = tmp_path / "shutdown.key"
    source.write_bytes(b"owned")
    assert await storage.upload(source, source.name) == (True, "")
    assert storage._client is not None
    if cache == "removed":
        manager.remove_storage(storage.config.id)
    elif cache == "replaced":
        manager.add_storage(storage.config)
    await manager.close()
    assert storage._client is None
    await manager.close()


@pytest.mark.asyncio
async def test_multipart_abort_error_preserves_primary_upload_error(
    s3_service, tmp_path, monkeypatch
):
    from app.config import settings

    storage, state = s3_service
    monkeypatch.setattr(settings, "S3_MULTIPART_THRESHOLD_BYTES", 5 * 1024 * 1024)
    source = tmp_path / "failure.tar"
    source.write_bytes(b"x" * (6 * 1024 * 1024))
    state.mode, state.abort_mode = "part_failure", "failed"
    success, error = await storage.upload(source, source.name)
    assert not success and "SourceRejected" in error
    assert state.abort_entered.is_set()
    assert state.uploads and not state.objects
    assert_file_closed(source)


@pytest.mark.asyncio
async def test_cancellation_with_unreturned_upload_id_is_bounded(
    s3_service, tmp_path, monkeypatch, caplog
):
    from app.config import settings

    storage, state = s3_service
    monkeypatch.setattr(settings, "S3_MULTIPART_THRESHOLD_BYTES", 5 * 1024 * 1024)
    monkeypatch.setattr(S3Storage, "MULTIPART_CLEANUP_TIMEOUT", 0.05)
    source = tmp_path / "creation.tar"
    source.write_bytes(b"x" * (6 * 1024 * 1024))
    state.mode = "create_hold"
    task = asyncio.create_task(storage.upload(source, source.name))
    try:
        await asyncio.wait_for(state.created.wait(), 5)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 2)
        assert "provider lifecycle cleanup may be required" in caplog.text
        assert state.uploads and not state.abort_entered.is_set()
    finally:
        state.release.set()
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
