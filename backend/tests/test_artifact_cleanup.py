"""Orphan collection must never unlink a live backup's owned artifacts."""

import asyncio
from pathlib import Path
import threading
from types import SimpleNamespace
from unittest.mock import patch

import pytest

from app.backup_engine import BackupEngine
from app.database import (
    Backup,
    BackupStatus,
    BackupTarget,
    EncryptionConfig,
    RemoteStorage,
)
from app.retention import RetentionManager


@pytest.mark.asyncio
@pytest.mark.parametrize("phase", ["archive", "checksum", "encryption", "upload"])
async def test_cleanup_preserves_active_artifacts_but_deletes_real_orphan(
    test_db, tmp_path, phase
):
    source = tmp_path / "source"
    source.mkdir()
    (source / "payload").write_bytes(b"recoverable synthetic payload")
    backups = tmp_path / "backups"
    async with test_db() as session:
        target = BackupTarget(
            name="active",
            target_type="path",
            host_path=str(source),
            compression_enabled=True,
            stop_container=False,
            dependencies=[],
            remote_storage_ids=[7] if phase == "upload" else [],
        )
        session.add(target)
        await session.flush()
        backup = Backup(target_id=target.id)
        session.add(backup)
        if phase in ("encryption", "upload"):
            session.add(
                EncryptionConfig(
                    public_key="synthetic-stage-boundary",
                    encryption_enabled=True,
                    setup_completed=True,
                )
            )
        if phase == "upload":
            session.add(
                RemoteStorage(
                    id=7,
                    name="owned-local",
                    storage_type="local",
                    base_path=str(tmp_path / "remote"),
                    enabled=True,
                )
            )
        await session.commit()
        backup_id = backup.id
    engine = BackupEngine()
    entered = threading.Event()
    released = threading.Event()
    original_archive = engine._create_tar
    original_checksum = engine._calculate_checksum
    original_upload = engine._sync_to_remote

    def archive(*args, **kwargs):
        original_archive(*args, **kwargs)
        if phase == "archive":
            entered.set()
            if not released.wait(20):
                raise RuntimeError("test archive release missing")

    async def pause(selected):
        if phase == selected:
            entered.set()
            assert await asyncio.to_thread(released.wait, 20)

    async def checksum(path):
        await pause("checksum")
        return await original_checksum(path)

    async def encrypt(path, _public_key):
        # Controlled crypto boundary: real files and rename, not a crypto claim.
        encrypted = Path(str(path) + ".enc")
        temporary = Path(str(encrypted) + ".tmp")
        key = Path(str(path) + ".key")
        temporary.write_bytes(path.read_bytes())
        key.write_bytes(b"synthetic wrapped key")
        await pause("encryption")
        temporary.replace(encrypted)
        path.unlink()
        return SimpleNamespace(
            encrypted_path=encrypted,
            key_path=key,
            dek_encrypted=b"synthetic wrapped key",
        )

    async def upload(*args):
        await pause("upload")
        return await original_upload(*args)

    from app.remote_storage import storage_manager

    with (
        patch("app.backup_engine.async_session", test_db),
        patch("app.retention.async_session", test_db),
        patch("app.backup_engine.settings.BACKUP_BASE_PATH", str(backups)),
        patch.object(engine, "_sudo_available", return_value=False),
        patch.object(engine, "_create_tar", archive),
        patch.object(engine, "_calculate_checksum", checksum),
        patch.object(engine, "_sync_to_remote", upload),
        patch("app.backup_engine.encrypt_backup", encrypt),
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
    ):
        task = asyncio.create_task(engine.run_backup(backup_id, skip_validation=True))
        try:
            assert await asyncio.to_thread(entered.wait, 20)
            active_files = list(backups.rglob("*.tar*"))
            assert active_files
            orphan = backups / "active" / "orphan.tar.gz"
            orphan.write_bytes(b"orphan")
            result = await RetentionManager().cleanup_orphaned_files()
            assert result["deleted"] == 1
            assert not orphan.exists()
            assert all(path.exists() for path in active_files)
        finally:
            released.set()
            successful = await task
        assert successful is True
    async with test_db() as session:
        completed = await session.get(Backup, backup_id)
        assert completed.status == BackupStatus.COMPLETED
        assert Path(completed.file_path).is_file()
        if phase in ("encryption", "upload"):
            assert Path(completed.encryption_key_path).is_file()


@pytest.mark.asyncio
async def test_cleanup_rechecks_completion_after_stale_inventory(test_db, tmp_path):
    path = tmp_path / "completed.tar.gz"
    path.write_bytes(b"completed owned archive")
    async with test_db() as session:
        target = BackupTarget(name="completed", target_type="path")
        session.add(target)
        await session.flush()
        backup = Backup(target_id=target.id, status=BackupStatus.RUNNING)
        session.add(backup)
        await session.commit()
        backup_id = backup.id

    # Pause after a real DB snapshot while another task completes the backup.
    inventoried = asyncio.Event()
    release = asyncio.Event()

    async def complete_after_inventory():
        await inventoried.wait()
        async with test_db() as session:
            row = await session.get(Backup, backup_id)
            row.status = BackupStatus.COMPLETED
            row.file_path = str(path)
            await session.commit()
        release.set()

    manager = RetentionManager()
    # The DB lookup wrapper returns real rows; only scheduling is controlled.
    from sqlalchemy.ext.asyncio import AsyncSession

    original_execute = AsyncSession.execute
    initial = True

    async def snapshot_then_complete(session, *args, **kwargs):
        nonlocal initial
        result = await original_execute(session, *args, **kwargs)
        if initial:
            initial = False
            inventoried.set()
            await release.wait()
        return result

    with (
        patch("app.retention.async_session", test_db),
        patch("app.retention.settings.BACKUP_BASE_PATH", str(tmp_path)),
        patch.object(AsyncSession, "execute", snapshot_then_complete),
    ):
        completing = asyncio.create_task(complete_after_inventory())
        result = await manager.cleanup_orphaned_files()
        await completing
    assert result["deleted"] == 0
    assert path.read_bytes() == b"completed owned archive"
