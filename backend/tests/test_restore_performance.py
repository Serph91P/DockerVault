"""Authenticating an immutable restore archive must not interrupt live sources."""

import io
import tarfile
from unittest.mock import AsyncMock

import pytest

from app.backup_engine import BackupEngine
from app.database import Backup, BackupStatus, BackupTarget
from app.encryption import encrypt_backup, generate_key_pair


@pytest.mark.asyncio
@pytest.mark.parametrize("valid_key", [True, False])
@pytest.mark.parametrize("compressed", [True, False])
async def test_restore_decrypts_before_stop_and_wrong_key_never_stops(
    async_client, test_db, tmp_path, monkeypatch, valid_key, compressed
):
    from app import backup_engine as module

    monkeypatch.setattr(module.settings, "BACKUP_BASE_PATH", str(tmp_path))
    source = tmp_path / ("archive.tar.gz" if compressed else "archive.tar")
    with tarfile.open(source, "w:gz" if compressed else "w") as tar:
        member = tarfile.TarInfo("payload")
        member.size = 4
        tar.addfile(member, io.BytesIO(b"data"))
    keys = await generate_key_pair()
    encrypted = await encrypt_backup(source, keys.public_key)
    source.write_bytes(b"legitimate existing plain archive must survive")
    destination = tmp_path / "restored"
    destination.mkdir()
    (destination / "payload").write_bytes(b"changed source")
    async with test_db() as session:
        target = BackupTarget(
            name="owned",
            target_type="path",
            host_path=str(destination),
            stop_container=True,
            container_name="owned",
        )
        session.add(target)
        await session.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(encrypted.encrypted_path),
            encrypted=True,
            encryption_key_path=str(encrypted.key_path),
            backup_metadata={
                "archive_layout": {"version": 1, "kind": "single", "root": ""}
            },
        )
        session.add(backup)
        await session.commit()
    state = {"value": "running"}
    events = []
    monkeypatch.setattr(
        module.docker_client,
        "get_container_identity",
        AsyncMock(return_value="owned-id"),
    )
    monkeypatch.setattr(
        module.docker_client,
        "get_container_state",
        AsyncMock(side_effect=lambda name: state["value"]),
    )

    async def stop(name):
        events.append("stop")
        state["value"] = "exited"
        return True

    async def start(name):
        events.append("start")
        state["value"] = "running"
        return True

    original = module.decrypt_backup

    async def decrypt(*args):
        events.append("decrypt:" + state["value"])
        return await original(*args)

    monkeypatch.setattr(module.docker_client, "stop_container", stop)
    monkeypatch.setattr(module.docker_client, "start_container", start)
    monkeypatch.setattr(module, "decrypt_backup", decrypt)
    result = await BackupEngine().restore_backup(
        backup.id,
        private_key=keys.private_key
        if valid_key
        else (await generate_key_pair()).private_key,
    )
    assert result is valid_key
    assert events[0] == "decrypt:running"
    assert events == (
        ["decrypt:running", "stop", "start"] if valid_key else ["decrypt:running"]
    )
    assert (
        source.exists()
        and source.read_bytes() == b"legitimate existing plain archive must survive"
    )
    assert not list(tmp_path.glob(".dockervault-restore-*.tmp"))
    assert (destination / "payload").read_bytes() == (
        b"data" if valid_key else b"changed source"
    )
