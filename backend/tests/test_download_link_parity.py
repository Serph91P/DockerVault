"""Both download endpoints preserve native archive-internal link resolution."""

import asyncio
import io
import tarfile
import threading
from pathlib import Path

import pytest

from app.api import backups
from app.database import Backup
from app.encryption import encrypt_backup, generate_key_pair
from tests.test_download_performance import catalog


# Each entry is (name, bytes) or (name, link type, target).
@pytest.mark.parametrize("encrypted", [False, True], ids=["plain", "encrypted"])
@pytest.mark.parametrize("compressed", [False, True], ids=["tar", "gzip"])
@pytest.mark.parametrize(
    "entries,requested,expected",
    [
        (
            [("./owned/value", b"data"), ("owned/alias", "s", "value")],
            "owned/alias",
            b"data",
        ),
        (
            [("./owned/value", b"data"), ("owned/alias", "h", "./owned/value")],
            "owned/alias",
            b"data",
        ),
        (
            [
                ("owned/value", b"old!"),
                ("./owned/value", b"new!"),
                ("owned/alias", "s", "value"),
            ],
            "owned/alias",
            b"new!",
        ),
        (
            [
                ("owned/value", b"old!"),
                ("./owned/value", b"new!"),
                ("owned/alias", "h", "./owned/value"),
            ],
            "owned/alias",
            b"new!",
        ),
        (
            [("owned/value", b"data"), ("owned/alias", "s", "/value")],
            "owned/alias",
            b"data",
        ),
        (
            [
                ("owned/value", b"data"),
                ("/value", b"wrong"),
                ("owned/alias", "s", "/value"),
            ],
            "owned/alias",
            b"data",
        ),
        (
            [
                ("./owned/value", b"data"),
                ("owned/middle", "h", "./owned/value"),
                ("owned/alias", "s", "./middle"),
            ],
            "owned/alias",
            b"data",
        ),
        (
            [
                ("owned/value", b"old!"),
                ("owned/alias", "h", "./owned/value"),
                ("./owned/value", b"late"),
            ],
            "owned/alias",
            b"old!",
        ),
        ([("./owned/value", b"data")], "owned/value", None),
        (
            [("owned/alias", "h", "./owned/value"), ("./owned/value", b"late")],
            "owned/alias",
            None,
        ),
        ([("owned/alias", "s", "missing")], "owned/alias", None),
        (
            [("owned/alias", "s", "middle"), ("owned/middle", "s", "alias")],
            "owned/alias",
            None,
        ),
    ],
    ids=[
        "dot-symlink",
        "dot-hardlink",
        "duplicate-symlink",
        "duplicate-hardlink",
        "absolute-link",
        "absolute-link-shadow",
        "mixed-chain",
        "hardlink-offset",
        "initial-exact",
        "forward-hardlink",
        "missing-link",
        "cyclic-link",
    ],
)
@pytest.mark.asyncio
async def test_download_link_lookup_preserves_archived_bytes(
    async_client,
    test_db,
    tmp_path,
    monkeypatch,
    encrypted,
    compressed,
    entries,
    requested,
    expected,
):
    identifier, archive = await catalog(test_db, tmp_path)
    if compressed:
        archive = archive.with_suffix(".tar.gz")
    with tarfile.open(archive, "w:gz" if compressed else "w:") as tar:
        for entry in entries:
            member = tarfile.TarInfo(entry[0])
            if len(entry) == 2:
                member.size = len(entry[1])
                tar.addfile(member, io.BytesIO(entry[1]))
            else:
                member.type = tarfile.SYMTYPE if entry[1] == "s" else tarfile.LNKTYPE
                member.linkname = entry[2]
                tar.addfile(member)
    # Independently hand-derived bytes agree with the baseline stdlib read path.
    with tarfile.open(archive, "r:*") as tar:
        if expected is None:
            with pytest.raises((KeyError, RecursionError)):
                tar.extractfile(tar.getmember(requested))
        else:
            with tar.extractfile(tar.getmember(requested)) as stream:
                assert stream.read() == expected

    private_key = None
    async with test_db() as session:
        row = await session.get(Backup, identifier)
        row.file_path = str(archive)
        if encrypted:
            keys = await generate_key_pair()
            output = await encrypt_backup(archive, keys.public_key)
            private_key = keys.private_key
            row.file_path = str(output.encrypted_path)
            row.encrypted = True
            row.encryption_key_path = str(output.key_path)
        await session.commit()
    opened = []
    plaintexts = []
    original_open = tarfile.open
    original_decrypt = backups._get_decrypted_archive_path

    def tracked_open(*args, **kwargs):
        assert threading.current_thread() is not threading.main_thread()
        assert kwargs["stream"] is True
        tar = original_open(*args, **kwargs)
        opened.append(tar)
        return tar

    async def tracked_decrypt(*args, **kwargs):
        result = await original_decrypt(*args, **kwargs)
        plaintexts.append(result[2])
        return result

    monkeypatch.setattr(tarfile, "open", tracked_open)
    monkeypatch.setattr(backups, "_get_decrypted_archive_path", tracked_decrypt)
    url = f"/api/v1/backups/{identifier}/files/{requested}"
    response = (
        await async_client.post(url, json={"private_key": private_key})
        if encrypted
        else await async_client.get(url)
    )
    assert response.status_code == (404 if expected is None else 200)
    if expected is not None:
        assert response.content == expected
    assert opened and all(tar.closed and not tar.members for tar in opened)
    assert len(plaintexts) == int(encrypted)
    assert all(not path.exists() for path in plaintexts)


