"""Real loopback HTTP contract: logical DAV paths must retain object identity."""

import asyncio
import io
import tarfile
from unittest.mock import AsyncMock
from xml.sax.saxutils import escape

from aiohttp import web
import pytest

from app.remote_storage import StorageConfig, StorageType, WebDAVStorage


@pytest.mark.asyncio
@pytest.mark.parametrize("base", ["/dav%20root", "/dav%2Froot?tenant=one%26two"])
@pytest.mark.parametrize(
    "directory,encoded",
    [
        ("project#1", "project%231"),
        ("project%2F1", "project%252F1"),
        ("project?1", "project%3F1"),
        ("space name", "space%20name"),
        ("编码", "%E7%BC%96%E7%A0%81"),
    ],
)
async def test_real_webdav_archive_identity(
    tmp_path, monkeypatch, base, directory, encoded
):
    from app.api import settings as api_settings
    from app.config import settings

    monkeypatch.setattr(api_settings, "get_setting", AsyncMock(return_value="false"))
    monkeypatch.setattr(settings, "STORAGE_VERIFY_AFTER_UPLOAD", True)
    objects, requests = {}, []

    async def service(request):
        key = request.raw_path.split("?", 1)[0]
        requests.append((request.method, key, request.raw_path.partition("?")[2]))
        if request.method == "MKCOL":
            return web.Response(status=201)
        if request.method == "PUT":
            objects[key] = await request.read()
            return web.Response(status=201)
        if request.method == "HEAD":
            return web.Response(
                status=200 if key in objects else 404,
                headers={"Content-Length": str(len(objects.get(key, b"")))},
            )
        if request.method == "GET":
            return (
                web.Response(body=objects[key])
                if key in objects
                else web.Response(status=404)
            )
        if request.method == "DELETE":
            objects.pop(key, None)
            return web.Response(status=204)
        if request.method == "PROPFIND":
            prefix = key.rstrip("/") + "/"
            entries = [(key, b"", True)] + [
                (k, v, False) for k, v in objects.items() if k.startswith(prefix)
            ]
            xml = (
                '<d:multistatus xmlns:d="DAV:">'
                + "".join(
                    f"<d:response><d:href>{escape(k)}</d:href><d:propstat><d:prop>"
                    f"<d:getcontentlength>{len(v)}</d:getcontentlength><d:resourcetype>"
                    + ("<d:collection/>" if collection else "")
                    + "</d:resourcetype></d:prop></d:propstat></d:response>"
                    for k, v, collection in entries
                )
                + "</d:multistatus>"
            )
            return web.Response(status=207, text=xml, content_type="application/xml")
        raise AssertionError(request.method)

    app = web.Application()
    app.router.add_route("*", "/{path:.*}", service)
    runner = web.AppRunner(app, access_log=None)
    await runner.setup()
    assert runner.server is not None
    server = await asyncio.get_running_loop().create_server(
        runner.server, "127.0.0.1", 0
    )
    port = server.sockets[0].getsockname()[1]
    storage = WebDAVStorage(
        StorageConfig(
            id=1,
            name="owned-loopback",
            storage_type=StorageType.WEBDAV,
            webdav_url=f"http://127.0.0.1:{port}{base}",
            base_path="/backups #?%",
        )
    )
    root = base.split("?", 1)[0] + "/backups%20%23%3F%25/" + encoded
    names = [
        ("first#?%.tar", "first%23%3F%25.tar"),
        ("second 编 1.tar", "second%20%E7%BC%96%201.tar"),
    ]
    try:
        for index, (name, wire_name) in enumerate(names):
            source = tmp_path / f"source-{index}.tar"
            content = f"archive payload {index}".encode()
            member = tarfile.TarInfo("payload.txt")
            member.size = len(content)
            with tarfile.open(source, "w") as tar:
                tar.addfile(member, io.BytesIO(content))
            assert await storage.upload(source, f"{directory}/{name}") == (True, "")
        assert set(objects) == {root + "/" + wire for _, wire in names}
        assert any(method == "MKCOL" and key == root for method, key, _ in requests)
        listing = await storage.list_files(directory)
        assert {item["name"] for item in listing} == {name for name, _ in names}
        for index, (name, wire) in enumerate(names):
            restored = tmp_path / f"restored-{index}.tar"
            assert await storage.download(f"{directory}/{name}", restored)
            assert (
                restored.read_bytes() == (tmp_path / f"source-{index}.tar").read_bytes()
            )
            assert any(
                method == "HEAD" and key == root + "/" + wire
                for method, key, _ in requests
            )
        assert await storage.delete(f"{directory}/{names[0][0]}")
        assert set(objects) == {root + "/" + names[1][1]}
        if "?" in base:
            assert all(query == "tenant=one%26two" for _, _, query in requests)
    finally:
        server.close()
        await server.wait_closed()
        await runner.cleanup()
