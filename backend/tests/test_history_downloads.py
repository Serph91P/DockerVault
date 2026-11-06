"""Whole archive adapter reuses canonical preparation and owns temporary resources."""

from pathlib import Path
from types import SimpleNamespace
import asyncio
import io
import tarfile

import pytest
from app.api import backups
from app.database import Backup, BackupTarget, BackupStatus
from app.encryption import generate_key_pair, encrypt_backup


@pytest.mark.asyncio
async def test_encrypted_whole_export_preserves_ciphertext_and_dek_sidecar(
    async_client, db_session, tmp_path
):
    archive = tmp_path / "private.tar"
    with tarfile.open(archive, "w") as tar:
        info = tarfile.TarInfo("nested/value")
        info.size = 4
        tar.addfile(info, io.BytesIO(b"data"))
    keys = await generate_key_pair()
    encrypted = await encrypt_backup(archive, keys.public_key)
    target = BackupTarget(
        name="Owned export", target_type="path", host_path=str(tmp_path)
    )
    db_session.add(target)
    await db_session.flush()
    row = Backup(
        target_id=target.id,
        status=BackupStatus.COMPLETED,
        encrypted=True,
        file_path=str(encrypted.encrypted_path),
        encryption_key_path=str(encrypted.key_path),
    )
    db_session.add(row)
    await db_session.commit()
    ciphertext = await async_client.get(f"/api/v1/backups/{row.id}/download")
    sidecar = await async_client.get(f"/api/v1/backups/{row.id}/download?sidecar=true")
    assert ciphertext.status_code == sidecar.status_code == 200
    assert ciphertext.content == Path(encrypted.encrypted_path).read_bytes()
    assert sidecar.content == Path(encrypted.key_path).read_bytes()
    assert b"AGE-SECRET" not in ciphertext.content + sidecar.content
    Path(encrypted.key_path).unlink()
    assert (
        await async_client.get(f"/api/v1/backups/{row.id}/download?sidecar=true")
    ).status_code == 404
    row.status = BackupStatus.FAILED
    await db_session.commit()
    assert (
        await async_client.get(f"/api/v1/backups/{row.id}/download")
    ).status_code == 409


@pytest.mark.asyncio
async def test_whole_download_owns_temp_until_failed_send_finishes(
    tmp_path, monkeypatch
):
    temp = tmp_path / "remote"
    temp.mkdir()
    archive = temp / "value.tar"
    archive.write_bytes(b"owned archive")

    async def prepare(backup):
        return str(archive), temp

    monkeypatch.setattr(backups, "_ensure_backup_locally", prepare)
    semaphore = asyncio.Semaphore(1)
    monkeypatch.setattr(backups, "_archive_scan_semaphore", semaphore)

    async def send(message):
        assert temp.exists()
        if message["type"] == "http.response.body":
            raise OSError("explicit failed transport")

    async def receive():
        await asyncio.Event().wait()  # An ASGI receiver waits; it must not busy-loop.
        return {"type": "http.disconnect"}

    with pytest.raises(OSError, match="explicit failed transport"):
        await backups._WholeArchiveResponse(SimpleNamespace())(
            {"type": "http", "method": "GET", "headers": []}, receive, send
        )
    assert not temp.exists()
    assert not semaphore.locked()
