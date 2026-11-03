"""Cancellation must settle real provider effects; exact local sidecars beat decoys."""

import asyncio
from pathlib import Path
import shutil
import threading

from fastapi import HTTPException
import pytest
from sqlalchemy import select

from app.backup_engine import backup_engine
from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
)
from app.encryption import DecryptionError
from app.remote_storage import storage_manager
from tests.test_remote_object_ownership import test_engine  # noqa: F401


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "directory,stem",
    [("vault", "archive"), ("vault.enc", "archive"), ("vault", "archive.enc")],
)
@pytest.mark.parametrize("legacy", [False, True])
async def test_exact_local_key_lookup_never_prefers_foreign_decoy(
    tmp_path, monkeypatch, directory, stem, legacy
):
    from app.api import backups

    original = tmp_path / (directory + (":old" if legacy else "")) / (stem + ".tar.enc")
    resolved = Path(str(original).replace(":", "_")) if legacy else original
    resolved.parent.mkdir(parents=True)
    resolved.write_bytes(b"owned ciphertext")
    correct = original.with_suffix(".key")
    correct.parent.mkdir(parents=True, exist_ok=True)
    correct.write_bytes(b"owned sidecar")
    decoy = Path(str(original).replace(".enc", ".key"))
    if decoy != correct:
        decoy.parent.mkdir(parents=True, exist_ok=True)
        decoy.write_bytes(b"foreign sidecar must remain")
    selected = []

    async def decrypt(*, encrypted_path, key_path, private_key, output_path):
        selected.append(key_path)
        if key_path != correct:
            raise DecryptionError("wrong backup sidecar selected")
        output_path.write_bytes(b"owned plaintext boundary stub")

    monkeypatch.setattr(backups, "decrypt_backup", decrypt)
    record = Backup(
        id=161,
        target_id=1,
        file_path=str(original),
        encrypted=True,
        encryption_key_path=None,
    )
    result = None
    try:
        try:
            result = await backups._get_decrypted_archive_path(
                record, "synthetic non-secret key boundary"
            )
        except HTTPException:
            pass
        assert selected == [correct], (
            "A foreign decoy must never precede the exact existing sidecar"
        )
        assert (
            result and Path(result[0]).read_bytes() == b"owned plaintext boundary stub"
        )
        if decoy != correct:
            assert decoy.read_bytes() == b"foreign sidecar must remain"
    finally:
        if result and result[2]:
            result[2].unlink(missing_ok=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("phase", ["upload", "verification"])
async def test_cancelled_real_copy_cannot_overwrite_later_acknowledged_identity(
    async_client, test_db, tmp_path, monkeypatch, phase
):
    from unittest.mock import patch

    paths = []
    for name, payload in [
        ("a", b"first cancelled identity"),
        ("b", b"second completed identity"),
    ]:
        path = tmp_path / name / "snapshot.tar"
        path.parent.mkdir()
        path.write_bytes(payload)
        paths.append(path)
    remote = tmp_path / "remote"
    async with test_db() as session:
        targets = [
            BackupTarget(name=name, target_type="path", host_path=str(path.parent))
            for name, path in zip(("reports?", "reports*"), paths)
        ]
        storage = RemoteStorage(
            name="owned", storage_type="local", base_path=str(remote)
        )
        session.add_all([*targets, storage])
        await session.flush()
        records = [
            Backup(
                target_id=target.id, status=BackupStatus.COMPLETED, file_path=str(path)
            )
            for target, path in zip(targets, paths)
        ]
        session.add_all(records)
        await session.commit()
        ids, storage_id = [record.id for record in records], storage.id
    entered, settled, contention = asyncio.Event(), asyncio.Event(), asyncio.Event()
    release = threading.Event()
    loop = asyncio.get_running_loop()
    original_copy = shutil.copy2
    held_source = (
        paths[0]
        if phase == "upload"
        else remote
        / backup_engine._sanitize_path_name(targets[0].name)
        / "snapshot.tar"
    )
    verification_destinations = []

    def copy(source, destination, *args, **kwargs):
        if Path(source) == held_source:
            if phase == "verification":
                verification_destinations.append(Path(destination))
            loop.call_soon_threadsafe(entered.set)
            if not release.wait(15):
                raise TimeoutError("owned copy fixture was not released")
            try:
                return original_copy(source, destination, *args, **kwargs)
            finally:
                loop.call_soon_threadsafe(settled.set)
        return original_copy(source, destination, *args, **kwargs)

    class ObservedLock(asyncio.Lock):
        async def acquire(self):
            if self.locked():
                contention.set()
            return await super().acquire()

    class Registry(dict):
        def setdefault(self, key, default=None):
            return super().setdefault(key, ObservedLock())

    monkeypatch.setattr(backup_engine, "_remote_sync_locks", Registry())
    monkeypatch.setattr("app.remote_storage.shutil.copy2", copy)
    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
        patch("app.config.settings.BACKUP_BASE_PATH", str(tmp_path)),
    ):
        first = asyncio.create_task(
            async_client.post(f"/api/v1/storage/{storage_id}/sync/{ids[0]}")
        )
        second = observer = None
        try:
            await asyncio.wait_for(entered.wait(), 10)
            first.cancel()
            await asyncio.sleep(
                0
            )  # Deliver cancellation while the actual executor is blocked.
            second = asyncio.create_task(
                async_client.post(f"/api/v1/storage/{storage_id}/sync/{ids[1]}")
            )
            observer = asyncio.create_task(contention.wait())
            done, _ = await asyncio.wait(
                (second, observer), timeout=10, return_when=asyncio.FIRST_COMPLETED
            )
            assert done
            first.cancel()  # Repeated cancellation must not release the protected transfer.
            release.set()
            await asyncio.wait_for(settled.wait(), 10)
            with pytest.raises(asyncio.CancelledError):
                await first
            response = await asyncio.wait_for(second, 10)
            async with test_db() as session:
                rows = (
                    await session.scalars(
                        select(BackupStorageSync).where(
                            BackupStorageSync.sync_status == "completed"
                        )
                    )
                ).all()
            assert rows
            for row in rows:
                assert (remote / row.remote_path).read_bytes() == paths[
                    ids.index(row.backup_id)
                ].read_bytes(), (
                    "Cancelled executor overwrote an acknowledged different identity"
                )
            # Cancellation is not success: A may settle bytes but must not become completed.
            assert (
                response.status_code == 200
                and len(rows) == 1
                and rows[0].backup_id == ids[1]
            )
            assert all(
                not destination.parent.exists()
                for destination in verification_destinations
            )
        finally:
            release.set()
            await asyncio.wait_for(settled.wait(), 10)
            await asyncio.gather(
                *(task for task in (first, second) if task), return_exceptions=True
            )
            if observer:
                observer.cancel()
                await asyncio.gather(observer, return_exceptions=True)