@pytest.mark.parametrize("encrypted", [False, True], ids=["plain", "encrypted"])
@pytest.mark.parametrize("stage", ["prepare", "response-start"])
@pytest.mark.asyncio
async def test_link_download_cancel_settles_before_archive_and_plaintext_cleanup(
    async_client, test_db, tmp_path, monkeypatch, encrypted, stage
):
    identifier, archive = await catalog(test_db, tmp_path)
    with tarfile.open(archive, "w:") as tar:
        member = tarfile.TarInfo("./owned/value")
        member.size = 4
        tar.addfile(member, io.BytesIO(b"data"))
        alias = tarfile.TarInfo("owned/alias")
        alias.type, alias.linkname = tarfile.SYMTYPE, "value"
        tar.addfile(alias)
    backup = Backup(file_path=str(archive))
    private_key = None
    if encrypted:
        keys = await generate_key_pair()
        output = await encrypt_backup(archive, keys.public_key)
        backup.file_path = str(output.encrypted_path)
        backup.encryption_key_path = str(output.key_path)
        private_key = keys.private_key

    entered, release = threading.Event(), threading.Event()
    original = backups._open_download_member
    owned = {}

    def held_open(path, mode, name, resources):
        filename = original(path, mode, name, resources)
        owned.update(resources)
        owned["path"] = path
        entered.set()
        if stage == "prepare":
            assert release.wait(5)
        return filename

    monkeypatch.setattr(backups, "_open_download_member", held_open)
    monkeypatch.setattr(backups, "_archive_scan_semaphore", asyncio.Semaphore(1))
    response = backups._ArchiveFileResponse(backup, "owned/alias", private_key)

    async def receive():
        await asyncio.sleep(60)

    async def send(message):
        if message["type"] == "http.response.start":
            raise asyncio.CancelledError

    operation = asyncio.create_task(
        response({"type": "http", "asgi": {"spec_version": "2.4"}}, receive, send)
    )
    try:
        if stage == "prepare":
            async with asyncio.timeout(5):
                while not entered.is_set():
                    await asyncio.sleep(0.001)
            operation.cancel()
            done, _ = await asyncio.wait([operation], timeout=0.05)
            assert not done
            assert backups._archive_scan_semaphore.locked()
            assert not owned["tar"].closed and not owned["file"].closed
            release.set()
        with pytest.raises(asyncio.CancelledError):
            await operation
        assert entered.is_set()
        assert owned["tar"].closed and owned["file"].closed
        assert not backups._archive_scan_semaphore.locked()
        assert response.private_key is None
        if encrypted:
            assert not Path(owned["path"]).exists()
        assert Path(backup.file_path).exists()
    finally:
        release.set()
        await asyncio.gather(operation, return_exceptions=True)
