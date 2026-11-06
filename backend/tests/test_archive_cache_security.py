"""Cache hits retain real key/archive authentication and bounded-prefix behavior."""

import io
import tarfile
from pathlib import Path

import pytest

from app.api import backups
from app.database import Backup, BackupStatus, BackupTarget
from app.encryption import encrypt_backup, generate_key_pair


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["wrong-key", "tamper", "missing-sidecar"])
async def test_encrypted_cache_hit_still_checks_real_key_and_archive(
    async_client, test_db, tmp_path, monkeypatch, change
):
    source = tmp_path / "archive.tar"
    with tarfile.open(source, "w") as tar:
        member = tarfile.TarInfo("owned/value")
        member.size = 4
        tar.addfile(member, io.BytesIO(b"data"))
    keys = await generate_key_pair()
    encrypted = await encrypt_backup(source, keys.public_key)
    async with test_db() as session:
        target = BackupTarget(
            name="owned", target_type="path", host_path=str(tmp_path / "source")
        )
        session.add(target)
        await session.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(encrypted.encrypted_path),
            encrypted=True,
            encryption_key_path=str(encrypted.key_path),
        )
        session.add(backup)
        await session.commit()
    decrypted_paths = []
    original = backups._get_decrypted_archive_path

    async def tracked(*args):
        result = await original(*args)
        decrypted_paths.append(result[2])
        return result

    monkeypatch.setattr(backups, "_get_decrypted_archive_path", tracked)
    endpoint = f"/api/v1/backups/{backup.id}/files"
    first = await async_client.post(endpoint, json={"private_key": keys.private_key})
    assert first.status_code == 200
    assert any(entry[1] for entry in backups._archive_listing_cache.values())
    private_key = keys.private_key
    if change == "wrong-key":
        private_key = (await generate_key_pair()).private_key
    elif change == "tamper":
        path = Path(encrypted.encrypted_path)
        data = bytearray(path.read_bytes())
        data[-40] ^= 1
        path.write_bytes(data)
    else:
        Path(encrypted.key_path).unlink()
    failed = await async_client.post(endpoint, json={"private_key": private_key})
    assert failed.status_code == (404 if change == "missing-sidecar" else 400)
    assert all(not path.exists() for path in decrypted_paths)


@pytest.mark.asyncio
async def test_bounded_index_prefix_fallback_keeps_correct_pages(tmp_path, monkeypatch):
    source = tmp_path / "archive.tar"
    with tarfile.open(source, "w") as tar:
        for index in range(20):
            tar.addfile(tarfile.TarInfo(f"owned/{index:02d}"))
    monkeypatch.setattr(backups, "_ARCHIVE_CACHE_MEMBERS", 3)
    with backups._archive_cache_lock:
        backups._archive_listing_cache.clear()
    first, total = await backups._list_archive_entries(str(source), "r:", 2, 0)
    middle, middle_total = await backups._list_archive_entries(str(source), "r:", 5, 10)
    last, last_total = await backups._list_archive_entries(str(source), "r:", 5, 18)
    assert total == middle_total == last_total == 20
    assert [entry.path for entry in middle] == [
        f"owned/{index:02d}" for index in range(10, 15)
    ]
    assert [entry.path for entry in last] == ["owned/18", "owned/19"]
    assert len(first) == 2
